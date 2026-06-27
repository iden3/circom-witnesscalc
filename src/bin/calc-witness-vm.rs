use std::collections::HashMap;
use std::{env};
use std::fs::File;
use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;
use std::time::Instant;
use ruint::aliases::U256;
use circom_witnesscalc::{wtns_from_u256_witness, Error};
use circom_witnesscalc::Error::InputsUnmarshal;
use circom_witnesscalc::storage::{deserialize_witnesscalc_vm, InputList};
use circom_witnesscalc::vm::{build_component, execute, Template};

struct Args {
    vm_file: String,
    inputs_file: String,
    witness_file: String,
    component_counter: bool
}

fn parse_args() -> Args {
    let args: Vec<String> = env::args().collect();
    let mut wcd_file: Option<String> = None;
    let mut inputs_file: Option<String> = None;
    let mut wtns_file: Option<String> = None;
    let mut component_counter = false;

    let usage = |err_msg: &str| {
        if !err_msg.is_empty() {
            eprintln!("ERROR:");
            eprintln!("    {}", err_msg);
            eprintln!();
        }
        eprintln!("USAGE:");
        eprintln!("    {} <wcd_file> <input_json> <output_path> [OPTIONS]", args[0]);
        eprintln!();
        eprintln!("ARGUMENTS:");
        eprintln!("    <wcd_file>    Path to the WCD file with calculation graph");
        eprintln!("    <input_json>  JSON file containing inputs for the circuit");
        eprintln!("    <output_path> File where the witness will be saved");
        eprintln!();
        eprintln!("OPTIONS:");
        eprintln!("    -h | --help                Display this help message");
        eprintln!("    --component-counter   Output statistics of component counters");
        let exit_code = if !err_msg.is_empty() { 1i32 } else { 0i32 };
        std::process::exit(exit_code);
    };

    let mut i = 1;
    while i < args.len() {
        if args[i] == "--component-counter" {
            component_counter = true;
        } else if args[i] == "--help" || args[i] == "-h" {
            usage("");
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
        vm_file: wcd_file.unwrap_or_else(|| { usage("missing WCD file"); String::new() }),
        inputs_file: inputs_file.unwrap_or_else(|| { usage("missing inputs file"); String::new() }),
        witness_file: wtns_file.unwrap_or_else(|| { usage("missing output .wtns file"); String::new() }),
        component_counter,
    }
}

fn calc_len(vs: &Vec<serde_json::Value>) -> usize {
    let mut len = vs.len();

    for v in vs {
        if let serde_json::Value::Array(arr) = v {
            len += calc_len(arr)-1;
        }
    }

    len
}

fn flatten_array(
    key: &str, vs: &Vec<serde_json::Value>) -> Result<Vec<U256>, Error> {

    let mut vals: Vec<U256> = Vec::with_capacity(calc_len(vs));

    for v in vs {
        match v {
            serde_json::Value::String(s) => {
                let i = U256::from_str_radix(s.as_str(), 10)
                    .map_err(
                        |_| {InputsUnmarshal(format!(
                            "can't convert to field number: {}",
                            s.as_str()))})?;
                vals.push( i);
            }
            serde_json::Value::Number(n) => {
                vals.push(U256::from(
                    n.as_u64()
                        .ok_or(Error::InputsUnmarshal(format!(
                            "signal value is not a positive integer: {}",
                            key).to_string()))?));
            }
            serde_json::Value::Array(arr) => {
                vals.extend_from_slice(flatten_array(key, arr)?.as_slice());
            }
            _ => {
                return Err(Error::InputsUnmarshal(
                    format!("inputs must be a string: {}", key).to_string()));
            }
        };

    }
    Ok(vals)
}

pub fn deserialize_inputs(inputs_data: &[u8]) -> Result<HashMap<String, Vec<U256>>, Error> {
    let v: serde_json::Value = serde_json::from_slice(inputs_data).unwrap();

    let map = v.as_object()
        .ok_or_else(
            || Error::InputsUnmarshal("inputs must be an object".to_string()))?;

    let mut inputs: HashMap<String, Vec<U256>> = HashMap::new();
    for (k, v) in map {
        match v {
            serde_json::Value::String(s) => {
                let i = U256::from_str_radix(s.as_str(), 10)
                    .map_err(
                        |_| {InputsUnmarshal(format!(
                            "can't convert to field number: {}",
                            s.as_str()))})?;
                inputs.insert(k.clone(), vec![i]);
            }
            serde_json::Value::Number(n) => {
                if !n.is_u64() {
                    return Err(Error::InputsUnmarshal("signal value is not a positive integer".to_string()));
                }
                let i = U256::from(n.as_u64().unwrap());
                inputs.insert(k.clone(), vec![i]);
            }
            serde_json::Value::Array(ss) => {
                let vals: Vec<U256> = flatten_array(k.as_str(), ss)?;
                inputs.insert(k.clone(), vals);
            }
            _ => {
                return Err(Error::InputsUnmarshal(format!(
                    "value for key {} must be an a number as a string, as a number of an array of strings of numbers",
                    k.clone())));
            }
        }
    }
    Ok(inputs)
}

fn templates_stats(tmpls: &[Template], main_template_id: usize) {
    let mut counters: HashMap<String, usize> = HashMap::new();
    templates_stats_int(tmpls, main_template_id, &mut counters);
    println!("Template usage [begin]");
    for (k, v) in counters.iter() {
        println!("{}: {}", k, v);
    }
    println!("Template usage [begin]");
}

fn templates_stats_int(
    templates: &[Template], template_id: usize,
    counters: &mut HashMap<String, usize>) {

    let key = templates[template_id].name.clone();
    *counters.entry(key).or_insert(0) += 1;

    for i in templates[template_id].components.iter() {
        for _j in 0..i.number_of_cmp {
            templates_stats_int(templates, i.template_id, counters);
        }
    }
}

fn main() {
    let args = parse_args();

    let inputs_data = std::fs::read_to_string(&args.inputs_file)
        .expect("Failed to read input file");

    let vm_data = std::fs::read(&args.vm_file).expect("Failed to read vm file");

    let start0 = Instant::now();

    let start = Instant::now();

    let inputs = deserialize_inputs(inputs_data.as_bytes()).unwrap();
    println!("Inputs loaded in {:?}, number of input keys {}.", start.elapsed(), inputs.len());


    let start = Instant::now();
    let cs =
        deserialize_witnesscalc_vm(std::io::Cursor::new(&vm_data)).unwrap();
    println!(
        "VM file read and parsed in {:?}. Templates: {}, functions: {}.",
        start.elapsed(), cs.templates.len(), cs.functions.len());


    if args.component_counter {
        templates_stats(&cs.templates, cs.main_template_id);
    }

    let start = Instant::now();
    let main_component_signals_start = 1;
    let main_component = build_component(
        &cs.templates, cs.main_template_id, main_component_signals_start);
    let main_component = Rc::new(RefCell::new(main_component));
    println!("Component tree built in {:?}.", start.elapsed());

    let start = Instant::now();
    let mut signals = Vec::new();
    signals.resize(cs.signals_num, None);
    init_input_signals(&cs.inputs, &inputs, &mut signals);
    println!("Signals initialized in {:?}.", start.elapsed());

    let start = Instant::now();
    // TODO: pass expected signals
    if let Err(err) = execute(
        main_component, &cs.templates, &cs.functions, &cs.constants,
        &mut signals, &cs.io_map, None) {
        eprintln!("VM execution failed: {}", err);
        std::process::exit(1);
    }
    println!("VM execution done in {:?}.", start.elapsed());

    let start = Instant::now();

    let mut witness = Vec::with_capacity(cs.witness_signals.len());
    for w in cs.witness_signals.iter() {
        witness.push(signals[*w].unwrap());
    }

    let wtns_bytes = wtns_from_u256_witness(witness);

    {
        let mut f = File::create(&args.witness_file).unwrap();
        f.write_all(&wtns_bytes).unwrap();
    }

    println!("Witness created from signals in {:?}", start.elapsed());

    println!("Total time {:?}", start0.elapsed());
}

fn init_input_signals(
    inputs_desc: &InputList,
    inputs: &HashMap<String, Vec<U256>>,
    signals: &mut [Option<U256>],
) {
    signals[0] = Some(U256::from(1u64));

    for (name, offset, len) in inputs_desc {
        match inputs.get(name) {
            Some(values) => {
                if values.len() != *len {
                    panic!(
                        "input signal {} has different length in inputs file, want {}, actual {}",
                        name, len, values.len());
                }
                for (i, v) in values.iter().enumerate() {
                    signals[*offset + i] = Some(*v);
                }
            }
            None => {
                panic!("input signal {} is not found in inputs file", name);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_ok() {
        println!("OK");
    }
}
