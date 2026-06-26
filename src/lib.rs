#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
// #[allow(dead_code)]
pub mod field;
pub mod graph;
pub mod storage;
pub mod vm;
pub mod vm2;
mod vm2_setup;
pub mod ast;

pub mod parser;

use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void, CStr};
use std::io::Cursor;
use std::slice::from_raw_parts;
use anyhow::anyhow;
use ruint::aliases::U256;
use ruint::ParseError;
use crate::graph::{evaluate, Nodes, NodesInterface, NodesStorage, VecNodes};
use wtns_file::FieldElement;
use ark_bn254::Fr;
use ark_ff::{BigInteger, PrimeField};
use indicatif::{ProgressBar, ProgressStyle};
use crate::field::{bn254_prime, Field, FieldOperations, FieldOps, U254, U64};
use crate::storage::proto_deserializer::{deserialize_witnesscalc_graph_from_bytes, InputInfo};
use crate::storage::{deserialize_witnesscalc_vm2_body, read_witnesscalc_vm2_header, WITNESSCALC_CVM_MAGIC, WITNESSCALC_GRAPH_MAGIC_002, WITNESSCALC_GRAPH_MAGIC_001};
use crate::vm2::{execute, Circuit, Component};
use crate::vm2::InputInfoSliceExt;
use crate::vm2_setup::{build_component_tree, init_signals, validate_types};

pub type InputSignalsInfo = HashMap<String, (usize, usize)>;

pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/circom_witnesscalc.proto.rs"));

    pub mod vm {
        include!(concat!(env!("OUT_DIR"), "/circom_witnesscalc.proto.vm.rs"));
    }
}

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
// include!("bindings.rs");

fn prepare_status(status: *mut gw_status_t, code: GW_ERROR_CODE, error_msg: &str) {
    if !status.is_null() {
        let bs = error_msg.as_bytes();
        unsafe {
            (*status).code = code;
            (*status).error_msg = libc::malloc(bs.len()+1) as *mut c_char;
            libc::memcpy((*status).error_msg as *mut c_void, bs.as_ptr() as *mut c_void, bs.len());
            *((*status).error_msg.add(bs.len())) = 0;
        }
    }
}

/// # Safety
/// 
/// This function is unsafe because it dereferences raw pointers and can cause
/// undefined behavior if misused.
#[no_mangle]
pub unsafe extern "C" fn gw_calc_witness(
    inputs: *const c_char,
    graph_data: *const c_void, graph_data_len: usize,
    wtns_data: *mut *mut c_void, wtns_len: *mut usize,
    status: *mut gw_status_t) -> c_int {

    if inputs.is_null() {
        prepare_status(status, GW_ERROR_CODE_ERROR, "inputs is null");
        return 1;
    }

    if graph_data.is_null() {
        prepare_status(status, GW_ERROR_CODE_ERROR, "graph_data is null");
        return 1;
    }

    if graph_data_len == 0 {
        prepare_status(status, GW_ERROR_CODE_ERROR, "graph_data_len is 0");
        return 1;
    }

    let graph_data_r: &[u8];
    unsafe {
        graph_data_r = from_raw_parts(graph_data as *const u8, graph_data_len);
    }


    let inputs_str: &str;
    unsafe {
        let c = CStr::from_ptr(inputs);
        match c.to_str() {
            Ok(x) => {
                inputs_str = x;
            }
            Err(e) => {
                prepare_status(
                    status, GW_ERROR_CODE_ERROR,
                    format!(
                        "Failed to parse inputs as UTF-8 string: {}",
                        e).as_str());
                return 1;
            }
        }
    }

    let witness_data = match calc_witness(inputs_str, graph_data_r) {
        Ok(witness) => witness,
        Err(e) => {
            prepare_status(status, GW_ERROR_CODE_ERROR, format!("Failed to calculate witness: {:?}", e).as_str());
            return 1;
        }
    };

    unsafe {
        *wtns_len = witness_data.len();
        *wtns_data = libc::malloc(witness_data.len());
        if (*wtns_data).is_null() {
            prepare_status(status, GW_ERROR_CODE_ERROR, "Failed to allocate memory for wtns_data");
            return 1;
        }
        libc::memcpy(*wtns_data, witness_data.as_ptr() as *const c_void, witness_data.len());
    }

    prepare_status(status, GW_ERROR_CODE_ERROR, "test error");

    0
}

// create a wtns file bytes from witness (array of field elements)
pub fn wtns_from_u256_witness(witness: Vec<U256>) -> Vec<u8> {
    let vec_witness: Vec<FieldElement<32>> = witness
        .iter()
        .map(|a| TryInto::<[u8; 32]>::try_into(a.as_le_slice()).unwrap().into())
        .collect();
    wtns_from_witness(vec_witness)
}

fn wtns_from_witness(witness: Vec<FieldElement<32>>) -> Vec<u8> {
    let mut buf = Vec::new();
    let m: [u8; 32] = Fr::MODULUS.to_bytes_le().as_slice().try_into().unwrap();
    let mut wtns_f = wtns_file::WtnsFile::from_vec(witness, m.into());
    wtns_f.version = 2;
    // We write into the buffer, so we should not have any errors here.
    // Panic in case of out of memory is fine.
    wtns_f.write(&mut buf).unwrap();
    buf
}

pub fn wtns_from_witness2<const FS: usize, T: FieldOps>(
    witness: Vec<FieldElement<FS>>, prime: T) -> Vec<u8> {

    let mut buf = Vec::new();
    let m: [u8; FS] = prime.to_le_bytes().as_slice().try_into().unwrap();
    let mut wtns_f = wtns_file::WtnsFile::from_vec(witness, m.into());
    wtns_f.version = 2;
    // We write into the buffer, so we should not have any errors here.
    // Panic in case of out of memory is fine.
    wtns_f.write(&mut buf).unwrap();
    buf
}

pub fn calc_witness(
    inputs: &str,
    wcd_data: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if wcd_data.starts_with(WITNESSCALC_GRAPH_MAGIC_002) ||
        wcd_data.starts_with(WITNESSCALC_GRAPH_MAGIC_001) {
        calc_witness_graph(inputs, wcd_data)
    } else if wcd_data.starts_with(WITNESSCALC_CVM_MAGIC) {
        calc_witness_vm2_buf(wcd_data, inputs)
    } else {
        Err("Unknown WCD file format".into())
    }
}

fn calc_witness_graph(
    inputs: &str,
    graph_data: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {

    let start = std::time::Instant::now();
    let (nodes, signals, input_info): (Box<dyn NodesInterface>, Vec<usize>, InputInfo) =
        deserialize_witnesscalc_graph_from_bytes(graph_data).unwrap();
    println!("Graph loaded in {:?}", start.elapsed());

    let start = std::time::Instant::now();
    // let mut inputs_buffer = get_inputs_buffer(nodes.get_inputs_size());
    // populate_inputs(&inputs, &input_mapping, &mut inputs_buffer);
    println!("Inputs populated in {:?}", start.elapsed());

    if let Some(nodes) = nodes.as_any().downcast_ref::<Nodes<U254, VecNodes>>() {
        let result = calc_witness_typed(
            nodes, inputs, &signals, &input_info)?;
        let vec_witness: Vec<FieldElement<32>> = result
            .iter()
            .map(|a| TryInto::<[u8; 32]>::try_into(a.as_le_slice()).unwrap().into())
            .collect();
        Ok(wtns_from_witness2(vec_witness, nodes.prime()))
    } else if let Some(nodes) = nodes.as_any().downcast_ref::<Nodes<U64, VecNodes>>() {
        let result = calc_witness_typed(
            nodes, inputs, &signals, &input_info)?;
        let vec_witness: Vec<FieldElement<8>> = result
            .iter()
            .map(|a| TryInto::<[u8; 8]>::try_into(a.as_le_slice()).unwrap().into())
            .collect();
        Ok(wtns_from_witness2(vec_witness, nodes.prime()))
    } else {
        Err(anyhow!("Invalid nodes type").into())
    }
}

fn calc_witness_typed<T: FieldOps, NS: NodesStorage>(
    nodes: &Nodes<T, NS>, inputs: &str, signals: &[usize],
    inputs_info: &InputInfo) -> Result<Vec<T>, Box<dyn std::error::Error>> {

    let inputs = match inputs_info {
        InputInfo::V1(inputs_mapping) => {
            // flatten inputs as flat arrays
            let inputs = deserialize_and_flatten_inputs(
                inputs.as_bytes(), &nodes.ff)?;
            init_inputs_from_inputs_mapping(&inputs, inputs_mapping)?
        },
        InputInfo::V2{input_info, types} => {
            init_inputs_from_v2(inputs, &nodes.ff, input_info, types)?
        }
    };

    let result = evaluate(
        &nodes.ff, &nodes.nodes, &inputs, signals, &nodes.constants);

    Ok(result)
}

fn init_inputs_from_inputs_mapping<T: FieldOps>(
    input_list: &HashMap<String, Vec<T>>,
    inputs_info: &InputSignalsInfo) -> Result<Vec<T>, Box<dyn std::error::Error>> {

    let mut inputs_len: usize = 1;
    for (offset, len) in inputs_info.values() {
        let idx = offset + len;
        if idx > inputs_len {
            inputs_len = idx;
        }
    }
    let mut inputs = vec![T::zero(); inputs_len];
    inputs[0] = T::one();
    let mut inputs_filled = 1;
    for (key, value) in input_list {
        match inputs_info.get(key) {
            None => {
                return Err(anyhow!("Invalid input signal name for the circuit: {}", key).into());
            }
            Some(&(offset, len)) => {
                if len != value.len() {
                    return Err(anyhow!("Invalid input signal {} length: {}", key, len).into());
                }
                for (i, v) in value.iter().enumerate() {
                    inputs[offset + i] = *v;
                    inputs_filled += 1;
                }
            }
        }
    };

    if inputs_filled != inputs_len {
        return Err(anyhow!("Invalid input signal count: {}, expected {}", inputs_filled, inputs_len).into());
    }

    Ok(inputs)
}

fn init_inputs_from_v2<T: FieldOps>(
    inputs_json: &str,
    ff: &Field<T>,
    input_info: &[vm2::InputInfo],
    types: &[vm2::Type],
) -> Result<Vec<T>, Box<dyn std::error::Error>> {
    let inputs_size = input_info.get_total_size(types)?;
    let min_offset = input_info.min_offset().unwrap_or(0);
    let signals_num = min_offset + inputs_size;
    let mut component = Component::new(0, 0, vec![], inputs_size, signals_num);
    let inputs_cursor = Cursor::new(inputs_json.as_bytes());
    init_signals(inputs_cursor, ff, types, input_info, &mut component)?;
    let mut component_signals = Vec::with_capacity(signals_num);
    component.write_all_signals(&mut component_signals);

    let mut inputs = Vec::with_capacity(inputs_size + 1);
    inputs.push(T::one());
    inputs.extend(
        component_signals.iter()
            .skip(min_offset)
            .take(inputs_size)
            .map(|x| x.expect(
                "[assertion] init_signals should not allow None input signals")));

    Ok(inputs)
}

#[derive(Debug)]
pub enum Error {
    InputsUnmarshal(String),
    InputFieldNumberParseError(ParseError)
}

impl From<ParseError> for Error {
    fn from(e: ParseError) -> Self {
        Error::InputFieldNumberParseError(e)
    }
}

fn calc_value_len(v: &serde_json::Value) -> usize {
    match v {
        serde_json::Value::Array(arr) => arr.iter().map(calc_value_len).sum(),
        serde_json::Value::Object(map) => map.values().map(calc_value_len).sum(),
        _ => 1,
    }
}

fn flatten_value(
    key: &str,
    v: &serde_json::Value,
    vals: &mut Vec<U256>) -> Result<(), Error> {

    match v {
        serde_json::Value::String(s) => {
            vals.push(U256::from_str_radix(s.as_str(),10)?);
        }
        serde_json::Value::Number(n) => {
            vals.push(U256::from(
                n.as_u64()
                    .ok_or(Error::InputsUnmarshal(format!(
                        "signal value is not a positive integer: {}",
                        key).to_string()))?));
        }
        serde_json::Value::Array(arr) => {
            for (idx, nested) in arr.iter().enumerate() {
                let nested_key = format!("{}[{}]", key, idx);
                flatten_value(&nested_key, nested, vals)?;
            }
        }
        serde_json::Value::Object(map) => {
            for (nested_key, nested_val) in map.iter() {
                let nested_path = if key.is_empty() {
                    nested_key.to_string()
                } else {
                    format!("{}.{}", key, nested_key)
                };
                flatten_value(&nested_path, nested_val, vals)?;
            }
        }
        _ => {
            return Err(Error::InputsUnmarshal(
                format!(
                    "value for key {} must be an a number as a string, as a number of an array of strings of numbers",
                    key)));
        }
    };

    Ok(())
}

fn flatten_array<T: FieldOps>(
    key: &str,
    v: &serde_json::Value,
    ff: &Field<T>,
    vals: &mut Vec<T>) -> Result<(), Box<dyn std::error::Error>> {

    match v {
        serde_json::Value::String(s) => {
            let i = ff.parse_str(s)?;
            vals.push(i);
        }
        serde_json::Value::Number(n) => {
            if !n.is_u64() {
                return Err(anyhow!("signal value is not a positive integer: {}", key).into());
            }
            let n = n.as_u64().unwrap().to_string();
            let i = ff.parse_str(&n)?;
            vals.push(i);
        }
        serde_json::Value::Array(arr) => {
            for (idx, nested) in arr.iter().enumerate() {
                let nested_key = format!("{}[{}]", key, idx);
                flatten_array(&nested_key, nested, ff, vals)?;
            }
        }
        _ => {
            return Err(anyhow!(
                "value for key {} must be an a number as a string, as a number of an array of strings or numbers",
                key).into());
        }
    };

    Ok(())
}

pub fn deserialize_inputs(inputs_data: &[u8]) -> Result<HashMap<String, Vec<U256>>, Error> {
    let v: serde_json::Value = serde_json::from_slice(inputs_data).unwrap();

    let map = if let serde_json::Value::Object(map) = v {
        map
    } else {
        return Err(Error::InputsUnmarshal("inputs must be an object".to_string()));
    };

    let mut inputs: HashMap<String, Vec<U256>> = HashMap::new();
    for (k, v) in map {
        let vals = match v {
            serde_json::Value::String(_) |
            serde_json::Value::Number(_) |
            serde_json::Value::Array(_) |
            serde_json::Value::Object(_) => {
                let mut buf = Vec::with_capacity(calc_value_len(&v));
                flatten_value(k.as_str(), &v, &mut buf)?;
                buf
            }
            _ => {
                return Err(Error::InputsUnmarshal(format!(
                    "value for key {} must be an a number as a string, as a number of an array of strings of numbers",
                    k.clone())));
            }
        };
        inputs.insert(k.clone(), vals);
    }
    Ok(inputs)
}

pub fn deserialize_and_flatten_inputs<T: FieldOps>(
    inputs_data: &[u8],
    ff: &Field<T>) -> Result<HashMap<String, Vec<T>>, Box<dyn std::error::Error>> {

    let v: serde_json::Value = serde_json::from_slice(inputs_data)?;

    let map = if let serde_json::Value::Object(map) = v {
        map
    } else {
        return Err(anyhow!("inputs must be an object").into());
    };

    let mut inputs: HashMap<String, Vec<T>> = HashMap::new();
    for (k, v) in map {
        let vals = match v {
            serde_json::Value::String(_) |
            serde_json::Value::Number(_) |
            serde_json::Value::Array(_) => {
                let mut buf = Vec::with_capacity(calc_value_len(&v));
                flatten_array(k.as_str(), &v, ff, &mut buf)?;
                buf
            }
            _ => {
                return Err(anyhow!(
                    "value for key {} must be an a number as a string, as a number of an array of strings or numbers",
                    k.clone()).into());
            }
        };
        inputs.insert(k.clone(), vals);
    }
    Ok(inputs)
}

pub fn progress_bar(n: usize) -> ProgressBar {
    let n: u64 = n.try_into().unwrap();
    let pb = ProgressBar::new(n);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})\n{msg}")
            .unwrap()
            .progress_chars("#>-")
    );
    pb
}

pub fn calc_witness_vm2_buf(
    compiled_bytecode: &[u8],
    inputs_json: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {

    let mut reader = std::io::Cursor::new(&compiled_bytecode);
    let inputs_reader = std::io::Cursor::new(
        inputs_json.as_bytes());
    let prime = read_witnesscalc_vm2_header(&mut reader)?;
    if prime == num_bigint::BigUint::from_bytes_le(&bn254_prime.to_le_bytes_vec()) {
        let ff = Field::new(bn254_prime);
        let circuit = deserialize_witnesscalc_vm2_body(&mut reader, ff)?;
        let mut witness_buf: Vec<u8> = Vec::new();
        calculate_witness_vm2(&circuit, inputs_reader, &mut witness_buf)?;
        Ok(witness_buf)
    } else {
        Err("ERROR: Unsupported prime field".into())
    }
}

pub fn calculate_witness_vm2<T: FieldOps>(
    circuit: &Circuit<T>, inputs_json: impl std::io::Read,
    mut w: impl std::io::Write) -> Result<(), Box<dyn std::error::Error>> {

    validate_types(&circuit.types, &circuit.templates)?;

    let mut component_tree = build_component_tree(
        circuit.main_template_id, &circuit.templates)?;

    init_signals(
        inputs_json, &circuit.field, &circuit.types, &circuit.input_infos,
        &mut component_tree)?;

    #[cfg(feature = "debug_vm2")]
    vm2_setup::debug_component_tree(&component_tree, &circuit.templates);

    let start = std::time::Instant::now();
    execute(circuit, &circuit.field, &mut component_tree)
        .map_err(|e| -> Box<dyn std::error::Error> { e })?;
    println!("VM2 executed in {:?}", start.elapsed());

    let witness_signals = witness_signals(&component_tree, &circuit.witness);
    let wtns_data = witness(witness_signals, circuit.field.prime)?;

    w.write_all(&wtns_data)?;
    w.flush()?;

    Ok(())
}

fn witness_signals<T: FieldOps>(
    component_tree: &vm2::Component<T>,
    witness_signals: &[usize]) -> Vec<T> {

    let start = std::time::Instant::now();
    let signals_num = component_tree.total_signals_len() + 1;
    let mut signals = Vec::with_capacity(signals_num);
    signals.push(Some(T::one()));
    component_tree.write_all_signals(&mut signals);

    let mut witness: Vec<T> = Vec::with_capacity(witness_signals.len());
    for idx in witness_signals {
        witness.push(signals[*idx].unwrap_or_else(T::zero));
    }

    println!(
        "Witness signals gathered in {:?}. Total signals: {}, witness signals: {}.",
        start.elapsed(), signals_num, witness.len());

    witness
}
fn witness<T: FieldOps>(
    witness_signals: Vec<T>,
    prime: T) -> Result<Vec<u8>, Box<dyn std::error::Error>> {

    match T::BYTES {
        8 => {
            let vec_witness: Vec<FieldElement<8>> = witness_signals
                .iter()
                .map(|a| {
                    let a: [u8; 8] = a.to_le_bytes().try_into().unwrap();
                    a.into()
                })
                .collect();
            Ok(wtns_from_witness2(vec_witness, prime))
        }
        32 => {
            let vec_witness: Vec<FieldElement<32>> = witness_signals
                .iter()
                .map(|a| {
                    let a: [u8; 32] = a.to_le_bytes().try_into().unwrap();
                    a.into()
                })
                .collect();
            Ok(wtns_from_witness2(vec_witness, prime))
        }
        _ => {
            todo!()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use prost::Message;
    use ruint::aliases::U256;
    use ruint::uint;
    use crate::proto::InputNode;
    use crate::field::{Field, U254, bn254_prime};

    #[test]
    fn test_ok() {
        let data = r#"
    {
        "key1": ["123", "456", 100500],
        "key2": "789",
        "key3": 123123
    }
    "#;
        let inputs = super::deserialize_inputs(data.as_bytes()).unwrap();
        let want: HashMap<String, Vec<U256>> = [
            ("key1".to_string(), vec![uint!(123_U256), uint!(456_U256), uint!(100500_U256)]),
            ("key2".to_string(), vec![uint!(789_U256)]),
            ("key3".to_string(), vec![uint!(123123_U256)]),
        ].iter().cloned().collect();

        // Check that both maps have the same length
        assert_eq!(inputs.len(), want.len(), "HashMaps do not have the same length");

        // Iterate and compare each key-value pair
        for (key, value) in &inputs {
            assert_eq!(want.get(key), Some(value), "Mismatch at key: {}", key);
        }
    }

    #[test]
    fn calc_witness_rejects_out_of_range_main_template() {
        // A well-formed-but-malicious artifact: it decodes cleanly, yet its
        // main_template_id points past the (empty) templates table. calc_witness
        // must return an error instead of panicking inside the interpreter.
        let ff = Field::new(bn254_prime);
        let circuit = crate::vm2::Circuit {
            main_template_id: 0,
            templates: vec![],
            functions: vec![],
            function_registry: HashMap::new(),
            field: ff,
            witness: vec![],
            signals_num: 0,
            input_infos: vec![],
            types: vec![],
        };
        let mut artifact = Vec::new();
        crate::storage::serialize_witnesscalc_vm2(&mut artifact, &circuit).unwrap();

        let err = super::calc_witness("{}", &artifact).unwrap_err();
        assert!(err.to_string().contains("Invalid template ID"),
            "expected the template-id guard to reject it, got: {err}");
    }

    #[test]
    fn calc_witness_rejects_cyclic_bus_type() {
        // The artifact decodes cleanly, but its bus type contains a field of its
        // own type. Sizing a signal of that type would recurse forever, so
        // calc_witness must reject it instead of overflowing the stack.
        let ff = Field::new(bn254_prime);
        let circuit = crate::vm2::Circuit {
            main_template_id: 0,
            templates: vec![crate::vm2::Template {
                name: "Main".to_string(),
                code: vec![],
                signals_num: 1,
                number_of_inputs: 0,
                components: vec![],
                inputs: vec![],
                outputs: vec![],
                ff_variable_names: vec![],
                i64_variable_names: vec![],
            }],
            functions: vec![],
            function_registry: HashMap::new(),
            field: ff,
            witness: vec![],
            signals_num: 1,
            input_infos: vec![],
            types: vec![crate::vm2::Type {
                name: "b".to_string(),
                fields: vec![crate::vm2::TypeField {
                    name: "f".to_string(),
                    kind: crate::vm2::TypeFieldKind::Bus(0),
                    offset: 0,
                    base_type_size: 1,
                    dims: vec![],
                }],
            }],
        };
        let mut artifact = Vec::new();
        crate::storage::serialize_witnesscalc_vm2(&mut artifact, &circuit).unwrap();

        let err = super::calc_witness("{}", &artifact).unwrap_err();
        assert!(err.to_string().contains("cycle"),
            "expected the bus-type cycle guard to reject it, got: {err}");
    }

    #[test]
    fn calc_witness_rejects_dangling_nested_bus_type() {
        let mut circuit = minimal_vm2_circuit(vec![], vec![]);
        circuit.templates[0].outputs = vec![crate::vm2::Signal::Bus(0, vec![])];
        circuit.types = vec![crate::vm2::Type {
            name: "outer".to_string(),
            fields: vec![crate::vm2::TypeField {
                name: "inner".to_string(),
                kind: crate::vm2::TypeFieldKind::Bus(7),
                offset: 0,
                base_type_size: 1,
                dims: vec![],
            }],
        }];

        let err = calc_vm2_err(&circuit);
        assert!(err.contains("Invalid type ID: 7"),
            "expected the nested bus-type guard to reject it, got: {err}");
    }

    #[test]
    fn test_ok2() {
        let i: InputNode = InputNode {
            idx: 1,
        };
        let v = i.encode_to_vec();
        println!("{:?}", v.len());
    }

    #[test]
    fn test_deserialize_and_flatten_inputs() {
        let data = r#"{"v":{"a":1,"b":{"c":2},"d":[3,4]}}"#;
        let ff = Field::new(bn254_prime);
        let err = super::deserialize_and_flatten_inputs::<U254>(data.as_bytes(), &ff).unwrap_err();
        assert!(err.to_string().contains("value for key v must be an a number as a string, as a number of an array of strings or numbers"));

        let data = r#"{"v":"1", "b": [[1, "2"], [3]]}"#;
        let res = super::deserialize_and_flatten_inputs::<U254>(data.as_bytes(), &ff).unwrap();
        let mut want = HashMap::new();
        want.insert(String::from("v"), vec![U254::from(1)]);
        want.insert(String::from("b"), vec![
            U254::from(1),
            U254::from(2),
            U254::from(3)]);
        assert_eq!(res, want);
    }
}
