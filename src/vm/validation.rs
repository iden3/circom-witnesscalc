use crate::storage::TemplateInstanceIOMap;

use super::{read_i32, read_u32, read_usize, Function, InputStatus, OpCode, Template};

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
