use std::cell::RefCell;
use std::fmt::{Debug, Display};
use std::num::TryFromIntError;
use std::rc::Rc;
use ruint::aliases::U256;

mod execution;
mod validation;

pub use execution::{build_component, execute};
pub(crate) use validation::validate_compiled_bytecode;

pub struct Component {
    pub vars: Vec<Option<U256>>,
    pub signals_start: usize,
    pub template_id: usize,
    pub subcomponents: Vec<Rc<RefCell<Component>>>,
    pub number_of_inputs: usize,
}

#[cfg_attr(test, derive(Debug))]
#[derive(Clone)]
pub struct Function {
    pub name: String,
    pub symbol: String,
    pub code: Vec<u8>,
    pub line_numbers: Vec<usize>,
}

impl TryInto<crate::proto::vm::Function> for &Function {
    type Error = TryFromIntError;

    fn try_into(self) -> Result<crate::proto::vm::Function, Self::Error> {
        Ok(crate::proto::vm::Function{
            name: self.name.clone(),
            symbol: self.symbol.clone(),
            code: self.code.clone(),
            line_numbers: self.line_numbers.iter()
                .map(|x| TryInto::try_into(*x))
                .collect::<Result<Vec<u64>, TryFromIntError>>()?,
        })
    }
}

impl TryFrom<&crate::proto::vm::Function> for Function {
    type Error = TryFromIntError;

    fn try_from(value: &crate::proto::vm::Function) -> Result<Self, Self::Error> {
        Ok(Function{
            name: value.name.clone(),
            symbol: value.symbol.clone(),
            code: value.code.clone(),
            line_numbers: value.line_numbers
                .iter()
                .map(|x| TryInto::<usize>::try_into(*x))
                .collect::<Result<Vec<usize>, TryFromIntError>>()?,
        })
    }
}

#[cfg_attr(test, derive(Debug))]
#[derive(Clone)]
pub struct Template {
    pub name: String,
    pub code: Vec<u8>,
    pub line_numbers: Vec<usize>,
    pub components: Vec<ComponentTmpl>,
    pub var_stack_depth: usize,
    pub number_of_inputs: usize,
}

impl TryInto<crate::proto::vm::Template> for &Template {
    type Error = TryFromIntError;

    fn try_into(self) -> Result<crate::proto::vm::Template, Self::Error> {
        Ok(crate::proto::vm::Template{
            name:self.name.clone(),
            code: self.code.clone(),
            line_numbers: self.line_numbers.iter()
                .map(|x| TryInto::try_into(*x))
                .collect::<Result<Vec<u64>, TryFromIntError>>()?,
            components: self.components.iter()
                .map(TryInto::<crate::proto::vm::ComponentTmpl>::try_into)
                .collect::<Result<Vec<_>, _>>()?,
            var_stack_depth: self.var_stack_depth.try_into()?,
            number_of_inputs: self.number_of_inputs.try_into()?,
        })
    }
}

impl TryFrom<&crate::proto::vm::Template> for Template {
    type Error = TryFromIntError;

    fn try_from(value: &crate::proto::vm::Template) -> Result<Self, Self::Error> {
        Ok(Template{
            name: value.name.clone(),
            code: value.code.clone(),
            line_numbers: value.line_numbers
                .iter()
                .map(|x| TryInto::<usize>::try_into(*x))
                .collect::<Result<Vec<usize>, TryFromIntError>>()?,
            components: value.components.iter()
                .map(ComponentTmpl::try_from)
                .collect::<Result<Vec<ComponentTmpl>, TryFromIntError>>()?,
            var_stack_depth: value.var_stack_depth.try_into()?,
            number_of_inputs: value.number_of_inputs.try_into()?,
        })
    }
}


#[cfg_attr(test, derive(Debug))]
#[derive(Clone)]
pub struct ComponentTmpl {
    pub symbol: String,
    pub sub_cmp_idx: usize,
    pub number_of_cmp: usize,
    pub name_subcomponent: String,
    pub signal_offset: usize,
    pub signal_offset_jump: usize,
    pub template_id: usize,
    pub has_inputs: bool,
}

impl TryInto<crate::proto::vm::ComponentTmpl> for &ComponentTmpl {
    type Error = TryFromIntError;

    fn try_into(self) -> Result<crate::proto::vm::ComponentTmpl, Self::Error> {
        Ok(crate::proto::vm::ComponentTmpl{
            symbol: self.symbol.clone(),
            sub_cmp_idx: self.sub_cmp_idx.try_into()?,
            number_of_cmp: self.number_of_cmp.try_into()?,
            name_subcomponent: self.name_subcomponent.clone(),
            signal_offset: self.signal_offset.try_into()?,
            signal_offset_jump: self.signal_offset_jump.try_into()?,
            template_id: self.template_id.try_into()?,
            has_inputs: self.has_inputs,
        })
    }
}

impl TryFrom<&crate::proto::vm::ComponentTmpl> for ComponentTmpl {
    type Error = TryFromIntError;

    fn try_from(value: &crate::proto::vm::ComponentTmpl) -> Result<Self, Self::Error> {
        Ok(ComponentTmpl {
            symbol: value.symbol.clone(),
            sub_cmp_idx: value.sub_cmp_idx.try_into()?,
            number_of_cmp: value.number_of_cmp.try_into()?,
            name_subcomponent: value.name_subcomponent.clone(),
            signal_offset: value.signal_offset.try_into()?,
            signal_offset_jump: value.signal_offset_jump.try_into()?,
            template_id: value.template_id.try_into()?,
            has_inputs: value.has_inputs,
        })
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy)]
pub enum OpCode {
    NoOp = 0,
    GetConstant8   =  1, // pushes the value of a constant to the stack. Address of the constant is u64 little endian
    Push8          =  2, // pushes the value of the following 4 bytes as a little endian u64 to the stack
    Push4          =  3, // pushes the value of the following 4 bytes as a little endian u64 to the stack_u32
    GetVariable4   =  4,
    // Put variables to the stack
    // arguments:          variables number as u32 to put on stack
    // required stack_u32: variable index
    GetVariable    =  5,
    SetVariable4   =  6,
    // Set variables from the stack
    // arguments:          variables number u32
    // required stack_u32: variable index
    // required stack:     values to store equal to variables number from arguments
    SetVariable    =  7,
    // Put signals to the stack
    // arguments: signal index u32, signals number u32
    GetSelfSignal4 =  8,
    // Put signals to the stack
    // arguments:          signals number u32
    // required stack_u32: signal index
    GetSelfSignal  =  9,
    SetSelfSignal4 = 10,
    SetSelfSignal  = 11,
    // Put subcomponent signals to the stack
    // arguments:
    // - signals number u32;
    // - flags
    //   - 7th bit is set when it is a mapped signal index
    // - [optional: if flags' 7th bit is set] indexes number u32
    // - [optional: if flags' 7th bit is set] signal code u32
    // required stack_u32:
    // - if signal is not mapped:
    //   - first signal index;
    //   - subcomponent index
    // - if signal mapped:
    //   - mapped indexes (number of indexes passed from the arguments);
    //   - subcomponent index
    GetSubSignal   = 13,
    // arguments:
    // - signals number u32;
    // - flags
    //   - 0-1 bits is an InputStatus;
    //   - 7th bit is set when it is a mapped signal index
    // - [optional: if flags' 7th bit is set] indexes number u32
    // - [optional: if flags' 7th bit is set] signal code u32
    // required stack: values to store (equal to signals number)
    // required stack_u32:
    // - if signal is not mapped:
    //   - first signal index;
    //   - subcomponent index
    // - if signal mapped:
    //   - mapped indexes (number of indexes passed from the arguments);
    //   - subcomponent index
    SetSubSignal   = 14,
    JumpIfFalse    = 15, // Jump to the offset i32 if the value on the top of the stack is false
    Jump           = 16, // Jump to the offset i32
    OpMul          = 17,
    OpDiv          = 18,
    OpAdd          = 19,
    OpSub          = 20,
    OpPow          = 21,
    OpIntDiv       = 22,
    OpMod          = 23,
    OpShL          = 24,
    OpShR          = 25,
    OpLtE          = 26,
    OpGtE          = 27,
    OpLt           = 28,
    OpGt           = 29,
    OpEq           = 30,
    OpNe           = 31,
    OpBoolOr       = 32,
    OpBoolAnd      = 33,
    OpBoolNot      = 34,
    OpBitOr        = 35,
    OpBitAnd       = 36,
    OpBitXor       = 37,
    OpBitNot       = 38,
    OpNeg          = 39,
    OpToAddr       = 40,
    OpMulAddr      = 41,
    OpAddAddr      = 42,
    CmpCall        = 43,
    FnCall         = 44,
    FnReturn       = 45,
    // TODO: Assert should accept an index of the string to print
    Assert         = 46,
}

impl TryFrom<u8> for OpCode {
    type Error = ();

    // The discriminants are not contiguous (12 is unused), so an exhaustive
    // match is the only sound decode: transmuting an out-of-range or gap byte
    // would be undefined behavior.
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Ok(match value {
            0 => OpCode::NoOp,
            1 => OpCode::GetConstant8,
            2 => OpCode::Push8,
            3 => OpCode::Push4,
            4 => OpCode::GetVariable4,
            5 => OpCode::GetVariable,
            6 => OpCode::SetVariable4,
            7 => OpCode::SetVariable,
            8 => OpCode::GetSelfSignal4,
            9 => OpCode::GetSelfSignal,
            10 => OpCode::SetSelfSignal4,
            11 => OpCode::SetSelfSignal,
            13 => OpCode::GetSubSignal,
            14 => OpCode::SetSubSignal,
            15 => OpCode::JumpIfFalse,
            16 => OpCode::Jump,
            17 => OpCode::OpMul,
            18 => OpCode::OpDiv,
            19 => OpCode::OpAdd,
            20 => OpCode::OpSub,
            21 => OpCode::OpPow,
            22 => OpCode::OpIntDiv,
            23 => OpCode::OpMod,
            24 => OpCode::OpShL,
            25 => OpCode::OpShR,
            26 => OpCode::OpLtE,
            27 => OpCode::OpGtE,
            28 => OpCode::OpLt,
            29 => OpCode::OpGt,
            30 => OpCode::OpEq,
            31 => OpCode::OpNe,
            32 => OpCode::OpBoolOr,
            33 => OpCode::OpBoolAnd,
            34 => OpCode::OpBoolNot,
            35 => OpCode::OpBitOr,
            36 => OpCode::OpBitAnd,
            37 => OpCode::OpBitXor,
            38 => OpCode::OpBitNot,
            39 => OpCode::OpNeg,
            40 => OpCode::OpToAddr,
            41 => OpCode::OpMulAddr,
            42 => OpCode::OpAddAddr,
            43 => OpCode::CmpCall,
            44 => OpCode::FnCall,
            45 => OpCode::FnReturn,
            46 => OpCode::Assert,
            _ => return Err(()),
        })
    }
}

fn read_instruction(code: &[u8], ip: usize) -> Result<OpCode, String> {
    let byte = *code.get(ip).ok_or_else(|| {
        format!(
            "instruction pointer {ip} out of bounds (code len {})",
            code.len()
        )
    })?;
    OpCode::try_from(byte)
        .map_err(|()| format!("invalid opcode byte {byte} at ip {ip}"))
}

fn read_usize(code: &[u8], ip: usize) -> usize {
    usize::from_le_bytes(code[ip..ip+size_of::<usize>()].try_into().unwrap())
}

fn read_u32(code: &[u8], ip: usize) -> u32 {
    u32::from_le_bytes(code[ip..ip+size_of::<u32>()].try_into().unwrap())
}

fn read_i32(code: &[u8], ip: usize) -> i32 {
    i32::from_le_bytes(code[ip..ip+size_of::<i32>()].try_into().unwrap())
}

#[repr(u8)]
#[derive(Debug, Clone)]
pub enum InputStatus {
    Last    = 0,
    NoLast  = 1,
    Unknown = 2,
}

impl Display for InputStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = match self {
            InputStatus::Last => "LAST".to_string(),
            InputStatus::NoLast => "NO_LAST".to_string(),
            InputStatus::Unknown => "UNKNOWN".to_string(),
        };
        write!(f, "{}", str)
    }
}

impl TryFrom<u8> for InputStatus {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(InputStatus::Last),
            1 => Ok(InputStatus::NoLast),
            2 => Ok(InputStatus::Unknown),
            _ => Err(()),
        }
    }
}

impl From<InputStatus> for u8 {
    fn from(status: InputStatus) -> u8 {
        status as u8
    }
}

fn unpack_flags(flags: u8) -> (InputStatus, bool) {
    let is_mapped_signal_idx = flags & 0b1000_0000 != 0;
    let input_status = InputStatus::try_from(flags & 0b0000_0011).unwrap();
    (input_status, is_mapped_signal_idx)
}

pub fn disassemble_instruction(
    code: &[u8], line_numbers: &[usize], ip: usize, name: &str,
    functions: &[Function]) -> usize {

    print!("{:08x} [{:10}:{:4}] ", ip, name, line_numbers[ip]);

    let op = match OpCode::try_from(code[ip]) {
        Ok(op) => op,
        Err(()) => {
            println!("<invalid opcode 0x{:02x}>", code[ip]);
            return ip + 1;
        }
    };
    let mut ip = ip + 1;


    match op {
        OpCode::NoOp => {
            println!("NoOp")
        }
        OpCode::SetSelfSignal4 => {
            let sig_idx = read_u32(code, ip);
            ip += size_of::<u32>();

            let sigs_number = read_u32(code, ip);
            ip += size_of::<u32>();

            println!("SetSelfSignal4 [{},{}]", sig_idx, sigs_number);
        }
        OpCode::SetSelfSignal => {
            let sigs_number = read_u32(code, ip);
            ip += size_of::<u32>();

            println!("SetSelfSignal [{}]", sigs_number);
        }
        OpCode::GetConstant8 => {
            let const_idx = read_usize(code, ip);
            ip += size_of::<usize>();

            println!("GetConstant8 [{}]", const_idx);
        }
        OpCode::Push8 => {
            let val = read_usize(code, ip);
            ip += size_of::<usize>();

            println!("Push8 [{}]", val);
        }
        OpCode::Push4 => {
            let val = read_u32(code, ip);
            ip += size_of::<u32>();

            println!("Push4 [{}]", val);
        }
        OpCode::GetVariable4 => {
            let var_idx = read_u32(code, ip);
            ip += size_of::<u32>();

            let vars_number = read_u32(code, ip);
            ip += size_of::<u32>();

            println!("GetVariable4 [{},{}]", var_idx, vars_number);
        }
        OpCode::GetVariable => {
            let vars_number = read_u32(code, ip);
            ip += size_of::<u32>();

            println!("GetVariable [{}]", vars_number);
        }
        OpCode::SetVariable4 => {
            let var_idx = read_u32(code, ip);
            ip += size_of::<u32>();

            let vars_num = read_u32(code, ip);
            ip += size_of::<u32>();

            println!("SetVariable4 [{},{}]", var_idx, vars_num);
        }
        OpCode::SetVariable => {
            let vars_num = read_u32(code, ip);
            ip += size_of::<u32>();

            println!("SetVariable [{}]", vars_num);
        }
        OpCode::GetSubSignal => {
            let sigs_number = read_u32(code, ip);
            ip += size_of::<u32>();

            let flags: u8 = code[ip];
            ip += 1;

            if flags & 0b1000_0000 != 0 {
                let indexes_number = read_u32(code, ip);
                ip += size_of::<u32>();

                let signal_code = read_u32(code, ip);
                ip += size_of::<u32>();

                println!(
                    "GetSubSignal mapped [M,{},{},{}]",
                    sigs_number, indexes_number, signal_code);
            } else {
                println!(
                    "GetSubSignal [NM,{}]", sigs_number);
            }
        }
        OpCode::SetSubSignal => {
            let sigs_number = read_u32(code, ip);
            ip += size_of::<u32>();

            let flags = code[ip];
            ip += 1;

            let (input_status, is_mapped_signal_idx) = unpack_flags(flags);

            if is_mapped_signal_idx {
                let indexes_number = read_u32(code, ip);
                ip += size_of::<u32>();

                let signal_code = read_u32(code, ip);
                ip += size_of::<u32>();

                println!(
                    "SetSubSignal [M,{},{},{},{}]",
                    sigs_number, input_status, indexes_number, signal_code);
            } else {
                println!(
                    "SetSubSignal [NM,{},{}]", sigs_number, input_status);
            }
        }
        OpCode::JumpIfFalse => {
            let offset = read_i32(code, ip);
            ip += size_of::<i32>();

            println!("JumpIfFalse [{} -> {:x}]", offset, ip as i64 + offset as i64);
        }
        OpCode::Jump => {
            let offset = read_i32(code, ip);
            ip += size_of::<i32>();

            println!("Jump [{} -> {:x}]", offset, ip as i64 + offset as i64);
        }
        OpCode::OpMul => {
            println!("OpMul");
        }
        OpCode::OpDiv => {
            println!("OpDiv");
        }
        OpCode::OpAdd => {
            println!("OpAdd");
        }
        OpCode::OpSub => {
            println!("OpSub");
        }
        OpCode::OpPow => {
            println!("OpPow");
        }
        OpCode::OpIntDiv => {
            println!("OpIntDiv");
        }
        OpCode::OpMod => {
            println!("OpMod");
        }
        OpCode::OpShL => {
            println!("OpShL");
        }
        OpCode::OpShR => {
            println!("OpShR");
        }
        OpCode::OpLtE => {
            println!("OpLtE");
        }
        OpCode::OpGtE => {
            println!("OpGtE");
        }
        OpCode::OpLt => {
            println!("OpLt");
        }
        OpCode::OpGt => {
            println!("OpGt");
        }
        OpCode::OpEq => {
            println!("OpEq");
        }
        OpCode::OpNe => {
            println!("OpNe");
        }
        OpCode::OpBoolOr => {
            println!("OpBoolOr");
        }
        OpCode::OpBoolAnd => {
            println!("OpBoolAnd");
        }
        OpCode::OpBoolNot => {
            println!("OpBoolNot");
        }
        OpCode::OpBitOr => {
            println!("OpBitOr");
        }
        OpCode::OpBitAnd => {
            println!("OpBitAnd");
        }
        OpCode::OpBitXor => {
            println!("OpBitXor");
        }
        OpCode::OpBitNot => {
            println!("OpBitNot");
        }
        OpCode::OpNeg => {
            println!("OpNeg");
        }
        OpCode::GetSelfSignal4 => {
            let sig_idx = read_u32(code, ip);
            ip += size_of::<u32>();

            let sigs_number = read_u32(code, ip);
            ip += size_of::<u32>();

            println!("GetSelfSignal4 [{},{}]", sig_idx, sigs_number);
        }
        OpCode::GetSelfSignal => {
            let sigs_number = read_u32(code, ip);
            ip += size_of::<u32>();

            println!("GetSelfSignal [{}]", sigs_number);
        }
        OpCode::OpToAddr => {
            println!("OpToAddr");
        }
        OpCode::OpMulAddr => {
            println!("OpMulAddr");
        }
        OpCode::OpAddAddr => {
            println!("OpAddAddr");
        }
        OpCode::CmpCall => {
            let cmp_idx = read_u32(code, ip);
            ip += size_of::<u32>();

            println!("CmpCall [{}]", cmp_idx);
        }
        OpCode::FnCall => {
            let fn_idx = read_u32(code, ip);
            ip += size_of::<u32>();

            let args_num = read_u32(code, ip);
            ip += size_of::<u32>();

            let return_num = read_u32(code, ip);
            ip += size_of::<u32>();

            let fn_name = &functions[fn_idx as usize].name;

            println!(
                "FnCall [{}:{},{},{}]", fn_idx, fn_name, args_num, return_num);
        }
        OpCode::FnReturn => {
            let return_num = read_u32(code, ip);
            ip += size_of::<u32>();

            println!("FnReturn [{}]", return_num);
        }
        OpCode::Assert => {
            println!("Assert");
        }
    }

    ip
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use ruint::aliases::U256;
    use crate::storage::TemplateInstanceIOMap;
    use super::{build_component, execute, Function, OpCode, Template};

    #[test]
    fn ok() {}

    #[test]
    fn opcode_try_from_rejects_unused_and_out_of_range_bytes() {
        // Defined opcodes decode and round-trip.
        for &byte in &[0u8, 1, 11, 13, 17, 46] {
            assert_eq!(OpCode::try_from(byte).unwrap() as u8, byte);
        }
        // 12 is an unused discriminant; anything above the last opcode is invalid.
        // Both must be rejected rather than transmuted into an invalid variant.
        assert!(OpCode::try_from(12).is_err());
        assert!(OpCode::try_from(47).is_err());
        assert!(OpCode::try_from(u8::MAX).is_err());
    }

    fn execute_unvalidated_code_err(code: Vec<u8>) -> String {
        let templates = vec![Template {
            name: "main".to_string(),
            line_numbers: vec![0; code.len()],
            code,
            components: vec![],
            var_stack_depth: 0,
            number_of_inputs: 0,
        }];
        let functions: Vec<Function> = vec![];
        let constants = vec![];
        let io_map = TemplateInstanceIOMap::new();
        let component = Rc::new(RefCell::new(build_component(&templates, 0, 1)));
        let mut signals = vec![Some(U256::from(1u64))];

        execute(
            component,
            &templates,
            &functions,
            &constants,
            &mut signals,
            &io_map,
            None,
        )
        .unwrap_err()
    }

    #[test]
    fn execute_rejects_invalid_opcode_without_validation() {
        let err = execute_unvalidated_code_err(vec![12]);
        assert_eq!(err, "invalid opcode byte 12 at ip 0");
    }

    #[test]
    fn execute_rejects_out_of_bounds_instruction_pointer() {
        let mut code = vec![OpCode::Jump as u8];
        code.extend(1_i32.to_le_bytes());
        let err = execute_unvalidated_code_err(code);
        assert_eq!(err, "instruction pointer 6 out of bounds (code len 5)");
    }
}
