use std::cell::RefCell;
use std::cmp::Ordering;
use std::fmt::{Debug, Display};
use std::num::TryFromIntError;
use std::rc::Rc;
use ruint::aliases::U256;
use crate::field::M;
use crate::graph::{Operation, UnoOperation};
use crate::storage::TemplateInstanceIOMap;

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

enum Frame<'a, 'b> {
    Component {
        ip: usize,
        code: &'a [u8],
        component: Rc<RefCell<Component>>,
    },
    Function {
        ip: usize,
        vars: Vec<Option<U256>>,
        code: &'a [u8],
        function: &'b Function,
        return_num: usize,
    }
}

impl Frame<'_, '_> {
    fn new_component(component: Rc<RefCell<Component>>, templates: &[Template]) -> Frame<'_, '_> {
        let template_id = component.borrow().template_id;
        Frame::Component {
            ip: 0,
            code: &templates[template_id].code,
            component,
        }
    }

    fn new_function(
        fn_idx: usize, functions: &[Function], args_num: usize,
        return_num: usize) -> Frame<'_, '_> {

        Frame::Function {
            ip: 0,
            vars: Vec::with_capacity(args_num),
            code: &functions[fn_idx].code,
            function: &functions[fn_idx],
            return_num,
        }
    }
}

struct VM<'a, 'b> {
    templates: &'a Vec<Template>,
    signals: &'a mut [Option<U256>],
    constants: &'a Vec<U256>,
    stack: Vec<U256>,
    stack_u32: Vec<u32>,
    expected_signals: Option<&'a Vec<U256>>,
    call_frames: Vec<Frame<'a, 'b>>,
}

impl VM<'_, '_> {
    fn assert_signal(&self, sig_idx: usize) {
        if let Some(expected_signals) = self.expected_signals {
            let cmp = expected_signals[sig_idx].cmp(&self.signals[sig_idx].unwrap());
            match cmp {
                Ordering::Equal => (),
                _ => {
                    self.print_stack_trace();
                    panic!(
                        "Signal at {} want {}, got {}", sig_idx,
                        expected_signals[sig_idx],
                        self.signals[sig_idx].unwrap())
                },
            }
        }
    }

    /// Set the signal sig_idx to the value from the top of the stack
    fn set_signal(&mut self, sig_idx: usize) {
        if self.signals[sig_idx].is_some() {
            panic!("Signal already set");
        }
        let v = self.stack.pop().unwrap();
        if v > M {
            panic!("v = {}", v);
        }
        self.signals[sig_idx] = Some(v);
        self.assert_signal(sig_idx);
    }

    #[cfg(feature = "print_opcode")]
    fn print_stack(&self) {
        for (i, s) in self.stack.iter().enumerate() {
            let s = if s.is_zero() { String:: from("0") } else { s.to_string() };
            println!("{:04x}: {}", i, s);
        }
    }
    #[cfg(feature = "print_opcode")]
    fn print_stack_u32(&self) {
        for (i, s) in self.stack_u32.iter().enumerate() {
            println!("{:04x}: {}", i, s);
        }
    }

    fn print_stack_trace(&self) {
        for frame in self.call_frames.iter().rev() {
            match frame {
                Frame::Component { component, ip, .. } => {
                    let component = component.borrow();
                    let template_id = component.template_id;
                    let line_no = if *ip < self.templates[template_id].line_numbers.len() {
                        self.templates[template_id].line_numbers[*ip]
                    } else {
                        self.templates[template_id].line_numbers
                            .iter()
                            .rev()
                            .find(|&&x| x != usize::MAX)
                            .copied()
                            .unwrap_or(0)
                    };
                    let tmpl_name = self.templates[template_id].name.clone();
                    println!("Component: {}:{}", tmpl_name, line_no);
                }
                Frame::Function { function, ip, .. } => {
                    let line_no = function.line_numbers[*ip];
                    println!("Function: {}:{}", function.name, line_no);
                }
            }
        }
    }

    fn push_stack(&mut self, v: U256) {
        if v > M {
            panic!("v = {}", v);
        }
        self.stack.push(v);
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

pub(crate) fn validate_compiled_bytecode(
    templates: &[Template], functions: &[Function],
    io_map: &TemplateInstanceIOMap, constants_len: usize,
    main_template_id: usize) -> Result<(), String> {

    if main_template_id >= templates.len() {
        return Err(format!(
            "main template id {} is out of bounds (templates: {})",
            main_template_id, templates.len()));
    }

    validate_component_graph(templates, main_template_id)?;

    let function_returns = functions.iter().enumerate()
        .map(|(idx, function)| {
            legacy_function_return_arity(
                &function.code,
                &format!("function {} ({})", idx, function.name))
        })
        .collect::<Result<Vec<_>, _>>()?;

    for (idx, template) in templates.iter().enumerate() {
        let owner = format!("template {} ({})", idx, template.name);
        let component_count = expanded_component_count(template)?;
        validate_instruction_stream(
            &template.code, &owner,
            CodeContext::Template {
                component_count,
                template_id: idx,
            },
            templates, io_map, constants_len, &function_returns)?;
    }

    for (idx, function) in functions.iter().enumerate() {
        let owner = format!("function {} ({})", idx, function.name);
        validate_instruction_stream(
            &function.code, &owner, CodeContext::Function,
            templates, io_map, constants_len, &function_returns)?;
    }

    Ok(())
}

#[derive(Clone, Copy)]
enum CodeContext {
    Template { component_count: usize, template_id: usize },
    Function,
}

fn expanded_component_count(template: &Template) -> Result<usize, String> {
    template.components.iter().try_fold(0usize, |acc, component| {
        acc.checked_add(component.number_of_cmp)
            .ok_or_else(|| format!(
                "template {} has too many expanded subcomponents",
                template.name))
    })
}

fn validate_component_graph(
    templates: &[Template], main_template_id: usize) -> Result<(), String> {

    #[derive(Clone, Copy, PartialEq)]
    enum Mark { Unvisited, OnPath }

    fn walk(
        template_id: usize, signals_start: usize, templates: &[Template],
        marks: &mut [Mark], max_checked_start: &mut [Option<usize>])
        -> Result<(), String> {

        let template = templates.get(template_id)
            .ok_or_else(|| format!(
                "template id {} is out of bounds (templates: {})",
                template_id, templates.len()))?;

        match marks[template_id] {
            Mark::OnPath => {
                return Err(format!(
                    "component graph contains a cycle at template {} ({})",
                    template_id, template.name));
            }
            Mark::Unvisited => {}
        }
        if let Some(max_start) = max_checked_start[template_id] {
            if max_start >= signals_start {
                return Ok(());
            }
        }

        marks[template_id] = Mark::OnPath;
        for component in &template.components {
            if component.template_id >= templates.len() {
                return Err(format!(
                    "template {} ({}) references template id {} (templates: {})",
                    template_id, template.name,
                    component.template_id, templates.len()));
            }

            component.sub_cmp_idx.checked_add(component.number_of_cmp)
                .ok_or_else(|| format!(
                    "template {} ({}) has overflowing component range",
                    template_id, template.name))?;

            if component.number_of_cmp > 0 {
                let last_instance = component.number_of_cmp - 1;
                let last_jump = component.signal_offset_jump
                    .checked_mul(last_instance)
                    .ok_or_else(|| format!(
                        "template {} ({}) has overflowing component signal stride",
                        template_id, template.name))?;
                let last_offset = component.signal_offset
                    .checked_add(last_jump)
                    .ok_or_else(|| format!(
                        "template {} ({}) has overflowing component signal offset",
                        template_id, template.name))?;
                signals_start.checked_add(component.signal_offset)
                    .ok_or_else(|| format!(
                        "template {} ({}) has overflowing first signal start",
                        template_id, template.name))?;
                let child_signals_start = signals_start.checked_add(last_offset)
                    .ok_or_else(|| format!(
                        "template {} ({}) has overflowing last signal start",
                        template_id, template.name))?;

                walk(
                    component.template_id, child_signals_start, templates,
                    marks, max_checked_start)?;
            }
        }
        marks[template_id] = Mark::Unvisited;
        max_checked_start[template_id] = Some(
            max_checked_start[template_id].map_or(signals_start, |max_start| {
                max_start.max(signals_start)
            }));

        Ok(())
    }

    let mut marks = vec![Mark::Unvisited; templates.len()];
    let mut max_checked_start = vec![None; templates.len()];
    walk(main_template_id, 1, templates, &mut marks, &mut max_checked_start)
}

fn validate_instruction_stream(
    code: &[u8], owner: &str, context: CodeContext,
    templates: &[Template], io_map: &TemplateInstanceIOMap,
    constants_len: usize, function_returns: &[usize]) -> Result<(), String> {

    let mut ip = 0usize;
    let mut instruction_starts = Vec::new();
    let mut jump_targets = Vec::new();
    while ip < code.len() {
        let op_ip = ip;
        instruction_starts.push(op_ip);
        let byte = code[op_ip];
        let op = OpCode::try_from(byte)
            .map_err(|()| format!(
                "{} has invalid opcode byte {} at ip {}",
                owner, byte, op_ip))?;
        ip += 1;

        match op {
            OpCode::NoOp | OpCode::Assert => {}

            OpCode::OpMul
            | OpCode::OpDiv
            | OpCode::OpAdd
            | OpCode::OpSub
            | OpCode::OpPow
            | OpCode::OpIntDiv
            | OpCode::OpMod
            | OpCode::OpShL
            | OpCode::OpShR
            | OpCode::OpLtE
            | OpCode::OpGtE
            | OpCode::OpLt
            | OpCode::OpGt
            | OpCode::OpEq
            | OpCode::OpNe
            | OpCode::OpBoolOr
            | OpCode::OpBoolAnd
            | OpCode::OpBitOr
            | OpCode::OpBitAnd
            | OpCode::OpBitXor => {}

            OpCode::OpBoolNot | OpCode::OpBitNot | OpCode::OpNeg => {}

            OpCode::OpToAddr => {}

            OpCode::OpMulAddr | OpCode::OpAddAddr => {}

            OpCode::GetConstant8 => {
                let const_ip = take_operand(
                    code, owner, op, &mut ip, size_of::<usize>())?;
                let const_idx = read_usize(code, const_ip);
                if const_idx >= constants_len {
                    return Err(format!(
                        "{} has GetConstant8 target {} at ip {}, but only {} constants exist",
                        owner, const_idx, op_ip, constants_len));
                }
            }
            OpCode::Push8 => {
                take_operand(code, owner, op, &mut ip, size_of::<usize>())?;
            }
            OpCode::Push4 => {
                take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
            }
            OpCode::GetVariable4 => {
                take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
            }
            OpCode::GetVariable => {
                take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
            }
            OpCode::SetVariable4 => {
                take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
            }
            OpCode::SetVariable => {
                take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
            }
            OpCode::GetSelfSignal | OpCode::SetSelfSignal => {
                validate_component_context(owner, context, op)?;
                take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
            }
            OpCode::GetSelfSignal4 | OpCode::SetSelfSignal4 => {
                validate_component_context(owner, context, op)?;
                take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
            }
            OpCode::GetSubSignal => {
                validate_component_context(owner, context, op)?;
                take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                let flags_ip = take_operand(code, owner, op, &mut ip, 1)?;
                if code[flags_ip] & 0b1000_0000 != 0 {
                    let indexes_num_ip = take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                    let indexes_num = read_u32(code, indexes_num_ip) as usize;
                    let signal_code_ip = take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                    validate_mapped_signal_operand(
                        templates, io_map, context,
                        read_u32(code, signal_code_ip), indexes_num,
                        owner, op_ip)?;
                }
            }
            OpCode::SetSubSignal => {
                validate_component_context(owner, context, op)?;
                take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                let flags_ip = take_operand(code, owner, op, &mut ip, 1)?;
                let flags = code[flags_ip];
                InputStatus::try_from(flags & 0b0000_0011)
                    .map_err(|()| format!(
                        "{} has invalid SetSubSignal flags {} at ip {}",
                        owner, flags, flags_ip))?;
                if flags & 0b1000_0000 != 0 {
                    let indexes_num_ip = take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                    let indexes_num = read_u32(code, indexes_num_ip) as usize;
                    let signal_code_ip = take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                    validate_mapped_signal_operand(
                        templates, io_map, context,
                        read_u32(code, signal_code_ip), indexes_num,
                        owner, op_ip)?;
                }
            }
            OpCode::JumpIfFalse | OpCode::Jump => {
                let offset_ip = take_operand(
                    code, owner, op, &mut ip, size_of::<i32>())?;
                let offset = read_i32(code, offset_ip);
                jump_targets.push((
                    validate_jump_target(code.len(), ip, offset, owner, op, op_ip)?,
                    op,
                    op_ip));
            }
            OpCode::CmpCall => {
                let cmp_ip = take_operand(
                    code, owner, op, &mut ip, size_of::<u32>())?;
                let cmp_idx = read_u32(code, cmp_ip) as usize;
                match context {
                    CodeContext::Template { component_count, .. } => {
                        if cmp_idx >= component_count {
                            return Err(format!(
                                "{} has CmpCall target {} at ip {}, but only {} subcomponents exist",
                                owner, cmp_idx, op_ip, component_count));
                        }
                    }
                    CodeContext::Function => validate_component_context(owner, context, op)?,
                }
            }
            OpCode::FnCall => {
                let fn_ip = take_operand(
                    code, owner, op, &mut ip, size_of::<u32>())?;
                let fn_idx = read_u32(code, fn_ip) as usize;
                let Some(function_return_num) = function_returns.get(fn_idx) else {
                    return Err(format!(
                        "{} has FnCall target {} at ip {}, but only {} functions exist",
                        owner, fn_idx, op_ip, function_returns.len()));
                };
                take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                let return_num_ip = take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                let return_num = read_u32(code, return_num_ip) as usize;
                if return_num != *function_return_num {
                    return Err(format!(
                        "{} has FnCall return count {} at ip {}, but function {} returns {}",
                        owner, return_num, op_ip, fn_idx, function_return_num));
                }
            }
            OpCode::FnReturn => {
                if let CodeContext::Template { .. } = context {
                    return Err(format!(
                        "{} has FnReturn in template code at ip {}",
                        owner, op_ip));
                }
                take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
            }
        }
    }

    for (target, op, op_ip) in jump_targets {
        if target == code.len() {
            continue;
        } else if !instruction_starts.contains(&target) {
            return Err(format!(
                "{} has {:?} at ip {} targeting non-instruction byte {}",
                owner, op, op_ip, target));
        }
    }
    validate_stack_cfg(code, owner, context)?;

    Ok(())
}

fn validate_mapped_signal_operand(
    templates: &[Template], io_map: &TemplateInstanceIOMap,
    context: CodeContext, signal_code: u32, indexes_num: usize,
    owner: &str, op_ip: usize) -> Result<(), String> {

    let CodeContext::Template { template_id, .. } = context else {
        return Ok(());
    };
    let template = &templates[template_id];
    let mut checked_any_child = false;
    let mut checked_compatible_child = false;
    for component in template.components.iter()
        .filter(|component| component.number_of_cmp > 0) {

        checked_any_child = true;
        if let Some(io_defs) = io_map.get(&component.template_id) {
            if let Some(io_def) = io_defs.get(signal_code as usize) {
                if io_def.lengths.len() == indexes_num {
                    checked_compatible_child = true;
                }
            }
        }
    }
    if !checked_any_child {
        return Err(format!(
            "{} has mapped signal access at ip {}, but template {} has no subcomponents",
            owner, op_ip, template_id));
    }
    if !checked_compatible_child {
        return Err(format!(
            "{} has mapped signal code {} at ip {} with {} indexes, but no subcomponent template has a compatible io_map entry",
            owner, signal_code, op_ip, indexes_num));
    }

    Ok(())
}

fn legacy_function_return_arity(code: &[u8], owner: &str) -> Result<usize, String> {
    let mut ip = 0usize;
    let mut return_num = None;
    while ip < code.len() {
        let op_ip = ip;
        let op = OpCode::try_from(code[op_ip])
            .map_err(|()| format!(
                "{} has invalid opcode byte {} at ip {}",
                owner, code[op_ip], op_ip))?;
        ip += 1;

        match op {
            OpCode::GetConstant8 | OpCode::Push8 => {
                take_operand(code, owner, op, &mut ip, size_of::<usize>())?;
            }
            OpCode::Push4
            | OpCode::GetVariable
            | OpCode::SetVariable
            | OpCode::JumpIfFalse
            | OpCode::Jump
            | OpCode::CmpCall => {
                take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
            }
            OpCode::GetVariable4
            | OpCode::SetVariable4
            | OpCode::GetSelfSignal4
            | OpCode::SetSelfSignal4 => {
                take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
            }
            OpCode::GetSelfSignal | OpCode::SetSelfSignal => {
                take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
            }
            OpCode::GetSubSignal | OpCode::SetSubSignal => {
                take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                let flags_ip = take_operand(code, owner, op, &mut ip, 1)?;
                if code[flags_ip] & 0b1000_0000 != 0 {
                    take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                    take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                }
            }
            OpCode::FnCall => {
                take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
            }
            OpCode::FnReturn => {
                let return_num_ip = take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                let current = read_u32(code, return_num_ip) as usize;
                match return_num {
                    Some(expected) if expected != current => {
                        return Err(format!(
                            "{} has inconsistent FnReturn counts {} and {}",
                            owner, expected, current));
                    }
                    Some(_) => {}
                    None => return_num = Some(current),
                }
            }
            OpCode::NoOp
            | OpCode::OpMul
            | OpCode::OpDiv
            | OpCode::OpAdd
            | OpCode::OpSub
            | OpCode::OpPow
            | OpCode::OpIntDiv
            | OpCode::OpMod
            | OpCode::OpShL
            | OpCode::OpShR
            | OpCode::OpLtE
            | OpCode::OpGtE
            | OpCode::OpLt
            | OpCode::OpGt
            | OpCode::OpEq
            | OpCode::OpNe
            | OpCode::OpBoolOr
            | OpCode::OpBoolAnd
            | OpCode::OpBoolNot
            | OpCode::OpBitOr
            | OpCode::OpBitAnd
            | OpCode::OpBitXor
            | OpCode::OpBitNot
            | OpCode::OpNeg
            | OpCode::OpToAddr
            | OpCode::OpMulAddr
            | OpCode::OpAddAddr
            | OpCode::Assert => {}
        }
    }

    return_num.ok_or_else(|| format!("{} falls through without FnReturn", owner))
}

#[derive(Clone, Copy)]
struct StackInstruction {
    start: usize,
    next: usize,
    op: OpCode,
    jump_target: Option<usize>,
    pop_field: usize,
    push_field: usize,
    pop_address: usize,
    push_address: usize,
}

fn validate_stack_cfg(
    code: &[u8], owner: &str, context: CodeContext) -> Result<(), String> {

    let instructions = decode_stack_instructions(code, owner)?;
    if instructions.is_empty() {
        return Ok(());
    }

    let mut states = vec![None; instructions.len()];
    let mut work = vec![0usize];
    states[0] = Some((0usize, 0usize));

    while let Some(idx) = work.pop() {
        let (mut field_stack, mut address_stack) = states[idx].unwrap();
        let inst = instructions[idx];
        pop_stack(
            &mut field_stack, inst.pop_field,
            owner, inst.op, inst.start, "field")?;
        pop_stack(
            &mut address_stack, inst.pop_address,
            owner, inst.op, inst.start, "address")?;
        push_stack_count(
            &mut field_stack, inst.push_field,
            owner, inst.op, inst.start, "field")?;
        push_stack_count(
            &mut address_stack, inst.push_address,
            owner, inst.op, inst.start, "address")?;

        let mut successors = Vec::new();
        match inst.op {
            OpCode::Jump => {
                if let Some(target) = inst.jump_target {
                    successors.push(target);
                }
            }
            OpCode::JumpIfFalse => {
                if let Some(target) = inst.jump_target {
                    successors.push(target);
                }
                successors.push(inst.next);
            }
            OpCode::FnReturn => {}
            _ => successors.push(inst.next),
        }

        for target in successors {
            if target == code.len() {
                if matches!(context, CodeContext::Function) {
                    return Err(format!(
                        "{} reaches function fallthrough from {:?} at ip {}",
                        owner, inst.op, inst.start));
                }
                continue;
            }
            let Some(target_idx) = instructions.iter()
                .position(|candidate| candidate.start == target) else {
                    return Err(format!(
                        "{} reaches non-instruction byte {} from {:?} at ip {}",
                        owner, target, inst.op, inst.start));
                };
            let next_state = (field_stack, address_stack);
            match states[target_idx] {
                Some(existing) if existing != next_state => {
                    return Err(format!(
                        "{} reaches ip {} with inconsistent stack depths {:?} and {:?}",
                        owner, target, existing, next_state));
                }
                Some(_) => {}
                None => {
                    states[target_idx] = Some(next_state);
                    work.push(target_idx);
                }
            }
        }
    }

    Ok(())
}

fn decode_stack_instructions(
    code: &[u8], owner: &str) -> Result<Vec<StackInstruction>, String> {

    let mut ip = 0usize;
    let mut instructions = Vec::new();
    while ip < code.len() {
        let start = ip;
        let op = OpCode::try_from(code[start])
            .map_err(|()| format!(
                "{} has invalid opcode byte {} at ip {}",
                owner, code[start], start))?;
        ip += 1;

        let mut inst = StackInstruction {
            start,
            next: ip,
            op,
            jump_target: None,
            pop_field: 0,
            push_field: 0,
            pop_address: 0,
            push_address: 0,
        };

        match op {
            OpCode::NoOp | OpCode::Assert => {}
            OpCode::OpMul
            | OpCode::OpDiv
            | OpCode::OpAdd
            | OpCode::OpSub
            | OpCode::OpPow
            | OpCode::OpIntDiv
            | OpCode::OpMod
            | OpCode::OpShL
            | OpCode::OpShR
            | OpCode::OpLtE
            | OpCode::OpGtE
            | OpCode::OpLt
            | OpCode::OpGt
            | OpCode::OpEq
            | OpCode::OpNe
            | OpCode::OpBoolOr
            | OpCode::OpBoolAnd
            | OpCode::OpBitOr
            | OpCode::OpBitAnd
            | OpCode::OpBitXor => {
                inst.pop_field = 2;
                inst.push_field = 1;
            }
            OpCode::OpBoolNot | OpCode::OpBitNot | OpCode::OpNeg => {
                inst.pop_field = 1;
                inst.push_field = 1;
            }
            OpCode::OpToAddr => {
                inst.pop_field = 1;
                inst.push_address = 1;
            }
            OpCode::OpMulAddr | OpCode::OpAddAddr => {
                inst.pop_address = 2;
                inst.push_address = 1;
            }
            OpCode::GetConstant8 | OpCode::Push8 => {
                take_operand(code, owner, op, &mut ip, size_of::<usize>())?;
                inst.push_field = 1;
            }
            OpCode::Push4 => {
                take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                inst.push_address = 1;
            }
            OpCode::GetVariable4 => {
                take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                let vars_num_ip = take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                inst.push_field = read_u32(code, vars_num_ip) as usize;
            }
            OpCode::GetVariable => {
                let vars_num_ip = take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                inst.pop_address = 1;
                inst.push_field = read_u32(code, vars_num_ip) as usize;
            }
            OpCode::SetVariable4 => {
                take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                let vars_num_ip = take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                inst.pop_field = read_u32(code, vars_num_ip) as usize;
            }
            OpCode::SetVariable => {
                let vars_num_ip = take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                inst.pop_address = 1;
                inst.pop_field = read_u32(code, vars_num_ip) as usize;
            }
            OpCode::GetSelfSignal => {
                let sigs_num_ip = take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                inst.pop_address = 1;
                inst.push_field = read_u32(code, sigs_num_ip) as usize;
            }
            OpCode::SetSelfSignal => {
                let sigs_num_ip = take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                inst.pop_address = 1;
                inst.pop_field = read_u32(code, sigs_num_ip) as usize;
            }
            OpCode::GetSelfSignal4 => {
                take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                let sigs_num_ip = take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                inst.push_field = read_u32(code, sigs_num_ip) as usize;
            }
            OpCode::SetSelfSignal4 => {
                take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                let sigs_num_ip = take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                inst.pop_field = read_u32(code, sigs_num_ip) as usize;
            }
            OpCode::GetSubSignal => {
                let sigs_num_ip = take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                let flags_ip = take_operand(code, owner, op, &mut ip, 1)?;
                inst.pop_address = 1;
                if code[flags_ip] & 0b1000_0000 != 0 {
                    let indexes_num_ip = take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                    inst.pop_address = inst.pop_address
                        .checked_add(read_u32(code, indexes_num_ip) as usize)
                        .ok_or_else(|| format!(
                            "{} has overflowing address stack need at ip {}",
                            owner, start))?;
                    take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                } else {
                    inst.pop_address += 1;
                }
                inst.push_field = read_u32(code, sigs_num_ip) as usize;
            }
            OpCode::SetSubSignal => {
                let sigs_num_ip = take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                let flags_ip = take_operand(code, owner, op, &mut ip, 1)?;
                inst.pop_address = 1;
                if code[flags_ip] & 0b1000_0000 != 0 {
                    let indexes_num_ip = take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                    inst.pop_address = inst.pop_address
                        .checked_add(read_u32(code, indexes_num_ip) as usize)
                        .ok_or_else(|| format!(
                            "{} has overflowing address stack need at ip {}",
                            owner, start))?;
                    take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                } else {
                    inst.pop_address += 1;
                }
                inst.pop_field = read_u32(code, sigs_num_ip) as usize;
            }
            OpCode::JumpIfFalse | OpCode::Jump => {
                let offset_ip = take_operand(code, owner, op, &mut ip, size_of::<i32>())?;
                inst.jump_target = Some(validate_jump_target(
                    code.len(), ip, read_i32(code, offset_ip), owner, op, start)?);
                if matches!(op, OpCode::JumpIfFalse) {
                    inst.pop_field = 1;
                }
            }
            OpCode::CmpCall => {
                take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
            }
            OpCode::FnCall => {
                take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                let args_num_ip = take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                let return_num_ip = take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                inst.pop_field = read_u32(code, args_num_ip) as usize;
                inst.push_field = read_u32(code, return_num_ip) as usize;
            }
            OpCode::FnReturn => {
                let return_num_ip = take_operand(code, owner, op, &mut ip, size_of::<u32>())?;
                inst.pop_field = read_u32(code, return_num_ip) as usize;
            }
        }

        inst.next = ip;
        instructions.push(inst);
    }

    Ok(instructions)
}

fn validate_component_context(
    owner: &str, context: CodeContext, op: OpCode) -> Result<(), String> {

    if matches!(context, CodeContext::Function) {
        return Err(format!(
            "{} has component-only opcode {:?} in function code",
            owner, op));
    }
    Ok(())
}

fn pop_stack(
    stack: &mut usize, count: usize, owner: &str, op: OpCode, op_ip: usize,
    name: &str) -> Result<(), String> {

    if *stack < count {
        return Err(format!(
            "{} has {:?} at ip {} with {} {} stack values available, needs {}",
            owner, op, op_ip, *stack, name, count));
    }
    *stack -= count;
    Ok(())
}

fn push_stack_count(
    stack: &mut usize, count: usize, owner: &str, op: OpCode, op_ip: usize,
    name: &str) -> Result<(), String> {

    *stack = stack.checked_add(count)
        .ok_or_else(|| format!(
            "{} has {:?} at ip {} overflowing {} stack depth",
            owner, op, op_ip, name))?;
    Ok(())
}

fn take_operand(
    code: &[u8], owner: &str, op: OpCode, ip: &mut usize,
    len: usize) -> Result<usize, String> {

    let start = *ip;
    let end = start.checked_add(len)
        .ok_or_else(|| format!(
            "{} has overflowing operand for {:?} at ip {}",
            owner, op, start))?;
    if end > code.len() {
        return Err(format!(
            "{} has truncated operand for {:?} at ip {}: needs {} bytes, code len {}",
            owner, op, start, len, code.len()));
    }
    *ip = end;
    Ok(start)
}

fn validate_jump_target(
    code_len: usize, ip_after_operand: usize, offset: i32, owner: &str,
    op: OpCode, op_ip: usize) -> Result<usize, String> {

    let target = if offset < 0 {
        ip_after_operand.checked_sub(offset.unsigned_abs() as usize)
    } else {
        ip_after_operand.checked_add(offset as usize)
    };
    match target {
        Some(target) if target <= code_len => Ok(target),
        _ => Err(format!(
            "{} has out-of-bounds {:?} target from ip {} with offset {}",
            owner, op, op_ip, offset)),
    }
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

fn calc_mapped_signal_idx(
    template_id: usize, io_map: &TemplateInstanceIOMap,
    signal_code: u32, indexes: &[u32]) -> Result<u32, String> {

    let signals = io_map.get(&template_id)
        .ok_or_else(|| format!("template not found in io_map: {}", template_id))?;
    let def = signals.get(signal_code as usize)
        .ok_or_else(|| format!(
            "signal code {} not found in io_map for template {}",
            signal_code, template_id))?;
    let mut sig_idx: u32 = def.offset.try_into()
        .map_err(|_| "Signal index is too large".to_string())?;

    if indexes.is_empty() {
        if def.lengths.is_empty() {
            return Ok(sig_idx);
        }
        return Err(format!(
            "mapped signal code {} for template {} expects {} indexes, got 0",
            signal_code, template_id, def.lengths.len()));
    }

    if def.lengths.len() != indexes.len() {
        return Err(format!(
            "mapped signal code {} for template {} expects {} indexes, got {}",
            signal_code, template_id, def.lengths.len(), indexes.len()));
    }

    // Compute strides
    let mut strides = vec![1u32; def.lengths.len()];
    for i in (0..def.lengths.len() - 1).rev() {
        let ln: u32 = def.lengths[i+1].try_into()
            .map_err(|_| "Length is too large".to_string())?;
        strides[i] = strides[i+1].checked_mul(ln)
            .ok_or_else(|| "Stride is too large".to_string())?;
    }

    for (i, idx_ip) in indexes.iter().enumerate() {
        let x = idx_ip.checked_mul(strides[i])
            .ok_or_else(|| "Index is too large".to_string())?;

        sig_idx = sig_idx.checked_add(x)
            .ok_or_else(|| "Signal index is too large".to_string())?;
    };

    Ok(sig_idx)
}

fn get_subcomponent(
    component: &Rc<RefCell<Component>>, cmp_idx: u32) -> Result<Rc<RefCell<Component>>, String> {

    let cmp_idx: usize = cmp_idx.try_into()
        .map_err(|_| "Subcomponent index is too large".to_string())?;
    component.borrow().subcomponents.get(cmp_idx)
        .cloned()
        .ok_or_else(|| format!("Subcomponent index {} is out of bounds", cmp_idx))
}

pub fn build_component(
    compiled_templates: &[Template],
    template_id: usize, signals_start: usize) -> Component {

    let mut subcomponents = Vec::with_capacity(
        compiled_templates[template_id].components.len());

    for c in &compiled_templates[template_id].components {
        let mut cmp_signal_offset = c.signal_offset;

        for _ in c.sub_cmp_idx..c.sub_cmp_idx+c.number_of_cmp {
            let subcomponent = build_component(
                compiled_templates, c.template_id,
                signals_start + cmp_signal_offset);
            subcomponents.push(Rc::new(RefCell::new(subcomponent)));
            cmp_signal_offset += c.signal_offset_jump;
        }
    }

    Component {
        vars: vec![None; compiled_templates[template_id].var_stack_depth],
        signals_start,
        template_id,
        subcomponents,
        number_of_inputs: compiled_templates[template_id].number_of_inputs,
    }
}

// pub fn print_component_tree(c: &Component, indent: usize, templates: &[Template]) {
//     if indent == 0 {
//         println!("Component tree:");
//     }
//     let indent_str = " ".repeat(indent);
//     println!("{}{}", indent_str, templates[c.template_id].name);
//     for sc in &c.subcomponents {
//         print_component_tree(&sc.borrow(), indent + 2, templates);
//     }
// }

pub fn execute(
    component: Rc<RefCell<Component>>, templates: &Vec<Template>,
    functions: &[Function], constants: &Vec<U256>,
    signals: &mut [Option<U256>], io_map: &TemplateInstanceIOMap,
    expected_signals: Option<&Vec<U256>>) -> Result<(), String> {

    let mut vm = VM {
        templates,
        signals,
        constants,
        stack: vec![],
        stack_u32: vec![],
        expected_signals,
        call_frames: vec![],
    };

    vm.call_frames.push(Frame::new_component(component.clone(), templates));

    let mut ip = 0usize;
    let mut code: &[u8];
    #[cfg(feature = "print_opcode")]
    let mut template_id: usize;
    let mut signals_start: usize;
    {
        match vm.call_frames.last().unwrap() {
            Frame::Component { code: code_local, component, .. } => {
                code = *code_local;
                #[cfg(feature = "print_opcode")]
                {
                    template_id = component.borrow().template_id;
                }
                signals_start = component.borrow().signals_start;
            }
            Frame::Function { .. } => {
                panic!("Function frame on top of the call stack");
            }
        }
    }

    'main: loop {
        while ip == code.len() {
            vm.call_frames.pop();
            if vm.call_frames.is_empty() {
                break 'main;
            }

            let last_frame = vm.call_frames.last().unwrap();
            match last_frame {
                Frame::Component { ip: ip_local, component, .. } => {
                    ip = *ip_local;
                    let component = component.borrow();
                    #[cfg(feature = "print_opcode")]
                    {
                        template_id = component.template_id;
                    }
                    code = &templates[component.template_id].code;
                    signals_start = component.signals_start;
                }
                Frame::Function { .. } => {
                    panic!("Suppose to exit from the function with the Return statement");
                }
            }
        }

        #[cfg(feature = "print_opcode")]
        {
            let (line_numbers, name) = match vm.call_frames.last().unwrap() {
                Frame::Component { .. } => {
                    (&templates[template_id].line_numbers, templates[template_id].name.as_str())
                }
                Frame::Function { function, .. } => {
                    (&function.line_numbers, function.name.as_str())
                }
            };
            disassemble_instruction(code, line_numbers, ip, name, functions);
        }

        let op = read_instruction(code, ip)?;
        ip += 1;

        match op {
            OpCode::NoOp => {
                // do nothing
            }
            OpCode::SetSelfSignal4 => {
                let sig_offset = read_u32(code, ip);
                ip += size_of::<u32>();

                let sigs_number = read_u32(code, ip);
                ip += size_of::<u32>();

                let sig_offset = usize::try_from(sig_offset)
                    .expect("Signal index is too large");

                let (sig_start, overflowed) = signals_start
                    .overflowing_add(sig_offset);

                if overflowed {
                    panic!("Signal index is too large");
                }

                let (sig_end, overflowed) = sig_start
                    .overflowing_add(sigs_number as usize);

                if overflowed || sig_end > vm.signals.len() {
                    panic!("Signal index is too large");
                }

                for sig_idx in (sig_start..sig_end).rev() {
                    vm.set_signal(sig_idx);
                }
            }
            OpCode::SetSelfSignal => {
                let cmp_signal_offset =
                    vm.stack_u32.pop().unwrap() as usize;

                let sigs_number = read_u32(code, ip);
                ip += size_of::<u32>();

                let (sig_start, overflowed) = signals_start
                    .overflowing_add(cmp_signal_offset);
                if overflowed {
                    panic!("Signal index is too large");
                }

                let (sig_end, overflowed) =
                    sig_start.overflowing_add(sigs_number as usize);
                if overflowed || sig_end > vm.signals.len() {
                    panic!("Signal index is too large");
                }

                for sig_idx in (sig_start..sig_end).rev() {
                    vm.set_signal(sig_idx);
                }
            }
            OpCode::GetConstant8 => {
                let const_idx = read_usize(code, ip);
                ip += size_of::<usize>();
                vm.push_stack(vm.constants[const_idx]);
            }
            OpCode::Push8 => {
                let val = read_usize(code, ip);
                ip += size_of::<usize>();
                let val = U256::from(val as u64);
                vm.push_stack(val);
            }
            OpCode::Push4 => {
                let val = read_u32(code, ip);
                ip += size_of::<u32>();
                vm.stack_u32.push(val);
            }
            OpCode::GetVariable4 => {
                let var_idx = read_u32(code, ip);
                ip += size_of::<u32>();

                let var_start = var_idx as usize;

                let vars_number = read_u32(code, ip);
                ip += size_of::<u32>();

                let (var_end, overflow) = var_start.overflowing_add(vars_number as usize);
                if overflow {
                    panic!("Variable index is too large");
                }

                let last_frame = vm.call_frames.last().unwrap();
                let vars = match last_frame {
                    Frame::Component { component, .. } => {
                        &mut component.borrow_mut().vars
                    }
                    Frame::Function { vars, .. } => {
                        vars
                    }
                };

                if vars.len() < var_end {
                    panic!("Variable is not set as {}", var_end-1);
                }

                for var in vars.iter().take(var_end).skip(var_start) {
                    if var.is_none() {
                        panic!("Variable not set");
                    }
                    let var = var.unwrap();
                    if var > M {
                        panic!("var = {}", var);
                    }
                    vm.stack.push(var);
                }
            }
            OpCode::GetVariable => {
                let var_idx = vm.stack_u32.pop().unwrap() as usize;

                let vars_number = read_u32(code, ip) as usize;
                ip += size_of::<u32>();

                let last_frame = vm.call_frames.last().unwrap();
                let vars = match last_frame {
                    Frame::Component { component, .. } => {
                        &mut component.borrow_mut().vars
                    }
                    Frame::Function { vars, .. } => {
                        vars
                    }
                };

                if var_idx + vars_number > vars.len() {
                    vm.print_stack_trace();
                    panic!(
                        "Variable not set. Total variables length: {}, var_idx: {}, vars_number: {}",
                        vars.len(), var_idx, vars_number);
                }

                for var in vars.iter().skip(var_idx).take(vars_number) {
                    if var.is_none() {
                        panic!("Variable not set");
                    }
                    let var = var.unwrap();
                    if var > M {
                        panic!("v = {}", var);
                    }
                    vm.stack.push(var);
                }
            }
            OpCode::SetVariable4 => {
                let var_idx = read_u32(code, ip);
                ip += size_of::<u32>();

                let var_start = usize::try_from(var_idx).unwrap();

                let vars_number = read_u32(code, ip);
                ip += size_of::<u32>();

                if vars_number as usize > vm.stack.len() {
                    panic!("Number of variables is greater the stack depth");
                }

                let (var_end, overflow) =
                    var_start.overflowing_add(vars_number as usize);
                if overflow {
                    panic!("Variable index is too large");
                }

                let last_frame = vm.call_frames.last_mut().unwrap();
                let vars: &mut Vec<Option<U256>> = match last_frame {
                    Frame::Component { component, .. } => {
                        &mut component.borrow_mut().vars
                    }
                    Frame::Function { ref mut vars, .. } => {
                        vars
                    }
                };

                if vars.len() < var_end {
                    vars.resize(var_end, None);
                }

                for var_idx in (var_start..var_end).rev() {
                    vars[var_idx] = Some(vm.stack.pop().unwrap());
                }
            }
            OpCode::SetVariable => {
                let var_start = vm.stack_u32.pop().unwrap() as usize;

                let vars_number = read_u32(code, ip);
                ip += size_of::<u32>();

                if vars_number as usize > vm.stack.len() {
                    panic!("Number of variables is greater the stack depth");
                }

                let (var_end, overflow) =
                    var_start.overflowing_add(vars_number as usize);
                if overflow {
                    panic!("Variable index is too large");
                }

                let last_frame = vm.call_frames.last_mut().unwrap();
                let vars: &mut Vec<Option<U256>> = match last_frame {
                    Frame::Component { component, .. } => {
                        &mut component.borrow_mut().vars
                    }
                    Frame::Function { ref mut vars, .. } => {
                        vars
                    }
                };

                if vars.len() < var_end {
                    vars.resize(var_end, None);
                }

                for var_idx in (var_start..var_end).rev() {
                    vars[var_idx] = Some(vm.stack.pop().unwrap());
                }
            }
            OpCode::GetSubSignal => {
                let sigs_number = read_u32(code, ip);
                ip += size_of::<u32>();

                let flags = code[ip];
                ip += 1;
                let is_mapped_signal_idx = flags & 0b1000_0000 != 0;

                let cmp_idx = vm.stack_u32.pop().unwrap();

                let cmp = if let Frame::Component { component, .. } = vm.call_frames.last().unwrap() {
                    component
                } else {
                    panic!("GetSubSignal instruction inside a function");
                };

                let sig_idx = if is_mapped_signal_idx {
                    let indexes_number = read_u32(code, ip);
                    ip += size_of::<u32>();

                    let signal_code = read_u32(code, ip);
                    ip += size_of::<u32>();

                    let indexes_idx = vm.stack_u32.len() - indexes_number as usize;

                    let indexes = vm.stack_u32.split_off(indexes_idx);

                    let subcomponent = get_subcomponent(cmp, cmp_idx)?;
                    let subcmp_template_id = subcomponent.borrow().template_id;

                    calc_mapped_signal_idx(subcmp_template_id, io_map, signal_code, &indexes)?
                } else {
                    vm.stack_u32.pop().unwrap()
                };

                let subcomponent = get_subcomponent(cmp, cmp_idx)?;
                let subcmp_signals_start = subcomponent.borrow().signals_start;

                let (sig_start, overflowed) =
                    subcmp_signals_start.overflowing_add(sig_idx as usize);

                if overflowed {
                    panic!("Subcomponent signal index is too large");
                }

                let (sig_end, overflowed) = sig_start
                    .overflowing_add(sigs_number as usize);

                if overflowed || sig_end > vm.signals.len() {
                    panic!("Subcomponent signal index is too large");
                }

                for sig_idx in sig_start..sig_end {
                    vm.push_stack(vm.signals[sig_idx].expect("Subcomponent signal is not set"));
                }
            }
            OpCode::SetSubSignal => {
                let sigs_number = read_u32(code, ip);
                ip += size_of::<u32>();

                let flags = code[ip];
                ip += 1;

                let (input_status, is_mapped_signal_idx) = unpack_flags(flags);

                let cmp_idx = vm.stack_u32.pop().unwrap();

                let cmp = match vm.call_frames.last().unwrap() {
                    Frame::Component { component, .. } => {
                        component.clone()
                    }
                    Frame::Function { .. } => {
                        panic!("SetSubSignal instruction inside a function");
                    }
                };

                let sig_idx = if is_mapped_signal_idx {
                    let indexes_number = read_u32(code, ip);
                    ip += size_of::<u32>();

                    let signal_code = read_u32(code, ip);
                    ip += size_of::<u32>();

                    let indexes = vm.stack_u32.split_off(indexes_number as usize);

                    let subcomponent = get_subcomponent(&cmp, cmp_idx)?;
                    let subcmp_template_id = subcomponent.borrow().template_id;

                    calc_mapped_signal_idx(subcmp_template_id, io_map, signal_code, &indexes)?
                } else {
                    vm.stack_u32.pop().unwrap()
                };

                if vm.stack.len() < sigs_number as usize {
                    panic!("Number of signals is greater than the stack depth");
                }

                let should_call_cmp = {
                    let subcomponent = get_subcomponent(&cmp, cmp_idx)?;
                    let mut subcmp = subcomponent.borrow_mut();

                    let (sigs_start, overflowed) =
                        subcmp.signals_start.overflowing_add(sig_idx as usize);

                    if overflowed || sigs_start >= vm.signals.len() {
                        panic!("Subcomponent signal start index is too large");
                    }

                    let (sigs_end, overflowed) =
                        sigs_start.overflowing_add(sigs_number as usize);

                    if overflowed || sigs_end > vm.signals.len() {
                        panic!("Subcomponent signal end index is too large");
                    }

                    for sig_idx in (sigs_start..sigs_end).rev() {
                        vm.set_signal(sig_idx);
                    }

                    subcmp.number_of_inputs -= sigs_number as usize;

                    match input_status {
                        InputStatus::Last => true,
                        InputStatus::NoLast => false,
                        InputStatus::Unknown => subcmp.number_of_inputs == 0,
                    }
                };

                if should_call_cmp {
                    match vm.call_frames.last_mut().unwrap() {
                        Frame::Component { ip: ip_local, .. } => {
                            *ip_local = ip;
                        }
                        Frame::Function { .. } => {
                            panic!("Does not expect to call a subcomponent from inside the function");
                        }
                    }

                    let subcomponent = get_subcomponent(&cmp, cmp_idx)?;
                    vm.call_frames.push(
                        Frame::new_component(
                            subcomponent, templates));

                    ip = 0usize;
                    match vm.call_frames.last().unwrap() {
                        Frame::Component { code: code_local, component,  .. } => {
                            code = *code_local;
                            #[cfg(feature = "print_opcode")]
                            {
                                template_id = component.borrow().template_id;
                            }
                            signals_start = component.borrow().signals_start;
                        }
                        Frame::Function { .. } => {
                            panic!("No way, we just added a component frame");
                        }
                    }
                }
            }
            OpCode::JumpIfFalse => {
                let offset = read_i32(code, ip);
                ip += size_of::<i32>();

                let c = vm.stack.pop().unwrap();
                if c.is_zero() {
                    if offset < 0 {
                        ip -= offset.unsigned_abs() as usize;
                    } else {
                        ip += offset as usize;
                    }
                }
            }
            OpCode::Jump => {
                let offset = read_i32(code, ip);
                ip += size_of::<i32>();
                if offset < 0 {
                    ip -= offset.unsigned_abs() as usize;
                } else {
                    ip += offset as usize;
                }
            }
            OpCode::OpMul => {
                let b = vm.stack.pop().unwrap();
                let a = vm.stack.pop().unwrap();
                vm.push_stack(Operation::Mul.eval(a, b));
            }
            OpCode::OpDiv => {
                let b = vm.stack.pop().unwrap();
                let a = vm.stack.pop().unwrap();
                if b.is_zero() {
                    panic!("Division by zero");
                }
                vm.push_stack(Operation::Div.eval(a, b));
            }
            OpCode::OpAdd => {
                let b = vm.stack.pop().unwrap();
                let a = vm.stack.pop().unwrap();
                vm.push_stack(Operation::Add.eval(a, b));
            }
            OpCode::OpSub => {
                let b = vm.stack.pop().unwrap();
                let a = vm.stack.pop().unwrap();
                vm.push_stack(Operation::Sub.eval(a, b));
            }
            OpCode::OpPow => {
                let b = vm.stack.pop().unwrap();
                let a = vm.stack.pop().unwrap();
                vm.push_stack(Operation::Pow.eval(a, b));
            }
            OpCode::OpIntDiv => {
                let b = vm.stack.pop().unwrap();
                let a = vm.stack.pop().unwrap();
                vm.push_stack(Operation::Idiv.eval(a, b));
            }
            OpCode::OpMod => {
                let b = vm.stack.pop().unwrap();
                let a = vm.stack.pop().unwrap();
                vm.push_stack(Operation::Mod.eval(a, b));
            }
            OpCode::OpShL => {
                let b = vm.stack.pop().unwrap();
                let a = vm.stack.pop().unwrap();
                vm.push_stack(Operation::Shl.eval(a, b));
            }
            OpCode::OpShR => {
                let b = vm.stack.pop().unwrap();
                let a = vm.stack.pop().unwrap();
                vm.push_stack(Operation::Shr.eval(a, b));
            }
            OpCode::OpLtE => {
                let b = vm.stack.pop().unwrap();
                let a = vm.stack.pop().unwrap();
                vm.push_stack(Operation::Leq.eval(a, b));
            }
            OpCode::OpGtE => {
                let b = vm.stack.pop().unwrap();
                let a = vm.stack.pop().unwrap();
                vm.push_stack(Operation::Geq.eval(a, b));
            }
            OpCode::OpLt => {
                let b = vm.stack.pop().unwrap();
                let a = vm.stack.pop().unwrap();
                vm.push_stack(Operation::Lt.eval(a, b));
            }
            OpCode::OpGt => {
                let b = vm.stack.pop().unwrap();
                let a = vm.stack.pop().unwrap();
                vm.push_stack(Operation::Gt.eval(a, b));
            }
            OpCode::OpEq => {
                let b = vm.stack.pop().unwrap();
                let a = vm.stack.pop().unwrap();
                vm.push_stack(Operation::Eq.eval(a, b));
            }
            OpCode::OpNe => {
                let b = vm.stack.pop().unwrap();
                let a = vm.stack.pop().unwrap();
                vm.push_stack(Operation::Neq.eval(a, b));
            }
            OpCode::OpBoolOr => {
                let b = vm.stack.pop().unwrap();
                let a = vm.stack.pop().unwrap();
                vm.push_stack(Operation::Lor.eval(a, b));
            }
            OpCode::OpBoolAnd => {
                let b = vm.stack.pop().unwrap();
                let a = vm.stack.pop().unwrap();
                vm.push_stack(Operation::Land.eval(a, b));
            }
            OpCode::OpBoolNot => {
                let a = vm.stack.pop().unwrap();
                vm.push_stack(UnoOperation::Lnot.eval(a));
            }
            OpCode::OpBitOr => {
                let b = vm.stack.pop().unwrap();
                let a = vm.stack.pop().unwrap();
                vm.push_stack(Operation::Bor.eval(a, b));
            }
            OpCode::OpBitAnd => {
                let b = vm.stack.pop().unwrap();
                let a = vm.stack.pop().unwrap();
                vm.push_stack(Operation::Band.eval(a, b));
            }
            OpCode::OpBitXor => {
                let b = vm.stack.pop().unwrap();
                let a = vm.stack.pop().unwrap();
                vm.push_stack(Operation::Bxor.eval(a, b));
            }
            OpCode::OpBitNot => {
                let a = vm.stack.pop().unwrap();
                vm.push_stack(UnoOperation::Bnot.eval(a));
            }
            OpCode::OpNeg => {
                let a = vm.stack.pop().unwrap();
                vm.push_stack(UnoOperation::Neg.eval(a));
            }
            OpCode::GetSelfSignal4 => {
                let sig_idx = read_u32(code, ip);
                ip += size_of::<u32>();

                let sigs_number = read_u32(code, ip);
                ip += size_of::<u32>();

                let (sig_start, overflow) = signals_start
                    .overflowing_add(sig_idx as usize);
                if overflow {
                    panic!("Signal index is too large");
                }

                let (sig_end, overflow) = sig_start
                    .overflowing_add(sigs_number as usize);

                if overflow || sig_end > vm.signals.len() {
                    panic!("Signal index is too large");
                }

                for sig_idx in sig_start..sig_end {
                    vm.push_stack(vm.signals[sig_idx].expect("Signal is not set"));
                }
            }
            OpCode::GetSelfSignal => {
                let cmp_signal_offset = vm.stack_u32.pop().unwrap();
                let (sig_start, overflow) =
                    signals_start.overflowing_add(cmp_signal_offset as usize);
                if overflow {
                    panic!(
                        "First signal index is too large: [{} + {}] = {}",
                        signals_start, cmp_signal_offset, sig_start);
                }

                let sigs_num = read_u32(code, ip);
                ip += size_of::<u32>();

                let (sig_end, overflow) = sig_start
                    .overflowing_add(sigs_num as usize);

                if overflow || sig_end > vm.signals.len() {
                    panic!(
                        "Last signal index is too large: [{} + {}] = {}",
                        sig_start, sigs_num, sig_end);
                }

                for sig_idx in sig_start..sig_end {
                    vm.push_stack(vm.signals[sig_idx].expect("Signal is not set"));
                }
            }
            OpCode::OpToAddr => {
                let f = vm.stack.pop().unwrap();
                let f = TryInto::<u32>::try_into(f)
                    .expect("Value is too large for address");
                vm.stack_u32.push(f);
            }
            OpCode::OpMulAddr => {
                let b = vm.stack_u32.pop().unwrap();
                let a = vm.stack_u32.pop().unwrap();
                let (r, overflow) = a.overflowing_mul(b);
                if overflow {
                    panic!("Address multiplication overflow");
                }
                vm.stack_u32.push(r);
            }
            OpCode::OpAddAddr => {
                let b = vm.stack_u32.pop().unwrap();
                let a = vm.stack_u32.pop().unwrap();
                let (r, overflow) = a.overflowing_add(b);
                if overflow {
                    panic!("Address addition overflow");
                }
                vm.stack_u32.push(r);
            }
            OpCode::CmpCall => {
                let cmp_idx = read_u32(code, ip);
                ip += size_of::<u32>();

                let cmp = match vm.call_frames.last().unwrap() {
                    Frame::Component { component, .. } => {
                        component.clone()
                    }
                    Frame::Function { .. } => {
                        panic!("SetSubSignal instruction inside a function");
                    }
                };

                let subcomponent = get_subcomponent(&cmp, cmp_idx)?;
                match vm.call_frames.last_mut().unwrap() {
                    Frame::Component { ip: ip_local, .. } => {
                        *ip_local = ip;
                    }
                    Frame::Function { .. } => {
                        panic!("Does not expect to call a subcomponent from inside the function");
                    }
                }

                vm.call_frames.push(
                    Frame::new_component(
                        subcomponent, templates));

                ip = 0usize;
                match vm.call_frames.last().unwrap() {
                    Frame::Component { code: code_local, component,  .. } => {
                        code = *code_local;
                        #[cfg(feature = "print_opcode")]
                        {
                            template_id = component.borrow().template_id;
                        }
                        signals_start = component.borrow().signals_start;
                    }
                    Frame::Function { .. } => {
                        panic!("No way, we just added a component frame");
                    }
                }
            }
            OpCode::FnCall => {
                let fn_idx = read_u32(code, ip) as usize;
                ip += size_of::<u32>();

                let args_num = read_u32(code, ip) as usize;
                ip += size_of::<u32>();

                let return_num = read_u32(code, ip) as usize;
                ip += size_of::<u32>();

                match vm.call_frames.last_mut().unwrap() {
                    Frame::Component { ip: ip_local, .. } | Frame::Function { ip: ip_local, .. } => {
                        *ip_local = ip;
                    }
                }

                vm.call_frames.push(Frame::new_function(
                    fn_idx, functions, args_num, return_num));

                ip = 0usize;
                match vm.call_frames.last().unwrap() {
                    Frame::Component { .. } => {
                        panic!("No way, we just added a function frame");
                    }
                    Frame::Function { code: code_local, .. } => {
                        code = *code_local;
                        #[cfg(feature = "print_opcode")]
                        {
                            template_id = usize::MAX;
                        }
                        signals_start = usize::MAX;
                    }
                }

                if let Frame::Function {vars, ..} = vm.call_frames.last_mut().unwrap() {
                    vars.resize(args_num, None);
                    for i in (0..args_num).rev() {
                        vars[i] = Some(vm.stack.pop().unwrap());
                    }
                } else {
                    panic!("No way, we just added a function frame")
                }
            }
            OpCode::FnReturn => {
                let return_num = read_u32(code, ip) as usize;
                // ip += size_of::<u32>(); // no need to increment ip, it would be changed to where we return

                match vm.call_frames.last().unwrap() {
                    Frame::Component { .. } => {
                        panic!("Return instruction inside the component");
                    }
                    Frame::Function { return_num: return_num_local, function, .. } => {
                        assert_eq!(
                            *return_num_local, return_num,
                            "Function {} is supposed to return {} values, but actually returned {}",
                            function.name, *return_num_local, return_num);
                    }
                }

                vm.call_frames.pop();
                if vm.call_frames.is_empty() {
                    panic!("We supposed to exit from the function to the component at least")
                }

                let last_frame = vm.call_frames.last().unwrap();
                match last_frame {
                    Frame::Component { ip: ip_local, component, .. } => {
                        ip = *ip_local;
                        let component = component.borrow();
                        #[cfg(feature = "print_opcode")]
                        {
                            template_id = component.template_id;
                        }
                        code = &templates[component.template_id].code;
                        signals_start = component.signals_start;
                    }
                    Frame::Function { ip: ip_local, code: code_local, .. } => {
                        ip = *ip_local;
                        code = *code_local;
                    }
                }
            }
            OpCode::Assert => {
                panic!("Assert instruction");
            }
        }

        #[cfg(feature = "print_opcode")]
        {
            println!("==[ Stack ]==");
            vm.print_stack();
            println!("==[ Stack32 ]==");
            vm.print_stack_u32();
        }
    }

    Ok(())
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
