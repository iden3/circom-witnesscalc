#[allow(non_snake_case,dead_code)]
mod field;
pub mod graph;

use std::collections::HashMap;
use ruint::aliases::U256;
use ruint::ParseError;
use crate::graph::Node;
use wtns_file::FieldElement;
use crate::field::M;

// create a wtns file bytes from witness (array of field elements)
pub fn wtns_from_witness(witness: Vec<U256>) -> Vec<u8> {
    let vec_witness: Vec<FieldElement<32>> = witness.iter().map(|a| u256_to_field_element(a)).collect();
    let mut buf = Vec::new();
    let mut wtns_f = wtns_file::WtnsFile::from_vec(vec_witness, u256_to_field_element(&M));
    wtns_f.version = 2;
    // We write into the buffer, so we should not have any errors here.
    // Panic in case of out of memory is fine.
    wtns_f.write(&mut buf).unwrap();
    buf
}

pub fn calc_witness(inputs: &[u8], graph_data: &[u8]) -> Result<Vec<U256>, Error> {

    let inputs = deserialize_inputs(inputs)?;

    let (nodes, signals, input_mapping): (Vec<Node>, Vec<usize>, HashMap<String, (usize, usize)>) =
        postcard::from_bytes(graph_data).unwrap();

    let mut inputs_buffer = get_inputs_buffer(get_inputs_size(&nodes));
    populate_inputs(&inputs, &input_mapping, &mut inputs_buffer);

    Ok(graph::evaluate(&nodes, inputs_buffer.as_slice(), &signals))
}

fn get_inputs_size(nodes: &Vec<Node>) -> usize {
    let mut start = false;
    let mut max_index = 0usize;
    for &node in nodes.iter() {
        if let Node::Input(i) = node {
            if i > max_index {
                max_index = i;
            }
            start = true
        } else if start {
            break;
        }
    }
    max_index + 1
}

fn populate_inputs(
    input_list: &HashMap<String, Vec<U256>>,
    inputs_info: &HashMap<String, (usize, usize)>,
    input_buffer: &mut Vec<U256>) {
    for (key, value) in input_list {
        let (offset, len) = inputs_info[key];
        if len != value.len() {
            panic!("Invalid input length for {}", key);
        }
        println!("input {}, offset {}, len {}", key, offset, len);

        for (i, v) in value.iter().enumerate() {
            input_buffer[offset + i] = v.clone();
        }
    }
}

fn u256_to_field_element(a: &U256) -> FieldElement<32> {
    let x: [u8; 32] = a.as_le_slice().try_into().unwrap();
    x.into()
}


/// Allocates inputs vec with position 0 set to 1
fn get_inputs_buffer(size: usize) -> Vec<U256> {
    let mut inputs = vec![U256::ZERO; size];
    inputs[0] = U256::from(1);
    inputs
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

fn deserialize_inputs(inputs_data: &[u8]) -> Result<HashMap<String, Vec<U256>>, Error> {
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
                let mut vals: Vec<U256> = Vec::with_capacity(ss.len());
                for v in &ss {
                    let i = match v {
                        serde_json::Value::String(s) => {
                            U256::from_str_radix(s.as_str(),10)?
                        }
                        serde_json::Value::Number(n) => {
                            if !n.is_u64() {
                                return Err(Error::InputsUnmarshal("signal value is not a positive integer".to_string()));
                            }
                            U256::from(n.as_u64().unwrap())
                        }
                        _ => {
                            return Err(Error::InputsUnmarshal("inputs must be a string".to_string()));
                        }
                    };
                    vals.push(i);
                }
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use ruint::aliases::U256;
    use ruint::{uint};

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

}