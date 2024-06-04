use compiler::circuit_design::template::{TemplateCode, TemplateCodeInfo};
use compiler::compiler_interface::{run_compiler, Circuit, Config};
use compiler::intermediate_representation::ir_interface::{
    AddressType, CreateCmpBucket, InputInformation, Instruction, InstructionPointer, LoadBucket,
    LocationRule, OperatorType, StatusInput, StoreBucket, ValueType,
};
use compiler::intermediate_representation::InstructionList;
use constraint_generation::{build_circuit, BuildConfig};
use program_structure::error_definition::Report;
use ruint::aliases::U256;
use ruint::uint;
use std::path::PathBuf;
use type_analysis::check_types::check_types;
use witness::graph::Node::Constant;
use witness::graph::{Node, Operation};

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

fn int_from_value_instruction(inst: &InstructionPointer, nodes: &Vec<Node>) -> U256 {
    match **inst {
        Instruction::Value(ref value_bucket) => match value_bucket.parse_as {
            ValueType::BigInt => match nodes[value_bucket.value] {
                Constant(ref c) => c.clone(),
                _ => panic!("not a constant"),
            },
            ValueType::U32 => U256::from(value_bucket.value),
        },
        _ => {
            panic!("not a value instruction: {}", inst.to_string())
        }
    }
}

fn operator_argument_instruction(
    inst: &InstructionPointer,
    nodes: &mut Vec<Node>,
    signal_node_idx: &mut Vec<usize>,
    vars: &mut Vec<U256>,
    component_signal_start: usize,
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
                        let signal_idx = calc_const_expression(location, nodes, vars);
                        let signal_idx = bigint_to_usize(signal_idx);
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
                _ => {
                    panic!("not implemented");
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
            );
            nodes.push(node);
            return nodes.len() - 1;
        }
        _ => {
            panic!("not implemented");
        }
    }
}

fn build_node_from_instruction(
    inst: &InstructionPointer,
    nodes: &mut Vec<Node>,
    signal_node_idx: &mut Vec<usize>,
    vars: &mut Vec<U256>,
    component_signal_start: usize,
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
                    );
                    let arg2 = operator_argument_instruction(
                        &compute_bucket.stack[1],
                        nodes,
                        signal_node_idx,
                        vars,
                        component_signal_start,
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
                    );
                    let arg2 = operator_argument_instruction(
                        &compute_bucket.stack[1],
                        nodes,
                        signal_node_idx,
                        vars,
                        component_signal_start,
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
    vars: &mut Vec<U256>,
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
            // println!(
            //     "dest addr type: {:?}",
            //     match store_bucket.dest_address_type {
            //         AddressType::Variable => "Variable",
            //         AddressType::Signal => "Signal",
            //         AddressType::SubcmpSignal { .. } => "SubcmpSignal",
            //     }
            // );
            // println!(
            //     "location rule: {:?}",
            //     match store_bucket.dest {
            //         LocationRule::Indexed { .. } =>
            //             format!("Indexed: {}", store_bucket.dest.to_string()),
            //         LocationRule::Mapped { .. } => "Mapped".to_string(),
            //     }
            // );
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
                            );
                            let signal_idx = calc_const_expression(location, nodes, vars);
                            let signal_idx = bigint_to_usize(signal_idx);
                            let signal_idx = component_signal_start + signal_idx;
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
                            vars[lvar_idx] = calc_const_expression(&store_bucket.src, nodes, vars);
                            // vars[lvar_idx] = int_from_value_instruction(&store_bucket.src, nodes);
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
                    let subcomponent_idx = calc_const_expression(cmp_address, nodes, vars);
                    let subcomponent_idx = bigint_to_usize(subcomponent_idx);
                    print!(
                        "cmp_address = {}, is_output: {}, status: {}",
                        subcomponent_idx,
                        is_output,
                        match input_status {
                            StatusInput::Last => "Last",
                            StatusInput::NoLast => "NoLast",
                            StatusInput::Unknown => "Unknown",
                        }
                    );

                    let node_idx = operator_argument_instruction(
                        &store_bucket.src,
                        nodes,
                        signal_node_idx,
                        vars,
                        component_signal_start,
                    );

                    match store_bucket.dest {
                        LocationRule::Indexed {
                            ref location,
                            ref template_header,
                        } => {
                            let signal_idx = calc_const_expression(location, nodes, vars);
                            let signal_idx = bigint_to_usize(signal_idx);
                            let signal_idx = &subcomponents[subcomponent_idx]
                                .as_ref()
                                .unwrap()
                                .signal_offset
                                + signal_idx;
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

                    let mut run = false;
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
                        StatusInput::Unknown => number_of_inputs == 0
                    };

                    if run_component {
                        run_template(
                            templates,
                            subcomponents[subcomponent_idx].as_ref().unwrap().template_id,
                            nodes,
                            signal_node_idx,
                            subcomponents[subcomponent_idx].as_ref().unwrap().signal_offset)
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
        Instruction::Branch(_) => {
            panic!("not implemented");
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
            if check_continue_condition(&loop_bucket.continue_condition, nodes, vars) {
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
            let sub_cmp_id =
                calc_const_expression(&create_component_bucket.sub_cmp_id, nodes, vars);

            let cmp_idx = bigint_to_usize(sub_cmp_id);
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
                fmt_create_cmp_bucket(create_component_bucket, nodes, vars)
            );
        }
    }
}

fn bigint_to_usize(value: U256) -> usize {
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
    nodes: &Vec<Node>,
    vars: &Vec<U256>,
) -> String {
    let sub_cmp_id = calc_const_expression(&cmp_bucket.sub_cmp_id, nodes, vars);
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

// This function should calculate node based only on constant or variable
// values. Not based on signal values.
fn calc_const_expression(inst: &InstructionPointer, nodes: &Vec<Node>, vars: &Vec<U256>) -> U256 {
    match **inst {
        Instruction::Value(ref value_bucket) => int_from_value_instruction(inst, nodes),
        Instruction::Load(ref load_bucket) => variable_from_load_bucket(load_bucket, vars),
        Instruction::Compute(ref compute_bucket) => match compute_bucket.op {
            OperatorType::Lesser => {
                assert_eq!(compute_bucket.stack.len(), 2);
                let arg1 = calc_const_expression(&compute_bucket.stack[0], nodes, vars);
                let arg2 = calc_const_expression(&compute_bucket.stack[1], nodes, vars);
                if arg1 < arg2 {
                    U256::from(1)
                } else {
                    U256::ZERO
                }
            }
            OperatorType::Add => {
                assert_eq!(compute_bucket.stack.len(), 2);
                let arg1 = calc_const_expression(&compute_bucket.stack[0], nodes, vars);
                let arg2 = calc_const_expression(&compute_bucket.stack[1], nodes, vars);
                arg1.add_mod(arg2, M)
            }
            OperatorType::AddAddress => {
                assert_eq!(compute_bucket.stack.len(), 2);
                let arg1 = calc_const_expression(&compute_bucket.stack[0], nodes, vars);
                let arg2 = calc_const_expression(&compute_bucket.stack[1], nodes, vars);
                arg1 + arg2
            }
            OperatorType::MulAddress => {
                assert_eq!(compute_bucket.stack.len(), 2);
                let arg1 = calc_const_expression(&compute_bucket.stack[0], nodes, vars);
                let arg2 = calc_const_expression(&compute_bucket.stack[1], nodes, vars);
                arg1 * arg2
            }
            OperatorType::ToAddress => {
                assert_eq!(compute_bucket.stack.len(), 1);
                let arg1 = calc_const_expression(&compute_bucket.stack[0], nodes, vars);
                arg1
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
                "unable to evaluate constant instruction: {}",
                inst.to_string()
            );
        }
    }
}

fn check_continue_condition(
    inst: &InstructionPointer,
    nodes: &Vec<Node>,
    vars: &Vec<U256>,
) -> bool {
    let val = calc_const_expression(inst, nodes, vars);
    return val != U256::ZERO;
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

// Read input signals info from circuit and append them to the nodes list
fn append_signals(circuit: &Circuit, nodes: &mut Vec<Node>) -> Vec<U256> {
    let input_list = circuit.c_producer.get_main_input_list();
    let mut signal_values: Vec<U256> = Vec::new();
    signal_values.push(U256::from(1));
    nodes.push(Node::Input(signal_values.len() - 1));
    for (name, offset, len) in input_list {
        for i in 0..*len {
            signal_values.push(U256::ZERO);
            nodes.push(Node::Input(signal_values.len() - 1));
        }
    }
    return signal_values;
}

fn init_input_signals(
    circuit: &Circuit,
    nodes: &mut Vec<Node>,
    signal_node_idx: &mut Vec<usize>,
) -> Vec<U256> {
    let input_list = circuit.c_producer.get_main_input_list();
    let mut signal_values: Vec<U256> = Vec::new();
    signal_values.push(U256::from(1));
    nodes.push(Node::Input(signal_values.len() - 1));
    signal_node_idx[0] = nodes.len() - 1;

    for (name, offset, len) in input_list {
        for i in 0..*len {
            signal_values.push(U256::ZERO);
            nodes.push(Node::Input(signal_values.len() - 1));
            signal_node_idx[offset + i] = nodes.len() - 1;
        }
    }

    return signal_values;
}

fn run_template(
    templates: &Vec<TemplateCode>,
    template_id: usize,
    nodes: &mut Vec<Node>,
    signal_node_idx: &mut Vec<usize>,
    component_signal_start: usize,
) {
    let tmpl = &templates[template_id];

    println!("Run template #{}: {}, body length: {}", tmpl.id, tmpl.name, tmpl.body.len());

    // print_instruction_list(&tmpl.body);

    let mut vars = vec![U256::ZERO; tmpl.var_stack_depth];
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

fn main() {
    let version = "2.1.9";

    let main_file = "/Users/alek/src/simple-circuit/circuit4.circom";
    // let main_file = "/Users/alek/src/circuits/circuits/authV2.circom";
    let link_libraries: Vec<PathBuf> =
        vec!["/Users/alek/src/circuits/node_modules/circomlib/circuits".into()];
    let parser_result = parser::run_parser(main_file.to_string(), version, link_libraries);
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
    append_signals(&circuit, &mut nodes);
    let signal_values = init_input_signals(&circuit, &mut nodes, &mut signal_node_idx);

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
}
