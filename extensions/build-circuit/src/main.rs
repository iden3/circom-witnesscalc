use compiler::circuit_design::template::TemplateCode;
use compiler::compiler_interface::{run_compiler, Circuit, Config, VCP};
use compiler::intermediate_representation::ir_interface::{AddressType, ComputeBucket, CreateCmpBucket, FinalData, InputInformation, Instruction, InstructionPointer, LoadBucket, LocationRule, ObtainMeta, OperatorType, ReturnBucket, ReturnType, StatusInput, StoreBucket, ValueBucket, ValueType};
use constraint_generation::{build_circuit, BuildConfig};
use program_structure::error_definition::Report;
use std::collections::HashMap;
use std::{env, fmt, fs};
use std::error::Error;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Instant;
use anyhow::anyhow;
use code_producers::c_elements::IODef;
use code_producers::components::TemplateInstanceIOMap;
use compiler::circuit_design::function::FunctionCode;
use indicatif::ProgressBar;
use ruint::aliases::U256;
use type_analysis::check_types::check_types;
use circom_witnesscalc::{progress_bar, InputSignalsInfo};
use circom_witnesscalc::field::{Field, FieldOperations, FieldOps, U254, U64};
use circom_witnesscalc::graph::{Node, Operation, UnoOperation, TresOperation, Nodes, NodeConstErr, NodeIdx, NodesInterface, optimize, MMapNodes, NodesStorage};
use circom_witnesscalc::storage::serialize_witnesscalc_graph;

fn try_into_operation(op: OperatorType) -> Result<Operation, String> {
    match op {
        OperatorType::Mul => Ok(Operation::Mul),
        OperatorType::Div => Ok(Operation::Div),
        OperatorType::Add => Ok(Operation::Add),
        OperatorType::Sub => Ok(Operation::Sub),
        OperatorType::Pow => Ok(Operation::Pow),
        OperatorType::IntDiv => Ok(Operation::Idiv),
        OperatorType::Mod => Ok(Operation::Mod),
        OperatorType::ShiftL => Ok(Operation::Shl),
        OperatorType::ShiftR => Ok(Operation::Shr),
        OperatorType::LesserEq => Ok(Operation::Leq),
        OperatorType::GreaterEq => Ok(Operation::Geq),
        OperatorType::Lesser => Ok(Operation::Lt),
        OperatorType::Greater => Ok(Operation::Gt),
        OperatorType::Eq(1) => Ok(Operation::Eq),
        OperatorType::Eq(_) => todo!(),
        OperatorType::NotEq => Ok(Operation::Neq),
        OperatorType::BoolOr => Ok(Operation::Lor),
        OperatorType::BoolAnd => Ok(Operation::Land),
        OperatorType::BitOr => Ok(Operation::Bor),
        OperatorType::BitAnd => Ok(Operation::Band),
        OperatorType::BitXor => Ok(Operation::Bxor),
        OperatorType::PrefixSub => Err("Not a binary operation".to_string()),
        OperatorType::BoolNot => Err("Not a binary operation".to_string()),
        OperatorType::Complement => Err("Not a binary operation".to_string()),
        OperatorType::ToAddress => Err("Not a binary operation".to_string()),
        OperatorType::MulAddress => Ok(Operation::Mul),
        OperatorType::AddAddress => Ok(Operation::Add),
    }
}

fn try_into_uno_operation(op: OperatorType) -> Result<UnoOperation, String> {
    let err = Err("Not an unary operation".to_string());
    match op {
        OperatorType::Mul => err,
        OperatorType::Div => err,
        OperatorType::Add => err,
        OperatorType::Sub => err,
        OperatorType::Pow => err,
        OperatorType::IntDiv => err,
        OperatorType::Mod => err,
        OperatorType::ShiftL => err,
        OperatorType::ShiftR => err,
        OperatorType::LesserEq => err,
        OperatorType::GreaterEq => err,
        OperatorType::Lesser => err,
        OperatorType::Greater => err,
        OperatorType::Eq(1) => err,
        OperatorType::Eq(_) => err,
        OperatorType::NotEq => err,
        OperatorType::BoolOr => err,
        OperatorType::BoolAnd => err,
        OperatorType::BitOr => err,
        OperatorType::BitAnd => err,
        OperatorType::BitXor => err,
        OperatorType::PrefixSub => Ok(UnoOperation::Neg),
        OperatorType::BoolNot => Ok(UnoOperation::Lnot),
        OperatorType::Complement => Ok(UnoOperation::Bnot),
        OperatorType::ToAddress => Ok(UnoOperation::Id),
        OperatorType::MulAddress => err,
        OperatorType::AddAddress => err,
    }
}

// if instruction pointer is a store to the signal, return the signal index
// and the src instruction to store to the signal
fn try_signal_store<'a, T: FieldOps + 'static, NS: NodesStorage + 'static>(
    ctx: &mut BuildCircuitContext<T, NS>,
    cmp: &mut ComponentInstance<T>,
    inst: &'a InstructionPointer,
    print_debug: bool,
    call_stack: &Vec<String>,
) -> Option<(usize, &'a InstructionPointer)> {
    let store_bucket = match **inst {
        Instruction::Store(ref store_bucket) => store_bucket,
        _ => return None,
    };
    if let AddressType::Signal = store_bucket.dest_address_type {} else { return None; };
    match &store_bucket.dest {
        LocationRule::Indexed {
            location,
            template_header,
        } => {
            if template_header.is_some() {
                panic!("not implemented: template_header expected to be None");
            }
            let signal_idx =
                calc_expression(ctx, location, cmp, print_debug, call_stack)
                    .must_const_usize(ctx.nodes, call_stack);

            let signal_idx = cmp.signal_offset + signal_idx;
            Some((signal_idx, &store_bucket.src))
        }
        LocationRule::Mapped { .. } => {
            todo!()
        }
    }
}

fn var_from_value_instruction_n<T, NS>(
    value_bucket: &ValueBucket, nodes: &Nodes<T, NS>, n: usize,
    call_stack: &[String]) -> Vec<Var<T>>
where
    T: FieldOps + 'static,
    NS: NodesStorage + 'static {

    match value_bucket.parse_as {
        ValueType::BigInt => {
            let mut result = Vec::with_capacity(n);

            for i in 0..n {
                assert!(
                    matches!(
                        nodes.get(NodeIdx(value_bucket.value+i)),
                        Some(Node::Constant(..))),
                    "node #{} expected to be a constant, but it is not; {}",
                    value_bucket.value+i, call_stack.join(" -> "));
                result.push(Var::Node(value_bucket.value+i));
            };

            result
        },
        ValueType::U32 => {
            assert_eq!(n, 1,
                       "for ValueType::U32 number of values is expected to be 1; {}",
                       call_stack.join(" -> "));
            let v = (&nodes.ff).parse_usize(value_bucket.value).unwrap();
            vec![Var::Value(v)]
        },
    }
}

fn operator_argument_instruction_n<T, NS>(
    ctx: &mut BuildCircuitContext<T, NS>, inst: &InstructionPointer,
    cmp: &mut ComponentInstance<T>, size: usize, print_debug: bool,
    call_stack: &Vec<String>) -> Vec<usize>
where
    T: FieldOps + 'static,
    NS: NodesStorage + 'static {

    assert!(size > 0, "size = {}", size);

    if size == 1 {
        // operator_argument_instruction implements much more cases than
        // this function, so we can use it here is size == 1
        return vec![operator_argument_instruction(
            ctx, cmp, inst, print_debug, call_stack)];
    }

    match **inst {
        Instruction::Load(ref load_bucket) => {
            match load_bucket.address_type {
                AddressType::Signal => match &load_bucket.src {
                    LocationRule::Indexed {
                        location,
                        template_header,
                    } => {
                        if template_header.is_some() {
                            panic!("not implemented: template_header expected to be None");
                        }
                        let signal_idx =
                            calc_expression(
                                ctx, location, cmp, print_debug, call_stack)
                                .must_const_usize(ctx.nodes, call_stack);
                        let mut result = Vec::with_capacity(size);
                        for i in 0..size {
                            let signal_node = ctx
                                .signal_node_idx[cmp.signal_offset + signal_idx + i];
                            assert_ne!(
                                signal_node, usize::MAX,
                                "signal {}/{}/{} is not set yet",
                                cmp.signal_offset, signal_idx, i);
                            result.push(signal_node);
                        }
                        result
                    }
                    LocationRule::Mapped { .. } => {
                        todo!()
                    }
                },
                AddressType::SubcmpSignal { ref cmp_address, .. } => {
                    let subcomponent_idx =
                        calc_expression(
                            ctx, cmp_address, cmp, print_debug, call_stack)
                            .must_const_usize(ctx.nodes, call_stack);

                    let (signal_idx, template_header) = match load_bucket.src {
                        LocationRule::Indexed {
                            ref location,
                            ref template_header,
                        } => {
                            let signal_idx = calc_expression(
                                ctx, location, cmp, print_debug, call_stack);
                            let signal_idx = signal_idx.to_const_usize(ctx.nodes)
                                .unwrap_or_else(|e| panic!(
                                    "can't calculate const usize signal index: {}: {}",
                                    e, call_stack.join(" -> ")));
                            (signal_idx,
                             template_header.as_ref().unwrap_or(&"-".to_string()).clone())
                        }
                        LocationRule::Mapped { ref signal_code, ref indexes } => {
                            calc_mapped_signal_idx(
                                ctx, cmp, subcomponent_idx, *signal_code,
                                indexes, print_debug, call_stack)
                        }
                    };
                    let signal_offset = cmp.subcomponents[subcomponent_idx]
                        .as_ref()
                        .unwrap()
                        .signal_offset;

                    if print_debug {
                        let location_rule = match load_bucket.src {
                            LocationRule::Indexed { .. } => "Indexed",
                            LocationRule::Mapped { .. } => "Mapped",
                        };
                        println!(
                            "Load subcomponent signal (location: {}, template: {}, subcomponent idx: {}, size: {}): {} + {} = {}",
                            location_rule, template_header, subcomponent_idx, size,
                            signal_offset, signal_idx, signal_offset + signal_idx);
                    }

                    let signal_idx = signal_offset + signal_idx;

                    let mut result = Vec::with_capacity(size);
                    for i in 0..size {
                        let signal_node = ctx.signal_node_idx[signal_idx + i];
                        assert_ne!(
                            signal_node, usize::MAX,
                            "signal {}/{}/{} is not set yet",
                            cmp.signal_offset, signal_idx, i);
                        result.push(signal_node);
                    }
                    result
                }
                AddressType::Variable => {
                    let location = match load_bucket.src {
                        LocationRule::Indexed { ref location, .. } => location,
                        LocationRule::Mapped { .. } => {
                            panic!("mapped signals are supported on for subcmp signals");
                        }
                    };
                    let var_idx =
                        calc_expression(
                            ctx, location, cmp, print_debug, call_stack)
                            .must_const_usize(ctx.nodes, call_stack);
                    let mut result = Vec::with_capacity(size);
                    for i in 0..size {
                        match cmp.vars[var_idx+i] {
                            Some(Var::Node(idx)) => {
                                result.push(idx);
                            },
                            Some(Var::Value(ref v)) => {
                                result.push(
                                    ctx.nodes.const_node_idx_from_value(*v));
                            }
                            None => { panic!("variable is not set: {}, {:?}",
                                             load_bucket.line, call_stack); }
                        };
                    }
                    result
                }
            }
        }
        _ => {
            panic!("multi-operator is not implemented for instruction: {}", inst.to_string());
        }
    }
}


fn operator_argument_instruction<T, NS>(
    ctx: &mut BuildCircuitContext<T, NS>, cmp: &mut ComponentInstance<T>,
    inst: &InstructionPointer, print_debug: bool,
    call_stack: &Vec<String>) -> usize
where
    T: FieldOps + 'static,
    NS: NodesStorage + 'static {

    match **inst {
        Instruction::Load(ref load_bucket) => {
            match load_bucket.address_type {
                AddressType::Signal => match &load_bucket.src {
                    LocationRule::Indexed {
                        location,
                        template_header,
                    } => {
                        if template_header.is_some() {
                            panic!("not implemented: template_header expected to be None");
                        }
                        let signal_idx =
                            calc_expression(
                                ctx, location, cmp, print_debug, call_stack)
                                .must_const_usize(ctx.nodes, call_stack);
                        let signal_idx = cmp.signal_offset + signal_idx;
                        let signal_node = ctx.signal_node_idx[signal_idx];
                        assert_ne!(signal_node, usize::MAX, "signal is not set yet");
                        signal_node
                    }
                    LocationRule::Mapped { .. } => {
                        todo!()
                    }
                },
                AddressType::SubcmpSignal {
                    ref cmp_address, ..
                } => {
                    let subcomponent_idx =
                        calc_expression(
                            ctx, cmp_address, cmp, print_debug, call_stack)
                            .must_const_usize(ctx.nodes, call_stack);

                    let (signal_idx, template_header) = match load_bucket.src {
                        LocationRule::Indexed {
                            ref location,
                            ref template_header,
                        } => {
                            let signal_idx =
                                calc_expression(
                                    ctx, location, cmp, print_debug, call_stack)
                                    .must_const_usize(ctx.nodes, call_stack);
                            (signal_idx,
                             template_header.as_ref().unwrap_or(&"-".to_string()).clone())
                        }
                        LocationRule::Mapped { ref signal_code, ref indexes } => {
                            calc_mapped_signal_idx(
                                ctx, cmp, subcomponent_idx, *signal_code,
                                indexes, print_debug, call_stack)
                        }
                    };

                    let signal_offset = cmp.subcomponents[subcomponent_idx]
                        .as_ref().unwrap().signal_offset;

                    if print_debug {
                        println!(
                            "Load subcomponent signal: ({}) [{}] {} + {} = {}",
                            template_header, subcomponent_idx, signal_offset,
                            signal_idx, signal_offset + signal_idx);
                    }

                    let signal_idx = signal_offset + signal_idx;
                    let signal_node = ctx.signal_node_idx[signal_idx];
                    assert_ne!(signal_node, usize::MAX, "signal is not set yet");
                    signal_node
                }
                AddressType::Variable => {
                    match load_bucket.src {
                        LocationRule::Indexed { ref location, .. } => {
                            let var_idx =
                                calc_expression(
                                    ctx, location, cmp, print_debug, call_stack)
                                    .must_const_usize(ctx.nodes, call_stack);
                            match cmp.vars[var_idx] {
                                Some(Var::Node(idx)) => idx,
                                Some(Var::Value(ref v)) => {
                                    ctx.nodes.const_node_idx_from_value(*v)
                                }
                                None => { panic!("variable is not set"); }
                            }
                        }
                        LocationRule::Mapped { .. } => {
                            todo!()
                        }
                    }
                }
            }
        }
        Instruction::Compute(ref compute_bucket) => {
            let node = node_from_compute_bucket(
                ctx, cmp, compute_bucket, print_debug, call_stack);
            ctx.nodes.push(node).0
        }
        Instruction::Value(ref value_bucket) => {
            match value_bucket.parse_as {
                ValueType::BigInt => match ctx.nodes.get(NodeIdx(value_bucket.value)) {
                    Some(Node::Constant(..)) => {
                        value_bucket.value
                    }
                    _ => {
                        panic!("there is expected to be constant node");
                    }
                },
                ValueType::U32 => {
                    // in case it is a valid case, maybe we can make a
                    // constant, add it to nodes and return its index
                    panic!("not implemented");
                }
            }
        }
        _ => {
            panic!("not implemented for instruction: {}", inst.to_string());
        }
    }
}

fn node_from_compute_bucket<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    ctx: &mut BuildCircuitContext<T, NS>,
    cmp: &mut ComponentInstance<T>,
    compute_bucket: &ComputeBucket,
    print_debug: bool,
    call_stack: &Vec<String>,
) -> Node {
    if let Ok(op) = try_into_operation(compute_bucket.op) {
        let arg1 = operator_argument_instruction(
            ctx, cmp, &compute_bucket.stack[0], print_debug, call_stack);
        let arg2 = operator_argument_instruction(
            ctx, cmp, &compute_bucket.stack[1], print_debug, call_stack);
        Node::Op(op, arg1, arg2)
    } else if let Ok(op) = try_into_uno_operation(compute_bucket.op) {
        let arg1 = operator_argument_instruction(
            ctx, cmp, &compute_bucket.stack[0], print_debug, call_stack);
        Node::UnoOp(op, arg1)
    } else {
        panic!(
            "not implemented: this operator is not supported to be converted to Node: {}",
            compute_bucket.to_string());
    }
}

fn calc_mapped_signal_idx<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    ctx: &mut BuildCircuitContext<T, NS>, cmp: &mut ComponentInstance<T>,
    subcomponent_idx: usize, signal_code: usize,
    indexes: &[InstructionPointer], print_debug: bool,
    call_stack: &Vec<String>) -> (usize, String) {

    let template_id = &cmp.subcomponents[subcomponent_idx]
        .as_ref()
        .unwrap()
        .template_id;
    let signals = ctx.io_map.get(template_id).unwrap();
    let template_def = format!("<template id: {}>", template_id);
    let def: &IODef = &signals[signal_code];
    let mut map_access = def.offset;

    if !indexes.is_empty() {
        let lengths = &def.lengths;
        // I'm not sure if this assert should be here.
        assert_eq!(
            lengths.len(),
            indexes.len(),
            "Number of indexes does not match the number of dimensions"
        );

        // Compute strides
        let mut strides = vec![1usize; lengths.len()];
        for i in (0..lengths.len() - 1).rev() {
            strides[i] = strides[i + 1] * lengths[i + 1];
        }

        // Calculate linear index
        for (i, idx_ip) in indexes.iter().enumerate() {
            let idx_value = calc_expression(
                ctx, idx_ip, cmp, print_debug, call_stack);
            let idx_value = idx_value.must_const_usize(
                ctx.nodes, call_stack);

            // Ensure index is within bounds
            assert!(
                idx_value < lengths[i],
                "Index out of bounds: index {} >= dimension size {}",
                idx_value,
                lengths[i]
            );

            map_access += idx_value * strides[i];
        }
    }

    (map_access, template_def)
}

struct CallStack {
    stack: Vec<(String, usize)>,
    stack_fmt: String,
}


impl CallStack {
    fn new() -> Self {
        CallStack {
            stack: Vec::new(),
            stack_fmt: String::new(),
        }
    }

    fn update_line(&mut self, line_no: usize) {
        if let Some((_, ln)) = self.stack.last_mut() {
            *ln = line_no;
        }
        self.update_fmt();
    }

    fn update_fmt(&mut self) {
        self.stack_fmt = self.stack.iter()
            .map(|(name, line_no)| format!("{}:{}", name, line_no))
            .collect::<Vec<String>>()
            .join(" -> ");
    }

    fn push(&mut self, name: String) {
        self.stack.push((name, 0));
    }

    fn pop(&mut self) {
        self.stack.pop();
        self.update_fmt();
    }
}

struct BuildCircuitContext<'a, T: FieldOps, NS: NodesStorage> {
    nodes: &'a mut Nodes<T, NS>,
    signal_node_idx: &'a mut Vec<usize>,
    signals_set: usize,
    progress_bar: ProgressBar,
    templates: &'a Vec<TemplateCode>,
    functions: &'a Vec<FunctionCode>,
    io_map: &'a TemplateInstanceIOMap,
    stack: CallStack,
}

impl<T, NS> BuildCircuitContext<'_, T, NS>
where
    T: FieldOps + 'static,
    NS: NodesStorage + 'static {

    fn new<'a>(
        nodes: &'a mut Nodes<T, NS>, signal_node_idx: &'a mut Vec<usize>,
        templates: &'a Vec<TemplateCode>, functions: &'a Vec<FunctionCode>,
        io_map: &'a TemplateInstanceIOMap) -> BuildCircuitContext<'a, T, NS> {

        let signal_nodes_num = signal_node_idx.len();

        BuildCircuitContext {
            nodes,
            signal_node_idx,
            signals_set: 0,
            progress_bar: progress_bar(signal_nodes_num),
            templates,
            functions,
            io_map,
            stack: CallStack::new(),
        }
    }
    fn new_component(
        &self, template_id: usize, signal_offset: usize,
        component_offset: usize) -> ComponentInstance<T> {

        let tmpl = &self.templates[template_id];
        let mut cmp = ComponentInstance {
            template_id,
            signal_offset,
            number_of_inputs: tmpl.number_of_inputs,
            component_offset,
            vars: vec![None; tmpl.var_stack_depth],
            subcomponents: Vec::with_capacity(tmpl.number_of_components),
        };
        cmp.subcomponents.resize_with(tmpl.number_of_components, || None);
        cmp
    }

    fn associate_signal_to_node(&mut self, signal_idx: usize, node_idx: usize) {
        if self.signal_node_idx[signal_idx] != usize::MAX {
            panic!("signal is already set");
        }
        self.signal_node_idx[signal_idx] = node_idx;
        self.signals_set += 1;
        self.update_progress_message();
        self.progress_bar.inc(1);
    }

    fn update_progress_message(&mut self) {
        let nodes_num: u64 = self.nodes.len().try_into().unwrap();
        let const_num: u64 = self.nodes.constants.len().try_into().unwrap();
        let msg = format!(
            "nodes: {}; constants: {}\n{}",
            indicatif::HumanCount(nodes_num), indicatif::HumanCount(const_num),
            self.stack.stack_fmt);
        self.progress_bar.set_message(msg);
    }

    fn push_stack(&mut self, name: String) {
        self.stack.push(name);
    }

    fn update_line(&mut self, line_no: usize) {
        self.stack.update_line(line_no);
        self.update_progress_message();
    }

    fn pop_stack(&mut self) {
        self.stack.pop();
        self.update_progress_message();
    }
}

fn process_instruction<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    ctx: &mut BuildCircuitContext<T, NS>, inst: &InstructionPointer,
    print_debug: bool, call_stack: &Vec<String>,
    cmp: &mut ComponentInstance<T>) {

    ctx.update_line(inst.get_line());

    match **inst {
        Instruction::Value(..) => {
            panic!("not implemented");
        }
        Instruction::Load(..) => {
            panic!("not implemented");
        }
        Instruction::Store(ref store_bucket) => {
            match store_bucket.dest_address_type {
                AddressType::Signal => {
                    match &store_bucket.dest {
                        LocationRule::Indexed {
                            location,
                            template_header,
                        } => {
                            if template_header.is_some() {
                                panic!("not implemented: template_header expected to be None");
                            }
                            let signal_idx =
                                calc_expression(
                                    ctx, location, cmp, print_debug, call_stack)
                                    .must_const_usize(ctx.nodes, call_stack);

                            if print_debug {
                                println!(
                                    "Store signal at offset {} + {} = {}",
                                    cmp.signal_offset, signal_idx,
                                    cmp.signal_offset + signal_idx);
                            }
                            let signal_idx =
                                cmp.signal_offset + signal_idx;

                            let node_idxs = operator_argument_instruction_n(
                                ctx, &store_bucket.src, cmp,
                                store_bucket.context.size, print_debug,
                                call_stack);

                            assert_eq!(node_idxs.len(), store_bucket.context.size);

                            for (i, item) in node_idxs.iter()
                                .enumerate().take(store_bucket.context.size) {

                                ctx.associate_signal_to_node(
                                    signal_idx + i, *item);
                            }
                        }
                        // LocationRule::Mapped { signal_code, indexes } => {}
                        _ => {
                            panic!(
                                "not implemented: store destination support only Indexed type: {}",
                                store_bucket.dest.to_string()
                            );
                        }
                    }
                }
                AddressType::Variable => {
                    match &store_bucket.dest {
                        LocationRule::Indexed {
                            location,
                            template_header,
                        } => {
                            if template_header.is_some() {
                                panic!("not implemented: template_header expected to be None");
                            }
                            let lvar_idx =
                                calc_expression(
                                    ctx, location, cmp, print_debug, call_stack
                                )
                                    .must_const_usize(ctx.nodes, call_stack);
                            let var_exprs = calc_expression_n(
                                ctx, &store_bucket.src, cmp,
                                store_bucket.context.size, print_debug,
                                call_stack);

                            for (i, item) in var_exprs.iter()
                                .enumerate().take(store_bucket.context.size) {

                                cmp.vars[lvar_idx + i] = Some(item.clone());
                            }
                        }
                        LocationRule::Mapped {..} => {
                            panic!("mapped location is not supported for AddressType::Variable");
                        }
                    }
                }
                AddressType::SubcmpSignal {
                    ref cmp_address,
                    ref input_information,
                    ..
                } => {
                    let node_idxs = operator_argument_instruction_n(
                        ctx, &store_bucket.src, cmp, store_bucket.context.size,
                        print_debug, call_stack);
                    assert_eq!(node_idxs.len(), store_bucket.context.size);

                    let dest = SignalDestination {
                        cmp_address,
                        input_information,
                        dest: &store_bucket.dest,
                    };

                    store_subcomponent_signals(
                        ctx, &node_idxs, print_debug, call_stack, cmp, &dest);
                }
            };
        }
        Instruction::Compute(_) => {
            panic!("not implemented");
        }
        Instruction::Call(ref call_bucket) => {
            let mut fn_vars: Vec<Option<Var<T>>> = vec![None; call_bucket.arena_size];

            let mut count: usize = 0;
            for (idx, inst2) in call_bucket.arguments.iter().enumerate() {
                let args = calc_expression_n(
                    ctx, inst2, cmp, call_bucket.argument_types[idx].size,
                    print_debug, call_stack);
                for arg in args {
                    fn_vars[count] = Some(arg);
                    count += 1;
                }
            }

            let r = run_function(
                ctx, &call_bucket.symbol, &mut fn_vars, print_debug,
                call_stack);

            match call_bucket.return_info {
                ReturnType::Intermediate{ ..} => { todo!(); }
                ReturnType::Final( ref final_data ) => {
                    if let FnReturn::FnVar {ln, ..} = r {
                        assert!(final_data.context.size >= ln);
                    }
                    // assert_eq!(final_data.context.size, r.ln);
                    store_function_return_results(
                        ctx, final_data, &fn_vars, &r, print_debug, call_stack,
                        cmp);
                }
            }
        }
        Instruction::Branch(ref branch_bucket) => {
            let cond = calc_expression(
                ctx, &branch_bucket.cond, cmp, print_debug, call_stack);
            match cond.to_const(ctx.nodes) {
                Ok(cond_val) => {
                    let inst_list = if cond_val.is_zero() {
                        &branch_bucket.else_branch
                    } else {
                        &branch_bucket.if_branch
                    };
                    for inst in inst_list {
                        process_instruction(
                            ctx, inst, print_debug, call_stack, cmp);
                    }
                }
                Err(NodeConstErr::InputSignal) => {
                    // If we have this error, then condition is a node
                    // that depends on input signal
                    let node_idx = if let Var::Node(node_idx) = cond {
                        node_idx
                    } else {
                        panic!("[assertion] expected to have a node with ternary operation here");
                    };

                    if branch_bucket.if_branch.len() != 1 || branch_bucket.else_branch.len() != 1 {
                        panic!("Non-constant condition may be used only in ternary operation and both branches of code must be of length 1");
                    }
                    let if_branch = try_signal_store(
                        ctx, cmp, &branch_bucket.if_branch[0], print_debug,
                        call_stack);
                    let else_branch = try_signal_store(
                        ctx, cmp, &branch_bucket.else_branch[0],
                        print_debug, call_stack);
                    match (if_branch, else_branch) {
                        (Some((if_signal_idx, if_src)), Some((else_signal_idx, else_src))) => {
                            if if_signal_idx != else_signal_idx {
                                panic!("if and else branches must store to the same signal");
                            }

                            assert_eq!(
                                ctx.signal_node_idx[if_signal_idx],
                                usize::MAX,
                                "signal already set"
                            );

                            let node_idx_if = operator_argument_instruction(
                                ctx, cmp, if_src, print_debug, call_stack);

                            let node_idx_else = operator_argument_instruction(
                                ctx, cmp, else_src, print_debug, call_stack);

                            let node = Node::TresOp(TresOperation::TernCond, node_idx, node_idx_if, node_idx_else);
                            let node_idx = ctx.nodes.push(node).0;
                            ctx.associate_signal_to_node(
                                if_signal_idx, node_idx);
                        }
                        _ => {
                            panic!(
                                "if branch or else branch is not a store to the signal, which is the only option for ternary operation\n{}\nIF: {}\nELSE: {}",
                                call_stack.join(" -> "),
                                branch_bucket.if_branch[0].to_string(),
                                branch_bucket.else_branch[0].to_string());
                        }
                    }
                }
                Err(e) => {
                    panic!(
                        "unexpected condition error: {}: {}",
                        e, call_stack.join(" -> "));
                }
            }
        }
        Instruction::Return(_) => {
            panic!("not implemented");
        }
        Instruction::Assert(_) => {
            // asserts are not supported in witness graph
            // panic!("not implemented");
        }
        Instruction::Log(_) => {
            // we are unable to log anything in witness graph
            // panic!("not implemented");
        }
        Instruction::Loop(ref loop_bucket) => {
            while check_continue_condition(
                ctx, cmp, &loop_bucket.continue_condition, print_debug,
                call_stack) {

                for i in &loop_bucket.body {
                    process_instruction(
                        ctx, i, print_debug, call_stack, cmp);
                }
            }
        }
        Instruction::CreateCmp(ref create_component_bucket) => {
            let sub_cmp_idx =
                calc_expression(
                    ctx, &create_component_bucket.sub_cmp_id, cmp, print_debug,
                    call_stack)
                    .must_const_usize(ctx.nodes, call_stack);

            assert!(
                sub_cmp_idx + create_component_bucket.number_of_cmp - 1
                    < cmp.subcomponents.len(),
                "cmp_idx = {}, number_of_cmp = {}, subcomponents.len() = {}",
                sub_cmp_idx,
                create_component_bucket.number_of_cmp,
                cmp.subcomponents.len()
            );

            let mut cmp_signal_offset = create_component_bucket.signal_offset;
            let mut cmp_offset = create_component_bucket.component_offset;

            for (i, _ ) in create_component_bucket.defined_positions.iter() {
                let i = sub_cmp_idx + *i;

                if cmp.subcomponents[i].is_some() {
                    panic!("subcomponent already set");
                }
                cmp.subcomponents[i] = Some(ctx.new_component(
                    create_component_bucket.template_id,
                    cmp.signal_offset + cmp_signal_offset,
                    cmp.component_offset + cmp_offset + 1,
                ));
                cmp_offset += create_component_bucket.component_offset_jump;
                cmp_signal_offset += create_component_bucket.signal_offset_jump;
            }

            if print_debug {
                println!(
                    "{}",
                    fmt_create_cmp_bucket(
                        ctx, create_component_bucket, cmp, print_debug,
                        call_stack));
            }
            if !create_component_bucket.has_inputs {
                for i in sub_cmp_idx..sub_cmp_idx + create_component_bucket.number_of_cmp {
                    let cmp = cmp.subcomponents[i].as_mut().unwrap();
                    run_template(ctx, print_debug, call_stack, cmp)
                }
            }
        }
    }
}

fn store_function_return_results_into_variable<T, NS>(
    final_data: &FinalData, src_vars: &[Option<Var<T>>], ret: &FnReturn<T>,
    dst_vars: &mut Vec<Option<Var<T>>>, nodes: &mut Nodes<T, NS>,
    call_stack: &Vec<String>)
where
    T: FieldOps + 'static,
    NS: NodesStorage + 'static {

    assert!(matches!(final_data.dest_address_type, AddressType::Variable));

    match &final_data.dest {
        LocationRule::Indexed {
            location,
            template_header,
        } => {
            if template_header.is_some() {
                panic!("not implemented: template_header expected to be None");
            }
            let lvar_idx =
                calc_function_expression(location, dst_vars, nodes, call_stack)
                    .must_const_usize(nodes, call_stack);

            match ret {
                FnReturn::FnVar { idx, .. } => {
                    for i in 0..final_data.context.size {
                        let v = if let Some(v) = &src_vars[idx + i] {
                            v
                        } else {
                            panic!("return value is not set {} / {}", idx, i)
                        };
                        dst_vars[lvar_idx + i] = Some(v.clone());
                    }

                }
                FnReturn::Value(v) => {
                    assert_eq!(final_data.context.size, 1);
                    dst_vars[lvar_idx] = Some(v.clone());
                }
            }
        }
        LocationRule::Mapped { .. } => { todo!() }
    }
}

fn store_function_return_results_into_subsignal<T, NS>(
    ctx: &mut BuildCircuitContext<T, NS>, final_data: &FinalData,
    src_vars: &[Option<Var<T>>], ret: &FnReturn<T>, print_debug: bool,
    call_stack: &Vec<String>, cmp: &mut ComponentInstance<T>)
where
    T: FieldOps + 'static,
    NS: NodesStorage + 'static {

    let (cmp_address, input_information) = if let AddressType::SubcmpSignal {cmp_address, input_information, ..} = &final_data.dest_address_type {
        (cmp_address, input_information)
    } else {
        panic!("expected SubcmpSignal destination address type");
    };

    let mut src_node_idxs: Vec<usize> = Vec::new();
    match ret {
        FnReturn::FnVar { idx, .. } => {
            for i in 0..final_data.context.size {
                match src_vars[idx+i] {
                    Some(Var::Node(node_idx)) => {
                        src_node_idxs.push(node_idx);
                    }
                    Some(Var::Value(v)) => {
                        src_node_idxs.push(
                            ctx.nodes.const_node_idx_from_value(v));
                    }
                    None => {
                        panic!("variable at index {} is not set", i);
                    }
                }
            }

        }
        FnReturn::Value(v) => {
            assert_eq!(final_data.context.size, 1);
            match v {
                Var::Node(node_idx) => {
                    src_node_idxs.push(*node_idx);
                }
                Var::Value(v) => {
                    src_node_idxs.push(ctx.nodes.const_node_idx_from_value(*v));
                }
            }
        }
    }

    let dest = SignalDestination {
        cmp_address,
        input_information,
        dest: &final_data.dest,
    };

    store_subcomponent_signals(
        ctx, &src_node_idxs, print_debug, call_stack, cmp, &dest);
}

fn store_function_return_results<T, NS> (
    ctx: &mut BuildCircuitContext<T, NS>, final_data: &FinalData,
    src_vars: &[Option<Var<T>>], ret: &FnReturn<T>, print_debug: bool,
    call_stack: &Vec<String>, cmp: &mut ComponentInstance<T>)
where
    T: FieldOps + 'static,
    NS: NodesStorage + 'static {

    match &final_data.dest_address_type {
        AddressType::Signal => todo!("Signal"),
        AddressType::Variable => {
            store_function_return_results_into_variable(
                final_data, src_vars, ret, &mut cmp.vars, ctx.nodes,
                call_stack);
        }
        AddressType::SubcmpSignal {..} => {
            store_function_return_results_into_subsignal(
                ctx, final_data, src_vars, ret, print_debug, call_stack, cmp);
        }
    }
}

fn run_function<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    ctx: &mut BuildCircuitContext<T, NS>, fn_name: &str,
    fn_vars: &mut Vec<Option<Var<T>>>, print_debug: bool,
    call_stack: &[String]) -> FnReturn<T> {

    // for i in functions {
    //     println!("Function: {} {}", i.header, i.name);
    // }

    let f = find_function(fn_name, ctx.functions);
    let mut start: Option<Instant> = None;
    if print_debug {
        start = Some(Instant::now());
        println!(
            "Run function {}, vars num: {}, call stack: {}", fn_name,
            fn_vars.len(), call_stack.join(" -> "));
    }

    let mut call_stack = call_stack.to_owned();
    call_stack.push(f.name.clone());

    ctx.push_stack(fn_name.to_string());

    let mut r: Option<FnReturn<T>> = None;
    for i in &f.body {
        r = process_function_instruction(
            ctx, i, fn_vars, print_debug, &call_stack);
        if r.is_some() {
            break;
        }
    }
    // println!("{}", f.to_string());

    ctx.pop_stack();

    let r = r.expect("no return found");
    if print_debug {
        println!("Function {} done in {:?}", fn_name, start.unwrap().elapsed());
    }
    r
}
fn calc_function_expression_n<T, NS>(
    inst: &InstructionPointer, fn_vars: &mut Vec<Option<Var<T>>>,
    nodes: &mut Nodes<T, NS>, n: usize,
    call_stack: &Vec<String>) -> Vec<Var<T>>
where
    T: FieldOps + 'static,
    NS: NodesStorage + 'static{

    if n == 1 {
        let v = calc_function_expression(inst, fn_vars, nodes, call_stack);
        return vec![v];
    }

    match **inst {
        Instruction::Value(ref value_bucket) => {
            var_from_value_instruction_n(value_bucket, nodes, n, call_stack)
        }
        Instruction::Load(ref load_bucket) => {
            match load_bucket.address_type {
                AddressType::Variable => match load_bucket.src {
                    LocationRule::Indexed {
                        ref location,
                        ref template_header,
                    } => {
                        if template_header.is_some() {
                            panic!("not implemented: template_header expected to be None");
                        }
                        let var_idx = calc_function_expression(
                            location, fn_vars, nodes, call_stack);
                        let var_idx = var_idx.must_const_usize(
                            nodes, call_stack);
                        let mut result = Vec::with_capacity(n);
                        for i in 0..n {
                            result.push(match fn_vars[var_idx+i] {
                                Some(ref v) => v.clone(),
                                None => panic!("variable is not set yet"),
                            });
                        };
                        result
                    }
                    LocationRule::Mapped { .. } => {
                        todo!()
                    }
                },
                _ => {
                    panic!("not implemented for a function: {}", load_bucket.to_string());
                }
            }
        }
        _ => {
            panic!("not implemented: {}", inst.to_string())
        }
    }
}

fn calc_function_expression<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    inst: &InstructionPointer, fn_vars: &mut Vec<Option<Var<T>>>,
    nodes: &mut Nodes<T, NS>, call_stack: &Vec<String>) -> Var<T> {

    match **inst {
        Instruction::Value(ref value_bucket) => {
            match value_bucket.parse_as {
                ValueType::BigInt => match nodes.get(NodeIdx(value_bucket.value)) {
                    Some(Node::Constant(..)) => Var::Node(value_bucket.value),
                    _ => panic!("not a constant"),
                },
                ValueType::U32 => {
                    let v = (&nodes.ff).parse_usize(value_bucket.value)
                        .unwrap();
                    Var::Value(v)
                },
            }
        }
        Instruction::Load(ref load_bucket) => {
            match load_bucket.address_type {
                AddressType::Variable => match load_bucket.src {
                    LocationRule::Indexed {
                        ref location,
                        ref template_header,
                    } => {
                        if template_header.is_some() {
                            panic!("not implemented: template_header expected to be None");
                        }
                        let var_idx = calc_function_expression(
                            location, fn_vars, nodes, call_stack);
                        let var_idx = var_idx.to_const_usize(nodes)
                            .unwrap_or_else(|e| {
                                panic!("expected constant value: {}: {}",
                                       e, call_stack.join(" -> "));
                            });
                        match fn_vars[var_idx] {
                            Some(ref v) => v.clone(),
                            None => panic!("variable is not set yet"),
                        }
                    }
                    LocationRule::Mapped { .. } => {
                        todo!()
                    }
                },
                _ => {
                    panic!("not implemented for function: {}", load_bucket.to_string());
                }
            }
        }
        Instruction::Compute(ref compute_bucket) => {
            compute_function_expression(
                compute_bucket, fn_vars, nodes, call_stack)
        },
        _ => {
            panic!("not implemented: {}", inst.to_string())
        }
    }
}

fn node_from_var<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    v: &Var<T>, nodes: &mut Nodes<T, NS>) -> usize {

    match v {
        Var::Value(v) => {
            nodes.const_node_idx_from_value(*v)
        }
        Var::Node(node_idx) => *node_idx,
    }
}

fn compute_function_expression<T, NS>(
    compute_bucket: &ComputeBucket, fn_vars: &mut Vec<Option<Var<T>>>,
    nodes: &mut Nodes<T, NS>, call_stack: &Vec<String>) -> Var<T>
where
    T: FieldOps + 'static,
    NS: NodesStorage + 'static {

    if let Ok(op) = try_into_operation(compute_bucket.op) {
        assert_eq!(compute_bucket.stack.len(), 2);
        let a = calc_function_expression(
            compute_bucket.stack.first().unwrap(), fn_vars,
            nodes, call_stack);
        let b = calc_function_expression(
            compute_bucket.stack.get(1).unwrap(), fn_vars,
            nodes, call_stack);
        match (&a, &b) {
            (Var::Value(a), Var::Value(b)) => {
                Var::Value((&nodes.ff).op_duo(op, *a, *b))
            }
            _ => {
                let a_idx = node_from_var(&a, nodes);
                let b_idx = node_from_var(&b, nodes);
                Var::Node(nodes.push(Node::Op(op, a_idx, b_idx)).0)
            }
        }
    } else if let Ok(op) = try_into_uno_operation(compute_bucket.op) {
        assert_eq!(compute_bucket.stack.len(), 1);
        let a = calc_function_expression(
            compute_bucket.stack.first().unwrap(), fn_vars, nodes, call_stack);
        match &a {
            Var::Value(v) => {
                Var::Value((&nodes.ff).op_uno(op, *v))
            }
            Var::Node(node_idx) => {
                Var::Node(nodes.push(Node::UnoOp(op, *node_idx)).0)
            }
        }
    } else {
        panic!(
            "unsupported operator: {}: {}",
            compute_bucket.op.to_string(), call_stack.join(" -> "));
    }
}

enum FnReturn<T: FieldOps> {
    FnVar{idx: usize, ln: usize},
    Value(Var<T>),
}

fn build_return<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    return_bucket: &ReturnBucket, fn_vars: &mut Vec<Option<Var<T>>>,
    nodes: &mut Nodes<T, NS>, call_stack: &Vec<String>) -> FnReturn<T> {

    match *return_bucket.value {
        Instruction::Load(ref load_bucket) => {
            FnReturn::FnVar {
                idx: calc_return_load_idx(
                    load_bucket, fn_vars, nodes, call_stack),
                ln: return_bucket.with_size,
            }
        }
        Instruction::Compute(ref compute_bucket) => {
            let v = compute_function_expression(
                compute_bucket, fn_vars, nodes, call_stack);
            FnReturn::Value(v)
        }
        Instruction::Value(ref value_bucket) => {
            let mut vars = var_from_value_instruction_n(
                value_bucket, nodes, 1, call_stack);
            assert_eq!(vars.len(), 1, "expected one result value");
            FnReturn::Value(vars.pop().unwrap())
        }
        _ => {
            panic!("unexpected instruction for return statement: {}",
                   return_bucket.value.to_string());
        }
    }
}

fn calc_return_load_idx<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    load_bucket: &LoadBucket, fn_vars: &mut Vec<Option<Var<T>>>,
    nodes: &mut Nodes<T, NS>, call_stack: &Vec<String>) -> usize {

    match &load_bucket.address_type {
        AddressType::Variable => {}, // OK
        _ => {
            panic!("expected the return statement support only variable address type");
        }
    }
    let ip = if let LocationRule::Indexed { location, .. } = &load_bucket.src {
        location
    } else {
        panic!("not implemented: location rule supposed to be Indexed");
    };
    let idx = calc_function_expression(ip, fn_vars, nodes, call_stack);
    idx.must_const_usize(nodes, call_stack)
}

// return variable value and it's index
fn store_function_variable<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    store_bucket: &StoreBucket, fn_vars: &mut Vec<Option<Var<T>>>,
    nodes: &mut Nodes<T, NS>, call_stack: &Vec<String>) -> (Var<T>, usize) {

    assert!(matches!(store_bucket.dest_address_type, AddressType::Variable),
            "functions can store only inside variables: dest_address_type: {}",
            store_bucket.dest_address_type.to_string());

    let (location, template_header) = match &store_bucket.dest {
        LocationRule::Indexed {
            location,
            template_header,
        } => {
            (location, template_header)
        }
        LocationRule::Mapped { .. } => {
            panic!("location rule supposed to be Indexed for variables");
        }
    };

    let lvar_idx =
        calc_function_expression(
            location, fn_vars, nodes, call_stack)
            .must_const_usize(nodes, call_stack);

    assert_eq!(
        store_bucket.context.size, 1,
        "variable size in ternary expression must be 1: {}, {}:{}",
        template_header.as_ref().unwrap_or(&"-".to_string()),
        call_stack.join(" -> "), store_bucket.get_line());

    let v = calc_function_expression(
        &store_bucket.src, fn_vars, nodes, call_stack);

    (v, lvar_idx)
}

fn process_function_instruction<T, NS>(
    ctx: &mut BuildCircuitContext<T, NS>, inst: &InstructionPointer,
    fn_vars: &mut Vec<Option<Var<T>>>, print_debug: bool,
    call_stack: &Vec<String>) -> Option<FnReturn<T>>
where
    T: FieldOps + 'static,
    NS: NodesStorage + 'static {

    ctx.update_line(inst.get_line());

    // println!("function instruction: {:?}", inst.to_string());
    match **inst {
        Instruction::Store(ref store_bucket) => {
            // println!("store bucket: {}", store_bucket.to_string());
            match store_bucket.dest_address_type {
                AddressType::Variable => {
                    match &store_bucket.dest {
                        LocationRule::Indexed {
                            location,
                            template_header,
                        } => {
                            if template_header.is_some() {
                                panic!("not implemented: template_header expected to be None");
                            }
                            let lvar_idx =
                                calc_function_expression(
                                    location, fn_vars, ctx.nodes, call_stack)
                                    .must_const_usize(ctx.nodes, call_stack);
                            if store_bucket.context.size == 1 {
                                fn_vars[lvar_idx] = Some(calc_function_expression(
                                    &store_bucket.src, fn_vars, ctx.nodes,
                                    call_stack));
                            } else {
                                let values = calc_function_expression_n(
                                    &store_bucket.src, fn_vars, ctx.nodes,
                                    store_bucket.context.size, call_stack);
                                assert_eq!(values.len(), store_bucket.context.size);
                                for i in 0..store_bucket.context.size {
                                    fn_vars[lvar_idx + i] = Some(values[i].clone());
                                }
                            }
                            None
                        }
                        LocationRule::Mapped {..} => {
                            panic!("mapped location is not supported");
                        }
                    }
                }
                _ => {panic!("not a variable store inside a function")}
            }
        }
        Instruction::Branch(ref branch_bucket) => {
            // println!("branch bucket: {}", branch_bucket.to_string());

            let cond = calc_function_expression(
                &branch_bucket.cond, fn_vars, ctx.nodes, call_stack);
            let cond_const = cond.to_const(ctx.nodes);

            match cond_const {
                Ok(cond_const) => { // condition expression is static
                    let branch = if !cond_const.is_zero() {
                        &branch_bucket.if_branch
                    } else {
                        &branch_bucket.else_branch
                    };
                    for i in branch {
                        let r = process_function_instruction(
                            ctx, i, fn_vars, print_debug, call_stack);
                        if r.is_some() {
                            return r;
                        }
                    }
                }
                Err(NodeConstErr::InputSignal) => { // dynamic condition expression
                    // The only supported dynamic condition is a ternary operation
                    // Both branches should be exactly one operation of
                    // storing a variable to the same signal index.
                    assert_eq!(
                        branch_bucket.if_branch.len(), 1,
                        "expected a ternary operation but it doesn't looks like one as the 'if' branch is not of length 1: {}: {}:{}",
                        branch_bucket.else_branch.len(),
                        call_stack.join(" -> "), branch_bucket.get_line());
                    assert_eq!(
                        branch_bucket.else_branch.len(), 1,
                        "expected a ternary operation but it doesn't looks like one as the 'else' branch is not of length 1: {}: {}:{}",
                        branch_bucket.else_branch.len(),
                        call_stack.join(" -> "),
                        branch_bucket.line);
                    let (var_if, var_if_idx) = match *branch_bucket.if_branch[0] {
                        Instruction::Store(ref store_bucket) => {
                            store_function_variable(
                                store_bucket, fn_vars, ctx.nodes, call_stack)
                        }
                        _ => {
                            panic!(
                                "expected store operation in ternary operation of branch 'if': {}:{}",
                                call_stack.join(" -> "),
                                branch_bucket.if_branch[0].get_line());
                        }
                    };
                    let (var_else, var_else_idx) = match *branch_bucket.else_branch[0] {
                        Instruction::Store(ref store_bucket) => {
                            store_function_variable(
                                store_bucket, fn_vars, ctx.nodes, call_stack)
                        }
                        _ => {
                            panic!(
                                "expected store operation in ternary operation of branch 'else': {}",
                                call_stack.join(" -> "));
                        }
                    };
                    assert_eq!(
                        var_if_idx, var_else_idx,
                        "in ternary operation if and else branches must store to the same variable");

                    let cond_node_idx = node_from_var(&cond, ctx.nodes);
                    let if_node_idx = node_from_var(&var_if, ctx.nodes);
                    let else_node_idx = node_from_var(&var_else, ctx.nodes);
                    let tern_node_idx = ctx.nodes.push(Node::TresOp(
                        TresOperation::TernCond, cond_node_idx, if_node_idx,
                        else_node_idx));
                    fn_vars[var_if_idx] = Some(Var::Node(tern_node_idx.0));
                }
                Err(e) => {
                    panic!(
                        "error calculating function branch condition: {}: {}",
                        e, call_stack.join(" -> "));
                }
            }
            None
        }
        Instruction::Return(ref return_bucket) => {
            // println!("return bucket: {}", return_bucket.to_string());
            Some(build_return(return_bucket, fn_vars, ctx.nodes, call_stack))
        }
        Instruction::Loop(ref loop_bucket) => {
            // if call_stack.last().unwrap() == "long_div" {
            //     println!("loop: {}", loop_bucket.to_string());
            // }
            while check_continue_condition_function(
                &loop_bucket.continue_condition, fn_vars, ctx.nodes,
                call_stack) {

                for i in &loop_bucket.body {
                    process_function_instruction(
                        ctx, i, fn_vars, print_debug, call_stack);
                }
            };
            None
        }
        Instruction::Call(ref call_bucket) => {
            let mut new_fn_vars: Vec<Option<Var<T>>> = vec![None; call_bucket.arena_size];

            let mut count: usize = 0;
            for (idx, inst2) in call_bucket.arguments.iter().enumerate() {
                let args = calc_function_expression_n(
                    inst2, fn_vars, ctx.nodes,
                    call_bucket.argument_types[idx].size, call_stack);
                for arg in args {
                    new_fn_vars[count] = Some(arg);
                    count += 1;
                }
            }

            let r = run_function(
                ctx, &call_bucket.symbol, &mut new_fn_vars, print_debug,
                call_stack);

            match call_bucket.return_info {
                ReturnType::Intermediate{ ..} => { todo!(); }
                ReturnType::Final( ref final_data ) => {
                    if let FnReturn::FnVar { ln, ..} = r {
                        assert!(final_data.context.size >= ln);
                    }
                    // assert_eq!(final_data.context.size, r.ln);
                    store_function_return_results_into_variable(
                        final_data, &new_fn_vars, &r, fn_vars, ctx.nodes,
                        call_stack);
                }
            };
            None
        }
        Instruction::Assert(_) => {
            // asserts are not supported in witness graph
            None
        }
        _ => {
            panic!(
                "not implemented: {}; {}",
                inst.to_string(), call_stack.join(" -> "));
        }
    }
}

fn check_continue_condition_function<T, NS>(
    inst: &InstructionPointer, fn_vars: &mut Vec<Option<Var<T>>>,
    nodes: &mut Nodes<T, NS>, call_stack: &Vec<String>) -> bool
where
    T: FieldOps + 'static,
    NS: NodesStorage + 'static {

    let val = calc_function_expression(inst, fn_vars, nodes, call_stack)
        .to_const(nodes)
        .unwrap_or_else(
            |e| panic!(
                "condition is not a constant: {}: {}:{}",
                e, call_stack.join(" -> "), inst.get_line()));

    !val.is_zero()
}

fn find_function<'a>(name: &str, functions: &'a [FunctionCode]) -> &'a FunctionCode {
    functions.iter().find(|f| f.header == name).expect("function not found")
}

fn bigint_to_usize<T: FieldOps>(value: T) -> Result<usize, Box<dyn Error>> {
    let bs = value.to_le_bytes();
    for item in bs.iter().skip(size_of::<usize>()) {
        if *item != 0 {
            return Err(anyhow!("value is too big").into());
        }
    }
    let usize_bytes: [u8; size_of::<usize>()] = bs[..size_of::<usize>()]
        .try_into()?;
    Ok(usize::from_le_bytes(usize_bytes))
}


struct ComponentInstance<T: FieldOps> {
    template_id: usize,
    signal_offset: usize,
    number_of_inputs: usize,
    component_offset: usize, // global component index
    vars: Vec<Option<Var<T>>>,
    subcomponents: Vec<Option<ComponentInstance<T>>>,
}

fn fmt_create_cmp_bucket<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    ctx: &mut BuildCircuitContext<T, NS>,
    cmp_bucket: &CreateCmpBucket,
    cmp: &mut ComponentInstance<T>,
    print_debug: bool,
    call_stack: &Vec<String>,
) -> String {
    let sub_cmp_id = calc_expression(
        ctx, &cmp_bucket.sub_cmp_id, cmp, print_debug, call_stack);

    let sub_cmp_id = match sub_cmp_id {
        Var::Value(ref c) => format!("Constant {}", c),
        Var::Node(idx) => format!("Variable {}", idx)
    };

    format!(
        r#"CreateCmpBucket: template_id: {}
                 cmp_unique_id: {}
                 symbol: {}
                 sub_cmp_id: {}
                 name_subcomponent: {}
                 defined_positions: {:?}
                 dimensions: {:?}
                 signal_offset: {}
                 signal_offset_jump: {}
                 component_offset: {}
                 component_offset_jump: {}
                 number_of_cmp: {}
                 has_inputs: {}
                 component_signal_start: {}"#,
        cmp_bucket.template_id,
        cmp_bucket.cmp_unique_id,
        cmp_bucket.symbol,
        sub_cmp_id,
        cmp_bucket.name_subcomponent,
        cmp_bucket.defined_positions,
        cmp_bucket.dimensions,
        cmp_bucket.signal_offset,
        cmp_bucket.signal_offset_jump,
        cmp_bucket.component_offset,
        cmp_bucket.component_offset_jump,
        cmp_bucket.number_of_cmp,
        cmp_bucket.has_inputs,
        cmp.signal_offset,
    )
}

#[derive(Clone, Debug)]
enum Var<T: FieldOps> {
    Value(T),
    Node(usize),
}

impl<T: FieldOps> fmt::Display for Var<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Var::Value(c) => write!(f, "Var::Value({})", c),
            Var::Node(idx) => write!(f, "Var::Node({})", idx),
        }
    }
}


impl<T: FieldOps + 'static> Var<T> {
    fn to_const<NS: NodesStorage + 'static>(
        &self, nodes: &Nodes<T, NS>) -> Result<T, NodeConstErr> {

        match self {
            Var::Value(v) => Ok(*v),
            Var::Node(node_idx) => nodes.to_const_recursive(NodeIdx::from(*node_idx)),
        }
    }

    fn to_const_usize<NS: NodesStorage + 'static>(
        &self, nodes: &Nodes<T, NS>) -> Result<usize, Box<dyn Error>> {

        let c = self.to_const(nodes)?;
        bigint_to_usize(c)
    }

    fn must_const_usize<NS: NodesStorage + 'static>(
        &self, nodes: &Nodes<T, NS>, call_stack: &[String]) -> usize {

        self.to_const_usize(nodes).unwrap_or_else(|e| {
            panic!("{}: {}", e, call_stack.join(" -> "));
        })
    }
}

fn load_n<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    ctx: &mut BuildCircuitContext<T, NS>, load_bucket: &LoadBucket,
    cmp: &mut ComponentInstance<T>, size: usize, print_debug: bool,
    call_stack: &Vec<String>) -> Vec<Var<T>> {

    match load_bucket.address_type {
        AddressType::Signal => match &load_bucket.src {
            LocationRule::Indexed {
                location,
                template_header,
            } => {
                if template_header.is_some() {
                    panic!("not implemented: template_header expected to be None");
                }
                let signal_idx = calc_expression(
                    ctx, location, cmp, print_debug, call_stack);
                let signal_idx = signal_idx.must_const_usize(
                    ctx.nodes, call_stack);
                let mut result = Vec::with_capacity(size);
                for i in 0..size {
                    let signal_idx = cmp.signal_offset + signal_idx + i;
                    let signal_node = ctx.signal_node_idx[signal_idx];
                    assert_ne!(
                        signal_node, usize::MAX,
                        "signal {}/{}/{} is not set yet",
                        cmp.signal_offset, signal_idx, i);
                    result.push(Var::Node(signal_node));
                }
                result
            }
            LocationRule::Mapped { .. } => {
                panic!("mapped signals expect only on address type SubcmpSignal");
            }
        },
        AddressType::SubcmpSignal {
            ref cmp_address, ..
        } => {
            let subcomponent_idx =
                calc_expression(
                    ctx, cmp_address, cmp, print_debug, call_stack)
                    .must_const_usize(ctx.nodes, call_stack);

            let (signal_idx, template_header) = match load_bucket.src {
                LocationRule::Indexed {
                    ref location,
                    ref template_header,
                } => {
                    let signal_idx = calc_expression(
                        ctx, location, cmp, print_debug, call_stack);
                    let signal_idx = signal_idx.to_const_usize(ctx.nodes)
                        .unwrap_or_else(|e| panic!(
                            "can't calculate signal index: {}: {}",
                            e, call_stack.join(" -> ")));
                    (signal_idx,
                     template_header.as_ref().unwrap_or(&"-".to_string()).clone())
                }
                LocationRule::Mapped { ref signal_code, ref indexes } => {
                    calc_mapped_signal_idx(
                        ctx, cmp, subcomponent_idx, *signal_code, indexes,
                        print_debug, call_stack)
                }
            };
            let signal_offset = cmp.subcomponents[subcomponent_idx]
                .as_ref().unwrap().signal_offset;

            if print_debug {
                let location_rule = match load_bucket.src {
                    LocationRule::Indexed { .. } => "Indexed",
                    LocationRule::Mapped { .. } => "Mapped",
                };
                println!(
                    "Load subcomponent signal (location: {}, template: {}, subcomponent idx: {}, size: {}): {} + {} = {}",
                    location_rule, template_header, subcomponent_idx, size,
                    signal_offset, signal_idx, signal_offset + signal_idx);
            }

            let signal_idx = signal_offset + signal_idx;
            let mut result = Vec::with_capacity(size);
            for i in 0..size {
                let signal_node = ctx.signal_node_idx[signal_idx + i];
                assert_ne!(
                    signal_node, usize::MAX,
                    "subcomponent signal {}/{}/{} is not set yet",
                    cmp.signal_offset, signal_idx, i);
                result.push(Var::Node(signal_node));
            }
            result
        }
        AddressType::Variable => {
            let location = if let LocationRule::Indexed { location, template_header } = &load_bucket.src {
                if template_header.is_some() {
                    panic!("template_header expected to be None");
                }
                location
            } else {
                panic!("location rule supposed to be Indexed for AddressType::Variable");
            };
            let var_idx =
                calc_expression(ctx, location, cmp, print_debug, call_stack)
                    .must_const_usize(ctx.nodes, call_stack);

            let mut result: Vec<Var<T>> = Vec::with_capacity(size);
            for i in 0..size {
                result.push(match cmp.vars[var_idx + i] {
                    Some(ref v) => v.clone(),
                    None => panic!("variable is not set yet"),
                });
            }
            result
        },
    }
}

fn build_unary_op_var<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    ctx: &mut BuildCircuitContext<T, NS>,
    cmp: &mut ComponentInstance<T>,
    op: UnoOperation,
    stack: &[InstructionPointer],
    print_debug: bool,
    call_stack: &Vec<String>,
) -> Var<T> {
    assert_eq!(stack.len(), 1);
    let a = calc_expression(
        ctx, &stack[0], cmp, print_debug, call_stack);

    match &a {
        Var::Value(a) => {
            Var::Value((&ctx.nodes.ff).op_uno(op, *a))
        }
        Var::Node(node_idx) => {
            let node = Node::UnoOp(op, *node_idx);
            Var::Node(ctx.nodes.push(node).0)
        }
    }
}

// Create a Var from operation on two arguments a anb b
fn build_binary_op_var<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    ctx: &mut BuildCircuitContext<T, NS>,
    cmp: &mut ComponentInstance<T>,
    op: Operation,
    stack: &[InstructionPointer],
    print_debug: bool,
    call_stack: &Vec<String>,
) -> Var<T> {
    assert_eq!(stack.len(), 2);
    let a = calc_expression(
        ctx, &stack[0], cmp, print_debug, call_stack);
    let b = calc_expression(
        ctx, &stack[1], cmp, print_debug, call_stack);

    let mut node_idx = |v: &Var<T>| match v {
        Var::Value(c) => {
            ctx.nodes.const_node_idx_from_value(*c)
        }
        Var::Node(idx) => { *idx }
    };

    match (&a, &b) {
        (Var::Value(a), Var::Value(b)) => {
            Var::Value((&ctx.nodes.ff).op_duo(op, *a, *b))
        }
        _ => {
            let node = Node::Op(op, node_idx(&a), node_idx(&b));
            Var::Node(ctx.nodes.push(node).0)
        }
    }
}

// This function should calculate node based only on constant or variable
// values. Not based on signal values.
fn calc_expression<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    ctx: &mut BuildCircuitContext<T, NS>,
    inst: &InstructionPointer,
    cmp: &mut ComponentInstance<T>,
    print_debug: bool,
    call_stack: &Vec<String>,
) -> Var<T> {
    match **inst {
        Instruction::Value(ref value_bucket) => {
            let mut vars = var_from_value_instruction_n(
                value_bucket, ctx.nodes, 1, call_stack);
            assert_eq!(vars.len(), 1, "expected one result value");
            vars.pop().unwrap()
        }
        Instruction::Load(ref load_bucket) => {
            let r = load_n(
                ctx, load_bucket, cmp, 1, print_debug, call_stack);
            assert_eq!(r.len(), 1);
            r[0].clone()
        },
        Instruction::Compute(ref compute_bucket) => {
            // try duo operation
            if let Ok(op) = try_into_operation(compute_bucket.op) {
                build_binary_op_var(
                    ctx, cmp, op, &compute_bucket.stack, print_debug,
                    call_stack)
            }

            // try uno operation
            else if let Ok(op) = try_into_uno_operation(compute_bucket.op) {
                build_unary_op_var(
                    ctx, cmp, op, &compute_bucket.stack, print_debug,
                    call_stack)
            }

            else {
                panic!("Unsupported operator type: {}", compute_bucket.op.to_string());
            }

        }
        _ => {
            panic!(
                "instruction evaluation is not supported: {}",
                inst.to_string()
            );
        }
    }
}

// This function should calculate node based only on constant or variable
// values. Not based on signal values.
fn calc_expression_n<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    ctx: &mut BuildCircuitContext<T, NS>,
    inst: &InstructionPointer,
    cmp: &mut ComponentInstance<T>,
    size: usize,
    print_debug: bool,
    call_stack: &Vec<String>,
) -> Vec<Var<T>> {
    if size == 1 {
        return vec![calc_expression(ctx, inst, cmp, print_debug, call_stack)];
    }

    match **inst {
        Instruction::Load(ref load_bucket) => {
            load_n(
                ctx, load_bucket, cmp, size, print_debug, call_stack)
        },
        _ => {
            panic!(
                "instruction evaluation is not supported for multiple values: {}",
                inst.to_string()
            );
        }
    }
}

fn check_continue_condition<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    ctx: &mut BuildCircuitContext<T, NS>,
    cmp: &mut ComponentInstance<T>,
    inst: &InstructionPointer,
    print_debug: bool,
    call_stack: &Vec<String>,
) -> bool {
    let val = calc_expression(ctx, inst, cmp, print_debug, call_stack)
        .to_const(ctx.nodes)
        .unwrap_or_else(
            |e| panic!(
                "condition is not a constant: {}: {}",
                e, call_stack.join(" -> ")));

    !val.is_zero()
}

fn get_constants<T: FieldOps>(
    ff: &Field<T>,
    circuit: &Circuit) -> Result<Vec<T>, Box<dyn Error>> {

    let mut constants: Vec<T> = Vec::new();
    for c in &circuit.c_producer.field_tracking {
        constants.push(ff.parse_str(c.as_str())?);
    }
    Ok(constants)
}

fn init_input_signals<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    circuit: &Circuit,
    nodes: &mut Nodes<T, NS>,
    signal_node_idx: &mut [usize],
) -> InputSignalsInfo {

    let input_list = circuit.c_producer.get_main_input_list();
    let mut signal_values: Vec<T> = Vec::new();
    signal_values.push(T::one());
    signal_node_idx[0] = nodes.push(Node::Input(signal_values.len() - 1)).0;
    let mut inputs_info = HashMap::new();

    for (name, offset, len) in input_list {
        inputs_info.insert(name.clone(), (signal_values.len(), *len));
        for i in 0..*len {
            signal_values.push(T::zero());
            signal_node_idx[offset + i] = nodes.push(
                Node::Input(signal_values.len() - 1)).0;
        }
    }

    inputs_info
}

fn run_template<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    ctx: &mut BuildCircuitContext<T, NS>, print_debug: bool, call_stack: &[String],
    cmp: &mut ComponentInstance<T>) {

    let tmpl = &ctx.templates[cmp.template_id];

    let tmpl_name: String = format!("{}_{}", tmpl.name, tmpl.id);
    let mut call_stack = call_stack.to_owned();
    call_stack.push(tmpl_name.clone());

    ctx.push_stack(tmpl_name);
    if print_debug {
        println!(
            "Run template {}_{}: body length: {}", tmpl.name, tmpl.id,
            tmpl.body.len());
    }

    for inst in &tmpl.body {
        process_instruction(ctx, inst, print_debug, &call_stack, cmp);
    }

    ctx.pop_stack();

    if print_debug {
        println!("Template {}_{} finished", tmpl.name, tmpl.id);
    }
    // TODO: assert all components run
}

enum Prime {
    Bn128,
    // Bls12381,
    Goldilocks,
    Grumpkin,
    // Pallas,
    // Vesta,
    // Secq256r1,
}

impl Prime {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Bn128 => "bn128",
            Self::Goldilocks => "goldilocks",
            Self::Grumpkin => "grumpkin",
        }
    }
}

impl FromStr for Prime {
    type Err = Box<dyn Error>;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            s if s == Self::Bn128.as_str() => Ok(Prime::Bn128),
            s if s == Self::Goldilocks.as_str() => Ok(Prime::Goldilocks),
            s if s == Self::Grumpkin.as_str() => Ok(Prime::Grumpkin),
            _ => Err(anyhow!("Invalid prime: {}", s).into()),
        }
    }
}

enum OptimizationLevel {
    O0,
    O1,
    O2,
}

struct Args {
    circuit_file: String,
    graph_file: String,
    link_libraries: Vec<PathBuf>,
    print_debug: bool,
    prime: Prime,
    r1cs: Option<String>,
    sym: Option<String>,
    optimization_level: Option<OptimizationLevel>,
    named_temp: bool,
    temp_dir: PathBuf,
}

fn parse_args() -> Args {
    let args: Vec<String> = env::args().collect();
    let mut i = 1;
    let mut circuit_file: Option<String> = None;
    let mut graph_file: Option<String> = None;
    let mut link_libraries: Vec<PathBuf> = Vec::new();
    let mut print_debug = false;
    let mut r1cs_file: Option<String> = None;
    let mut sym_file: Option<String> = None;
    let mut prime: Option<Prime> = None;
    let mut optimization_level: Option<OptimizationLevel> = None;
    let mut named_temp: bool = false;
    let mut temp_dir: Option<PathBuf> = None;

    let usage = |err_msg: &str| {
        if !err_msg.is_empty() {
            eprintln!("ERROR:");
            eprintln!("    {}", err_msg);
            eprintln!();
        }
        eprintln!("USAGE:");
        eprintln!("    {} <circuit_file> <wcd_file> [OPTIONS]", args[0]);
        eprintln!();
        eprintln!("ARGUMENTS:");
        eprintln!("    <circuit_file>    Path to the Circom file with main template");
        eprintln!("    <wcd_file>        File where the witness calculation description will be saved");
        eprintln!();
        eprintln!("OPTIONS:");
        eprintln!("    -h | --help             Display this help message");
        eprintln!("    -l <link_libraries>...  Adds directory to library search path.");
        eprintln!("                            Can be used multiple times.");
        eprintln!("    -p <prime>              The prime field to use. Valid options are 'bn128' (default),");
        eprintln!("                            'goldilocks', and 'grumpkin'");
        eprintln!("    --sym <sym_file>        Path to the .sym file.");
        eprintln!("    --r1cs <r1cs_file>      Path to the R1CS file. If provided, the R1CS file will be");
        eprintln!("                            saved along with the generated graph");
        eprintln!("    --O0, --O1, --O2        Optimization level for circom. Default is --O2");
        eprintln!("    -v                      Verbose mode");
        eprintln!("    --named-temp            Normally, we create an unnamed temporary file that the OS");
        eprintln!("                            deletes when the process exits. With this flag enabled, we");
        eprintln!("                            create a named file so it can be inspected if needed. It is");
        eprintln!("                            usually removed on exit, but if the process is killed abnormally,");
        eprintln!("                            the file may remain");
        eprintln!("    --temp-dir              Directory for creating the temporary file with nodes.");
        eprintln!("                            Even for unnamed temporary files, this directory can specify");
        eprintln!("                            the target partition");
        let ret_code = if err_msg.is_empty() { 0 } else { 1 };
        std::process::exit(ret_code);
    };

    while i < args.len() {
        if args[i] == "-l" {
            i += 1;
            if i >= args.len() {
                usage("missing argument for -l");
            }
            link_libraries.push(args[i].clone().into());
        } else if args[i] == "--help" || args[i] == "-h" {
            usage("");
        } else if args[i].starts_with("-l") {
            link_libraries.push(args[i][2..].to_string().into())
        } else if args[i] == "-v" {
            print_debug = true;
        } else if args[i] == "--r1cs" {
            i += 1;
            if i >= args.len() {
                usage("missing argument for --r1cs");
            }
            if r1cs_file.is_some() {
                usage("multiple r1cs files");
            }
            r1cs_file = Some(args[i].clone());
        } else if args[i] == "--sym" {
            i += 1;
            if i >= args.len() {
                usage("missing argument for --sym");
            }
            if sym_file.is_some() {
                usage("multiple sym files");
            }
            sym_file = Some(args[i].clone());
        } else if args[i] == "--O0" {
            if optimization_level.is_some() {
                usage("multiple optimization levels set");
            }
            optimization_level = Some(OptimizationLevel::O0);
        } else if args[i] == "--O1" {
            if optimization_level.is_some() {
                usage("multiple optimization levels set");
            }
            optimization_level = Some(OptimizationLevel::O1);
        } else if args[i] == "--O2" {
            if optimization_level.is_some() {
                usage("multiple optimization levels set");
            }
            optimization_level = Some(OptimizationLevel::O2);
        } else if args[i] == "-p" {
            i += 1;
            if i >= args.len() {
                usage("missing argument for -p");
            }
            if prime.is_some() {
                usage("multiple primes specified");
            }
            match Prime::from_str(&args[i]) {
                Err(e) => {
                    usage(e.to_string().as_str());
                }
                Ok(p) => {
                    prime = Some(p)
                }
            }
        } else if args[i] == "--named-temp" {
            named_temp = true;
        } else if args[i] == "--temp-dir" {
            i += 1;
            if i >= args.len() {
                usage("missing argument for --temp-dir");
            }
            if temp_dir.is_some() {
                usage("multiple --temp-dir specified");
            }
            temp_dir = Some(args[i].clone().into());
        } else if args[i].starts_with("-") {
            let message = format!("unknown argument: {}", args[i]);
            usage(&message);
        } else if circuit_file.is_none() {
            circuit_file = Some(args[i].clone());
        } else if graph_file.is_none() {
            graph_file = Some(args[i].clone());
        } else {
            usage(format!("unexpected argument: {}", args[i]).as_str());
        }
        i += 1;
    };

    Args {
        circuit_file: circuit_file.unwrap_or_else(|| { usage("missing circuit file") }),
        graph_file: graph_file.unwrap_or_else(|| { usage("missing graph file") }),
        link_libraries,
        print_debug,
        prime: prime.unwrap_or(Prime::Bn128),
        r1cs: r1cs_file,
        sym: sym_file,
        optimization_level,
        named_temp,
        temp_dir: temp_dir.unwrap_or_else(env::temp_dir),
    }
}

fn main() {
    let args = parse_args();

    let version = "2.1.9";

    let parser_result = parser::run_parser(
        args.circuit_file.clone(), version, args.link_libraries.clone());
    let mut program_archive = match parser_result {
        Err((file_library, report_collection)) => {
            Report::print_reports(&report_collection, &file_library);
            panic!("Parser error");
        }
        Ok((program_archive, warnings)) => {
            if !warnings.is_empty() {
                println!("Parser warnings:");
                for warning in warnings {
                    println!("{}", warning.get_message());
                }
            }
            program_archive
        }
    };

    match check_types(&mut program_archive) {
        Err(errs) => {
            Report::print_reports(&errs, &program_archive.file_library);
            std::process::exit(1);
        }
        Ok(warns) => {
            if !warns.is_empty() {
                Report::print_reports(&warns, &program_archive.file_library);
            }
        }
    }

    let (no_rounds, flag_s, flag_f) =
        match args.optimization_level {
            Some(OptimizationLevel::O0) => {
                (0usize, false, true)
            }
            Some(OptimizationLevel::O1) => {
                (0usize, true, false)
            },
            Some(OptimizationLevel::O2) | None => {
                (usize::MAX, false, false)
            }
        };

    let build_config = BuildConfig {
        no_rounds,
        flag_json_sub: false,
        json_substitutions: String::new(),
        flag_s,
        flag_f,
        flag_p: false,
        flag_verbose: false,
        flag_old_heuristics: false,
        inspect_constraints: false,
        prime: args.prime.as_str().to_string(),
    };

    let custom_gates = program_archive.custom_gates;

    let (cw, vcp) =
        build_circuit(program_archive, build_config)
            .unwrap();

    if let Some(r1cs_file) = &args.r1cs {
        if cw.r1cs(r1cs_file, custom_gates).is_err() {
            panic!(
                "Could not write the output in the given path: {}",
                r1cs_file);
        }
        println!("R1CS written successfully: {}", r1cs_file);
    }

    if let Some(sym_file) = &args.sym {
        if cw.sym(sym_file).is_err() {
            panic!(
                "Could not write the symbol output in the given path: {}",
                sym_file);
        }
        println!(".sym written successfully: {}", sym_file);
    }

    let circuit = run_compiler(
        vcp.clone(),
        Config {
            debug_output: false,
            produce_input_log: true,
            wat_flag: false,
        },
        version)
        .unwrap();
    println!("prime: {}", circuit.c_producer.prime);
    println!("prime_str: {}", circuit.c_producer.prime_str);
    println!("templates len: {}", circuit.templates.len());
    println!("functions len: {}", circuit.functions.len());
    println!("main header: {}", circuit.c_producer.main_header);


    let prime = U256::from_str_radix(
        circuit.c_producer.prime.as_str(), 10).unwrap();
    match prime.bit_len() {
        64 => {
            let prime = U64::from_str(
                circuit.c_producer.prime.as_str()).unwrap();
            build_graph(
                prime, circuit.c_producer.prime_str.as_str(), &circuit, &args,
                &vcp, args.named_temp, &args.temp_dir)
        }
        254 => {
            let prime = <U254 as FieldOps>::from_str(
                circuit.c_producer.prime.as_str()).unwrap();
            build_graph(
                prime, circuit.c_producer.prime_str.as_str(), &circuit, &args,
                &vcp, args.named_temp, &args.temp_dir)
        }
        _ => {
            panic!(
                "unsupported prime size: {}, prime: {}",
                prime.bit_len(), prime);
        }
    }
}

fn build_graph<T: FieldOps + 'static>(
    prime: T, curve_name: &str, circuit: &Circuit,
    args: &Args, vcp: &VCP, named_temp: bool, temp_dir: &Path) {

    let nodes_storage = MMapNodes::new(named_temp, temp_dir);
    let mut nodes = Nodes::new(
        prime, curve_name, nodes_storage);
    let constants = get_constants(&nodes.ff, circuit).unwrap();
    for c in constants.iter() {
        nodes.const_node_idx_from_value(*c);
    }
    assert_eq!(
        nodes.nodes.len(), constants.len(),
        "duplicate constant found in circuit");

    // The node indexes for each signal. For example in
    // signal_node_idx[3] stored the node index for signal 3.
    let mut signal_node_idx: Vec<usize> =
        vec![usize::MAX; circuit.c_producer.total_number_of_signals];

    let input_signals: InputSignalsInfo = init_input_signals(
        circuit, &mut nodes, &mut signal_node_idx);

    // assert that template id is equal to index in templates list
    for (i, t) in circuit.templates.iter().enumerate() {
        assert_eq!(i, t.id);
        if args.print_debug {
            println!("Template #{}: {}", i, t.name);
        }
    }

    let mut ctx = BuildCircuitContext::new(
        &mut nodes, &mut signal_node_idx, &circuit.templates,
        &circuit.functions, circuit.c_producer.get_io_map());

    let main_component_signal_start = 1usize;
    let main_template_id = vcp.main_id;
    let mut main_component = ctx.new_component(
        main_template_id, main_component_signal_start, 0);

    run_template(&mut ctx, args.print_debug, &[], &mut main_component);

    ctx.update_progress_message();
    ctx.progress_bar.finish();

    let unset_signals_num = signal_node_idx.iter()
        .filter(|s| **s == usize::MAX)
        .count();
    if unset_signals_num > 0 {
        println!("[warning] {} signals are not set", unset_signals_num);
    }
    // for (idx, i) in signal_node_idx.iter().enumerate() {
    //     if *i == usize::MAX {
    //         println!("[warning] signal #{} is not set", idx);
    //     }
    // }

    let witness_list = vcp.get_witness_list().clone();
    let mut witness_node_idxes = witness_list
        .iter().enumerate()
        .map(|(idx, i)| {
            assert_ne!(*i, usize::MAX, "signal #{} is not set", idx);
            signal_node_idx[*i]
        })
        .collect::<Vec<_>>();

    println!("number of nodes {}, signals {}", nodes.len(), witness_node_idxes.len());

    optimize(&mut nodes, &mut witness_node_idxes);

    println!(
        "number of nodes after optimize {}, signals {}",
        nodes.len(), witness_node_idxes.len());

    let f = fs::File::create(&args.graph_file).unwrap();
    serialize_witnesscalc_graph(f, &nodes, &witness_node_idxes, &input_signals).unwrap();

    println!("circuit graph saved to file: {}", &args.graph_file);
}

struct SignalDestination<'a> {
    cmp_address: &'a InstructionPointer,
    input_information: &'a InputInformation,
    dest: &'a LocationRule,
}

fn store_subcomponent_signals<T, NS>(
    ctx: &mut BuildCircuitContext<T, NS>, src_node_idxs: &[usize],
    print_debug: bool, call_stack: &Vec<String>, cmp: &mut ComponentInstance<T>,
    dst: &SignalDestination)
where
    T: FieldOps + 'static,
    NS: NodesStorage + 'static {

    let input_status: &StatusInput;
    if let InputInformation::Input { status } = dst.input_information {
        input_status = status;
    } else {
        panic!("incorrect input information for subcomponent signal");
    }

    let subcomponent_idx =
        calc_expression(ctx, dst.cmp_address, cmp, print_debug, call_stack)
            .must_const_usize(ctx.nodes, call_stack);

    let (signal_idx, template_header) = match dst.dest {
        LocationRule::Indexed {
            location,
            template_header,
        } => {
            let signal_idx =
                calc_expression(ctx, location, cmp, print_debug, call_stack)
                    .must_const_usize(ctx.nodes, call_stack);
            (signal_idx, template_header.as_ref().unwrap_or(&"-".to_string()).clone())
        }
        LocationRule::Mapped { signal_code, indexes } => {
            calc_mapped_signal_idx(
                ctx, cmp, subcomponent_idx, *signal_code, indexes, print_debug,
                call_stack)
        }
    };

    let sub_cmp = cmp.subcomponents[subcomponent_idx]
        .as_mut().unwrap();
    let signal_offset = sub_cmp.signal_offset;

    if print_debug {
        let location = match dst.dest {
            LocationRule::Indexed { .. } => "Indexed",
            LocationRule::Mapped { .. } => "Mapped",
        };
        println!(
            "Store subcomponent signal (location: {}, template: {}, subcomponent idx: {}, num: {}): {} + {} = {}",
            location, template_header, subcomponent_idx, src_node_idxs.len(),
            signal_offset, signal_idx, signal_offset + signal_idx);
    }

    let signal_idx = signal_offset + signal_idx;
    for (i, v) in src_node_idxs.iter().enumerate() {
        ctx.associate_signal_to_node(signal_idx + i, *v);
    }
    sub_cmp.number_of_inputs -= src_node_idxs.len();

    let run_component = match input_status {
        StatusInput::Last => {
            assert_eq!(sub_cmp.number_of_inputs, 0);
            true
        }
        StatusInput::NoLast => {
            assert!(sub_cmp.number_of_inputs > 0);
            false
        }
        StatusInput::Unknown => sub_cmp.number_of_inputs == 0,
    };

    if run_component {
        run_template(ctx, print_debug, call_stack, sub_cmp)
    }
}

#[cfg(test)]
mod tests {
    use circom_witnesscalc::field::U254;
    use crate::bigint_to_usize;

    fn u254(s: &str) -> U254 {
        let x = U254::from_str_radix(s, 10).expect(s);
        let prime = U254::from_str_radix(
            "21888242871839275222246405745257275088548364400416034343698204186575808495617",
            10)
            .unwrap();
        if x > prime {
            panic!("{}", s)
        };
        x
    }

    #[test]
    fn test_bigint_to_usize() {
        let v = u254("0");
        let r = bigint_to_usize(v);
        assert!(matches!(r, Ok(0usize)));

        let v = U254::from(usize::MAX);
        let r = bigint_to_usize(v);
        assert!(matches!(r, Ok(usize::MAX)));

        let v = v + v;
        let r = bigint_to_usize(v);
        assert!(r.is_err());
    }
}
