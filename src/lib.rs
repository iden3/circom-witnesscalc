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
use std::io::{Cursor, Read};
use std::slice::from_raw_parts;
use anyhow::anyhow;
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use ruint::aliases::U256;
use ruint::ParseError;
use crate::graph::{evaluate, Nodes, NodesInterface, NodesStorage, VecNodes};
use wtns_file::FieldElement;
use ark_bn254::Fr;
use ark_ff::{BigInteger, PrimeField};
use indicatif::{ProgressBar, ProgressStyle};
use crate::field::{bn254_prime, Field, FieldOperations, FieldOps, U254, U64};
use crate::storage::proto_deserializer::deserialize_witnesscalc_graph_from_bytes;
use crate::storage::{deserialize_witnesscalc_vm2_body, read_witnesscalc_vm2_header, WITNESSCALC_CVM_MAGIC, WITNESSCALC_GRAPH_MAGIC};
use crate::vm2::{execute, Circuit};
use crate::vm2_setup::{build_component_tree, init_signals};

pub type InputSignalsInfo = HashMap<String, (usize, usize)>;

const WITNESS_CACHE_MAGIC: &[u8] = b"wtns.cache.001";

pub struct CalcWitnessOptions<'a> {
    pub reuse_cache: Option<&'a [u8]>,
    pub generate_cache: bool,
}

impl<'a> Default for CalcWitnessOptions<'a> {
    fn default() -> Self {
        Self {
            reuse_cache: None,
            generate_cache: false,
        }
    }
}

pub struct CalcWitnessResult {
    pub witness: Vec<u8>,
    pub cache: Option<Vec<u8>>,
}

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

pub fn calc_witness_with_cache<'a>(
    inputs: &str,
    wcd_data: &[u8],
    options: CalcWitnessOptions<'a>,
) -> Result<CalcWitnessResult, Box<dyn std::error::Error>> {
    if wcd_data.starts_with(WITNESSCALC_GRAPH_MAGIC) {
        calc_witness_graph(inputs, wcd_data, options)
    } else if wcd_data.starts_with(WITNESSCALC_CVM_MAGIC) {
        if options.reuse_cache.is_some() || options.generate_cache {
            return Err("Witness cache is not supported for VM circuits".into());
        }
        let witness = calc_witness_vm2_buf(wcd_data, inputs)?;
        Ok(CalcWitnessResult { witness, cache: None })
    } else {
        Err("Unknown WCD file format".into())
    }
}

pub fn calc_witness(
    inputs: &str,
    wcd_data: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let result = calc_witness_with_cache(inputs, wcd_data, CalcWitnessOptions::default())?;
    Ok(result.witness)
}

fn calc_witness_graph(
    inputs: &str,
    graph_data: &[u8],
    options: CalcWitnessOptions) -> Result<CalcWitnessResult, Box<dyn std::error::Error>> {

    let start = std::time::Instant::now();
    // let inputs = deserialize_inputs(inputs.as_bytes())?;
    println!("Inputs loaded in {:?}", start.elapsed());

    let start = std::time::Instant::now();
    let (nodes, signals, input_mapping): (Box<dyn NodesInterface>, Vec<usize>, InputSignalsInfo) =
        deserialize_witnesscalc_graph_from_bytes(graph_data).unwrap();
    println!("Graph loaded in {:?}", start.elapsed());

    let start = std::time::Instant::now();
    // let mut inputs_buffer = get_inputs_buffer(nodes.get_inputs_size());
    // populate_inputs(&inputs, &input_mapping, &mut inputs_buffer);
    println!("Inputs populated in {:?}", start.elapsed());

    let reuse_cache = options.reuse_cache;
    let generate_cache = options.generate_cache;

    if let Some(nodes) = nodes.as_any().downcast_ref::<Nodes<U254, VecNodes>>() {
        let result = calc_witness_typed(
            nodes, inputs, &input_mapping, &signals, reuse_cache, generate_cache)?;
        let vec_witness: Vec<FieldElement<32>> = result.witness_values
            .iter()
            .map(|a| TryInto::<[u8; 32]>::try_into(a.as_le_slice()).unwrap().into())
            .collect();
        Ok(CalcWitnessResult {
            witness: wtns_from_witness2(vec_witness, nodes.prime()),
            cache: result.cache_bytes,
        })
    } else if let Some(nodes) = nodes.as_any().downcast_ref::<Nodes<U64, VecNodes>>() {
        let result = calc_witness_typed(
            nodes, inputs, &input_mapping, &signals, reuse_cache, generate_cache)?;
        let vec_witness: Vec<FieldElement<8>> = result.witness_values
            .iter()
            .map(|a| TryInto::<[u8; 8]>::try_into(a.as_le_slice()).unwrap().into())
            .collect();
        Ok(CalcWitnessResult {
            witness: wtns_from_witness2(vec_witness, nodes.prime()),
            cache: result.cache_bytes,
        })
    } else {
        Err(anyhow!("Invalid nodes type").into())
    }
}

fn calc_witness_typed<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    nodes: &Nodes<T, NS>, inputs: &str, input_mapping: &InputSignalsInfo,
    signals: &[usize], reuse_cache: Option<&[u8]>,
    generate_cache: bool) -> Result<TypedWitnessResult<T>, Box<dyn std::error::Error>> {

    let inputs = deserialize_inputs2(
        inputs.as_bytes(), &nodes.ff)?;
    let flattened_inputs = create_inputs(&inputs, input_mapping)?;
    let prime_bytes = T::to_le_bytes(&nodes.prime());

    let (witness_values, node_values) = if let Some(cache_bytes) = reuse_cache {
        let cache = GraphCacheValues::deserialize(
            cache_bytes, &prime_bytes, nodes.len(),
            flattened_inputs.len())?;
        evaluate_with_cache(
            &nodes.ff, &nodes.nodes, &flattened_inputs,
            &cache, signals, &nodes.constants)
    } else {
        evaluate(
            &nodes.ff, &nodes.nodes, &flattened_inputs, signals,
            &nodes.constants)
    };

    let cache_bytes = if generate_cache {
        Some(GraphCacheValues::serialize(
            &flattened_inputs, &node_values, &prime_bytes)?)
    } else {
        None
    };

    Ok(TypedWitnessResult {
        witness_values,
        cache_bytes,
    })
}

struct TypedWitnessResult<T: FieldOps> {
    witness_values: Vec<T>,
    cache_bytes: Option<Vec<u8>>,
}

struct GraphCacheValues<T: FieldOps> {
    inputs: Vec<T>,
    node_values: Vec<T>,
}

impl<T: FieldOps> GraphCacheValues<T> {
    fn serialize(
        inputs: &[T], node_values: &[T], prime_bytes: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut buf = Vec::new();
        buf.extend_from_slice(WITNESS_CACHE_MAGIC);
        buf.write_u32::<LittleEndian>(T::BYTES as u32)?;
        buf.write_u32::<LittleEndian>(prime_bytes.len() as u32)?;
        buf.write_u32::<LittleEndian>(node_values.len() as u32)?;
        buf.write_u32::<LittleEndian>(inputs.len() as u32)?;
        buf.extend_from_slice(prime_bytes);
        for value in node_values {
            let mut bytes = value.to_le_bytes();
            if bytes.len() != T::BYTES {
                bytes.resize(T::BYTES, 0);
            }
            buf.extend_from_slice(&bytes);
        }
        for value in inputs {
            let mut bytes = value.to_le_bytes();
            if bytes.len() != T::BYTES {
                bytes.resize(T::BYTES, 0);
            }
            buf.extend_from_slice(&bytes);
        }
        Ok(buf)
    }

    fn deserialize(
        data: &[u8], expected_prime: &[u8], node_len: usize,
        input_len: usize) -> Result<Self, Box<dyn std::error::Error>> {
        if data.len() < WITNESS_CACHE_MAGIC.len() {
            return Err(anyhow!("Witness cache file is too short").into());
        }
        let mut cursor = Cursor::new(data);
        let mut magic = vec![0u8; WITNESS_CACHE_MAGIC.len()];
        cursor.read_exact(&mut magic)?;
        if magic != WITNESS_CACHE_MAGIC {
            return Err(anyhow!("Invalid witness cache file").into());
        }
        let field_bytes = cursor.read_u32::<LittleEndian>()? as usize;
        if field_bytes != T::BYTES {
            return Err(anyhow!("Witness cache field size mismatch").into());
        }
        let prime_len = cursor.read_u32::<LittleEndian>()? as usize;
        let stored_nodes = cursor.read_u32::<LittleEndian>()? as usize;
        let stored_inputs = cursor.read_u32::<LittleEndian>()? as usize;
        if stored_nodes != node_len {
            return Err(anyhow!(
                "Witness cache node count mismatch (expected {}, got {})",
                node_len, stored_nodes).into());
        }
        if stored_inputs != input_len {
            return Err(anyhow!(
                "Witness cache input count mismatch (expected {}, got {})",
                input_len, stored_inputs).into());
        }
        let mut prime_bytes = vec![0u8; prime_len];
        cursor.read_exact(&mut prime_bytes)?;
        if prime_bytes != expected_prime {
            return Err(anyhow!("Witness cache field prime mismatch").into());
        }
        let mut node_values = Vec::with_capacity(stored_nodes);
        let mut buf = vec![0u8; field_bytes];
        for _ in 0..stored_nodes {
            cursor.read_exact(&mut buf)?;
            let value = T::from_le_bytes(&buf).map_err(|e| -> Box<dyn std::error::Error> { e })?;
            node_values.push(value);
        }
        let mut inputs = Vec::with_capacity(stored_inputs);
        for _ in 0..stored_inputs {
            cursor.read_exact(&mut buf)?;
            let value = T::from_le_bytes(&buf).map_err(|e| -> Box<dyn std::error::Error> { e })?;
            inputs.push(value);
        }
        Ok(Self { inputs, node_values })
    }
}

fn evaluate_with_cache<T: FieldOps, F: FieldOperations<Type = T>, NS: NodesStorage>(
    ff: F, nodes: &NS, inputs: &[T], cache: &GraphCacheValues<T>,
    outputs: &[usize], constants: &[T]) -> (Vec<T>, Vec<T>)
where Vec<T>: FromIterator<<F as FieldOperations>::Type> {
    assert_eq!(cache.node_values.len(), nodes.len());
    assert_eq!(cache.inputs.len(), inputs.len());
    let mut values = Vec::with_capacity(nodes.len());
    let mut dirty = Vec::with_capacity(nodes.len());

    for i in 0..nodes.len() {
        let node = nodes.get(i).unwrap();
        match node {
            graph::Node::Unknown => panic!("Unknown node"),
            graph::Node::Constant(idx) => {
                values.push(constants[idx]);
                dirty.push(false);
            }
            graph::Node::Input(idx) => {
                let new_val = inputs[idx];
                let old_val = cache.inputs[idx];
                let changed = new_val != old_val;
                let value = if changed {
                    new_val
                } else {
                    cache.node_values[i]
                };
                values.push(value);
                dirty.push(changed);
            }
            graph::Node::Op(op, a, b) => {
                let changed = dirty[a] || dirty[b];
                if changed {
                    let value = ff.op_duo(op, values[a], values[b]);
                    values.push(value);
                } else {
                    values.push(cache.node_values[i]);
                }
                dirty.push(changed);
            }
            graph::Node::UnoOp(op, a) => {
                let changed = dirty[a];
                if changed {
                    let value = ff.op_uno(op, values[a]);
                    values.push(value);
                } else {
                    values.push(cache.node_values[i]);
                }
                dirty.push(changed);
            }
            graph::Node::TresOp(op, a, b, c) => {
                let changed = dirty[a] || dirty[b] || dirty[c];
                if changed {
                    let value = ff.op_tres(op, values[a], values[b], values[c]);
                    values.push(value);
                } else {
                    values.push(cache.node_values[i]);
                }
                dirty.push(changed);
            }
        }
    }

    let witness = outputs.iter().map(|&i| values[i]).collect();
    (witness, values)
}

fn create_inputs<T: FieldOps>(
    input_list: &HashMap<String, Vec<T>>,
    inputs_info: &InputSignalsInfo) -> Result<Vec<T>, Box<dyn std::error::Error>> {

    let mut max_idx: usize = 0;
    for (offset, len) in inputs_info.values() {
        let idx = offset + len;
        if idx > max_idx {
            max_idx = idx;
        }
    }
    let mut inputs = vec![T::zero(); max_idx + 1];
    inputs[0] = T::one();
    for (key, value) in input_list {
        let (offset, len) = inputs_info[key];
        if len != value.len() {
            return Err(anyhow!("Invalid input length for {}", key).into());
        }

        for (i, v) in value.iter().enumerate() {
            inputs[offset + i] = *v;
        }
    };
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

fn flatten_array2<T: FieldOps>(
    key: &str,
    vs: &Vec<serde_json::Value>,
    ff: &Field<T>) -> Result<Vec<T>, Box<dyn std::error::Error>> {

    let mut vals: Vec<T> = Vec::with_capacity(calc_len(vs));

    for v in vs {
        match v {
            serde_json::Value::String(s) => {
                let i = ff.parse_str(s)?;
                vals.push(i);
            }
            serde_json::Value::Number(n) => {
                if !n.is_u64() {
                    return Err(anyhow!("signal value is not a positive integer").into());
                }
                let n = n.as_u64().unwrap().to_string();
                let i = ff.parse_str(&n)?;
                vals.push(i);
            }
            serde_json::Value::Array(arr) => {
                vals.extend_from_slice(flatten_array2(key, arr, ff)?.as_slice());
            }
            _ => {
                return Err(anyhow!("inputs must be a string: {}", key).into());
            }
        };

    }
    Ok(vals)
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
        match v {
            serde_json::Value::String(s) => {
                let i = U256::from_str_radix(s.as_str(),10)?;
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
                let vals: Vec<U256> = flatten_array(k.as_str(), &ss)?;
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

pub fn deserialize_inputs2<T: FieldOps>(
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
        match v {
            serde_json::Value::String(s) => {
                let i = ff.parse_str(s.as_str())?;
                inputs.insert(k.clone(), vec![i]);
            }
            serde_json::Value::Number(n) => {
                if !n.is_u64() {
                    return Err(anyhow!("signal value is not a positive integer").into());
                }
                let s = format!("{}", n.as_u64().unwrap());
                let i = ff.parse_str(&s)?;
                inputs.insert(k.clone(), vec![i]);
            }
            serde_json::Value::Array(ss) => {
                let vals: Vec<T> = flatten_array2(k.as_str(), &ss, ff)?;
                inputs.insert(k.clone(), vals);
            }
            _ => {
                return Err(anyhow!(
                    "value for key {} must be an a number as a string, as a number of an array of strings of numbers",
                    k.clone()).into());
            }
        }
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
    let prime = read_witnesscalc_vm2_header(&mut reader).unwrap();
    if prime == num_bigint::BigUint::from_bytes_le(&bn254_prime.to_le_bytes_vec()) {
        let ff = Field::new(bn254_prime);
        let circuit = deserialize_witnesscalc_vm2_body(&mut reader, ff).unwrap();
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

    let mut component_tree = build_component_tree(
        circuit.main_template_id, &circuit.templates);

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
    use crate::flatten_array;
    use crate::proto::InputNode;

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
    fn test_ok2() {
        let i: InputNode = InputNode {
            idx: 1,
        };
        let v = i.encode_to_vec();
        println!("{:?}", v.len());
    }

    #[test]
    fn test_flatten_array() {
        let data = r#"["123", "456", 100500, [1, 2]]"#;
        let v = serde_json::from_str(data).unwrap();
        let res = flatten_array("key1", &v).unwrap();

        let want = vec![uint!(123_U256), uint!(456_U256), uint!(100500_U256), uint!(1_U256), uint!(2_U256)];
        assert_eq!(want, res);
    }

    #[test]
    fn test_calc_len() {
        let data = r#"["123", "456", 100500]"#;
        let v = serde_json::from_str(data).unwrap();
        let l = super::calc_len(&v);
        assert_eq!(l, 3);

        let data = r#"["123", ["456", true], 100500]"#;
        let v = serde_json::from_str(data).unwrap();
        let l = super::calc_len(&v);
        assert_eq!(l, 4);
    }

}
