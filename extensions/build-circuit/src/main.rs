use compiler::circuit_design::template::TemplateCode;
use compiler::compiler_interface::{run_compiler, Circuit, Config, VCP};
use compiler::intermediate_representation::ir_interface::{AccessType, AddressType, BranchBucket, ComputeBucket, CreateCmpBucket, FinalData, InputInformation, Instruction, InstructionPointer, LoadBucket, LocationRule, ObtainMeta, OperatorType, ReturnBucket, ReturnType, SizeOption, StatusInput, ValueBucket, ValueType};
use constraint_generation::{build_circuit, BuildConfig};
use program_structure::error_definition::Report;
use std::collections::{BTreeMap, BTreeSet};
use std::{env, fmt, fs};
use std::error::Error;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Instant;
use anyhow::anyhow;
use code_producers::c_elements::{FieldMap, IODef};
use code_producers::components::TemplateInstanceIOMap;
use compiler::circuit_design::function::FunctionCode;
use indicatif::ProgressBar;
use ruint::aliases::U256;
use type_analysis::check_types::check_types;
use program_structure::constants::UsefulConstants;
use circom_witnesscalc::{progress_bar, vm2};
use circom_witnesscalc::field::{Field, FieldOperations, FieldOps, U254, U64};
use circom_witnesscalc::graph::{Node, Operation, UnoOperation, TresOperation, Nodes, NodeConstErr, NodeIdx, NodesInterface, optimize, MMapNodes, NodesStorage};
use circom_witnesscalc::storage::serialize_witnesscalc_graph;
use compiler::hir::very_concrete_program::{BusInstance, FieldInfo};
use program_structure::ast::SignalType;
use circom_witnesscalc::vm2::{TypeField, TypeFieldKind};
use circom_witnesscalc::vm2::InputInfoSliceExt;

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
        OperatorType::Eq(SizeOption::Single(1)) => Ok(Operation::Eq),
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
        OperatorType::Eq(SizeOption::Single(1)) => err,
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

fn resolve_size_for_template(size: &SizeOption, template_id: usize) -> usize {
    match size {
        SizeOption::Single(value) => *value,
        SizeOption::Multiple(values) => values
            .iter()
            .find(|(id, _)| *id == template_id)
            .map(|(_, len)| *len)
            .unwrap_or_else(|| panic!("size for template {} not found", template_id)),
    }
}

fn expect_single_size(size: &SizeOption) -> usize {
    match size {
        SizeOption::Single(value) => *value,
        SizeOption::Multiple(values) => panic!(
            "size cannot depend on template in this context: {:?}",
            values
        ),
    }
}

// if instruction pointer is a store to the signal, return the signal index
// and the src instruction to store to the signal
#[derive(Default)]
struct BranchSignalAssign {
    if_value: Option<usize>,
    else_value: Option<usize>,
}

struct BranchVariableAssign<T: FieldOps> {
    if_value: Option<Var<T>>,
    else_value: Option<Var<T>>,
}

impl<T: FieldOps> Default for BranchVariableAssign<T> {
    fn default() -> Self {
        Self {
            if_value: None,
            else_value: None,
        }
    }
}

enum BranchStore<T: FieldOps> {
    Signal { signal_idx: usize, node_idx: usize },
    Variable { var_idx: usize, value: Var<T> },
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
    cmp: &mut ComponentInstance<T>, size: usize,
    call_stack: &Vec<String>) -> Vec<usize>
where
    T: FieldOps + 'static,
    NS: NodesStorage + 'static {

    assert!(size > 0, "size = {}", size);

    if size == 1 {
        // operator_argument_instruction implements much more cases than
        // this function, so we can use it here is size == 1
        return vec![operator_argument_instruction(
            ctx, cmp, inst, call_stack)];
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
                            calc_expression(ctx, location, cmp, call_stack)
                                .must_const_usize(ctx.nodes, call_stack);
                        let mut result = Vec::with_capacity(size);
                        for i in 0..size {
                            let idx = cmp.signal_offset + signal_idx + i;
                            if ctx.signal_node_idx[idx] == usize::MAX {
                                ctx.signal_node_idx[idx] =
                                    ctx.nodes.const_node_idx_from_value(T::zero());
                            }
                            result.push(ctx.signal_node_idx[idx]);
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
                            ctx, cmp_address, cmp, call_stack)
                            .must_const_usize(ctx.nodes, call_stack);

                    let (signal_idx, template_header, _) = match load_bucket.src {
                        LocationRule::Indexed {
                            ref location,
                            ref template_header,
                        } => {
                            let signal_idx = calc_expression(
                                ctx, location, cmp, call_stack);
                            let signal_idx = signal_idx.to_const_usize(ctx.nodes)
                                .unwrap_or_else(|e| panic!(
                                    "can't calculate const usize signal index: {}: {}",
                                    e, call_stack.join(" -> ")));
                            (signal_idx,
                             template_header.as_ref().unwrap_or(&"-".to_string()).clone(),
                             size)
                        }
                        LocationRule::Mapped { ref signal_code, ref indexes } => {
                            calc_mapped_signal_idx(
                                ctx, cmp, subcomponent_idx, *signal_code,
                                indexes, call_stack)
                        }
                    };
                    let signal_offset = cmp.subcomponents[subcomponent_idx]
                        .as_ref()
                        .unwrap()
                        .signal_offset;

                    if ctx.print_debug {
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
                            ctx, location, cmp, call_stack)
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

fn collect_branch_stores<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    ctx: &mut BuildCircuitContext<T, NS>,
    cmp: &mut ComponentInstance<T>,
    instructions: &[InstructionPointer],
    call_stack: &Vec<String>,
) -> Vec<BranchStore<T>> {
    let mut stores = Vec::new();
    for inst in instructions {
        match **inst {
            Instruction::Store(ref store_bucket) => {
                match store_bucket.dest_address_type {
                    AddressType::Signal => {
                        let signal_idx = match &store_bucket.dest {
                            LocationRule::Indexed { location, template_header } => {
                                if template_header.is_some() {
                                    panic!("template header is not supported for ternary signal store");
                                }
                                calc_expression(ctx, location, cmp, call_stack)
                                    .must_const_usize(ctx.nodes, call_stack)
                            }
                            _ => panic!("mapped destination is not supported for ternary signal store"),
                        };
                        let dest_size = resolve_size_for_template(
                            &store_bucket.context.size, cmp.template_id);
                        let node_idxs = operator_argument_instruction_n(
                            ctx, &store_bucket.src, cmp, dest_size,
                            call_stack);
                        let base_idx = cmp.signal_offset + signal_idx;
                        for (offset, node_idx) in node_idxs.into_iter().enumerate() {
                            stores.push(BranchStore::Signal {
                                signal_idx: base_idx + offset,
                                node_idx,
                            });
                        }
                    }
                    AddressType::Variable => {
                        let (location, template_header) = match &store_bucket.dest {
                            LocationRule::Indexed { location, template_header } => {
                                (location, template_header)
                            }
                            _ => panic!("mapped destination is not supported for ternary variable store"),
                        };
                        if template_header.is_some() {
                            panic!("template header is not supported for ternary variable store");
                        }
                        let var_idx =
                            calc_expression(ctx, location, cmp, call_stack)
                                .must_const_usize(ctx.nodes, call_stack);
                        let dest_size = resolve_size_for_template(
                            &store_bucket.context.size, cmp.template_id);
                        let values = calc_expression_n(
                            ctx, &store_bucket.src, cmp, dest_size,
                            call_stack);
                        for (offset, value) in values.into_iter().enumerate() {
                            stores.push(BranchStore::Variable {
                                var_idx: var_idx + offset,
                                value,
                            });
                        }
                    }
                    _ => {
                        panic!(
                            "unsupported store destination in ternary branch: {}",
                            store_bucket.dest_address_type.to_string());
                    }
                }
            }
            _ => {
                match **inst {
                    Instruction::Branch(ref branch_bucket) => {
                        let nested = collect_branch_stores_from_branch(
                            ctx, cmp, branch_bucket, call_stack);
                        stores.extend(nested);
                    }
                    Instruction::Assert(_) | Instruction::Log(_) => {}
                    _ => panic!(
                        "unsupported instruction in non-constant branch: {}: {}",
                        inst.to_string(), call_stack.join(" -> ")),
                }
            }
        }
    }
    stores
}

fn stores_to_maps<T: FieldOps>(
    stores: Vec<BranchStore<T>>,
) -> (BTreeMap<usize, usize>, BTreeMap<usize, Var<T>>) {
    let mut signal_map = BTreeMap::new();
    let mut var_map = BTreeMap::new();
    for store in stores {
        match store {
            BranchStore::Signal { signal_idx, node_idx } => {
                signal_map.insert(signal_idx, node_idx);
            }
            BranchStore::Variable { var_idx, value } => {
                var_map.insert(var_idx, value);
            }
        }
    }
    (signal_map, var_map)
}

fn collect_branch_stores_from_branch<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    ctx: &mut BuildCircuitContext<T, NS>,
    cmp: &mut ComponentInstance<T>,
    branch_bucket: &BranchBucket,
    call_stack: &Vec<String>,
) -> Vec<BranchStore<T>> {
    let cond = calc_expression(
        ctx, &branch_bucket.cond, cmp, call_stack);
    match cond.to_const(ctx.nodes) {
        Ok(cond_val) => {
            let branch = if cond_val.is_zero() {
                &branch_bucket.else_branch
            } else {
                &branch_bucket.if_branch
            };
            collect_branch_stores(ctx, cmp, branch, call_stack)
        }
        Err(NodeConstErr::InputSignal) => {
            let cond_node_idx = if let Var::Node(node_idx) = cond {
                node_idx
            } else {
                panic!("expected node for ternary condition");
            };
            let if_stores = collect_branch_stores(
                ctx, cmp, &branch_bucket.if_branch, call_stack);
            let else_stores = if branch_bucket.else_branch.is_empty() {
                Vec::new()
            } else {
                collect_branch_stores(
                    ctx, cmp, &branch_bucket.else_branch, call_stack)
            };
            let (if_signals, if_vars) = stores_to_maps(if_stores);
            let (else_signals, else_vars) = stores_to_maps(else_stores);
            let mut result = Vec::new();
            let mut signal_keys: BTreeSet<usize> =
                if_signals.keys().copied().collect();
            signal_keys.extend(else_signals.keys().copied());
            for signal_idx in signal_keys {
                let if_value = if_signals.get(&signal_idx)
                    .copied()
                    .unwrap_or_else(|| ctx.nodes.const_node_idx_from_value(T::zero()));
                let else_value = if let Some(val) = else_signals.get(&signal_idx) {
                    *val
                } else {
                    ctx.nodes.const_node_idx_from_value(T::zero())
                };
                let final_value = if if_value == else_value {
                    if_value
                } else {
                    ctx.nodes.push(Node::TresOp(
                        TresOperation::TernCond, cond_node_idx,
                        if_value, else_value)).0
                };
                result.push(BranchStore::Signal {
                    signal_idx,
                    node_idx: final_value,
                });
            }
            let mut var_keys: BTreeSet<usize> =
                if_vars.keys().copied().collect();
            var_keys.extend(else_vars.keys().copied());
            for var_idx in var_keys {
                let base_value = cmp.vars[var_idx].as_ref().unwrap_or_else(|| panic!(
                    "variable #{} is not set before ternary operation: {}",
                    var_idx, call_stack.join(" -> "))).clone();
                let if_value = if if_vars.contains_key(&var_idx) {
                    if_vars.get(&var_idx).unwrap().clone()
                } else {
                    base_value.clone()
                };
                let else_value = if else_vars.contains_key(&var_idx) {
                    else_vars.get(&var_idx).unwrap().clone()
                } else {
                    base_value.clone()
                };
                let final_value = {
                    let if_node = node_from_var(&if_value, ctx.nodes);
                    let else_node = node_from_var(&else_value, ctx.nodes);
                    if if_node == else_node {
                        if_value
                    } else {
                        let node_idx = ctx.nodes.push(Node::TresOp(
                            TresOperation::TernCond, cond_node_idx,
                            if_node, else_node)).0;
                        Var::Node(node_idx)
                    }
                };
                result.push(BranchStore::Variable {
                    var_idx,
                    value: final_value,
                });
            }
            result
        }
        Err(e) => {
            panic!("unexpected condition error: {}: {}", e, call_stack.join(" -> "));
        }
    }
}


fn operator_argument_instruction<T, NS>(
    ctx: &mut BuildCircuitContext<T, NS>, cmp: &mut ComponentInstance<T>,
    inst: &InstructionPointer,
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
                    calc_expression(ctx, location, cmp, call_stack)
                        .must_const_usize(ctx.nodes, call_stack);
                let signal_idx = cmp.signal_offset + signal_idx;
                if ctx.signal_node_idx[signal_idx] == usize::MAX {
                    let zero_node = ctx.nodes.const_node_idx_from_value(T::zero());
                    ctx.signal_node_idx[signal_idx] = zero_node;
                }
                ctx.signal_node_idx[signal_idx]
            }
                    LocationRule::Mapped { .. } => {
                        todo!()
                    }
                },
                AddressType::SubcmpSignal {
                    ref cmp_address, ..
                } => {
                    let subcomponent_idx =
                        calc_expression(ctx, cmp_address, cmp, call_stack)
                            .must_const_usize(ctx.nodes, call_stack);

                    let (signal_idx, template_header, _) = match load_bucket.src {
                        LocationRule::Indexed {
                            ref location,
                            ref template_header,
                        } => {
                            let signal_idx =
                                calc_expression(ctx, location, cmp, call_stack)
                                    .must_const_usize(ctx.nodes, call_stack);
                            (signal_idx,
                             template_header.as_ref().unwrap_or(&"-".to_string()).clone(),
                             1)
                        }
                        LocationRule::Mapped { ref signal_code, ref indexes } => {
                            calc_mapped_signal_idx(
                                ctx, cmp, subcomponent_idx, *signal_code,
                                indexes, call_stack)
                        }
                    };

                    let signal_offset = cmp.subcomponents[subcomponent_idx]
                        .as_ref().unwrap().signal_offset;

                    if ctx.print_debug {
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
                                calc_expression(ctx, location, cmp, call_stack)
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
                ctx, cmp, compute_bucket, call_stack);
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
    call_stack: &Vec<String>,
) -> Node {
    if let Ok(op) = try_into_operation(compute_bucket.op.clone()) {
        let arg1 = operator_argument_instruction(
            ctx, cmp, &compute_bucket.stack[0], call_stack);
        let arg2 = operator_argument_instruction(
            ctx, cmp, &compute_bucket.stack[1], call_stack);
        Node::Op(op, arg1, arg2)
    } else if let Ok(op) = try_into_uno_operation(compute_bucket.op.clone()) {
        let arg1 = operator_argument_instruction(
            ctx, cmp, &compute_bucket.stack[0], call_stack);
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
    indexes: &[AccessType],
    call_stack: &Vec<String>) -> (usize, String, usize) {

    let template_id = cmp.subcomponents[subcomponent_idx]
        .as_ref()
        .unwrap()
        .template_id;
    let signals = ctx.io_map.get(&template_id).unwrap();
    let template_def = format!("<template id: {}>", template_id);
    let def: &IODef = &signals[signal_code];
    let mut map_access = def.offset;
    let mut current_lengths = def.lengths.clone();
    let mut current_bus_id = def.bus_id;

    for access in indexes {
        match access {
            AccessType::Indexed(index_info) => {
                if index_info.indexes.is_empty() {
                    continue;
                }
                assert!(
                    !current_lengths.is_empty(),
                    "indexed access on scalar signal: {}",
                    call_stack.join(" -> ")
                );
                let index_count = index_info.indexes.len();
                assert!(
                    index_count <= current_lengths.len(),
                    "number of indexes ({}) exceeds number of dimensions ({})",
                    index_count,
                    current_lengths.len()
                );

                let mut strides = vec![1usize; current_lengths.len()];
                if current_lengths.len() > 1 {
                    for i in (0..current_lengths.len() - 1).rev() {
                        strides[i] = strides[i + 1] * current_lengths[i + 1];
                    }
                }

                for i in 0..index_count {
                    let idx_ip = &index_info.indexes[i];
                    let idx_value = calc_expression(
                        ctx, idx_ip, cmp, call_stack);
                    let idx_value = idx_value.must_const_usize(
                        ctx.nodes, call_stack);
                    assert!(
                        idx_value < current_lengths[i],
                        "index {} >= dimension size {}",
                        idx_value,
                        current_lengths[i]
                    );
                    map_access += idx_value * strides[i];
                }

                current_lengths = current_lengths[index_count..].to_vec();
            }
            AccessType::Qualified(field_id) => {
                let bus_id = current_bus_id.expect("bus field access requires bus id");
                let field = ctx.bus_field_info.get(bus_id)
                    .unwrap_or_else(|| panic!("invalid bus id {}", bus_id))
                    .get(*field_id)
                    .unwrap_or_else(|| panic!("invalid field id {} for bus {}", field_id, bus_id))
                    .clone();
                map_access += field.offset;
                current_lengths = field.dimensions.clone();
                current_bus_id = field.bus_id;
            }
        }
    }

    let total_len = if current_lengths.is_empty() {
        1
    } else {
        current_lengths.iter().product()
    };

    (map_access, template_def, total_len)
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
    bus_field_info: &'a FieldMap,
    stack: CallStack,
    print_debug: bool,
}

impl<T, NS> BuildCircuitContext<'_, T, NS>
where
    T: FieldOps + 'static,
    NS: NodesStorage + 'static {

    fn new<'a>(
        nodes: &'a mut Nodes<T, NS>,
        signal_node_idx: &'a mut Vec<usize>,
        templates: &'a Vec<TemplateCode>,
        functions: &'a Vec<FunctionCode>,
        io_map: &'a TemplateInstanceIOMap,
        bus_field_info: &'a FieldMap,
        print_debug: bool,
    ) -> BuildCircuitContext<'a, T, NS> {

        let signal_nodes_num = signal_node_idx.len();

        BuildCircuitContext {
            nodes,
            signal_node_idx,
            signals_set: 0,
            progress_bar: progress_bar(signal_nodes_num),
            templates,
            functions,
            io_map,
            bus_field_info,
            stack: CallStack::new(),
            print_debug,
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
        if self.signal_node_idx[signal_idx] == usize::MAX {
            self.signal_node_idx[signal_idx] = node_idx;
            self.signals_set += 1;
            self.update_progress_message();
            self.progress_bar.inc(1);
        } else {
            // already set; assume equality constraint ensures consistency
        }
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
    ctx: &mut BuildCircuitContext<T, NS>,
    inst: &InstructionPointer,
    call_stack: &Vec<String>,
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
                                    ctx, location, cmp, call_stack)
                                    .must_const_usize(ctx.nodes, call_stack);

                            if ctx.print_debug {
                                println!(
                                    "Store signal at offset {} + {} = {}",
                                    cmp.signal_offset, signal_idx,
                                    cmp.signal_offset + signal_idx);
                            }
                            let signal_idx =
                                cmp.signal_offset + signal_idx;

                            let dest_size = resolve_size_for_template(
                                &store_bucket.context.size, cmp.template_id);
                            let node_idxs = operator_argument_instruction_n(
                                ctx, &store_bucket.src, cmp,
                                dest_size, call_stack);

                            assert_eq!(node_idxs.len(), dest_size);

                            for (i, item) in node_idxs.iter()
                                .enumerate().take(dest_size) {

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
                                calc_expression(ctx, location, cmp, call_stack)
                                    .must_const_usize(ctx.nodes, call_stack);
                            let dest_size = resolve_size_for_template(
                                &store_bucket.context.size, cmp.template_id);
                            let var_exprs = calc_expression_n(
                                ctx, &store_bucket.src, cmp,
                                dest_size, call_stack);

                            for (i, item) in var_exprs.iter()
                                .enumerate().take(dest_size) {

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
                    let subcomponent_idx =
                        calc_expression(ctx, cmp_address, cmp, call_stack)
                            .must_const_usize(ctx.nodes, call_stack);
                    let sub_cmp = cmp.subcomponents[subcomponent_idx]
                        .as_ref()
                        .unwrap();
                    let dest_size = resolve_size_for_template(
                        &store_bucket.context.size, sub_cmp.template_id);
                    let node_idxs = operator_argument_instruction_n(
                        ctx, &store_bucket.src, cmp, dest_size, call_stack);
                    assert_eq!(node_idxs.len(), dest_size);

                    let dest = SignalDestination {
                        subcomponent_idx,
                        input_information,
                        dest: &store_bucket.dest,
                    };

                    store_subcomponent_signals(
                        ctx, &node_idxs, call_stack, cmp, &dest);
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
                let arg_size = resolve_size_for_template(
                    &call_bucket.argument_types[idx].size, cmp.template_id);
                let args = calc_expression_n(
                    ctx, inst2, cmp, arg_size, call_stack);
                for arg in args {
                    fn_vars[count] = Some(arg);
                    count += 1;
                }
            }

            let r = run_function(
                ctx, &call_bucket.symbol, &mut fn_vars, call_stack);

            match call_bucket.return_info {
                ReturnType::Intermediate{ ..} => { todo!(); }
                ReturnType::Final( ref final_data ) => {
                    store_function_return_results(
                        ctx, final_data, &fn_vars, &r, call_stack,
                        cmp);
                }
            }
        }
        Instruction::Branch(ref branch_bucket) => {
            let cond = calc_expression(
                ctx, &branch_bucket.cond, cmp, call_stack);
            match cond.to_const(ctx.nodes) {
                Ok(cond_val) => {
                    let inst_list = if cond_val.is_zero() {
                        &branch_bucket.else_branch
                    } else {
                        &branch_bucket.if_branch
                    };
                    for inst in inst_list {
                        process_instruction(ctx, inst, call_stack, cmp);
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
                    if ctx.print_debug {
                        println!(
                            "dynamic branch at [{}]: IF len {}, ELSE len {}",
                            call_stack.join(" -> "),
                            branch_bucket.if_branch.len(),
                            branch_bucket.else_branch.len());
                        println!("  IF[0]: {}", branch_bucket.if_branch[0].to_string());
                        if !branch_bucket.else_branch.is_empty() {
                            println!("  ELSE[0]: {}", branch_bucket.else_branch[0].to_string());
                        }
                    }

                    let cond_node_idx = node_idx;
                    let if_stores = collect_branch_stores(
                        ctx, cmp, &branch_bucket.if_branch, call_stack);
                    let else_stores = if branch_bucket.else_branch.is_empty() {
                        Vec::new()
                    } else {
                        collect_branch_stores(
                            ctx, cmp, &branch_bucket.else_branch, call_stack)
                    };

                    let mut signal_assigns: BTreeMap<usize, BranchSignalAssign> = BTreeMap::new();
                    let mut var_assigns: BTreeMap<usize, BranchVariableAssign<T>> = BTreeMap::new();

                    for store in if_stores {
                        match store {
                            BranchStore::Signal { signal_idx, node_idx } => {
                                signal_assigns.entry(signal_idx)
                                    .or_default()
                                    .if_value = Some(node_idx);
                            }
                            BranchStore::Variable { var_idx, value } => {
                                var_assigns.entry(var_idx)
                                    .or_default()
                                    .if_value = Some(value);
                            }
                        }
                    }
                    for store in else_stores {
                        match store {
                            BranchStore::Signal { signal_idx, node_idx } => {
                                signal_assigns.entry(signal_idx)
                                    .or_default()
                                    .else_value = Some(node_idx);
                            }
                            BranchStore::Variable { var_idx, value } => {
                                var_assigns.entry(var_idx)
                                    .or_default()
                                    .else_value = Some(value);
                            }
                        }
                    }

                    for (signal_idx, assign) in signal_assigns {
                        if ctx.signal_node_idx[signal_idx] != usize::MAX {
                            panic!(
                                "signal #{} is already set before ternary operation: {}",
                                signal_idx, call_stack.join(" -> "));
                        }
                        let base_node = ctx.nodes.const_node_idx_from_value(T::zero());
                        let if_node = assign.if_value.unwrap_or(base_node);
                        let else_node = assign.else_value.unwrap_or(base_node);
                        let result_node = if if_node == else_node {
                            if_node
                        } else {
                            ctx.nodes.push(Node::TresOp(
                                TresOperation::TernCond, cond_node_idx, if_node, else_node)).0
                        };
                        ctx.associate_signal_to_node(signal_idx, result_node);
                    }

                    for (var_idx, assign) in var_assigns {
                        let prev = cmp.vars[var_idx].as_ref().unwrap_or_else(|| panic!(
                            "variable #{} is not set before ternary operation: {}",
                            var_idx, call_stack.join(" -> "))).clone();
                        let if_var = assign.if_value.unwrap_or(prev.clone());
                        let else_var = assign.else_value.unwrap_or(prev.clone());
                        let if_node_idx = node_from_var(&if_var, ctx.nodes);
                        let else_node_idx = node_from_var(&else_var, ctx.nodes);
                        let new_var = if if_node_idx == else_node_idx {
                            if_var
                        } else {
                            let node_idx = ctx.nodes.push(Node::TresOp(
                                TresOperation::TernCond, cond_node_idx,
                                if_node_idx, else_node_idx)).0;
                            Var::Node(node_idx)
                        };
                        cmp.vars[var_idx] = Some(new_var);
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
                ctx, cmp, &loop_bucket.continue_condition, call_stack) {

                for i in &loop_bucket.body {
                    process_instruction(ctx, i, call_stack, cmp);
                }
            }
        }
        Instruction::CreateCmp(ref create_component_bucket) => {
            let sub_cmp_idx =
                calc_expression(
                    ctx, &create_component_bucket.sub_cmp_id, cmp, call_stack)
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

            if ctx.print_debug {
                println!(
                    "{}",
                    fmt_create_cmp_bucket(
                        ctx, create_component_bucket, cmp, call_stack));
            }
            if !create_component_bucket.has_inputs {
                for i in sub_cmp_idx..sub_cmp_idx + create_component_bucket.number_of_cmp {
                    let cmp = cmp.subcomponents[i].as_mut().unwrap();
                    run_template(ctx, call_stack, cmp)
                }
            }
        }
    }
}

fn store_function_return_results_into_variable<T, NS>(
    final_data: &FinalData, dest_size: usize, src_vars: &[Option<Var<T>>],
    ret: &FnReturn<T>, dst_vars: &mut Vec<Option<Var<T>>>,
    nodes: &mut Nodes<T, NS>, call_stack: &Vec<String>)
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
                FnReturn::FnVar { idx, ln } => {
                    assert_eq!(
                        dest_size, *ln,
                        "function returned {} values but destination expects {}",
                        ln, dest_size);
                    for i in 0..dest_size {
                        let v = if let Some(v) = &src_vars[idx + i] {
                            v
                        } else {
                            panic!("return value is not set {} / {}", idx, i)
                        };
                        dst_vars[lvar_idx + i] = Some(v.clone());
                    }

                }
                FnReturn::Value(v) => {
                    assert_eq!(dest_size, 1);
                    dst_vars[lvar_idx] = Some(v.clone());
                }
            }
        }
        LocationRule::Mapped { .. } => { todo!() }
    }
}

fn store_function_return_results_into_subsignal<T, NS>(
    ctx: &mut BuildCircuitContext<T, NS>,
    final_data: &FinalData,
    subcomponent_idx: usize,
    src_vars: &[Option<Var<T>>],
    ret: &FnReturn<T>,
    call_stack: &Vec<String>,
    cmp: &mut ComponentInstance<T>)
where
    T: FieldOps + 'static,
    NS: NodesStorage + 'static {

    let sub_cmp = cmp.subcomponents[subcomponent_idx]
        .as_ref()
        .unwrap();
    let dest_size = resolve_size_for_template(
        &final_data.context.size, sub_cmp.template_id);

    let input_information = if let AddressType::SubcmpSignal { input_information, .. } = &final_data.dest_address_type {
        input_information
    } else {
        panic!("expected SubcmpSignal destination address type");
    };

    let mut src_node_idxs: Vec<usize> = Vec::new();
    match ret {
        FnReturn::FnVar { idx, ln } => {
            assert_eq!(
                dest_size, *ln,
                "function returned {} values but destination expects {}",
                ln, dest_size);
            for i in 0..dest_size {
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
            assert_eq!(dest_size, 1);
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
        subcomponent_idx,
        input_information,
        dest: &final_data.dest,
    };

    store_subcomponent_signals(
        ctx, &src_node_idxs, call_stack, cmp, &dest);
}

fn store_function_return_results<T, NS> (
    ctx: &mut BuildCircuitContext<T, NS>, final_data: &FinalData,
    src_vars: &[Option<Var<T>>], ret: &FnReturn<T>,
    call_stack: &Vec<String>, cmp: &mut ComponentInstance<T>)
where
    T: FieldOps + 'static,
    NS: NodesStorage + 'static {

    match &final_data.dest_address_type {
        AddressType::Signal => todo!("Signal"),
        AddressType::Variable => {
            let dest_size = resolve_size_for_template(
                &final_data.context.size, cmp.template_id);
            store_function_return_results_into_variable(
                final_data, dest_size, src_vars, ret, &mut cmp.vars, ctx.nodes,
                call_stack);
        }
        AddressType::SubcmpSignal { cmp_address, .. } => {
            let subcomponent_idx =
                calc_expression(ctx, cmp_address, cmp, call_stack)
                    .must_const_usize(ctx.nodes, call_stack);
            store_function_return_results_into_subsignal(
                ctx, final_data, subcomponent_idx, src_vars, ret,
                call_stack, cmp);
        }
    }
}

fn run_function<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    ctx: &mut BuildCircuitContext<T, NS>, fn_name: &str,
    fn_vars: &mut Vec<Option<Var<T>>>,
    call_stack: &[String]) -> FnReturn<T> {

    if fn_name.starts_with("sqrt") {
        return run_builtin_sqrt(ctx, fn_vars, call_stack);
    }

    // for i in functions {
    //     println!("Function: {} {}", i.header, i.name);
    // }

    let f = find_function(fn_name, ctx.functions);
    let mut start: Option<Instant> = None;
    if ctx.print_debug {
        start = Some(Instant::now());
        println!(
            "Run function {}, vars num: {}, call stack: {}", fn_name,
            fn_vars.len(), call_stack.join(" -> "));
    }

    let mut call_stack = call_stack.to_owned();
    call_stack.push(f.name.clone());

    ctx.push_stack(fn_name.to_string());

    let mut r: Option<FnReturn<T>> = None;
    let mut pending_returns: Vec<(usize, Var<T>)> = Vec::new();
    for i in &f.body {
        r = process_function_instruction(
            ctx, i, fn_vars, &call_stack, &mut pending_returns);
        if r.is_some() {
            break;
        }
    }
    // println!("{}", f.to_string());

    ctx.pop_stack();

    let r = r.expect("no return found");
    let r = finalize_function_return(
        r, &mut pending_returns, fn_vars, ctx.nodes, &call_stack);
    if ctx.print_debug {
        println!("Function {} done in {:?}", fn_name, start.unwrap().elapsed());
    }
    r
}

fn run_builtin_sqrt<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    ctx: &mut BuildCircuitContext<T, NS>, fn_vars: &mut [Option<Var<T>>],
    call_stack: &[String]) -> FnReturn<T> {

    assert!(
        !fn_vars.is_empty(),
        "sqrt function expects at least one argument: {}",
        call_stack.join(" -> "));
    let arg = fn_vars[0].as_ref().unwrap_or_else(|| {
        panic!("sqrt argument is not set: {}", call_stack.join(" -> "));
    }).clone();

    let arg_idx = node_from_var(&arg, ctx.nodes);
    let node_idx = ctx.nodes.push(Node::UnoOp(UnoOperation::Sqrt, arg_idx)).0;
    FnReturn::Value(Var::Node(node_idx))
}

fn finalize_function_return<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    ret: FnReturn<T>, pending_returns: &mut Vec<(usize, Var<T>)>,
    fn_vars: &[Option<Var<T>>], nodes: &mut Nodes<T, NS>,
    call_stack: &[String]) -> FnReturn<T> {

    if pending_returns.is_empty() {
        return ret;
    }

    let mut ret_var = fn_return_to_var(ret, fn_vars, call_stack);
    while let Some((cond_idx, if_var)) = pending_returns.pop() {
        let if_idx = node_from_var(&if_var, nodes);
        let else_idx = node_from_var(&ret_var, nodes);
        let node_idx = nodes.push(Node::TresOp(
            TresOperation::TernCond, cond_idx, if_idx, else_idx)).0;
        ret_var = Var::Node(node_idx);
    }
    FnReturn::Value(ret_var)
}

fn fn_return_to_var<T: FieldOps + 'static>(
    ret: FnReturn<T>, fn_vars: &[Option<Var<T>>],
    call_stack: &[String]) -> Var<T> {

    match ret {
        FnReturn::Value(v) => v,
        FnReturn::FnVar { idx, ln } => {
            assert_eq!(
                ln, 1,
                "multi-value returns are not supported in conditional return handling: {}",
                call_stack.join(" -> "));
            fn_vars[idx].as_ref().unwrap_or_else(|| {
                panic!(
                    "return variable is not set: {}/{}",
                    idx, call_stack.join(" -> "));
            }).clone()
        }
    }
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

    if let Ok(op) = try_into_operation(compute_bucket.op.clone()) {
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
    } else if let Ok(op) = try_into_uno_operation(compute_bucket.op.clone()) {
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


// Reconciles a dynamic (signal-dependent) if/else's two independently-run
// variable states back into fn_vars, one slot at a time: where a variable's
// value is the same on both sides, keep it as-is; where the branches
// disagree, build a TernCond over the two values, falling back to the
// pre-branch value for whichever side never touched that variable at all
// (mirrors collect_branch_stores_from_branch's template-level equivalent).
fn merge_ternary_fn_vars<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    ctx: &mut BuildCircuitContext<T, NS>, cond_node_idx: usize,
    vars_before: &[Option<Var<T>>], vars_after_if: &[Option<Var<T>>],
    vars_after_else: &[Option<Var<T>>], fn_vars: &mut [Option<Var<T>>],
    call_stack: &Vec<String>) {

    for idx in 0..fn_vars.len() {
        let if_val = vars_after_if[idx].clone().or_else(|| vars_before[idx].clone());
        let else_val = vars_after_else[idx].clone().or_else(|| vars_before[idx].clone());

        fn_vars[idx] = match (if_val, else_val) {
            (None, None) => None,
            (Some(if_v), Some(else_v)) => {
                let if_node = node_from_var(&if_v, ctx.nodes);
                let else_node = node_from_var(&else_v, ctx.nodes);
                if if_node == else_node {
                    Some(if_v)
                } else {
                    let node_idx = ctx.nodes.push(Node::TresOp(
                        TresOperation::TernCond, cond_node_idx, if_node,
                        else_node)).0;
                    Some(Var::Node(node_idx))
                }
            }
            _ => panic!(
                "variable #{} is set on only one side of a dynamic \
                 (signal-dependent) if/else with no prior value to fall \
                 back on for the other side: {}",
                idx, call_stack.join(" -> ")),
        };
    }
}

fn process_function_instruction<T, NS>(
    ctx: &mut BuildCircuitContext<T, NS>, inst: &InstructionPointer,
    fn_vars: &mut Vec<Option<Var<T>>>,
    call_stack: &Vec<String>, pending_returns: &mut Vec<(usize, Var<T>)>
) -> Option<FnReturn<T>>
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
                            let dest_size =
                                expect_single_size(&store_bucket.context.size);
                            if dest_size == 1 {
                                fn_vars[lvar_idx] = Some(calc_function_expression(
                                    &store_bucket.src, fn_vars, ctx.nodes,
                                    call_stack));
                            } else {
                                let values = calc_function_expression_n(
                                    &store_bucket.src, fn_vars, ctx.nodes,
                                    dest_size, call_stack);
                                assert_eq!(values.len(), dest_size);
                                for i in 0..dest_size {
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
                            ctx, i, fn_vars, call_stack, pending_returns);
                        if r.is_some() {
                            return r;
                        }
                    }
                }
                Err(NodeConstErr::InputSignal) => { // dynamic condition expression
                    // A signal-dependent condition means both possible
                    // outcomes have to be built into the graph and combined
                    // with a TernCond, since we can't know at build time
                    // which one the real witness will take. Each branch may
                    // be an arbitrary sequence of statements (assignments,
                    // nested branches - dynamic or static, calls, etc.),
                    // not just a single instruction: run each branch for
                    // real (via the ordinary recursive interpreter, so
                    // read-after-write within a branch and nested branches
                    // both just work) against its own copy of fn_vars
                    // starting from the same pre-branch snapshot, then
                    // reconcile the two resulting states.
                    let cond_node_idx = if let Var::Node(node_idx) = cond {
                        node_idx
                    } else {
                        panic!("[assertion] expected to have a node with ternary operation here");
                    };

                    let vars_before = fn_vars.clone();

                    let mut if_ret: Option<FnReturn<T>> = None;
                    for inst in &branch_bucket.if_branch {
                        if let Some(r) = process_function_instruction(
                            ctx, inst, fn_vars, call_stack, pending_returns) {
                            if_ret = Some(r);
                            break;
                        }
                    }
                    let vars_after_if = fn_vars.clone();
                    let if_ret_var = if_ret.map(
                        |r| fn_return_to_var(r, &vars_after_if, call_stack));

                    *fn_vars = vars_before.clone();
                    let mut else_ret: Option<FnReturn<T>> = None;
                    for inst in &branch_bucket.else_branch {
                        if let Some(r) = process_function_instruction(
                            ctx, inst, fn_vars, call_stack, pending_returns) {
                            else_ret = Some(r);
                            break;
                        }
                    }
                    let vars_after_else = fn_vars.clone();
                    let else_ret_var = else_ret.map(
                        |r| fn_return_to_var(r, &vars_after_else, call_stack));

                    match (if_ret_var, else_ret_var) {
                        (Some(vif), Some(velse)) => {
                            // Both branches unconditionally return - nothing
                            // after this statement is reachable either way,
                            // so the ternary of the two return values IS
                            // this whole statement's result. Propagate it up
                            // immediately, same as the function's ordinary
                            // Instruction::Return handling.
                            let if_idx = node_from_var(&vif, ctx.nodes);
                            let else_idx = node_from_var(&velse, ctx.nodes);
                            let tern_node_idx = ctx.nodes.push(Node::TresOp(
                                TresOperation::TernCond, cond_node_idx,
                                if_idx, else_idx));
                            return Some(FnReturn::Value(
                                Var::Node(tern_node_idx.0)));
                        }
                        (Some(vif), None) => {
                            // If-branch always returns, else-branch (which
                            // may be empty) falls through. Defer the early
                            // return via pending_returns - resolved against
                            // the function's eventual real return value in
                            // finalize_function_return - and continue with
                            // the else-branch's variable state, since that's
                            // what's actually reachable when cond is false.
                            pending_returns.push((cond_node_idx, vif));
                            *fn_vars = vars_after_else;
                        }
                        (None, Some(velse)) => {
                            // Symmetric case: pending_returns is always
                            // "cond true -> early return", so negate the
                            // condition for an else-only return.
                            let not_cond_idx = ctx.nodes.push(
                                Node::UnoOp(UnoOperation::Lnot, cond_node_idx)).0;
                            pending_returns.push((not_cond_idx, velse));
                            *fn_vars = vars_after_if;
                        }
                        (None, None) => {
                            // Neither branch returns - merge local variable
                            // state var-by-var. fn_vars currently holds the
                            // else-branch's post-state (from the reset+run
                            // above); merge_ternary_fn_vars overwrites it in
                            // place with the reconciled result.
                            merge_ternary_fn_vars(
                                ctx, cond_node_idx, &vars_before,
                                &vars_after_if, &vars_after_else, fn_vars,
                                call_stack);
                        }
                    }
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
                        ctx, i, fn_vars, call_stack, pending_returns);
                }
            };
            None
        }
        Instruction::Call(ref call_bucket) => {
            let mut new_fn_vars: Vec<Option<Var<T>>> = vec![None; call_bucket.arena_size];

            let mut count: usize = 0;
            for (idx, inst2) in call_bucket.arguments.iter().enumerate() {
                let arg_size =
                    expect_single_size(&call_bucket.argument_types[idx].size);
                let args = calc_function_expression_n(
                    inst2, fn_vars, ctx.nodes,
                    arg_size, call_stack);
                for arg in args {
                    new_fn_vars[count] = Some(arg);
                    count += 1;
                }
            }

            let r = run_function(
                ctx, &call_bucket.symbol, &mut new_fn_vars, call_stack);

            match call_bucket.return_info {
                ReturnType::Intermediate{ ..} => { todo!(); }
                ReturnType::Final( ref final_data ) => {
                    let dest_size = expect_single_size(&final_data.context.size);
                    store_function_return_results_into_variable(
                        final_data, dest_size, &new_fn_vars, &r, fn_vars, ctx.nodes,
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
    call_stack: &Vec<String>,
) -> String {
    let sub_cmp_id = calc_expression(
        ctx, &cmp_bucket.sub_cmp_id, cmp, call_stack);

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
    cmp: &mut ComponentInstance<T>, size: usize,
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
                    ctx, location, cmp, call_stack);
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
                calc_expression(ctx, cmp_address, cmp, call_stack)
                    .must_const_usize(ctx.nodes, call_stack);

            let (signal_idx, template_header, available_len) = match load_bucket.src {
                LocationRule::Indexed {
                    ref location,
                    ref template_header,
                } => {
                    let signal_idx = calc_expression(
                        ctx, location, cmp, call_stack);
                    let signal_idx = signal_idx.to_const_usize(ctx.nodes)
                        .unwrap_or_else(|e| panic!(
                            "can't calculate signal index: {}: {}",
                            e, call_stack.join(" -> ")));
                    (signal_idx,
                     template_header.as_ref().unwrap_or(&"-".to_string()).clone(),
                     size)
                }
                LocationRule::Mapped { ref signal_code, ref indexes } => {
                    calc_mapped_signal_idx(
                        ctx, cmp, subcomponent_idx, *signal_code, indexes,
                        call_stack)
                }
            };
            let signal_offset = cmp.subcomponents[subcomponent_idx]
                .as_ref().unwrap().signal_offset;

            if ctx.print_debug {
                let location_rule = match load_bucket.src {
                    LocationRule::Indexed { .. } => "Indexed",
                    LocationRule::Mapped { .. } => "Mapped",
                };
                println!(
                    "Load subcomponent signal (location: {}, template: {}, subcomponent idx: {}, size: {}): {} + {} = {}",
                    location_rule, template_header, subcomponent_idx, size,
                    signal_offset, signal_idx, signal_offset + signal_idx);
            }

            let base_idx = signal_offset + signal_idx;
            let mut result = Vec::with_capacity(size);
            for i in 0..size {
                let node_idx = if i < available_len {
                    let idx = base_idx + i;
                    let signal_node = ctx.signal_node_idx[idx];
                    if signal_node == usize::MAX {
                        ctx.nodes.const_node_idx_from_value(T::zero())
                    } else {
                        signal_node
                    }
                } else {
                    ctx.nodes.const_node_idx_from_value(T::zero())
                };
                result.push(Var::Node(node_idx));
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
                calc_expression(ctx, location, cmp, call_stack)
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
    call_stack: &Vec<String>,
) -> Var<T> {
    assert_eq!(stack.len(), 1);
    let a = calc_expression(ctx, &stack[0], cmp, call_stack);

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
    call_stack: &Vec<String>,
) -> Var<T> {
    assert_eq!(stack.len(), 2);
    let a = calc_expression(ctx, &stack[0], cmp, call_stack);
    let b = calc_expression(ctx, &stack[1], cmp, call_stack);

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
                ctx, load_bucket, cmp, 1, call_stack);
            assert_eq!(r.len(), 1);
            r[0].clone()
        },
        Instruction::Compute(ref compute_bucket) => {
            // try duo operation
            if let Ok(op) = try_into_operation(compute_bucket.op.clone()) {
                build_binary_op_var(
                    ctx, cmp, op, &compute_bucket.stack, call_stack)
            }

            // try uno operation
            else if let Ok(op) = try_into_uno_operation(compute_bucket.op.clone()) {
                build_unary_op_var(
                    ctx, cmp, op, &compute_bucket.stack, call_stack)
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
    call_stack: &Vec<String>,
) -> Vec<Var<T>> {
    if size == 1 {
        return vec![calc_expression(ctx, inst, cmp, call_stack)];
    }

    match **inst {
        Instruction::Load(ref load_bucket) => {
            load_n(ctx, load_bucket, cmp, size, call_stack)
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
    call_stack: &Vec<String>,
) -> bool {
    let val = calc_expression(ctx, inst, cmp, call_stack)
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

fn build_types_list(buses: &[BusInstance]) -> Vec<vm2::Type> {
    let mut types: Vec<vm2::Type> = Vec::new();

    for (idx, b) in buses.iter().enumerate() {
        let mut bus_fields: Vec<(&String, &FieldInfo)> = b.fields.iter()
            .collect();
        // sort fields by field ID
        bus_fields.sort_by_key(|x| x.1.field_id);
        let fields: Vec<TypeField> = bus_fields.into_iter().map(|(field_name, field)| TypeField {
            name: field_name.clone(),
            kind: match field.bus_id {
                None => TypeFieldKind::Ff,
                Some(id) => TypeFieldKind::Bus(id),
            },
            offset: field.offset,
            base_type_size: field.size / field.dimensions.iter().product::<usize>(),
            dims: field.dimensions.clone(),
        }).collect();
        types.push(vm2::Type{ name: format!("{}_{}", b.name.clone(), idx), fields });
    }

    types
}

fn setup_input_nodes<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    nodes: &mut Nodes<T, NS>,
    signal_node_idx: &mut [usize],
    inputs_num: usize,
    inputs_offset: usize,
) {

    let mut signal_values: Vec<T> = Vec::with_capacity(inputs_num + 1);
    signal_values.push(T::one());
    signal_node_idx[0] = nodes.push(Node::Input(signal_values.len() - 1)).0;

    for item in signal_node_idx.iter_mut()
        .skip(inputs_offset)
        .take(inputs_num) {

        signal_values.push(T::zero());
        *item = nodes.push(Node::Input(signal_values.len()-1)).0
    }
}

fn run_template<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    ctx: &mut BuildCircuitContext<T, NS>, call_stack: &[String],
    cmp: &mut ComponentInstance<T>) {

    let tmpl = &ctx.templates[cmp.template_id];

    let tmpl_name: String = format!("{}_{}", tmpl.name, tmpl.id);
    let mut call_stack = call_stack.to_owned();
    call_stack.push(tmpl_name.clone());

    ctx.push_stack(tmpl_name);
    if ctx.print_debug {
        println!(
            "Run template {}_{}: body length: {}", tmpl.name, tmpl.id,
            tmpl.body.len());
    }

    for inst in &tmpl.body {
        process_instruction(ctx, inst, &call_stack, cmp);
    }

    ctx.pop_stack();

    if ctx.print_debug {
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
    optimization_level: Option<OptimizationLevel>,
    named_temp: bool,
    temp_dir: PathBuf,
    use_old_simplification_heuristics: bool,
}

fn parse_args() -> Args {
    let args: Vec<String> = env::args().collect();
    let mut i = 1;
    let mut circuit_file: Option<String> = None;
    let mut graph_file: Option<String> = None;
    let mut link_libraries: Vec<PathBuf> = Vec::new();
    let mut inputs_file: Option<String> = None;
    let mut print_debug = false;
    let mut r1cs_file: Option<String> = None;
    let mut prime: Option<Prime> = None;
    let mut optimization_level: Option<OptimizationLevel> = None;
    let mut named_temp: bool = false;
    let mut temp_dir: Option<PathBuf> = None;
    let mut use_old_simplification_heuristics: bool = false;

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
        eprintln!("    -i <inputs_file.json>   Path to the inputs file. If provided, the inputs will be used");
        eprintln!("                            to generate the witness. Otherwise, inputs will be set to 0");
        eprintln!("    -p <prime>              The prime field to use. Valid options are 'bn128' (default),");
        eprintln!("                            'goldilocks', and 'grumpkin'");
        eprintln!("    --r1cs <r1cs_file>      Path to the R1CS file. If provided, the R1CS file will be");
        eprintln!("                            saved along with the generated graph");
        eprintln!("    --O0, --O1, --O2        Optimization level for circom. Default is --O2");
        eprintln!("    --use_old_simplification_heuristics");
        eprintln!("                            Use old simplification heuristics from circom < 2.0.9");
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
        } else if args[i] == "-i" {
            i += 1;
            if i >= args.len() {
                usage("missing argument for -i");
            }
            if inputs_file.is_none() {
                inputs_file = Some(args[i].clone());
            } else {
                usage("multiple inputs files");
            }
        } else if args[i].starts_with("-i") {
            if inputs_file.is_none() {
                inputs_file = Some(args[i][2..].to_string());
            } else {
                usage("multiple inputs files");
            }
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
        } else if args[i] == "--use_old_simplification_heuristics" {
            use_old_simplification_heuristics = true;
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
        optimization_level,
        named_temp,
        temp_dir: temp_dir.unwrap_or_else(env::temp_dir),
        use_old_simplification_heuristics,
    }
}

fn main() {
    let args = parse_args();

    let version = "2.2.3";
    let prime_name = args.prime.as_str().to_string();
    let field_constant = UsefulConstants::new(&prime_name).get_p().clone();

    let parser_result = parser::run_parser(
        args.circuit_file.clone(), version, args.link_libraries.clone(),
        &field_constant, false);
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
        flag_old_heuristics: args.use_old_simplification_heuristics,
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

    let circuit = run_compiler(
        vcp.clone(),
        Config {
            debug_output: false,
            produce_input_log: true,
            wat_flag: false,
            sanity_check_style: 2,
            no_asm_flag: false,
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

    let types = build_types_list(&vcp.buses);

    let mut input_infos = vec![];
    for w in &circuit.templates[vcp.main_id].wires {
        if w.xtype() != SignalType::Input {
            continue;
        }
        input_infos.push(vm2::InputInfo{
            name: w.name().clone(),
            offset: w.dag_local_id(),
            lengths: w.lengths().clone(),
            type_id: w.bus_id().map(|id|types[id].name.clone())
        });
    }

    let inputs_size = input_infos.get_total_size(&types).unwrap();
    let inputs_offset = input_infos.min_offset().unwrap_or(0);

    // The node indexes for each signal. For example, in
    // signal_node_idx[3] stored the node index for signal 3.
    let mut signal_node_idx: Vec<usize> =
        vec![usize::MAX; circuit.c_producer.total_number_of_signals];

    setup_input_nodes(
        &mut nodes, &mut signal_node_idx, inputs_size, inputs_offset);

    // assert that template id is equal to index in templates list
    for (i, t) in circuit.templates.iter().enumerate() {
        assert_eq!(i, t.id);
        if args.print_debug {
            println!("Template #{}: {}", i, t.name);
        }
    }

    let mut ctx = BuildCircuitContext::new(
        &mut nodes, &mut signal_node_idx, &circuit.templates,
        &circuit.functions, circuit.c_producer.get_io_map(),
        circuit.c_producer.get_busid_field_info(), args.print_debug);

    let main_component_signal_start = 1usize;
    let main_template_id = vcp.main_id;
    let mut main_component = ctx.new_component(
        main_template_id, main_component_signal_start, 0);

    run_template(&mut ctx, &[], &mut main_component);

    ctx.update_progress_message();
    ctx.progress_bar.finish();
    drop(ctx);

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
    let mut witness_node_idxes = Vec::with_capacity(witness_list.len());
    for signal_idx in witness_list.iter() {
        let node_idx = if signal_node_idx[*signal_idx] == usize::MAX {
            nodes.const_node_idx_from_value(T::zero())
        } else {
            signal_node_idx[*signal_idx]
        };
        signal_node_idx[*signal_idx] = node_idx;
        witness_node_idxes.push(node_idx);
    }

    println!("number of nodes {}, signals {}", nodes.len(), witness_node_idxes.len());

    optimize(&mut nodes, &mut witness_node_idxes);

    println!(
        "number of nodes after optimize {}, signals {}",
        nodes.len(), witness_node_idxes.len());

    let f = fs::File::create(&args.graph_file).unwrap();
    serialize_witnesscalc_graph(f, &nodes, &witness_node_idxes, &input_infos, &types).unwrap();

    println!("circuit graph saved to file: {}", &args.graph_file);
}

struct SignalDestination<'a> {
    subcomponent_idx: usize,
    input_information: &'a InputInformation,
    dest: &'a LocationRule,
}

fn store_subcomponent_signals<T, NS>(
    ctx: &mut BuildCircuitContext<T, NS>, src_node_idxs: &[usize],
    call_stack: &Vec<String>, cmp: &mut ComponentInstance<T>,
    dst: &SignalDestination)
where
    T: FieldOps + 'static,
    NS: NodesStorage + 'static {

    let input_status: &StatusInput;
    if let InputInformation::Input { status, .. } = dst.input_information {
        input_status = status;
    } else {
        panic!("incorrect input information for subcomponent signal");
    }

    let subcomponent_idx = dst.subcomponent_idx;

    let (signal_idx, template_header, _) = match dst.dest {
        LocationRule::Indexed {
            location,
            template_header,
        } => {
            let signal_idx =
                calc_expression(ctx, location, cmp, call_stack)
                    .must_const_usize(ctx.nodes, call_stack);
            (signal_idx, template_header.as_ref().unwrap_or(&"-".to_string()).clone(), src_node_idxs.len())
        }
        LocationRule::Mapped { signal_code, indexes } => {
            calc_mapped_signal_idx(
                ctx, cmp, subcomponent_idx, *signal_code, indexes, call_stack)
        }
    };

    let sub_cmp = cmp.subcomponents[subcomponent_idx]
        .as_mut().unwrap();
    let signal_offset = sub_cmp.signal_offset;

    if ctx.print_debug {
        let location = match dst.dest {
            LocationRule::Indexed { .. } => "Indexed",
            LocationRule::Mapped { .. } => "Mapped",
        };
        println!(
            "Store subcomponent signal (location: {}, template: {}, subcomponent idx: {}, num: {}): {} + {} = {}",
            location, template_header, subcomponent_idx, src_node_idxs.len(),
            signal_offset, signal_idx, signal_offset + signal_idx);
        if template_header.starts_with("AliasCheck") {
            println!(
                "  alias store call stack [{}], nodes {:?}",
                call_stack.join(" -> "),
                src_node_idxs);
        }
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
        run_template(ctx, call_stack, sub_cmp)
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
