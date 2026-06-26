use std::collections::HashMap;
use std::error::Error;
#[cfg(feature = "parallel_components")]
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock};
#[cfg(not(feature = "parallel_components"))]
use std::sync::{Arc, OnceLock, RwLock};
use bitvec::order::Lsb0;
use bitvec::vec::BitVec;
use crate::field::{Field, FieldOperations, FieldOps};

#[derive(Debug, Clone, PartialEq)]
pub struct InputInfo {
    pub name: String,
    pub offset: usize,
    pub lengths: Vec<usize>,
    pub type_id: Option<String>,
}

pub trait InputInfoSliceExt {
    fn get_total_size(&self, types: &[Type]) -> Result<usize, RuntimeError>;
    fn min_offset(&self) -> Option<usize>;
}

fn checked_product(values: &[usize]) -> Result<usize, RuntimeError> {
    values.iter().try_fold(1usize, |acc, value| {
        acc.checked_mul(*value)
            .ok_or(RuntimeError::OperationOverflows)
    })
}

impl InputInfoSliceExt for [InputInfo] {
    fn get_total_size(&self, types: &[Type]) -> Result<usize, RuntimeError> {
        let mut total_size = 0usize;
        for i in self {
            let base_type_size: usize = match &i.type_id {
                None => 1,
                Some(type_id) => {
                    match types.iter().find(|x| &x.name == type_id) {
                        None => { return Err(RuntimeError::UnknownTypeName(type_id.clone())); }
                        Some(t) => t.get_total_size()?
                    }
                }
            };

            let length_product = checked_product(&i.lengths)?;
            let input_size = length_product
                .checked_mul(base_type_size)
                .ok_or(RuntimeError::OperationOverflows)?;
            total_size = total_size
                .checked_add(input_size)
                .ok_or(RuntimeError::OperationOverflows)?;
        }
        Ok(total_size)
    }

    fn min_offset(&self) -> Option<usize> {
        self.iter().map(|i| i.offset).min()
    }
}

#[repr(u8)]
#[derive(Debug)]
pub enum OpCode {
    NoOp                 = 0,
    // Put signals to the stack
    // required stack_i64: signal index
    LoadSignal           = 1,
    // Store the signal
    // stack_ff contains the value to store
    // stack_i64 contains the signal index
    StoreSignal          = 2,
    PushI64              = 3, // Push i64 value to the stack
    PushFf               = 4, // Push ff value to the stack
    // Set FF variable from the stack
    // arguments: offset from the base pointer
    // stack_ff:  value to store
    StoreVariableFf      = 5,
    LoadVariableFf       = 6,
    // Set I64 variable from the stack
    // arguments: offset from the base pointer
    // stack_i64:  value to store
    StoreVariableI64     = 7,
    LoadVariableI64      = 8,
    // Jump to the instruction if there is 0 on stack_ff
    // arguments: 4 byte LE offset to jump
    // stack_ff:  the value to check for failure
    JumpIfFalseFf        = 9,
    // Jump to the instruction if there is 0 on stack_i64
    // arguments: 4 byte LE offset to jump
    // stack_i64: the value to check for failure
    JumpIfFalseI64       = 10,
    // Jump to the instruction
    // arguments:      4 byte LE offset to jump
    Jump                 = 11,
    // stack_i64 contains the error code
    Error                = 12,
    // Get the component signal and put it to the stack_ff
    // stack_i64:0 contains the signal index
    // stack_i64:-1 contains the component index
    LoadCmpSignal        = 13,
    // Store the component signal and run
    // stack_ff contains the value to store
    // stack_i64:0 contains the signal index
    // stack_i64:-1 contains the component index
    StoreCmpSignalAndRun   = 14,
    StoreCmpSignalCntCheck = 15,
    // Store the component input without decrementing input counter
    // stack_ff contains the value to store
    // stack_i64:0 contains the signal index
    // stack_i64:-1 contains the component index
    StoreCmpInput        = 16,
    OpMul                = 17,
    OpAdd                = 18,
    OpNeq                = 19,
    OpDiv                = 20,
    OpSub                = 21,
    OpEq                 = 22,
    OpEqz                = 23,
    OpI64Add             = 24,
    OpI64Sub             = 25,
    // Memory return operation
    // Copy data from source memory to destination memory
    // stack_i64:0 contains the size (number of elements)
    // stack_i64:-1 contains the source address
    // stack_i64:-2 contains the destination address
    FfMReturn            = 26,
    // Function call operation
    // arguments: 4-byte function index + 1-byte argument count
    // Then for each argument:
    //   1-byte argument type:
    //     0 = i64 literal
    //     1 = ff literal
    //     4-7 = ff.memory (bit flags: bit 0 = addr is variable, bit 1 = size is variable)
    //     8-11 = i64.memory (bit flags: bit 0 = addr is variable, bit 1 = size is variable)
    //   For literals: value bytes (8 for i64, T::BYTES for ff)
    //   For memory: 2 i64 values (either literal values or variable indices based on type flags)
    FfMCall              = 27,
    // Memory store operation (ff.store)
    // stack_ff:0 contains the value to store
    // stack_i64:0 contains the memory address
    FfStore              = 28,
    FfMStore             = 56,
    FfMStoreFromSignal = 61,
    // Memory load operation (ff.load)
    // stack_i64:0 contains the memory address
    // Result pushed to stack_ff
    FfLoad               = 29,
    // Memory load operation (i64.load)
    // stack_i64:0 contains the memory address
    // Result pushed to stack_i64
    I64Load              = 30,
    // Field less-than comparison (ff.lt)
    // stack_ff:0 contains right operand
    // stack_ff:-1 contains left operand
    // Result pushed to stack_ff (1 if lhs < rhs, 0 otherwise)
    OpLt                 = 31,
    // Field greater-than comparison (ff.gt)
    // stack_ff:0 contains right operand
    // stack_ff:-1 contains left operand
    // Result pushed to stack_ff (1 if lhs > rhs, 0 otherwise)
    OpGt                 = 32,
    // Integer multiplication (i64.mul)
    // stack_i64:0 contains right operand
    // stack_i64:-1 contains left operand
    // Result pushed to stack_i64
    OpI64Mul             = 33,
    // Integer less-than-or-equal comparison (i64.le)
    // stack_i64:0 contains right operand
    // stack_i64:-1 contains left operand
    // Result pushed to stack_i64 (1 if lhs <= rhs, 0 otherwise)
    OpI64Lte             = 34,
    // Integer less-than comparison (i64.lt)
    // stack_i64:0 contains right operand
    // stack_i64:-1 contains left operand
    // Result pushed to stack_i64 (1 if lhs < rhs, 0 otherwise)
    OpI64Lt = 70,
    // Wrap field element to i64 (i64.wrap_ff)
    // stack_ff:0 contains the field element
    // Result pushed to stack_i64
    I64WrapFf            = 35,
    // Field shift right (ff.shr)
    // stack_ff:0 contains right operand (shift amount)
    // stack_ff:-1 contains left operand (value to shift)
    // Result pushed to stack_ff
    OpShr                = 36,
    // Field bitwise AND (ff.band)
    // stack_ff:0 contains right operand
    // stack_ff:-1 contains left operand
    // Result pushed to stack_ff
    OpBand               = 37,
    OpRem                = 38,
    // Logical AND operation for field elements
    // stack_ff:0 contains right operand
    // stack_ff:-1 contains left operand
    // Result pushed to stack_ff (1 if both operands non-zero, 0 otherwise)
    OpAnd                = 39,
    // Logical OR operation for field elements
    // stack_ff:0 contains right operand
    // stack_ff:-1 contains left operand
    // Result pushed to stack_ff (1 if either operand non-zero, 0 otherwise)
    OpOr                 = 54,
    // Get template ID of a component
    // stack_i64:0 contains the component index
    // Result pushed to stack_i64 (template_id of the component)
    GetTemplateId        = 40,
    // Get signal position in template
    // stack_i64:0 contains template_id
    // stack_i64:-1 contains signal_id
    // Result pushed to stack_i64 (offset of the signal)
    GetTemplateSignalPosition = 41,
    // Get signal size in template
    // stack_i64:0 contains template_id
    // stack_i64:-1 contains signal_id
    // Result pushed to stack_i64 (size of the signal)
    GetTemplateSignalSize = 42,
    // Get signal type in template
    // stack_i64:0 contains template_id
    // stack_i64:-1 contains signal_id
    // Result pushed to stack_i64 (type of the signal)
    GetTemplateSignalType = 63,
    // Shift left operation for field elements
    // stack_ff:0 contains rhs (shift amount)
    // stack_ff:-1 contains lhs (value to shift)
    // Result pushed to stack_ff (lhs << rhs)
    OpShl                = 43,
    // Return from function with single field element
    // stack_ff:0 contains the return value
    FfReturn             = 44,
    // Bitwise XOR operation for field elements
    // stack_ff:0 contains rhs
    // stack_ff:-1 contains lhs
    // Result pushed to stack_ff (lhs ^ rhs)
    OpBxor               = 45,
    // Bitwise OR operation for field elements
    // stack_ff:0 contains rhs
    // stack_ff:-1 contains lhs
    // Result pushed to stack_ff (lhs | rhs)
    OpBor                = 46,
    // Bitwise NOT operation for field elements
    // stack_ff:0 contains operand
    // Result pushed to stack_ff (~operand)
    OpBnot               = 47,
    // Greater than or equal comparison for field elements
    // stack_ff:0 contains rhs
    // stack_ff:-1 contains lhs
    // Result pushed to stack_ff (lhs >= rhs)
    OpGe                 = 48,
    // Store component input and decrement counter without checking
    // stack_ff contains the value to store
    // stack_i64:0 contains the signal index
    // stack_i64:-1 contains the component index
    StoreCmpInputCnt     = 49,
    // Integer division in field arithmetic (ff.idiv)
    // stack_ff:0 contains divisor
    // stack_ff:-1 contains dividend
    // Result pushed to stack_ff
    OpIdiv               = 50,
    // Field less-than-or-equal comparison (ff.le)
    // stack_ff:0 contains right operand
    // stack_ff:-1 contains left operand
    // Result pushed to stack_ff (1 if lhs <= rhs, 0 otherwise)
    OpLe                 = 51,
    // Gets the length of a specific dimension of a signal in a template
    // stack_i64:0 contains template_id
    // stack_i64:-1 contains signal_id
    // stack_i64:-2 contains dimension_index
    // Result pushed to stack_i64 (length of the dimension)
    GetTemplateSignalDimension = 52,
    // Power operation for field elements (ff.pow)
    // stack_ff:0 contains base
    // stack_ff:-1 contains exponent
    // Result pushed to stack_ff (base^exponent mod prime)
    OpPow                = 53,
    // Copy signals from self to the component by index
    // arguments:
    //   flags: u8
    //     - first 2 bits is a set mode:
    //       - 00 - do nothing
    //       - 01 - update the signals' counter but do not check if run is needed
    //       - 10 - run the component
    //       - 11 - update the signals' counter and check if run is needed.
    // stack_i64:
    //    0: component index
    //   -1: component signal index
    //   -2: self-source signal index
    //   -3: number of signals to copy
    CopyCmpInputsFromSelf = 55,
    // Copy signals from a component to another component's inputs by index
    // arguments:
    //   flags: u8
    //     - first 2 bits are set mode (same semantics as CopyCmpInputsFromSelf)
    // stack_i64:
    //    0: destination component index
    //   -1: destination signal index within the destination component
    //   -2: source component index
    //   -3: source component signal index
    //   -4: number of signals to copy
    CopyCmpInputsFromCmp = 59,
    // Copy signals inside the current component by index
    // stack_i64:
    //    0: destination signal index within current component
    //   -1: source signal index within current component
    //   -2: number of signals to copy
    CopySignal = 60,
    // Copy signals from a component to the current component by index
    // stack_i64:
    //    0: destination signal index within current component
    //   -1: source component index
    //   -2: source component signal index
    //   -3: number of signals to copy
    CopySignalFromCmp = 57,
    // Copy signals from FF memory to the current component's signal range
    // stack_i64:
    //    0: destination signal index within current component
    //   -1: memory address (relative to current FF memory base)
    //   -2: number of signals to copy
    CopySignalFromMemory = 58,
    CopyCmpInputsFromMemory = 62,
    GetBusFieldPosition = 64,
    GetBusFieldSize = 65,
    OpI64Eq = 66,
    FfMStoreFromCmpSignal = 67,
    GetBusFieldType = 68,
    GetBusFieldDimension = 69,
    OpI64Gt = 71,
    OpI64Gte = 72,
    OpI64Eqz = 73,
}

impl TryFrom<u8> for OpCode {
    type Error = RuntimeError;

    fn try_from(byte: u8) -> Result<Self, RuntimeError> {
        Ok(match byte {
            0 => OpCode::NoOp,
            1 => OpCode::LoadSignal,
            2 => OpCode::StoreSignal,
            3 => OpCode::PushI64,
            4 => OpCode::PushFf,
            5 => OpCode::StoreVariableFf,
            6 => OpCode::LoadVariableFf,
            7 => OpCode::StoreVariableI64,
            8 => OpCode::LoadVariableI64,
            9 => OpCode::JumpIfFalseFf,
            10 => OpCode::JumpIfFalseI64,
            11 => OpCode::Jump,
            12 => OpCode::Error,
            13 => OpCode::LoadCmpSignal,
            14 => OpCode::StoreCmpSignalAndRun,
            15 => OpCode::StoreCmpSignalCntCheck,
            16 => OpCode::StoreCmpInput,
            17 => OpCode::OpMul,
            18 => OpCode::OpAdd,
            19 => OpCode::OpNeq,
            20 => OpCode::OpDiv,
            21 => OpCode::OpSub,
            22 => OpCode::OpEq,
            23 => OpCode::OpEqz,
            24 => OpCode::OpI64Add,
            25 => OpCode::OpI64Sub,
            26 => OpCode::FfMReturn,
            27 => OpCode::FfMCall,
            28 => OpCode::FfStore,
            29 => OpCode::FfLoad,
            30 => OpCode::I64Load,
            31 => OpCode::OpLt,
            32 => OpCode::OpGt,
            33 => OpCode::OpI64Mul,
            34 => OpCode::OpI64Lte,
            35 => OpCode::I64WrapFf,
            36 => OpCode::OpShr,
            37 => OpCode::OpBand,
            38 => OpCode::OpRem,
            39 => OpCode::OpAnd,
            40 => OpCode::GetTemplateId,
            41 => OpCode::GetTemplateSignalPosition,
            42 => OpCode::GetTemplateSignalSize,
            43 => OpCode::OpShl,
            44 => OpCode::FfReturn,
            45 => OpCode::OpBxor,
            46 => OpCode::OpBor,
            47 => OpCode::OpBnot,
            48 => OpCode::OpGe,
            49 => OpCode::StoreCmpInputCnt,
            50 => OpCode::OpIdiv,
            51 => OpCode::OpLe,
            52 => OpCode::GetTemplateSignalDimension,
            53 => OpCode::OpPow,
            54 => OpCode::OpOr,
            55 => OpCode::CopyCmpInputsFromSelf,
            56 => OpCode::FfMStore,
            57 => OpCode::CopySignalFromCmp,
            58 => OpCode::CopySignalFromMemory,
            59 => OpCode::CopyCmpInputsFromCmp,
            60 => OpCode::CopySignal,
            61 => OpCode::FfMStoreFromSignal,
            62 => OpCode::CopyCmpInputsFromMemory,
            63 => OpCode::GetTemplateSignalType,
            64 => OpCode::GetBusFieldPosition,
            65 => OpCode::GetBusFieldSize,
            66 => OpCode::OpI64Eq,
            67 => OpCode::FfMStoreFromCmpSignal,
            68 => OpCode::GetBusFieldType,
            69 => OpCode::GetBusFieldDimension,
            70 => OpCode::OpI64Lt,
            71 => OpCode::OpI64Gt,
            72 => OpCode::OpI64Gte,
            73 => OpCode::OpI64Eqz,
            _ => return Err(RuntimeError::InvalidOpCode(byte)),
        })
    }
}

pub struct Signals<T: FieldOps> {
    present: BitVec,
    signals: Vec<T>,
}

impl <T: FieldOps> Signals<T> {
    pub fn new(n: usize) -> Signals<T> {
        Signals {
            present: BitVec::<usize, Lsb0>::repeat(false, n),
            signals: vec![T::zero(); n],
        }
    }

    pub fn set(&mut self, idx: usize, val: T) -> Result<(), Box<dyn Error + Sync + Send>> {
        match self.signals.get_mut(idx) {
            Some(slot) => {
                if self.present[idx] {
                    return Err(Box::new(RuntimeError::SignalIsAlreadySet));
                }
                self.present.set(idx, true);
                *slot = val;
                Ok(())
            }
            None => {
                Err(Box::new(RuntimeError::SignalIndexOutOfBounds))
            }
        }
    }

    pub fn get(&self, idx: usize) -> Result<T, Box<dyn Error + Sync + Send>> {
        match self.signals.get(idx) {
            None => {
                Err(Box::new(RuntimeError::SignalIndexOutOfBounds))
            }
            Some(s) => {
                if !self.present[idx] {
                    return Err(Box::new(RuntimeError::SignalIsNotSet("[1]".to_string() + &std::backtrace::Backtrace::force_capture().to_string())))
                }
                Ok(*s)
            }
        }
    }

    pub fn signals_len(&self) -> usize {
        self.signals.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = Option<T>> + '_ {
        self.present.iter().zip(self.signals.iter()).map(|(bit, val)| {
            if *bit {
                Some(*val)
            } else {
                None
            }
        })
    }
}

pub struct Component<T: FieldOps> {
    pub signals_start: usize,
    pub template_id: usize,
    // pub components: Vec<Option<Box<Component<T>>>>,
    pub components: Vec<Option<Arc<RwLock<Component<T>>>>>,
    pub number_of_inputs: usize,
    signals: Signals<T>,
    pub execution_result: Arc<OnceLock<Option<Arc<dyn Error + Sync + Send>>>>,
}

impl <T: FieldOps> Component<T> {
    pub fn new(
        signals_start: usize,
        template_id: usize,
        components: Vec<Option<Arc<RwLock<Component<T>>>>>,
        number_of_inputs: usize,
        signals_num: usize) -> Component<T> {
        Component {
            signals_start,
            template_id,
            components,
            number_of_inputs,
            signals: Signals::new(signals_num),
            execution_result: Arc::new(OnceLock::new()),
        }
    }

    pub fn set_signal(&mut self, idx: usize, val: T) -> Result<(), Box<dyn Error + Sync + Send>> {
        // println!("SET SIGNAL {}/{}/{}", self.template_id, self.signals_start, idx);
        self.signals.set(idx, val)
    }

    pub fn get_signal(&self, idx: usize) -> Result<T, Box<dyn Error + Sync + Send>> {
        // println!("GET SIGNAL {}/{}/{}", self.template_id, self.signals_start, idx);
        if let Some(Some(err)) = self.execution_result.get() {
            return Err(Box::new(std::io::Error::other(
                format!("Component execution failed: {}", err)
            )));
        }
        self.signals.get(idx)
    }

    pub fn signals_len(&self) -> usize {
        self.signals.signals_len()
    }

    // signal length: self plus lengths of all subcomponents
    pub fn total_signals_len(&self) -> usize {
        self.signals.signals_len() + self.components.iter().flatten()
            .fold(
                0,
                |acc, x| acc + x.read().unwrap().total_signals_len())
        // self.signals.signals_len() + self.components2.iter().flatten().fold(
        //     0,
        // |acc, x| {acc + x.read().unwrap().total_signals_len()})
    }

    pub fn write_all_signals(&self, signals: &mut Vec<Option<T>>) {
        signals.extend(self.signals.iter());
        for component in self.components.iter().flatten() {
            component.read().unwrap().write_all_signals(signals);
        }
    }

    fn decrement_inputs(&mut self, count: usize) -> Result<(), RuntimeError> {
        self.number_of_inputs = self
            .number_of_inputs
            .checked_sub(count)
            .ok_or(RuntimeError::ComponentInputCountUnderflow)?;
        Ok(())
    }
}

fn checked_component<T: FieldOps>(
    component_tree: &Component<T>,
    cmp_idx: usize,
) -> Result<Arc<RwLock<Component<T>>>, RuntimeError> {
    component_tree
        .components
        .get(cmp_idx)
        .ok_or(RuntimeError::InvalidComponentIndex {
            index: cmp_idx,
            len: component_tree.components.len(),
        })?
        .as_ref()
        .cloned()
        .ok_or(RuntimeError::UninitializedComponent)
}

pub struct Circuit<T: FieldOps> {
    pub main_template_id: usize,
    pub templates: Vec<Template>,
    pub functions: Vec<Function>,
    pub function_registry: HashMap<String, usize>, // Function name -> index mapping
    pub field: Field<T>,
    pub witness: Vec<usize>,
    pub signals_num: usize,
    pub input_infos: Vec<InputInfo>,
    pub types: Vec<Type>,
}

#[derive(Debug, Clone)]
pub enum Signal {
    Ff(Vec<usize>),          // dimensions
    Bus(usize, Vec<usize>),  // bus type index and dimensions
}

fn calculate_signal_size(signal: &Signal, types: &[Type]) -> Result<usize, RuntimeError> {
    match signal {
        Signal::Ff(dims) => {
            if dims.is_empty() { Ok(1) } else { checked_product(dims) }
        }
        Signal::Bus(type_idx, dims) => {
            let bus_type = types.get(*type_idx)
                .ok_or(RuntimeError::InvalidTypeId(*type_idx))?;
            let bus_size = bus_type.get_total_size()?;
            if dims.is_empty() {
                Ok(bus_size)
            } else {
                let dim_product = checked_product(dims)?;
                bus_size.checked_mul(dim_product)
                    .ok_or(RuntimeError::OperationOverflows)
            }
        }
    }
}

fn calculate_signal_base_size(signal: &Signal, types: &[Type]) -> Result<usize, RuntimeError> {
    match signal {
        Signal::Ff(..) => Ok(1),
        Signal::Bus(type_idx, ..) => {
            let bus_type = types.get(*type_idx)
                .ok_or(RuntimeError::InvalidTypeId(*type_idx))?;
            bus_type.get_total_size()
        }
    }
}

fn calculate_signal_offset(
    signals: &[Signal], signal_id: usize, types: &[Type]) -> Result<usize, RuntimeError> {

    signals.iter().take(signal_id).try_fold(0usize, |acc, sig| {
        let size = calculate_signal_size(sig, types)?;
        acc.checked_add(size)
            .ok_or(RuntimeError::OperationOverflows)
    })
}

impl Signal {
    pub fn from_ast(ast_signal: &crate::ast::Signal, type_map: &HashMap<String, usize>) -> Self {
        match ast_signal {
            crate::ast::Signal::Ff(dims) => Signal::Ff(dims.clone()),
            crate::ast::Signal::Bus(bus_name, dims) => {
                let index = type_map.get(bus_name)
                    .unwrap_or_else(|| panic!("Bus type '{}' not found in type map", bus_name));
                Signal::Bus(*index, dims.clone())
            }
        }
    }
}

pub struct Template {
    pub name: String,
    pub code: Vec<u8>,
    pub signals_num: usize,
    pub number_of_inputs: usize,
    pub components: Vec<Option<usize>>,
    pub inputs: Vec<Signal>,
    pub outputs: Vec<Signal>,
    // Variable name mappings for debugging
    pub ff_variable_names: Vec<String>,
    pub i64_variable_names: Vec<String>,
}

pub struct Function {
    pub name: String,
    pub code: Vec<u8>,
    // Variable name mappings for debugging
    pub ff_variable_names: Vec<String>,
    pub i64_variable_names: Vec<String>,
}

fn read_byte(code: &[u8], ip: usize) -> Result<u8, RuntimeError> {
    code.get(ip)
        .copied()
        .ok_or(RuntimeError::CodeIndexOutOfBounds)
}

fn read_range(code: &[u8], start: usize, len: usize) -> Result<&[u8], RuntimeError> {
    let end = start
        .checked_add(len)
        .ok_or(RuntimeError::CodeRangeOutOfBounds {
            start,
            len,
            code_len: code.len(),
        })?;
    code.get(start..end)
        .ok_or(RuntimeError::CodeRangeOutOfBounds {
            start,
            len,
            code_len: code.len(),
        })
}

fn advance_ip(ip: usize, len: usize) -> Result<usize, RuntimeError> {
    ip.checked_add(len)
        .ok_or(RuntimeError::CodeIndexOutOfBounds)
}

fn read_byte_advance(code: &[u8], ip: &mut usize) -> Result<u8, RuntimeError> {
    let byte = read_byte(code, *ip)?;
    *ip = advance_ip(*ip, 1)?;
    Ok(byte)
}

fn read_u32_le(code: &[u8], start: usize) -> Result<u32, RuntimeError> {
    let bytes = read_range(code, start, size_of::<u32>())?;
    Ok(u32::from_le_bytes(
        bytes
            .try_into()
            .map_err(|_| RuntimeError::CodeIndexOutOfBounds)?,
    ))
}

fn read_i32_le(code: &[u8], start: usize) -> Result<i32, RuntimeError> {
    let bytes = read_range(code, start, size_of::<i32>())?;
    Ok(i32::from_le_bytes(
        bytes
            .try_into()
            .map_err(|_| RuntimeError::CodeIndexOutOfBounds)?,
    ))
}

fn read_i64_le(code: &[u8], start: usize) -> Result<i64, RuntimeError> {
    let bytes = read_range(code, start, size_of::<i64>())?;
    Ok(i64::from_le_bytes(
        bytes
            .try_into()
            .map_err(|_| RuntimeError::CodeIndexOutOfBounds)?,
    ))
}

fn read_i32_advance(code: &[u8], ip: &mut usize) -> Result<i32, RuntimeError> {
    let value = read_i32_le(code, *ip)?;
    *ip = advance_ip(*ip, size_of::<i32>())?;
    Ok(value)
}

fn read_i64_advance(code: &[u8], ip: &mut usize) -> Result<i64, RuntimeError> {
    let value = read_i64_le(code, *ip)?;
    *ip = advance_ip(*ip, size_of::<i64>())?;
    Ok(value)
}

fn read_range_advance<'a>(
    code: &'a [u8],
    ip: &mut usize,
    len: usize,
) -> Result<&'a [u8], RuntimeError> {
    let bytes = read_range(code, *ip, len)?;
    *ip = advance_ip(*ip, len)?;
    Ok(bytes)
}

fn i64_to_usize(value: i64) -> Result<usize, RuntimeError> {
    value
        .try_into()
        .map_err(|_| RuntimeError::I32ToUsizeConversion)
}

fn read_usize_advance(code: &[u8], ip: &mut usize) -> Result<usize, RuntimeError> {
    i64_to_usize(read_i64_advance(code, ip)?)
}

fn checked_jump_target(
    ip_after_operand: usize,
    offset: i32,
    code_len: usize,
) -> Result<usize, RuntimeError> {
    let target = if offset < 0 {
        ip_after_operand
            .checked_sub(offset.unsigned_abs() as usize)
            .ok_or(RuntimeError::CodeIndexOutOfBounds)?
    } else {
        ip_after_operand
            .checked_add(offset as usize)
            .ok_or(RuntimeError::CodeIndexOutOfBounds)?
    };
    if target <= code_len {
        Ok(target)
    } else {
        Err(RuntimeError::CodeIndexOutOfBounds)
    }
}

fn checked_stack_index(base: usize, offset: usize) -> Result<usize, RuntimeError> {
    base.checked_add(offset).ok_or(RuntimeError::StackOverflow)
}

fn checked_memory_index(base: usize, offset: usize) -> Result<usize, RuntimeError> {
    base.checked_add(offset)
        .ok_or(RuntimeError::MemoryAddressOutOfBounds)
}

fn checked_signal_index(base: usize, offset: usize) -> Result<usize, RuntimeError> {
    base.checked_add(offset)
        .ok_or(RuntimeError::SignalIndexOutOfBounds)
}

fn read_instruction(code: &[u8], ip: usize) -> Result<OpCode, RuntimeError> {
    let byte = read_byte(code, ip)?;
    OpCode::try_from(byte)
}

// read 4 bytes from the code and return usize and the next instruction pointer
fn read_usize32(code: &[u8], ip: usize) -> Result<(usize, usize), RuntimeError> {
    let v = read_u32_le(code, ip)? as usize;
    Ok((v, advance_ip(ip, size_of::<u32>())?))
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("Stack is empty")]
    StackUnderflow,
    #[error("Stack is not large enough")]
    StackOverflow,
    #[error("Value on the stack is None")]
    StackVariableIsNotSet,
    #[error("Failed to convert from i32 to usize")]
    I32ToUsizeConversion,
    #[error("Failed to convert from usize to i64")]
    UsizeToI64Conversion,
    #[error("Signal index is out of bounds")]
    SignalIndexOutOfBounds,
    #[error("Signal is not set:\n{0}")]
    SignalIsNotSet(String),
    #[error("Signal is already set")]
    SignalIsAlreadySet,
    #[error("Code index is out of bounds")]
    CodeIndexOutOfBounds,
    #[error("Code range is out of bounds: start {start}, len {len}, code len {code_len}")]
    CodeRangeOutOfBounds {
        start: usize,
        len: usize,
        code_len: usize,
    },
    #[error("Invalid opcode byte: {0}")]
    InvalidOpCode(u8),
    #[error("component is not initialized")]
    UninitializedComponent,
    #[error("Component index {index} is out of bounds (components: {len})")]
    InvalidComponentIndex { index: usize, len: usize },
    #[error("Component input counter underflow")]
    ComponentInputCountUnderflow,
    #[error("Memory address is out of bounds")]
    MemoryAddressOutOfBounds,
    #[error("Value in the memory is None")]
    MemoryVariableIsNotSet,
    #[error("assertion: {0}")]
    Assertion(i64),
    #[error("Call stack overflow (max depth: 16384)")]
    CallStackOverflow,
    #[error("Call stack underflow")]
    CallStackUnderflow,
    #[error("Invalid function index: {0}")]
    InvalidFunctionIndex(usize),
    #[error("Unknown argument type in function call: {0}")]
    UnknownArgumentType(u8),
    #[error("Invalid template ID: {0}")]
    InvalidTemplateId(usize),
    #[error("Component graph is cyclic")]
    CyclicComponentGraph,
    #[error("Signal ID {0} is out of bounds (max {1})")]
    SignalIdOutOfBounds(usize, usize),
    #[error("Dimension index {0} is out of bounds (signal has {1} dimensions)")]
    DimensionIndexOutOfBounds(usize, usize),
    #[error("Invalid type ID: {0}")]
    InvalidTypeId(usize),
    #[error("Unknown type name: {0}")]
    UnknownTypeName(String),
    #[error("Bus type table contains a cycle")]
    CyclicBusType,
    #[error("Invalid field ID: {1}, type ID: {0}")]
    InvalidFieldId(usize, usize),
    #[error("Operation overflows")]
    OperationOverflows
}

#[derive(Debug, Clone)]
enum ExecutionContext {
    Template,           // Executing template code
    Function(usize),    // Executing function code (function index)
}

#[derive(Debug)]
struct CallFrame {
    // Return execution context
    return_ip: usize,
    return_context: ExecutionContext,
    
    // Stack base pointers to restore
    return_stack_base_pointer_ff: usize,
    return_stack_base_pointer_i64: usize,
    
    // Memory base pointers to restore  
    return_memory_base_pointer_ff: usize,
    return_memory_base_pointer_i64: usize,
}

struct VM<T: FieldOps> {
    stack_ff: Vec<Option<T>>,
    stack_i64: Vec<Option<i64>>,
    stack_base_pointer_ff: usize,
    stack_base_pointer_i64: usize,
    memory_ff: Vec<Option<T>>,
    memory_i64: Vec<Option<i64>>,
    memory_base_pointer_ff: usize,
    memory_base_pointer_i64: usize,
    call_stack: Vec<CallFrame>,
    current_execution_context: ExecutionContext,
}

impl<T: FieldOps> VM<T> {
    fn new() -> Self {
        Self {
            stack_ff: Vec::new(),
            stack_i64: Vec::new(),
            stack_base_pointer_ff: 0,
            stack_base_pointer_i64: 0,
            memory_ff: vec![],
            memory_i64: vec![],
            memory_base_pointer_ff: 0,
            memory_base_pointer_i64: 0,
            call_stack: Vec::new(),
            current_execution_context: ExecutionContext::Template,
        }
    }

    fn push_ff(&mut self, value: T) {
        self.stack_ff.push(Some(value));
    }

    fn pop_ff(&mut self) -> Result<T, RuntimeError> {
        self.stack_ff.pop().ok_or(RuntimeError::StackUnderflow)?
            .ok_or(RuntimeError::StackVariableIsNotSet)
    }

    #[cfg(feature = "debug_vm2")]
    fn peek_ff(&self) -> Result<T, RuntimeError> {
        self.stack_ff.last().and_then(|v| v.as_ref())
            .cloned().ok_or(RuntimeError::StackUnderflow)
    }

    fn push_i64(&mut self, value: i64) {
        self.stack_i64.push(Some(value));
    }

    fn pop_i64(&mut self) -> Result<i64, RuntimeError> {
        self.stack_i64
            .pop().ok_or(RuntimeError::StackUnderflow)?
            .ok_or(RuntimeError::StackVariableIsNotSet)
    }

    fn push_usize(&mut self, value: usize) -> Result<(), RuntimeError> {
        let v: i64 = value.try_into()
            .map_err(|_| RuntimeError::UsizeToI64Conversion)?;
        self.push_i64(v);
        Ok(())
    }

    fn pop_usize(&mut self) -> Result<usize, RuntimeError> {
        self.pop_i64()?
            .try_into()
            .map_err(|_| RuntimeError::I32ToUsizeConversion)
    }

    fn load_ff_stack(&self, base: usize, var_idx: usize) -> Result<T, RuntimeError> {
        let idx = checked_stack_index(base, var_idx)?;
        self.stack_ff
            .get(idx)
            .ok_or(RuntimeError::StackOverflow)?
            .ok_or(RuntimeError::StackVariableIsNotSet)
    }

    fn load_i64_stack(&self, base: usize, var_idx: usize) -> Result<i64, RuntimeError> {
        let idx = checked_stack_index(base, var_idx)?;
        self.stack_i64
            .get(idx)
            .ok_or(RuntimeError::StackOverflow)?
            .ok_or(RuntimeError::StackVariableIsNotSet)
    }

    fn store_ff_stack(
        &mut self,
        base: usize,
        var_idx: usize,
        value: T,
    ) -> Result<(), RuntimeError> {
        let idx = checked_stack_index(base, var_idx)?;
        let slot = self
            .stack_ff
            .get_mut(idx)
            .ok_or(RuntimeError::StackOverflow)?;
        *slot = Some(value);
        Ok(())
    }

    fn store_i64_stack(
        &mut self,
        base: usize,
        var_idx: usize,
        value: i64,
    ) -> Result<(), RuntimeError> {
        let idx = checked_stack_index(base, var_idx)?;
        let slot = self
            .stack_i64
            .get_mut(idx)
            .ok_or(RuntimeError::StackOverflow)?;
        *slot = Some(value);
        Ok(())
    }

    fn store_ff_memory(
        &mut self,
        base: usize,
        offset: usize,
        value: T,
    ) -> Result<(), RuntimeError> {
        let idx = checked_memory_index(base, offset)?;
        let len = checked_memory_index(idx, 1)?;
        if self.memory_ff.len() < len {
            self.memory_ff.resize(len, None);
        }
        self.memory_ff[idx] = Some(value);
        Ok(())
    }

    fn store_i64_memory(
        &mut self,
        base: usize,
        offset: usize,
        value: i64,
    ) -> Result<(), RuntimeError> {
        let idx = checked_memory_index(base, offset)?;
        let len = checked_memory_index(idx, 1)?;
        if self.memory_i64.len() < len {
            self.memory_i64.resize(len, None);
        }
        self.memory_i64[idx] = Some(value);
        Ok(())
    }
}

// Helper function to calculate the size of function arguments in bytecode
fn calculate_args_size<T: FieldOps>(code: &[u8], arg_count: u8) -> Result<usize, RuntimeError> {
    let mut offset = 0usize;
    for _ in 0..arg_count {
        let arg_type = read_byte_advance(code, &mut offset)?;
        let len = match arg_type {
            0 => 8,        // i64 literal
            1 => T::BYTES, // ff literal
            2 => 8,        // ff variable
            3 => 8,        // i64 variable
            4..=7 => 16,   // ff.memory (addr + size, both i64)
            8..=11 => 16,  // i64.memory (addr + size, both i64)
            12..=15 => 16, // signal (idx + size, both i64)
            16..=23 => 24, // subcomponent signal (cmp_idx + sig_idx + size)
            _ => return Err(RuntimeError::CodeIndexOutOfBounds),
        };
        read_range_advance(code, &mut offset, len)?;
    }
    Ok(offset)
}

fn read_resolved_usize<T: FieldOps>(
    vm: &VM<T>,
    code: &[u8],
    offset: &mut usize,
    is_variable: bool,
    caller_stack_base: usize,
) -> Result<usize, RuntimeError> {
    if is_variable {
        let var_idx = read_usize_advance(code, offset)?;
        i64_to_usize(vm.load_i64_stack(caller_stack_base, var_idx)?)
    } else {
        read_usize_advance(code, offset)
    }
}

// Helper function to process function arguments
fn process_function_arguments<T: FieldOps>(
    vm: &mut VM<T>,
    code: &[u8],
    arg_count: u8,
    component_tree: &Component<T>,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    let mut offset = 0usize;
    let mut ff_arg_idx = 0usize;
    let mut i64_arg_idx = 0usize;

    for _ in 0..arg_count {
        let arg_type = read_byte_advance(code, &mut offset)?;

        match arg_type {
            0 => {
                // i64 literal
                let value = read_i64_advance(code, &mut offset)?;

                // Store in function's memory
                vm.store_i64_memory(vm.memory_base_pointer_i64, i64_arg_idx, value)?;
                i64_arg_idx = checked_memory_index(i64_arg_idx, 1)?;
            }
            1 => {
                // ff literal
                let value = T::from_le_bytes(read_range_advance(code, &mut offset, T::BYTES)?)?;

                // Store in function's memory
                vm.store_ff_memory(vm.memory_base_pointer_ff, ff_arg_idx, value)?;
                ff_arg_idx = checked_memory_index(ff_arg_idx, 1)?;
            }
            2 => {
                // ff variable
                let var_idx = read_usize_advance(code, &mut offset)?;

                // Get caller's context from the call frame we just pushed
                let frame = vm
                    .call_stack
                    .last()
                    .ok_or(RuntimeError::CallStackUnderflow)?;
                let caller_stack_base = frame.return_stack_base_pointer_ff;

                // Load value from caller's ff variable stack
                let value = vm.load_ff_stack(caller_stack_base, var_idx)?;

                // Store in function's memory
                vm.store_ff_memory(vm.memory_base_pointer_ff, ff_arg_idx, value)?;
                ff_arg_idx = checked_memory_index(ff_arg_idx, 1)?;
            }
            3 => {
                // i64 variable
                let var_idx = read_usize_advance(code, &mut offset)?;

                // Get caller's context from the call frame we just pushed
                let frame = vm
                    .call_stack
                    .last()
                    .ok_or(RuntimeError::CallStackUnderflow)?;
                let caller_stack_base = frame.return_stack_base_pointer_i64;

                // Load value from caller's i64 variable stack
                let value = vm.load_i64_stack(caller_stack_base, var_idx)?;

                // Store in function's memory
                vm.store_i64_memory(vm.memory_base_pointer_i64, i64_arg_idx, value)?;
                i64_arg_idx = checked_memory_index(i64_arg_idx, 1)?;
            }
            4..=7 => {
                // ff.memory argument
                // Decode bit flags
                let addr_is_variable = (arg_type & 1) != 0;
                let size_is_variable = (arg_type & 2) != 0;

                // Get caller's context from the call frame we just pushed
                let frame = vm
                    .call_stack
                    .last()
                    .ok_or(RuntimeError::CallStackUnderflow)?;
                let caller_base_pointer_ff = frame.return_memory_base_pointer_ff;
                let caller_stack_base = frame.return_stack_base_pointer_i64;

                // Read and resolve address
                let src_addr = read_resolved_usize(
                    vm,
                    code,
                    &mut offset,
                    addr_is_variable,
                    caller_stack_base,
                )?;

                // Read and resolve size
                let size = read_resolved_usize(
                    vm,
                    code,
                    &mut offset,
                    size_is_variable,
                    caller_stack_base,
                )?;

                // Add caller's base pointer to source address
                let src_addr = checked_memory_index(caller_base_pointer_ff, src_addr)?;

                // Ensure source memory is valid
                let src_end = checked_memory_index(src_addr, size)?;
                if src_end > vm.memory_ff.len() {
                    return Err(Box::new(RuntimeError::MemoryAddressOutOfBounds));
                }

                // Copy from caller's memory to function's memory
                let dst_base = checked_memory_index(vm.memory_base_pointer_ff, ff_arg_idx)?;
                let dst_end = checked_memory_index(dst_base, size)?;
                if vm.memory_ff.len() < dst_end {
                    vm.memory_ff.resize(dst_end, None);
                }

                for i in 0..size {
                    vm.memory_ff[dst_base + i] = vm.memory_ff[src_addr + i];
                }

                ff_arg_idx = checked_memory_index(ff_arg_idx, size)?;
            }
            8..=11 => {
                // i64.memory argument
                // Decode bit flags
                let addr_is_variable = (arg_type & 1) != 0;
                let size_is_variable = (arg_type & 2) != 0;

                // Get caller's context from the call frame we just pushed
                let frame = vm
                    .call_stack
                    .last()
                    .ok_or(RuntimeError::CallStackUnderflow)?;
                let caller_base_pointer_i64 = frame.return_memory_base_pointer_i64;
                let caller_stack_base = frame.return_stack_base_pointer_i64;

                // Read and resolve address
                let src_addr = read_resolved_usize(
                    vm,
                    code,
                    &mut offset,
                    addr_is_variable,
                    caller_stack_base,
                )?;

                // Read and resolve size
                let size = read_resolved_usize(
                    vm,
                    code,
                    &mut offset,
                    size_is_variable,
                    caller_stack_base,
                )?;

                // Add caller's base pointer to source address
                let src_addr = checked_memory_index(caller_base_pointer_i64, src_addr)?;

                // Ensure source memory is valid
                let src_end = checked_memory_index(src_addr, size)?;
                if src_end > vm.memory_i64.len() {
                    return Err(Box::new(RuntimeError::MemoryAddressOutOfBounds));
                }

                // Copy from caller's memory to function's memory
                let dst_base = checked_memory_index(vm.memory_base_pointer_i64, i64_arg_idx)?;
                let dst_end = checked_memory_index(dst_base, size)?;
                if vm.memory_i64.len() < dst_end {
                    vm.memory_i64.resize(dst_end, None);
                }

                for i in 0..size {
                    vm.memory_i64[dst_base + i] = vm.memory_i64[src_addr + i];
                }

                i64_arg_idx = checked_memory_index(i64_arg_idx, size)?;
            }
            12..=15 => {
                // signal argument (only valid in component context)
                // Decode bit flags
                let idx_is_variable = (arg_type & 1) != 0;
                let size_is_variable = (arg_type & 2) != 0;

                // Get caller's context from the call frame we just pushed
                let frame = vm
                    .call_stack
                    .last()
                    .ok_or(RuntimeError::CallStackUnderflow)?;
                let caller_stack_base = frame.return_stack_base_pointer_i64;

                // Read and resolve signal index
                let signal_idx =
                    read_resolved_usize(vm, code, &mut offset, idx_is_variable, caller_stack_base)?;

                // Read and resolve size
                let size = read_resolved_usize(
                    vm,
                    code,
                    &mut offset,
                    size_is_variable,
                    caller_stack_base,
                )?;

                // Copy from component signals to function's memory
                let dst_base = checked_memory_index(vm.memory_base_pointer_ff, ff_arg_idx)?;
                let dst_end = checked_memory_index(dst_base, size)?;
                if vm.memory_ff.len() < dst_end {
                    vm.memory_ff.resize(dst_end, None);
                }

                for i in 0..size {
                    let signal_idx = checked_signal_index(signal_idx, i)?;
                    vm.memory_ff[dst_base + i] = Some(component_tree.get_signal(signal_idx)?);
                }

                ff_arg_idx = checked_memory_index(ff_arg_idx, size)?;
            }
            0b0001_0000u8..=0b0001_0111u8 => {
                // signal argument (only valid in component context)
                // Decode bit flags
                let cmp_idx_is_variable = (arg_type & 1) != 0;
                let sig_idx_is_variable = (arg_type & 2) != 0;
                let size_is_variable = (arg_type & 4) != 0;

                // Get caller's context from the call frame we just pushed
                let frame = vm
                    .call_stack
                    .last()
                    .ok_or(RuntimeError::CallStackUnderflow)?;
                let caller_stack_base = frame.return_stack_base_pointer_i64;

                // Read and resolve signal index
                let cmp_idx = read_resolved_usize(
                    vm,
                    code,
                    &mut offset,
                    cmp_idx_is_variable,
                    caller_stack_base,
                )?;

                let sig_idx = read_resolved_usize(
                    vm,
                    code,
                    &mut offset,
                    sig_idx_is_variable,
                    caller_stack_base,
                )?;

                // Read and resolve size
                let size = read_resolved_usize(
                    vm,
                    code,
                    &mut offset,
                    size_is_variable,
                    caller_stack_base,
                )?;

                // Copy from component signals to function's memory
                let dst_base = checked_memory_index(vm.memory_base_pointer_ff, ff_arg_idx)?;
                let dst_end = checked_memory_index(dst_base, size)?;
                if vm.memory_ff.len() < dst_end {
                    vm.memory_ff.resize(dst_end, None);
                }

                let component = checked_component(component_tree, cmp_idx)?;
                for i in 0..size {
                    let sig_idx = checked_signal_index(sig_idx, i)?;
                    vm.memory_ff[dst_base + i] =
                        Some(component.read().unwrap().get_signal(sig_idx)?);
                }
                ff_arg_idx = checked_memory_index(ff_arg_idx, size)?;
            }
            _ => return Err(Box::new(RuntimeError::UnknownArgumentType(arg_type))),
        }
    }

    Ok(())
}

// Converts 8 bytes from the code to i64 and then to usize. Returns error
// if the code length is too short or if i64 < 0 or if i64 is too big to fit
// into usize.
fn usize_from_code(code: &[u8], ip: usize) -> Result<(usize, usize), RuntimeError> {
    let mut next_ip = ip;
    let v = read_usize_advance(code, &mut next_ip)?;
    Ok((v, next_ip))
}

pub fn disassemble_instruction_to_string<T>(
    code: &[u8],
    ip: usize,
    name: &str,
    ff_variable_names: &[String],
    i64_variable_names: &[String],
) -> (usize, String)
where
    T: FieldOps,
{
    let mut output = format!("{:08x} [{:10}] ", ip, name);

    let op_code = match read_instruction(code, ip) {
        Ok(op_code) => op_code,
        Err(e) => {
            output.push_str(&e.to_string());
            return (ip.saturating_add(1), output);
        }
    };
    let mut ip = ip + 1usize;

    macro_rules! decode_or_return {
        ($expr:expr) => {
            match $expr {
                Ok(value) => value,
                Err(e) => {
                    output.push_str(&format!("decode error: {}", e));
                    return (code.len(), output);
                }
            }
        };
    }

    match op_code {
        OpCode::NoOp => {
            output.push_str("NoOp");
        }
        OpCode::LoadSignal => {
            output.push_str("LoadSignal");
        }
        OpCode::StoreSignal => {
            output.push_str("StoreSignal");
        }
        OpCode::PushI64 => {
            let v = decode_or_return!(read_i64_advance(code, &mut ip));
            output.push_str(&format!("PushI64: {}", v));
        }
        OpCode::PushFf => {
            let s = decode_or_return!(read_range_advance(code, &mut ip, T::BYTES));
            let v = decode_or_return!(
                T::from_le_bytes(s).map_err(|_| RuntimeError::CodeIndexOutOfBounds)
            );
            output.push_str(&format!("PushFf: {}", v));
        }
        OpCode::StoreVariableFf => {
            let var_idx: usize;
            (var_idx, ip) = decode_or_return!(usize_from_code(code, ip));
            let var_name = ff_variable_names
                .get(var_idx)
                .map(|s| format!(" ({})", s))
                .unwrap_or_default();
            output.push_str(&format!("StoreVariableFf: {}{}", var_idx, var_name));
        }
        OpCode::StoreVariableI64 => {
            let var_idx: usize;
            (var_idx, ip) = decode_or_return!(usize_from_code(code, ip));
            let var_name = i64_variable_names
                .get(var_idx)
                .map(|s| format!(" ({})", s))
                .unwrap_or_default();
            output.push_str(&format!("StoreVariableI64: {}{}", var_idx, var_name));
        }
        OpCode::LoadVariableI64 => {
            let var_idx: usize;
            (var_idx, ip) = decode_or_return!(usize_from_code(code, ip));
            let var_name = i64_variable_names
                .get(var_idx)
                .map(|s| format!(" ({})", s))
                .unwrap_or_default();
            output.push_str(&format!("LoadVariableI64: {}{}", var_idx, var_name));
        }
        OpCode::LoadVariableFf => {
            let var_idx: usize;
            (var_idx, ip) = decode_or_return!(usize_from_code(code, ip));
            let var_name = ff_variable_names
                .get(var_idx)
                .map(|s| format!(" ({})", s))
                .unwrap_or_default();
            output.push_str(&format!("LoadVariableFf: {}{}", var_idx, var_name));
        }
        OpCode::JumpIfFalseFf => {
            let v = decode_or_return!(read_i32_advance(code, &mut ip));
            let new_ip = decode_or_return!(checked_jump_target(ip, v, code.len()));
            output.push_str(&format!("JumpIfFalseFf: {:+} -> {:08x}", v, new_ip));
        }
        OpCode::JumpIfFalseI64 => {
            let v = decode_or_return!(read_i32_advance(code, &mut ip));
            let new_ip = decode_or_return!(checked_jump_target(ip, v, code.len()));
            output.push_str(&format!("JumpIfFalseI64: {:+} -> {:08x}", v, new_ip));
        }
        OpCode::Jump => {
            let v = decode_or_return!(read_i32_advance(code, &mut ip));
            let new_ip = decode_or_return!(checked_jump_target(ip, v, code.len()));
            output.push_str(&format!("Jump: {:+} -> {:08x}", v, new_ip));
        }
        OpCode::LoadCmpSignal => {
            output.push_str("LoadCmpSignal");
        }
        OpCode::StoreCmpSignalAndRun => {
            output.push_str("StoreCmpSignalAndRun");
        }
        OpCode::StoreCmpSignalCntCheck => {
            output.push_str("StoreCmpSignalCntCheck");
        }
        OpCode::StoreCmpInput => {
            output.push_str("StoreCmpInput");
        }
        OpCode::OpMul => {
            output.push_str("OpMul");
        }
        OpCode::OpAdd => {
            output.push_str("OpAdd");
        }
        OpCode::OpNeq => {
            output.push_str("OpNeq");
        }
        OpCode::OpDiv => {
            output.push_str("OpDiv");
        }
        OpCode::OpIdiv => {
            output.push_str("OpIdiv");
        }
        OpCode::OpSub => {
            output.push_str("OpSub");
        }
        OpCode::OpEq => {
            output.push_str("OpEq");
        }
        OpCode::OpEqz => {
            output.push_str("OpEqz");
        }
        OpCode::OpRem => {
            output.push_str("OpRem");
        }
        OpCode::OpI64Add => {
            output.push_str("OpI64Add");
        }
        OpCode::OpI64Sub => {
            output.push_str("OpI64Sub");
        }
        OpCode::Error => {
            output.push_str("Error");
        }
        OpCode::FfMReturn => {
            output.push_str("FfMReturn");
        }
        OpCode::FfMStore => {
            output.push_str("FfMStore");
        }
        OpCode::FfMStoreFromSignal => {
            output.push_str("FfMStoreFromSignal");
        }
        OpCode::FfMStoreFromCmpSignal => {
            output.push_str("FfMStoreFromCmpSignal");
        }
        OpCode::FfMCall => {
            // Read function index
            let func_idx = decode_or_return!(read_u32_le(code, ip));
            ip = decode_or_return!(advance_ip(ip, size_of::<u32>()));

            // Read argument count
            let arg_count = decode_or_return!(read_byte_advance(code, &mut ip));

            output.push_str(&format!("FfMCall: func_idx={}, args=[", func_idx));

            // Parse each argument
            for i in 0..arg_count {
                if i > 0 {
                    output.push_str(", ");
                }

                let arg_type = decode_or_return!(read_byte_advance(code, &mut ip));

                match arg_type {
                    0 => {
                        // i64 literal
                        let v = decode_or_return!(read_i64_advance(code, &mut ip));
                        output.push_str(&format!("i64.{}", v));
                    }
                    1 => {
                        // ff literal
                        let bytes = decode_or_return!(read_range_advance(code, &mut ip, T::BYTES));
                        let v =
                            decode_or_return!(T::from_le_bytes(bytes)
                                .map_err(|_| RuntimeError::CodeIndexOutOfBounds));
                        output.push_str(&format!("ff.{}", v));
                    }
                    2 => {
                        // ff variable
                        let v = decode_or_return!(read_i64_advance(code, &mut ip));
                        output.push_str(&format!("ff.var[{}]", v));
                    }
                    3 => {
                        // i64 variable
                        let v = decode_or_return!(read_i64_advance(code, &mut ip));
                        output.push_str(&format!("i64.var[{}]", v));
                    }
                    4..=7 => {
                        // ff memory
                        let addr_is_variable = (arg_type & 1) != 0;
                        let size_is_variable = (arg_type & 2) != 0;

                        let addr_val = decode_or_return!(read_i64_advance(code, &mut ip));
                        let size_val = decode_or_return!(read_i64_advance(code, &mut ip));

                        output.push_str("ff.memory(");
                        if addr_is_variable {
                            let var_name = i64_variable_names
                                .get(addr_val as usize)
                                .map(|s| format!(" ({})", s))
                                .unwrap_or_default();
                            output.push_str(&format!("var[{}]{}", addr_val, var_name));
                        } else {
                            output.push_str(&format!("{}", addr_val));
                        }
                        output.push(',');
                        if size_is_variable {
                            let var_name = i64_variable_names.get(size_val as usize)
                                .map(|s| format!(" ({})", s))
                                .unwrap_or_default();
                            output.push_str(&format!("var[{}]{}", size_val, var_name));
                        } else {
                            output.push_str(&format!("{}", size_val));
                        }
                        output.push(')');
                    }
                    8..=11 => { // i64 memory
                        let addr_is_variable = (arg_type & 1) != 0;
                        let size_is_variable = (arg_type & 2) != 0;

                        let addr_val = decode_or_return!(read_i64_advance(code, &mut ip));
                        let size_val = decode_or_return!(read_i64_advance(code, &mut ip));

                        output.push_str("i64.memory(");
                        if addr_is_variable {
                            let var_name = i64_variable_names
                                .get(addr_val as usize)
                                .map(|s| format!(" ({})", s))
                                .unwrap_or_default();
                            output.push_str(&format!("var[{}]{}", addr_val, var_name));
                        } else {
                            output.push_str(&format!("{}", addr_val));
                        }
                        output.push(',');
                        if size_is_variable {
                            let var_name = i64_variable_names.get(size_val as usize)
                                .map(|s| format!(" ({})", s))
                                .unwrap_or_default();
                            output.push_str(&format!("var[{}]{}", size_val, var_name));
                        } else {
                            output.push_str(&format!("{}", size_val));
                        }
                        output.push(')');
                    }
                    12..=15 => { // signal
                        let idx_is_variable = (arg_type & 1) != 0;
                        let size_is_variable = (arg_type & 2) != 0;

                        let idx_val = decode_or_return!(read_i64_advance(code, &mut ip));
                        let size_val = decode_or_return!(read_i64_advance(code, &mut ip));

                        output.push_str("signal(");
                        if idx_is_variable {
                            let var_name = i64_variable_names
                                .get(idx_val as usize)
                                .map(|s| format!(" ({})", s))
                                .unwrap_or_default();
                            output.push_str(&format!("var[{}]{}", idx_val, var_name));
                        } else {
                            output.push_str(&format!("{}", idx_val));
                        }
                        output.push(',');
                        if size_is_variable {
                            let var_name = i64_variable_names.get(size_val as usize)
                                .map(|s| format!(" ({})", s))
                                .unwrap_or_default();
                            output.push_str(&format!("var[{}]{}", size_val, var_name));
                        } else {
                            output.push_str(&format!("{}", size_val));
                        }
                        output.push(')');
                    }
                    _ => {
                        output.push_str(&format!("unknown_arg_type({})", arg_type));
                    }
                }
            }
            
            output.push(']');
        }
        OpCode::FfStore => {
            output.push_str("FfStore");
        }
        OpCode::FfLoad => {
            output.push_str("FfLoad");
        }
        OpCode::I64Load => {
            output.push_str("I64Load");
        }
        OpCode::OpLt => {
            output.push_str("OpLt");
        }
        OpCode::OpLe => {
            output.push_str("OpLe");
        }
        OpCode::OpGt => {
            output.push_str("OpGt");
        }
        OpCode::OpI64Mul => {
            output.push_str("OpI64Mul");
        }
        OpCode::OpI64Lt => {
            output.push_str("OpI64Lt");
        }
        OpCode::OpI64Lte => {
            output.push_str("OpI64Lte");
        }
        OpCode::OpI64Gt => {
            output.push_str("OpI64Gt");
        }
        OpCode::OpI64Gte => {
            output.push_str("OpI64Gte");
        }
        OpCode::I64WrapFf => {
            output.push_str("I64WrapFf");
        }
        OpCode::OpI64Eq => {
            output.push_str("OpI64Eq");
        }
        OpCode::OpI64Eqz => {
            output.push_str("OpI64Eqz");
        }
        OpCode::OpShr => {
            output.push_str("OpShr");
        }
        OpCode::OpBand => {
            output.push_str("OpBand");
        }
        OpCode::OpAnd => {
            output.push_str("OpAnd");
        }
        OpCode::OpOr => {
            output.push_str("OpOr");
        }
        OpCode::GetTemplateId => {
            output.push_str("GetTemplateId");
        }
        OpCode::GetTemplateSignalPosition => {
            output.push_str("GetTemplateSignalPosition");
        }
        OpCode::GetTemplateSignalSize => {
            output.push_str("GetTemplateSignalSize");
        }
        OpCode::GetTemplateSignalType => {
            output.push_str("GetTemplateSignalType");
        }
        OpCode::GetTemplateSignalDimension => {
            output.push_str("GetTemplateSignalDimension");
        }
        OpCode::OpPow => {
            output.push_str("OpPow");
        }
        OpCode::OpShl => {
            output.push_str("OpShl");
        }
        OpCode::FfReturn => {
            output.push_str("FfReturn");
        }
        OpCode::OpBxor => {
            output.push_str("OpBxor");
        }
        OpCode::OpBor => {
            output.push_str("OpBor");
        }
        OpCode::OpBnot => {
            output.push_str("OpBnot");
        }
        OpCode::OpGe => {
            output.push_str("OpGe");
        }
        OpCode::StoreCmpInputCnt => {
            output.push_str("StoreCmpInputCnt");
        }
        OpCode::CopyCmpInputsFromSelf => {
            output.push_str("CopyCmpInputsFromSelf");
        }
        OpCode::CopyCmpInputsFromCmp => {
            output.push_str("CopyCmpInputsFromCmp");
        }
        OpCode::CopySignal => {
            output.push_str("CopySignal");
        }
        OpCode::CopySignalFromCmp => {
            output.push_str("CopySignalFromCmp");
        }
        OpCode::CopySignalFromMemory => {
            output.push_str("CopySignalFromMemory");
        }
        OpCode::CopyCmpInputsFromMemory => {
            output.push_str("CopyCmpInputsFromMemory");
        }
        OpCode::GetBusFieldPosition => {
            output.push_str("GetBusFieldPosition");
        }
        OpCode::GetBusFieldSize => {
            output.push_str("GetBusFieldSize");
        }
        OpCode::GetBusFieldType => {
            output.push_str("GetBusFieldType");
        }
        OpCode::GetBusFieldDimension => {
            output.push_str("GetBusFieldDimension");
        }
    }

    (ip, output)
}

pub fn disassemble_instruction<T>(
    code: &[u8], ip: usize, name: &str,
    ff_variable_names: &[String],
    i64_variable_names: &[String]) -> usize
where
    T: FieldOps {
    let (new_ip, output) = disassemble_instruction_to_string::<T>(
        code, ip, name, ff_variable_names, i64_variable_names);
    println!("{}", output);
    new_ip
}

// Helper function to get the currently executing template/function
#[cfg(feature = "debug_vm2")]
fn get_current_context<'a, T: FieldOps>(
    vm: &VM<T>,
    circuit: &'a Circuit<T>,
    component_tree: &Component<T>,
) -> (&'a [u8], &'a str, &'a Vec<String>, &'a Vec<String>) {
    match vm.current_execution_context {
        ExecutionContext::Template => (
            &circuit.templates[component_tree.template_id].code,
            &circuit.templates[component_tree.template_id].name,
            &circuit.templates[component_tree.template_id].ff_variable_names,
            &circuit.templates[component_tree.template_id].i64_variable_names,
        ),
        ExecutionContext::Function(func_idx) => (
            &circuit.functions[func_idx].code,
            &circuit.functions[func_idx].name,
            &circuit.functions[func_idx].ff_variable_names,
            &circuit.functions[func_idx].i64_variable_names,
        ),
    }
}

#[cfg(not(feature = "debug_vm2"))]
fn get_current_context<'a, T: FieldOps>(
    vm: &VM<T>,
    circuit: &'a Circuit<T>,
    component_tree: &Component<T>,
) -> &'a [u8] {
    match vm.current_execution_context {
        ExecutionContext::Template =>
            &circuit.templates[component_tree.template_id].code,
        ExecutionContext::Function(func_idx) =>
            &circuit.functions[func_idx].code,
    }
}

#[cfg(feature = "parallel_components")]
fn spawn_component_execution<'scope, 'env, F, T>(
    scope: &'scope std::thread::Scope<'scope, 'env>,
    circuit: &'env Circuit<T>,
    ff: &'env F,
    c: Arc<RwLock<Component<T>>>,
)
where
    for <'a> &'a F: FieldOperations<Type = T>,
    T: FieldOps + 'env,
{
    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    let pair2 = Arc::clone(&pair);
    let c_clone = c.clone();
    let execution_result = c.read().unwrap().execution_result.clone();

    scope.spawn(move || {
        let mut component = c_clone.write().unwrap();

        let (lock, cvar) = &*pair2;
        let mut started = lock.lock().unwrap();
        *started = true;
        cvar.notify_one();

        let result = execute(circuit, ff, &mut component);
        let error_opt = result.err().map(|e| Arc::from(e) as Arc<dyn Error + Sync + Send>);
        let _ = execution_result.set(error_opt);
    });

    let (lock, cvar) = &*pair;
    let mut started = lock.lock().unwrap();
    while !*started {
        started = cvar.wait(started).unwrap();
    }
}
pub fn execute<F, T: FieldOps>(
    circuit: &Circuit<T>,
    ff: &F,
    component_tree: &mut Component<T>,
) -> Result<(), Box<dyn Error + Sync + Send>>
where
    for<'a> &'a F: FieldOperations<Type = T>,
{
    std::thread::scope(|_scope| -> Result<(), Box<dyn Error + Sync + Send>> {
        #[cfg(feature = "parallel_components")]
        let scope = _scope;
        #[cfg(feature = "debug_vm2")]
        {
            let template_name = &circuit.templates[component_tree.template_id].name;
            println!(
                "execute {}[{}]",
                template_name, component_tree.signals_start
            );
        }
        #[cfg(feature = "parallel_components")]
        component_tree
            .components
            .iter_mut()
            .filter_map(|x| x.as_mut())
            .filter(|x| x.read().unwrap().number_of_inputs == 0)
            .for_each(|c| spawn_component_execution(scope, circuit, ff, c.clone()));

        #[cfg(not(feature = "parallel_components"))]
        component_tree
            .components
            .iter_mut()
            .filter_map(|x| x.as_mut())
            .filter(|x| x.read().unwrap().number_of_inputs == 0)
            .try_for_each(|c| -> Result<(), Box<dyn Error + Sync + Send>> {
                let mut component = c
                    .write()
                    .map_err(|e| format!("Failed to lock component: {}", e))?;
                execute(circuit, ff, &mut component)?;
                Ok(())
            })?;

        let mut ip: usize = 0;
        let mut vm = VM::<T>::new();

        // Initialize with template's variable counts (function calls will resize as needed)
        // TODO every time we switch the context, we should check the stacks have sufficient size
        vm.stack_ff.resize_with(
            circuit.templates[component_tree.template_id]
                .ff_variable_names
                .len(),
            || None,
        );
        vm.stack_i64.resize_with(
            circuit.templates[component_tree.template_id]
                .i64_variable_names
                .len(),
            || None,
        );

        #[cfg(feature = "debug_vm2")]
        let (mut code, mut name, mut ff_variable_names, mut i64_variable_names) =
            get_current_context(&vm, circuit, component_tree);
        #[cfg(not(feature = "debug_vm2"))]
        let mut code = get_current_context(&vm, circuit, component_tree);

        'label: loop {
            if ip == code.len() {
                // Handle end of current execution context
                match vm.current_execution_context {
                    ExecutionContext::Template => {
                        // Template completed normally
                        break 'label;
                    }
                    ExecutionContext::Function(_) => {
                        // Function ended without explicit return - this is an error
                        return Err(Box::new(RuntimeError::Assertion(-998))); // Function didn't return
                    }
                }
            }

            #[cfg(feature = "debug_vm2")]
            disassemble_instruction::<T>(code, ip, name, ff_variable_names, i64_variable_names);

            let op_code = read_instruction(code, ip)?;
            ip += 1;

            match op_code {
                OpCode::NoOp => (),
                OpCode::LoadSignal => {
                    let sig_idx = vm.pop_usize()?;
                    let sig = component_tree.get_signal(sig_idx)?;

                    #[cfg(feature = "debug_vm2")]
                    {
                        println!(
                            "LoadSignal [S{}]: {}: {}",
                            component_tree.signals_start + sig_idx,
                            sig_idx,
                            sig
                        );
                    }

                    vm.push_ff(sig);
                }
                OpCode::StoreSignal => {
                    let signal_idx = vm.pop_usize()?;
                    let value = vm.pop_ff()?;
                    #[cfg(feature = "debug_vm2")]
                    {
                        println!(
                            "StoreSignal [S{}]: {} = {}",
                            component_tree.signals_start + signal_idx,
                            signal_idx,
                            value
                        );
                    }
                    component_tree.set_signal(signal_idx, value)?;
                }
                OpCode::PushI64 => {
                    vm.push_i64(read_i64_advance(code, &mut ip)?);
                }
                OpCode::PushFf => {
                    let s = read_range_advance(code, &mut ip, T::BYTES)?;
                    let v = ff.parse_le_bytes(s)?;
                    vm.push_ff(v);
                }
                OpCode::StoreVariableFf => {
                    let var_idx: usize;
                    (var_idx, ip) = usize_from_code(code, ip)?;
                    let value = vm.pop_ff()?;
                    vm.store_ff_stack(vm.stack_base_pointer_ff, var_idx, value)?;
                    #[cfg(feature = "debug_vm2")]
                    {
                        let var_name = ff_variable_names
                            .get(var_idx)
                            .map(|s| format!(" ({})", s))
                            .unwrap_or_default();
                        println!("StoreVariableFf: {}{} = {}", var_idx, var_name, value);
                    }
                }
                OpCode::StoreVariableI64 => {
                    let var_idx: usize;
                    (var_idx, ip) = usize_from_code(code, ip)?;
                    let value = vm.pop_i64()?;
                    vm.store_i64_stack(vm.stack_base_pointer_i64, var_idx, value)?;
                    #[cfg(feature = "debug_vm2")]
                    {
                        let var_name = i64_variable_names
                            .get(var_idx)
                            .map(|s| format!(" ({})", s))
                            .unwrap_or_default();
                        println!("StoreVariableI64: {}{} = {}", var_idx, var_name, value);
                    }
                }
                OpCode::LoadVariableI64 => {
                    let var_idx: usize;
                    (var_idx, ip) = usize_from_code(code, ip)?;
                    let var = vm.load_i64_stack(vm.stack_base_pointer_i64, var_idx)?;
                    #[cfg(feature = "debug_vm2")]
                    {
                        let var_name = i64_variable_names
                            .get(var_idx)
                            .map(|s| format!(" ({})", s))
                            .unwrap_or_default();
                        println!("LoadVariableI64: {}{} = {}", var_idx, var_name, var);
                    }
                    vm.push_i64(var);
                }
                OpCode::LoadVariableFf => {
                    let var_idx: usize;
                    (var_idx, ip) = usize_from_code(code, ip)?;
                    let var = vm.load_ff_stack(vm.stack_base_pointer_ff, var_idx)?;
                    vm.push_ff(var);
                }
                OpCode::OpMul => {
                    let lhs = vm.pop_ff()?;
                    let rhs = vm.pop_ff()?;
                    vm.push_ff(ff.mul(lhs, rhs));
                }
                OpCode::OpAdd => {
                    let lhs = vm.pop_ff()?;
                    let rhs = vm.pop_ff()?;
                    vm.push_ff(ff.add(lhs, rhs));
                }
                OpCode::OpNeq => {
                    let lhs = vm.pop_ff()?;
                    let rhs = vm.pop_ff()?;
                    vm.push_ff(ff.neq(lhs, rhs));
                }
                OpCode::LoadCmpSignal => {
                    let sig_idx = vm.pop_usize()?;
                    let cmp_idx = vm.pop_usize()?;
                    let component = checked_component(component_tree, cmp_idx)?;
                    let sig = component.read().unwrap().get_signal(sig_idx)?;
                    #[cfg(feature = "debug_vm2")]
                    {
                        let signals_start = component.read().unwrap().signals_start;
                        println!(
                            "LoadCmpSignal [S{}]: {} = {}",
                            signals_start + sig_idx,
                            sig_idx,
                            sig
                        );
                    }
                    vm.push_ff(sig);
                }
                OpCode::StoreCmpSignalAndRun => {
                    let sig_idx = vm.pop_usize()?;
                    let cmp_idx = vm.pop_usize()?;
                    let value = vm.pop_ff()?;
                    let component = checked_component(component_tree, cmp_idx)?;
                    {
                        let mut c = component.write().unwrap();
                        c.set_signal(sig_idx, value)?;
                        c.decrement_inputs(1)?;
                    }
                    #[cfg(feature = "debug_vm2")]
                    {
                        let c = component.read().unwrap();
                        println!(
                        "StoreCmpSignalAndRun [S{}]: {}[{}/{}] = {}, inputs left: {}, template: {}",
                        c.signals_start + sig_idx, cmp_idx, c.signals_start, sig_idx, value,
                        c.number_of_inputs, circuit.templates[c.template_id].name);
                        println!("StoreCmpSignalAndRun: Run component {}", cmp_idx);
                    }

                    #[cfg(feature = "parallel_components")]
                    spawn_component_execution(scope, circuit, ff, component.clone());

                    #[cfg(not(feature = "parallel_components"))]
                    {
                        let mut component = component
                            .write()
                            .map_err(|e| format!("Failed to lock component: {}", e))?;
                        execute(circuit, ff, &mut component)?;
                    }
                }
                OpCode::StoreCmpSignalCntCheck => {
                    let sig_idx = vm.pop_usize()?;
                    let cmp_idx = vm.pop_usize()?;
                    let value = vm.pop_ff()?;
                    let component = checked_component(component_tree, cmp_idx)?;
                    let mut run = false;
                    #[cfg(feature = "debug_vm2")]
                    {
                        let c = component.read().unwrap();
                        let inputs_left = c
                            .number_of_inputs
                            .checked_sub(1)
                            .ok_or(RuntimeError::ComponentInputCountUnderflow)?;
                        println!(
                        "StoreCmpSignalCntCheck [S{}]: cmp {} ({}) sig {} = {}, inputs left: {}",
                        c.signals_start+sig_idx, cmp_idx,
                        circuit.templates[c.template_id].name, sig_idx,
                        value, inputs_left);
                    }

                    {
                        let mut c = component.write().unwrap();
                        c.set_signal(sig_idx, value)?;
                        c.decrement_inputs(1)?;
                        if c.number_of_inputs == 0 {
                            run = true;
                        }
                    }
                    if run {
                        #[cfg(feature = "debug_vm2")]
                        {
                            println!("StoreCmpSignalCntCheck: Run component {}", cmp_idx);
                        }
                        #[cfg(feature = "parallel_components")]
                        spawn_component_execution(scope, circuit, ff, component.clone());

                        #[cfg(not(feature = "parallel_components"))]
                        {
                            let mut component = component
                                .write()
                                .map_err(|e| format!("Failed to lock component: {}", e))?;
                            execute(circuit, ff, &mut component)?;
                        }
                    }
                }
                OpCode::StoreCmpInputCnt => {
                    let sig_idx = vm.pop_usize()?;
                    let cmp_idx = vm.pop_usize()?;
                    let value = vm.pop_ff()?;
                    let component = checked_component(component_tree, cmp_idx)?;
                    #[cfg(feature = "debug_vm2")]
                    {
                        let c = component.read().unwrap();
                        let inputs_left = c
                            .number_of_inputs
                            .checked_sub(1)
                            .ok_or(RuntimeError::ComponentInputCountUnderflow)?;
                        println!(
                            "StoreCmpInputCnt [S{}]: cmp {} ({}) sig {} = {}, inputs left: {}",
                            c.signals_start + sig_idx,
                            cmp_idx,
                            circuit.templates[c.template_id].name,
                            sig_idx,
                            value,
                            inputs_left
                        );
                    }

                    {
                        let mut c = component.write().unwrap();
                        c.set_signal(sig_idx, value)?;
                        c.decrement_inputs(1)?;
                    }
                    // Skip the check for c.number_of_inputs == 0 and component execution
                }
                OpCode::StoreCmpInput => {
                    let sig_idx = vm.pop_usize()?;
                    let cmp_idx = vm.pop_usize()?;
                    let value = vm.pop_ff()?;
                    let component = checked_component(component_tree, cmp_idx)?;
                    component.write().unwrap().set_signal(sig_idx, value)?;
                    #[cfg(feature = "debug_vm2")]
                    {
                        let c = component.read().unwrap();
                        println!(
                            "StoreCmpInput [S{}]: {}[{}/{}] = {}, inputs left: {}, template: {}",
                            c.signals_start + sig_idx,
                            cmp_idx,
                            c.signals_start,
                            sig_idx,
                            value,
                            c.number_of_inputs,
                            circuit.templates[c.template_id].name
                        );
                    }
                }
                OpCode::JumpIfFalseFf => {
                    let offset = read_i32_advance(code, &mut ip)?;

                    if vm.pop_ff()?.is_zero() {
                        ip = checked_jump_target(ip, offset, code.len())?;
                    }
                }
                OpCode::JumpIfFalseI64 => {
                    let offset = read_i32_advance(code, &mut ip)?;

                    if vm.pop_i64()? == 0 {
                        ip = checked_jump_target(ip, offset, code.len())?;
                    }
                }
                OpCode::Error => {
                    let error_code = vm.pop_i64()?;
                    return Err(Box::new(RuntimeError::Assertion(error_code)));
                }
                OpCode::Jump => {
                    let offset = read_i32_advance(code, &mut ip)?;
                    ip = checked_jump_target(ip, offset, code.len())?;
                }
                OpCode::OpDiv => {
                    let lhs = vm.pop_ff()?;
                    let rhs = vm.pop_ff()?;
                    vm.push_ff(ff.div(lhs, rhs));
                }
                OpCode::OpIdiv => {
                    let lhs = vm.pop_ff()?;
                    let rhs = vm.pop_ff()?;
                    vm.push_ff(ff.idiv(lhs, rhs));
                }
                OpCode::OpSub => {
                    let lhs = vm.pop_ff()?;
                    let rhs = vm.pop_ff()?;
                    vm.push_ff(ff.sub(lhs, rhs));
                }
                OpCode::OpEq => {
                    let lhs = vm.pop_ff()?;
                    let rhs = vm.pop_ff()?;
                    vm.push_ff(ff.eq(lhs, rhs));
                }
                OpCode::OpEqz => {
                    let arg = vm.pop_ff()?;
                    if arg.is_zero() {
                        vm.push_ff(T::one());
                    } else {
                        vm.push_ff(T::zero());
                    }
                }
                OpCode::OpI64Add => {
                    let lhs = vm.pop_i64()?;
                    let rhs = vm.pop_i64()?;
                    vm.push_i64(lhs + rhs);
                }
                OpCode::OpI64Sub => {
                    let lhs = vm.pop_i64()?;
                    let rhs = vm.pop_i64()?;
                    vm.push_i64(lhs - rhs);
                }
                OpCode::FfMReturn => {
                    // Pop size, src, dst from stack
                    let size = vm.pop_usize()?;
                    let src_addr = vm.pop_usize()?;
                    let dst_addr = vm.pop_usize()?;

                    // Pop the call frame to get return context
                    let call_frame = vm
                        .call_stack
                        .pop()
                        .ok_or(RuntimeError::CallStackUnderflow)?;

                    #[cfg(feature = "debug_vm2")]
                    {
                        println!("FfMReturn:");
                    }
                    // Copy memory from a function's space to caller's space
                    let src_start = checked_memory_index(vm.memory_base_pointer_ff, src_addr)?;
                    let dst_start = checked_memory_index(
                        call_frame.return_memory_base_pointer_ff,
                        dst_addr,
                    )?;
                    for i in 0..size {
                        let src_idx = checked_memory_index(src_start, i)?;
                        let dst_idx = checked_memory_index(dst_start, i)?;

                        if src_idx < vm.memory_ff.len() {
                            if dst_idx >= vm.memory_ff.len() {
                                vm.memory_ff.resize(
                                    checked_memory_index(dst_idx, 1)?,
                                    None,
                                );
                            }
                            #[cfg(feature = "debug_vm2")]
                            {
                                let val = match vm.memory_ff[src_idx] {
                                    Some(val) => val.to_string(),
                                    None => "None".to_string(),
                                };
                                println!("  {} -> {}: {}", src_idx, dst_idx, val);
                            }
                            if src_idx != dst_idx {
                                vm.memory_ff[dst_idx] = vm.memory_ff[src_idx];
                            }
                        } else {
                            return Err(Box::new(RuntimeError::MemoryAddressOutOfBounds));
                        }
                    }

                    // Shrink memory to remove function's garbage while preserving
                    // caller's space. This ensures the next memory growth
                    // initializes with None instead of stale values.
                    let mut memory_ff_size = vm.memory_base_pointer_ff;
                    let last_dst_idx = checked_memory_index(dst_start, size)?;
                    if last_dst_idx > memory_ff_size {
                        memory_ff_size = last_dst_idx;
                    }
                    vm.memory_ff.resize(memory_ff_size, None);

                    // Restore execution context
                    ip = call_frame.return_ip;
                    vm.current_execution_context = call_frame.return_context;
                    vm.stack_base_pointer_ff = call_frame.return_stack_base_pointer_ff;
                    vm.stack_base_pointer_i64 = call_frame.return_stack_base_pointer_i64;
                    vm.memory_base_pointer_ff = call_frame.return_memory_base_pointer_ff;
                    vm.memory_base_pointer_i64 = call_frame.return_memory_base_pointer_i64;

                    // Switch back to the caller's execution context
                    #[cfg(feature = "debug_vm2")]
                    {
                        (code, name, ff_variable_names, i64_variable_names) =
                            get_current_context(&vm, circuit, component_tree);
                    }
                    #[cfg(not(feature = "debug_vm2"))]
                    {
                        code = get_current_context(&vm, circuit, component_tree);
                    }
                }
                OpCode::FfMStore => {
                    let size = vm.pop_usize()?;
                    let src_addr = vm.pop_usize()?;
                    let dst_addr = vm.pop_usize()?;

                    let dst_start = dst_addr
                        .checked_add(vm.memory_base_pointer_ff)
                        .ok_or(RuntimeError::MemoryAddressOutOfBounds)?;
                    let src_start = src_addr
                        .checked_add(vm.memory_base_pointer_ff)
                        .ok_or(RuntimeError::MemoryAddressOutOfBounds)?;

                    for offset in 0..size {
                        let src_idx = src_start
                            .checked_add(offset)
                            .ok_or(RuntimeError::MemoryAddressOutOfBounds)?;
                        let dst_idx = dst_start
                            .checked_add(offset)
                            .ok_or(RuntimeError::MemoryAddressOutOfBounds)?;

                        if src_idx >= vm.memory_ff.len() {
                            return Err(Box::new(RuntimeError::MemoryAddressOutOfBounds));
                        }
                        let value = *vm
                            .memory_ff
                            .get(src_idx)
                            .and_then(|v| v.as_ref())
                            .ok_or(RuntimeError::MemoryVariableIsNotSet)?;

                        if dst_idx >= vm.memory_ff.len() {
                            vm.memory_ff.resize(dst_idx + 1, None);
                        }
                        vm.memory_ff[dst_idx] = Some(value);
                    }

                    #[cfg(feature = "debug_vm2")]
                    {
                        println!(
                            "FfMStore: copied {} elements from [{}] to [{}]",
                            size, src_start, dst_start,
                        );
                    }
                }
                OpCode::FfMStoreFromSignal => {
                    let size = vm.pop_usize()?;
                    let sig_idx = vm.pop_usize()?;
                    let dst_addr = vm.pop_usize()?;

                    let dst_start = vm
                        .memory_base_pointer_ff
                        .checked_add(dst_addr)
                        .ok_or(RuntimeError::MemoryAddressOutOfBounds)?;

                    let want_len = dst_start
                        .checked_add(size)
                        .ok_or(RuntimeError::MemoryAddressOutOfBounds)?;
                    if want_len > vm.memory_ff.len() {
                        vm.memory_ff.resize(want_len, None);
                    }
                    for offset in 0..size {
                        let i = sig_idx
                            .checked_add(offset)
                            .ok_or(RuntimeError::SignalIndexOutOfBounds)?;
                        let value = component_tree.get_signal(i)?;
                        vm.memory_ff[dst_start + offset] = Some(value);
                    }

                    #[cfg(feature = "debug_vm2")]
                    {
                        println!(
                        "FfMStoreFromSignal: copied {} elements from signals[S{}..S{}) to memory[M{}..M{})",
                        size, sig_idx, sig_idx + size, dst_start, dst_start + size
                    );
                    }
                }
                OpCode::FfMStoreFromCmpSignal => {
                    let size = vm.pop_usize()?;
                    let sig_idx = vm.pop_usize()?;
                    let cmp_idx = vm.pop_usize()?;
                    let dst_addr = vm.pop_usize()?;

                    let dst_start = vm
                        .memory_base_pointer_ff
                        .checked_add(dst_addr)
                        .ok_or(RuntimeError::MemoryAddressOutOfBounds)?;

                    let want_len = dst_start
                        .checked_add(size)
                        .ok_or(RuntimeError::MemoryAddressOutOfBounds)?;
                    if want_len > vm.memory_ff.len() {
                        vm.memory_ff.resize(want_len, None);
                    }
                    let component = checked_component(component_tree, cmp_idx)?;
                    for offset in 0..size {
                        let sig_idx = checked_signal_index(sig_idx, offset)?;
                        let value = component.read().unwrap().get_signal(sig_idx)?;
                        vm.memory_ff[dst_start + offset] = Some(value);
                    }

                    #[cfg(feature = "debug_vm2")]
                    {
                        println!(
                        "FfMStoreFromCmpSignal: copied {} elements from cmp {} signals[S{}..S{}) to memory[M{}..M{})",
                        size,
                        cmp_idx,
                        sig_idx,
                        sig_idx + size,
                        dst_start,
                        dst_start + size
                    );
                    }
                }
                OpCode::FfMCall => {
                    // Check call stack depth
                    if vm.call_stack.len() >= 16384 {
                        return Err(Box::new(RuntimeError::CallStackOverflow));
                    }

                    let func_idx: usize;
                    (func_idx, ip) = read_usize32(code, ip)?;

                    // Validate function index
                    if func_idx >= circuit.functions.len() {
                        return Err(Box::new(RuntimeError::InvalidFunctionIndex(func_idx)));
                    }

                    // Read argument count
                    let arg_count = read_byte_advance(code, &mut ip)?;
                    let args_size = calculate_args_size::<T>(&code[ip..], arg_count)?;
                    let return_ip = ip
                        .checked_add(args_size)
                        .ok_or(RuntimeError::CodeIndexOutOfBounds)?;

                    // Create call frame
                    let call_frame = CallFrame {
                        return_ip,
                        return_context: vm.current_execution_context.clone(),
                        return_stack_base_pointer_ff: vm.stack_base_pointer_ff,
                        return_stack_base_pointer_i64: vm.stack_base_pointer_i64,
                        return_memory_base_pointer_ff: vm.memory_base_pointer_ff,
                        return_memory_base_pointer_i64: vm.memory_base_pointer_i64,
                    };
                    vm.call_stack.push(call_frame);

                    // Set up a new execution context
                    vm.current_execution_context = ExecutionContext::Function(func_idx);
                    vm.stack_base_pointer_ff = vm.stack_ff.len();
                    vm.stack_base_pointer_i64 = vm.stack_i64.len();
                    vm.memory_base_pointer_ff = vm.memory_ff.len();
                    vm.memory_base_pointer_i64 = vm.memory_i64.len();

                    // Allocate space for function's local variables
                    vm.stack_ff.resize(
                        vm.stack_base_pointer_ff
                            + circuit.functions[func_idx].ff_variable_names.len(),
                        None,
                    );
                    vm.stack_i64.resize(
                        vm.stack_base_pointer_i64
                            + circuit.functions[func_idx].i64_variable_names.len(),
                        None,
                    );

                    // Process arguments and copy to function memory
                    process_function_arguments(&mut vm, &code[ip..], arg_count, component_tree)?;

                    // Switch to function execution context
                    #[cfg(feature = "debug_vm2")]
                    {
                        (code, name, ff_variable_names, i64_variable_names) =
                            get_current_context(&vm, circuit, component_tree);
                    }
                    #[cfg(not(feature = "debug_vm2"))]
                    {
                        code = get_current_context(&vm, circuit, component_tree);
                    }
                    ip = 0; // Start executing function from beginning
                }
                OpCode::FfReturn => {
                    // Pop the return value from stack
                    let return_value = vm.pop_ff()?;

                    // Pop call frame to get return context
                    let call_frame = vm
                        .call_stack
                        .pop()
                        .ok_or(RuntimeError::CallStackUnderflow)?;

                    // Restore execution context
                    ip = call_frame.return_ip;
                    vm.current_execution_context = call_frame.return_context;
                    vm.stack_base_pointer_ff = call_frame.return_stack_base_pointer_ff;
                    vm.stack_base_pointer_i64 = call_frame.return_stack_base_pointer_i64;
                    vm.memory_base_pointer_ff = call_frame.return_memory_base_pointer_ff;
                    vm.memory_base_pointer_i64 = call_frame.return_memory_base_pointer_i64;

                    // Push return value to caller's stack
                    vm.push_ff(return_value);

                    // Switch back to caller's execution context
                    #[cfg(feature = "debug_vm2")]
                    {
                        (code, name, ff_variable_names, i64_variable_names) =
                            get_current_context(&vm, circuit, component_tree);
                    }
                    #[cfg(not(feature = "debug_vm2"))]
                    {
                        code = get_current_context(&vm, circuit, component_tree);
                    }
                }
                OpCode::FfStore => {
                    let addr: usize = vm
                        .pop_i64()?
                        .try_into()
                        .map_err(|_| Box::new(RuntimeError::MemoryAddressOutOfBounds))?;
                    let addr = addr
                        .checked_add(vm.memory_base_pointer_ff)
                        .ok_or(Box::new(RuntimeError::MemoryAddressOutOfBounds))?;
                    if addr >= vm.memory_ff.len() {
                        vm.memory_ff.resize(addr + 1, None);
                    }
                    let value = vm.pop_ff()?;
                    vm.memory_ff[addr] = Some(value);
                    #[cfg(feature = "debug_vm2")]
                    {
                        println!("FfStore: [{}] = {}", addr, vm.memory_ff[addr].unwrap());
                    }
                }
                OpCode::FfLoad => {
                    let addr: usize = vm
                        .pop_i64()?
                        .try_into()
                        .map_err(|_| Box::new(RuntimeError::MemoryAddressOutOfBounds))?;
                    let addr = addr
                        .checked_add(vm.memory_base_pointer_ff)
                        .ok_or(Box::new(RuntimeError::MemoryAddressOutOfBounds))?;
                    if addr >= vm.memory_ff.len() {
                        return Err(Box::new(RuntimeError::MemoryAddressOutOfBounds));
                    }
                    let value = vm
                        .memory_ff
                        .get(addr)
                        .and_then(|v| v.as_ref())
                        .ok_or(RuntimeError::MemoryVariableIsNotSet)?;
                    vm.push_ff(*value);
                }
                OpCode::I64Load => {
                    let addr: usize = vm
                        .pop_i64()?
                        .try_into()
                        .map_err(|_| Box::new(RuntimeError::MemoryAddressOutOfBounds))?;
                    let addr = addr
                        .checked_add(vm.memory_base_pointer_i64)
                        .ok_or(Box::new(RuntimeError::MemoryAddressOutOfBounds))?;
                    if addr >= vm.memory_i64.len() {
                        return Err(Box::new(RuntimeError::MemoryAddressOutOfBounds));
                    }
                    let value = vm
                        .memory_i64
                        .get(addr)
                        .and_then(|v| v.as_ref())
                        .ok_or(RuntimeError::MemoryVariableIsNotSet)?;
                    vm.push_i64(*value);
                }
                OpCode::OpLt => {
                    let lhs = vm.pop_ff()?;
                    let rhs = vm.pop_ff()?;
                    let result = ff.lt(lhs, rhs);
                    #[cfg(feature = "debug_vm2")]
                    {
                        println!("OpLt: {} < {} = {}", lhs, rhs, result);
                    }
                    vm.push_ff(result);
                }
                OpCode::OpLe => {
                    let lhs = vm.pop_ff()?;
                    let rhs = vm.pop_ff()?;
                    let result = ff.lte(lhs, rhs);
                    vm.push_ff(result);
                }
                OpCode::OpGt => {
                    let lhs = vm.pop_ff()?;
                    let rhs = vm.pop_ff()?;
                    let result = ff.gt(lhs, rhs);
                    #[cfg(feature = "debug_vm2")]
                    {
                        println!("OpGt: {} > {} = {}", lhs, rhs, result);
                    }
                    vm.push_ff(result);
                }
                OpCode::OpGe => {
                    let lhs = vm.pop_ff()?;
                    let rhs = vm.pop_ff()?;
                    let result = ff.gte(lhs, rhs);
                    #[cfg(feature = "debug_vm2")]
                    {
                        println!("OpGe: {} >= {} = {}", lhs, rhs, result);
                    }
                    vm.push_ff(result);
                }
                OpCode::OpI64Mul => {
                    let lhs = vm.pop_i64()?;
                    let rhs = vm.pop_i64()?;
                    vm.push_i64(lhs * rhs);
                }
                OpCode::OpI64Lt => {
                    let lhs = vm.pop_i64()?;
                    let rhs = vm.pop_i64()?;
                    vm.push_i64(if lhs < rhs { 1 } else { 0 });
                }
                OpCode::OpI64Lte => {
                    let lhs = vm.pop_i64()?;
                    let rhs = vm.pop_i64()?;
                    vm.push_i64(if lhs <= rhs { 1 } else { 0 });
                }
                OpCode::OpI64Gt => {
                    let lhs = vm.pop_i64()?;
                    let rhs = vm.pop_i64()?;
                    vm.push_i64(if lhs > rhs { 1 } else { 0 });
                }
                OpCode::OpI64Gte => {
                    let lhs = vm.pop_i64()?;
                    let rhs = vm.pop_i64()?;
                    vm.push_i64(if lhs >= rhs { 1 } else { 0 });
                }
                OpCode::I64WrapFf => {
                    let ff_val = vm.pop_ff()?;
                    // Convert field element to i64 by taking lower 64 bits
                    // This matches the behavior expected by i64.wrap_ff
                    let bytes = ff_val.to_le_bytes();
                    let i64_bytes: [u8; 8] = bytes[0..8].try_into().unwrap();
                    let i64_val = i64::from_le_bytes(i64_bytes);
                    vm.push_i64(i64_val);
                }
                OpCode::OpI64Eq => {
                    let rhs = vm.pop_i64()?;
                    let lhs = vm.pop_i64()?;
                    vm.push_i64(if lhs == rhs { 1 } else { 0 });
                }
                OpCode::OpI64Eqz => {
                    let arg = vm.pop_i64()?;
                    vm.push_i64(if arg == 0 { 1 } else { 0 });
                }
                OpCode::OpShr => {
                    let lhs = vm.pop_ff()?;
                    let rhs = vm.pop_ff()?;
                    vm.push_ff(ff.shr(lhs, rhs));
                    #[cfg(feature = "debug_vm2")]
                    {
                        println!("OpShr: {} >> {} = {}", lhs, rhs, vm.peek_ff()?);
                    }
                }
                OpCode::OpShl => {
                    let lhs = vm.pop_ff()?;
                    let rhs = vm.pop_ff()?;
                    vm.push_ff(ff.shl(lhs, rhs));
                    #[cfg(feature = "debug_vm2")]
                    {
                        println!("OpShl: {} << {} = {}", lhs, rhs, vm.peek_ff()?);
                    }
                }
                OpCode::OpBand => {
                    let lhs = vm.pop_ff()?;
                    let rhs = vm.pop_ff()?;
                    vm.push_ff(ff.band(lhs, rhs));
                }
                OpCode::OpAnd => {
                    let lhs = vm.pop_ff()?;
                    let rhs = vm.pop_ff()?;
                    vm.push_ff(ff.land(lhs, rhs));
                }
                OpCode::OpOr => {
                    let lhs = vm.pop_ff()?;
                    let rhs = vm.pop_ff()?;
                    vm.push_ff(ff.lor(lhs, rhs));
                }
                OpCode::OpBxor => {
                    let lhs = vm.pop_ff()?;
                    let rhs = vm.pop_ff()?;
                    vm.push_ff(ff.bxor(lhs, rhs));
                }
                OpCode::OpBor => {
                    let lhs = vm.pop_ff()?;
                    let rhs = vm.pop_ff()?;
                    vm.push_ff(ff.bor(lhs, rhs));
                }
                OpCode::OpBnot => {
                    let operand = vm.pop_ff()?;
                    vm.push_ff(ff.bnot(operand));
                }
                OpCode::OpRem => {
                    let lhs = vm.pop_ff()?;
                    let rhs = vm.pop_ff()?;
                    vm.push_ff(ff.modulo(lhs, rhs));
                }
                OpCode::OpPow => {
                    let base = vm.pop_ff()?;
                    let exponent = vm.pop_ff()?;
                    vm.push_ff(ff.pow(base, exponent));
                }
                OpCode::GetTemplateId => {
                    let cmp_idx = vm.pop_usize()?;
                    let template_id = checked_component(component_tree, cmp_idx)?
                        .read()
                        .unwrap()
                        .template_id as i64;
                    vm.push_i64(template_id);
                }
                OpCode::GetTemplateSignalPosition => {
                    let template_id = vm.pop_usize()?;
                    let signal_id = vm.pop_usize()?;

                    if template_id >= circuit.templates.len() {
                        return Err(Box::new(RuntimeError::InvalidTemplateId(template_id)));
                    }
                    let template = &circuit.templates[template_id];

                    let num_outputs = template.outputs.len();
                    let num_inputs = template.inputs.len();
                    let total_io_signals = num_outputs + num_inputs;

                    if signal_id >= total_io_signals {
                        return Err(Box::new(RuntimeError::SignalIdOutOfBounds(
                            signal_id,
                            total_io_signals,
                        )));
                    }

                    let position = if signal_id < num_outputs {
                        calculate_signal_offset(&template.outputs, signal_id, &circuit.types)?
                    } else {
                        let output_total_size =
                            template.outputs.iter().try_fold(0usize, |acc, sig| {
                                let size = calculate_signal_size(sig, &circuit.types)?;
                                acc.checked_add(size)
                                    .ok_or(RuntimeError::OperationOverflows)
                            })?;
                        let input_offset = calculate_signal_offset(
                            &template.inputs,
                            signal_id - num_outputs,
                            &circuit.types,
                        )?;
                        output_total_size
                            .checked_add(input_offset)
                            .ok_or(RuntimeError::OperationOverflows)?
                    };

                    vm.push_usize(position)?;
                }
                OpCode::GetTemplateSignalSize => {
                    let template_id = vm.pop_usize()?;
                    let signal_id = vm.pop_usize()?;

                    if template_id >= circuit.templates.len() {
                        return Err(Box::new(RuntimeError::InvalidTemplateId(template_id)));
                    }
                    let template = &circuit.templates[template_id];

                    let num_outputs = template.outputs.len();
                    let num_inputs = template.inputs.len();
                    let total_io_signals = num_outputs + num_inputs;

                    if signal_id >= total_io_signals {
                        return Err(Box::new(RuntimeError::SignalIdOutOfBounds(
                            signal_id,
                            total_io_signals,
                        )));
                    }

                    let size = if signal_id < num_outputs {
                        calculate_signal_base_size(&template.outputs[signal_id], &circuit.types)?
                    } else {
                        calculate_signal_base_size(
                            &template.inputs[signal_id - num_outputs],
                            &circuit.types,
                        )?
                    };

                    vm.push_usize(size)?;
                }
                OpCode::GetTemplateSignalType => {
                    let template_id = vm.pop_usize()?;
                    let signal_id = vm.pop_usize()?;

                    if template_id >= circuit.templates.len() {
                        return Err(Box::new(RuntimeError::InvalidTemplateId(template_id)));
                    }
                    let template = &circuit.templates[template_id];

                    let num_outputs = template.outputs.len();
                    let num_inputs = template.inputs.len();
                    let total_io_signals = num_outputs + num_inputs;

                    if signal_id >= total_io_signals {
                        return Err(Box::new(RuntimeError::SignalIdOutOfBounds(
                            signal_id,
                            total_io_signals,
                        )));
                    }

                    let signal = if signal_id < num_outputs {
                        &template.outputs[signal_id]
                    } else {
                        &template.inputs[signal_id - num_outputs]
                    };

                    let type_id: i64 = match signal {
                        Signal::Ff(_) => 0,
                        Signal::Bus(bus_type_id, _) => *bus_type_id as i64 + 1,
                    };

                    vm.push_i64(type_id);

                    #[cfg(feature = "debug_vm2")]
                    {
                        println!(
                            "GetTemplateSignalType: template_id {} type_id {} signal_id {}",
                            template_id, type_id, signal_id
                        );
                    }
                }
                OpCode::GetTemplateSignalDimension => {
                    let template_id = vm.pop_usize()?;
                    let signal_id = vm.pop_usize()?;
                    let dimension_index = vm.pop_usize()?;

                    if template_id >= circuit.templates.len() {
                        return Err(Box::new(RuntimeError::InvalidTemplateId(template_id)));
                    }
                    let template = &circuit.templates[template_id];

                    let num_outputs = template.outputs.len();
                    let num_inputs = template.inputs.len();
                    let total_io_signals = num_outputs + num_inputs;

                    if signal_id >= total_io_signals {
                        return Err(Box::new(RuntimeError::SignalIdOutOfBounds(
                            signal_id,
                            total_io_signals,
                        )));
                    }

                    let signal = if signal_id < num_outputs {
                        &template.outputs[signal_id]
                    } else {
                        &template.inputs[signal_id - num_outputs]
                    };

                    let dims = match signal {
                        Signal::Ff(dims) => dims,
                        Signal::Bus(_, dims) => dims,
                    };

                    if dimension_index >= dims.len() {
                        return Err(Box::new(RuntimeError::DimensionIndexOutOfBounds(
                            dimension_index,
                            dims.len(),
                        )));
                    }

                    vm.push_i64(dims[dimension_index] as i64);
                }
                OpCode::GetBusFieldPosition => {
                    let bus_type_id = vm.pop_usize()?;
                    if bus_type_id == 0 {
                        return Err(Box::new(RuntimeError::InvalidTypeId(0)));
                    }
                    let bus_type_id = bus_type_id - 1;
                    let field_id = vm.pop_usize()?;

                    if bus_type_id >= circuit.types.len() {
                        return Err(Box::new(RuntimeError::InvalidTypeId(bus_type_id + 1)));
                    }

                    let bus_type = &circuit.types[bus_type_id];

                    if field_id >= bus_type.fields.len() {
                        return Err(Box::new(RuntimeError::SignalIdOutOfBounds(
                            field_id,
                            bus_type.fields.len(),
                        )));
                    }

                    let position = bus_type.fields[field_id].offset;
                    vm.push_i64(position as i64);

                    #[cfg(feature = "debug_vm2")]
                    {
                        println!(
                            "GetBusFieldPosition: bus_type {} field {} => offset {}",
                            bus_type_id, field_id, position
                        );
                    }
                }
                OpCode::GetBusFieldSize => {
                    let bus_type_id = vm.pop_usize()?;
                    let field_id = vm.pop_usize()?;

                    if bus_type_id == 0 {
                        return Err(Box::new(RuntimeError::InvalidTypeId(0)));
                    }
                    let bus_type_id = bus_type_id - 1;
                    if bus_type_id >= circuit.types.len() {
                        return Err(Box::new(RuntimeError::InvalidTypeId(bus_type_id + 1)));
                    }

                    let bus_type = &circuit.types[bus_type_id];

                    if field_id >= bus_type.fields.len() {
                        return Err(Box::new(RuntimeError::SignalIdOutOfBounds(
                            field_id,
                            bus_type.fields.len(),
                        )));
                    }

                    let size = bus_type.fields[field_id].base_type_size;
                    vm.push_i64(size as i64);

                    #[cfg(feature = "debug_vm2")]
                    {
                        println!(
                            "GetBusFieldSize: bus_type {} field {} => size {}",
                            bus_type_id, field_id, size
                        );
                    }
                }
                OpCode::GetBusFieldType => {
                    let bus_type_id = vm.pop_usize()?;
                    let field_id = vm.pop_usize()?;

                    if bus_type_id == 0 {
                        return Err(Box::new(RuntimeError::InvalidTypeId(0)));
                    }

                    // type ID 0 is FF. Buses start from index 1
                    let bus_type_id = bus_type_id - 1;

                    if bus_type_id >= circuit.types.len() {
                        return Err(Box::new(RuntimeError::InvalidTypeId(bus_type_id + 1)));
                    }

                    let bus_type = &circuit.types[bus_type_id];

                    if field_id >= bus_type.fields.len() {
                        return Err(Box::new(RuntimeError::InvalidFieldId(
                            bus_type_id + 1,
                            field_id,
                        )));
                    }

                    let field = &bus_type.fields[field_id];
                    let type_id = match field.kind {
                        TypeFieldKind::Ff => 0,
                        TypeFieldKind::Bus(bus_idx) => bus_idx
                            .checked_add(1)
                            .ok_or(Box::new(RuntimeError::OperationOverflows))?,
                    };

                    vm.push_usize(type_id)?;

                    #[cfg(feature = "debug_vm2")]
                    {
                        println!(
                            "GetBusFieldType: bus_type {} field {} => type {}",
                            bus_type_id, field_id, type_id
                        );
                    }
                }
                OpCode::GetBusFieldDimension => {
                    let dimension_idx = vm.pop_usize()?;
                    let field_id = vm.pop_usize()?;
                    let bus_type_id = vm.pop_usize()?;

                    if bus_type_id == 0 {
                        return Err(Box::new(RuntimeError::InvalidTypeId(0)));
                    }

                    // type ID 0 is FF. Buses start from index 1
                    let bus_type_id = bus_type_id - 1;

                    if bus_type_id >= circuit.types.len() {
                        return Err(Box::new(RuntimeError::InvalidTypeId(bus_type_id + 1)));
                    }

                    let bus_type = &circuit.types[bus_type_id];

                    if field_id >= bus_type.fields.len() {
                        return Err(Box::new(RuntimeError::SignalIdOutOfBounds(
                            field_id,
                            bus_type.fields.len(),
                        )));
                    }

                    let field = &bus_type.fields[field_id];

                    let dims = &field.dims;
                    if dimension_idx >= dims.len() {
                        return Err(Box::new(RuntimeError::DimensionIndexOutOfBounds(
                            dimension_idx,
                            dims.len(),
                        )));
                    }

                    let dim_length = dims[dimension_idx] as i64;
                    vm.push_i64(dim_length);

                    #[cfg(feature = "debug_vm2")]
                    println!(
                        "GetBusFieldDimension: bus_type={} field={} dim[{}]={}",
                        bus_type_id, field_id, dimension_idx, dim_length
                    );
                }
                OpCode::CopyCmpInputsFromSelf => {
                    let flags = read_byte_advance(code, &mut ip)?;
                    let cmp_idx = vm.pop_usize()?;
                    let cmp_sig_idx = vm.pop_usize()?;
                    let self_sig_idx = vm.pop_usize()?;
                    let num_signals = vm.pop_usize()?;
                    let component = checked_component(component_tree, cmp_idx)?;

                    for offset in 0..num_signals {
                        let src_idx = checked_signal_index(self_sig_idx, offset)?;
                        let dst_idx = checked_signal_index(cmp_sig_idx, offset)?;
                        let value = component_tree.get_signal(src_idx)?;

                        #[cfg(feature = "debug_vm2")]
                        {
                            let c = component.read().unwrap();
                            println!(
                                "CopyCmpInputsFromSelf [S{} -> S{}]: cmp {} ({}) sig {} = {}",
                                component_tree.signals_start + src_idx,
                                c.signals_start + dst_idx,
                                cmp_idx,
                                circuit.templates[c.template_id].name,
                                dst_idx,
                                value
                            );
                        }

                        component.write().unwrap().set_signal(dst_idx, value)?;
                    }

                    let mode = flags & 0b11;
                    let mut should_run = false;

                    match mode {
                        0b00 => {}
                        0b01 => {
                            component.write().unwrap().decrement_inputs(num_signals)?;
                        }
                        0b10 => {
                            should_run = true;
                        }
                        0b11 => {
                            let mut c = component.write().unwrap();
                            c.decrement_inputs(num_signals)?;
                            if c.number_of_inputs == 0 {
                                should_run = true;
                            }
                        }
                        _ => {}
                    }

                    #[cfg(feature = "debug_vm2")]
                    {
                        let c = component.read().unwrap();
                        println!(
                            "CopyCmpInputsFromSelf: cmp {} ({}) inputs left: {}",
                            cmp_idx, circuit.templates[c.template_id].name, c.number_of_inputs
                        );
                    }

                    if should_run {
                        #[cfg(feature = "debug_vm2")]
                        {
                            println!("CopyCmpInputsFromSelf: Run component {}", cmp_idx);
                        }
                        #[cfg(feature = "parallel_components")]
                        spawn_component_execution(scope, circuit, ff, component.clone());
                        #[cfg(not(feature = "parallel_components"))]
                        {
                            let mut component = component
                                .write()
                                .map_err(|e| format!("Failed to lock component: {}", e))?;
                            execute(circuit, ff, &mut component)?;
                        }
                    }
                }
                OpCode::CopyCmpInputsFromCmp => {
                    let flags = read_byte_advance(code, &mut ip)?;

                    let dst_cmp_idx = vm.pop_usize()?;
                    let dst_sig_idx = vm.pop_usize()?;
                    let src_cmp_idx = vm.pop_usize()?;
                    let src_sig_idx = vm.pop_usize()?;
                    let num_signals = vm.pop_usize()?;
                    let src_component = checked_component(component_tree, src_cmp_idx)?;
                    let dst_component = checked_component(component_tree, dst_cmp_idx)?;

                    for offset in 0..num_signals {
                        let src_idx = checked_signal_index(src_sig_idx, offset)?;
                        let dst_idx = checked_signal_index(dst_sig_idx, offset)?;
                        let value = src_component.write().unwrap().get_signal(src_idx)?;
                        dst_component.write().unwrap().set_signal(dst_idx, value)?;

                        #[cfg(feature = "debug_vm2")]
                        {
                            let src_index_start = src_component.read().unwrap().signals_start;
                            let dst_index_start = dst_component.read().unwrap().signals_start;
                            println!(
                                "CopyCmpInputsFromCmp [cmp {} S{} {} -> cmp {} S{} {}] = {}",
                                src_cmp_idx,
                                src_index_start + src_idx,
                                src_idx,
                                dst_cmp_idx,
                                dst_index_start + dst_idx,
                                dst_idx,
                                value
                            );
                        }
                    }

                    let mode = flags & 0b11;
                    let mut should_run = false;

                    match mode {
                        0b00 => {}
                        0b01 => {
                            dst_component
                                .write()
                                .unwrap()
                                .decrement_inputs(num_signals)?;
                        }
                        0b10 => {
                            should_run = true;
                        }
                        0b11 => {
                            let mut c = dst_component.write().unwrap();
                            c.decrement_inputs(num_signals)?;
                            if c.number_of_inputs == 0 {
                                should_run = true;
                            }
                        }
                        _ => {}
                    }

                    #[cfg(feature = "debug_vm2")]
                    {
                        let c = dst_component.read().unwrap();
                        println!(
                            "CopyCmpInputsFromCmp: cmp {} inputs left: {}, template: {}",
                            dst_cmp_idx, c.number_of_inputs, circuit.templates[c.template_id].name
                        );
                    }

                    if should_run {
                        #[cfg(feature = "debug_vm2")]
                        {
                            println!("CopyCmpInputsFromCmp: Run component {}", dst_cmp_idx);
                        }
                        #[cfg(feature = "parallel_components")]
                        spawn_component_execution(scope, circuit, ff, dst_component.clone());

                        #[cfg(not(feature = "parallel_components"))]
                        {
                            let mut component = dst_component
                                .write()
                                .map_err(|e| format!("Failed to lock component: {}", e))?;
                            execute(circuit, ff, &mut component)?;
                        }
                    }
                }
                OpCode::CopyCmpInputsFromMemory => {
                    let flags = read_byte_advance(code, &mut ip)?;

                    let dst_cmp_idx = vm.pop_usize()?;
                    let dst_sig_idx = vm.pop_usize()?;
                    let sig_addr = vm.pop_usize()?;
                    let num_signals = vm.pop_usize()?;
                    let dst_component = checked_component(component_tree, dst_cmp_idx)?;

                    let memory_start = vm
                        .memory_base_pointer_ff
                        .checked_add(sig_addr)
                        .ok_or(RuntimeError::MemoryAddressOutOfBounds)?;

                    for offset in 0..num_signals {
                        let src_idx = memory_start
                            .checked_add(offset)
                            .ok_or(RuntimeError::MemoryAddressOutOfBounds)?;
                        let dst_idx = checked_signal_index(dst_sig_idx, offset)?;
                        if src_idx >= vm.memory_ff.len() {
                            return Err(Box::new(RuntimeError::MemoryAddressOutOfBounds));
                        }

                        let value = vm
                            .memory_ff
                            .get(src_idx)
                            .and_then(|v| v.as_ref())
                            .ok_or(RuntimeError::MemoryVariableIsNotSet)?;

                        dst_component.write().unwrap().set_signal(dst_idx, *value)?;

                        #[cfg(feature = "debug_vm2")]
                        {
                            let dst_idx_global =
                                dst_component.read().unwrap().signals_start + dst_idx;
                            println!(
                                "CopyCmpInputsFromMemory: M{} -> cmp {} S{} (global S{}), value={}",
                                src_idx, dst_cmp_idx, dst_idx, dst_idx_global, *value
                            );
                        }
                    }

                    let mode = flags & 0b11;
                    let mut should_run = false;
                    match mode {
                        0b00 => {}
                        0b01 => {
                            dst_component
                                .write()
                                .unwrap()
                                .decrement_inputs(num_signals)?;
                        }
                        0b10 => {
                            should_run = true;
                        }
                        0b11 => {
                            let mut c = dst_component.write().unwrap();
                            c.decrement_inputs(num_signals)?;
                            if c.number_of_inputs == 0 {
                                should_run = true;
                            }
                        }
                        _ => {}
                    }

                    #[cfg(feature = "debug_vm2")]
                    {
                        let c = dst_component.read().unwrap();
                        println!(
                            "CopyCmpInputsFromMemory: cmp {} inputs left: {}, template: {}",
                            dst_cmp_idx, c.number_of_inputs, circuit.templates[c.template_id].name
                        );
                    }

                    if should_run {
                        #[cfg(feature = "debug_vm2")]
                        {
                            println!("CopyCmpInputsFromMemory: Run component {}", dst_cmp_idx);
                        }
                        #[cfg(feature = "parallel_components")]
                        spawn_component_execution(scope, circuit, ff, dst_component.clone());
                        #[cfg(not(feature = "parallel_components"))]
                        {
                            let mut component = dst_component
                                .write()
                                .map_err(|e| format!("Failed to lock component: {}", e))?;
                            execute(circuit, ff, &mut component)?;
                        }
                    }
                }
                OpCode::CopySignal => {
                    let dst_idx = vm.pop_usize()?;
                    let src_idx = vm.pop_usize()?;
                    let num_signals = vm.pop_usize()?;

                    for offset in 0..num_signals {
                        let src_idx = checked_signal_index(src_idx, offset)?;
                        let dst_idx = checked_signal_index(dst_idx, offset)?;
                        let value = component_tree.get_signal(src_idx)?;
                        component_tree.set_signal(dst_idx, value)?;
                        #[cfg(feature = "debug_vm2")]
                        {
                            let src_global = component_tree.signals_start + src_idx;
                            let dst_global = component_tree.signals_start + dst_idx;
                            println!(
                                "CopySignal [S{} -> S{}] = {}",
                                src_global, dst_global, value
                            );
                        }
                    }
                }
                OpCode::CopySignalFromCmp => {
                    let dst_idx = vm.pop_usize()?;
                    let cmp_idx = vm.pop_usize()?;
                    let cmp_sig_idx = vm.pop_usize()?;
                    let num_signals = vm.pop_usize()?;
                    let component = checked_component(component_tree, cmp_idx)?;

                    for offset in 0..num_signals {
                        let src_idx = checked_signal_index(cmp_sig_idx, offset)?;
                        let dst_idx = checked_signal_index(dst_idx, offset)?;
                        let value = component.read().unwrap().get_signal(src_idx)?;
                        component_tree.set_signal(dst_idx, value)?;

                        #[cfg(feature = "debug_vm2")]
                        {
                            let c = component.read().unwrap();
                            let src_sig_idx = c.signals_start + src_idx;
                            let dst_sig_idx = component_tree.signals_start + dst_idx;
                            println!(
                                "CopySignalFromCmp [S{} -> S{}]: cmp {} sig {} = {}",
                                src_sig_idx, dst_sig_idx, cmp_idx, src_idx, value,
                            );
                        }
                    }
                }
                OpCode::CopySignalFromMemory => {
                    let dst_idx = vm.pop_usize()?;
                    let addr = vm.pop_usize()?;
                    let num_signals = vm.pop_usize()?;

                    let memory_start = vm
                        .memory_base_pointer_ff
                        .checked_add(addr)
                        .ok_or(RuntimeError::MemoryAddressOutOfBounds)?;

                    for offset in 0..num_signals {
                        let src_idx = memory_start
                            .checked_add(offset)
                            .ok_or(RuntimeError::MemoryAddressOutOfBounds)?;
                        if src_idx >= vm.memory_ff.len() {
                            return Err(Box::new(RuntimeError::MemoryAddressOutOfBounds));
                        }
                        let value = vm
                            .memory_ff
                            .get(src_idx)
                            .and_then(|v| v.as_ref())
                            .ok_or(RuntimeError::MemoryVariableIsNotSet)?;

                        let dst_idx = checked_signal_index(dst_idx, offset)?;
                        component_tree.set_signal(dst_idx, *value)?;

                        #[cfg(feature = "debug_vm2")]
                        {
                            let dst_idx_global = component_tree.signals_start + dst_idx;
                            println!(
                                "CopySignalFromMemory [M{} -> S{}]: value = {}",
                                src_idx, dst_idx_global, value,
                            );
                        }
                    }
                }
            }
        }
        Ok(())
    })?;

    Ok(())
}

#[derive(Debug, Clone)]
#[cfg_attr(test, derive(PartialEq))]
pub struct Type {
    pub name: String,
    pub fields: Vec<TypeField>,
}

impl Type {
    pub fn get_total_size(&self) -> Result<usize, RuntimeError> {
        self.fields.iter().try_fold(0usize, |acc, field| {
            let field_size = field.get_total_size()?;
            acc.checked_add(field_size)
                .ok_or(RuntimeError::OperationOverflows)
        })
    }
}

#[derive(Debug, Clone)]
#[cfg_attr(test, derive(PartialEq))]
pub struct TypeField {
    pub name: String,
    pub kind: TypeFieldKind,
    pub offset: usize,
    pub base_type_size: usize,
    pub dims: Vec<usize>,
}

impl TypeField {
    pub fn get_total_size(&self) -> Result<usize, RuntimeError> {
        let dim_product = checked_product(&self.dims)?;
        if dim_product == 0 {
            Ok(self.base_type_size)
        } else {
            self.base_type_size
                .checked_mul(dim_product)
                .ok_or(RuntimeError::OperationOverflows)
        }
    }
}

#[derive(Debug, Clone)]
#[cfg_attr(test, derive(PartialEq))]
pub enum TypeFieldKind {
    Ff,
    Bus(usize), // Index into the types vector
}

// Type conversion functions that require type name to index mapping
impl Type {
    pub fn from_ast(ast_type: &crate::ast::Type, type_map: &HashMap<String, usize>) -> Self {
        Type {
            name: ast_type.name.clone(),
            fields: ast_type.fields.iter()
                .map(|field| TypeField::from_ast(field, type_map))
                .collect(),
        }
    }
}

impl TypeField {
    pub fn from_ast(ast_field: &crate::ast::TypeField, type_map: &HashMap<String, usize>) -> Self {
        TypeField {
            name: ast_field.name.clone(),
            kind: match &ast_field.kind {
                crate::ast::TypeFieldKind::Ff => TypeFieldKind::Ff,
                crate::ast::TypeFieldKind::Bus(name) => {
                    let index = type_map.get(name)
                        .unwrap_or_else(|| panic!("Bus type '{}' not found in type map", name));
                    TypeFieldKind::Bus(*index)
                },
            },
            offset: ast_field.offset,
            base_type_size: ast_field.base_type_size,
            dims: ast_field.dims.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    // use bitvec::vec::BitVec;
    use super::{OpCode, RuntimeError, read_instruction};
    use super::disassemble_instruction_to_string;
    use crate::field::U254;
    use bitvec::prelude::*;

    #[test]
    fn test_ok() {
        let mut x = BitVec::<usize, Lsb0>::repeat(false, 100500);

        println!("{:?}", x[4000]);
        println!("{:?}", x[4001]);

        x.set(4000, true);
        println!("{:?}", x[4000]);

        println!("{:?}", x[4001]);
        println!("OK");
    }

    #[test]
    fn opcode_try_from_rejects_out_of_range_bytes() {
        // Every valid discriminant decodes; bytes past the last one are
        // rejected instead of being interpreted as opcodes.
        for byte in 0..=(OpCode::OpI64Eqz as u8) {
            assert!(OpCode::try_from(byte).is_ok(), "byte {byte} should decode");
        }
        for byte in (OpCode::OpI64Eqz as u8 + 1)..=u8::MAX {
            assert!(matches!(
                OpCode::try_from(byte),
                Err(RuntimeError::InvalidOpCode(b)) if b == byte));
        }
    }

    #[test]
    fn read_instruction_reports_out_of_bounds_ip() {
        assert!(matches!(
            read_instruction(&[], 0),
            Err(RuntimeError::CodeIndexOutOfBounds)));
        assert!(matches!(
            read_instruction(&[OpCode::NoOp as u8], 1),
            Err(RuntimeError::CodeIndexOutOfBounds)));
    }

    #[test]
    fn disassemble_instruction_reports_truncated_operands() {
        let (_, disassembled) = disassemble_instruction_to_string::<U254>(
            &[OpCode::PushI64 as u8],
            0,
            "Main",
            &[],
            &[],
        );

        assert!(disassembled.contains("decode error"));
        assert!(disassembled.contains("Code range is out of bounds"));
    }
}
