use std::{collections::HashMap, ops::Div};
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
    // Memory return operation
    // Copy data from source memory to destination memory
    // stack_i64:0 contains the size (number of elements)
    // stack_i64:-1 contains the source address
    // stack_i64:-2 contains the destination address
    I64MReturn            = 89,
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
    // Return from call, passing an i64 value (i64.return)
    // stack_i64:0 contains the return value
    // Restores caller's execution context and pushes value to caller's stack_i64
    I64Return              = 88,
    // Store i64 value (i64.store)
    // stack_i64:0  contains right operand  (address)
    // stack_i64:-1 contains left operand   (value)
    // Effect: memory_i64[address] = value
    // No value is pushed to stack_i64
    I64Store              = 86,
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
    // Integer division (i64.div)
    // stack_i64:0 contains right operand
    // stack_i64:-1 contains left operand
    // Result pushed to stack_i64
    OpI64Div             = 74,
    // Bitwise AND (i64.and)
    // stack_i64:0  contains right operand
    // stack_i64:-1 contains left operand
    // Result (left & right) pushed to stack_i64
    OpI64And             = 75,
    // Bitwise OR (i64.or)
    // stack_i64:0  contains right operand
    // stack_i64:-1 contains left operand
    // Result (left | right) is pushed to stack_i64
    OpI64Or             = 76,
    // Integer exponentiation (i64.pow)
    // stack_i64:0  contains right operand (exponent)
    // stack_i64:-1 contains left operand  (base)
    // Result (base ^ exponent) is pushed to stack_i64
    OpI64Pow             = 78,
    // Integer remainder (i64.rem)
    // stack_i64:0  contains right operand  (divisor)
    // stack_i64:-1 contains left operand   (numerator)
    // Result (left % right) is pushed to stack_i64
    OpI64Rem             = 79,
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
    // Left shift (i64.shl)
    // stack_i64:0  contains right operand  (shift amount)
    // stack_i64:-1 contains left operand   (value)
    // Result (value << shift) is pushed to stack_i64
    OpI64Shl             = 80,
    // Arithmetic right shift (i64.shr)
    // stack_i64:0  contains right operand  (shift amount)
    // stack_i64:-1 contains left operand   (value)
    // Result (value >> shift) is pushed to stack_i64
    OpI64Shr             = 81,
    // Bitwise NOT (i64.bnot)
    // stack_i64:0 contains the operand
    // Result (~value) is pushed to stack_i64
    OpI64BNot            = 82,
    // Bitwise XOR (i64.bxor)
    // stack_i64:0  contains right operand
    // stack_i64:-1 contains left operand
    // Result (left ^ right) is pushed to stack_i64
    OpI64BXor            = 83,
    // Bitwise OR (i64.bor)
    // stack_i64:0  contains right operand
    // stack_i64:-1 contains left operand
    // Result (left | right) is pushed to stack_i64
    OpI64BOr             = 84,
    // Bitwise AND (i64.band)
    // stack_i64:0  contains right operand
    // stack_i64:-1 contains left operand
    // Result (left & right) is pushed to stack_i64
    OpI64BAnd            = 85,
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
    // Convert i64 to field element (ff.extend_i64)
    // stack_i64:0 contains the operand (i64)
    // Result (FF(value)) is pushed to stack_ff
    FfExtendI64             = 87,
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
    GetBusSignalPosition = 64,
    GetBusSignalSize = 65,
    OpI64Eq = 66,
    // Inequality comparison (i64.neq)
    // stack_i64:0  contains right operand
    // stack_i64:-1 contains left operand
    // Result (left != right ? 1 : 0) is pushed to stack_i64
    OpI64Neq = 77,
    FfMStoreFromCmpSignal = 67,
    GetBusSignalType = 68,
    GetBusSignalDimension = 69,
    OpI64Gt = 71,
    OpI64Gte = 72,
    OpI64Eqz = 73,
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

fn calculate_signal_size(signal: &Signal, types: &[Type]) -> usize {
    match signal {
        Signal::Ff(dims) => {
            if dims.is_empty() { 1 } else { dims.iter().product() }
        }
        Signal::Bus(type_idx, dims) => {
            let bus_type = &types[*type_idx];
            let base_size: usize = bus_type.fields.iter().map(|f| f.size).sum();
            if dims.is_empty() { base_size } else { base_size * dims.iter().product::<usize>() }
        }
    }
}

fn calculate_signal_offset(signals: &[Signal], signal_id: usize, types: &[Type]) -> usize {
    signals.iter().take(signal_id)
        .map(|sig| calculate_signal_size(sig, types))
        .sum()
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

fn read_instruction(code: &[u8], ip: usize) -> OpCode {
    unsafe { std::mem::transmute::<u8, OpCode>(code[ip]) }
}

// read 4 bytes from the code and return usize and the next instruction pointer
fn read_usize32(code: &[u8], ip: usize) -> (usize, usize) {
    let slice = code.get(ip..ip + 4)
        .expect("Code index out of bounds for usize32 read");
    let bytes: [u8; 4] = slice.try_into()
        .expect("Failed to convert slice to [u8; 4]");
    let v = u32::from_le_bytes(bytes) as usize;
    (v, ip + 4)
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
    #[error("Signal index is out of bounds")]
    SignalIndexOutOfBounds,
    #[error("Signal is not set:\n{0}")]
    SignalIsNotSet(String),
    #[error("Signal is already set")]
    SignalIsAlreadySet,
    #[error("Code index is out of bounds")]
    CodeIndexOutOfBounds,
    #[error("component is not initialized")]
    UninitializedComponent,
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
    #[error("Signal ID {0} is out of bounds (max {1})")]
    SignalIdOutOfBounds(usize, usize),
    #[error("Dimension index {0} is out of bounds (signal has {1} dimensions)")]
    DimensionIndexOutOfBounds(usize, usize),
    #[error("Attempt divide by zero")]
    AttemptDivideByZeroComponent,
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

    fn pop_usize(&mut self) -> Result<usize, RuntimeError> {
        self.pop_i64()?
            .try_into()
            .map_err(|_| RuntimeError::I32ToUsizeConversion)
    }

}

// Helper function to calculate the size of function arguments in bytecode
fn calculate_args_size<T: FieldOps>(code: &[u8], arg_count: u8) -> Result<usize, RuntimeError> {
    let mut offset = 0;
    for _ in 0..arg_count {
        if offset >= code.len() {
            return Err(RuntimeError::CodeIndexOutOfBounds);
        }
        
        let arg_type = code[offset];
        offset += 1;
        
        match arg_type {
            0 => offset += 8,  // i64 literal
            1 => offset += T::BYTES, // ff literal
            2 => offset += 8,  // ff variable
            3 => offset += 8,  // i64 variable
            4..=7 => offset += 16, // ff.memory (addr + size, both i64)
            8..=11 => offset += 16, // i64.memory (addr + size, both i64)
            12..=15 => offset += 16, // signal (idx + size, both i64)
            16..=23 => offset += 24, // subcomponent signal (cmp_idx + sig_idg + size, all i64)
            _ => return Err(RuntimeError::CodeIndexOutOfBounds),
        }
    }
    Ok(offset)
}

// Helper function to process function arguments
fn process_function_arguments<T: FieldOps>(
    vm: &mut VM<T>, code: &[u8], arg_count: u8,
    component_tree: &Component<T>) -> Result<(), Box<dyn Error + Sync + Send>> {

    let mut offset = 0;
    let mut ff_arg_idx = 0;
    let mut i64_arg_idx = 0;
    
    for _ in 0..arg_count {
        if offset >= code.len() {
            return Err(Box::new(RuntimeError::CodeIndexOutOfBounds));
        }
        
        let arg_type = code[offset];
        offset += 1;
        
        match arg_type {
            0 => { // i64 literal
                let value = i64::from_le_bytes(code[offset..offset+8].try_into().unwrap());
                offset += 8;
                
                // Store in function's memory
                if vm.memory_i64.len() <= vm.memory_base_pointer_i64 + i64_arg_idx {
                    vm.memory_i64.resize(vm.memory_base_pointer_i64 + i64_arg_idx + 1, None);
                }
                vm.memory_i64[vm.memory_base_pointer_i64 + i64_arg_idx] = Some(value);
                i64_arg_idx += 1;
            }
            1 => { // ff literal  
                let value = T::from_le_bytes(&code[offset..offset+T::BYTES]).unwrap();
                offset += T::BYTES;
                
                // Store in function's memory
                if vm.memory_ff.len() <= vm.memory_base_pointer_ff + ff_arg_idx {
                    vm.memory_ff.resize(vm.memory_base_pointer_ff + ff_arg_idx + 1, None);
                }
                vm.memory_ff[vm.memory_base_pointer_ff + ff_arg_idx] = Some(value);
                ff_arg_idx += 1;
            }
            2 => { // ff variable
                let var_idx = i64::from_le_bytes(code[offset..offset+8].try_into().unwrap()) as usize;
                offset += 8;
                
                // Get caller's context from the call frame we just pushed
                let frame = vm.call_stack.last()
                    .ok_or(RuntimeError::CallStackUnderflow)?;
                let caller_stack_base = frame.return_stack_base_pointer_ff;
                
                // Load value from caller's ff variable stack
                let value = *vm.stack_ff.get(caller_stack_base + var_idx)
                    .and_then(|v| v.as_ref())
                    .ok_or(RuntimeError::StackVariableIsNotSet)?;
                
                // Store in function's memory
                if vm.memory_ff.len() <= vm.memory_base_pointer_ff + ff_arg_idx {
                    vm.memory_ff.resize(vm.memory_base_pointer_ff + ff_arg_idx + 1, None);
                }
                vm.memory_ff[vm.memory_base_pointer_ff + ff_arg_idx] = Some(value);
                ff_arg_idx += 1;
            }
            3 => { // i64 variable
                let var_idx = i64::from_le_bytes(code[offset..offset+8].try_into().unwrap()) as usize;
                offset += 8;
                
                // Get caller's context from the call frame we just pushed
                let frame = vm.call_stack.last()
                    .ok_or(RuntimeError::CallStackUnderflow)?;
                let caller_stack_base = frame.return_stack_base_pointer_i64;
                
                // Load value from caller's i64 variable stack
                let value = *vm.stack_i64.get(caller_stack_base + var_idx)
                    .and_then(|v| v.as_ref())
                    .ok_or(RuntimeError::StackVariableIsNotSet)?;
                
                // Store in function's memory
                if vm.memory_i64.len() <= vm.memory_base_pointer_i64 + i64_arg_idx {
                    vm.memory_i64.resize(vm.memory_base_pointer_i64 + i64_arg_idx + 1, None);
                }
                vm.memory_i64[vm.memory_base_pointer_i64 + i64_arg_idx] = Some(value);
                i64_arg_idx += 1;
            }
            4..=7 => { // ff.memory argument
                // Decode bit flags
                let addr_is_variable = (arg_type & 1) != 0;
                let size_is_variable = (arg_type & 2) != 0;
                
                // Get caller's context from the call frame we just pushed
                let frame = vm.call_stack.last()
                    .ok_or(RuntimeError::CallStackUnderflow)?;
                let caller_base_pointer_ff = frame.return_memory_base_pointer_ff;
                let caller_stack_base = frame.return_stack_base_pointer_i64;
                
                // Read and resolve address
                let src_addr = if addr_is_variable {
                    // It's a variable index - need to load from caller's stack
                    let var_idx = i64::from_le_bytes(code[offset..offset+8].try_into().unwrap()) as usize;
                    offset += 8;
                    
                    *vm.stack_i64.get(caller_stack_base + var_idx)
                        .and_then(|v| v.as_ref())
                        .ok_or(RuntimeError::StackVariableIsNotSet)? as usize
                } else {
                    // It's a literal value
                    let value = i64::from_le_bytes(code[offset..offset+8].try_into().unwrap());
                    offset += 8;
                    value as usize
                };
                
                // Read and resolve size
                let size = if size_is_variable {
                    // It's a variable index - need to load from caller's stack
                    let var_idx = i64::from_le_bytes(code[offset..offset+8].try_into().unwrap()) as usize;
                    offset += 8;
                    
                    *vm.stack_i64.get(caller_stack_base + var_idx)
                        .and_then(|v| v.as_ref())
                        .ok_or(RuntimeError::StackVariableIsNotSet)? as usize
                } else {
                    // It's a literal value
                    let value = i64::from_le_bytes(code[offset..offset+8].try_into().unwrap());
                    offset += 8;
                    value as usize
                };
                
                // Add caller's base pointer to source address
                let src_addr = src_addr + caller_base_pointer_ff;
                
                // Ensure source memory is valid
                if src_addr + size > vm.memory_ff.len() {
                    return Err(Box::new(RuntimeError::MemoryAddressOutOfBounds));
                }
                
                // Copy from caller's memory to function's memory
                let dst_base = vm.memory_base_pointer_ff + ff_arg_idx;
                if vm.memory_ff.len() <= dst_base + size {
                    vm.memory_ff.resize(dst_base + size, None);
                }
                
                for i in 0..size {
                    vm.memory_ff[dst_base + i] = vm.memory_ff[src_addr + i];
                }
                
                ff_arg_idx += size;
            }
            8..=11 => { // i64.memory argument
                // Decode bit flags
                let addr_is_variable = (arg_type & 1) != 0;
                let size_is_variable = (arg_type & 2) != 0;
                
                // Get caller's context from the call frame we just pushed
                let frame = vm.call_stack.last()
                    .ok_or(RuntimeError::CallStackUnderflow)?;
                let caller_base_pointer_i64 = frame.return_memory_base_pointer_i64;
                let caller_stack_base = frame.return_stack_base_pointer_i64;
                
                // Read and resolve address
                let src_addr = if addr_is_variable {
                    // It's a variable index - need to load from caller's stack
                    let var_idx = i64::from_le_bytes(code[offset..offset+8].try_into().unwrap()) as usize;
                    offset += 8;
                    
                    *vm.stack_i64.get(caller_stack_base + var_idx)
                        .and_then(|v| v.as_ref())
                        .ok_or(RuntimeError::StackVariableIsNotSet)? as usize
                } else {
                    // It's a literal value
                    let value = i64::from_le_bytes(code[offset..offset+8].try_into().unwrap());
                    offset += 8;
                    value as usize
                };
                
                // Read and resolve size
                let size = if size_is_variable {
                    // It's a variable index - need to load from caller's stack
                    let var_idx = i64::from_le_bytes(code[offset..offset+8].try_into().unwrap()) as usize;
                    offset += 8;
                    
                    *vm.stack_i64.get(caller_stack_base + var_idx)
                        .and_then(|v| v.as_ref())
                        .ok_or(RuntimeError::StackVariableIsNotSet)? as usize
                } else {
                    // It's a literal value
                    let value = i64::from_le_bytes(code[offset..offset+8].try_into().unwrap());
                    offset += 8;
                    value as usize
                };
                
                // Add caller's base pointer to source address
                let src_addr = src_addr + caller_base_pointer_i64;
                
                // Ensure source memory is valid
                if src_addr + size > vm.memory_i64.len() {
                    return Err(Box::new(RuntimeError::MemoryAddressOutOfBounds));
                }
                
                // Copy from caller's memory to function's memory
                let dst_base = vm.memory_base_pointer_i64 + i64_arg_idx;
                if vm.memory_i64.len() <= dst_base + size {
                    vm.memory_i64.resize(dst_base + size, None);
                }
                
                for i in 0..size {
                    vm.memory_i64[dst_base + i] = vm.memory_i64[src_addr + i];
                }
                
                i64_arg_idx += size;
            }
            12..=15 => { // signal argument (only valid in component context)
                // Decode bit flags
                let idx_is_variable = (arg_type & 1) != 0;
                let size_is_variable = (arg_type & 2) != 0;
                
                // Get caller's context from the call frame we just pushed
                let frame = vm.call_stack.last()
                    .ok_or(RuntimeError::CallStackUnderflow)?;
                let caller_stack_base = frame.return_stack_base_pointer_i64;
                
                // Read and resolve signal index
                let signal_idx = if idx_is_variable {
                    // It's a variable index - need to load from caller's stack
                    let var_idx = i64::from_le_bytes(code[offset..offset+8].try_into().unwrap()) as usize;
                    offset += 8;
                    
                    *vm.stack_i64.get(caller_stack_base + var_idx)
                        .and_then(|v| v.as_ref())
                        .ok_or(RuntimeError::StackVariableIsNotSet)? as usize
                } else {
                    // It's a literal value
                    let value = i64::from_le_bytes(code[offset..offset+8].try_into().unwrap());
                    offset += 8;
                    value as usize
                };
                
                // Read and resolve size
                let size = if size_is_variable {
                    // It's a variable index - need to load from caller's stack
                    let var_idx = i64::from_le_bytes(code[offset..offset+8].try_into().unwrap()) as usize;
                    offset += 8;
                    
                    *vm.stack_i64.get(caller_stack_base + var_idx)
                        .and_then(|v| v.as_ref())
                        .ok_or(RuntimeError::StackVariableIsNotSet)? as usize
                } else {
                    // It's a literal value
                    let value = i64::from_le_bytes(code[offset..offset+8].try_into().unwrap());
                    offset += 8;
                    value as usize
                };
                
                // Copy from component signals to function's memory
                let dst_base = vm.memory_base_pointer_ff + ff_arg_idx;
                if vm.memory_ff.len() <= dst_base + size {
                    vm.memory_ff.resize(dst_base + size, None);
                }

                for i in 0..size {
                    vm.memory_ff[dst_base + i] =
                        Some(component_tree.get_signal(signal_idx+i)?);
                }

                ff_arg_idx += size;
            }
            0b0001_0000u8..=0b0001_0111u8 => { // signal argument (only valid in component context)
                // Decode bit flags
                let cmp_idx_is_variable = (arg_type & 1) != 0;
                let sig_idx_is_variable = (arg_type & 2) != 0;
                let size_is_variable = (arg_type & 4) != 0;

                // Get caller's context from the call frame we just pushed
                let frame = vm.call_stack.last()
                    .ok_or(RuntimeError::CallStackUnderflow)?;
                let caller_stack_base = frame.return_stack_base_pointer_i64;

                // Read and resolve signal index
                let cmp_idx = if cmp_idx_is_variable {
                    // It's a variable index - need to load from caller's stack
                    let var_idx = i64::from_le_bytes(code[offset..offset+8].try_into().unwrap()) as usize;
                    offset += 8;

                    *vm.stack_i64.get(caller_stack_base + var_idx)
                        .and_then(|v| v.as_ref())
                        .ok_or(RuntimeError::StackVariableIsNotSet)? as usize
                } else {
                    // It's a literal value
                    let value = i64::from_le_bytes(code[offset..offset+8].try_into().unwrap());
                    offset += 8;
                    value as usize
                };

                let sig_idx = if sig_idx_is_variable {
                    // It's a variable index - need to load from caller's stack
                    let var_idx = i64::from_le_bytes(code[offset..offset+8].try_into().unwrap()) as usize;
                    offset += 8;

                    *vm.stack_i64.get(caller_stack_base + var_idx)
                        .and_then(|v| v.as_ref())
                        .ok_or(RuntimeError::StackVariableIsNotSet)? as usize
                } else {
                    // It's a literal value
                    let value = i64::from_le_bytes(code[offset..offset+8].try_into().unwrap());
                    offset += 8;
                    value as usize
                };

                // Read and resolve size
                let size = if size_is_variable {
                    // It's a variable index - need to load from caller's stack
                    let var_idx = i64::from_le_bytes(code[offset..offset+8].try_into().unwrap()) as usize;
                    offset += 8;

                    *vm.stack_i64.get(caller_stack_base + var_idx)
                        .and_then(|v| v.as_ref())
                        .ok_or(RuntimeError::StackVariableIsNotSet)? as usize
                } else {
                    // It's a literal value
                    let value = i64::from_le_bytes(code[offset..offset+8].try_into().unwrap());
                    offset += 8;
                    value as usize
                };

                // Copy from component signals to function's memory
                let dst_base = vm.memory_base_pointer_ff + ff_arg_idx;
                if vm.memory_ff.len() <= dst_base + size {
                    vm.memory_ff.resize(dst_base + size, None);
                }

                match component_tree.components[cmp_idx] {
                    None => {
                        return Err(Box::new(RuntimeError::UninitializedComponent));
                    }
                    Some(ref c) => {
                        for i in 0..size {
                            vm.memory_ff[dst_base+i] = Some(c.read().unwrap().get_signal(sig_idx+i)?);
                        }
                        ff_arg_idx += size;
                    }
                }
            }
            _ => return Err(Box::new(RuntimeError::UnknownArgumentType(arg_type))),
        }
    }
    
    Ok(())
}

// Converts 8 bytes from the code to i64 and then to usize. Returns error
// if the code length is too short or if i64 < 0 or if i64 is too big to fit
// into usize.
fn usize_from_code(
    code: &[u8], ip: usize) -> Result<(usize, usize), RuntimeError> {

    let slice = code.get(ip..ip+size_of::<u64>())
        .ok_or(RuntimeError::CodeIndexOutOfBounds)?;
    let bytes: [u8; 8] = slice.try_into()
        .map_err(|_| RuntimeError::I32ToUsizeConversion)?;
    let v = i64::from_le_bytes(bytes);
    let v: usize = v.try_into()
        .map_err(|_| RuntimeError::I32ToUsizeConversion)?;

    Ok((v, ip+8))
}

pub fn disassemble_instruction_to_string<T>(
    code: &[u8], ip: usize, name: &str,
    ff_variable_names: &[String],
    i64_variable_names: &[String]) -> (usize, String)
where
    T: FieldOps {

    let mut output = format!("{:08x} [{:10}] ", ip, name);

    let op_code = read_instruction(code, ip);
    let mut ip = ip + 1usize;

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
            let v = i64::from_le_bytes((&code[ip..ip+8]).try_into().unwrap());
            ip += size_of::<i64>();
            output.push_str(&format!("PushI64: {}", v));
        }
        OpCode::PushFf => {
            let s = &code[ip..ip+T::BYTES];
            ip += T::BYTES;
            let v = T::from_le_bytes(s).unwrap();
            output.push_str(&format!("PushFf: {}", v));
        }
        OpCode::StoreVariableFf => {
            let var_idx: usize;
            (var_idx, ip) = usize_from_code(code, ip).unwrap();
            let var_name = ff_variable_names.get(var_idx)
                .map(|s| format!(" ({})", s))
                .unwrap_or_default();
            output.push_str(&format!("StoreVariableFf: {}{}", var_idx, var_name));
        }
        OpCode::StoreVariableI64 => {
            let var_idx: usize;
            (var_idx, ip) = usize_from_code(code, ip).unwrap();
            let var_name = i64_variable_names.get(var_idx)
                .map(|s| format!(" ({})", s))
                .unwrap_or_default();
            output.push_str(&format!("StoreVariableI64: {}{}", var_idx, var_name));
        }
        OpCode::LoadVariableI64 => {
            let var_idx: usize;
            (var_idx, ip) = usize_from_code(code, ip).unwrap();
            let var_name = i64_variable_names.get(var_idx)
                .map(|s| format!(" ({})", s))
                .unwrap_or_default();
            output.push_str(&format!("LoadVariableI64: {}{}", var_idx, var_name));
        }
        OpCode::LoadVariableFf => {
            let var_idx: usize;
            (var_idx, ip) = usize_from_code(code, ip).unwrap();
            let var_name = ff_variable_names.get(var_idx)
                .map(|s| format!(" ({})", s))
                .unwrap_or_default();
            output.push_str(&format!("LoadVariableFf: {}{}", var_idx, var_name));
        }
        OpCode::JumpIfFalseFf => {
            let v = i32::from_le_bytes((&code[ip..ip+size_of::<i32>()]).try_into().unwrap());
            ip += size_of::<i32>();
            let newIP = if v < 0 {
                ip - (v.unsigned_abs() as usize)
            } else {
                ip + (v as usize)
            };
            output.push_str(&format!("JumpIfFalseFf: {:+} -> {:08x}", v, newIP));
        }
        OpCode::JumpIfFalseI64 => {
            let v = i32::from_le_bytes((&code[ip..ip+size_of::<i32>()]).try_into().unwrap());
            ip += size_of::<i32>();
            let newIP = if v < 0 {
                ip - (v.unsigned_abs() as usize)
            } else {
                ip + (v as usize)
            };
            output.push_str(&format!("JumpIfFalseI64: {:+} -> {:08x}", v, newIP));
        }
        OpCode::Jump => {
            let v = i32::from_le_bytes((&code[ip..ip+size_of::<i32>()]).try_into().unwrap());
            ip += size_of::<i32>();
            let newIP = if v < 0 {
                ip - (v.unsigned_abs() as usize)
            } else {
                ip + (v as usize)
            };
            output.push_str(&format!("Jump: {:+} -> {:08x}", v, newIP));
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
        OpCode::OpI64Div => {
            output.push_str("OpI64Div");
        }
        OpCode::OpI64Pow => {
            output.push_str("OpI64Pow");
        }
        OpCode::OpI64Rem => {
            output.push_str("OpI64Rem");
        }
        OpCode::OpI64And => {
            output.push_str("OpI64And");
        }
        OpCode::OpI64Or => {
            output.push_str("OpI64Or");
        }
        OpCode::OpI64Shl => {
            output.push_str("OpI64Shl");
        }
        OpCode::OpI64Shr => {
            output.push_str("OpI64Shr");
        }
        OpCode::OpI64BNot => {
            output.push_str("OpI64BNot");
        }
        OpCode::OpI64BXor => {
            output.push_str("OpI64BXor");
        }
        OpCode::OpI64BOr => {
            output.push_str("OpI64BOr");
        }
        OpCode::OpI64BAnd => {
            output.push_str("OpI64BAnd");
        }
        OpCode::Error => {
            output.push_str("Error");
        }
        OpCode::FfMReturn => {
            output.push_str("FfMReturn");
        }
        OpCode::I64MReturn => {
            output.push_str("I64MReturn");
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
            let func_idx = u32::from_le_bytes((&code[ip..ip+4]).try_into().unwrap());
            ip += 4;
            
            // Read argument count
            let arg_count = code[ip];
            ip += 1;
            
            output.push_str(&format!("FfMCall: func_idx={}, args=[", func_idx));
            
            // Parse each argument
            for i in 0..arg_count {
                if i > 0 {
                    output.push_str(", ");
                }
                
                let arg_type = code[ip];
                ip += 1;
                
                match arg_type {
                    0 => { // i64 literal
                        let v = i64::from_le_bytes((&code[ip..ip+8]).try_into().unwrap());
                        ip += 8;
                        output.push_str(&format!("i64.{}", v));
                    }
                    1 => { // ff literal
                        let v = T::from_le_bytes(&code[ip..ip+T::BYTES]).unwrap();
                        ip += T::BYTES;
                        output.push_str(&format!("ff.{}", v));
                    }
                    2 => { // ff variable
                        let v = i64::from_le_bytes((&code[ip..ip+8]).try_into().unwrap());
                        ip += 8;
                        output.push_str(&format!("ff.var[{}]", v));
                    }
                    3 => { // i64 variable
                        let v = i64::from_le_bytes((&code[ip..ip+8]).try_into().unwrap());
                        ip += 8;
                        output.push_str(&format!("i64.var[{}]", v));
                    }
                    4..=7 => { // ff memory
                        let addr_is_variable = (arg_type & 1) != 0;
                        let size_is_variable = (arg_type & 2) != 0;
                        
                        let addr_val = i64::from_le_bytes((&code[ip..ip+8]).try_into().unwrap());
                        ip += 8;
                        let size_val = i64::from_le_bytes((&code[ip..ip+8]).try_into().unwrap());
                        ip += 8;
                        
                        output.push_str("ff.memory(");
                        if addr_is_variable {
                            let var_name = i64_variable_names.get(addr_val as usize)
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
                        
                        let addr_val = i64::from_le_bytes((&code[ip..ip+8]).try_into().unwrap());
                        ip += 8;
                        let size_val = i64::from_le_bytes((&code[ip..ip+8]).try_into().unwrap());
                        ip += 8;
                        
                        output.push_str("i64.memory(");
                        if addr_is_variable {
                            let var_name = i64_variable_names.get(addr_val as usize)
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
                        
                        let idx_val = i64::from_le_bytes((&code[ip..ip+8]).try_into().unwrap());
                        ip += 8;
                        let size_val = i64::from_le_bytes((&code[ip..ip+8]).try_into().unwrap());
                        ip += 8;
                        
                        output.push_str("signal(");
                        if idx_is_variable {
                            let var_name = i64_variable_names.get(idx_val as usize)
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
        OpCode::I64Return => {
            output.push_str("I64Return");
        }
        OpCode::I64Store => {
            output.push_str("I64Store");
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
        OpCode::OpI64Neq => {
            output.push_str("OpI64Neq");
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
        OpCode::FfExtendI64 => {
            output.push_str("FfExtendI64");
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
        OpCode::GetBusSignalPosition => {
            output.push_str("GetBusSignalPosition");
        }
        OpCode::GetBusSignalSize => {
            output.push_str("GetBusSignalSize");
        }
        OpCode::GetBusSignalType => {
            output.push_str("GetBusSignalType");
        }
        OpCode::GetBusSignalDimension => {
            output.push_str("GetBusSignalDimension");
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
fn spawn_component_execution<'scope, 'env, F, T: FieldOps>(
    scope: &'scope std::thread::Scope<'scope, 'env>,
    circuit: &'env Circuit<T>,
    ff: &'env F,
    c: Arc<RwLock<Component<T>>>,
)
where
    for <'a> &'a F: FieldOperations<Type = T>,
    T: 'env,
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
    circuit: &Circuit<T>, ff: &F,
    component_tree: &mut Component<T>) -> Result<(), Box<dyn Error + Sync + Send>>
where
    for <'a> &'a F: FieldOperations<Type = T> {

    std::thread::scope(|_scope| -> Result<(), Box<dyn Error + Sync + Send>> {
    #[cfg(feature = "parallel_components")]
    let scope = _scope;
    #[cfg(feature = "debug_vm2")]
    {
        let template_name = &circuit.templates[component_tree.template_id].name;
        println!("execute {}[{}]", template_name, component_tree.signals_start);
    }
    #[cfg(feature = "parallel_components")]
    component_tree.components.iter_mut()
        .filter_map(|x| x.as_mut())
        .filter(|x| x.read().unwrap().number_of_inputs == 0)
        .for_each(|c| spawn_component_execution(scope, circuit, ff, c.clone()));

    #[cfg(not(feature = "parallel_components"))]
    component_tree.components.iter_mut()
        .filter_map(|x| x.as_mut())
        .filter(|x| x.read().unwrap().number_of_inputs == 0)
        .try_for_each(|c| -> Result<(), Box<dyn Error + Sync + Send>> {
            let mut component = c.write()
                .map_err(|e| format!("Failed to lock component: {}", e))?;
            execute(circuit, ff, &mut component)?;
            Ok(())
        })?;

    let mut ip: usize = 0;
    let mut vm = VM::<T>::new();

    // Initialize with template's variable counts (function calls will resize as needed)
    // TODO every time we switch the context, we should check the stacks have sufficient size
    vm.stack_ff.resize_with(
        circuit.templates[component_tree.template_id].ff_variable_names.len(), || None);
    vm.stack_i64.resize_with(
        circuit.templates[component_tree.template_id].i64_variable_names.len(), || None);

    #[cfg(feature = "debug_vm2")]
    let (mut code, mut name, mut ff_variable_names, mut i64_variable_names) = get_current_context(&vm, circuit, component_tree);
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
        disassemble_instruction::<T>(
            code, ip, name, ff_variable_names, i64_variable_names);

        let op_code = read_instruction(code, ip);
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
                        component_tree.signals_start + sig_idx, sig_idx, sig);
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
                        component_tree.signals_start + signal_idx, signal_idx,
                        value);
                }
                component_tree.set_signal(signal_idx, value)?;
            }
            OpCode::PushI64 => {
                vm.push_i64(
                    i64::from_le_bytes((&code[ip..ip+8]).try_into().unwrap()));
                ip += 8;
            }
            OpCode::PushFf => {
                let s = &code[ip..ip+T::BYTES];
                ip += T::BYTES;
                let v = ff.parse_le_bytes(s)?;
                vm.push_ff(v);
            }
            OpCode::StoreVariableFf => {
                let var_idx: usize;
                (var_idx, ip) = usize_from_code(code, ip)?;
                let value = vm.pop_ff()?;
                vm.stack_ff[vm.stack_base_pointer_ff + var_idx] = Some(value);
                #[cfg(feature = "debug_vm2")]
                {
                    let var_name = ff_variable_names
                        .get(var_idx)
                        .map(|s| format!(" ({})", s))
                        .unwrap_or_default();
                    println!("StoreVariableFf: {}{} = {}", var_idx, var_name, vm.stack_ff[vm.stack_base_pointer_ff + var_idx].unwrap());
                }
            }
            OpCode::StoreVariableI64 => {
                let var_idx: usize;
                (var_idx, ip) = usize_from_code(code, ip)?;
                let value = vm.pop_i64()?;
                vm.stack_i64[vm.stack_base_pointer_i64 + var_idx] = Some(value);
                #[cfg(feature = "debug_vm2")]
                {
                    let var_name = i64_variable_names
                        .get(var_idx)
                        .map(|s| format!(" ({})", s))
                        .unwrap_or_default();
                    println!("StoreVariableI64: {}{} = {}", var_idx, var_name, vm.stack_i64[vm.stack_base_pointer_i64 + var_idx].unwrap());
                }
            }
            OpCode::LoadVariableI64 => {
                let var_idx: usize;
                (var_idx, ip) = usize_from_code(code, ip)?;
                let var = match vm.stack_i64.get(vm.stack_base_pointer_i64 + var_idx) {
                    Some(v) => v,
                    None => return Err(Box::new(RuntimeError::StackOverflow)),
                };
                let var = match var {
                    Some(v) => v,
                    None => return Err(Box::new(RuntimeError::StackVariableIsNotSet)),
                };
                #[cfg(feature = "debug_vm2")]
                {
                    let var_name = i64_variable_names
                        .get(var_idx)
                        .map(|s| format!(" ({})", s))
                        .unwrap_or_default();
                    println!("LoadVariableI64: {}{} = {}", var_idx, var_name, var);
                }
                vm.push_i64(*var);
            }
            OpCode::LoadVariableFf => {
                let var_idx: usize;
                (var_idx, ip) = usize_from_code(code, ip)?;
                let var = match vm.stack_ff.get(vm.stack_base_pointer_ff + var_idx) {
                    Some(v) => v,
                    None => return Err(Box::new(RuntimeError::StackOverflow)),
                };
                let var = match var {
                    Some(v) => v,
                    None => return Err(Box::new(RuntimeError::StackVariableIsNotSet)),
                };
                vm.push_ff(*var);
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
                let sig = component_tree.components[cmp_idx].as_ref()
                    .ok_or_else(|| Box::new(RuntimeError::UninitializedComponent))?
                    .read().unwrap()
                    .get_signal(sig_idx)?;
                #[cfg(feature = "debug_vm2")]
                {
                    let signals_start = component_tree.components[cmp_idx]
                        .as_ref().unwrap().read().unwrap().signals_start;
                    println!(
                        "LoadCmpSignal [S{}]: {} = {}",
                        signals_start + sig_idx, sig_idx, sig);
                }
                vm.push_ff(sig);
            }
            OpCode::StoreCmpSignalAndRun => {
                let sig_idx = vm.pop_usize()?;
                let cmp_idx = vm.pop_usize()?;
                let value = vm.pop_ff()?;
                match component_tree.components[cmp_idx] {
                    None => {
                        return Err(
                            Box::new(RuntimeError::UninitializedComponent))
                    }
                    Some(ref c) => {
                        {
                            let mut c = c.write().unwrap();
                            c.set_signal(sig_idx, value)?;
                            c.number_of_inputs -= 1;
                        }
                        #[cfg(feature = "debug_vm2")]
                        {
                            let c = c.read().unwrap();
                            println!(
                                "StoreCmpSignalAndRun [S{}]: {}[{}/{}] = {}, inputs left: {}, template: {}",
                                c.signals_start + sig_idx, cmp_idx, c.signals_start, sig_idx, value,
                                c.number_of_inputs, circuit.templates[c.template_id].name);
                            println!(
                                "StoreCmpSignalAndRun: Run component {}",
                                cmp_idx);
                        }

                        #[cfg(feature = "parallel_components")]
                        spawn_component_execution(scope, circuit, ff, c.clone());

                        #[cfg(not(feature = "parallel_components"))]
                        {
                            let mut component = c.write()
                                .map_err(|e| format!("Failed to lock component: {}", e))?;
                            execute(circuit, ff, &mut component)?;
                        }
                    }
                }
            }
            OpCode::StoreCmpSignalCntCheck => {
                let sig_idx = vm.pop_usize()?;
                let cmp_idx = vm.pop_usize()?;
                let value = vm.pop_ff()?;
                match component_tree.components[cmp_idx] {
                    None => {
                        return Err(
                            Box::new(RuntimeError::UninitializedComponent))
                    }
                    Some(ref mut c) => {
                        let mut run = false;
                        {
                            let mut c = c.write().unwrap();
                            c.set_signal(sig_idx, value)?;
                            c.number_of_inputs -= 1;
                            if c.number_of_inputs == 0 {
                                run = true;
                            }
                        }
                        #[cfg(feature = "debug_vm2")]
                        {
                            let c = c.read().unwrap();
                            println!(
                                "StoreCmpSignalCntCheck [S{}]: {}[{}/{}] = {}, inputs left: {}, template: {}",
                                c.signals_start + sig_idx, cmp_idx, c.signals_start, sig_idx, value,
                                c.number_of_inputs, circuit.templates[c.template_id].name);
                        }
                        if run {
                            #[cfg(feature = "debug_vm2")]
                            {
                                println!(
                                    "StoreCmpSignalCntCheck: Run component {}",
                                    cmp_idx);
                            }
                            #[cfg(feature = "parallel_components")]
                            spawn_component_execution(scope, circuit, ff, c.clone());

                            #[cfg(not(feature = "parallel_components"))]
                            {
                                let mut component = c.write()
                                    .map_err(|e| format!("Failed to lock component: {}", e))?;
                                execute(circuit, ff, &mut component)?;
                            }
                        }
                    }
                }
            }
            OpCode::StoreCmpInputCnt => {
                let sig_idx = vm.pop_usize()?;
                let cmp_idx = vm.pop_usize()?;
                let value = vm.pop_ff()?;
                match component_tree.components[cmp_idx] {
                    None => {
                        return Err(
                            Box::new(RuntimeError::UninitializedComponent))
                    }
                    Some(ref mut c) => {
                        {
                            let mut c = c.write().unwrap();
                            c.set_signal(sig_idx, value)?;
                            c.number_of_inputs -= 1;
                        }
                        #[cfg(feature = "debug_vm2")]
                        {
                            let c = c.read().unwrap();
                            println!(
                                "StoreCmpInputCnt [S{}]: {}[{}/{}] = {}, inputs left: {}, template: {}",
                                c.signals_start + sig_idx, cmp_idx, c.signals_start, sig_idx, value,
                                c.number_of_inputs, circuit.templates[c.template_id].name);
                        }
                        // Skip the check for c.number_of_inputs == 0 and component execution
                    }
                }
            }
            OpCode::StoreCmpInput => {
                let sig_idx = vm.pop_usize()?;
                let cmp_idx = vm.pop_usize()?;
                let value = vm.pop_ff()?;
                match component_tree.components[cmp_idx] {
                    None => {
                        return Err(
                            Box::new(RuntimeError::UninitializedComponent))
                    }
                    Some(ref mut c) => {
                        c.write().unwrap().set_signal(sig_idx, value)?;
                        #[cfg(feature = "debug_vm2")]
                        {
                            let c = c.read().unwrap();
                            println!(
                                "StoreCmpInput [S{}]: {}[{}/{}] = {}, inputs left: {}, template: {}",
                                c.signals_start + sig_idx, cmp_idx, c.signals_start, sig_idx, value,
                                c.number_of_inputs, circuit.templates[c.template_id].name);
                        }
                    }
                }
            }
            OpCode::JumpIfFalseFf => {
                let offset_bytes = &code[ip..ip + size_of::<i32>()];
                let offset = i32::from_le_bytes((offset_bytes).try_into().unwrap());
                ip += size_of::<i32>();

                if vm.pop_ff()?.is_zero() {
                    if offset < 0 {
                        ip -= offset.unsigned_abs() as usize;
                    } else {
                        ip += offset as usize;
                    }
                }
            }
            OpCode::JumpIfFalseI64 => {
                let offset_bytes = &code[ip..ip + size_of::<i32>()];
                let offset = i32::from_le_bytes((offset_bytes).try_into().unwrap());
                ip += size_of::<i32>();

                if vm.pop_i64()? == 0 {
                    if offset < 0 {
                        ip -= offset.unsigned_abs() as usize;
                    } else {
                        ip += offset as usize;
                    }
                }
            }
            OpCode::Error => {
                let error_code = vm.pop_i64()?;
                return Err(Box::new(RuntimeError::Assertion(error_code)));
            }
            OpCode::Jump => {
                let offset_bytes = &code[ip..ip + size_of::<i32>()];
                let offset = i32::from_le_bytes((offset_bytes).try_into().unwrap());
                ip += size_of::<i32>();

                if offset < 0 {
                    ip -= offset.unsigned_abs() as usize;
                } else {
                    ip += offset as usize;
                }
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
                vm.push_i64(lhs+rhs);
            }
            OpCode::OpI64Sub => {
                let lhs = vm.pop_i64()?;
                let rhs = vm.pop_i64()?;
                vm.push_i64(lhs-rhs);
            }
            OpCode::OpI64Div => {
                let lhs = vm.pop_i64()?;
                let rhs = vm.pop_i64()?;
                if rhs == 0 {
                    return Err(Box::new(RuntimeError::AttemptDivideByZeroComponent));
                }
                vm.push_i64(lhs.div(rhs));
            }
            OpCode::OpI64Pow => {
                let exp = vm.pop_i64()?;
                let base = vm.pop_i64()?;

                if exp < 0 {
                    return Err(Box::new(RuntimeError::I32ToUsizeConversion));
                }

                vm.push_i64(base.pow(exp as u32));
            }
            OpCode::OpI64Rem => {
                let b = vm.pop_i64()?;
                let a = vm.pop_i64()?;

                if b == 0 {
                    return Err(Box::new(RuntimeError::AttemptDivideByZeroComponent));
                }

                vm.push_i64(a % b);
            }
            OpCode::OpI64And => {
                let lhs = vm.pop_i64()?;
                let rhs = vm.pop_i64()?;
                vm.push_i64(lhs & rhs);
            }
            OpCode::OpI64Or => {
                let lhs = vm.pop_i64()?;
                let rhs = vm.pop_i64()?;
                vm.push_i64(lhs | rhs);
            }
            OpCode::OpI64Shl => {
                let shift = vm.pop_i64()?;
                let value = vm.pop_i64()?;

                // Rust masks shift by & 63 automatically, but we ensure u32
                // Ref: https://doc.rust-lang.org/std/primitive.i64.html
                let s = (shift as u64 & 63) as u32;

                let result = value.wrapping_shl(s);
                vm.push_i64(result);
            }
            OpCode::OpI64Shr => {
                let shift = vm.pop_i64()?;
                let value = vm.pop_i64()?;

                // Rust masks shift by & 63 automatically, but we ensure u32
                // Ref: https://doc.rust-lang.org/std/primitive.i64.html
                let s = (shift as u64 & 63) as u32;

                let result = value.wrapping_shr(s);
                vm.push_i64(result);
            }
            OpCode::OpI64BNot => {
                let val = vm.pop_i64()?;
                vm.push_i64(!val);
            }
            OpCode::OpI64BXor => {
                let rhs = vm.pop_i64()?;
                let lhs = vm.pop_i64()?;
                vm.push_i64(rhs ^ lhs);
            }
            OpCode::OpI64BOr => {
                let rhs = vm.pop_i64()?;
                let lhs = vm.pop_i64()?;
                vm.push_i64(rhs | lhs);
            }
            OpCode::OpI64BAnd => {
                let rhs = vm.pop_i64()?;
                let lhs = vm.pop_i64()?;
                vm.push_i64(rhs & lhs);
            }
            OpCode::I64MReturn => {
                let size = vm.pop_usize()?;
                let src_addr = vm.pop_usize()?;
                let dst_addr = vm.pop_usize()?;

                let call_frame = vm.call_stack.pop()
                    .ok_or(RuntimeError::CallStackUnderflow)?;

                for i in 0..size {
                    let src_idx = src_addr + i + vm.memory_base_pointer_i64;
                    let dst_idx = dst_addr + i + call_frame.return_memory_base_pointer_i64;

                    if src_idx < vm.memory_i64.len() {
                        if dst_idx >= vm.memory_i64.len() {
                            vm.memory_i64.resize(dst_idx + 1, None);
                        }

                        vm.memory_i64[dst_idx] = vm.memory_i64[src_idx];
                        vm.memory_i64[src_idx] = None;
                    }
                }

                ip = call_frame.return_ip;
                vm.current_execution_context = call_frame.return_context;
                vm.stack_base_pointer_ff = call_frame.return_stack_base_pointer_ff;
                vm.stack_base_pointer_i64 = call_frame.return_stack_base_pointer_i64;
                vm.memory_base_pointer_ff = call_frame.return_memory_base_pointer_ff;
                vm.memory_base_pointer_i64 = call_frame.return_memory_base_pointer_i64;

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
            OpCode::FfMReturn => {
                // Pop size, src, dst from stack
                let size = vm.pop_usize()?;
                let src_addr = vm.pop_usize()?;
                let dst_addr = vm.pop_usize()?;
                
                // Pop the call frame to get return context
                let call_frame = vm.call_stack.pop()
                    .ok_or(RuntimeError::CallStackUnderflow)?;

                #[cfg(feature = "debug_vm2")]
                {
                    println!("FfMReturn:");
                }
                // Copy memory from a function's space to caller's space
                for i in 0..size {
                    let src_idx = src_addr + i + vm.memory_base_pointer_ff;
                    let dst_idx = dst_addr + i + call_frame.return_memory_base_pointer_ff;

                    if src_idx < vm.memory_ff.len() {
                        if dst_idx >= vm.memory_ff.len() {
                            vm.memory_ff.resize(dst_idx + 1, None);
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
                let last_dst_idx = dst_addr + size + call_frame.return_memory_base_pointer_ff;
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
                    (code, name, ff_variable_names, i64_variable_names) = get_current_context(&vm, circuit, component_tree);
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

                let dst_start = dst_addr.checked_add(vm.memory_base_pointer_ff)
                    .ok_or(RuntimeError::MemoryAddressOutOfBounds)?;
                let src_start = src_addr.checked_add(vm.memory_base_pointer_ff)
                    .ok_or(RuntimeError::MemoryAddressOutOfBounds)?;

                for offset in 0..size {
                    let src_idx = src_start.checked_add(offset)
                        .ok_or(RuntimeError::MemoryAddressOutOfBounds)?;
                    let dst_idx = dst_start.checked_add(offset)
                        .ok_or(RuntimeError::MemoryAddressOutOfBounds)?;

                    if src_idx >= vm.memory_ff.len() {
                        return Err(Box::new(RuntimeError::MemoryAddressOutOfBounds));
                    }
                    let value = *vm.memory_ff.get(src_idx)
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
                        size,
                        src_start,
                        dst_start,
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

                let want_len = dst_start.checked_add(size)
                    .ok_or(RuntimeError::MemoryAddressOutOfBounds)?;
                if want_len > vm.memory_ff.len() {
                    vm.memory_ff.resize(want_len, None);
                }
                for offset in 0..size {
                    let i = sig_idx.checked_add(offset)
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

                let want_len = dst_start.checked_add(size).
                    ok_or(RuntimeError::MemoryAddressOutOfBounds)?;
                if want_len > vm.memory_ff.len() {
                    vm.memory_ff.resize(want_len, None);
                }
                for offset in 0..size {
                    let value = component_tree.components[cmp_idx]
                        .as_ref().unwrap()
                        .read().unwrap()
                        .get_signal(sig_idx + offset)?;
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
                (func_idx, ip) = read_usize32(code, ip);

                // Validate function index
                if func_idx >= circuit.functions.len() {
                    return Err(Box::new(RuntimeError::InvalidFunctionIndex(func_idx)));
                }

                // Read argument count
                let arg_count = code[ip];
                ip += 1;
                
                // Create call frame
                let call_frame = CallFrame {
                    return_ip: ip + calculate_args_size::<T>(&code[ip..], arg_count)?,
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
                vm.stack_ff.resize(vm.stack_base_pointer_ff + circuit.functions[func_idx].ff_variable_names.len(), None);
                vm.stack_i64.resize(vm.stack_base_pointer_i64 + circuit.functions[func_idx].i64_variable_names.len(), None);
                
                // Process arguments and copy to function memory
                process_function_arguments(
                    &mut vm, &code[ip..], arg_count, component_tree)?;
                
                // Switch to function execution context
                #[cfg(feature = "debug_vm2")]
                {
                    (code, name, ff_variable_names, i64_variable_names) = get_current_context(&vm, circuit, component_tree);
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
                let call_frame = vm.call_stack.pop()
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
                    (code, name, ff_variable_names, i64_variable_names) = get_current_context(&vm, circuit, component_tree);
                }
                #[cfg(not(feature = "debug_vm2"))]
                {
                    code = get_current_context(&vm, circuit, component_tree);
                }
            }
            OpCode::FfExtendI64 => {
                let value = vm.pop_i64()?;

                let ff_value = FieldOps::from_le_bytes(&value.to_le_bytes()).unwrap();

                vm.push_ff(ff_value);

                #[cfg(feature = "debug_vm2")]
                {
                    println!("FfExtendI64: {} -> {}", value, fe);
                }
            }
            OpCode::FfStore => {
                let addr: usize = vm.pop_i64()?.try_into()
                    .map_err(|_| Box::new(RuntimeError::MemoryAddressOutOfBounds))?;
                let addr = addr.checked_add(vm.memory_base_pointer_ff)
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
                let addr: usize = vm.pop_i64()?.try_into()
                    .map_err(|_| Box::new(RuntimeError::MemoryAddressOutOfBounds))?;
                let addr = addr.checked_add(vm.memory_base_pointer_ff)
                    .ok_or(Box::new(RuntimeError::MemoryAddressOutOfBounds))?;
                if addr >= vm.memory_ff.len() {
                    return Err(Box::new(RuntimeError::MemoryAddressOutOfBounds));
                }
                let value = vm.memory_ff.get(addr)
                    .and_then(|v| v.as_ref())
                    .ok_or(RuntimeError::MemoryVariableIsNotSet)?;
                vm.push_ff(*value);
            }
            OpCode::I64Load => {
                let addr: usize = vm.pop_i64()?.try_into()
                    .map_err(|_| Box::new(RuntimeError::MemoryAddressOutOfBounds))?;
                let addr = addr.checked_add(vm.memory_base_pointer_i64)
                    .ok_or(Box::new(RuntimeError::MemoryAddressOutOfBounds))?;
                if addr >= vm.memory_i64.len() {
                    return Err(Box::new(RuntimeError::MemoryAddressOutOfBounds));
                }
                let value = vm.memory_i64.get(addr)
                    .and_then(|v| v.as_ref())
                    .ok_or(RuntimeError::MemoryVariableIsNotSet)?;
                vm.push_i64(*value);
            }
            OpCode::I64Return => {
                let return_value = vm.pop_i64()?;
                let call_frame = vm.call_stack.pop()
                    .ok_or(RuntimeError::CallStackUnderflow)?;

                ip = call_frame.return_ip;
                vm.current_execution_context = call_frame.return_context;
                vm.stack_base_pointer_ff = call_frame.return_stack_base_pointer_ff;
                vm.stack_base_pointer_i64 = call_frame.return_stack_base_pointer_i64;
                vm.memory_base_pointer_ff = call_frame.return_memory_base_pointer_ff;
                vm.memory_base_pointer_i64 = call_frame.return_memory_base_pointer_i64;

                vm.push_i64(return_value);

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
            OpCode::I64Store => {
                let addr: usize = vm.pop_i64()?.try_into()
                    .map_err(|_| Box::new(RuntimeError::MemoryAddressOutOfBounds))?;
                let addr = addr.checked_add(vm.memory_base_pointer_i64)
                    .ok_or(Box::new(RuntimeError::MemoryAddressOutOfBounds))?;

                    if addr >= vm.memory_i64.len() {
                    vm.memory_i64.resize(addr + 1, None);
                }

                let value = vm.pop_i64()?;
                vm.memory_i64[addr] = Some(value);
                #[cfg(feature = "debug_vm2")]
                {
                    println!("I64Store: [{}] = {}", addr, value);
                }
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
            OpCode::OpI64Neq => {
                let rhs = vm.pop_i64()?;
                let lhs = vm.pop_i64()?;
                vm.push_i64(if lhs != rhs { 1 } else { 0 });
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
                let template_id = match component_tree.components[cmp_idx] {
                    None => {
                        return Err(Box::new(RuntimeError::UninitializedComponent))
                    }
                    Some(ref c) => c.read().unwrap().template_id as i64
                };
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
                    return Err(Box::new(RuntimeError::SignalIdOutOfBounds(signal_id, total_io_signals)));
                }
                
                let position = if signal_id < num_outputs {
                    calculate_signal_offset(&template.outputs, signal_id, &circuit.types)
                } else {
                    let output_total_size: usize = template.outputs.iter()
                        .map(|sig| calculate_signal_size(sig, &circuit.types))
                        .sum();
                    output_total_size + calculate_signal_offset(&template.inputs, signal_id - num_outputs, &circuit.types)
                };
                
                vm.push_i64(position as i64);
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
                    return Err(Box::new(RuntimeError::SignalIdOutOfBounds(signal_id, total_io_signals)));
                }
                
                let size = if signal_id < num_outputs {
                    calculate_signal_size(&template.outputs[signal_id], &circuit.types)
                } else {
                    calculate_signal_size(&template.inputs[signal_id - num_outputs], &circuit.types)
                };
                
                vm.push_i64(size as i64);
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
                    Signal::Ff(_) => -1,
                    Signal::Bus(bus_type_id, _) => *bus_type_id as i64,
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
                    return Err(Box::new(RuntimeError::SignalIdOutOfBounds(signal_id, total_io_signals)));
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
                    return Err(Box::new(RuntimeError::DimensionIndexOutOfBounds(dimension_index, dims.len())));
                }
                
                vm.push_i64(dims[dimension_index] as i64);
            }
            OpCode::GetBusSignalPosition => {
                let bus_type_id = vm.pop_usize()?;
                let field_id = vm.pop_usize()?;

                if bus_type_id >= circuit.types.len() {
                    return Err(Box::new(RuntimeError::InvalidTemplateId(bus_type_id)));
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
                        "GetBusSignalPosition: bus_type {} field {} => offset {}",
                        bus_type_id, field_id, position
                    );
                }
            }
            OpCode::GetBusSignalSize => {
                let bus_type_id = vm.pop_usize()?;
                let field_id = vm.pop_usize()?;

                if bus_type_id >= circuit.types.len() {
                    return Err(Box::new(RuntimeError::InvalidTemplateId(bus_type_id)));
                }

                let bus_type = &circuit.types[bus_type_id];

                if field_id >= bus_type.fields.len() {
                    return Err(Box::new(RuntimeError::SignalIdOutOfBounds(
                        field_id,
                        bus_type.fields.len(),
                    )));
                }

                let size = bus_type.fields[field_id].size;
                vm.push_i64(size as i64);

                #[cfg(feature = "debug_vm2")]
                {
                    println!(
                        "GetBusSignalSize: bus_type {} field {} => size {}",
                        bus_type_id, field_id, size
                    );
                }
            }
            OpCode::GetBusSignalType => {
                let bus_type_id = vm.pop_usize()?;
                let field_id = vm.pop_usize()?;

                if bus_type_id >= circuit.types.len() {
                    return Err(Box::new(RuntimeError::InvalidTemplateId(bus_type_id)));
                }

                let bus_type = &circuit.types[bus_type_id];

                if field_id >= bus_type.fields.len() {
                    return Err(Box::new(RuntimeError::SignalIdOutOfBounds(
                        field_id,
                        bus_type.fields.len(),
                    )));
                }

                let field = &bus_type.fields[field_id];
                let type_id: i64 = match field.kind {
                    TypeFieldKind::Ff => -1,
                    TypeFieldKind::Bus(inner_type_id) => inner_type_id as i64,
                };

                vm.push_i64(type_id);

                #[cfg(feature = "debug_vm2")]
                {
                    println!(
                        "GetBusSignalType: bus_type {} field {} => type {}",
                        bus_type_id, field_id, type_id
                    );
                }
            }
            OpCode::GetBusSignalDimension => {
                let dimension_idx = vm.pop_usize()?;
                let field_id = vm.pop_usize()?;
                let bus_type_id = vm.pop_usize()?;

                if bus_type_id >= circuit.types.len() {
                    return Err(Box::new(RuntimeError::InvalidTemplateId(bus_type_id)));
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
                    "GetBusSignalDimension: bus_type={} field={} dim[{}]={}",
                    bus_type_id, field_id, dimension_idx, dim_length
                );
            }
            OpCode::CopyCmpInputsFromSelf => {
                let flags = if let Some(flag) = code.get(ip) {
                    ip += 1;
                    *flag
                } else {
                    return Err(Box::new(RuntimeError::CodeIndexOutOfBounds));
                };
                let cmp_idx = vm.pop_usize()?;
                let cmp_sig_idx = vm.pop_usize()?;
                let self_sig_idx = vm.pop_usize()?;
                let num_signals = vm.pop_usize()?;

                for offset in 0..num_signals {
                    let value = component_tree.get_signal(self_sig_idx + offset)?;
                    component_tree.components[cmp_idx]
                        .as_ref()
                        .ok_or(RuntimeError::UninitializedComponent)?
                        .write().unwrap()
                        .set_signal(cmp_sig_idx+offset, value)?;

                    #[cfg(feature = "debug_vm2")]
                    {
                        let dst_idx =
                            component_tree.components[cmp_idx]
                                .as_ref()
                                .unwrap()
                                .read().unwrap()
                                .signals_start
                                + cmp_sig_idx + offset;
                        println!(
                            "CopyCmpInputsFromSelf [S{} -> S{}]: cmp {} sig {} = {} / {}",
                            component_tree.signals_start + self_sig_idx + offset,
                            dst_idx, cmp_idx,
                            cmp_sig_idx + offset, value, value);
                    }
                }

                let mode = flags & 0b11;
                let mut should_run = false;

                match mode {
                    0b00 => {}
                    0b01 => {
                        component_tree.components[cmp_idx].as_ref().unwrap().write().unwrap().number_of_inputs -= num_signals;
                    }
                    0b10 => {
                        should_run = true;
                    }
                    0b11 => {
                        let mut c = component_tree.components[cmp_idx].as_ref().unwrap().write().unwrap();
                        c.number_of_inputs -= num_signals;
                        if c.number_of_inputs == 0 {
                            should_run = true;
                        }
                    }
                    _ => {}
                }

                #[cfg(feature = "debug_vm2")]
                {
                    let c = component_tree.components[cmp_idx]
                        .as_ref().unwrap()
                        .read().unwrap();
                    println!(
                        "CopyCmpInputsFromSelf: cmp {} inputs left: {}, template: {}",
                        cmp_idx, c.number_of_inputs,
                        circuit.templates[c.template_id].name);
                }


                if should_run {
                    #[cfg(feature = "debug_vm2")]
                    {
                        println!(
                            "CopyCmpInputsFromSelf: Run component {}",
                            cmp_idx);
                    }
                    #[cfg(feature = "parallel_components")]
                    spawn_component_execution(
                        scope, circuit, ff,
                        component_tree.components[cmp_idx].as_ref().unwrap().clone());
                    #[cfg(not(feature = "parallel_components"))]
                    {
                        let c = component_tree.components[cmp_idx].as_ref().unwrap();
                        let mut component = c.write()
                            .map_err(|e| format!("Failed to lock component: {}", e))?;
                        execute(circuit, ff, &mut component)?;
                    }
                }
            }
            OpCode::CopyCmpInputsFromCmp => {
                let flags = if let Some(flag) = code.get(ip) {
                    ip += 1;
                    *flag
                } else {
                    return Err(Box::new(RuntimeError::CodeIndexOutOfBounds));
                };

                let dst_cmp_idx = vm.pop_usize()?;
                let dst_sig_idx = vm.pop_usize()?;
                let src_cmp_idx = vm.pop_usize()?;
                let src_sig_idx = vm.pop_usize()?;
                let num_signals = vm.pop_usize()?;

                for offset in 0..num_signals {
                    let value = component_tree.components[src_cmp_idx].as_ref()
                        .ok_or_else(|| Box::new(RuntimeError::UninitializedComponent))?
                        .write().unwrap()
                        .get_signal(src_sig_idx + offset)?;
                    component_tree.components[dst_cmp_idx].as_ref()
                        .ok_or_else(|| Box::new(RuntimeError::UninitializedComponent))?
                        .write().unwrap()
                        .set_signal(dst_sig_idx + offset, value)?;

                    #[cfg(feature = "debug_vm2")]
                    {
                        let src_index_start = component_tree.components[src_cmp_idx].as_ref().unwrap().read().unwrap().signals_start;
                        let dst_index_start = component_tree.components[dst_cmp_idx].as_ref().unwrap().read().unwrap().signals_start;
                        println!(
                            "CopyCmpInputsFromCmp [cmp {} S{} {} -> cmp {} S{} {}] = {}",
                            src_cmp_idx,
                            src_index_start + src_sig_idx + offset,
                            src_sig_idx + offset,
                            dst_cmp_idx,
                            dst_index_start + dst_sig_idx + offset,
                            dst_sig_idx + offset,
                            value
                        );
                    }
                }

                let mode = flags & 0b11;
                let mut should_run = false;

                match mode {
                    0b00 => {}
                    0b01 => {
                        component_tree.components[dst_cmp_idx]
                            .as_ref().unwrap()
                            .write().unwrap()
                            .number_of_inputs -= num_signals;
                    }
                    0b10 => {
                        should_run = true;
                    }
                    0b11 => {
                        let mut c = component_tree.components[dst_cmp_idx]
                            .as_ref().unwrap()
                            .write().unwrap();
                        c.number_of_inputs -= num_signals;
                        if c.number_of_inputs == 0 {
                            should_run = true;
                        }
                    }
                    _ => {}
                }

                #[cfg(feature = "debug_vm2")]
                {
                    let c = component_tree.components[dst_cmp_idx]
                        .as_ref().unwrap()
                        .read().unwrap();
                    println!(
                        "CopyCmpInputsFromCmp: cmp {} inputs left: {}, template: {}",
                        dst_cmp_idx, c.number_of_inputs,
                        circuit.templates[c.template_id].name
                    );
                }

                if should_run {
                    #[cfg(feature = "debug_vm2")]
                    {
                        println!(
                            "CopyCmpInputsFromCmp: Run component {}",
                            dst_cmp_idx
                        );
                    }
                    #[cfg(feature = "parallel_components")]
                    spawn_component_execution(
                        scope, circuit, ff,
                        component_tree.components[dst_cmp_idx]
                            .as_ref().unwrap()
                            .clone());

                    #[cfg(not(feature = "parallel_components"))]
                    {
                        let c = component_tree.components[dst_cmp_idx].as_ref().unwrap();
                        let mut component = c.write()
                            .map_err(|e| format!("Failed to lock component: {}", e))?;
                        execute(circuit, ff, &mut component)?;
                    }
                }

            }
            OpCode::CopyCmpInputsFromMemory => {
                let flags = if let Some(flag) = code.get(ip) {
                    ip += 1;
                    *flag
                } else {
                    return Err(Box::new(RuntimeError::CodeIndexOutOfBounds));
                };

                let dst_cmp_idx = vm.pop_usize()?;
                let dst_sig_idx = vm.pop_usize()?;
                let sig_addr = vm.pop_usize()?;
                let num_signals = vm.pop_usize()?;

                let memory_start = vm
                    .memory_base_pointer_ff
                    .checked_add(sig_addr)
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

                    component_tree.components[dst_cmp_idx]
                        .as_ref()
                        .ok_or(RuntimeError::UninitializedComponent)?
                        .write().unwrap()
                        .set_signal(dst_sig_idx + offset, *value)?;

                    #[cfg(feature = "debug_vm2")]
                    {
                        let dst_idx_global =
                            component_tree.components[dst_cmp_idx]
                                .as_ref()
                                .unwrap()
                                .read().unwrap()
                                .signals_start
                                + dst_sig_idx + offset;
                        println!(
                            "CopyCmpInputsFromMemory: M{} -> cmp {} S{} (global S{}), value={}",
                            src_idx,
                            dst_cmp_idx,
                            dst_sig_idx + offset,
                            dst_idx_global,
                            *value
                        );
                    }
                }

                let mode = flags & 0b11;
                let mut should_run = false;
                match mode {
                    0b00 => {}
                    0b01 => {
                        component_tree.components[dst_cmp_idx]
                            .as_ref().unwrap()
                            .write().unwrap()
                            .number_of_inputs -= num_signals;
                    }
                    0b10 => {
                        should_run = true;
                    }
                    0b11 => {
                        let mut c =
                            component_tree.components[dst_cmp_idx]
                            .as_ref().unwrap().write().unwrap();
                        c.number_of_inputs -= num_signals;
                        if c.number_of_inputs == 0 {
                            should_run = true;
                        }
                    }
                    _ => {}
                }

                #[cfg(feature = "debug_vm2")]
                {
                    let c = component_tree.components[dst_cmp_idx]
                        .as_ref().unwrap()
                        .read().unwrap();
                    println!(
                        "CopyCmpInputsFromMemory: cmp {} inputs left: {}, template: {}",
                        dst_cmp_idx,
                        c.number_of_inputs,
                        circuit.templates[c.template_id].name
                    );
                }

                if should_run {
                    #[cfg(feature = "debug_vm2")]
                    {
                        println!("CopyCmpInputsFromMemory: Run component {}", dst_cmp_idx);
                    }
                    #[cfg(feature = "parallel_components")]
                    spawn_component_execution(
                        scope, circuit, ff,
                        component_tree.components[dst_cmp_idx].as_ref().unwrap().clone());
                    #[cfg(not(feature = "parallel_components"))]
                    {
                        let c = component_tree.components[dst_cmp_idx].as_ref().unwrap();
                        let mut component = c.write()
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
                    let value = component_tree.get_signal(src_idx+offset)?;
                    component_tree.set_signal(dst_idx+offset, value)?;
                    #[cfg(feature = "debug_vm2")]
                    {
                        let src_global = component_tree.signals_start + src_idx + offset;
                        let dst_global = component_tree.signals_start + dst_idx + offset;
                        println!(
                            "CopySignal [S{} -> S{}] = {}",
                            src_global, dst_global, value);
                    }
                }
            }
            OpCode::CopySignalFromCmp => {
                let dst_idx = vm.pop_usize()?;
                let cmp_idx = vm.pop_usize()?;
                let cmp_sig_idx = vm.pop_usize()?;
                let num_signals = vm.pop_usize()?;

                for offset in 0 .. num_signals {
                    let value = match component_tree.components[cmp_idx] {
                        None => {
                            return Err(Box::new(RuntimeError::UninitializedComponent));
                        }
                        Some(ref component) => component
                            .read().unwrap()
                            .get_signal(cmp_sig_idx + offset)?,
                    };
                    component_tree.set_signal(dst_idx+offset, value)?;

                    #[cfg(feature = "debug_vm2")]
                    {
                        let c = component_tree.components[cmp_idx]
                            .as_ref().unwrap()
                            .read().unwrap();
                        let src_sig_idx =
                            c.signals_start + cmp_sig_idx + offset;
                        let dst_sig_idx =
                            component_tree.signals_start + dst_idx + offset;
                        println!(
                            "CopySignalFromCmp [S{} -> S{}]: cmp {} sig {} = {}",
                            src_sig_idx, dst_sig_idx,
                            cmp_idx,
                            cmp_sig_idx + offset,
                            value,
                        );
                    }
                }
            }
            OpCode::CopySignalFromMemory => {
                let dst_idx = vm.pop_usize()?;
                let addr = vm.pop_usize()?;
                let num_signals = vm.pop_usize()?;

                let memory_start = vm.memory_base_pointer_ff
                    .checked_add(addr)
                    .ok_or(RuntimeError::MemoryAddressOutOfBounds)?;

                for offset in 0..num_signals {
                    let src_idx = memory_start
                        .checked_add(offset)
                        .ok_or(RuntimeError::MemoryAddressOutOfBounds)?;
                    if src_idx >= vm.memory_ff.len() {
                        return Err(Box::new(RuntimeError::MemoryAddressOutOfBounds));
                    }
                    let value = vm.memory_ff.get(src_idx)
                        .and_then(|v| v.as_ref())
                        .ok_or(RuntimeError::MemoryVariableIsNotSet)?;

                    component_tree.set_signal(dst_idx+offset, *value)?;

                    #[cfg(feature = "debug_vm2")]
                    {
                        let dst_idx_global = component_tree.signals_start + dst_idx + offset;
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
pub struct Type {
    pub name: String,
    pub fields: Vec<TypeField>,
}

#[derive(Debug, Clone)]
pub struct TypeField {
    pub name: String,
    pub kind: TypeFieldKind,
    pub offset: usize,
    pub size: usize,
    pub dims: Vec<usize>,
}

#[derive(Debug, Clone)]
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
            size: ast_field.size,
            dims: ast_field.dims.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    // use bitvec::vec::BitVec;
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
}
