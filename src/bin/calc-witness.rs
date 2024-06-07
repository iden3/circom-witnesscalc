use std::collections::HashMap;
use std::fs::File;
use ruint::aliases::U256;
use ruint::uint;
use witness::{get_inputs_buffer, graph};
use witness::graph::Node;
use wtns_file::FieldElement;

// TODO: use this constant from library
pub const M: U256 =
    uint!(21888242871839275222246405745257275088548364400416034343698204186575808495617_U256);

pub fn get_inputs_size(nodes: &Vec<Node>) -> usize {
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

pub fn populate_inputs(
    input_list: &HashMap<String, Vec<U256>>,
    inputs_info: &HashMap<String, (usize, usize)>,
    input_buffer: &mut Vec<U256>,
) {
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

fn conv(a: &U256) -> FieldElement<32> {
    let x: [u8; 32] = a.as_le_slice().try_into().unwrap();
    x.into()
}


fn main() {
    let inputs_file = "/Users/alek/src/simple-circuit/circuit3_inputs.json";
    let graph_file = "/Users/alek/src/witness/graph_v2.bin";
    let witness_file = "/Users/alek/src/simple-circuit/output.wtns";

    let inputs_data = std::fs::read(inputs_file).expect("Failed to read input file");
    let inputs: HashMap<String, Vec<U256>> = serde_json::from_slice(inputs_data.as_slice()).unwrap();

    let graph_data = std::fs::read(graph_file).expect("Failed to read graph file");

    let (nodes, signals, input_mapping): (Vec<Node>, Vec<usize>, HashMap<String, (usize, usize)>) =
        postcard::from_bytes(graph_data.as_slice()).unwrap();

    let inputs_size = get_inputs_size(&nodes);
    println!("input size {}", inputs_size);
    for &n in nodes.iter() {
        match n {
            Node::Input(s) => { println! {"input {}", s} },
            _ => {}
        }
    }
    let mut inputs_buffer = get_inputs_buffer(get_inputs_size(&nodes));
    populate_inputs(&inputs, &input_mapping, &mut inputs_buffer);


    println!("inputs: {:?}", inputs);

    let witness = graph::evaluate(&nodes, inputs_buffer.as_slice(), &signals);
    // println!("witness: {:?}", witness);

    let vec_witness: Vec<FieldElement<32>> = witness.iter().map(|a| conv(a)).collect();
    let mut wtns_f = wtns_file::WtnsFile::from_vec(vec_witness, conv(&M));
    wtns_f.version = 2;
    {
        let f = File::create(witness_file).unwrap();
        wtns_f.write(f).unwrap();
    }

}