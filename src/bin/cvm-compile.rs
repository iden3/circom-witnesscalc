use std::{env, fs, process};
use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::io::Write;
use std::time::Instant;
use num_bigint::BigUint;
use num_traits::{Num, ToBytes};
use circom_witnesscalc::{ast, calculate_witness_vm2, vm2};
use circom_witnesscalc::ast::{Expr, FfExpr, I64Expr, I64Operand, Statement, AST};
use circom_witnesscalc::field::{bn254_prime, Field, FieldOperations, FieldOps};
use circom_witnesscalc::parser::parse;
use circom_witnesscalc::storage::serialize_witnesscalc_vm2;
use circom_witnesscalc::vm2::{Circuit, OpCode, InputInfo};
#[cfg(feature = "debug_vm2")]
use circom_witnesscalc::vm2::{Template, Function};
#[cfg(feature = "debug_vm2")]
use circom_witnesscalc::vm2::disassemble_instruction;

struct Args {
    cvm_file: String,
    output_file: Option<String>,
    wtns_file: Option<String>,
    inputs_file: Option<String>,
}

#[derive(Debug, thiserror::Error)]
enum CompilationError {
    #[error("Main template ID is not found")]
    MainTemplateIDNotFound,
    #[error("jump offset is too large")]
    JumpOffsetIsTooLarge,
    #[error("[assertion] Loop control stack is empty")]
    LoopControlJumpsEmpty,
    #[error("Bus type `{0}` not found in type definitions")]
    BusTypeNotFound(String),
}

fn parse_args() -> Args {
    let mut cvm_file: Option<String> = None;
    let mut output_file: Option<String> = None;
    let mut wtns_file: Option<String> = None;
    let mut inputs_file: Option<String> = None;

    let args: Vec<String> = env::args().collect();

    let usage = |err_msg: &str| -> ! {
        if !err_msg.is_empty() {
            eprintln!("ERROR:");
            eprintln!("    {}", err_msg);
            eprintln!();
        }
        eprintln!("USAGE:");
        eprintln!("    {} <cvm_file> [OPTIONS]", args[0]);
        eprintln!();
        eprintln!("ARGUMENTS:");
        eprintln!("    <cvm_file>    Path to the CVM file with compiled circuit");
        eprintln!();
        eprintln!("OPTIONS:");
        eprintln!("    -h | --help            Display this help message");
        eprintln!("    --wtns <output.wtns>   If file is provided, the witness will be calculated and saved in this file. Inputs file MUST be provided as well.");
        eprintln!("    --inputs <inputs.json> File with inputs for the circuit. Required if --wtns is provided.");
        eprintln!("    -o <circuit.wcd>       Wile where compiled bytecode could be saved for faster execution next time.");
        let exit_code = if !err_msg.is_empty() { 1i32 } else { 0i32 };
        std::process::exit(exit_code);
    };

    let mut i = 1;
    while i < args.len() {
        if args[i] == "--help" || args[i] == "-h" {
            usage("");
        } else if args[i] == "--wtns" {
            i += 1;
            if i >= args.len() {
                usage("missing argument for --wtns");
            }
            if wtns_file.is_some() {
                usage("multiple witness files");
            }
            wtns_file = Some(args[i].clone());
        } else if args[i] == "--inputs" {
            i += 1;
            if i >= args.len() {
                usage("missing argument for --inputs");
            }
            if inputs_file.is_some() {
                usage("multiple inputs files");
            }
            inputs_file = Some(args[i].clone());
        } else if args[i] == "-o" {
            i += 1;
            if i >= args.len() {
                usage("missing argument for -o");
            }
            if output_file.is_some() {
                usage("multiple bytecode output files");
            }
            output_file = Some(args[i].clone());
        } else if args[i].starts_with("-") {
            usage(format!("Unknown option: {}", args[i]).as_str());
        } else if cvm_file.is_none() {
            cvm_file = Some(args[i].clone());
        } else {
            usage(format!("unexpected argument: {}", args[i]).as_str());
        }
        i += 1;
    }

    if wtns_file.is_some() && inputs_file.is_none() {
        usage("need to provide inputs file with --inputs if --wtns is set");
    }

    Args {
        cvm_file: cvm_file.unwrap_or_else(|| { usage("missing CVM file") }),
        output_file,
        wtns_file,
        inputs_file,
    }
}

fn main() {
    let args = parse_args();
    let program_text = fs::read_to_string(&args.cvm_file).unwrap();
    let program = match parse(&program_text) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", e);
            process::exit(1);
        }
    };

    let bn254 = BigUint::from_str_radix("21888242871839275222246405745257275088548364400416034343698204186575808495617", 10).unwrap();
    if program.prime == bn254 {
        let ff = Field::new(bn254_prime);
        run(&ff, &program, &args).unwrap();
    } else {
        eprintln!("ERROR: Unsupported prime field");
        std::process::exit(1);
    }
}

fn run<T: FieldOps>(ff: &Field<T>, program: &AST, args: &Args) -> Result<(), Box<dyn Error>> {
    let start = Instant::now();
    let circuit = compile(ff, program)?;
    let mut bytecode_buf: Vec<u8> = Vec::new();

    println!("Circuit compiled in: {:?}", start.elapsed());

    #[cfg(feature = "debug_vm2")]
    {
        for t in &circuit.templates {
            disassemble::<T>(&TF::T(t))
        }
        for f in &circuit.functions {
            disassemble::<T>(&TF::F(f))
        }
    }

    if let Some(ref inputs_file) = args.inputs_file {
        let inputs_reader = File::open(inputs_file)
            .map_err(|e| format!("Failed to open inputs file {}: {}", inputs_file, e))?;
        let start = Instant::now();
        let mut witness_buf: Vec<u8> = Vec::new();
        calculate_witness_vm2(&circuit, &inputs_reader, &mut witness_buf).unwrap();
        println!("Witness generated in: {:?}", start.elapsed());

        if let Some(ref wtns_file) = args.wtns_file {
            let mut f = File::create(wtns_file).unwrap();
            f.write_all(&witness_buf).unwrap();
            println!("Witness saved to {}", wtns_file);
        }
    }

    if args.output_file.is_some() {
        serialize_witnesscalc_vm2(&mut bytecode_buf, &circuit).unwrap();
    }

    if let Some(ref output_file) = args.output_file {
        fs::write(output_file, &bytecode_buf).unwrap();
        println!("Bytecode saved to {}", output_file);
    }

    Ok(())
}

/// Build input info directly from AST Input nodes
fn build_input_info(
    inputs: &[ast::Input],
    main_template: &ast::Template,
    types: &[ast::Type],
) -> Result<Vec<InputInfo>, Box<dyn Error>> {
    // Calculate the number of output signals to skip
    let outputs_count = calculate_outputs_count(&main_template.outputs, types)?;
    
    let mut input_infos = Vec::new();
    let mut current_offset = outputs_count + 1; // signal #0 is always 1
    
    // Process each input from the AST
    for input in inputs {
        let lengths = match &input.signal {
            ast::Signal::Ff(dims) => dims.to_vec(),
            ast::Signal::Bus(_, dims) => dims.to_vec(),
        };
        
        // Create the input info
        input_infos.push(InputInfo {
            name: input.name.clone(),
            offset: current_offset,
            lengths,
            type_id: match &input.signal {
                ast::Signal::Ff(_) => None,
                ast::Signal::Bus(bus_type, _) => Some(bus_type.clone()),
            },
        });
        
        // Calculate signal size and advance offset
        current_offset += calculate_signal_size(&input.signal, types)?;
    }
    
    Ok(input_infos)
}

/// Calculate the total number of output signals
fn calculate_outputs_count(outputs: &[ast::Signal], types: &[ast::Type]) -> Result<usize, Box<dyn Error>> {
    let mut count = 0;
    for signal in outputs {
        count += calculate_signal_size(signal, types)?;
    }
    Ok(count)
}

/// Calculate the size of a signal (number of elements)
fn calculate_signal_size(signal: &ast::Signal, types: &[ast::Type]) -> Result<usize, Box<dyn Error>> {
    match signal {
        ast::Signal::Ff(dims) => {
            Ok(if dims.is_empty() { 1 } else { dims.iter().product() })
        }
        ast::Signal::Bus(bus_type, dims) => {
            let type_def = types.iter().find(|t| t.name == *bus_type)
                .ok_or_else(|| Box::new(CompilationError::BusTypeNotFound(bus_type.clone())))?;
            let base_size = calculate_bus_size(type_def, types);
            Ok(if dims.is_empty() { base_size } else { base_size * dims.iter().product::<usize>() })
        }
    }
}

fn calculate_bus_size(bus_type: &ast::Type, _all_types: &[ast::Type]) -> usize {
    let mut total_size = 0;

    for field in &bus_type.fields {
        // The size field already contains the total size for this field
        total_size += field.size;
    }

    total_size
}

#[cfg(feature = "debug_vm2")]
enum TF<'a> {
    T(&'a Template),
    F(&'a Function),
}

#[cfg(feature = "debug_vm2")]
impl TF<'_> {
    fn code(&self) -> &[u8] {
        match self {
            TF::T(t) => &t.code,
            TF::F(f) => &f.code,
        }
    }
    fn name(&self) -> &str {
        match self {
            TF::T(t) => &t.name,
            TF::F(f) => &f.name,
        }
    }
    fn ff_variable_names(&self) -> &Vec<String> {
        match self {
            TF::T(t) => &t.ff_variable_names,
            TF::F(f) => &f.ff_variable_names,
        }
    }
    fn i64_variable_names(&self) -> &Vec<String> {
        match self {
            TF::T(t) => &t.i64_variable_names,
            TF::F(f) => &f.i64_variable_names,
        }
    }
}

#[cfg(feature = "debug_vm2")]
fn disassemble<T: FieldOps>(tf: &TF) {
    match tf {
        TF::T(t) => {
            println!("[begin]Template: {}", t.name);
        }
        TF::F(f) => {
            println!("[begin]Function: {}", f.name);
        }
    }

    let mut ip: usize = 0;
    while ip < tf.code().len() {
        ip = disassemble_instruction::<T>(
            tf.code(), ip, tf.name(), tf.ff_variable_names(),
            tf.i64_variable_names());
    }
    println!("[end]")
}

fn compile<T: FieldOps>(
    ff: &Field<T>, tree: &ast::AST) -> Result<Circuit<T>, Box<dyn Error>>
where {

    // First, compile functions and build function registry
    let mut functions = Vec::new();
    let mut function_registry = HashMap::new();

    for (i, f) in tree.functions.iter().enumerate() {
        function_registry.insert(f.name.clone(), i);
    }

    for f in tree.functions.iter() {
        let compiled_function = compile_function(f, ff, &function_registry)?;
        functions.push(compiled_function);
    }

    let type_map: HashMap<String, usize> = tree.types
        .iter()
        .enumerate()
        .map(|(idx, typ)| (typ.name.clone(), idx))
        .collect();

    let mut templates = Vec::new();

    for t in tree.templates.iter() {
        let compiled_template = compile_template(
            t, ff, &function_registry, &tree.types, &type_map)?;
        templates.push(compiled_template);
        // println!("Template: {}", t.name);
        // println!("Compiled code len: {}", compiled_template.code.len());
    }

    let mut main_template_id = None;
    for (i, t) in templates.iter().enumerate() {
        if t.name == tree.start {
            main_template_id = Some(i)
        }
    }

    let main_template_id = main_template_id
        .ok_or(CompilationError::MainTemplateIDNotFound)?;
    
    // Build input info from AST inputs
    let main_template = &tree.templates[main_template_id];
    let input_infos = build_input_info(
        &tree.inputs, main_template, &tree.types)?;
    
    let types: Vec<vm2::Type> = tree.types
        .iter()
        .map(|typ| vm2::Type::from_ast(typ, &type_map))
        .collect();

    Ok(Circuit {
        main_template_id,
        templates,
        functions,
        function_registry,
        field: ff.clone(),
        witness: tree.witness.clone(),
        signals_num: tree.signals,
        input_infos,
        types,
    })
}

#[derive(Debug, Clone, Copy)]
enum VariableType {
    Ff,
    I64,
}

struct TemplateCompilationContext<'a> {
    code: Vec<u8>,
    ff_variable_indexes: HashMap<String, i64>,
    i64_variable_indexes: HashMap<String, i64>,
    // Registry to track variable types
    variable_types: HashMap<String, VariableType>,
    // Stack of loop render frames.
    // * The first element of the tuple is the position of the first loop body
    // instruction (the continue statement).
    // * The second element is a vector of indexes where to inject the
    // position after loop body (the break statement)
    loop_control_jumps: Vec<(usize, Vec<usize>)>,
    // Function registry for resolving function names to indices
    function_registry: &'a HashMap<String, usize>,
}

impl<'a> TemplateCompilationContext<'a> {
    fn new(function_registry: &'a HashMap<String, usize>) -> Self {
        Self {
            code: vec![],
            ff_variable_indexes: HashMap::new(),
            i64_variable_indexes: HashMap::new(),
            variable_types: HashMap::new(),
            loop_control_jumps: vec![],
            function_registry,
        }
    }

    fn get_ff_variable_index(&mut self, var_name: &str) -> i64 {
        // Register the variable type
        self.variable_types.insert(var_name.to_string(), VariableType::Ff);
        
        let next_idx = self.ff_variable_indexes.len() as i64;
        *self.ff_variable_indexes
            .entry(var_name.to_string()).or_insert(next_idx)
    }
    fn get_i64_variable_index(&mut self, var_name: &str) -> i64 {
        // Register the variable type
        self.variable_types.insert(var_name.to_string(), VariableType::I64);
        
        let next_idx = self.i64_variable_indexes.len() as i64;
        *self.i64_variable_indexes
            .entry(var_name.to_string()).or_insert(next_idx)
    }

    fn append_i64operand(&mut self, operand: &I64Operand) {
        match operand {
            I64Operand::Literal(v) => {
                self.code.extend_from_slice(&v.to_le_bytes());
            }
            I64Operand::Variable(var_name) => {
                let var_idx = self.get_i64_variable_index(var_name);
                self.code.extend_from_slice(&var_idx.to_le_bytes());
            }
        }
    }
}

fn operand_i64<'a>(
    ctx: &mut TemplateCompilationContext<'a>, operand: &I64Operand) {

    match operand {
        I64Operand::Literal(v) => {
            ctx.code.push(OpCode::PushI64 as u8);
            ctx.code.extend_from_slice(v.to_le_bytes().as_slice());
        }
        I64Operand::Variable(var_name) => {
            let var_idx = ctx.get_i64_variable_index(var_name);
            ctx.code.push(OpCode::LoadVariableI64 as u8);
            ctx.code.extend_from_slice(var_idx.to_le_bytes().as_slice());
        }
    }
}

fn i64_expression<'a, F>(
    ctx: &mut TemplateCompilationContext<'a>, ff: &F,
    expr: &I64Expr) -> Result<(), Box<dyn Error>>
where
    for <'b> &'b F: FieldOperations {
    
    match expr {
        I64Expr::Variable(var_name) => {
            let var_idx = ctx.get_i64_variable_index(var_name);
            ctx.code.push(OpCode::LoadVariableI64 as u8);
            ctx.code.extend_from_slice(var_idx.to_le_bytes().as_slice());
        }
        I64Expr::Literal(value) => {
            ctx.code.push(OpCode::PushI64 as u8);
            ctx.code.extend_from_slice(value.to_le_bytes().as_slice());
        }
        I64Expr::Add(lhs, rhs) => {
            i64_expression(ctx, ff, rhs)?;
            i64_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpI64Add as u8);
        }
        I64Expr::Sub(lhs, rhs) => {
            i64_expression(ctx, ff, rhs)?;
            i64_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpI64Sub as u8);
        }
        I64Expr::Mul(lhs, rhs) => {
            i64_expression(ctx, ff, rhs)?;
            i64_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpI64Mul as u8);
        }
        I64Expr::Div(lhs, rhs) => {
            i64_expression(ctx, ff, rhs)?;
            i64_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpI64Div as u8);
        }
        I64Expr::Pow(lhs, rhs) => {
            i64_expression(ctx, ff, rhs)?;
            i64_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpI64Pow as u8);
        }
        I64Expr::Rem(lhs, rhs) => {
            i64_expression(ctx, ff, rhs)?;
            i64_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpI64Rem as u8);
        }
        I64Expr::And(lhs, rhs) => {
            i64_expression(ctx, ff, rhs)?;
            i64_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpI64And as u8);
        }
        I64Expr::Or(lhs, rhs) => {
            i64_expression(ctx, ff, rhs)?;
            i64_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpI64Or as u8);
        }
        I64Expr::Eq(lhs, rhs) => {
            operand_i64(ctx, rhs);
            operand_i64(ctx, lhs);
            ctx.code.push(OpCode::OpI64Eq as u8);
        }
        I64Expr::Neq(lhs, rhs) => {
            operand_i64(ctx, rhs);
            operand_i64(ctx, lhs);
            ctx.code.push(OpCode::OpI64Neq as u8);
        }
        I64Expr::Eqz(value) => {
            operand_i64(ctx, value);
            ctx.code.push(OpCode::OpI64Eqz as u8);
        }
        I64Expr::BNot(arg) => {
            operand_i64(ctx, arg);
            ctx.code.push(OpCode::OpI64BNot as u8);
        }
        I64Expr::BXor(rhs, lhs) => {
            i64_expression(ctx, ff, rhs)?;
            i64_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpI64BXor as u8);
        }
        I64Expr::BOr(rhs, lhs) => {
            i64_expression(ctx, ff, rhs)?;
            i64_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpI64BOr as u8);
        }
        I64Expr::BAnd(rhs, lhs) => {
            i64_expression(ctx, ff, rhs)?;
            i64_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpI64BAnd as u8);
        }
        I64Expr::Lt(lhs, rhs) => {
            i64_expression(ctx, ff, rhs)?;
            i64_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpI64Lt as u8);
        }
        I64Expr::Lte(lhs, rhs) => {
            i64_expression(ctx, ff, rhs)?;
            i64_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpI64Lte as u8);
        }
        I64Expr::Gt(lhs, rhs) => {
            i64_expression(ctx, ff, rhs)?;
            i64_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpI64Gt as u8);
        }
        I64Expr::Gte(lhs, rhs) => {
            i64_expression(ctx, ff, rhs)?;
            i64_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpI64Gte as u8);
        }
        I64Expr::Shl(lhs, rhs) => {
            i64_expression(ctx, ff, rhs)?;
            i64_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpI64Shl as u8);
        }
        I64Expr::Shr(lhs, rhs) => {
            i64_expression(ctx, ff, rhs)?;
            i64_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpI64Shr as u8);
        }
        I64Expr::Load(addr) => {
            operand_i64(ctx, addr);
            ctx.code.push(OpCode::I64Load as u8);
        }
        I64Expr::Return(addr) => {
            operand_i64(ctx, addr);
            ctx.code.push(OpCode::I64Return as u8);
        }
        I64Expr::MReturn { dst, src, size } => {
            // Push operands in reverse order so they are popped in correct order
            // The VM expects: stack[-2]=dst, stack[-1]=src, stack[0]=size
            operand_i64(ctx, dst);
            operand_i64(ctx, src);
            operand_i64(ctx, size);
            ctx.code.push(OpCode::I64MReturn as u8);
        },
        I64Expr::Store(index, value) => {
            i64_expression(ctx, ff, index)?;
            i64_expression(ctx, ff, value)?;
            ctx.code.push(OpCode::I64Store as u8);
        }
        I64Expr::Wrap(ff_expr) => {
            ff_expression(ctx, ff, ff_expr)?;
            ctx.code.push(OpCode::I64WrapFf as u8);
        }
        I64Expr::GetTemplateId(cmp_idx) => {
            operand_i64(ctx, cmp_idx);
            ctx.code.push(OpCode::GetTemplateId as u8);
        }
        I64Expr::GetTemplateSignalPosition(template_id, signal_id) => {
            operand_i64(ctx, signal_id);
            operand_i64(ctx, template_id);
            ctx.code.push(OpCode::GetTemplateSignalPosition as u8);
        }
        I64Expr::GetTemplateSignalSize(template_id, signal_id) => {
            operand_i64(ctx, signal_id);
            operand_i64(ctx, template_id);
            ctx.code.push(OpCode::GetTemplateSignalSize as u8);
        }
        I64Expr::GetTemplateSignalType(template_id, signal_id) => {
            operand_i64(ctx, signal_id);
            operand_i64(ctx, template_id);
            ctx.code.push(OpCode::GetTemplateSignalType as u8);
        }
        I64Expr::GetTemplateSignalDimension(template_id, signal_id, dimension_index) => {
            operand_i64(ctx, dimension_index);
            operand_i64(ctx, signal_id);
            operand_i64(ctx, template_id);
            ctx.code.push(OpCode::GetTemplateSignalDimension as u8);
        }
        I64Expr::GetBusSignalPosition(template_id, signal_id) => {
            operand_i64(ctx, signal_id);
            operand_i64(ctx, template_id);
            ctx.code.push(OpCode::GetBusSignalPosition as u8);
        }
        I64Expr::GetBusSignalSize(template_id, signal_id) => {
            operand_i64(ctx, signal_id);
            operand_i64(ctx, template_id);
            ctx.code.push(OpCode::GetBusSignalSize as u8);
        }
        I64Expr::GetBusSignalType(template_id, signal_id) => {
            operand_i64(ctx, signal_id);
            operand_i64(ctx, template_id);
            ctx.code.push(OpCode::GetBusSignalType as u8);
        }
        I64Expr::GetBusSignalDimension(template_id, signal_id, dimension_index) => {
            operand_i64(ctx, signal_id);
            operand_i64(ctx, template_id);
            operand_i64(ctx, dimension_index);
            ctx.code.push(OpCode::GetBusSignalDimension as u8);
        }
    }
    Ok(())
}

fn ff_expression<'a, F>(
    ctx: &mut TemplateCompilationContext<'a>, ff: &F,
    expr: &FfExpr) -> Result<(), Box<dyn Error>>
where
    for <'b> &'b F: FieldOperations {

    match expr {
        FfExpr::GetSignal(operand) => {
            operand_i64(ctx, operand);
            ctx.code.push(OpCode::LoadSignal as u8);
        }
        FfExpr::GetCmpSignal{ cmp_idx, sig_idx } => {
            operand_i64(ctx, cmp_idx);
            operand_i64(ctx, sig_idx);
            ctx.code.push(OpCode::LoadCmpSignal as u8);
        }
        FfExpr::FfMul(lhs, rhs) => {
            ff_expression(ctx, ff, rhs)?;
            ff_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpMul as u8);
        },
        FfExpr::FfAdd(lhs, rhs) => {
            ff_expression(ctx, ff, rhs)?;
            ff_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpAdd as u8);
        },
        FfExpr::FfNeq(lhs, rhs) => {
            ff_expression(ctx, ff, rhs)?;
            ff_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpNeq as u8);
        },
        FfExpr::FfDiv(lhs, rhs) => {
            ff_expression(ctx, ff, rhs)?;
            ff_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpDiv as u8);
        },
        FfExpr::Idiv(lhs, rhs) => {
            ff_expression(ctx, ff, rhs)?;
            ff_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpIdiv as u8);
        },
        FfExpr::FfSub(lhs, rhs) => {
            ff_expression(ctx, ff, rhs)?;
            ff_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpSub as u8);
        },
        FfExpr::FfEq(lhs, rhs) => {
            ff_expression(ctx, ff, rhs)?;
            ff_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpEq as u8);
        },
        FfExpr::FfEqz(lhs) => {
            ff_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpEqz as u8);
        },
        FfExpr::Variable( var_name ) => {
            let var_idx = ctx.get_ff_variable_index(var_name);
            ctx.code.push(OpCode::LoadVariableFf as u8);
            ctx.code.extend_from_slice(var_idx.to_le_bytes().as_slice());
        },
        FfExpr::Literal(v) => {
            ctx.code.push(OpCode::PushFf as u8);
            let x = ff.parse_le_bytes(v.to_le_bytes().as_slice())
                .map_err(|e| -> Box<dyn Error> {e})?;
            ctx.code.extend_from_slice(x.to_le_bytes().as_slice());
        },
        FfExpr::Load(idx) => {
            operand_i64(ctx, idx);
            ctx.code.push(OpCode::FfLoad as u8);
        },
        FfExpr::Lt(lhs, rhs) => {
            ff_expression(ctx, ff, rhs)?;
            ff_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpLt as u8);
        },
        FfExpr::Le(lhs, rhs) => {
            ff_expression(ctx, ff, rhs)?;
            ff_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpLe as u8);
        },
        FfExpr::Gt(lhs, rhs) => {
            ff_expression(ctx, ff, rhs)?;
            ff_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpGt as u8);
        },
        FfExpr::Ge(lhs, rhs) => {
            ff_expression(ctx, ff, rhs)?;
            ff_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpGe as u8);
        },
        FfExpr::FfShr(lhs, rhs) => {
            ff_expression(ctx, ff, rhs)?;
            ff_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpShr as u8);
        },
        FfExpr::Shl(lhs, rhs) => {
            ff_expression(ctx, ff, rhs)?;
            ff_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpShl as u8);
        },
        FfExpr::FfBand(lhs, rhs) => {
            ff_expression(ctx, ff, rhs)?;
            ff_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpBand as u8);
        },
        FfExpr::And(lhs, rhs) => {
            ff_expression(ctx, ff, rhs)?;
            ff_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpAnd as u8);
        },
        FfExpr::Or(lhs, rhs) => {
            ff_expression(ctx, ff, rhs)?;
            ff_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpOr as u8);
        },
        FfExpr::Bxor(lhs, rhs) => {
            ff_expression(ctx, ff, rhs)?;
            ff_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpBxor as u8);
        },
        FfExpr::Bor(lhs, rhs) => {
            ff_expression(ctx, ff, rhs)?;
            ff_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpBor as u8);
        },
        FfExpr::Bnot(operand) => {
            ff_expression(ctx, ff, operand)?;
            ctx.code.push(OpCode::OpBnot as u8);
        },
        FfExpr::Pow(lhs, rhs) => {
            ff_expression(ctx, ff, rhs)?;
            ff_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpPow as u8);
        },
        FfExpr::Rem(lhs, rhs) => {
            ff_expression(ctx, ff, rhs)?;
            ff_expression(ctx, ff, lhs)?;
            ctx.code.push(OpCode::OpRem as u8);
        },
        FfExpr::Call { name: function_name, args } => {
            // Reuse FfMCall opcode for ff.call expressions
            ctx.code.push(OpCode::FfMCall as u8);

            // Look up function by name to get its index
            let func_idx = *ctx.function_registry.get(function_name)
                .ok_or_else(|| format!("Function '{}' not found", function_name))?;
            let func_idx: u32 = func_idx.try_into()
                .map_err(|_| "Function index too large")?;
            ctx.code.extend_from_slice(&func_idx.to_le_bytes());

            // Emit argument count
            ctx.code.push(args.len() as u8);

            // Emit each argument (same as FfMCall statement)
            for arg in args {
                match arg {
                    ast::CallArgument::I64Literal(value) => {
                        ctx.code.push(0); // arg type 0 = i64 literal
                        ctx.code.extend_from_slice(&value.to_le_bytes());
                    }
                    ast::CallArgument::FfLiteral(value) => {
                        ctx.code.push(1); // arg type 1 = ff literal
                        let x = ff.parse_le_bytes(value.to_le_bytes().as_slice())
                            .map_err(|e| -> Box<dyn Error> {e})?;
                        ctx.code.extend_from_slice(x.to_le_bytes().as_slice());
                    }
                    ast::CallArgument::Variable(var_name) => {
                        // Look up variable type in registry
                        match ctx.variable_types.get(var_name) {
                            Some(VariableType::Ff) => {
                                ctx.code.push(2); // arg type 2 = ff variable
                                let var_idx = ctx.get_ff_variable_index(var_name);
                                ctx.code.extend_from_slice(&var_idx.to_le_bytes());
                            }
                            Some(VariableType::I64) => {
                                ctx.code.push(3); // arg type 3 = i64 variable
                                let var_idx = ctx.get_i64_variable_index(var_name);
                                ctx.code.extend_from_slice(&var_idx.to_le_bytes());
                            }
                            None => {
                                return Err(format!("Variable '{}' not found", var_name).into());
                            }
                        }
                    }
                    ast::CallArgument::I64Memory { addr, size } => {
                        // i64.memory uses types 8-11 (base 8 = 0b1000)
                        let arg_type = calc_arg_type(8u8, addr, size, None);
                        ctx.code.push(arg_type);
                        ctx.append_i64operand(addr);
                        ctx.append_i64operand(size);
                    }
                    ast::CallArgument::FfMemory { addr, size } => {
                        // ff.memory uses types 4-7 (base 4 = 0b0100)
                        let arg_type = calc_arg_type(4u8, addr, size, None);
                        ctx.code.push(arg_type);
                        ctx.append_i64operand(addr);
                        ctx.append_i64operand(size);
                    }
                    ast::CallArgument::Signal { idx, size } => {
                        // signal uses types 12-15 (base 12 = 0b1100)
                        let arg_type = calc_arg_type(0b0000_1100u8, idx, size, None);
                        ctx.code.push(arg_type);
                        ctx.append_i64operand(idx);
                        ctx.append_i64operand(size);
                    }
                    ast::CallArgument::CmpSignal { cmp_idx, sig_idx, size } => {
                        // signal uses types 16-23 (base 16 = 0b0001_0000)
                        let arg_type = calc_arg_type(
                            0b0001_0000u8, cmp_idx, sig_idx, Some(size));
                        ctx.code.push(arg_type);
                        ctx.append_i64operand(cmp_idx);
                        ctx.append_i64operand(sig_idx);
                        ctx.append_i64operand(size);
                    }
                }
            }
        },
    };
    Ok(())
}

fn compile_assignment<'a, F>(
    ctx: &mut TemplateCompilationContext<'a>, ff: &F,
    name: &str, value: &Expr) -> Result<(), Box<dyn Error>>
where
    for <'b> &'b F: FieldOperations {
    
    match value {
        Expr::Ff(ff_expr) => {
            ff_expression(ctx, ff, ff_expr)?;
            ctx.code.push(OpCode::StoreVariableFf as u8);
            let var_idx = ctx.get_ff_variable_index(name);
            ctx.code.extend_from_slice(var_idx.to_le_bytes().as_slice());
        }
        Expr::I64(i64_expr) => {
            i64_expression(ctx, ff, i64_expr)?;
            ctx.code.push(OpCode::StoreVariableI64 as u8);
            let var_idx = ctx.get_i64_variable_index(name);
            ctx.code.extend_from_slice(var_idx.to_le_bytes().as_slice());
        }
        Expr::Variable(source_var) => {
            // Check the type of the source variable
            match ctx.variable_types.get(source_var) {
                Some(VariableType::Ff) => {
                    // Load from FF variable and store to FF variable
                    let source_idx = ctx.get_ff_variable_index(source_var);
                    ctx.code.push(OpCode::LoadVariableFf as u8);
                    ctx.code.extend_from_slice(source_idx.to_le_bytes().as_slice());

                    ctx.code.push(OpCode::StoreVariableFf as u8);
                    let dest_idx = ctx.get_ff_variable_index(name);
                    ctx.code.extend_from_slice(dest_idx.to_le_bytes().as_slice());
                }
                Some(VariableType::I64) => {
                    // Load from I64 variable and store to I64 variable
                    let source_idx = ctx.get_i64_variable_index(source_var);
                    ctx.code.push(OpCode::LoadVariableI64 as u8);
                    ctx.code.extend_from_slice(source_idx.to_le_bytes().as_slice());

                    ctx.code.push(OpCode::StoreVariableI64 as u8);
                    let dest_idx = ctx.get_i64_variable_index(name);
                    ctx.code.extend_from_slice(dest_idx.to_le_bytes().as_slice());
                }
                None => {
                    return Err(format!("Variable '{}' not found in type registry", source_var).into());
                }
            }
        }
    }
    Ok(())
}

fn instruction<'a, F>(
    ctx: &mut TemplateCompilationContext<'a>, ff: &F,
    stmt: &ast::Statement) -> Result<(), Box<dyn Error>>
where
    for <'b> &'b F: FieldOperations {

    match stmt {
        Statement::SetSignal { idx, value } => {
            ff_expression(ctx, ff, value)?;
            operand_i64(ctx, idx);
            ctx.code.push(OpCode::StoreSignal as u8);
        },
        Statement::FfStore { idx, value } => {
            ff_expression(ctx, ff, value)?;
            operand_i64(ctx, idx);
            ctx.code.push(OpCode::FfStore as u8);
        },
        Statement::FfMStore { dst, src, size } => {
            operand_i64(ctx, dst);
            operand_i64(ctx, src);
            operand_i64(ctx, size);
            ctx.code.push(OpCode::FfMStore as u8);
        },
        Statement::FfMStoreFromSignal { dst, addr, size } => {
            operand_i64(ctx, dst);
            operand_i64(ctx, addr);
            operand_i64(ctx, size);
            ctx.code.push(OpCode::FfMStoreFromSignal as u8);
        }
        Statement::FfMStoreFromCmpSignal { dst, addr, src, size } => {
            operand_i64(ctx, dst);
            operand_i64(ctx, src);
            operand_i64(ctx, addr);
            operand_i64(ctx, size);
            ctx.code.push(OpCode::FfMStoreFromCmpSignal as u8);
        }
        Statement::CopyCmpInputFromCmp { dst_cmp_idx, dst_sig_idx, src_cmp_idx, src_sig_idx, size, mode } => {
            operand_i64(ctx, size);
            operand_i64(ctx, src_sig_idx);
            operand_i64(ctx, src_cmp_idx);
            operand_i64(ctx, dst_sig_idx);
            operand_i64(ctx, dst_cmp_idx);

            let flags = match mode {
                ast::CmpInputMode::None => 0b00u8,
                ast::CmpInputMode::UpdateCounter => 0b01u8,
                ast::CmpInputMode::Run => 0b10u8,
                ast::CmpInputMode::UpdateCounterAndCheck => 0b11u8,
            };

            ctx.code.push(OpCode::CopyCmpInputsFromCmp as u8);
            ctx.code.push(flags);
        },
        Statement::CopyCmpInputFromMemory {
            dst_cmp_idx,
            dst_sig_idx,
            sig_idx,
            size,
            mode,
        } => {
            operand_i64(ctx, size);
            operand_i64(ctx, sig_idx);
            operand_i64(ctx, dst_sig_idx);
            operand_i64(ctx, dst_cmp_idx);

            let flags = match mode {
                ast::CmpInputMode::None => 0b00u8,
                ast::CmpInputMode::UpdateCounter => 0b01u8,
                ast::CmpInputMode::Run => 0b10u8,
                ast::CmpInputMode::UpdateCounterAndCheck => 0b11u8,
            };

            ctx.code.push(OpCode::CopyCmpInputsFromMemory as u8);
            ctx.code.push(flags);
        }
        Statement::CopySignalFromCmp { dst_idx, cmp_idx, cmp_sig_idx, size } => {
            operand_i64(ctx, size);
            operand_i64(ctx, cmp_sig_idx);
            operand_i64(ctx, cmp_idx);
            operand_i64(ctx, dst_idx);
            ctx.code.push(OpCode::CopySignalFromCmp as u8);
        },
        Statement::CopySignal { dst_idx, src_idx, size } => {
            operand_i64(ctx, size);
            operand_i64(ctx, src_idx);
            operand_i64(ctx, dst_idx);
            ctx.code.push(OpCode::CopySignal as u8);
        },
        Statement::CopySignalFromMemory { dst_idx, addr, size } => {
            operand_i64(ctx, size);
            operand_i64(ctx, addr);
            operand_i64(ctx, dst_idx);
            ctx.code.push(OpCode::CopySignalFromMemory as u8);
        },
        Statement::SetCmpSignalRun { cmp_idx, sig_idx, value } => {
            operand_i64(ctx, cmp_idx);
            operand_i64(ctx, sig_idx);
            ff_expression(ctx, ff, value)?;
            ctx.code.push(OpCode::StoreCmpSignalAndRun as u8);
        },
        Statement::SetCmpInputCnt { cmp_idx, sig_idx, value } => {
            operand_i64(ctx, cmp_idx);
            operand_i64(ctx, sig_idx);
            ff_expression(ctx, ff, value)?;
            ctx.code.push(OpCode::StoreCmpInputCnt as u8);
        },
        Statement::SetCmpInputCntCheck { cmp_idx, sig_idx, value } => {
            operand_i64(ctx, cmp_idx);
            operand_i64(ctx, sig_idx);
            ff_expression(ctx, ff, value)?;
            ctx.code.push(OpCode::StoreCmpSignalCntCheck as u8);
        },
        Statement::SetCmpInput { cmp_idx, sig_idx, value } => {
            i64_expression(ctx, ff, cmp_idx)?;
            i64_expression(ctx, ff, sig_idx)?;
            ff_expression(ctx, ff, value)?;
            ctx.code.push(OpCode::StoreCmpInput as u8);
        },
        Statement::Branch { condition, if_block, else_block } => {
            // Resolve variable references to their typed equivalents
            let resolved_condition: Expr;
            let condition_to_evaluate = if let Expr::Variable(var_name) = condition {
                match ctx.variable_types.get(var_name) {
                    Some(VariableType::Ff) => {
                        resolved_condition = Expr::Ff(FfExpr::Variable(var_name.clone()));
                        &resolved_condition
                    }
                    Some(VariableType::I64) => {
                        resolved_condition = Expr::I64(I64Expr::Variable(var_name.clone()));
                        &resolved_condition
                    }
                    None => {
                        return Err(format!("Variable '{}' not found in type registry", var_name).into());
                    }
                }
            } else {
                condition
            };

            let else_jump_offset = match condition_to_evaluate {
                Expr::Ff(expr) => {
                    ff_expression(ctx, ff, expr)?;
                    pre_emit_jump_if_false(&mut ctx.code, true)
                },
                Expr::I64(expr) => {
                    i64_expression(ctx, ff, expr)?;
                    pre_emit_jump_if_false(&mut ctx.code, false)
                },
                Expr::Variable( .. ) => {
                    panic!("[assertion] expression should be already converted to Ff or I64");
                },
            };

            block(ctx, ff, if_block)?;

            if !else_block.is_empty() {
                // Only emit end jump if the if block doesn't end with return
                let end_jump_offset = pre_emit_jump(&mut ctx.code);

                // Now patch the else_jump to jump to the start of the else block
                let else_start = ctx.code.len();
                patch_jump(&mut ctx.code, else_jump_offset, else_start)?;

                block(ctx, ff, else_block)?;

                let to = ctx.code.len();
                patch_jump(&mut ctx.code, end_jump_offset, to)?;
            } else {
                // No else block, patch to jump to end
                let to = ctx.code.len();
                patch_jump(&mut ctx.code, else_jump_offset, to)?;
            }
        },
        Statement::Loop( loop_block ) => {
            // Start of loop
            let loop_start = ctx.code.len();
            ctx.loop_control_jumps.push((loop_start, vec![]));

            // Compile loop body
            block(ctx, ff, loop_block)?;

            let to = ctx.code.len();
            let (_, break_jumps) = ctx.loop_control_jumps.pop()
                .ok_or(CompilationError::LoopControlJumpsEmpty)?;
            for break_jump_offset in break_jumps {
                patch_jump(&mut ctx.code, break_jump_offset, to)?;
            }
        },
        Statement::Break => {
            let jump_offset = pre_emit_jump(&mut ctx.code);
            if let Some((_, break_jumps)) = ctx.loop_control_jumps.last_mut() {
                break_jumps.push(jump_offset);
            } else {
                return Err(Box::new(CompilationError::LoopControlJumpsEmpty));
            }
        },
        Statement::Continue => {
            let jump_offset = pre_emit_jump(&mut ctx.code);
            patch_jump(&mut ctx.code, jump_offset, ctx.loop_control_jumps.last()
                .ok_or(CompilationError::LoopControlJumpsEmpty)?.0)?;
        },
        Statement::Error { code } => {
            operand_i64(ctx, code);
            ctx.code.push(OpCode::Error as u8);
        },
        Statement::FfMReturn { dst, src, size } => {
            // Push operands in reverse order so they are popped in correct order
            // The VM expects: stack[-2]=dst, stack[-1]=src, stack[0]=size
            operand_i64(ctx, dst);
            operand_i64(ctx, src);
            operand_i64(ctx, size);
            ctx.code.push(OpCode::FfMReturn as u8);
        },
        Statement::FfReturn { value } => {
            ff_expression(ctx, ff, value)?;
            ctx.code.push(OpCode::FfReturn as u8);
        },
        Statement::FfExtendI64 { value } => {
            operand_i64(ctx, value);
            ctx.code.push(OpCode::FfExtendI64 as u8);
        },
        Statement::FfMCall { name: function_name, args } => {
            // Emit the FfMCall opcode
            ctx.code.push(OpCode::FfMCall as u8);

            // Look up function by name to get its index
            let func_idx = *ctx.function_registry.get(function_name)
                .ok_or_else(|| format!("Function '{}' not found", function_name))?;
            let func_idx: u32 = func_idx.try_into()
                .map_err(|_| "Function index too large")?;
            ctx.code.extend_from_slice(&func_idx.to_le_bytes());

            // Emit argument count
            ctx.code.push(args.len() as u8);

            // Emit each argument
            for arg in args {
                match arg {
                    ast::CallArgument::I64Literal(value) => {
                        ctx.code.push(0); // arg type 0 = i64 literal
                        ctx.code.extend_from_slice(&value.to_le_bytes());
                    }
                    ast::CallArgument::FfLiteral(value) => {
                        ctx.code.push(1); // arg type 1 = ff literal
                        let x = ff.parse_le_bytes(value.to_le_bytes().as_slice())
                            .map_err(|e| -> Box<dyn Error> {e})?;
                        ctx.code.extend_from_slice(x.to_le_bytes().as_slice());
                    }
                    ast::CallArgument::Variable(var_name) => {
                        // Look up variable type in registry
                        match ctx.variable_types.get(var_name) {
                            Some(VariableType::Ff) => {
                                ctx.code.push(2); // arg type 2 = ff variable
                                let var_idx = ctx.get_ff_variable_index(var_name);
                                ctx.code.extend_from_slice(&var_idx.to_le_bytes());
                            }
                            Some(VariableType::I64) => {
                                ctx.code.push(3); // arg type 3 = i64 variable
                                let var_idx = ctx.get_i64_variable_index(var_name);
                                ctx.code.extend_from_slice(&var_idx.to_le_bytes());
                            }
                            None => {
                                return Err(format!("Variable '{}' not found in type registry", var_name).into());
                            }
                        }
                    }
                    ast::CallArgument::I64Memory { addr, size } => {
                        // i64.memory uses types 8-11 (base 8 = 0b1000)
                        let arg_type = calc_arg_type(8u8, addr, size, None);
                        ctx.code.push(arg_type);
                        ctx.append_i64operand(addr);
                        ctx.append_i64operand(size);
                    }
                    ast::CallArgument::FfMemory { addr, size } => {
                        // ff.memory uses types 4-7 (base 4 = 0b0100)
                        let arg_type = calc_arg_type(4u8, addr, size, None);
                        ctx.code.push(arg_type);
                        ctx.append_i64operand(addr);
                        ctx.append_i64operand(size);
                    }
                    ast::CallArgument::Signal { idx, size } => {
                        // signal uses types 12-15 (base 12 = 0b1100)
                        let arg_type = calc_arg_type(12u8, idx, size, None);
                        ctx.code.push(arg_type);
                        ctx.append_i64operand(idx);
                        ctx.append_i64operand(size);
                    }
                    ast::CallArgument::CmpSignal { cmp_idx, sig_idx, size } => {
                        // signal uses types 16-23 (base 16 = 0b0001_0000)
                        let arg_type = calc_arg_type(
                            0b0001_0000u8, cmp_idx, sig_idx, Some(size));
                        ctx.code.push(arg_type);
                        ctx.append_i64operand(cmp_idx);
                        ctx.append_i64operand(sig_idx);
                        ctx.append_i64operand(size);
                    }
                }
            }
        }
        Statement::Assignment { name, value } => {
            compile_assignment(ctx, ff, name, value)?;
        }
        Statement::CopyCmpInputFromSelf { cmp_idx, cmp_sig_idx, self_sig_idx, size, mode } => {
            operand_i64(ctx, size);
            operand_i64(ctx, self_sig_idx);
            operand_i64(ctx, cmp_sig_idx);
            operand_i64(ctx, cmp_idx);

            let flags = match mode {
                ast::CmpInputMode::None => 0b00u8,
                ast::CmpInputMode::UpdateCounter => 0b01u8,
                ast::CmpInputMode::Run => 0b10u8,
                ast::CmpInputMode::UpdateCounterAndCheck => 0b11u8,
            };

            ctx.code.push(OpCode::CopyCmpInputsFromSelf as u8);
            ctx.code.push(flags);
        }
    }
    Ok(())
}

// Calculate the argument type based on the base value. The base value should
// have two lowest bits set to zero. Depending on first and second arguments,
// we set the bits 0 or 1 to indicate if the argument is a variable or a literal.
fn calc_arg_type(base: u8, arg1: &I64Operand, arg2: &I64Operand, arg3: Option<&I64Operand>) -> u8 {
    let mut arg_type = base;
    match arg1 {
        I64Operand::Variable(_) => { arg_type |= 1; }
        I64Operand::Literal(_) => (),
    }
    match arg2 {
        I64Operand::Variable(_) => { arg_type |= 2; }
        I64Operand::Literal(_) => (),
    }
    if let Some(arg3) = arg3 {
        match arg3 {
            I64Operand::Variable(_) => { arg_type |= 4; }
            I64Operand::Literal(_) => (),
        }
    }
    arg_type
}

fn block<'a, F>(
    ctx: &mut TemplateCompilationContext<'a>, ff: &F,
    statements: &[ast::Statement]) -> Result<(), Box<dyn Error>>
where
    for <'b> &'b F: FieldOperations {

    for inst in statements {
        instruction(ctx, ff, inst)?;
    }

    Ok(())
}

fn calc_jump_offset(from: usize, to: usize) -> Result<i32, CompilationError> {
    let from: i64 = from.try_into()
        .map_err(|_| CompilationError::JumpOffsetIsTooLarge)?;
    let to: i64 = to.try_into()
        .map_err(|_| CompilationError::JumpOffsetIsTooLarge)?;

    (to - from).try_into()
        .map_err(|_| CompilationError::JumpOffsetIsTooLarge)
}


/// We expect the jump offset located at `jump_offset_addr` to be 4 bytes long.
/// The jump offset is calculated as `to - jump_offset_addr - 4`.
fn patch_jump(
    code: &mut [u8], jump_offset_addr: usize,
    to: usize) -> Result<(), CompilationError> {

    let offset = calc_jump_offset(jump_offset_addr + 4, to)?;
    code[jump_offset_addr..jump_offset_addr+4].copy_from_slice(offset.to_le_bytes().as_ref());
    Ok(())
}


fn pre_emit_jump_if_false(code: &mut Vec<u8>, is_ff: bool) -> usize {
    if is_ff {
        code.push(OpCode::JumpIfFalseFf as u8);
    } else {
        code.push(OpCode::JumpIfFalseI64 as u8);
    }
    for _ in 0..4 { code.push(0xffu8); }
    code.len() - 4
}

fn pre_emit_jump(code: &mut Vec<u8>) -> usize {
    code.push(OpCode::Jump as u8);
    for _ in 0..4 { code.push(0xffu8); }
    code.len() - 4
}


fn compile_function<F>(
    f: &ast::Function, 
    ff: &F, 
    function_registry: &HashMap<String, usize>
) -> Result<vm2::Function, Box<dyn Error>>
where
    for <'a> &'a F: FieldOperations {

    let mut ctx = TemplateCompilationContext::new(function_registry);
    for i in &f.body {
        instruction(&mut ctx, ff, i)?;
    }

    // Build reverse mappings for variable names (index -> name)
    let mut ff_variable_names = vec![String::new(); ctx.ff_variable_indexes.len()];
    for (name, &index) in &ctx.ff_variable_indexes {
        let idx: usize = index.try_into()
            .map_err(|_| format!("Invalid ff variable index: {}", index))?;
        if idx >= ff_variable_names.len() {
            return Err(format!("FF variable index {} out of bounds (max {})",
                              idx, ff_variable_names.len() - 1).into());
        }
        ff_variable_names[idx] = name.clone();
    }

    let mut i64_variable_names = vec![String::new(); ctx.i64_variable_indexes.len()];
    for (name, &index) in &ctx.i64_variable_indexes {
        let idx: usize = index.try_into()
            .map_err(|_| format!("Invalid i64 variable index: {}", index))?;
        if idx >= i64_variable_names.len() {
            return Err(format!("I64 variable index {} out of bounds (max {})",
                              idx, i64_variable_names.len() - 1).into());
        }
        i64_variable_names[idx] = name.clone();
    }

    Ok(vm2::Function {
        name: f.name.clone(),
        code: ctx.code,
        ff_variable_names,
        i64_variable_names,
    })
}

fn compile_template<F>(
    t: &ast::Template, 
    ff: &F, 
    function_registry: &HashMap<String, usize>,
    types: &[ast::Type],
    type_map: &HashMap<String, usize>,
) -> Result<vm2::Template, Box<dyn Error>>
where
    for <'a> &'a F: FieldOperations {

    let mut ctx = TemplateCompilationContext::new(function_registry);
    for i in &t.body {
        instruction(&mut ctx, ff, i)?;
    }

    // Build reverse mappings for variable names (index -> name)
    let mut ff_variable_names = vec![String::new(); ctx.ff_variable_indexes.len()];
    for (name, &index) in &ctx.ff_variable_indexes {
        let idx: usize = index.try_into()
            .map_err(|_| format!("Invalid ff variable index: {}", index))?;
        if idx >= ff_variable_names.len() {
            return Err(format!("FF variable index {} out of bounds (max {})",
                              idx, ff_variable_names.len() - 1).into());
        }
        ff_variable_names[idx] = name.clone();
    }

    let mut i64_variable_names = vec![String::new(); ctx.i64_variable_indexes.len()];
    for (name, &index) in &ctx.i64_variable_indexes {
        let idx: usize = index.try_into()
            .map_err(|_| format!("Invalid i64 variable index: {}", index))?;
        if idx >= i64_variable_names.len() {
            return Err(format!("I64 variable index {} out of bounds (max {})",
                              idx, i64_variable_names.len() - 1).into());
        }
        i64_variable_names[idx] = name.clone();
    }

    let inputs = t.inputs.iter()
        .map(|signal| vm2::Signal::from_ast(signal, type_map))
        .collect();
    let outputs = t.outputs.iter()
        .map(|signal| vm2::Signal::from_ast(signal, type_map))
        .collect();

    Ok(vm2::Template {
        name: t.name.clone(),
        code: ctx.code,
        signals_num: t.signals_num,
        number_of_inputs: t.number_of_inputs(types),
        components: t.components.clone(),
        inputs,
        outputs,
        ff_variable_names,
        i64_variable_names,
    })
}

#[cfg(test)]
mod tests {
    use circom_witnesscalc::ast::{FfExpr, I64Operand, Statement, I64Expr, Expr};
    use circom_witnesscalc::vm2::disassemble_instruction_to_string;
    use circom_witnesscalc::field::{bn254_prime, Field, U254};
    use super::*;

    #[test]
    fn test_example() {
        // Placeholder test
    }

    #[test]
    fn test_compile_template() {
        let ast_tmpl = ast::Template {
            name: "Multiplier_0".to_string(),
            outputs: vec![],
            inputs: vec![],
            signals_num: 0,
            components: vec![],
            body: vec![
                assign_ff("x_0", &get_signal("1")),
                assign_ff("x_1", &get_signal("2")),
                assign_ff("x_2", &ff_mul("x_0", "x_1")),
                assign_ff("x_3", &ff_add("x_2", "2")),
                set_signal("0", "x_3"),
            ],
        };
        let ff = Field::new(bn254_prime);
        let function_registry = HashMap::new();
        let empty_type_map = HashMap::new();
        let vm_tmpl = compile_template(
            &ast_tmpl, &ff, &function_registry, &[], &empty_type_map).unwrap();

        let want_output = "00000000 [Multiplier_0] PushI64: 1
00000009 [Multiplier_0] LoadSignal
0000000a [Multiplier_0] StoreVariableFf: 0 (x_0)
00000013 [Multiplier_0] PushI64: 2
0000001c [Multiplier_0] LoadSignal
0000001d [Multiplier_0] StoreVariableFf: 1 (x_1)
00000026 [Multiplier_0] LoadVariableFf: 1 (x_1)
0000002f [Multiplier_0] LoadVariableFf: 0 (x_0)
00000038 [Multiplier_0] OpMul
00000039 [Multiplier_0] StoreVariableFf: 2 (x_2)
00000042 [Multiplier_0] PushFf: 2
00000063 [Multiplier_0] LoadVariableFf: 2 (x_2)
0000006c [Multiplier_0] OpAdd
0000006d [Multiplier_0] StoreVariableFf: 3 (x_3)
00000076 [Multiplier_0] LoadVariableFf: 3 (x_3)
0000007f [Multiplier_0] PushI64: 0
00000088 [Multiplier_0] StoreSignal
";

        let actual_output = capture_disassembly("Multiplier_0", &vm_tmpl.code, &vm_tmpl.ff_variable_names, &vm_tmpl.i64_variable_names);

        assert_eq!(actual_output, want_output);

        // disassemble::<U254>(&[vm_tmpl]);
    }

    #[test]
    fn test_compile_branch() {
        let ff = Field::new(bn254_prime);
        let function_registry = HashMap::new();
        let mut ctx = TemplateCompilationContext::new(&function_registry);
        let inst = Statement::Branch {
            condition: Expr::I64(I64Expr::Literal(3)),
            if_block: vec![
                assign_ff("x", &self::ff("5")),
            ],
            else_block: vec![
                assign_ff("x", &self::ff("10")),
            ],
        };
        instruction(&mut ctx, &ff, &inst).unwrap();
        ctx.code.push(OpCode::NoOp as u8);

        let want_output = concat!(
            "00000000 [test      ] PushI64: 3\n",
            "00000009 [test      ] JumpIfFalseI64: +47 -> 0000003d\n",
            "0000000e [test      ] PushFf: 5\n",
            "0000002f [test      ] StoreVariableFf: 0\n",
            "00000038 [test      ] Jump: +42 -> 00000067\n",
            "0000003d [test      ] PushFf: 10\n",
            "0000005e [test      ] StoreVariableFf: 0\n",
            "00000067 [test      ] NoOp\n",
        );

        let ff_variable_names = vec![];
        let i64_variable_names = vec![];
        let actual_output = capture_disassembly(
            "test", &ctx.code, &ff_variable_names, &i64_variable_names);

        assert_eq!(actual_output, want_output);
    }

    fn capture_disassembly(
        name: &str, code: &[u8], ff_variable_names: &Vec<String>,
        i64_variable_names: &Vec<String>) -> String {

        let mut actual_output = String::new();
        let mut ip: usize = 0;
        while ip < code.len() {
            let (new_ip, instruction_output) = disassemble_instruction_to_string::<U254>(
                code, ip, name, ff_variable_names, i64_variable_names);
            actual_output.push_str(&instruction_output);
            actual_output.push('\n');
            ip = new_ip;
        }
        actual_output
    }

    #[test]
    fn test_compile_assignment() {
        let ff = Field::new(bn254_prime);
        let function_registry = HashMap::new();
        let mut ctx = TemplateCompilationContext::new(&function_registry);

        // Test 1: Variable assignment (FF to FF)
        // First, create a source FF variable
        let inst1 = assign_ff("x_src", &self::ff("42"));
        instruction(&mut ctx, &ff, &inst1).unwrap();

        // Now test assignment from variable to variable
        let inst2 = Statement::Assignment {
            name: "x_dest".to_string(),
            value: Expr::Variable("x_src".to_string()),
        };
        instruction(&mut ctx, &ff, &inst2).unwrap();

        // Test 2: Variable assignment (I64 to I64)
        let inst3 = assign_i64("i_src", &I64Expr::Literal(100));
        instruction(&mut ctx, &ff, &inst3).unwrap();

        let inst4 = Statement::Assignment {
            name: "i_dest".to_string(),
            value: Expr::Variable("i_src".to_string()),
        };
        instruction(&mut ctx, &ff, &inst4).unwrap();

        // Test 3: Direct FF expression assignment
        let inst5 = Statement::Assignment {
            name: "x_direct".to_string(),
            value: Expr::Ff(FfExpr::Literal(BigUint::from(99u32))),
        };
        instruction(&mut ctx, &ff, &inst5).unwrap();

        // Test 4: Direct I64 expression assignment
        let inst6 = Statement::Assignment {
            name: "i_direct".to_string(),
            value: Expr::I64(I64Expr::Literal(200)),
        };
        instruction(&mut ctx, &ff, &inst6).unwrap();

        // Verify that variable types were tracked correctly
        assert!(matches!(ctx.variable_types.get("x_src"), Some(&VariableType::Ff)));
        assert!(matches!(ctx.variable_types.get("x_dest"), Some(&VariableType::Ff)));
        assert!(matches!(ctx.variable_types.get("i_src"), Some(&VariableType::I64)));
        assert!(matches!(ctx.variable_types.get("i_dest"), Some(&VariableType::I64)));
        assert!(matches!(ctx.variable_types.get("x_direct"), Some(&VariableType::Ff)));
        assert!(matches!(ctx.variable_types.get("i_direct"), Some(&VariableType::I64)));
    }

    fn assign_ff(var_name: &str, expr: &FfExpr) -> Statement {
        Statement::Assignment {
            name: var_name.to_string(),
            value: Expr::Ff(expr.clone()),
        }
    }

    fn assign_i64(var_name: &str, expr: &I64Expr) -> Statement {
        Statement::Assignment {
            name: var_name.to_string(),
            value: Expr::I64(expr.clone()),
        }
    }

    fn is_alpha_or_underscore(s: &str) -> bool {
        if let Some(c) = s.chars().next() {
            if c.is_alphabetic() || c == '_' {
                return true;
            }
        }
        false
    }

    fn i64_op(n: &str) -> I64Operand {
        let is_var = is_alpha_or_underscore(n);
        if is_var {
            I64Operand::Variable(n.to_string())
        } else {
            I64Operand::Literal(n.parse().unwrap())
        }
    }

    fn get_signal(op1: &str) -> FfExpr {
        FfExpr::GetSignal(i64_op(op1))
    }

    fn set_signal(op1: &str, op2: &str) -> Statement {
        Statement::SetSignal {
            idx: i64_op(op1),
            value: ff(op2),
        }
    }

    fn big_uint(n: &str) -> BigUint {
        BigUint::from_str_radix(n, 10).unwrap()
    }

    fn ff(n: &str) -> FfExpr {
        let is_var = is_alpha_or_underscore(n);

        if is_var {
            FfExpr::Variable(n.to_string())
        } else {
            FfExpr::Literal(big_uint(n))
        }
    }

    fn ff_mul(op1: &str, op2: &str) -> FfExpr {
        FfExpr::FfMul(
            Box::new(ff(op1)),
            Box::new(ff(op2))
        )
    }

    fn ff_add(op1: &str, op2: &str) -> FfExpr {
        FfExpr::FfAdd(
            Box::new(ff(op1)),
            Box::new(ff(op2))
        )
    }

    #[test]
    fn test_build_input_info() {
        // Parse the example CVM file
        let cvm_content = r#";; Prime value
%%prime 21888242871839275222246405745257275088548364400416034343698204186575808495617


;; Memory of signals
%%signals 44


;; Heap of components
%%components_heap 16


;; Types (for each field we store name type offset size nDims dims)
%%type $bus_0
       $x $ff 0 1 0
       $y $ff 1 1 0
%%type $bus_1
       $start $$bus_0 0 2 0
       $end $$bus_0 2 2 0
%%type $bus_2
       $v $$bus_1 0 4 1  2


;; Main template
%%start Main_2


;; Component creation mode (implicit/explicit)
%%components implicit


;; Witness (signal list)
%%witness 0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 30 31 32 33 35 36

;; Input signals
%%input 3
"a" ff 1 5
"inB" ff 0
"v" bus_2 0

%%template Tmpl1_0 [ ff 0 ] [ ff 1 2] [3] [ ]
x_0 = i64.0
x_1 = get_signal i64.1

%%template Tmpl2_1 [ ff 0 ] [ ff 1 3] [4] [ ]
x_4 = i64.0
x_5 = get_signal i64.1


%%template Main_2 [ ff 2 2 3 bus_1 0 ] [ ff 1 5 ff 0  bus_2 0 ] [33] [ 0 -1 0 1 ]
x_10 = i64.2
x_11 = get_signal i64.16
"#;

        // Parse the CVM content
        let program = parse(cvm_content).unwrap();

        // Find the main template ID
        let mut main_template_id = None;
        for (i, t) in program.templates.iter().enumerate() {
            if t.name == program.start {
                main_template_id = Some(i);
                break;
            }
        }
        let main_template_id = main_template_id.unwrap();

        // Build input info
        let input_infos = build_input_info(
            &program.inputs,
            &program.templates[main_template_id],
            &program.types
        ).unwrap();

        // Verify expected results
        let expected = vec![
            InputInfo {
                name: "a".to_string(),
                offset: 11,
                lengths: vec![5],
                type_id: None,
            },
            InputInfo {
                name: "inB".to_string(),
                offset: 16,
                lengths: vec![],
                type_id: None,
            },
            InputInfo {
                name: "v".to_string(),
                offset: 17,
                lengths: vec![],
                type_id: Some("bus_2".to_string()),
            },
        ];

        assert_eq!(input_infos, expected);
    }
}
