use std::env;
use std::fs::File;
use std::io::Write;
use std::time::Instant;
use circom_witnesscalc::{calc_witness_with_cache, CalcWitnessOptions};

struct Args {
    wcd_file: String,
    inputs_file: String,
    witness_file: String,
    cache_out: Option<String>,
    reuse_cache: Option<String>,
}

fn parse_args() -> Args {
    let args: Vec<String> = env::args().collect();
    let mut wcd_file: Option<String> = None;
    let mut inputs_file: Option<String> = None;
    let mut wtns_file: Option<String> = None;

    let usage = |err_msg: &str| {
        if !err_msg.is_empty() {
            eprintln!("ERROR:");
            eprintln!("    {}", err_msg);
            eprintln!();
        }
        eprintln!("USAGE:");
        eprintln!("    {} <wcd_file> <inputs_json> <wtns_file> [OPTIONS]", args[0]);
        eprintln!();
        eprintln!("ARGUMENTS:");
        eprintln!("    <wcd_file>     Path to the WCD file with compiled bytecode");
        eprintln!("    <inputs_json>  JSON file containing inputs for the circuit");
        eprintln!("    <wtns_file>    File where the witness will be saved");
        eprintln!();
        eprintln!("OPTIONS:");
        eprintln!("    --cache-out <file>         Save node-value cache to <file> after computing witness");
        eprintln!("    --reuse-cache <file>       Reuse cached node values from <file> to speed up computation");
        eprintln!("    -h | --help                Display this help message");
        let exit_code = if !err_msg.is_empty() { 1i32 } else { 0i32 };
        std::process::exit(exit_code);
    };

    let mut i = 1;
    let mut cache_out = None;
    let mut reuse_cache = None;
    while i < args.len() {
        if args[i] == "--help" || args[i] == "-h" {
            usage("");
        } else if args[i] == "--cache-out" {
            i += 1;
            if i >= args.len() {
                usage("missing value for --cache-out");
            }
            cache_out = Some(args[i].clone());
        } else if args[i] == "--reuse-cache" {
            i += 1;
            if i >= args.len() {
                usage("missing value for --reuse-cache");
            }
            reuse_cache = Some(args[i].clone());
        } else if args[i].starts_with("-") {
            usage(format!("Unknown option: {}", args[i]).as_str());
        } else if wcd_file.is_none() {
            wcd_file = Some(args[i].clone());
        } else if inputs_file.is_none() {
            inputs_file = Some(args[i].clone());
        } else if wtns_file.is_none() {
            wtns_file = Some(args[i].clone());
        } else {
            usage(format!("Unknown argument: {}", args[i]).as_str());
        }
        i += 1;
    }

    Args {
        wcd_file: wcd_file.unwrap_or_else(|| { usage("missing WCD file"); String::new() }),
        inputs_file: inputs_file.unwrap_or_else(|| { usage("missing inputs file"); String::new() }),
        witness_file: wtns_file.unwrap_or_else(|| { usage("missing output .wtns file"); String::new() }),
        cache_out,
        reuse_cache,
    }
}

fn main() {
    let args = parse_args();

    let inputs_data = std::fs::read_to_string(&args.inputs_file)
        .expect("Failed to read inputs file");

    let wcd_data = std::fs::read(&args.wcd_file)
        .expect("Failed to read wcd file");

    let start = Instant::now();

    let reuse_cache_data = args
        .reuse_cache
        .as_ref()
        .map(|path| std::fs::read(path).expect("Failed to read cache file"));
    let options = CalcWitnessOptions {
        reuse_cache: reuse_cache_data.as_deref(),
        generate_cache: args.cache_out.is_some(),
    };
    let result = calc_witness_with_cache(&inputs_data, &wcd_data, options).unwrap();

    let duration = start.elapsed();
    println!("Witness generated in: {:?}", duration);

    {
        let mut f = File::create(&args.witness_file).unwrap();
        f.write_all(&result.witness).unwrap();
    }

    println!("witness saved to {}", &args.witness_file);

    if let Some(path) = args.cache_out.as_ref() {
        let cache_bytes = result.cache.as_ref()
            .expect("cache data not available for this circuit type");
        let mut f = File::create(path).expect("Failed to create cache file");
        f.write_all(cache_bytes).expect("Failed to write cache file");
        println!("cache saved to {}", path);
    }
}
