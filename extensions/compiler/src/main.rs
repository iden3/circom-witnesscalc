use std::{env, fs};
use std::path::PathBuf;
use core::convert::TryInto;
use std::cell::RefCell;
use std::collections::{HashMap};
use std::fs::File;
use std::io::Write;
use std::rc::Rc;
use std::time::Instant;
use compiler::circuit_design::function::FunctionCode;
use compiler::circuit_design::template::TemplateCode;
use compiler::compiler_interface::{run_compiler, Circuit, Config};
use compiler::intermediate_representation::InstructionList;
use compiler::intermediate_representation::ir_interface::{AccessType, AddressType, ComputeBucket, CreateCmpBucket, InputInformation, Instruction, InstructionPointer, LoadBucket, LocationRule, ObtainMeta, OperatorType, ReturnType, SizeOption, StatusInput, ValueType};
use constraint_generation::{build_circuit, BuildConfig};
use program_structure::error_definition::Report;
use program_structure::constants::UsefulConstants;
use ruint::aliases::U256;
use type_analysis::check_types::check_types;
use circom_witnesscalc::{deserialize_inputs, wtns_from_u256_witness};
use circom_witnesscalc::graph::Operation;
use circom_witnesscalc::storage::{serialize_witnesscalc_vm, CompiledCircuit, init_input_signals, TemplateInstanceIOMap, IODef};
use circom_witnesscalc::vm::{Function, Template, execute, build_component, ComponentTmpl, InputStatus, disassemble_instruction, OpCode};

struct Args {
    circuit_file: String,
    inputs_file: Option<String>,
    vm_out_file: String,
    link_libraries: Vec<PathBuf>,
    print_debug: bool,
    witness_file: Option<String>,
    expected_signals: Option<Vec<U256>>,
}

#[derive(Debug)]
enum CompilationError {
    NotAnExpression(()),
}

enum U32OrExpression {
    U32(u32),
    BigInt(U256),
    Expression,
}

fn expect_single_size(size: &SizeOption) -> usize {
    match size {
        SizeOption::Single(value) => *value,
        SizeOption::Multiple(values) => {
            panic!("size depends on template at runtime: {:?}", values);
        }
    }
}

fn expect_single_size_u32(size: &SizeOption) -> u32 {
    let v = expect_single_size(size);
    v.try_into().expect("size does not fit into u32")
}

fn u32_or_expression(
    inst: &InstructionPointer,
    constants: &[U256]) -> Result<U32OrExpression, CompilationError> {

    match **inst {
        Instruction::Value(ref value) => {
            match value.parse_as {
                ValueType::U32 => {
                    // maybe remove this
                    // if value.value > u32::MAX as usize {
                    //     return Ok(U32OrExpression::Expression);
                    // }
                    let v: u32 = value.value.try_into()
                        .expect("value is too large for stack_u32");
                    Ok(U32OrExpression::U32(v))
                },
                ValueType::BigInt => {
                    Ok(U32OrExpression::BigInt(constants[value.value]))
                },
            }
        }
        Instruction::Load(_) => Ok(U32OrExpression::Expression),
        Instruction::Compute(ref compute_bucket) => {

            let two_op_fn = |op: Operation| -> Result<U32OrExpression, CompilationError> {
                assert_eq!(2, compute_bucket.stack.len());
                let a = match u32_or_expression(&compute_bucket.stack[0], constants)? {
                    U32OrExpression::U32(_) => panic!("expected BigInt value"),
                    U32OrExpression::BigInt(v) => v,
                    U32OrExpression::Expression => { return Ok(U32OrExpression::Expression) }
                };
                let b = match u32_or_expression(&compute_bucket.stack[1], constants)? {
                    U32OrExpression::U32(_) => panic!("expected BigInt value"),
                    U32OrExpression::BigInt(v) => v,
                    U32OrExpression::Expression => { return Ok(U32OrExpression::Expression) }
                };
                Ok(U32OrExpression::BigInt(op.eval(a, b)))
            };

            match compute_bucket.op {
                OperatorType::Mul => two_op_fn(Operation::Mul),
                OperatorType::Div => {todo!()}
                OperatorType::Add => two_op_fn(Operation::Add),
                OperatorType::Sub => two_op_fn(Operation::Sub),
                OperatorType::Pow => {todo!()}
                OperatorType::IntDiv => {two_op_fn(Operation::Idiv)}
                OperatorType::Mod => two_op_fn(Operation::Mod),
                OperatorType::ShiftL => {todo!()}
                OperatorType::ShiftR => {todo!()}
                OperatorType::LesserEq => {todo!()}
                OperatorType::GreaterEq => {todo!()}
                OperatorType::Lesser => {todo!()}
                OperatorType::Greater => {todo!()}
                OperatorType::Eq(_) => {todo!()}
                OperatorType::NotEq => {todo!()}
                OperatorType::BoolOr => {todo!()}
                OperatorType::BoolAnd => {todo!()}
                OperatorType::BitOr => {todo!()}
                OperatorType::BitAnd => {todo!()}
                OperatorType::BitXor => {todo!()}
                OperatorType::PrefixSub => {todo!()}
                OperatorType::BoolNot => {todo!()}
                OperatorType::Complement => {todo!()}
                OperatorType::ToAddress => {
                    assert_eq!(1, compute_bucket.stack.len());
                    match u32_or_expression(&compute_bucket.stack[0], constants)? {
                        U32OrExpression::U32(_) => panic!("expected big integer value"),
                        U32OrExpression::BigInt(v) => {
                            let v = bigint_to_u64(v)
                                .expect("value is too large for stack_u32");
                            let v: u32 = v.try_into()
                                .expect("value is too large for stack_u32");
                            Ok(U32OrExpression::U32(v))
                        },
                        U32OrExpression::Expression => Ok(U32OrExpression::Expression)
                    }
                }
                OperatorType::MulAddress => {
                    assert_eq!(2, compute_bucket.stack.len());
                    let a = match u32_or_expression(&compute_bucket.stack[0], constants)? {
                        U32OrExpression::U32(v) => v,
                        U32OrExpression::BigInt(_) => panic!("expected u32 value"),
                        U32OrExpression::Expression => { return Ok(U32OrExpression::Expression) }
                    };
                    let b = match u32_or_expression(&compute_bucket.stack[1], constants)? {
                        U32OrExpression::U32(v) => v,
                        U32OrExpression::BigInt(_) => panic!("expected u32 value"),
                        U32OrExpression::Expression => { return Ok(U32OrExpression::Expression) }
                    };
                    let (v, overflow) = a.overflowing_mul(b);
                    if overflow {
                        panic!("value is too large for stack_u32");
                    }
                    Ok(U32OrExpression::U32(v))
                }
                OperatorType::AddAddress => {
                    assert_eq!(2, compute_bucket.stack.len());
                    let a = match u32_or_expression(&compute_bucket.stack[0], constants)? {
                        U32OrExpression::U32(v) => v,
                        U32OrExpression::BigInt(_) => panic!("expected u32 value"),
                        U32OrExpression::Expression => { return Ok(U32OrExpression::Expression) }
                    };
                    let b = match u32_or_expression(&compute_bucket.stack[1], constants)? {
                        U32OrExpression::U32(v) => v,
                        U32OrExpression::BigInt(_) => panic!("expected u32 value"),
                        U32OrExpression::Expression => { return Ok(U32OrExpression::Expression) }
                    };
                    let (v, overflow) = a.overflowing_add(b);
                    if overflow {
                        panic!("value is too large for stack_u32");
                    }
                    Ok(U32OrExpression::U32(v))
                }
            }
        },
        _ => {
            Err(CompilationError::NotAnExpression(()))
        }
    }
}

fn parse_args() -> Args {
    let args: Vec<String> = env::args().collect();
    let mut i = 1;
    let mut circuit_file: Option<String> = None;
    let mut vm_out_file: Option<String> = None;
    let mut link_libraries: Vec<PathBuf> = Vec::new();
    let mut inputs_file: Option<String> = None;
    let mut witness_file: Option<String> = None;
    let mut print_debug = false;
    let mut expected_signals: Option<Vec<U256>> = None;

    let usage = |err_msg: &str| -> String {
        eprintln!("{}", err_msg);
        eprintln!("Usage: {} <circuit_file> <vm_out_file> [-l <link_library>]* [-i <inputs_file.json>] [-v] [-wtns <output.wtns]", args[0]);
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
        } else if args[i] == "-wtns" {
            i += 1;
            if i >= args.len() {
                usage("missing argument for -wtns");
            }
            if witness_file.is_some() {
                usage("multiple witness files");
            }
            witness_file = Some(args[i].clone());
        } else if args[i] == "-i" {
            i += 1;
            if i >= args.len() {
                usage("missing argument for -i");
            }
            if inputs_file.is_some() {
                usage("multiple inputs files");
            }
            inputs_file = Some(args[i].clone());
        } else if args[i].starts_with("-i") {
            if inputs_file.is_some() {
                usage("multiple inputs files");
            }
            inputs_file = Some(args[i][2..].to_string());
        } else if args[i] == "-v" {
            print_debug = true;
        } else if args[i] == "-expected-signals" {
            i += 1;
            if i >= args.len() {
                usage("missing argument for -expected-signals");
            }
            expected_signals = Some(load_signals_txt(&args[i]));
        } else if args[i].starts_with("-") {
            let message = format!("unknown argument: {}", args[i]);
            usage(&message);
        } else if circuit_file.is_none() {
            circuit_file = Some(args[i].clone());
        } else if vm_out_file.is_none() {
            vm_out_file = Some(args[i].clone());
        } else {
            usage(format!("unexpected argument: {}", args[i]).as_str());
        }
        i += 1;
    };

    Args {
        circuit_file: circuit_file.unwrap_or_else(|| { usage("missing circuit file") }),
        inputs_file,
        vm_out_file: vm_out_file.unwrap_or_else(|| { usage("missing vm_out_file file") }),
        link_libraries,
        print_debug,
        witness_file,
        expected_signals,
    }
}


fn load_signals_txt(file_name: &str) -> Vec<U256> {
    let content = fs::read_to_string(file_name)
        .expect("failed to read signals file");
    let mut signals = Vec::new();
    for line in content.lines() {
        let signal = U256::from_str_radix(line, 10)
            .expect("failed to parse signal");
        signals.push(signal);
    }
    signals
}


fn main() {
    let args = parse_args();

    if let Some(ref expected_signals) = args.expected_signals {
        println!("Expected signals:");
        for (i, s) in expected_signals.iter().enumerate() {
            println!("{}: {}", i, s);
        }
    }

    let version = "2.1.9";
    let prime_name = String::from("bn128");
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

    // let (input_signals, input_signal_values): (InputSignalsInfo, Vec<U256>) = init_input_signals(
    //     &circuit, &mut nodes, &mut signal_node_idx, args.inputs_file);

    // assert that template id is equal to index in templates list
    for (i, t) in circuit.templates.iter().enumerate() {
        assert_eq!(i, t.id);
        if args.print_debug {
            println!("Template #{}: {}", i, t.name);
        }
    }

    let constants = get_constants(&circuit);
    let main_component_signal_start = 1usize;
    let start = Instant::now();
    let (compiled_templates, compiled_functions) =
        compile(&circuit.templates, &circuit.functions, &constants);

    if args.print_debug {
        for (i, t) in compiled_templates.iter().enumerate() {
            println!("Compiled template #{}: {}", i, t.name);
            disassemble(
                &t.code, &t.line_numbers, t.name.as_str(), &compiled_functions);
        }

        for (i, t) in compiled_functions.iter().enumerate() {
            println!("Compiled function #{}: {}", i, t.name);
            disassemble(&t.code, &t.line_numbers, &t.name, &compiled_functions);
        }
    }

    println!("Compilation time: {:?}", start.elapsed());

    let sigs_num = circuit.c_producer.get_total_number_of_signals();

    let io_map: TemplateInstanceIOMap = circuit.c_producer.get_io_map().iter()
        .map(|(k, v)|(*k, v.iter()
            .map(|x| IODef {
                code: x.code,
                offset: x.offset,
                lengths: x.lengths.clone()
            })
            .collect()))
        .collect();

    let producer_inputs = circuit.c_producer.get_main_input_list();
    let main_inputs: Vec<(String, usize, usize)> = producer_inputs.iter()
        .map(|input| (input.name.clone(), input.start, input.size))
        .collect();

    let cs: CompiledCircuit = CompiledCircuit {
        main_template_id,
        templates: compiled_templates,
        functions: compiled_functions,
        signals_num: sigs_num,
        constants,
        inputs: main_inputs.clone(),
        witness_signals: witness_list,
        io_map,
    };

    if let Some(input_file) = args.inputs_file {

        if let Some(ref expected_signals) = args.expected_signals {
            assert_eq!(expected_signals.len(), sigs_num);
        }

        println!("Run VM");
        let start = Instant::now();

        let main_component = build_component(
            &cs.templates, main_template_id, main_component_signal_start);
        let main_component = Rc::new(RefCell::new(main_component));

        let mut signals2 = Vec::new();
        signals2.resize(sigs_num, None);

        let inputs_data = fs::read(&input_file)
            .expect("Failed to read input file");
        let inputs = deserialize_inputs(&inputs_data)
            .unwrap();
        init_input_signals(&main_inputs, &inputs, &mut signals2);

        if let Err(err) = execute(
            main_component, &cs.templates, &cs.functions, &cs.constants,
            &mut signals2, &cs.io_map, args.expected_signals.as_ref()) {
            eprintln!("VM execution failed: {}", err);
            std::process::exit(1);
        }

        println!("Execution time: {:?}", start.elapsed());

        if let Some(witness_file) = args.witness_file {
            let mut witness = Vec::with_capacity(cs.witness_signals.len());
            for w in cs.witness_signals.iter() {
                witness.push(signals2[*w].unwrap());
            }

            let wtns_bytes = wtns_from_u256_witness(witness);

            {
                let mut f = File::create(witness_file).unwrap();
                f.write_all(&wtns_bytes).unwrap();
            }
        } else {
            println!("Witness file is not set. Witness was calculated but not saved.")
        }
    }

    let mut f = fs::File::create(&args.vm_out_file).unwrap();
    serialize_witnesscalc_vm(&mut f, &cs).unwrap();
}

fn get_constants(circuit: &Circuit) -> Vec<U256> {
    let mut constants = Vec::new();
    for c in &circuit.c_producer.field_tracking {
        constants.push(U256::from_str_radix(c.as_str(), 10).unwrap());
    }
    constants
}

fn calc_jump_offset(from: usize, to: usize) -> i32 {
    let from: i64 = from.try_into().expect("Jump from offset is too large");
    let to: i64 = to.try_into().expect("Jump to offset is too large");

    (to - from).try_into().expect("Jump offset is too large")
}

fn emit_jump(code: &mut Vec<u8>, to: usize) {
    code.push(OpCode::Jump as u8);

    let jump_offset_addr = code.len();
    let offset = calc_jump_offset(jump_offset_addr + 4, to);

    code.extend_from_slice(offset.to_le_bytes().as_ref());
}


fn pre_emit_jump_if_false(code: &mut Vec<u8>) -> usize {
    code.push(OpCode::JumpIfFalse as u8);
    for _ in 0..4 { code.push(0xffu8); }
    code.len() - 4
}

fn pre_emit_jump(code: &mut Vec<u8>) -> usize {
    code.push(OpCode::Jump as u8);
    for _ in 0..4 { code.push(0xffu8); }
    code.len() - 4
}

/// We expect the jump offset located at `jump_offset_addr` to be 4 bytes long.
/// The jump offset is calculated as `to - jump_offset_addr - 4`.
fn patch_jump(code: &mut [u8], jump_offset_addr: usize, to: usize) {
    let offset = calc_jump_offset(jump_offset_addr + 4, to);
    code[jump_offset_addr..jump_offset_addr+4].copy_from_slice(offset.to_le_bytes().as_ref());
}

fn into_input_status(status: &StatusInput) -> InputStatus {
    match status {
        StatusInput::Last => InputStatus::Last,
        StatusInput::NoLast => InputStatus::NoLast,
        StatusInput::Unknown => InputStatus::Unknown,
    }
}

fn store_subsignal(
    ctx: &mut TmplCompilationCtx, dest: &LocationRule, line_no: usize,
    cmp_address: &InstructionPointer, input_information: &InputInformation,
    signals_num: u32) {

    let mut indexes_number = 0u32;
    let mut signal_code = 0u32;
    let is_signal_idx_mapped = match dest {
        LocationRule::Indexed { location, .. } => {
            let sig_idx =
                u32_or_expression(location, ctx.constants).unwrap();

            match sig_idx {
                U32OrExpression::U32(sig_idx) => {
                    ctx.code.push(OpCode::Push4 as u8);
                    ctx.line_numbers.push(line_no);

                    ctx.code.extend_from_slice(sig_idx.to_le_bytes().as_ref());
                    for _ in 0..4 { ctx.line_numbers.push(usize::MAX); }
                }
                U32OrExpression::BigInt(_) => {
                    panic!("Signal index is not u32");
                }
                U32OrExpression::Expression => {
                    expression_u32(
                        location, &mut ctx.code, ctx.constants,
                        &mut ctx.line_numbers, &ctx.components);
                }
            };

            false
        }
        LocationRule::Mapped { signal_code: signal_code_local, indexes } => {
            for access in indexes {
                match access {
                    AccessType::Indexed(index_info) => {
                        for idx_inst in &index_info.indexes {
                            expression_u32(
                                idx_inst, &mut ctx.code, ctx.constants,
                                &mut ctx.line_numbers, &ctx.components);
                            indexes_number = indexes_number
                                .checked_add(1)
                                .expect("Too many indexes");
                        }
                    }
                    AccessType::Qualified(_) => {
                        panic!("bus accesses are not supported in VM compiler");
                    }
                }
            }

            signal_code = (*signal_code_local).try_into()
                .expect("Too large signal code");

            true
        }
    };
    let cmp_idx =
        u32_or_expression(cmp_address, ctx.constants);
    match cmp_idx {
        Ok(ref cmp_idx) => {
            match cmp_idx {
                U32OrExpression::U32(cmp_idx) => {
                    ctx.code.push(OpCode::Push4 as u8);
                    ctx.line_numbers.push(line_no);

                    ctx.code.extend_from_slice(cmp_idx.to_le_bytes().as_ref());
                    for _ in 0..4 { ctx.line_numbers.push(usize::MAX); }
                },
                U32OrExpression::BigInt(_) => {
                    panic!("Component index is not u32");
                },
                U32OrExpression::Expression => {
                    expression_u32(
                        cmp_address, &mut ctx.code, ctx.constants,
                        &mut ctx.line_numbers, &ctx.components);
                }
            }
        }
        Err(e) => {
            panic!("Error: {:?}", e);
        }
    };

    ctx.code.push(OpCode::SetSubSignal as u8);
    ctx.line_numbers.push(line_no);

    ctx.code.extend_from_slice(signals_num.to_le_bytes().as_ref());
    for _ in 0..4 { ctx.line_numbers.push(usize::MAX); }

    let input_status: InputStatus = if let InputInformation::Input{ status, .. } = input_information {
        into_input_status(status)
    } else {
        panic!("Can't store signal to non-input subcomponent");
    };
    ctx.code.push(mk_flags(input_status, is_signal_idx_mapped));
    ctx.line_numbers.push(usize::MAX);

    if is_signal_idx_mapped {
        ctx.code.extend_from_slice(indexes_number.to_le_bytes().as_ref());
        for _ in 0..4 { ctx.line_numbers.push(usize::MAX); }

        ctx.code.extend_from_slice(signal_code.to_le_bytes().as_ref());
        for _ in 0..4 { ctx.line_numbers.push(usize::MAX); }
    }
}

// After statement execution, there should not be side effect on the stack
fn statement(
    ctx: &mut TmplCompilationCtx, inst: &InstructionPointer,
    fn_registry: &mut FnRegistry) {

    // println!("statement: {}", inst.to_string());

    match **inst {
        Instruction::Store(ref store_bucket) => {

            expression(
                &store_bucket.src, &mut ctx.code, ctx.constants,
                &mut ctx.line_numbers, &ctx.components);
            assert_eq!(ctx.line_numbers.len(), ctx.code.len());

            match store_bucket.dest_address_type {
                AddressType::Variable => {
                    let location = if let LocationRule::Indexed{ref location, ..} = store_bucket.dest {
                        location
                    } else {
                        panic!("Variable destination should be of Indexed type");
                    };

                    let var_idx =
                        u32_or_expression(location, ctx.constants).unwrap();

                    match var_idx {
                        U32OrExpression::U32(var_idx) => {
                            ctx.code.push(OpCode::SetVariable4 as u8);
                            ctx.line_numbers.push(store_bucket.line);
                            ctx.code.extend_from_slice(var_idx.to_le_bytes().as_ref());
                            for _ in 0..4 { ctx.line_numbers.push(usize::MAX); }
                        }
                        U32OrExpression::BigInt(_) => {
                            panic!("Variable index is not u32");
                        }
                        U32OrExpression::Expression => {
                            expression_u32(
                                location, &mut ctx.code, ctx.constants,
                                &mut ctx.line_numbers, &ctx.components);
                            ctx.code.push(OpCode::SetVariable as u8);
                            ctx.line_numbers.push(store_bucket.line);
                        }
                    }

                    let signals_num = expect_single_size_u32(
                        &store_bucket.context.size);
                    ctx.code.extend_from_slice(signals_num.to_le_bytes().as_ref());
                    for _ in 0..4 { ctx.line_numbers.push(usize::MAX); }
                }

                AddressType::Signal => {
                    let location = if let LocationRule::Indexed{ref location, ..} = store_bucket.dest {
                        location
                    } else {
                        panic!("Signal destination should be of Indexed type");
                    };

                    let sig_idx =
                        u32_or_expression(location, ctx.constants).unwrap();

                    match sig_idx {
                        U32OrExpression::U32(sig_idx) => {
                            ctx.code.push(OpCode::SetSelfSignal4 as u8);
                            ctx.line_numbers.push(store_bucket.line);
                            ctx.code.extend_from_slice(sig_idx.to_le_bytes().as_ref());
                            for _ in 0..4 { ctx.line_numbers.push(usize::MAX); }
                        }
                        U32OrExpression::BigInt(_) => {
                            panic!("Signal index is not u32");
                        }
                        U32OrExpression::Expression => {
                            expression_u32(
                                location, &mut ctx.code, ctx.constants,
                                &mut ctx.line_numbers, &ctx.components);
                            ctx.code.push(OpCode::SetSelfSignal as u8);
                            ctx.line_numbers.push(store_bucket.line);
                        }
                    }

                    let signals_num = expect_single_size_u32(
                        &store_bucket.context.size);
                    ctx.code.extend_from_slice(signals_num.to_le_bytes().as_ref());
                    for _ in 0..4 { ctx.line_numbers.push(usize::MAX); }
                }

                AddressType::SubcmpSignal { ref cmp_address, ref input_information, .. } => {
                    let signals_num = expect_single_size_u32(
                        &store_bucket.context.size);

                    store_subsignal(
                        ctx, &store_bucket.dest, store_bucket.get_line(),
                        cmp_address, input_information, signals_num);
                }
            }
            assert_eq!(ctx.line_numbers.len(), ctx.code.len());
        }
        Instruction::Loop(ref loop_bucket) => {
            let loop_start = ctx.code.len();

            expression(
                &loop_bucket.continue_condition, &mut ctx.code, ctx.constants,
                &mut ctx.line_numbers, &ctx.components);

            let loop_end_jump_offset =
                pre_emit_jump_if_false(&mut ctx.code);
            ctx.line_numbers.push(loop_bucket.line);
            ctx.line_numbers.push(usize::MAX);
            ctx.line_numbers.push(usize::MAX);
            ctx.line_numbers.push(usize::MAX);
            ctx.line_numbers.push(usize::MAX);

            block(ctx, &loop_bucket.body, fn_registry);

            emit_jump(&mut ctx.code, loop_start);
            ctx.line_numbers.push(loop_bucket.line);
            ctx.line_numbers.push(usize::MAX);
            ctx.line_numbers.push(usize::MAX);
            ctx.line_numbers.push(usize::MAX);
            ctx.line_numbers.push(usize::MAX);

            let to = ctx.code.len();

            patch_jump(&mut ctx.code, loop_end_jump_offset, to);
            assert_eq!(ctx.line_numbers.len(), ctx.code.len());
        }
        Instruction::CreateCmp(ref cmp_bucket) => {
            let sub_cmp_idx = u32_or_expression(
                &cmp_bucket.sub_cmp_id, ctx.constants).unwrap();

            let sub_cmp_idx = if let U32OrExpression::U32(sub_cmp_idx) = sub_cmp_idx {
                sub_cmp_idx
            } else {
                panic!("Subcomponent index suppose to be a constant");
            };

            // let want_defined_positions: Vec<(usize, bool)> = vec![(0, false)];
            // assert_eq!(cmp_bucket.defined_positions, want_defined_positions); // TODO

            if cmp_bucket.is_part_mixed_array_not_uniform_parallel {
                todo!();
            }

            // TODO
            assert!(matches!(cmp_bucket.uniform_parallel, Some(false)));

            // if cmp_bucket.dimensions.len() > 0 {
            //     todo!("{:?}: {}", cmp_bucket.dimensions, cmp_bucket.to_string());
            // }
            // if cmp_bucket.component_offset != 0 {
            //     todo!();
            // }
            // if cmp_bucket.component_offset_jump != 1 {
            //     todo!("{:?}: {}", cmp_bucket.component_offset_jump, cmp_bucket.to_string());
            // }
            // if cmp_bucket.number_of_cmp != 1 {
            //     todo!();
            // }

            ctx.components.push(ComponentTmpl {
                symbol: cmp_bucket.symbol.clone(),
                sub_cmp_idx: sub_cmp_idx as usize,
                number_of_cmp: cmp_bucket.number_of_cmp,
                name_subcomponent: cmp_bucket.name_subcomponent.clone(),
                signal_offset: cmp_bucket.signal_offset,
                signal_offset_jump: cmp_bucket.signal_offset_jump,
                template_id: cmp_bucket.template_id,
                has_inputs: cmp_bucket.has_inputs,
            });

            if false {
                println!("{}", fmt_create_cmp_bucket(cmp_bucket, sub_cmp_idx));
            }

            if !cmp_bucket.has_inputs {
                let components_number: u32 = cmp_bucket.number_of_cmp
                    .try_into().expect("Too many components");

                for i in 0..components_number {
                    ctx.code.push(OpCode::CmpCall as u8);
                    ctx.line_numbers.push(cmp_bucket.line);

                    let (sub_cmp_idx2, overflowed) = sub_cmp_idx.overflowing_add(i);
                    if overflowed {
                        panic!("Subcomponent index is too large");
                    }
                    ctx.code.extend_from_slice(sub_cmp_idx2.to_le_bytes().as_ref());
                    for _ in 0..4 { ctx.line_numbers.push(usize::MAX); }
                }
            }
        }
        Instruction::Call(ref call_bucket) => {
            // println!("call bucket: {}", _call_bucket.to_string());
            // println!("[3, component] {} {:?}", call_bucket.symbol, call_bucket.argument_types.iter().map(|x| format!("{}", x.size)).collect::<Vec<String>>());

            let args_num: usize = call_bucket.arguments.iter().map(
                |arg| {
                    expression(
                        arg, &mut ctx.code, ctx.constants,
                        &mut ctx.line_numbers, &ctx.components)
                }).sum();

            let fn_idx = fn_registry.get(&call_bucket.symbol);
            let fn_idx: u32 = fn_idx.try_into().expect("Such a lot of functions?");
            ctx.code.push(OpCode::FnCall as u8);
            ctx.line_numbers.push(call_bucket.line);

            ctx.code.extend_from_slice(fn_idx.to_le_bytes().as_ref());
            for _ in 0..4 { ctx.line_numbers.push(usize::MAX); }

            let args_num: u32 = args_num.try_into().expect("Too many arguments");
            ctx.code.extend_from_slice(args_num.to_le_bytes().as_ref());
            for _ in 0..4 { ctx.line_numbers.push(usize::MAX); }

            match call_bucket.return_info {
                ReturnType::Intermediate { .. } => todo!(),
                ReturnType::Final(ref final_data) => {
                    let return_num = expect_single_size_u32(
                        &final_data.context.size);
                    ctx.code.extend_from_slice(return_num.to_le_bytes().as_ref());
                    for _ in 0..4 { ctx.line_numbers.push(usize::MAX); }

                    match final_data.dest_address_type {
                        AddressType::Variable => {
                            let location = if let LocationRule::Indexed{ref location, ..} = final_data.dest {
                                location
                            } else {
                                panic!("Variable destination should be of Indexed type");
                            };
                            expression_u32(
                                location, &mut ctx.code, ctx.constants,
                                &mut ctx.line_numbers, &ctx.components);

                            ctx.code.push(OpCode::SetVariable as u8);
                            ctx.line_numbers.push(location.get_line());

                            ctx.code.extend_from_slice(return_num.to_le_bytes().as_ref());
                            for _ in 0..4 { ctx.line_numbers.push(usize::MAX); }
                        }
                        AddressType::Signal => {todo!()}
                        AddressType::SubcmpSignal { ref cmp_address, ref input_information, .. } => {
                            store_subsignal(
                                ctx, &final_data.dest, call_bucket.get_line(),
                                cmp_address, input_information, return_num);
                        }
                    }
                }
            }

        }
        Instruction::Value(_) => {
            panic!("An expression instruction Value found where statement expected");
        }
        Instruction::Load(_) => {
            panic!("An expression instruction Load found where statement expected");
        }
        Instruction::Compute(_) => {
            panic!("An expression instruction Compute found where statement expected");
        }
        Instruction::Branch(ref branch_bucket) => {
            expression(
                &branch_bucket.cond, &mut ctx.code, ctx.constants,
                &mut ctx.line_numbers, &ctx.components);

            let else_jump_offset = pre_emit_jump_if_false(&mut ctx.code);
            ctx.line_numbers.push(branch_bucket.line);
            for _ in 0..4 { ctx.line_numbers.push(usize::MAX); }

            block(ctx, &branch_bucket.if_branch, fn_registry);

            let end_jump_offset = pre_emit_jump(&mut ctx.code);
            ctx.line_numbers.push(branch_bucket.line);
            for _ in 0..4 { ctx.line_numbers.push(usize::MAX); }

            let to = ctx.code.len();
            patch_jump(&mut ctx.code, else_jump_offset, to);

            block(ctx, &branch_bucket.else_branch, fn_registry);

            let to = ctx.code.len();
            patch_jump(&mut ctx.code, end_jump_offset, to);

            assert_eq!(ctx.line_numbers.len(), ctx.code.len());
        }
        Instruction::Return(_) => {
            todo!();
        }
        Instruction::Assert(_) => {
            // TODO: maybe implement asserts at runtime
            // todo!();
        }
        Instruction::Log(_) => {
            // TODO: maybe introduce new instruction for logging
            // todo!();
        }
    }
}

fn fn_statement(
    inst: &InstructionPointer, code: &mut Vec<u8>, constants: &[U256],
    line_numbers: &mut Vec<usize>, fn_registry: &mut FnRegistry) {

    // println!("function statement: {}", inst.to_string());

    match **inst {
        Instruction::Store(ref store_bucket) => {

            let ln = fn_expression(&store_bucket.src, code, constants, line_numbers);
            assert_eq!(ln, expect_single_size(&store_bucket.context.size));
            assert!(matches!(store_bucket.dest_address_type, AddressType::Variable));

            let location = if let LocationRule::Indexed{ref location, ..} = store_bucket.dest {
                location
            } else {
                panic!("Variable destination should be of Indexed type");
            };

            let var_idx =
                u32_or_expression(location, constants).unwrap();

            match var_idx {
                U32OrExpression::U32(var_idx) => {
                    code.push(OpCode::SetVariable4 as u8);
                    line_numbers.push(store_bucket.line);
                    code.extend_from_slice(var_idx.to_le_bytes().as_ref());
                    for _ in 0..4 { line_numbers.push(usize::MAX); }
                }
                U32OrExpression::BigInt(_) => {
                    panic!("Variable index is not u32");
                }
                U32OrExpression::Expression => {
                    fn_expression_u32(location, code, constants, line_numbers);
                    code.push(OpCode::SetVariable as u8);
                    line_numbers.push(store_bucket.line);
                }
            }

            let values_num = expect_single_size_u32(
                &store_bucket.context.size);
            code.extend_from_slice(values_num.to_le_bytes().as_ref());
            for _ in 0..4 { line_numbers.push(usize::MAX); }

            assert_eq!(line_numbers.len(), code.len());
        }
        Instruction::Loop(ref loop_bucket) => {
            let loop_start = code.len();

            let ln = fn_expression(
                &loop_bucket.continue_condition, code, constants, line_numbers);
            assert_eq!(ln, 1);

            let loop_end_jump_offset = pre_emit_jump_if_false(code);
            line_numbers.push(loop_bucket.line);
            for _ in 0..4 { line_numbers.push(usize::MAX); }

            fn_block(
                &loop_bucket.body, code, constants, line_numbers, fn_registry);

            emit_jump(code, loop_start);
            line_numbers.push(loop_bucket.line);
            for _ in 0..4 { line_numbers.push(usize::MAX); }

            let to = code.len();
            patch_jump(code, loop_end_jump_offset, to);
            assert_eq!(line_numbers.len(), code.len());
        }
        Instruction::CreateCmp(..) => {
            panic!("`CreateCmp` instruction is not allowed in function body");
        }
        Instruction::Call(ref call_bucket) => {
            // println!("[3, function] {} {:?}", call_bucket.symbol, call_bucket.argument_types.iter().map(|x| format!("{}", x.size)).collect::<Vec<String>>());
            let args_num: usize = call_bucket.arguments.iter().map(
                |arg| {
                    fn_expression(arg, code, constants, line_numbers)
                }).sum();

            let fn_idx = fn_registry.get(&call_bucket.symbol);
            let fn_idx: u32 = fn_idx.try_into().expect("Such a lot of functions?");
            code.push(OpCode::FnCall as u8);
            line_numbers.push(call_bucket.line);

            code.extend_from_slice(fn_idx.to_le_bytes().as_ref());
            for _ in 0..4 { line_numbers.push(usize::MAX); }

            let args_num: u32 = args_num.try_into().expect("Too many arguments");
            code.extend_from_slice(args_num.to_le_bytes().as_ref());
            for _ in 0..4 { line_numbers.push(usize::MAX); }

            match call_bucket.return_info {
                ReturnType::Intermediate { .. } => todo!(),
                ReturnType::Final(ref final_data) => {
                    let return_num = expect_single_size_u32(
                        &final_data.context.size);
                    code.extend_from_slice(return_num.to_le_bytes().as_ref());
                    for _ in 0..4 { line_numbers.push(usize::MAX); }

                    match final_data.dest_address_type {
                        AddressType::Variable => {
                            let location = if let LocationRule::Indexed{ref location, ..} = final_data.dest {
                                location
                            } else {
                                panic!("Variable destination should be of Indexed type");
                            };
                            fn_expression_u32(
                                location, code, constants, line_numbers);

                            code.push(OpCode::SetVariable as u8);
                            line_numbers.push(location.get_line());

                            code.extend_from_slice(return_num.to_le_bytes().as_ref());
                            for _ in 0..4 { line_numbers.push(usize::MAX); }
                        }
                        AddressType::Signal => {todo!()}
                        AddressType::SubcmpSignal { .. } => {todo!()}
                    }
                }
            }
        }
        Instruction::Value(_) => {
            panic!("An expression instruction Value found where statement expected");
        }
        Instruction::Load(_) => {
            panic!("An expression instruction Load found where statement expected");
        }
        Instruction::Compute(_) => {
            panic!("An expression instruction Compute found where statement expected");
        }
        Instruction::Branch(ref branch_bucket) => {
            let ln = fn_expression(
                &branch_bucket.cond, code, constants, line_numbers);
            assert_eq!(ln, 1);

            let else_jump_offset = pre_emit_jump_if_false(code);
            line_numbers.push(branch_bucket.line);
            for _ in 0..4 { line_numbers.push(usize::MAX); }

            fn_block(
                &branch_bucket.if_branch, code, constants, line_numbers,
                fn_registry);

            let end_jump_offset = pre_emit_jump(code);
            line_numbers.push(branch_bucket.line);
            for _ in 0..4 { line_numbers.push(usize::MAX); }

            let to = code.len();
            patch_jump(code, else_jump_offset, to);

            fn_block(
                &branch_bucket.else_branch, code, constants, line_numbers,
                fn_registry);

            let to = code.len();
            patch_jump(code, end_jump_offset, to);

            assert_eq!(line_numbers.len(), code.len());
        }
        Instruction::Return(ref return_bucket) => {

            let return_num = fn_expression(&return_bucket.value, code, constants, line_numbers);
            let return_num: u32 = return_num.try_into().expect("Too many return values");

            code.push(OpCode::FnReturn as u8);
            line_numbers.push(return_bucket.line);

            code.extend_from_slice(return_num.to_le_bytes().as_ref());
            for _ in 0..4 { line_numbers.push(usize::MAX); }

            assert_eq!(line_numbers.len(), code.len());
        }
        Instruction::Assert(_) => {
            // TODO: maybe implement asserts at runtime
            // todo!();
        }
        Instruction::Log(_) => {
            todo!();
        }
    }
}

fn fmt_create_cmp_bucket(
    cmp_bucket: &CreateCmpBucket,
    sub_cmp_id: u32,
) -> String {
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
        cmp_bucket.has_inputs,
    )
}


fn assert_64() {
    const { assert!(cfg!(target_pointer_width = "64"), "Need a fix for non-64-bit architecture.") };
}

fn expression_load(
    lb: &LoadBucket, code: &mut Vec<u8>, constants: &[U256],
    line_numbers: &mut Vec<usize>, components: &[ComponentTmpl]) -> usize {

    match lb.address_type {
        AddressType::Signal => {
            let location = if let LocationRule::Indexed{ref location, ..} = lb.src {
                location
            } else {
                panic!("Signal source location should be of Indexed type");
            };

            let idx = u32_or_expression(location, constants)
                .unwrap();

            match idx {
                U32OrExpression::U32(idx) => {
                    code.push(OpCode::GetSelfSignal4 as u8);
                    line_numbers.push(lb.line);
                    code.extend_from_slice(idx.to_le_bytes().as_ref());
                    for _ in 0..4 { line_numbers.push(usize::MAX); }
                }
                U32OrExpression::BigInt(_) => {
                    panic!("Signal index is not u32");
                }
                U32OrExpression::Expression => {
                    expression_u32(
                        location, code, constants, line_numbers, components);
                    code.push(OpCode::GetSelfSignal as u8);
                    line_numbers.push(lb.line);
                }
            }

            let signals_num = expect_single_size_u32(&lb.context.size);
            code.extend_from_slice(signals_num.to_le_bytes().as_ref());
            for _ in 0..4 { line_numbers.push(usize::MAX); }
        }
        AddressType::SubcmpSignal { ref cmp_address, .. } => {

            let mut indexes_number = 0u32;
            let mut signal_code = 0u32;

            let is_mapped_signal = match lb.src {
                LocationRule::Indexed { ref location, .. } => {
                    let sig_idx =
                        u32_or_expression(location, constants).unwrap();

                    match sig_idx {
                        U32OrExpression::U32(sig_idx) => {
                            code.push(OpCode::Push4 as u8);
                            line_numbers.push(lb.line);

                            code.extend_from_slice(sig_idx.to_le_bytes().as_ref());
                            for _ in 0..4 { line_numbers.push(usize::MAX); }
                        }
                        U32OrExpression::BigInt(_) => {
                            panic!("Signal index is not u32");
                        }
                        U32OrExpression::Expression => {
                            expression_u32(
                                location, code, constants, line_numbers,
                                components);
                        }
                    };
                    false
                }
                LocationRule::Mapped { signal_code: ref signal_code_local, ref indexes } => {
                    for access in indexes {
                        match access {
                            AccessType::Indexed(index_info) => {
                                for idx_inst in &index_info.indexes {
                                    expression_u32(
                                        idx_inst, code, constants, line_numbers, components);
                                    indexes_number = indexes_number
                                        .checked_add(1)
                                        .expect("Too many indexes");
                                }
                            }
                            AccessType::Qualified(_) => {
                                panic!("bus accesses are not supported in VM compiler");
                            }
                        }
                    }

                    signal_code = (*signal_code_local).try_into()
                        .expect("Too large signal code");

                    true
                }

            };

            // Put component idx to the u32 stack
            match u32_or_expression(cmp_address, constants) {
                Ok(ref cmp_idx) => {
                    match cmp_idx {
                        U32OrExpression::U32(cmp_idx) => {
                            code.push(OpCode::Push4 as u8);
                            line_numbers.push(lb.line);

                            code.extend_from_slice(cmp_idx.to_le_bytes().as_ref());
                            for _ in 0..4 { line_numbers.push(usize::MAX); }
                        }
                        U32OrExpression::BigInt(_) => {
                            panic!("Component index is not u32");
                        }
                        U32OrExpression::Expression => {
                            expression_u32(
                                cmp_address, code, constants, line_numbers,
                                components);
                        }
                    }
                }
                Err(e) => {
                    panic!("Error: {:?}", e);
                }
            };

            // Put opcode
            code.push(OpCode::GetSubSignal as u8);
            line_numbers.push(lb.line);

            // Put signals number argument
            let signals_num = expect_single_size_u32(&lb.context.size);
            code.extend_from_slice(signals_num.to_le_bytes().as_ref());
            for _ in 0..4 { line_numbers.push(usize::MAX); }

            // Put flags argument
            let flags = if is_mapped_signal {
                0b1000_0000
            } else {
                0
            };
            code.push(flags);
            line_numbers.push(usize::MAX);

            if is_mapped_signal {
                // Put mapping indexes number
                code.extend_from_slice(indexes_number.to_le_bytes().as_ref());
                for _ in 0..4 { line_numbers.push(usize::MAX); }

                // Put signal code
                code.extend_from_slice(signal_code.to_le_bytes().as_ref());
                for _ in 0..4 { line_numbers.push(usize::MAX); }
            }
        }
        AddressType::Variable => {
            let location = if let LocationRule::Indexed{ref location, ..} = lb.src {
                location
            } else {
                panic!("Variable source location should be of Indexed type");
            };

            let idx = u32_or_expression(location, constants)
                .unwrap();

            match idx {
                U32OrExpression::U32(idx) => {
                    code.push(OpCode::GetVariable4 as u8);
                    line_numbers.push(lb.line);
                    code.extend_from_slice(idx.to_le_bytes().as_ref());
                    for _ in 0..4 { line_numbers.push(usize::MAX); }
                }
                U32OrExpression::BigInt(_) => {
                    panic!("Variable index is not u32");
                }
                U32OrExpression::Expression => {
                    expression_u32(
                        location, code, constants, line_numbers, components);
                    code.push(OpCode::GetVariable as u8);
                    line_numbers.push(lb.line);
                }
            }

            let signals_num = expect_single_size_u32(&lb.context.size);
            code.extend_from_slice(signals_num.to_le_bytes().as_ref());
            for _ in 0..4 { line_numbers.push(usize::MAX); }
        }
    }

    expect_single_size(&lb.context.size)
}

fn fn_expression_load(
    lb: &LoadBucket, code: &mut Vec<u8>, constants: &[U256],
    line_numbers: &mut Vec<usize>) -> usize {

    assert!(matches!(lb.address_type, AddressType::Variable));

    let location = if let LocationRule::Indexed{ref location, ..} = lb.src {
        location
    } else {
        panic!("Variable source location should be of Indexed type");
    };

    let idx = u32_or_expression(location, constants).unwrap();

    match idx {
        U32OrExpression::U32(idx) => {
            code.push(OpCode::GetVariable4 as u8);
            line_numbers.push(lb.line);
            code.extend_from_slice(idx.to_le_bytes().as_ref());
            for _ in 0..4 { line_numbers.push(usize::MAX); }
        }
        U32OrExpression::BigInt(_) => {
            panic!("Variable index is not u32");
        }
        U32OrExpression::Expression => {
            fn_expression_u32(location, code, constants, line_numbers);
            code.push(OpCode::GetVariable as u8);
            line_numbers.push(lb.line);
        }
    }

    let values_num = expect_single_size_u32(&lb.context.size);
    code.extend_from_slice(values_num.to_le_bytes().as_ref());
    for _ in 0..4 { line_numbers.push(usize::MAX); }

    expect_single_size(&lb.context.size)
}

fn expression_compute(
    cb: &ComputeBucket, code: &mut Vec<u8>, constants: &[U256],
    line_numbers: &mut Vec<usize>,
    components: &[ComponentTmpl]) -> usize {

    // two operand operations
    let mut op2 = |oc: OpCode| {
        assert_eq!(2, cb.stack.len());
        expression(&cb.stack[0], code, constants, line_numbers, components);
        expression(&cb.stack[1], code, constants, line_numbers, components);
        code.push(oc as u8);
        line_numbers.push(cb.line);
    };

    let op1 =
        |oc: OpCode, code: &mut Vec<u8>, line_numbers: &mut Vec<usize>| {
            assert_eq!(1, cb.stack.len());
            expression(
                &cb.stack[0], code, constants, line_numbers, components);
            code.push(oc as u8);
            line_numbers.push(cb.line);
        };

    match &cb.op {
        OperatorType::Mul => {
            op2(OpCode::OpMul);
        }
        OperatorType::Div => {
            op2(OpCode::OpDiv);
        }
        OperatorType::Add => {
            op2(OpCode::OpAdd);
        }
        OperatorType::Sub => {
            op2(OpCode::OpSub);
        }
        OperatorType::Pow => {
            op2(OpCode::OpPow);
        }
        OperatorType::IntDiv => {
            op2(OpCode::OpIntDiv);
        }
        OperatorType::Mod => {
            op2(OpCode::OpMod);
        }
        OperatorType::ShiftL => {
            op2(OpCode::OpShL);
        }
        OperatorType::ShiftR => {
            op2(OpCode::OpShR);
        }
        OperatorType::LesserEq => {
            op2(OpCode::OpLtE);
        }
        OperatorType::GreaterEq => {
            op2(OpCode::OpGtE);
        }
        OperatorType::Lesser => {
            op2(OpCode::OpLt);
        }
        OperatorType::Greater => {
            op2(OpCode::OpGt);
        }
        OperatorType::Eq(x) => {
            if expect_single_size(x) != 1 {
                todo!();
            }
            op2(OpCode::OpEq);
        }
        OperatorType::NotEq => {
            op2(OpCode::OpNe);
        }
        OperatorType::BoolOr => {
            op2(OpCode::OpBoolOr);
        }
        OperatorType::BoolAnd => {
            op2(OpCode::OpBoolAnd);
        }
        OperatorType::BitOr => {
            op2(OpCode::OpBitOr);
        }
        OperatorType::BitAnd => {
            op2(OpCode::OpBitAnd);
        }
        OperatorType::BitXor => {
            op2(OpCode::OpBitXor);
        }
        OperatorType::PrefixSub => {
            op1(OpCode::OpNeg, code, line_numbers)
        }
        OperatorType::BoolNot => {
            op1(OpCode::OpBoolNot, code, line_numbers)
        }
        OperatorType::Complement => {
            op1(OpCode::OpBitNot, code, line_numbers)
        }
        OperatorType::ToAddress | OperatorType::MulAddress | OperatorType::AddAddress => {
            panic!("Unexpected address operator: {}", cb.op.to_string());
        }
    };
    1
}

fn fn_expression_compute(
    cb: &ComputeBucket, code: &mut Vec<u8>, constants: &[U256],
    line_numbers: &mut Vec<usize>) -> usize {

    // two operand operations
    let mut op2 = |oc: OpCode| {
        assert_eq!(2, cb.stack.len());
        let ln = fn_expression(&cb.stack[0], code, constants, line_numbers);
        assert_eq!(1, ln);
        let ln = fn_expression(&cb.stack[1], code, constants, line_numbers);
        assert_eq!(1, ln);
        code.push(oc as u8);
        line_numbers.push(cb.line);
    };

    match &cb.op {
        OperatorType::Mul => {
            op2(OpCode::OpMul);
        }
        OperatorType::Div => {
            op2(OpCode::OpDiv);
        }
        OperatorType::Add => {
            op2(OpCode::OpAdd);
        }
        OperatorType::Sub => {
            op2(OpCode::OpSub);
        }
        OperatorType::Pow => {
            op2(OpCode::OpPow);
        }
        OperatorType::IntDiv => {
            op2(OpCode::OpIntDiv);
        }
        OperatorType::Mod => {
            op2(OpCode::OpMod);
        }
        OperatorType::ShiftL => {
            op2(OpCode::OpShL);
        }
        OperatorType::ShiftR => {
            op2(OpCode::OpShR);
        }
        OperatorType::LesserEq => {
            op2(OpCode::OpLtE);
        }
        OperatorType::GreaterEq => {
            op2(OpCode::OpGtE);
        }
        OperatorType::Lesser => {
            op2(OpCode::OpLt);
        }
        OperatorType::Greater => {
            op2(OpCode::OpGt);
        }
        OperatorType::Eq(x) => {
            if expect_single_size(x) != 1 {
                todo!();
            }
            op2(OpCode::OpEq);
        }
        OperatorType::NotEq => {
            op2(OpCode::OpNe);
        }
        OperatorType::BoolOr => {
            op2(OpCode::OpBoolOr);
        }
        OperatorType::BoolAnd => {
            op2(OpCode::OpBoolAnd);
        }
        OperatorType::BitOr => {
            op2(OpCode::OpBitOr);
        }
        OperatorType::BitAnd => {
            op2(OpCode::OpBitAnd);
        }
        OperatorType::BitXor => {
            op2(OpCode::OpBitXor);
        }
        OperatorType::PrefixSub => {
            assert_eq!(1, cb.stack.len());
            let ln = fn_expression(&cb.stack[0], code, constants, line_numbers);
            assert_eq!(1, ln);
            code.push(OpCode::OpNeg as u8);
            line_numbers.push(cb.line);
        }
        OperatorType::ToAddress | OperatorType::MulAddress | OperatorType::AddAddress => {
            panic!("Unexpected address operator: {}", cb.op.to_string());
        }
        _ => {
            todo!("not implemented function expression operator {}: {}",
                  cb.op.to_string(), cb.to_string());
        }
    };
    1
}

fn expression_compute_u32(
    cb: &ComputeBucket, code: &mut Vec<u8>, constants: &[U256],
    line_numbers: &mut Vec<usize>, components: &[ComponentTmpl]) {

    match cb.op {
        OperatorType::ToAddress => {
            assert_eq!(1, cb.stack.len());
            expression(&cb.stack[0], code, constants, line_numbers, components);
            code.push(OpCode::OpToAddr as u8);
            line_numbers.push(cb.line);
        }
        OperatorType::MulAddress => {
            assert_eq!(2, cb.stack.len());
            expression_u32(
                &cb.stack[0], code, constants, line_numbers, components);
            expression_u32(
                &cb.stack[1], code, constants, line_numbers, components);
            code.push(OpCode::OpMulAddr as u8);
            line_numbers.push(cb.line);
        }
        OperatorType::AddAddress => {
            assert_eq!(2, cb.stack.len());
            expression_u32(
                &cb.stack[0], code, constants, line_numbers, components);
            expression_u32(
                &cb.stack[1], code, constants, line_numbers, components);
            code.push(OpCode::OpAddAddr as u8);
            line_numbers.push(cb.line);
        }
        _ => {
            todo!("not implemented expression operator {}: {}",
                  cb.op.to_string(), cb.to_string());
        }
    };
}

fn fn_expression_compute_u32(
    cb: &ComputeBucket, code: &mut Vec<u8>, constants: &[U256],
    line_numbers: &mut Vec<usize>) {

    match cb.op {
        OperatorType::ToAddress => {
            assert_eq!(1, cb.stack.len());
            let ln = fn_expression(&cb.stack[0], code, constants, line_numbers);
            assert_eq!(1, ln);
            code.push(OpCode::OpToAddr as u8);
            line_numbers.push(cb.line);
        }
        OperatorType::MulAddress => {
            assert_eq!(2, cb.stack.len());
            fn_expression_u32(&cb.stack[0], code, constants, line_numbers);
            fn_expression_u32(&cb.stack[1], code, constants, line_numbers);
            code.push(OpCode::OpMulAddr as u8);
            line_numbers.push(cb.line);
        }
        OperatorType::AddAddress => {
            assert_eq!(2, cb.stack.len());
            fn_expression_u32(&cb.stack[0], code, constants, line_numbers);
            fn_expression_u32(&cb.stack[1], code, constants, line_numbers);
            code.push(OpCode::OpAddAddr as u8);
            line_numbers.push(cb.line);
        }
        _ => {
            todo!("not implemented function expression operator {}: {}",
                  cb.op.to_string(), cb.to_string());
        }
    };
}

// After expression execution, the value of the expression should be on the stack.
// Return the number of values put on the stack.
fn expression(
    inst: &InstructionPointer, code: &mut Vec<u8>, constants: &[U256],
    line_numbers: &mut Vec<usize>,
    components: &[ComponentTmpl]) -> usize {

    // println!("expression: {}", inst.to_string());
    match **inst {
        Instruction::Value(ref value_bucket) => {
            match value_bucket.parse_as {
                ValueType::U32 => {
                    code.push(OpCode::Push8 as u8);
                    line_numbers.push(value_bucket.line);
                    assert_64();
                    code.extend_from_slice(value_bucket.value.to_le_bytes().as_ref());
                    for _ in 0..8 { line_numbers.push(usize::MAX); }
                }
                ValueType::BigInt => {
                    code.push(OpCode::GetConstant8 as u8);
                    line_numbers.push(value_bucket.line);
                    assert_64();
                    code.extend_from_slice(value_bucket.value.to_le_bytes().as_ref());
                    for _ in 0..8 { line_numbers.push(usize::MAX); }
                }
            }
            assert_eq!(line_numbers.len(), code.len());
            1
        }
        Instruction::Load(ref load_bucket) => {
            let n = expression_load(
                load_bucket, code, constants, line_numbers, components);
            assert_eq!(line_numbers.len(), code.len());
            n
        }
        Instruction::Compute(ref compute_bucket) => {
            let n = expression_compute(
                compute_bucket, code, constants, line_numbers, components);
            assert_eq!(line_numbers.len(), code.len());
            n
        }
        _ => {
            panic!("Expression does not supported: {}", inst.to_string());
        }
    }
}

fn fn_expression(
    inst: &InstructionPointer, code: &mut Vec<u8>, constants: &[U256],
    line_numbers: &mut Vec<usize>) -> usize {

    match **inst {
        Instruction::Value(ref value_bucket) => {
            match value_bucket.parse_as {
                ValueType::U32 => {
                    code.push(OpCode::Push8 as u8);
                    line_numbers.push(value_bucket.line);
                    assert_64();
                    code.extend_from_slice(value_bucket.value.to_le_bytes().as_ref());
                    for _ in 0..8 { line_numbers.push(usize::MAX); }
                }
                ValueType::BigInt => {
                    code.push(OpCode::GetConstant8 as u8);
                    line_numbers.push(value_bucket.line);
                    assert_64();
                    code.extend_from_slice(value_bucket.value.to_le_bytes().as_ref());
                    for _ in 0..8 { line_numbers.push(usize::MAX); }
                }
            }
            assert_eq!(line_numbers.len(), code.len());
            1
        }
        Instruction::Load(ref load_bucket) => {
            let ln =
                fn_expression_load(load_bucket, code, constants, line_numbers);
            assert_eq!(line_numbers.len(), code.len());
            ln
        }
        Instruction::Compute(ref compute_bucket) => {
            let ln = fn_expression_compute(
                compute_bucket, code, constants, line_numbers);
            assert_eq!(line_numbers.len(), code.len());
            ln
        }
        _ => {
            panic!("Expression does not supported: {}", inst.to_string());
        }
    }
}

// After expression execution_u32, the value of the expression should be on the stack_u32
fn expression_u32(
    inst: &InstructionPointer, code: &mut Vec<u8>, constants: &[U256],
    line_numbers: &mut Vec<usize>, components: &[ComponentTmpl]) {

    // println!("expression: {}", inst.to_string());
    match **inst {
        Instruction::Value(ref value_bucket) => {
            match value_bucket.parse_as {
                ValueType::U32 => {
                    code.push(OpCode::Push4 as u8);
                    line_numbers.push(value_bucket.line);
                    let value: u32 = value_bucket.value.try_into().expect("Value is too large");
                    code.extend_from_slice(value.to_le_bytes().as_ref());
                    for _ in 0..4 { line_numbers.push(usize::MAX); }
                }
                ValueType::BigInt => {
                    todo!("convert to u32 and check this");
                    // code.push(OpCode::GetConstant8 as u8);
                    // line_numbers.push(value_bucket.line);
                    // assert_64();
                    // code.extend_from_slice(value_bucket.value.to_le_bytes().as_ref());
                    // for _ in 0..8 { line_numbers.push(usize::MAX); }
                }
            }
            assert_eq!(line_numbers.len(), code.len());
        }
        Instruction::Compute(ref compute_bucket) => {
            expression_compute_u32(
                compute_bucket, code, constants, line_numbers, components);
            assert_eq!(line_numbers.len(), code.len());
        }
        _ => {
            panic!("U32 expression does not supported: {}", inst.to_string());
        }
    }
}

fn fn_expression_u32(
    inst: &InstructionPointer, code: &mut Vec<u8>, constants: &[U256],
    line_numbers: &mut Vec<usize>) {

    match **inst {
        Instruction::Value(ref value_bucket) => {
            match value_bucket.parse_as {
                ValueType::U32 => {
                    code.push(OpCode::Push4 as u8);
                    line_numbers.push(value_bucket.line);
                    let value: u32 = value_bucket.value.try_into().expect("Value is too large");
                    code.extend_from_slice(value.to_le_bytes().as_ref());
                    for _ in 0..4 { line_numbers.push(usize::MAX); }
                }
                ValueType::BigInt => {
                    todo!("convert to u32 and check this");
                    // code.push(OpCode::GetConstant8 as u8);
                    // line_numbers.push(value_bucket.line);
                    // assert_64();
                    // code.extend_from_slice(value_bucket.value.to_le_bytes().as_ref());
                    // for _ in 0..8 { line_numbers.push(usize::MAX); }
                }
            }
            assert_eq!(line_numbers.len(), code.len());
        }
        Instruction::Compute(ref compute_bucket) => {
            fn_expression_compute_u32(
                compute_bucket, code, constants, line_numbers);
            assert_eq!(line_numbers.len(), code.len());
        }
        _ => {
            panic!(
                "U32 function expression does not supported: {}",
                inst.to_string());
        }
    }
}

fn block(
    ctx: &mut TmplCompilationCtx, inst_list: &InstructionList,
    fn_registry: &mut FnRegistry) {

    for inst in inst_list {
        statement(ctx, inst, fn_registry);
        assert_eq!(ctx.line_numbers.len(), ctx.code.len());
    }
}

fn fn_block(
    inst_list: &InstructionList, code: &mut Vec<u8>, constants: &[U256],
    line_numbers: &mut Vec<usize>, fn_registry: &mut FnRegistry) {

    for inst in inst_list {
        fn_statement(inst, code, constants, line_numbers, fn_registry);
        assert_eq!(line_numbers.len(), code.len());
    }
}

struct TmplCompilationCtx<'a> {
    code: Vec<u8>,
    line_numbers: Vec<usize>,
    components: Vec<ComponentTmpl>,
    constants: &'a [U256],
}

impl TmplCompilationCtx<'_> {
    fn new(constants: &[U256]) -> TmplCompilationCtx<'_> {
        TmplCompilationCtx {
            code: vec![],
            line_numbers: vec![],
            components: vec![],
            constants,
        }
    }
}

fn compile_template(
    tmpl_code: &TemplateCode, constants: &[U256],
    fn_registry: &mut FnRegistry) -> Template {

    // println!("Compile template {}", tmpl_code.name);

    let mut ctx = TmplCompilationCtx::new(constants);
    block(&mut ctx, &tmpl_code.body, fn_registry);

    assert_eq!(ctx.line_numbers.len(), ctx.code.len());

    Template {
        name: tmpl_code.name.clone(),
        code: ctx.code,
        line_numbers: ctx.line_numbers,
        components: ctx.components,
        var_stack_depth: tmpl_code.var_stack_depth,
        number_of_inputs: tmpl_code.number_of_inputs,
    }
}

fn compile_function(
    fn_code: &FunctionCode, constants: &[U256],
    fn_registry: &mut FnRegistry) -> Function {

    // println!("Compile function {}", fn_code.name);

    let mut code = vec![];
    let mut line_numbers = vec![];
    fn_block(
        &fn_code.body, &mut code, constants, &mut line_numbers, fn_registry);

    assert_eq!(line_numbers.len(), code.len());

    Function {
        name: fn_code.name.clone(),
        symbol: fn_code.header.clone(),
        code,
        line_numbers,
    }
}

struct FnRegistry (HashMap<String, usize>);

impl FnRegistry {
    fn new() -> Self {
        FnRegistry(HashMap::new())
    }

    fn sort(&self, fns: &mut [Function]) {
        fns.sort_by_key(|f| {
            match self.0.get(&f.symbol) {
                Some(idx) => *idx,
                None => {
                    println!("Function {} not found in registry", f.name);
                    usize::MAX
                },
            }
        });
    }

    fn get(&mut self, name: &str) -> usize {
        match self.0.get(name) {
            Some(idx) => *idx,
            None => {
                self.0.insert(name.to_string(), self.0.len());
                self.0.len() - 1
            },
        }
    }
}

fn compile(
    templates: &[TemplateCode],
    functions: &Vec<FunctionCode>,
    constants: &[U256],
) -> (Vec<Template>, Vec<Function>) {

    let mut fn_registry = FnRegistry::new();

    let mut compiled_functions = Vec::with_capacity(functions.len());

    for fun in functions {
        compiled_functions.push(
            compile_function(fun, constants, &mut fn_registry));
    }

    let mut compiled_templates = Vec::with_capacity(templates.len());
    for tmpl in templates.iter() {
        compiled_templates.push(
            compile_template(tmpl, constants, &mut fn_registry));
    }

    fn_registry.sort(&mut compiled_functions);

    // Assert all components created has has_inputs field consistent with
    // the number of inputs in the templates
    for (i, tmpl) in compiled_templates.iter().enumerate() {
        // println!("Template #{}: {}", i, tmpl.name);
        for c in tmpl.components.iter() {
            let has_inputs = templates[c.template_id].number_of_inputs > 0;
            assert_eq!(c.has_inputs, has_inputs, "#{} {}", i, c.symbol);
            // println!("Component: {} {}", c.symbol, c.template_id);
        }
    }

    (compiled_templates, compiled_functions)
}

fn bigint_to_u64(i: U256) -> Option<u64> {
    let ls = i.as_limbs();
    for limb in ls.iter().skip(1) {
        if *limb != 0 {
            return None;
        }
    }
    Some(ls[0])
}

fn disassemble(
    code: &[u8], line_numbers: &[usize], name: &str, functions: &[Function]) {

    let mut ip = 0usize;
    while ip < code.len() {
        ip = disassemble_instruction(code, line_numbers, ip, name, functions);
    }
}

fn mk_flags(input_status: InputStatus, is_mapped_signal_idx: bool) -> u8 {
    let mut flags: u8 = input_status as u8;

    if is_mapped_signal_idx {
        flags |= 0b1000_0000
    }

    flags
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap};
    use compiler::intermediate_representation::ir_interface::{InstrContext, StoreBucket, ValueBucket};
    use ruint::__private::ruint_macro::uint;
    use circom_witnesscalc::vm::{Function, Template, execute, Component};
    use super::*;

    fn assert_u64_value(inst: &InstructionPointer) -> u64 {
        let value = if let Instruction::Value(ref value) = **inst {
            value
        } else {
            panic!("Expected value instruction, got: {}", inst.to_string());
        };

        assert!(matches!(value.parse_as, ValueType::U32));
        value.value.try_into().expect("value is not u64")
    }

    #[test]
    fn test_parse_args() {
        let o = OpCode::SetSelfSignal4;
        println!("OK: {:?}", o);
        let i = o as u8;
        println!("OK: {:?}", i);
        let o2 = unsafe { std::mem::transmute::<u8, OpCode>(i) };
        println!("OK: {:?}", o2);
    }

    #[test]
    fn test_assert_u32() {
        let inst = Box::new(Instruction::Value(ValueBucket {
            line: 0,
            message_id: 0,
            parse_as: ValueType::U32,
            op_aux_no: 0,
            value: 42,
        }));
        let val: u64 = assert_u64_value(&inst);
        assert_eq!(val, 42);
    }

    #[test]
    #[should_panic(expected = "assertion failed: matches!(value.parse_as, ValueType::U32)")]
    fn test_assert_u32_not_u32() {
        let inst = Box::new(Instruction::Value(ValueBucket {
            line: 0,
            message_id: 0,
            parse_as: ValueType::BigInt,
            op_aux_no: 0,
            value: 42,
        }));
        let val: u64 = assert_u64_value(&inst);
        assert_eq!(val, 42);
    }

    #[test]
    fn test_expression_value_u32() {
        let mut code = vec![];
        let constants = vec![];
        let inst = Box::new(Instruction::Value(ValueBucket {
            line: 0,
            message_id: 0,
            parse_as: ValueType::U32,
            op_aux_no: 0,
            value: 42,
        }));
        expression(
            &inst, &mut code, &constants, &mut vec![],
            &[]);
        assert_eq!(code, vec![OpCode::Push8 as u8, 42, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn test_expression_value_bigint() {
        let mut code = vec![];
        let constants = vec![];
        let inst = Box::new(Instruction::Value(ValueBucket {
            line: 0,
            message_id: 0,
            parse_as: ValueType::BigInt,
            op_aux_no: 0,
            value: 42,
        }));
        expression(&inst, &mut code, &constants, &mut vec![], &[]);
        assert_eq!(code, vec![OpCode::GetConstant8 as u8, 42, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn statement_1() {
        /*
        STORE(
  line:8,
  template_id:0,
  dest_type:VARIABLE,
  dest:INDEXED:(
    VALUE(
	  line:8,
	  template_id:0,
	  as:U32,
	  op_number:0,
	  value:0),
	NONE),
  src:VALUE(
    line:8,
	template_id:0,
	as:BigInt,
	op_number:0,
	value:0)
)
         */
        let inst = Box::new(Instruction::Store(StoreBucket {
            line: 8,
            message_id: 0,
            context: InstrContext { size: SizeOption::Single(1) },
            src_context: InstrContext { size: SizeOption::Single(1) },
            dest_is_output: false,
            dest_address_type: AddressType::Variable,
            src_address_type: None,
            dest: LocationRule::Indexed {
                location: Box::new(Instruction::Value(ValueBucket {
                    line: 8,
                    message_id: 0,
                    parse_as: ValueType::U32,
                    op_aux_no: 0,
                    value: 555,
                })),
                template_header: None,
            },
            src: Box::new(Instruction::Value(ValueBucket {
                line: 8,
                message_id: 0,
                parse_as: ValueType::BigInt,
                op_aux_no: 0,
                value: 234,
            })),
        }));

        let constants = vec![];
        let mut fn_registry = FnRegistry::new();
        let mut ctx = TmplCompilationCtx::new(&constants);
        statement(&mut ctx, &inst, &mut fn_registry);
        assert_eq!(ctx.code,
                   vec![
                       OpCode::GetConstant8 as u8, 0xea, 0, 0, 0, 0, 0, 0, 0,
                       OpCode::SetVariable4 as u8, 0x2b, 0x2, 0, 0, 1, 0, 0, 0]);
    }

    #[test]
    fn statement_2() {
        /*
		STORE(
			line:12,
			template_id:0,
			dest_type:VARIABLE,
			dest:INDEXED:(VALUE(line:10,template_id:0,as:U32,op_number:0,value:1), NONE),
			src:COMPUTE(
				line:12,
				template_id:0,
				op_number:0,
				op:ADD,
				stack:
					LOAD(
						line:12,
						template_id:0,
						address_type:VARIABLE,
						src:INDEXED:(VALUE(line:10,template_id:0,as:U32,op_number:0,value:1),NONE)
					);
					LOAD(
						line:12,
						template_id:0,
						address_type:SIGNAL,
						src:INDEXED:(COMPUTE(
							line:0,
							template_id:0,
							op_number:0,
							op:ADD_ADDRESS,
							stack:
								COMPUTE(
									line:0,
									template_id:0,
									op_number:0,
									op:MUL_ADDRESS,
									stack:
										VALUE(line:0,template_id:0,as:U32,op_number:0,value:1);
										COMPUTE(
											line:12,
											template_id:0,
											op_number:0,
											op:TO_ADDRESS,
											stack:
												LOAD(line:12,template_id:0,address_type:VARIABLE,src:INDEXED: (VALUE(line:11,template_id:0,as:U32,op_number:0,value:2), NONE));
										);
								);
								VALUE(line:0,template_id:0,as:U32,op_number:0,value:1);
						), NONE)
					);
			)
		);
         */
        let inst = Box::new(Instruction::Store(StoreBucket {
            line: 12,
            message_id: 0,
            context: InstrContext { size: SizeOption::Single(1) },
            src_context: InstrContext { size: SizeOption::Single(1) },
            dest_is_output: false,
            dest_address_type: AddressType::Variable,
            src_address_type: None,
            dest: LocationRule::Indexed {
                location: Box::new(Instruction::Value(ValueBucket {
                    line: 8,
                    message_id: 0,
                    parse_as: ValueType::U32,
                    op_aux_no: 0,
                    value: 1,
                })),
                template_header: None,
            },
            src: Box::new(Instruction::Compute(ComputeBucket{
                line: 12,
                message_id: 0,
                op: OperatorType::Add,
                op_aux_no: 0,
                stack: vec![
                    Box::new(Instruction::Load(LoadBucket{
                        line: 0,
                        message_id: 0,
                        address_type: AddressType::Variable,
                        src: LocationRule::Indexed {
                            location: Box::new(Instruction::Value(ValueBucket {
                                line: 10,
                                message_id: 0,
                                parse_as: ValueType::U32,
                                op_aux_no: 0,
                                value: 1,
                            })),
                            template_header: None,
                        },
                        context: InstrContext { size: SizeOption::Single(1) },
                    })),
                    Box::new(Instruction::Load(LoadBucket{
                        line: 12,
                        message_id: 0,
                        address_type: AddressType::Signal,
                        src: LocationRule::Indexed {
                            location: Box::new(Instruction::Compute(ComputeBucket{
                                line: 0,
                                message_id: 0,
                                op: OperatorType::AddAddress,
                                op_aux_no: 0,
                                stack: vec![
                                    Box::new(Instruction::Compute(ComputeBucket{
                                        line: 0,
                                        message_id: 0,
                                        op: OperatorType::MulAddress,
                                        op_aux_no: 0,
                                        stack: vec![
                                            Box::new(Instruction::Value(ValueBucket{
                                                line: 0,
                                                message_id: 0,
                                                parse_as: ValueType::U32,
                                                op_aux_no: 0,
                                                value: 1,
                                            })),
                                            Box::new(Instruction::Compute(ComputeBucket{
                                                line: 12,
                                                message_id: 0,
                                                op: OperatorType::ToAddress,
                                                op_aux_no: 0,
                                                stack: vec![
                                                    Box::new(Instruction::Load(LoadBucket{
                                                        line: 12,
                                                        message_id: 0,
                                                        address_type: AddressType::Variable,
                                                        src: LocationRule::Indexed {
                                                            location: Box::new(Instruction::Value(ValueBucket {
                                                                line: 11,
                                                                message_id: 0,
                                                                parse_as: ValueType::U32,
                                                                op_aux_no: 0,
                                                                value: 2,
                                                            })),
                                                            template_header: None,
                                                        },
                                                        context: InstrContext { size: SizeOption::Single(1) },
                                                    })),
                                                ],
                                            })),
                                        ],
                                    })),
                                    Box::new(Instruction::Value(ValueBucket{
                                        line: 0,
                                        message_id: 0,
                                        parse_as: ValueType::U32,
                                        op_aux_no: 0,
                                        value: 1,
                                    })),
                                ],
                            })),
                            template_header: None,
                        },
                        context: InstrContext { size: SizeOption::Single(1) },
                    })),
                ],
            })),
        }));

        let constants = vec![];
        let mut ctx = TmplCompilationCtx::new(&constants);
        statement(&mut ctx, &inst, &mut FnRegistry::new());

        let var1 = Some(U256::from(2));
        let var2 = Some(U256::from(1));
        let c = Component{
            vars: vec![None, var1, var2],
            signals_start: 0,
            template_id: 0,
            subcomponents: vec![],
            number_of_inputs: 0,
        };
        let component = Rc::new(RefCell::new(c));
        let templates = vec![Template{
            name: "test".to_string(),
            code: ctx.code,
            line_numbers: ctx.line_numbers,
            components: ctx.components,
            var_stack_depth: 0,
            number_of_inputs: 0,
        }];

        let mut signals = vec![None, None, Some(U256::from(8))];

        let functions = vec![];

        disassemble(
            &templates[0].code, &templates[0].line_numbers, "test",
            &functions);

        let io_map = BTreeMap::new();

        println!("execute");
        execute(
            component.clone(), &templates, &functions, &constants,
            &mut signals, &io_map, None);
        println!("{:?}", component.borrow().vars);
        assert_eq!(
            &vec![None, Some(U256::from(10u64)), var2],
            &component.borrow().vars);
    }

    #[test]
    fn statement_3() {
        /*
STORE(
	line:207,
	template_id:69,
	dest_type:SIGNAL,
	dest:INDEXED:(VALUE(line:0,template_id:69,as:U32,op_number:0,value:0),NONE),
	src:LOAD(
		line:207,
		template_id:69,
		address_type:SUBCOMPONENT:VALUE(line:0,template_id:69,as:U32,op_number:0,value:0):NO_INPUT,
		src:INDEXED:(VALUE(line:0,template_id:68,as:U32,op_number:0,value:0),PoseidonEx_68),
		size:1
	),
	size:1
)
         */
        let inst = Box::new(Instruction::Store(StoreBucket {
            line: 207,
            message_id: 0,
            context: InstrContext { size: SizeOption::Single(1) },
            src_context: InstrContext { size: SizeOption::Single(1) },
            dest_is_output: false,
            dest_address_type: AddressType::Signal,
            src_address_type: None,
            dest: LocationRule::Indexed {
                location: Box::new(Instruction::Value(ValueBucket {
                    line: 0,
                    message_id: 0,
                    parse_as: ValueType::U32,
                    op_aux_no: 0,
                    value: 0,
                })),
                template_header: None,
            },
            src: Box::new(Instruction::Load(LoadBucket {
                line: 207,
                message_id: 0,
                address_type: AddressType::SubcmpSignal {
                    cmp_address: Box::new(Instruction::Value(ValueBucket {
                        line: 0,
                        message_id: 0,
                        parse_as: ValueType::U32,
                        op_aux_no: 0,
                        value: 0,
                    })),
                    uniform_parallel_value: None,
                    is_output: false,
                    input_information: InputInformation::NoInput,
                    is_anonymous: false,
                    cmp_name: String::new(),
                },
                src: LocationRule::Indexed {
                    location: Box::new(Instruction::Value(ValueBucket {
                        line: 0,
                        message_id: 0,
                        parse_as: ValueType::U32,
                        op_aux_no: 0,
                        value: 0,
                    })),
                    template_header: None },
                context: InstrContext { size: SizeOption::Single(1) },
            })),
        }));

        let constants = vec![];
        let functions = vec![];
        let mut ctx = TmplCompilationCtx::new(&constants);
        statement(&mut ctx, &inst, &mut FnRegistry::new());
        disassemble(&ctx.code, &ctx.line_numbers, "test", &functions);
    }

    #[test]
    fn test_bigint_to_u64() {
        let mut i = U256::from(u64::MAX);
        let j = bigint_to_u64(i);
        assert!(matches!(j, Some(u64::MAX)));

        i *= uint!(2_U256);
        let j = bigint_to_u64(i);
        assert!(j.is_none());
    }

    #[test]
    fn test_jump_if_false_backward() {
        let noop = OpCode::NoOp as u8;
        let mut code = vec![];
        code.push(noop);
        let to = code.len();
        code.push(noop);
        let jump_offset = pre_emit_jump_if_false(&mut code);

        assert_eq!(code, vec![noop, noop, OpCode::JumpIfFalse as u8, 0xff, 0xff, 0xff, 0xff]);
        assert_eq!(jump_offset, 3);

        patch_jump(&mut code, jump_offset, to);
        assert_eq!(code, vec![noop, noop, OpCode::JumpIfFalse as u8, 0xfa, 0xff, 0xff, 0xff]);
        assert_eq!(-6, i32::from_le_bytes(code[jump_offset..jump_offset+4].try_into().unwrap()));
    }
    #[test]
    fn test_jump_if_false_forward() {
        let noop = OpCode::NoOp as u8;
        let jmp = OpCode::JumpIfFalse as u8;
        let mut code = vec![];
        code.push(noop);
        let jump_offset = pre_emit_jump_if_false(&mut code);
        assert_eq!(code, vec![noop, jmp, 0xff, 0xff, 0xff, 0xff]);
        assert_eq!(jump_offset, 2);

        code.push(noop);
        code.push(noop);
        let to = code.len();
        assert_eq!(8, to);
        code.push(noop);

        patch_jump(&mut code, jump_offset, to);
        assert_eq!(code, vec![noop, jmp, 0x02, 0x00, 0x00, 0x00, noop, noop, noop, ]);
        assert_eq!(2, i32::from_le_bytes(code[jump_offset..jump_offset+4].try_into().unwrap()));
    }

    #[test]
    fn test_set_variable() {
        let code = vec![
            OpCode::Push8 as u8, 0, 0, 0, 0, 0, 0, 0, 0,
            OpCode::JumpIfFalse as u8, 9*2, 0x00, 0x00, 0x00,
            OpCode::Push8 as u8, 0x01, 0, 0, 0, 0, 0, 0, 0,
            OpCode::SetVariable4 as u8, 0, 0, 0, 0, 1, 0, 0, 0,
            OpCode::Push8 as u8, 0x02, 0, 0, 0, 0, 0, 0, 0,
            OpCode::SetVariable4 as u8, 1, 0, 0, 0, 1, 0, 0, 0,
            OpCode::NoOp as u8,
        ];
        let mut line_numbers = Vec::with_capacity(code.len());
        for (i, _) in code.iter().enumerate() {
            line_numbers.push(i);
        }

        let functions = vec![];

        disassemble(&code, &line_numbers, "test", &functions);
        println!("execute");

        let c = Component{
            vars: vec![],
            signals_start: 0,
            template_id: 0,
            subcomponents: vec![],
            number_of_inputs: 0,
        };
        let component = Rc::new(RefCell::new(c));
        let templates = vec![Template {
            name: "test".to_string(),
            code: code.clone(),
            line_numbers,
            components: vec![],
            var_stack_depth: 0,
            number_of_inputs: 0,
        }];
        let constants = vec![];
        let mut signals = vec![None; 10];
        let io_map = BTreeMap::new();
        execute(
            component.clone(), &templates, &functions, &constants,
            &mut signals, &io_map, None);
        println!("{:?}", &component.borrow().vars);
    }

    #[test]
    fn test_dump() {
        let noop = OpCode::NoOp as u8;
        let code = vec![noop];
        let c = Component{
            vars: vec![],
            signals_start: 0,
            template_id: 0,
            subcomponents: vec![],
            number_of_inputs: 0,
        };
        let component = Rc::new(RefCell::new(c));
        let templates = vec![Template{
            name: "test".to_string(),
            code: code.clone(),
            line_numbers: vec![0],
            components: vec![],
            var_stack_depth: 0,
            number_of_inputs: 0,
        }];
        let constants = vec![];
        let mut signals = vec![None; 10];
        let io_map = BTreeMap::new();
        execute(
            component, &templates, &[], &constants, &mut signals,
            &io_map, None);
    }

    fn names_from_fn_vec(fns: &[Function]) -> Vec<String> {
        fns.iter().map(|f| f.name.clone()).collect()
    }

    fn mk_empty_fun(name: &str, symbol: &str) -> Function {
        Function {
            name: name.to_string(),
            symbol: symbol.to_string(),
            code: vec![],
            line_numbers: vec![],
        }
    }

    #[test]
    fn test_fn_registry_sort() {
        let mut reg = FnRegistry::new();
        assert_eq!(0, reg.get("ssigma1_1"));
        assert_eq!(1, reg.get("ssigma0_2"));
        assert_eq!(2, reg.get("bsigma1_3"));
        assert_eq!(3, reg.get("Ch_4"));
        assert_eq!(4, reg.get("sha256K_5"));
        assert_eq!(5, reg.get("bsigma0_6"));
        assert_eq!(6, reg.get("Maj_7"));
        assert_eq!(7, reg.get("rrot_8"));
        assert_eq!(8, reg.get("sha256compression_0"));

        let mut fns: Vec<Function> = vec![
            mk_empty_fun("sha256compression", "sha256compression_0"),
            mk_empty_fun("ssigma1", "ssigma1_1"),
            mk_empty_fun("ssigma0", "ssigma0_2"),
            mk_empty_fun("bsigma1", "bsigma1_3"),
            mk_empty_fun("Ch", "Ch_4"),
            mk_empty_fun("sha256K", "sha256K_5"),
            mk_empty_fun("bsigma0", "bsigma0_6"),
            mk_empty_fun("Maj", "Maj_7"),
            mk_empty_fun("rrot", "rrot_8"),
        ];

        let want = vec![
            "ssigma1".to_string(),
            "ssigma0".to_string(),
            "bsigma1".to_string(),
            "Ch".to_string(),
            "sha256K".to_string(),
            "bsigma0".to_string(),
            "Maj".to_string(),
            "rrot".to_string(),
            "sha256compression".to_string(),
        ];

        reg.sort(&mut fns);
        assert_eq!(want, names_from_fn_vec(&fns));
    }
}
