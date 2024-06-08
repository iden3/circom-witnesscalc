use compiler::circuit_design::template::{TemplateCode};
use compiler::compiler_interface::{run_compiler, Circuit, Config};
use compiler::intermediate_representation::ir_interface::{AddressType, ComputeBucket, CreateCmpBucket, InputInformation, Instruction, InstructionPointer, LoadBucket, LocationRule, OperatorType, StatusInput, StoreBucket, ValueBucket, ValueType};
use compiler::intermediate_representation::InstructionList;
use constraint_generation::{build_circuit, BuildConfig};
use program_structure::error_definition::Report;
use ruint::aliases::U256;
use ruint::uint;
use std::collections::HashMap;
use std::env;
use std::fmt::{Display};
use std::path::PathBuf;
use type_analysis::check_types::check_types;
use witness::graph::Node::Constant;
use witness::graph::{optimize, Node, Operation};

pub const M: U256 =
    uint!(21888242871839275222246405745257275088548364400416034343698204186575808495617_U256);

fn print_instruction_list(il: &InstructionList) {
    for i in il.iter() {
        print_instruction(i);
    }
}

fn print_location_rule(lr: &LocationRule) {
    match lr {
        LocationRule::Indexed {
            location,
            template_header,
        } => {
            let s: String = template_header
                .as_ref()
                .map_or("-".to_string(), |v| v.clone());
            println!("[begin] Location Indexed: {}", s);
            print_instruction(&location);
            println!("[end] Location Indexed");
        }
        LocationRule::Mapped {
            signal_code,
            indexes,
        } => {
            println!("[begin] Location Mapped: {}", signal_code);
            print_instruction_list(&indexes);
            println!("[end] Location Mapped");
        }
    }
}

fn fmt_address_type() {}

fn fmt_operator_type(op: &OperatorType) -> &str {
    match op {
        OperatorType::Add => "Add",
        OperatorType::Mul => "Mul",
        OperatorType::Div => "Div",
        OperatorType::Sub => "Sub",
        OperatorType::Pow => "Pow",
        OperatorType::IntDiv => "IntDiv",
        OperatorType::Mod => "Mod",
        OperatorType::ShiftL => "ShiftL",
        OperatorType::ShiftR => "ShiftR",
        OperatorType::LesserEq => "LesserEq",
        OperatorType::GreaterEq => "GreaterEq",
        OperatorType::Lesser => "Lesser",
        OperatorType::Greater => "Greater",
        OperatorType::Eq(_) => "Eq",
        OperatorType::NotEq => "NotEq",
        OperatorType::BoolOr => "BoolOr",
        OperatorType::BoolAnd => "BoolAnd",
        OperatorType::BitOr => "BitOr",
        OperatorType::BitAnd => "BitAnd",
        OperatorType::BitXor => "BitXor",
        OperatorType::PrefixSub => "PrefixSub",
        OperatorType::BoolNot => "BoolNot",
        OperatorType::Complement => "Complement",
        OperatorType::ToAddress => "ToAddress",
        OperatorType::MulAddress => "MulAddress",
        OperatorType::AddAddress => "AddAddress",
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
    panic!("not implemented");
    0
}

fn int_from_value_instruction(value_bucket: &ValueBucket, nodes: &Vec<Node>) -> U256 {
    match value_bucket.parse_as {
        ValueType::BigInt => match nodes[value_bucket.value] {
            Constant(ref c) => c.clone(),
            _ => panic!("not a constant"),
        },
        ValueType::U32 => U256::from(value_bucket.value),
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
                        let signal_idx = if let Var::Constant(c) = signal_idx {
                            bigint_to_usize(&c)
                        } else {
                            panic!("signal index is not a constant")
                        };
                        let signal_idx = component_signal_start + signal_idx;
                        let signal_node = signal_node_idx[signal_idx];
                        assert_ne!(signal_node, usize::MAX, "signal is not set yet");
                        return signal_node;
                    }
                    LocationRule::Mapped {
                        signal_code,
                        indexes,
                    } => {
                        todo!()
                    }
                },
                AddressType::SubcmpSignal {
                    ref cmp_address, ..
                } => {
                    let subcomponent_idx = calc_expression(
                        cmp_address, nodes, vars, component_signal_start,
                        signal_node_idx, subcomponents);
                    let subcomponent_idx = if let Var::Constant(ref c) = subcomponent_idx {
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
                            let signal_idx = if let Var::Constant(ref c) = signal_idx {
                                bigint_to_usize(c)
                            } else {
                                panic!("signal index is not a constant")
                            };
                            let signal_offset = subcomponents[subcomponent_idx]
                                .as_ref()
                                .unwrap()
                                .signal_offset;
                            println!(
                                "Load subcomponent signal: [{}] {} + {} = {}",
                                subcomponent_idx,
                                signal_offset,
                                signal_idx,
                                signal_offset + signal_idx
                            );

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
            let node = build_node_from_instruction(
                inst,
                nodes,
                signal_node_idx,
                vars,
                component_signal_start,
                subcomponents,
            );
            nodes.push(node);
            return nodes.len() - 1;
        }
        Instruction::Value(ref value_bucket) => {
            match value_bucket.parse_as {
                ValueType::BigInt => match nodes[value_bucket.value] {
                    Constant(..) => {
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

fn build_node_from_instruction(
    inst: &InstructionPointer,
    nodes: &mut Vec<Node>,
    signal_node_idx: &mut Vec<usize>,
    vars: &mut Vec<Option<Var>>,
    component_signal_start: usize,
    subcomponents: &Vec<Option<ComponentInstance>>,
) -> Node {
    match **inst {
        Instruction::Compute(ref compute_bucket) => {
            match &compute_bucket.op {
                OperatorType::Add => {
                    assert_eq!(compute_bucket.stack.len(), 2);
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
                    return Node::Op(Operation::Add, arg1, arg2);
                }
                OperatorType::Mul => {
                    assert_eq!(compute_bucket.stack.len(), 2);
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
                    return Node::Op(Operation::Mul, arg1, arg2);
                }
                _ => {
                    panic!("not implemented: this operator is not supported to be converted to Node: {}", inst.to_string());
                }
            }
        }
        _ => {
            panic!(
                "not implemented: this instruction is not supported to be converted to Node: {}",
                inst.to_string()
            );
        }
    }
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
        Instruction::Value(ref value_bucket) => {
            panic!("not implemented");
        }
        Instruction::Load(ref load_bucket) => {
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
                            let node_idx = operator_argument_instruction(
                                &store_bucket.src,
                                nodes,
                                signal_node_idx,
                                vars,
                                component_signal_start,
                                subcomponents,
                            );
                            let signal_idx = calc_expression(
                                location, nodes, vars, component_signal_start,
                                signal_node_idx, subcomponents);
                            let signal_idx = if let Var::Constant(ref c) = signal_idx {
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
                            assert_eq!(
                                signal_node_idx[signal_idx],
                                usize::MAX,
                                "signal already set"
                            );
                            signal_node_idx[signal_idx] = node_idx;
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
                    ref is_output,
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
                    let subcomponent_idx = if let Var::Constant(ref c) = subcomponent_idx {
                        bigint_to_usize(&c)
                    } else {
                        panic!("subcomponent index is not a constant");
                    };

                    let node_idx = operator_argument_instruction(
                        &store_bucket.src,
                        nodes,
                        signal_node_idx,
                        vars,
                        component_signal_start,
                        subcomponents,
                    );

                    match store_bucket.dest {
                        LocationRule::Indexed {
                            ref location,
                            ref template_header,
                        } => {
                            let signal_idx = calc_expression(
                                location, nodes, vars, component_signal_start,
                                signal_node_idx, subcomponents);
                            let signal_idx = if let Var::Constant(ref c) = signal_idx {
                                bigint_to_usize(c)
                            } else {
                                panic!("signal index is not a constant");
                            };
                            let signal_offset = subcomponents[subcomponent_idx]
                                .as_ref()
                                .unwrap()
                                .signal_offset;
                            println!(
                                "Store subcomponent signal: [{}] {} + {} = {}",
                                subcomponent_idx,
                                signal_offset,
                                signal_idx,
                                signal_offset + signal_idx
                            );
                            let signal_idx = signal_offset + signal_idx;
                            assert_eq!(
                                signal_node_idx[signal_idx],
                                usize::MAX,
                                "subcomponent signal is already set"
                            );
                            signal_node_idx[signal_idx] = node_idx;
                            subcomponents[subcomponent_idx]
                                .as_mut()
                                .unwrap()
                                .number_of_inputs -= 1;
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
        Instruction::Compute(ref compute_bucket) => {
            panic!("not implemented");
        }
        Instruction::Call(_) => {
            panic!("not implemented");
        }
        Instruction::Branch(ref branch_bucket) => {
            if branch_bucket.if_branch.len() == 1 && branch_bucket.else_branch.len() == 1 {
                let v = calc_expression(
                    &branch_bucket.cond, nodes, vars, component_signal_start,
                    signal_node_idx, subcomponents);
                panic!("condition: {}", v.to_string());
            }
            // operator_argument_instruction()
            // branch_bucket.if_branch
            panic!(
                "not implemented: {} {}",
                branch_bucket.if_branch.len(),
                branch_bucket.else_branch.len()
            );
        }
        Instruction::Return(_) => {
            panic!("not implemented");
        }
        Instruction::Assert(_) => {
            panic!("not implemented");
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

            let cmp_idx = if let Var::Constant(ref c) = sub_cmp_id {
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
    let mut bytes = value.to_le_bytes::<32>().to_vec(); // Convert to little-endian bytes
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
        Var::Constant(ref c) => format!("Constant {}", c.to_string()),
        Var::Variable(idx) => format!("Variable {}", idx) };

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

fn variable_from_load_bucket(load_bucket: &LoadBucket, vars: &Vec<U256>) -> U256 {
    match load_bucket.address_type {
        AddressType::Variable => {}
        _ => {
            panic!("not a variable address type");
        }
    }
    match load_bucket.src {
        LocationRule::Indexed {
            ref location,
            ref template_header,
        } => {
            if template_header.is_some() {
                panic!("not implemented: template_header expected to be None");
            }
            let idx = value_from_instruction_usize(location);
            return vars[idx];
        }
        LocationRule::Mapped { .. } => {
            todo!()
        }
    }
}

#[derive(Clone)]
enum Var {
    Constant(U256),
    Variable(usize),
}

impl ToString for Var {
    fn to_string(&self) -> String {
        match self {
            Var::Constant(ref c) => { format!("Var::Constant({})", c.to_string()) }
            Var::Variable(idx) => { format!("Var::Variable({})", idx) }
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
                let signal_idx = if let Var::Constant(c) = signal_idx {
                    bigint_to_usize(&c)
                } else {
                    panic!("signal index is not a constant")
                };
                let signal_idx = component_signal_start + signal_idx;
                let signal_node = signal_node_idx[signal_idx];
                assert_ne!(signal_node, usize::MAX, "signal is not set yet");
                return Var::Variable(signal_node);
            }
            LocationRule::Mapped {
                signal_code,
                indexes,
            } => {
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
            let subcomponent_idx = if let Var::Constant(c) = subcomponent_idx {
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
                    let signal_idx = if let Var::Constant(c) = signal_idx {
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
                        "subcomponent signal is not set yet"
                    );
                    return Var::Variable(signal_node);
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
                let var_idx = if let Var::Constant(c) = var_idx {
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
        Var::Constant(ref a) => {
            Var::Constant(match compute_bucket.op {
                OperatorType::ToAddress => a.clone(),
                _ => {
                    todo!(
                        "unary operator not implemented: {}",
                        compute_bucket.op.to_string()
                    );
                }
            })
        },
        Var::Variable( .. ) => {
            panic!("not implemented");
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
        Var::Constant(ref c) => {
            nodes.push(Node::Constant(c.clone()));
            nodes.len() - 1
        }
        Var::Variable(idx) => { idx.clone() }
    };

    match (&a, &b) {
        (Var::Constant(ref a), Var::Constant(ref b)) => {
            Var::Constant(match compute_bucket.op {
                OperatorType::Lesser => if a < b { U256::from(1) } else { U256::ZERO }
                OperatorType::Add => a.add_mod(b.clone(), M),
                OperatorType::NotEq => U256::from(a != b),
                OperatorType::AddAddress => a + b,
                OperatorType::MulAddress => a * b,
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
                OperatorType::Lesser => Operation::Lt,
                OperatorType::Add => Operation::Add,
                OperatorType::NotEq => Operation::Neq,
                _ => {
                    todo!(
                        "operator not implemented: {}",
                        compute_bucket.op.to_string()
                    );
                }
            }, node_idx(&a), node_idx(&b));
            nodes.push(node);
            Var::Variable(nodes.len() - 1)
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
            Var::Constant(int_from_value_instruction(value_bucket, nodes))
        }
        Instruction::Load(ref load_bucket) => load(
            load_bucket,
            nodes,
            vars,
            component_signal_start,
            signal_node_idx,
            subcomponents,
        ),
        Instruction::Compute(ref compute_bucket) => match compute_bucket.op {
            OperatorType::Lesser | OperatorType::Add | OperatorType::NotEq | OperatorType::AddAddress | OperatorType::MulAddress => {
                build_binary_op_var(compute_bucket, nodes, vars, component_signal_start, signal_node_idx, subcomponents)
            }
            OperatorType::ToAddress => {
                build_unary_op_var(compute_bucket, nodes, vars, component_signal_start, signal_node_idx, subcomponents)
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
        Var::Constant(c) => c != U256::ZERO,
        _ => {
            panic!("continue condition is not a constant");
        }
    }
}

fn print_instruction(inst: &InstructionPointer) {
    // println!("statement: {}", inst.to_string());

    match **inst {
        Instruction::Value(ref value_bucket) => {
            println!(
                "Value {}/{}/{}",
                value_bucket.value,
                value_bucket.op_aux_no,
                value_bucket.parse_as.to_string()
            );
        }
        Instruction::Load(ref load_bucket) => {
            println!("[begin] Load");
            print_location_rule(&load_bucket.src);
            println!("[end] Load");
        }
        Instruction::Store(ref store_bucket) => {
            println!("[begin] Store");
            println!("SRC:");
            print_instruction(&store_bucket.src);
            println!("DST:");
            print_location_rule(&store_bucket.dest);
            println!("[end] Store");
        }
        Instruction::Compute(ref compute_bucket) => {
            println!("[begin] Compute {}", fmt_operator_type(&compute_bucket.op));
            for arg in &compute_bucket.stack {
                print_instruction(arg);
            }
            println!("[end] Compute {}", fmt_operator_type(&compute_bucket.op));
        }
        Instruction::Call(_) => {
            println!("Call");
        }
        Instruction::Branch(_) => {
            println!("Branch");
        }
        Instruction::Return(_) => {
            println!("Return");
        }
        Instruction::Assert(_) => {
            println!("assert");
        }
        Instruction::Log(_) => {
            println!("Log");
        }
        Instruction::Loop(_) => {
            println!("Log");
        }
        Instruction::CreateCmp(ref create_cmp_bucket) => {
            println!(
                "CreateCmp: signal_offset: {}, signal_offset_jump: {}, sub_cmp_id: {}",
                create_cmp_bucket.signal_offset,
                create_cmp_bucket.signal_offset_jump,
                create_cmp_bucket.sub_cmp_id.to_string()
            );
        }
    }
}

fn get_constants(circuit: &Circuit) -> Vec<Node> {
    let mut constants: Vec<Node> = Vec::new();
    for c in &circuit.c_producer.field_tracking {
        constants.push(Constant(U256::from_str_radix(c.as_str(), 10).unwrap()));
    }
    constants
}

fn init_input_signals(
    circuit: &Circuit,
    nodes: &mut Vec<Node>,
    signal_node_idx: &mut Vec<usize>,
) -> HashMap<String, (usize, usize)> {
    let input_list = circuit.c_producer.get_main_input_list();
    let mut signal_values: Vec<U256> = Vec::new();
    signal_values.push(U256::from(1));
    nodes.push(Node::Input(signal_values.len() - 1));
    signal_node_idx[0] = nodes.len() - 1;
    let mut inputs_info = HashMap::new();

    for (name, offset, len) in input_list {
        inputs_info.insert(name.clone(), (signal_values.len(), len.clone()));
        for i in 0..*len {
            signal_values.push(U256::ZERO);
            nodes.push(Node::Input(signal_values.len() - 1));
            signal_node_idx[offset + i] = nodes.len() - 1;
        }
    }

    return inputs_info;
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

    // print_instruction_list(&tmpl.body);

    let mut vars: Vec<Option<Var>> = vec![Option::None; tmpl.var_stack_depth];
    let mut components: Vec<Option<ComponentInstance>> = vec![];
    for i in 0..tmpl.number_of_components {
        components.push(None);
    }

    for (idx, inst) in tmpl.body.iter().enumerate() {
        println!("instruction #{}: {}", idx, inst.to_string());
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

fn parse_args() -> (String, Vec<PathBuf>) {
    let args: Vec<String> = env::args().collect();
    let mut i = 1;
    let mut circuit_file: Option<String> = None;
    let mut link_libraries: Vec<PathBuf> = Vec::new();
    while i < args.len() {
        if args[i] == "-l" {
            i += 1;
            if i >= args.len() {
                panic!("missing argument for -l");
            }
            link_libraries.push(args[i].clone().into());
        } else if args[i].starts_with("-l") {
            link_libraries.push(args[i][2..].to_string().into())
        } else if args[i].starts_with("-") {
            panic!("unknown argument: {}", args[i]);
        } else {
            circuit_file = Some(args[i].clone());
        }
        i += 1;
    };

    match circuit_file {
        Some(circuit_file) => (circuit_file, link_libraries),
        None => panic!("missing circuit file"),
    }
}

fn main() {
    let (circuit_file, link_libraries) = parse_args();

    let version = "2.1.9";

    // let main_file = "/Users/alek/src/simple-circuit/circuit3.circom";
    // let main_file = "/Users/alek/src/circuits/circuits/authV2.circom";
    // let link_libraries: Vec<PathBuf> =
    //     vec!["/Users/alek/src/circuits/node_modules/circomlib/circuits".into()];
    let parser_result = parser::run_parser(circuit_file, version, link_libraries);
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
    let input_signals = init_input_signals(&circuit, &mut nodes, &mut signal_node_idx);

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
    std::fs::write("graph_v2.bin", bytes).unwrap();

    println!("YAHOO")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calc_const_expression() {
        println!("OK");
    }
}
