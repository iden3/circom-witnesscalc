use compiler::circuit_design::template::{TemplateCode};
use compiler::compiler_interface::{run_compiler, Circuit, Config};
use compiler::intermediate_representation::ir_interface::{AddressType, ComputeBucket, CreateCmpBucket, InputInformation, Instruction, InstructionPointer, LoadBucket, LocationRule, OperatorType, StatusInput, ValueBucket, ValueType};
use constraint_generation::{build_circuit, BuildConfig};
use program_structure::error_definition::Report;
use ruint::aliases::U256;
use ruint::uint;
use std::collections::HashMap;
use std::{env, fs};
use std::path::PathBuf;
use lazy_static::lazy_static;
use type_analysis::check_types::check_types;
use witness::graph::{optimize, Node, Operation, UnoOperation, TresOperation};

pub const M: U256 =
    uint!(21888242871839275222246405745257275088548364400416034343698204186575808495617_U256);

// if instruction pointer is a store to the signal, return the signal index
// and the src instruction to store to the signal
fn try_signal_store<'a>(
    inst: &'a InstructionPointer,
    nodes: &mut Vec<Node>,
    vars: &Vec<Option<Var>>,
    component_signal_start: usize,
    signal_node_idx: &mut Vec<usize>,
    subcomponents: &Vec<Option<ComponentInstance>>,
) -> Option<(usize, &'a InstructionPointer)> {
    let store_bucket = match **inst {
        Instruction::Store(ref store_bucket) => store_bucket,
        _ => return None,
    };
    if let AddressType::Signal = store_bucket.dest_address_type {

    } else { return None; };
    match &store_bucket.dest {
        LocationRule::Indexed {
            location,
            template_header,
        } => {
            if template_header.is_some() {
                panic!("not implemented: template_header expected to be None");
            }
            let signal_idx = calc_expression(
                location, nodes, vars, component_signal_start,
                signal_node_idx, subcomponents);
            let signal_idx = if let Var::Value(ref c) = signal_idx {
                bigint_to_usize(c)
            } else {
                panic!("signal index is not a constant")
            };

            let signal_idx = component_signal_start + signal_idx;
            return Some((signal_idx, &store_bucket.src));
        }
        LocationRule::Mapped { .. } => {
            todo!()
        }
    }
}

fn value_from_instruction_usize(inst: &InstructionPointer) -> usize {
    match **inst {
        Instruction::Value(ref value_bucket) => match value_bucket.parse_as {
            ValueType::BigInt => {
                panic!("unexpected value type for usize: BigInt")
            }
            ValueType::U32 => return value_bucket.value,
        },
        _ => {
            panic!("not implemented");
        }
    }
}

fn int_from_value_instruction(value_bucket: &ValueBucket, nodes: &Vec<Node>) -> U256 {
    match value_bucket.parse_as {
        ValueType::BigInt => match nodes[value_bucket.value] {
            Node::Constant(ref c) => c.clone(),
            _ => panic!("not a constant"),
        },
        ValueType::U32 => U256::from(value_bucket.value),
    }
}

fn operator_argument_instruction_n(
    inst: &InstructionPointer,
    nodes: &mut Vec<Node>,
    signal_node_idx: &mut Vec<usize>,
    vars: &mut Vec<Option<Var>>,
    component_signal_start: usize,
    subcomponents: &Vec<Option<ComponentInstance>>,
    size: usize,
) -> Vec<usize> {

    assert!(size > 0, "size = {}", size);

    if size == 1 {
        // operator_argument_instruction implements much more cases than
        // this function, so we can use it here is size == 1
        return vec![operator_argument_instruction(
            inst, nodes, signal_node_idx, vars,
            component_signal_start, subcomponents)];
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
                        let signal_idx = calc_expression(
                            location, nodes, vars, component_signal_start,
                            signal_node_idx, subcomponents);
                        let signal_idx = if let Var::Value(c) = signal_idx {
                            bigint_to_usize(&c)
                        } else {
                            panic!("signal index is not a constant")
                        };
                        let mut result = Vec::with_capacity(size);
                        for i in 0..size {
                            let signal_node = signal_node_idx[component_signal_start + signal_idx + i];
                            assert_ne!(
                                signal_node, usize::MAX,
                                "signal {}/{}/{} is not set yet",
                                component_signal_start, signal_idx, i);
                            result.push(signal_node);
                        }
                        return result;
                    }
                    LocationRule::Mapped {..} => {
                        todo!()
                    }
                },
                AddressType::SubcmpSignal {..} => {
                    panic!("multi-index load is not implemented for SubcmpSignal");
                }
                AddressType::Variable => {
                    panic!("multi-index load is not implemented for Variable");
                }
            }
        }
        _ => {
            panic!("multi-operator is not implemented for instruction: {}", inst.to_string());
        }
    }
}


fn operator_argument_instruction(
    inst: &InstructionPointer,
    nodes: &mut Vec<Node>,
    signal_node_idx: &mut Vec<usize>,
    vars: &mut Vec<Option<Var>>,
    component_signal_start: usize,
    subcomponents: &Vec<Option<ComponentInstance>>,
) -> usize {
    match **inst {
        Instruction::Load(ref load_bucket) => {
            println!("load bucket: {}", load_bucket.src.to_string());
            println!(
                "load bucket addr type: {:?}",
                match load_bucket.address_type {
                    AddressType::Variable => "Variable",
                    AddressType::Signal => "Signal",
                    AddressType::SubcmpSignal { .. } => "SubcmpSignal",
                }
            );
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
                            location, nodes, vars, component_signal_start,
                            signal_node_idx, subcomponents);
                        let signal_idx = if let Var::Value(c) = signal_idx {
                            bigint_to_usize(&c)
                        } else {
                            panic!("signal index is not a constant")
                        };
                        let signal_idx = component_signal_start + signal_idx;
                        let signal_node = signal_node_idx[signal_idx];
                        assert_ne!(signal_node, usize::MAX, "signal is not set yet");
                        return signal_node;
                    }
                    LocationRule::Mapped { .. } => {
                        todo!()
                    }
                },
                AddressType::SubcmpSignal {
                    ref cmp_address, ..
                } => {
                    let subcomponent_idx = calc_expression(
                        cmp_address, nodes, vars, component_signal_start,
                        signal_node_idx, subcomponents);
                    let subcomponent_idx = if let Var::Value(ref c) = subcomponent_idx {
                        bigint_to_usize(c)
                    } else {
                        panic!("signal index is not a constant")
                    };

                    match load_bucket.src {
                        LocationRule::Indexed {
                            ref location,
                            ref template_header,
                        } => {
                            let signal_idx = calc_expression(
                                location, nodes, vars, component_signal_start,
                                signal_node_idx, subcomponents);
                            let signal_idx = if let Var::Value(ref c) = signal_idx {
                                bigint_to_usize(c)
                            } else {
                                panic!("signal index is not a constant")
                            };
                            let signal_offset = subcomponents[subcomponent_idx]
                                .as_ref()
                                .unwrap()
                                .signal_offset;
                            println!(
                                "Load subcomponent signal: ({}) [{}] {} + {} = {}",
                                template_header.as_ref().unwrap_or(&"-".to_string()),
                                subcomponent_idx, signal_offset, signal_idx,
                                signal_offset + signal_idx);

                            let signal_idx = subcomponents[subcomponent_idx]
                                .as_ref()
                                .unwrap()
                                .signal_offset
                                + signal_idx;
                            let signal_node = signal_node_idx[signal_idx];
                            assert_ne!(signal_node, usize::MAX, "signal is not set yet");
                            return signal_node;
                        }
                        LocationRule::Mapped { .. } => {
                            todo!()
                        }
                    }
                }
                AddressType::Variable => {
                    panic!("not implemented getting operand for variable");
                }
            }
        }
        Instruction::Compute(ref compute_bucket) => {
            let node = node_from_compute_bucket(
                compute_bucket, nodes, signal_node_idx, vars,
                component_signal_start, subcomponents);
            nodes.push(node);
            return nodes.len() - 1;
        }
        Instruction::Value(ref value_bucket) => {
            match value_bucket.parse_as {
                ValueType::BigInt => match nodes[value_bucket.value] {
                    Node::Constant(..) => {
                        return value_bucket.value;
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

lazy_static! {
    static ref DUO_OPERATORS_MAP: HashMap<OperatorType, Operation> = {
        let mut m = HashMap::new();
        m.insert(OperatorType::Mul, Operation::Mul);
        m.insert(OperatorType::Div, Operation::Div);
        m.insert(OperatorType::Add, Operation::Add);
        m.insert(OperatorType::Sub, Operation::Sub);
        m.insert(OperatorType::ShiftR, Operation::Shr);
        m.insert(OperatorType::BitAnd, Operation::Band);
        m
    };
    static ref UNO_OPERATORS_MAP: HashMap<OperatorType, UnoOperation> = {
        let mut m = HashMap::new();
        m.insert(OperatorType::PrefixSub, UnoOperation::Neg);
        m
    };
}

fn node_from_compute_bucket(
    compute_bucket: &ComputeBucket,
    nodes: &mut Vec<Node>,
    signal_node_idx: &mut Vec<usize>,
    vars: &mut Vec<Option<Var>>,
    component_signal_start: usize,
    subcomponents: &Vec<Option<ComponentInstance>>,
) -> Node {
    if let Some(op) = DUO_OPERATORS_MAP.get(&compute_bucket.op) {
        let arg1 = operator_argument_instruction(
            &compute_bucket.stack[0],
            nodes,
            signal_node_idx,
            vars,
            component_signal_start,
            subcomponents,
        );
        let arg2 = operator_argument_instruction(
            &compute_bucket.stack[1],
            nodes,
            signal_node_idx,
            vars,
            component_signal_start,
            subcomponents,
        );
        return Node::Op(op.clone(), arg1, arg2);
    }
    if let Some(op) = UNO_OPERATORS_MAP.get(&compute_bucket.op) {
        let arg1 = operator_argument_instruction(
            &compute_bucket.stack[0],
            nodes,
            signal_node_idx,
            vars,
            component_signal_start,
            subcomponents,
        );
        return Node::UnoOp(op.clone(), arg1);
    }
    panic!(
        "not implemented: this operator is not supported to be converted to Node: {}",
        compute_bucket.to_string());
}

fn process_instruction(
    inst: &InstructionPointer,
    nodes: &mut Vec<Node>,
    signal_node_idx: &mut Vec<usize>,
    vars: &mut Vec<Option<Var>>,
    subcomponents: &mut Vec<Option<ComponentInstance>>,
    templates: &Vec<TemplateCode>,
    component_signal_start: usize,
) {
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
                            let signal_idx = calc_expression(
                                location, nodes, vars, component_signal_start,
                                signal_node_idx, subcomponents);
                            let signal_idx = if let Var::Value(ref c) = signal_idx {
                                bigint_to_usize(c)
                            } else {
                                panic!("signal index is not a constant")
                            };

                            // println!(
                            //     "Store signal at offset {} + {} = {}",
                            //     component_signal_start,
                            //     signal_idx,
                            //     component_signal_start + signal_idx
                            // );
                            let signal_idx = component_signal_start + signal_idx;

                            let node_idxs = operator_argument_instruction_n(
                                &store_bucket.src, nodes, signal_node_idx, vars,
                                component_signal_start, subcomponents,
                                store_bucket.context.size);

                            assert_eq!(node_idxs.len(), store_bucket.context.size);

                            for i in 0..store_bucket.context.size {
                                if signal_node_idx[signal_idx + i] != usize::MAX {
                                    panic!("signal is already set");
                                }
                                signal_node_idx[signal_idx + i] = node_idxs[i];
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
                            let lvar_idx = value_from_instruction_usize(location);
                            vars[lvar_idx] = Some(calc_expression(
                                &store_bucket.src, nodes, vars,
                                component_signal_start, signal_node_idx,
                                subcomponents));
                        }
                        _ => {
                            panic!(
                                "not implemented: variable destination: {}",
                                store_bucket.dest.to_string()
                            );
                        }
                    }
                }
                AddressType::SubcmpSignal {
                    ref cmp_address,
                    ref input_information,
                    ..
                } => {
                    let input_status: &StatusInput;
                    if let InputInformation::Input { ref status } = input_information {
                        input_status = status;
                    } else {
                        panic!("incorrect input information for subcomponent signal");
                    }
                    let subcomponent_idx = calc_expression(
                        cmp_address, nodes, vars, component_signal_start,
                        signal_node_idx, subcomponents);
                    let subcomponent_idx = if let Var::Value(ref c) = subcomponent_idx {
                        bigint_to_usize(&c)
                    } else {
                        panic!("subcomponent index is not a constant");
                    };

                    let node_idxs = operator_argument_instruction_n(
                        &store_bucket.src, nodes, signal_node_idx, vars,
                        component_signal_start, subcomponents,
                        store_bucket.context.size);
                    assert_eq!(node_idxs.len(), store_bucket.context.size);

                    match store_bucket.dest {
                        LocationRule::Indexed {
                            ref location,
                            ref template_header,
                        } => {
                            let signal_idx = calc_expression(
                                location, nodes, vars, component_signal_start,
                                signal_node_idx, subcomponents);
                            let signal_idx = if let Var::Value(ref c) = signal_idx {
                                bigint_to_usize(c)
                            } else {
                                panic!("signal index is not a constant");
                            };
                            let signal_offset = subcomponents[subcomponent_idx]
                                .as_ref()
                                .unwrap()
                                .signal_offset;
                            println!(
                                "Store subcomponent signal: ({}) [{}] {} + {} = {}",
                                template_header.as_ref().unwrap_or(&"-".to_string()),
                                subcomponent_idx,
                                signal_offset,
                                signal_idx,
                                signal_offset + signal_idx
                            );
                            let signal_idx = signal_offset + signal_idx;
                            for i in 0..store_bucket.context.size {
                                if signal_node_idx[signal_idx + i] != usize::MAX {
                                    panic!("subcomponent signal is already set");
                                }
                                signal_node_idx[signal_idx + i] = node_idxs[i];
                            }
                            subcomponents[subcomponent_idx]
                                .as_mut()
                                .unwrap()
                                .number_of_inputs -= store_bucket.context.size;
                        }
                        LocationRule::Mapped { .. } => {
                            todo!()
                        }
                    }

                    let number_of_inputs = subcomponents[subcomponent_idx]
                        .as_ref()
                        .unwrap()
                        .number_of_inputs;

                    let run_component = match input_status {
                        StatusInput::Last => {
                            assert_eq!(number_of_inputs, 0);
                            true
                        }
                        StatusInput::NoLast => {
                            assert!(number_of_inputs > 0);
                            false
                        }
                        StatusInput::Unknown => number_of_inputs == 0,
                    };

                    if run_component {
                        run_template(
                            templates,
                            subcomponents[subcomponent_idx]
                                .as_ref()
                                .unwrap()
                                .template_id,
                            nodes,
                            signal_node_idx,
                            subcomponents[subcomponent_idx]
                                .as_ref()
                                .unwrap()
                                .signal_offset,
                        )
                    }
                }
            };
        }
        Instruction::Compute(_) => {
            panic!("not implemented");
        }
        Instruction::Call(_) => {
            panic!("not implemented");
        }
        Instruction::Branch(ref branch_bucket) => {
            let cond = calc_expression(
                &branch_bucket.cond, nodes, vars, component_signal_start,
                signal_node_idx, subcomponents);
            match cond {
                Var::Value(_c) => {
                    todo!("branch is implemented only for ternary operator")
                }
                Var::Node(node_idx) => {
                    // The only option for variable condition is a ternary operation.

                    if branch_bucket.if_branch.len() != 1 || branch_bucket.else_branch.len() != 1 {
                        panic!("Non-constant condition may be used only in ternary operation and both branches of code must be of length 1");
                    }
                    let if_branch = try_signal_store(
                        &branch_bucket.if_branch[0], nodes, vars,
                        component_signal_start, signal_node_idx, subcomponents);
                    let else_branch = try_signal_store(
                        &branch_bucket.else_branch[0], nodes, vars,
                        component_signal_start, signal_node_idx, subcomponents);
                    match (if_branch, else_branch) {
                        (Some((if_signal_idx, if_src)), Some((else_signal_idx, else_src))) => {
                            if if_signal_idx != else_signal_idx {
                                panic!("if and else branches must store to the same signal");
                            }

                            let node_idx_if = operator_argument_instruction(
                                if_src, nodes, signal_node_idx, vars,
                                component_signal_start, subcomponents);

                            let node_idx_else = operator_argument_instruction(
                                else_src, nodes, signal_node_idx, vars,
                                component_signal_start, subcomponents);

                            let node = Node::TresOp(TresOperation::TernCond, node_idx, node_idx_if, node_idx_else);
                            nodes.push(node);
                            assert_eq!(
                                signal_node_idx[if_signal_idx],
                                usize::MAX,
                                "signal already set"
                            );
                            signal_node_idx[if_signal_idx] = nodes.len() - 1;
                        }
                        _ => {
                            panic!(
                                "if branch or else branch is not a store to the signal, which is the only option for ternary operation {} {}",
                                branch_bucket.if_branch[0].to_string(),
                                branch_bucket.else_branch[0].to_string());
                        }
                    }
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
            panic!("not implemented");
        }
        Instruction::Loop(ref loop_bucket) => {
            while check_continue_condition(
                &loop_bucket.continue_condition, nodes, vars,
                component_signal_start, signal_node_idx, subcomponents) {

                for i in &loop_bucket.body {
                    process_instruction(
                        i,
                        nodes,
                        signal_node_idx,
                        vars,
                        subcomponents,
                        templates,
                        component_signal_start,
                    );
                }
            }
        }
        Instruction::CreateCmp(ref create_component_bucket) => {
            let sub_cmp_id = calc_expression(
                &create_component_bucket.sub_cmp_id, nodes, vars,
                component_signal_start, signal_node_idx, subcomponents);

            let cmp_idx = if let Var::Value(ref c) = sub_cmp_id {
                bigint_to_usize(c)
            } else {
                panic!("subcomponent index is not a constant");
            };
            assert!(
                cmp_idx + create_component_bucket.number_of_cmp - 1 < subcomponents.len(),
                "cmp_idx = {}, number_of_cmp = {}, subcomponents.len() = {}",
                cmp_idx,
                create_component_bucket.number_of_cmp,
                subcomponents.len()
            );

            let mut cmp_signal_offset = create_component_bucket.signal_offset;

            for i in cmp_idx..cmp_idx + create_component_bucket.number_of_cmp {
                if let Some(_) = subcomponents[i] {
                    panic!("subcomponent already set");
                }
                subcomponents[i] = Some(ComponentInstance {
                    template_id: create_component_bucket.template_id,
                    signal_offset: component_signal_start + cmp_signal_offset,
                    number_of_inputs: templates[create_component_bucket.template_id]
                        .number_of_inputs,
                });
                cmp_signal_offset += create_component_bucket.signal_offset_jump;
            }
            println!(
                "{}",
                fmt_create_cmp_bucket(
                    create_component_bucket, nodes, vars, component_signal_start,
                    signal_node_idx, subcomponents)
            );
        }
    }
}

fn bigint_to_usize(value: &U256) -> usize {
    // Convert U256 to usize
    let bytes = value.to_le_bytes::<32>().to_vec(); // Convert to little-endian bytes
    for i in std::mem::size_of::<usize>()..bytes.len() {
        if bytes[i] != 0 {
            panic!("Value is too large to fit into usize");
        }
    }
    usize::from_le_bytes(
        bytes[..std::mem::size_of::<usize>()]
            .try_into()
            .expect("slice with incorrect length"),
    )
}

struct ComponentInstance {
    template_id: usize,
    signal_offset: usize,
    number_of_inputs: usize,
}

fn fmt_create_cmp_bucket(
    cmp_bucket: &CreateCmpBucket,
    nodes: &mut Vec<Node>,
    vars: &Vec<Option<Var>>,
    component_signal_start: usize,
    signal_node_idx: &mut Vec<usize>,
    subcomponents: &Vec<Option<ComponentInstance>>,
) -> String {
    let sub_cmp_id = calc_expression(
        &cmp_bucket.sub_cmp_id, nodes, vars, component_signal_start,
        signal_node_idx, subcomponents);

    let sub_cmp_id = match sub_cmp_id {
        Var::Value(ref c) => format!("Constant {}", c.to_string()),
        Var::Node(idx) => format!("Variable {}", idx) };

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
                 has_inputs: {}"#,
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
        cmp_bucket.has_inputs
    )
}

#[derive(Clone)]
enum Var {
    Value(U256),
    Node(usize),
}

impl ToString for Var {
    fn to_string(&self) -> String {
        match self {
            Var::Value(ref c) => { format!("Var::Value({})", c.to_string()) }
            Var::Node(idx) => { format!("Var::Node({})", idx) }
        }
    }
}

fn load(
    load_bucket: &LoadBucket,
    nodes: &mut Vec<Node>,
    vars: &Vec<Option<Var>>,
    component_signal_start: usize,
    signal_node_idx: &mut Vec<usize>,
    subcomponents: &Vec<Option<ComponentInstance>>,
) -> Var {
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
                    location,
                    nodes,
                    vars,
                    component_signal_start,
                    signal_node_idx,
                    subcomponents,
                );
                let signal_idx = if let Var::Value(c) = signal_idx {
                    bigint_to_usize(&c)
                } else {
                    panic!("signal index is not a constant")
                };
                let signal_idx = component_signal_start + signal_idx;
                let signal_node = signal_node_idx[signal_idx];
                assert_ne!(signal_node, usize::MAX, "signal is not set yet");
                return Var::Node(signal_node);
            }
            LocationRule::Mapped { .. } => {
                todo!()
            }
        },
        AddressType::SubcmpSignal {
            ref cmp_address, ..
        } => {
            let subcomponent_idx = calc_expression(
                cmp_address,
                nodes,
                vars,
                component_signal_start,
                signal_node_idx,
                subcomponents,
            );
            let subcomponent_idx = if let Var::Value(c) = subcomponent_idx {
                bigint_to_usize(&c)
            } else {
                panic!("subcomponent index is not a constant");
            };

            match load_bucket.src {
                LocationRule::Indexed {
                    ref location,
                    ref template_header,
                } => {
                    let signal_idx = calc_expression(
                        location,
                        nodes,
                        vars,
                        component_signal_start,
                        signal_node_idx,
                        subcomponents,
                    );
                    let signal_idx = if let Var::Value(c) = signal_idx {
                        bigint_to_usize(&c)
                    } else {
                        panic!("signal index is not a constant");
                    };
                    let signal_offset = subcomponents[subcomponent_idx]
                        .as_ref()
                        .unwrap()
                        .signal_offset;

                    let signal_idx = signal_offset + signal_idx;
                    let signal_node = signal_node_idx[signal_idx];

                    assert_ne!(
                        signal_node,
                        usize::MAX,
                        "subcomponent signal is not set yet ({})",
                        template_header.as_ref().unwrap_or(&"-".to_string())
                    );
                    return Var::Node(signal_node);
                }
                LocationRule::Mapped { .. } => {
                    todo!()
                }
            }
        }
        AddressType::Variable => match load_bucket.src {
            LocationRule::Indexed {
                ref location,
                ref template_header,
            } => {
                if template_header.is_some() {
                    panic!("not implemented: template_header expected to be None");
                }
                let var_idx = calc_expression(
                    location,
                    nodes,
                    vars,
                    component_signal_start,
                    signal_node_idx,
                    subcomponents,
                );
                let var_idx = if let Var::Value(c) = var_idx {
                    bigint_to_usize(&c)
                } else {
                    panic!("signal index is not a constant");
                };

                return match vars[var_idx] {
                    Some(ref v) => v.clone(),
                    None => panic!("variable is not set yet"),
                }
            }
            LocationRule::Mapped { .. } => {
                todo!()
            }
        },
    }
}

fn build_unary_op_var(
    compute_bucket: &ComputeBucket,
    nodes: &mut Vec<Node>,
    vars: &Vec<Option<Var>>,
    component_signal_start: usize,
    signal_node_idx: &mut Vec<usize>,
    subcomponents: &Vec<Option<ComponentInstance>>,
) -> Var {

    assert_eq!(compute_bucket.stack.len(), 1);
    let a = calc_expression(
        &compute_bucket.stack[0],
        nodes,
        vars,
        component_signal_start,
        signal_node_idx,
        subcomponents,
    );

    match &a {
        Var::Value(ref a) => {
            Var::Value(match compute_bucket.op {
                OperatorType::ToAddress => a.clone(),
                OperatorType::PrefixSub => if a.clone() == U256::ZERO { U256::ZERO } else { M - a }
                _ => {
                    todo!(
                        "unary operator not implemented: {}",
                        compute_bucket.op.to_string()
                    );
                }
            })
        },
        Var::Node(node_idx ) => {
            let node = Node::UnoOp(match compute_bucket.op {
                OperatorType::PrefixSub => UnoOperation::Neg,
                OperatorType::ToAddress => { panic!("operator does not support variable address") },
                _ => {
                    todo!(
                        "operator not implemented: {}",
                        compute_bucket.op.to_string()
                    );
                }
            }, node_idx.clone());
            nodes.push(node);
            Var::Node(nodes.len() - 1)
        }
    }
}

// Create a Var from operation on two arguments a anb b
fn build_binary_op_var(
    compute_bucket: &ComputeBucket,
    nodes: &mut Vec<Node>,
    vars: &Vec<Option<Var>>,
    component_signal_start: usize,
    signal_node_idx: &mut Vec<usize>,
    subcomponents: &Vec<Option<ComponentInstance>>,
) -> Var {

    assert_eq!(compute_bucket.stack.len(), 2);
    let a = calc_expression(
        &compute_bucket.stack[0],
        nodes,
        vars,
        component_signal_start,
        signal_node_idx,
        subcomponents,
    );
    let b = calc_expression(
        &compute_bucket.stack[1],
        nodes,
        vars,
        component_signal_start,
        signal_node_idx,
        subcomponents,
    );

    let mut node_idx = |v: &Var| match v {
        Var::Value(ref c) => {
            nodes.push(Node::Constant(c.clone()));
            nodes.len() - 1
        }
        Var::Node(idx) => { idx.clone() }
    };

    match (&a, &b) {
        (Var::Value(ref a), Var::Value(ref b)) => {
            Var::Value(match compute_bucket.op {
                OperatorType::Div => if b.clone() == U256::ZERO {
                    // as we are simulating a circuit execution with signals
                    // values all equal to 0, just return 0 here in case of
                    // division by zero
                    U256::ZERO
                } else {
                    a.mul_mod(b.inv_mod(M).unwrap(), M)
                },
                OperatorType::Add => a.add_mod(b.clone(), M),
                OperatorType::Sub => a.add_mod(M - b, M),
                OperatorType::ShiftR => Operation::Shr.eval(a.clone(), b.clone()),
                OperatorType::Lesser => if a < b { U256::from(1) } else { U256::ZERO }
                OperatorType::NotEq => U256::from(a != b),
                OperatorType::BitAnd => Operation::Band.eval(a.clone(), b.clone()),
                OperatorType::MulAddress => a * b,
                OperatorType::AddAddress => a + b,
                _ => {
                    todo!(
                        "operator not implemented: {}",
                        compute_bucket.op.to_string()
                    );
                }
            })
        },
        _ => {
            let node = Node::Op(match compute_bucket.op {
                OperatorType::Div => Operation::Div,
                OperatorType::Add => Operation::Add,
                OperatorType::Sub => Operation::Sub,
                OperatorType::ShiftR => Operation::Shr,
                OperatorType::Lesser => Operation::Lt,
                OperatorType::NotEq => Operation::Neq,
                OperatorType::BitAnd => Operation::Band,
                _ => {
                    todo!(
                        "operator not implemented: {}",
                        compute_bucket.op.to_string()
                    );
                }
            }, node_idx(&a), node_idx(&b));
            nodes.push(node);
            Var::Node(nodes.len() - 1)
        }
    }
}

// This function should calculate node based only on constant or variable
// values. Not based on signal values.
fn calc_expression(
    inst: &InstructionPointer,
    nodes: &mut Vec<Node>,
    vars: &Vec<Option<Var>>,
    component_signal_start: usize,
    signal_node_idx: &mut Vec<usize>,
    subcomponents: &Vec<Option<ComponentInstance>>,
) -> Var {
    match **inst {
        Instruction::Value(ref value_bucket) => {
            Var::Value(int_from_value_instruction(value_bucket, nodes))
        }
        Instruction::Load(ref load_bucket) => load(
            load_bucket, nodes, vars, component_signal_start, signal_node_idx,
            subcomponents),
        Instruction::Compute(ref compute_bucket) => match compute_bucket.op {
            OperatorType::Div | OperatorType::Add | OperatorType::Sub
            | OperatorType::ShiftR | OperatorType::Lesser | OperatorType::NotEq
            | OperatorType::BitAnd | OperatorType::MulAddress
            | OperatorType::AddAddress => {
                build_binary_op_var(
                    compute_bucket, nodes, vars, component_signal_start,
                    signal_node_idx, subcomponents)
            }
            OperatorType::ToAddress | OperatorType::PrefixSub => {
                build_unary_op_var(
                    compute_bucket, nodes, vars, component_signal_start,
                    signal_node_idx, subcomponents)
            }
            _ => {
                todo!(
                    "operator not implemented: {}",
                    compute_bucket.op.to_string()
                );
            }
        },
        _ => {
            panic!(
                "instruction evaluation is not supported: {}",
                inst.to_string()
            );
        }
    }
}

fn check_continue_condition(
    inst: &InstructionPointer,
    nodes: &mut Vec<Node>,
    vars: &Vec<Option<Var>>,
    component_signal_start: usize,
    signal_node_idx: &mut Vec<usize>,
    subcomponents: &Vec<Option<ComponentInstance>>,
) -> bool {
    let val = calc_expression(inst, nodes, vars, component_signal_start, signal_node_idx, subcomponents);
    match val {
        Var::Value(c) => c != U256::ZERO,
        _ => {
            panic!("continue condition is not a constant");
        }
    }
}

fn get_constants(circuit: &Circuit) -> Vec<Node> {
    let mut constants: Vec<Node> = Vec::new();
    for c in &circuit.c_producer.field_tracking {
        constants.push(Node::Constant(U256::from_str_radix(c.as_str(), 10).unwrap()));
    }
    constants
}

fn init_input_signals(
    circuit: &Circuit,
    nodes: &mut Vec<Node>,
    signal_node_idx: &mut Vec<usize>,
    input_file: Option<String>,
) -> (HashMap<String, (usize, usize)>, Vec<U256>) {
    let input_list = circuit.c_producer.get_main_input_list();
    let mut signal_values: Vec<U256> = Vec::new();
    signal_values.push(U256::from(1));
    nodes.push(Node::Input(signal_values.len() - 1));
    signal_node_idx[0] = nodes.len() - 1;
    let mut inputs_info = HashMap::new();

    let inputs: Option<HashMap<String, Vec<U256>>> = match input_file {
        Some(file) => {
            let inputs_data = fs::read(file).expect("Failed to read input file");
            Some(serde_json::from_slice::<HashMap<String, Vec<U256>>>(inputs_data.as_slice()).unwrap())
        }
        None => {
            None
        }
    };

    for (name, offset, len) in input_list {
        inputs_info.insert(name.clone(), (signal_values.len(), len.clone()));
        match inputs {
            Some(ref inputs) => {
                match inputs.get(name) {
                    Some(values) => {
                        if values.len() != *len {
                            panic!(
                                "input signal {} has different length in inputs file, want {}, actual {}",
                                name, *len, values.len());
                        }
                        for (i, v) in values.iter().enumerate() {
                            signal_values.push(v.clone());
                            nodes.push(Node::Input(signal_values.len() - 1));
                            signal_node_idx[offset + i] = nodes.len() - 1;
                        }
                    }
                    None => {
                        panic!("input signal {} is not found in inputs file", name);
                    }
                }
            }
            None => {
                for i in 0..*len {
                    signal_values.push(U256::ZERO);
                    nodes.push(Node::Input(signal_values.len() - 1));
                    signal_node_idx[offset + i] = nodes.len() - 1;
                }
            }
        }
    }

    return (inputs_info, signal_values);
}

fn run_template(
    templates: &Vec<TemplateCode>,
    template_id: usize,
    nodes: &mut Vec<Node>,
    signal_node_idx: &mut Vec<usize>,
    component_signal_start: usize,
) {
    let tmpl = &templates[template_id];

    println!(
        "Run template #{}: {}, body length: {}",
        tmpl.id,
        tmpl.name,
        tmpl.body.len()
    );

    let mut vars: Vec<Option<Var>> = vec![Option::None; tmpl.var_stack_depth];
    let mut components: Vec<Option<ComponentInstance>> = vec![];
    for _ in 0..tmpl.number_of_components {
        components.push(None);
    }

    for (idx, inst) in tmpl.body.iter().enumerate() {
        println!("instruction {}/{}: {}", tmpl.id, idx, inst.to_string());
        process_instruction(
            &inst,
            nodes,
            signal_node_idx,
            &mut vars,
            &mut components,
            templates,
            component_signal_start,
        );
    }

    // TODO: assert all components run
}

struct Args {
    circuit_file: String,
    inputs_file: Option<String>,
    graph_file: String,
    link_libraries: Vec<PathBuf>,
    print_unoptimized: bool,
}

fn parse_args() -> Args {
    let args: Vec<String> = env::args().collect();
    let mut i = 1;
    let mut circuit_file: Option<String> = None;
    let mut graph_file: Option<String> = None;
    let mut link_libraries: Vec<PathBuf> = Vec::new();
    let mut inputs_file: Option<String> = None;
    let mut print_unoptimized = false;

    let usage = |err_msg: &str| -> String {
        eprintln!("{}", err_msg);
        eprintln!("Usage: {} <circuit_file> <graph_file> [-l <link_library>]* [-i <inputs_file.json>] [-print-unoptimized]", args[0]);
        std::process::exit(1);
    };

    while i < args.len() {
        if args[i] == "-l" {
            i += 1;
            if i >= args.len() {
                usage("missing argument for -l");
            }
            link_libraries.push(args[i].clone().into());
        } else if args[i].starts_with("-l") {
            link_libraries.push(args[i][2..].to_string().into())
        } else if args[i] == "-i" {
            i += 1;
            if i >= args.len() {
                usage("missing argument for -i");
            }
            if let None = inputs_file {
                inputs_file = Some(args[i].clone());
            } else {
                usage("multiple inputs files");
            }
        } else if args[i].starts_with("-i") {
            if let None = inputs_file {
                inputs_file = Some(args[i][2..].to_string());
            } else {
                usage("multiple inputs files");
            }
        } else if args[i] == "-print-unoptimized" {
            print_unoptimized = true;
        } else if args[i].starts_with("-") {
            let message = format!("unknown argument: {}", args[i]);
            usage(&message);
        } else if let None = circuit_file {
            circuit_file = Some(args[i].clone());
        } else if let None = graph_file {
            graph_file = Some(args[i].clone());
        } else {
            usage(format!("unexpected argument: {}", args[i]).as_str());
        }
        i += 1;
    };

    Args {
        circuit_file: circuit_file.unwrap_or_else(|| {usage("missing circuit file")}),
        inputs_file,
        graph_file: graph_file.unwrap_or_else(|| {usage("missing graph file")}),
        link_libraries,
        print_unoptimized,
    }
}

fn main() {
    let args = parse_args();

    let version = "2.1.9";

    // let main_file = "/Users/alek/src/simple-circuit/circuit3.circom";
    // let main_file = "/Users/alek/src/circuits/circuits/authV2.circom";
    // let link_libraries: Vec<PathBuf> =
    //     vec!["/Users/alek/src/circuits/node_modules/circomlib/circuits".into()];
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
            println!("Type errors:");
            for err in errs {
                println!("{}", err.get_message());
            }
            panic!();
        }
        Ok(warns) => {
            if !warns.is_empty() {
                println!("Type warnings:");
                for warn in warns {
                    println!("{}", warn.get_message());
                }
            }
        }
    }

    let build_config = BuildConfig {
        no_rounds: usize::MAX,
        flag_json_sub: false,
        json_substitutions: String::new(),
        flag_s: false,
        flag_f: false,
        flag_p: false,
        flag_verbose: false,
        flag_old_heuristics: false,
        inspect_constraints: false,
        prime: String::from("bn128"),
    };

    let (_, vcp) = build_circuit(program_archive, build_config).unwrap();

    let main_template_id = vcp.main_id;
    let witness_list = vcp.get_witness_list().clone();

    let circuit = run_compiler(
        vcp,
        Config {
            debug_output: true,
            produce_input_log: true,
            wat_flag: false,
        },
        version,
    )
    .unwrap();
    println!("prime: {}", circuit.c_producer.prime);
    println!("prime_str: {}", circuit.c_producer.prime_str);
    println!("templates len: {}", circuit.templates.len());
    println!("functions len: {}", circuit.functions.len());
    println!("main header: {}", circuit.c_producer.main_header);

    let mut signal_node_idx: Vec<usize> =
        vec![usize::MAX; circuit.c_producer.total_number_of_signals];

    let mut nodes: Vec<Node> = Vec::new();
    nodes.extend(get_constants(&circuit));
    let (input_signals, input_signal_values) = init_input_signals(
        &circuit, &mut nodes, &mut signal_node_idx, args.inputs_file);

    // assert that template id is equal to index in templates list
    for (i, t) in circuit.templates.iter().enumerate() {
        assert_eq!(i, t.id);
    }

    let main_component_signal_start = 1usize;
    run_template(
        &circuit.templates,
        main_template_id,
        &mut nodes,
        &mut signal_node_idx,
        main_component_signal_start,
    );

    for (idx, i) in signal_node_idx.iter().enumerate() {
        assert_ne!(i.clone(), usize::MAX, "signal #{} is not set", idx);
    }

    let mut signals = witness_list
        .iter()
        .map(|&i| signal_node_idx[i])
        .collect::<Vec<_>>();

    if args.print_unoptimized {
        println!("Unoptimized graph:");
        evaluate_unoptimized(&nodes, &input_signal_values, &signal_node_idx, &witness_list);
    }

    println!("number of nodes {}, signals {}", nodes.len(), signals.len());

    optimize(&mut nodes, &mut signals);

    println!(
        "number of nodes after optimize {}, signals {}",
        nodes.len(),
        signals.len()
    );

    // let mut input_signals: HashMap<String, (usize, usize)> = HashMap::new();
    // for (name, offset, len) in circuit.c_producer.get_main_input_list() {
    //     input_signals.insert(name.clone(), (offset.clone(), len.clone()));
    // }

    let bytes = postcard::to_stdvec(&(&nodes, &signals, &input_signals)).unwrap();
    fs::write(&args.graph_file, bytes).unwrap();

    println!("circuit graph saved to file: {}", &args.graph_file)
}

fn evaluate_unoptimized(nodes: &[Node], inputs: &[U256], signal_node_idx: &Vec<usize>, witness_signals: &[usize]) {
    let mut node_idx_to_signal: HashMap<usize, Vec<usize>> = HashMap::new();
    for (signal_idx, &node_idx) in signal_node_idx.iter().enumerate() {
        node_idx_to_signal.entry(node_idx).and_modify(|v| v.push(signal_idx)).or_insert(vec![signal_idx]);
    }

    let mut signal_to_witness: HashMap<usize, Vec<usize>> = HashMap::new();
    for (witness_idx, &signal_idx) in witness_signals.iter().enumerate() {
        println!("witness {} -> {}", witness_idx, signal_idx);
        signal_to_witness.entry(signal_idx).and_modify(|v| v.push(witness_idx)).or_insert(vec![witness_idx]);
    }

    let mut values = Vec::with_capacity(nodes.len());

    for (node_idx, &node) in nodes.iter().enumerate() {
        let value = match node {
            Node::Constant(c) => c,
            Node::MontConstant(_) => {panic!("no montgomery constant expected in unoptimized graph")},
            Node::Input(i) => inputs[i],
            Node::Op(op, a, b) => op.eval(values[a], values[b]),
            Node::UnoOp(op, a) => op.eval(values[a]),
            Node::TresOp(op, a, b, c) => op.eval(values[a], values[b], values[c]),
        };
        values.push(value);

        let empty_vec: Vec<usize> = Vec::new();
        let signals_for_node: &Vec<usize>= node_idx_to_signal.get(&node_idx).unwrap_or(&empty_vec);

        let input_idx = signals_for_node.iter().map(|&i| i.to_string()).collect::<Vec<String>>().join(", ");

        let mut output_signals: Vec<usize> = Vec::new();
        for &signal_idx in signals_for_node {
            output_signals.extend(signal_to_witness.get(&signal_idx).unwrap_or(&empty_vec));
        }
        let output_signals = output_signals.iter().map(|&i| i.to_string()).collect::<Vec<String>>().join(", ");

        println!("[{:4}] {:>77} ({:>4}) ({:>4}) {:?}", node_idx, value.to_string(), input_idx, output_signals, node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calc_const_expression() {
        println!("OK");
    }
}
