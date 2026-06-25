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
use crate::graph::{evaluate, validate_node_indices, Nodes, NodesInterface, NodesStorage, VecNodes};
use wtns_file::FieldElement;
use ark_bn254::Fr;
use ark_ff::{BigInteger, PrimeField};
use indicatif::{ProgressBar, ProgressStyle};
use crate::field::{bn254_prime, Field, FieldOperations, FieldOps, U254, U64};
use crate::storage::proto_deserializer::{deserialize_witnesscalc_graph_from_bytes, InputInfo};
use crate::storage::{deserialize_witnesscalc_vm2_body, read_witnesscalc_vm2_header, WITNESSCALC_CVM_MAGIC, WITNESSCALC_GRAPH_MAGIC_002, WITNESSCALC_GRAPH_MAGIC_001};
use crate::vm2::{execute, Circuit, Component};
use crate::vm2_setup::{build_component_tree, init_signals};

pub type InputSignalsInfo = HashMap<String, (usize, usize)>;

pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/circom_witnesscalc.proto.rs"));

    pub mod vm {
        include!(concat!(env!("OUT_DIR"), "/circom_witnesscalc.proto.vm.rs"));
    }
}

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
// include!("bindings.rs");

// Graph V2 input metadata is untrusted; cap the temporary component used to
// parse inputs before Component::new performs infallible signal allocation.
const MAX_GRAPH_V2_INPUT_SIGNALS: usize = 1 << 24;

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
        deserialize_witnesscalc_graph_from_bytes(graph_data)?;
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

    validate_node_indices(
        &nodes.nodes, inputs.len(), nodes.constants.len(), signals)?;

    let result = evaluate(
        &nodes.ff, &nodes.nodes, &inputs, signals, &nodes.constants);

    Ok(result)
}

fn init_inputs_from_inputs_mapping<T: FieldOps>(
    input_list: &HashMap<String, Vec<T>>,
    inputs_info: &InputSignalsInfo) -> Result<Vec<T>, Box<dyn std::error::Error>> {

    let mut inputs_len: usize = 1;
    for (offset, len) in inputs_info.values() {
        let idx = checked_input_add(*offset, *len, "offset plus length")?;
        if idx > inputs_len {
            inputs_len = idx;
        }
    }
    let mut inputs = try_reserve_input_vec(inputs_len, "V1 inputs")?;
    inputs.resize(inputs_len, T::zero());
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
    let (inputs_size, min_offset) = checked_v2_input_layout(input_info, types)?;
    checked_component_input_count(inputs_size)?;
    let shifted_input_info = shift_input_offsets(input_info, min_offset)?;
    let mut component = Component::new(0, 0, vec![], inputs_size, inputs_size);
    let inputs_cursor = Cursor::new(inputs_json.as_bytes());
    init_signals(inputs_cursor, ff, types, &shifted_input_info, &mut component)?;
    let mut component_signals = try_reserve_input_vec(inputs_size, "V2 component output")?;
    component.write_all_signals(&mut component_signals);

    let inputs_capacity = checked_input_add(inputs_size, 1, "inputs size plus one")?;
    let mut inputs = try_reserve_input_vec(inputs_capacity, "V2 inputs")?;
    inputs.push(T::one());
    inputs.extend(
        component_signals.iter()
            .take(inputs_size)
            .map(|x| x.expect(
                "[assertion] init_signals should not allow None input signals")));

    Ok(inputs)
}

fn checked_v2_input_layout(
    input_info: &[vm2::InputInfo],
    types: &[vm2::Type],
) -> Result<(usize, usize), Box<dyn std::error::Error>> {
    let mut inputs_size = 0usize;
    let mut ranges = try_reserve_input_vec(input_info.len(), "V2 input ranges")?;
    for info in input_info {
        let base_type_size = match &info.type_id {
            None => 1,
            Some(type_id) => {
                let type_idx = types.iter()
                    .position(|ty| &ty.name == type_id)
                    .ok_or_else(|| invalid_input_metadata("Unknown input type"))?;
                checked_type_size_by_index(type_idx, types, &mut Vec::new())?
            }
        };
        let array_count = checked_product(&info.lengths, "input dimensions")?;
        let input_size = checked_input_mul(array_count, base_type_size, "input size")?;
        if input_size == 0 {
            return Err(invalid_input_metadata("Input metadata size must be nonzero"));
        }
        inputs_size = checked_input_add(inputs_size, input_size, "total input size")?;
        let end = checked_input_add(info.offset, input_size, "input offset plus size")?;
        ranges.push((info.offset, end));
    }

    ranges.sort_by_key(|(offset, _)| *offset);
    let min_offset = ranges.first().map(|(offset, _)| *offset).unwrap_or(0);
    let mut expected_offset = min_offset;
    for (offset, end) in ranges {
        if offset != expected_offset {
            return Err(invalid_input_metadata(
                "Input metadata offsets must be contiguous and non-overlapping",
            ));
        }
        expected_offset = end;
    }
    let signals_num = checked_input_add(min_offset, inputs_size, "signal count")?;
    if expected_offset != signals_num {
        return Err(invalid_input_metadata("Input metadata span is inconsistent"));
    }
    Ok((inputs_size, min_offset))
}

fn shift_input_offsets(
    input_info: &[vm2::InputInfo],
    min_offset: usize,
) -> Result<Vec<vm2::InputInfo>, Box<dyn std::error::Error>> {
    let mut shifted = try_reserve_input_vec(input_info.len(), "V2 shifted input info")?;
    for info in input_info {
        let mut shifted_info = info.clone();
        shifted_info.offset = info.offset.checked_sub(min_offset).ok_or_else(|| {
            invalid_input_metadata("Input metadata offset is below minimum offset")
        })?;
        shifted.push(shifted_info);
    }
    Ok(shifted)
}

fn checked_component_input_count(
    signals_num: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    if signals_num > MAX_GRAPH_V2_INPUT_SIGNALS {
        return Err(invalid_input_metadata(
            "Input metadata V2 input count is too large",
        ));
    }
    Ok(())
}

fn checked_type_size_by_index(
    type_idx: usize,
    types: &[vm2::Type],
    visiting: &mut Vec<usize>,
) -> Result<usize, Box<dyn std::error::Error>> {
    if type_idx >= types.len() {
        return Err(invalid_input_metadata("Bus type index is out of range"));
    }
    if visiting.contains(&type_idx) {
        return Err(invalid_input_metadata("Bus type metadata contains a cycle"));
    }

    visiting.push(type_idx);
    let ty = &types[type_idx];
    let mut total_size = 0usize;
    for field in &ty.fields {
        let base_type_size = checked_field_base_size(field, types, visiting)?;
        if field.base_type_size != base_type_size {
            return Err(invalid_input_metadata(
                "Input metadata base_type_size is inconsistent with field type",
            ));
        }

        let field_size = if field.dims.is_empty() {
            base_type_size
        } else {
            let dim_product = checked_product(&field.dims, "type field dimensions")?;
            checked_input_mul(base_type_size, dim_product, "type field size")?
        };
        total_size = checked_input_add(total_size, field_size, "type size")?;
    }
    visiting.pop();
    Ok(total_size)
}

fn checked_field_base_size(
    field: &vm2::TypeField,
    types: &[vm2::Type],
    visiting: &mut Vec<usize>,
) -> Result<usize, Box<dyn std::error::Error>> {
    match &field.kind {
        vm2::TypeFieldKind::Ff => Ok(1),
        vm2::TypeFieldKind::Bus(bus_idx) => {
            checked_type_size_by_index(*bus_idx, types, visiting)
        }
    }
}

fn checked_product(
    values: &[usize],
    context: &str,
) -> Result<usize, Box<dyn std::error::Error>> {
    values.iter().try_fold(1usize, |acc, value| {
        checked_input_mul(acc, *value, context)
    })
}

fn checked_input_add(
    lhs: usize,
    rhs: usize,
    context: &str,
) -> Result<usize, Box<dyn std::error::Error>> {
    lhs.checked_add(rhs).ok_or_else(|| {
        invalid_input_metadata(format!("Input metadata {} overflows usize", context))
    })
}

fn checked_input_mul(
    lhs: usize,
    rhs: usize,
    context: &str,
) -> Result<usize, Box<dyn std::error::Error>> {
    lhs.checked_mul(rhs).ok_or_else(|| {
        invalid_input_metadata(format!("Input metadata {} overflows usize", context))
    })
}

fn try_reserve_input_vec<T>(
    len: usize,
    context: &str,
) -> Result<Vec<T>, Box<dyn std::error::Error>> {
    let mut vec = Vec::new();
    vec.try_reserve(len).map_err(|_| {
        invalid_input_metadata(format!(
            "Input metadata {} exceeds available memory",
            context))
    })?;
    Ok(vec)
}

fn invalid_input_metadata(message: impl Into<String>) -> Box<dyn std::error::Error> {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.into()).into()
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
    use std::panic;
    use prost::Message;
    use ruint::aliases::U256;
    use ruint::uint;
    use crate::proto::InputNode;
    use crate::field::{Field, U254, bn254_prime};
    use crate::storage::WITNESSCALC_GRAPH_MAGIC_002;

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

    #[test]
    fn test_calc_witness_returns_error_for_truncated_graph_artifact() {
        let bytes = WITNESSCALC_GRAPH_MAGIC_002.to_vec();
        let result = panic::catch_unwind(|| super::calc_witness("{}", &bytes));

        assert!(result.is_ok());
        assert!(result.unwrap().is_err());
    }

    #[test]
    fn test_calc_witness_rejects_out_of_range_node_index() {
        use crate::graph::{Node, Nodes, NodesInterface, Operation, VecNodes};
        use crate::storage::serialize_witnesscalc_graph;

        // A graph that decodes cleanly but whose second node references a
        // non-existent operand must produce an error, not a panic.
        let mut nodes = Nodes::new(bn254_prime, "bn128", VecNodes::new());
        nodes.push_noopt(Node::Input(0));
        nodes.push_noopt(Node::Op(Operation::Mul, 0, 5));

        let mut bytes = Vec::new();
        serialize_witnesscalc_graph(&mut bytes, &nodes, &[1], &[], &[]).unwrap();

        let result = panic::catch_unwind(|| super::calc_witness("{}", &bytes));
        assert!(result.is_ok(), "calc_witness must not panic");
        assert!(result.unwrap().is_err());
    }

    fn graph_with_v2_input_metadata(
        input_info: &[crate::vm2::InputInfo],
        types: &[crate::vm2::Type],
    ) -> Vec<u8> {
        use crate::graph::{Node, Nodes, NodesInterface, VecNodes};
        use crate::storage::serialize_witnesscalc_graph;

        let mut nodes = Nodes::new(bn254_prime, "bn128", VecNodes::new());
        nodes.push_noopt(Node::Input(1));

        let mut bytes = Vec::new();
        serialize_witnesscalc_graph(&mut bytes, &nodes, &[0], input_info, types).unwrap();
        bytes
    }

    fn calc_witness_err(inputs_json: &str, wcd_data: &[u8]) -> String {
        let result = panic::catch_unwind(|| super::calc_witness(inputs_json, wcd_data));
        assert!(result.is_ok(), "calc_witness must not panic");
        result.unwrap().unwrap_err().to_string()
    }

    #[test]
    fn test_calc_witness_handles_graph_mod_by_zero() {
        use crate::graph::{Node, Nodes, NodesInterface, Operation, VecNodes};
        use crate::storage::serialize_witnesscalc_graph;

        let mut nodes = Nodes::new(bn254_prime, "bn128", VecNodes::new());
        let zero = nodes.const_node_idx_from_value(U254::from(0));
        nodes.push_noopt(Node::Input(0));
        nodes.push_noopt(Node::Op(Operation::Mod, 1, zero));

        let mut bytes = Vec::new();
        serialize_witnesscalc_graph(&mut bytes, &nodes, &[2], &[], &[]).unwrap();

        let result = panic::catch_unwind(|| super::calc_witness("{}", &bytes));
        assert!(result.is_ok(), "calc_witness must not panic");
        assert!(result.unwrap().is_ok());
    }

    #[test]
    fn test_calc_witness_rejects_graph_composite_prime() {
        use crate::graph::{Node, Nodes, NodesInterface, Operation, VecNodes};
        use crate::storage::serialize_witnesscalc_graph;

        let composite_prime = bn254_prime - U254::from(1);
        let mut nodes = Nodes::new(composite_prime, "composite", VecNodes::new());
        let four = nodes.const_node_idx_from_value(U254::from(4));
        let two = nodes.const_node_idx_from_value(U254::from(2));
        nodes.push_noopt(Node::Op(Operation::Div, four, two));

        let mut bytes = Vec::new();
        serialize_witnesscalc_graph(&mut bytes, &nodes, &[2], &[], &[]).unwrap();

        let err = calc_witness_err("{}", &bytes);
        assert!(err.contains("Unsupported graph prime"));
    }

    #[test]
    fn test_graph_v1_input_metadata_overflow_returns_error() {
        let input_list: HashMap<String, Vec<U254>> = HashMap::new();
        let mut inputs_info = super::InputSignalsInfo::new();
        inputs_info.insert("a".to_string(), (usize::MAX, 1));

        let err = super::init_inputs_from_inputs_mapping(&input_list, &inputs_info)
            .unwrap_err();
        assert!(err.to_string().contains("overflows usize"));
    }

    #[test]
    fn test_graph_v2_input_metadata_overflow_returns_error() {
        let ff = Field::new(bn254_prime);
        let input_info = vec![crate::vm2::InputInfo {
            name: "a".to_string(),
            offset: 0,
            lengths: vec![usize::MAX, 2],
            type_id: None,
        }];

        let err = super::init_inputs_from_v2("{}", &ff, &input_info, &[])
            .unwrap_err();
        assert!(err.to_string().contains("overflows usize"));
    }

    #[test]
    fn test_graph_v2_input_metadata_rejects_invalid_bus_index() {
        let ff = Field::new(bn254_prime);
        let input_info = vec![crate::vm2::InputInfo {
            name: "a".to_string(),
            offset: 0,
            lengths: vec![],
            type_id: Some("bus".to_string()),
        }];
        let types = vec![crate::vm2::Type {
            name: "bus".to_string(),
            fields: vec![crate::vm2::TypeField {
                name: "x".to_string(),
                kind: crate::vm2::TypeFieldKind::Bus(7),
                offset: 0,
                base_type_size: 1,
                dims: vec![],
            }],
        }];

        let err = super::init_inputs_from_v2("{}", &ff, &input_info, &types)
            .unwrap_err();
        assert!(err.to_string().contains("Bus type index is out of range"));
    }

    #[test]
    fn test_calc_witness_rejects_nested_invalid_bus_index() {
        let input_info = vec![crate::vm2::InputInfo {
            name: "a".to_string(),
            offset: 0,
            lengths: vec![],
            type_id: Some("outer".to_string()),
        }];
        let types = vec![
            crate::vm2::Type {
                name: "outer".to_string(),
                fields: vec![crate::vm2::TypeField {
                    name: "inner".to_string(),
                    kind: crate::vm2::TypeFieldKind::Bus(1),
                    offset: 0,
                    base_type_size: 1,
                    dims: vec![],
                }],
            },
            crate::vm2::Type {
                name: "inner".to_string(),
                fields: vec![crate::vm2::TypeField {
                    name: "bad".to_string(),
                    kind: crate::vm2::TypeFieldKind::Bus(99),
                    offset: 0,
                    base_type_size: 1,
                    dims: vec![],
                }],
            },
        ];
        let bytes = graph_with_v2_input_metadata(&input_info, &types);

        let err = calc_witness_err("{}", &bytes);
        assert!(err.contains("Bus type index is out of range"));
    }

    #[test]
    fn test_calc_witness_rejects_type_base_size_desync() {
        let input_info = vec![crate::vm2::InputInfo {
            name: "a".to_string(),
            offset: 0,
            lengths: vec![],
            type_id: Some("bus".to_string()),
        }];
        let types = vec![crate::vm2::Type {
            name: "bus".to_string(),
            fields: vec![crate::vm2::TypeField {
                name: "x".to_string(),
                kind: crate::vm2::TypeFieldKind::Ff,
                offset: 0,
                base_type_size: 2,
                dims: vec![],
            }],
        }];
        let bytes = graph_with_v2_input_metadata(&input_info, &types);

        let err = calc_witness_err("{}", &bytes);
        assert!(err.contains("base_type_size"));
    }

    #[test]
    fn test_calc_witness_rejects_out_of_range_root_array_input() {
        let input_info = vec![crate::vm2::InputInfo {
            name: "a".to_string(),
            offset: 0,
            lengths: vec![1],
            type_id: None,
        }];
        let bytes = graph_with_v2_input_metadata(&input_info, &[]);

        let err = calc_witness_err(r#"{"[999999]": "3"}"#, &bytes);
        assert!(err.contains("outside input signal range"));
    }

    #[test]
    fn test_graph_v2_input_metadata_rejects_sparse_offsets() {
        let ff = Field::new(bn254_prime);
        let input_info = vec![
            crate::vm2::InputInfo {
                name: "a".to_string(),
                offset: 0,
                lengths: vec![1],
                type_id: None,
            },
            crate::vm2::InputInfo {
                name: "b".to_string(),
                offset: 4,
                lengths: vec![1],
                type_id: None,
            },
        ];

        let err = super::init_inputs_from_v2("{}", &ff, &input_info, &[])
            .unwrap_err();
        assert!(err.to_string().contains("contiguous"));
    }

    #[test]
    fn test_graph_v2_input_metadata_rejects_zero_sized_input() {
        let ff = Field::new(bn254_prime);
        let input_info = vec![crate::vm2::InputInfo {
            name: "a".to_string(),
            offset: 0,
            lengths: vec![0],
            type_id: None,
        }];

        let err = super::init_inputs_from_v2("{}", &ff, &input_info, &[])
            .unwrap_err();
        assert!(err.to_string().contains("must be nonzero"));
    }

    #[test]
    fn test_graph_v2_input_metadata_accepts_nonzero_contiguous_offsets() {
        let ff = Field::new(bn254_prime);
        let input_info = vec![crate::vm2::InputInfo {
            name: "a".to_string(),
            offset: 7,
            lengths: vec![1],
            type_id: None,
        }];

        let inputs = super::init_inputs_from_v2("{\"a\":[\"9\"]}", &ff, &input_info, &[])
            .unwrap();
        assert_eq!(inputs, vec![U254::from(1), U254::from(9)]);
    }

    #[test]
    fn test_graph_v2_input_metadata_rejects_oversized_component() {
        let ff = Field::new(bn254_prime);
        let input_info = vec![crate::vm2::InputInfo {
            name: "a".to_string(),
            offset: 0,
            lengths: vec![super::MAX_GRAPH_V2_INPUT_SIGNALS + 1],
            type_id: None,
        }];

        let err = super::init_inputs_from_v2("{}", &ff, &input_info, &[])
            .unwrap_err();
        assert!(err.to_string().contains("input count is too large"));
    }
}
