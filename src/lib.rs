#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
// #[allow(dead_code)]
pub mod field;
pub mod graph;
pub mod storage;
pub mod vm;
pub mod vm2;
pub mod ast;

pub mod parser;

use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void, CStr};
use std::slice::from_raw_parts;
use anyhow::anyhow;
use ruint::aliases::U256;
use ruint::ParseError;
use crate::graph::{evaluate, Nodes, NodesInterface, NodesStorage, VecNodes};
use wtns_file::FieldElement;
use ark_bn254::Fr;
use ark_ff::{BigInteger, PrimeField};
use indicatif::{ProgressBar, ProgressStyle};
use crate::field::{Field, FieldOperations, FieldOps, U254, U64};
use crate::storage::proto_deserializer::deserialize_witnesscalc_graph_from_bytes;

pub type InputSignalsInfo = HashMap<String, (usize, usize)>;

pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/circom_witnesscalc.proto.rs"));

    pub mod vm {
        include!(concat!(env!("OUT_DIR"), "/circom_witnesscalc.proto.vm.rs"));
    }
}

#[cfg(feature="c_bindings")]
include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

#[cfg(feature="c_bindings")]
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
#[cfg(feature="c_bindings")]
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
                prepare_status(status, GW_ERROR_CODE_ERROR, format!("Failed to parse inputs: {}", e).as_str());
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
    graph_data: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {

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

    if let Some(nodes) = nodes.as_any().downcast_ref::<Nodes<U254, VecNodes>>() {
        let result = calc_witness_typed(nodes, inputs, &input_mapping, &signals)?;
        let vec_witness: Vec<FieldElement<32>> = result
            .iter()
            .map(|a| TryInto::<[u8; 32]>::try_into(a.as_le_slice()).unwrap().into())
            .collect();
        Ok(wtns_from_witness2(vec_witness, nodes.prime()))
    } else if let Some(nodes) = nodes.as_any().downcast_ref::<Nodes<U64, VecNodes>>() {
        let result = calc_witness_typed(nodes, inputs, &input_mapping, &signals)?;
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
    nodes: &Nodes<T, NS>, inputs: &str, input_mapping: &InputSignalsInfo,
    signals: &[usize]) -> Result<Vec<T>, Box<dyn std::error::Error>> {

    let inputs = deserialize_inputs2(
        inputs.as_bytes(), &nodes.ff)?;
    let inputs = create_inputs(&inputs, input_mapping)?;
    let result = evaluate(
        &nodes.ff, &nodes.nodes, &inputs, signals,
        &nodes.constants);
    Ok(result)
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
