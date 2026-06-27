use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;

use ruint::aliases::U256;

use crate::field::M;
use crate::graph::{Operation, UnoOperation};
use crate::storage::TemplateInstanceIOMap;

#[cfg(feature = "print_opcode")]
use super::disassemble_instruction;
use super::{
    read_i32, read_instruction, read_u32, read_usize, unpack_flags, Component, Function,
    InputStatus, OpCode, Template,
};

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
