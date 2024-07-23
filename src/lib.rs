#[allow(non_snake_case,dead_code)]
mod field;
pub mod graph;

use std::collections::HashMap;
use ruint::aliases::U256;
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

pub fn calc_witness(
    inputs: &HashMap<String, Vec<U256>>,
    graph_data: &[u8]) -> Vec<U256> {

    let (nodes, signals, input_mapping): (Vec<Node>, Vec<usize>, HashMap<String, (usize, usize)>) =
        postcard::from_bytes(graph_data).unwrap();

    let mut inputs_buffer = get_inputs_buffer(get_inputs_size(&nodes));
    populate_inputs(inputs, &input_mapping, &mut inputs_buffer);

    graph::evaluate(&nodes, inputs_buffer.as_slice(), &signals)
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

#[cfg(test)]
mod tests {
    #[test]
    fn test_ok() {
        println!("OK");
    }

}