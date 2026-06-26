use std::io::{Cursor, Error, ErrorKind};
use crate::field::{FieldOperations, FieldOps, U254, U64};
use crate::graph::{Node, Nodes, NodesInterface, NodesStorage, Operation, TresOperation, UnoOperation, VecNodes};
use crate::InputSignalsInfo;
use crate::storage::{deserialize_input_signal_info, read_message, WriteBackReader, WITNESSCALC_GRAPH_MAGIC_002, WITNESSCALC_GRAPH_MAGIC_001};
use crate::vm2::Type;

#[cfg_attr(test, derive(Debug, PartialEq))]
pub enum InputInfo {
    V1(InputSignalsInfo),
    V2 {
        input_info: Vec<crate::vm2::InputInfo>,
        types: Vec<Type>,
    },
}

// deserialize_witnesscalc_graph_from_bytes is almost the same as
// deserialize_witnesscalc_graph but with custom implemented protobuf parser
// specifically optimized to unpack the list of Nodes.
pub fn deserialize_witnesscalc_graph_from_bytes(
    bytes: &[u8]
) -> std::io::Result<(Box<dyn NodesInterface>, Vec<usize>, InputInfo)> {

    let mut idx: usize = if bytes.starts_with(WITNESSCALC_GRAPH_MAGIC_002) {
        WITNESSCALC_GRAPH_MAGIC_002.len()
    } else if bytes.starts_with(WITNESSCALC_GRAPH_MAGIC_001) {
        WITNESSCALC_GRAPH_MAGIC_001.len()
    } else {
        return Err(Error::other("Invalid magic"));
    };

    let nodes_num = u64::from_le_bytes(bytes[idx..idx+8].try_into().unwrap());
    idx += 8;

    let vm_ptr = u64::from_le_bytes(bytes[bytes.len() - 8..bytes.len()]
        .try_into().unwrap());
    let r = Cursor::new(&bytes[vm_ptr as usize..]);
    let mut br = WriteBackReader::new(r);
    let md: crate::proto::GraphMetadata = read_message(&mut br)?;

    let (prime, curve_name) = match &md.prime {
        None => (
            U254::from_str(
                "21888242871839275222246405745257275088548364400416034343698204186575808495617")
                .unwrap(),
            "bn128"
        ),
        Some(p) => (
            <U254 as FieldOps>::from_le_bytes(p.value_le.as_slice())
                .unwrap(),
            md.prime_str.as_str()
        ),
    };

    let outer_nodes: Box<dyn NodesInterface> = match prime.bit_len() {
        64 => {
            let prime = U64::from_le_bytes(
                &<U254 as FieldOps>::to_le_bytes(&prime))
                .unwrap();
            let node_storage = VecNodes::new();
            let mut nodes = Nodes::new(
                prime, curve_name, node_storage);
            for _ in 0..nodes_num {
                let (msg_len, int_len) = decode_varint_u32(&bytes[idx..])?;
                idx += int_len;
                decode_node(&bytes[idx..idx+msg_len as usize], &mut nodes)?;
                idx += msg_len as usize;
            }
            Box::new(nodes)
        }
        254 => {
            let node_storage = VecNodes::new();
            let mut nodes = Nodes::new(
                prime, curve_name, node_storage);
            for _ in 0..nodes_num {
                let (msg_len, int_len) = decode_varint_u32(&bytes[idx..])?;
                idx += int_len;
                decode_node(&bytes[idx..idx+msg_len as usize], &mut nodes)?;
                idx += msg_len as usize;
            }
            Box::new(nodes)
        }
        _ => {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!("unknown prime {}", md.prime_str)));
        }
    };

    let witness_signals = md.witness_signals
        .iter()
        .map(|x| *x as usize)
        .collect::<Vec<usize>>();

    let inputs_info = if bytes.starts_with(WITNESSCALC_GRAPH_MAGIC_001) {
        InputInfo::V1(md.inputs.iter()
            .map(|(k, v)| {
                (k.clone(), (v.offset as usize, v.len as usize))
            })
            .collect::<InputSignalsInfo>())
    } else if bytes.starts_with(WITNESSCALC_GRAPH_MAGIC_002) {
        let (input_info, types) = deserialize_input_signal_info(&md.input_signal_info)?;
        InputInfo::V2 { input_info, types }
    } else {
        unreachable!("magic bytes already validated at start of function")
    };

    Ok((outer_nodes, witness_signals, inputs_info))
}

#[repr(u8)]
#[derive(Debug)]
enum WireType {
    Varint = 0,
    I64 = 1,
    Len = 2,
    SGroup = 3,
    EGroup = 4,
    I32 = 5,
}

impl TryFrom<u8> for WireType {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(WireType::Varint),
            1 => Ok(WireType::I64),
            2 => Ok(WireType::Len),
            3 => Ok(WireType::SGroup),
            4 => Ok(WireType::EGroup),
            5 => Ok(WireType::I32),
            _ => Err(()),
        }
    }
}

/// Decodes a protobuf Node message into a Node enum
pub fn decode_node<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    bytes: &[u8], nodes: &mut Nodes<T, NS>) -> Result<(), Error> {

    if bytes.is_empty() {
        return Err(Error::new(
            ErrorKind::UnexpectedEof,
            "Empty input buffer",
        ));
    }

    let (field_number, wire_type, tag_size) = read_tag(bytes)?;

    if !matches!(wire_type, WireType::Len) {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!(
                "Expected length-delimited field: field_number={}, wire_type={:?}",
                field_number, wire_type),
        ));
    }
    let bytes = &bytes[tag_size..];

    let (length, varint_size) = decode_varint_u32(bytes)?;
    let bytes = &bytes[varint_size..];
    if bytes.len() != length as usize {
        return Err(Error::new(
            ErrorKind::UnexpectedEof,
            "Incorrect ConstantNode field size",
        ));
    }

    match field_number {
        1 => decode_input_node(bytes, nodes),
        2 => decode_constant_node(bytes, nodes),
        3 => decode_uno_op_node(bytes, nodes),
        4 => decode_duo_op_node(bytes, nodes),
        5 => decode_tres_op_node(bytes, nodes),
        _ => {
            panic!("found unknown node")
        }
    }
}

fn decode_input_node<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    bytes: &[u8], nodes: &mut Nodes<T, NS>) -> Result<(), Error> {

    if bytes.is_empty() {
        nodes.push_noopt(Node::Input(0));
        return Ok(());
    }

    let (field_number, wire_type, tag_size) = read_tag(bytes)?;

    if field_number != 1 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "Expected field number 1 for InputNode",
        ));
    }

    if !matches!(wire_type, WireType::Varint) {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "Expected length-delimited field for InputNode",
        ));
    }

    let bytes = &bytes[tag_size..];

    let (value, varint_size) = decode_varint_u32(bytes)?;
    if varint_size != bytes.len() {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "Incorrect InputNode field size",
        ));
    }
    nodes.push_noopt(Node::Input(value as usize));
    Ok(())
}

fn decode_big_le_bytes(bytes: &[u8]) -> Result<Vec<u8>, Error> {
    if bytes.is_empty() {
        return Err(Error::new(
            ErrorKind::UnexpectedEof,
            "Empty input buffer",
        ));
    }

    let (field_number, wire_type, tag_size) = read_tag(bytes)?;
    if field_number != 1 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "Expected field number 1 for BigUInt",
        ));
    }
    if !matches!(wire_type, WireType::Len) {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "Expected length-delimited field for BigUInt",
        ));
    }

    let bytes = &bytes[tag_size..];

    let (length, varint_size) = decode_varint_u32(bytes)?;
    if bytes.len() - varint_size != length as usize {
        return Err(Error::new(
            ErrorKind::UnexpectedEof,
            "Incorrect BigUInt field size",
        ));
    }
    let bytes = &bytes[varint_size..];
    Ok(bytes.to_vec())
}

/// Decodes a UnoOpNode message into an Operation and two indices
fn decode_uno_op_node<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    bytes: &[u8], nodes: &mut Nodes<T, NS>) -> Result<(), Error> {

    if bytes.is_empty() {
        nodes.push_noopt(Node::UnoOp(UnoOperation::Neg, 0));
        return Ok(());
    }

    let mut offset = 0;

    let mut op = UnoOperation::Neg;
    let mut a_idx: usize = 0;

    // Process all fields in the message
    while offset < bytes.len() {
        let (field_number, wire_type, tag_size) = read_tag(&bytes[offset..])?;
        offset += tag_size;

        if !matches!(wire_type, WireType::Varint) {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "Expected varint as DuoOpNode field",
            ));
        }

        let (value, varint_size) = decode_varint_u32(&bytes[offset..])?;
        offset += varint_size;

        match field_number {
            1 => {
                op = match value {
                    0 => UnoOperation::Neg,
                    1 => UnoOperation::Id,
                    2 => UnoOperation::Lnot,
                    3 => UnoOperation::Bnot,
                    4 => UnoOperation::Sqrt,
                    _ => return Err(Error::new(
                        ErrorKind::InvalidData,
                        format!("Unknown UnoOp operation value: {}", value),
                    )),
                };
            },
            2 => {
                a_idx = value as usize;
            },
            _ => {
                return Err(Error::new(ErrorKind::InvalidData, "Unknown UnoOpNode tag"));
            }
        }
    }

    nodes.push_noopt(Node::UnoOp(op, a_idx));
    Ok(())
}

/// Decodes a DuoOpNode message into an Operation and two indices
fn decode_duo_op_node<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    bytes: &[u8], nodes: &mut Nodes<T, NS>) -> Result<(), Error> {

    if bytes.is_empty() {
        nodes.push_noopt(Node::Op(Operation::Mul, 0, 0));
        return Ok(());
    }

    let mut offset = 0;

    let mut op = Operation::Mul;
    let mut a_idx: usize = 0;
    let mut b_idx: usize = 0;

    // Process all fields in the message
    while offset < bytes.len() {
        let (field_number, wire_type, tag_size) = read_tag(&bytes[offset..])?;
        offset += tag_size;

        if !matches!(wire_type, WireType::Varint) {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "Expected varint as DuoOpNode field",
            ));
        }

        let (value, varint_size) = decode_varint_u32(&bytes[offset..])?;
        offset += varint_size;

        match field_number {
            1 => {
                op = match value {
                    0 => Operation::Mul,
                    1 => Operation::Div,
                    2 => Operation::Add,
                    3 => Operation::Sub,
                    4 => Operation::Pow,
                    5 => Operation::Idiv,
                    6 => Operation::Mod,
                    7 => Operation::Eq,
                    8 => Operation::Neq,
                    9 => Operation::Lt,
                    10 => Operation::Gt,
                    11 => Operation::Leq,
                    12 => Operation::Geq,
                    13 => Operation::Land,
                    14 => Operation::Lor,
                    15 => Operation::Shl,
                    16 => Operation::Shr,
                    17 => Operation::Bor,
                    18 => Operation::Band,
                    19 => Operation::Bxor,
                    _ => return Err(Error::new(
                        ErrorKind::InvalidData,
                        format!("Unknown DuoOp operation value: {}", value),
                    )),
                };
            },
            2 => {
                a_idx = value as usize;
            },
            3 => {
                b_idx = value as usize;
            },
            _ => {
                return Err(Error::new(ErrorKind::InvalidData, "Unknown DuoOpNode tag"));
            }
        }
    }

    nodes.push_noopt(Node::Op(op, a_idx, b_idx));
    Ok(())
}

fn decode_tres_op_node<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    bytes: &[u8], nodes: &mut Nodes<T, NS>) -> Result<(), Error> {

    if bytes.is_empty() {
        nodes.push_noopt(Node::TresOp(TresOperation::TernCond, 0, 0, 0));
        return Ok(());
    }

    let mut offset = 0;

    let mut op = TresOperation::TernCond;
    let mut a_idx: usize = 0;
    let mut b_idx: usize = 0;
    let mut c_idx: usize = 0;

    // Process all fields in the message
    while offset < bytes.len() {
        let (field_number, wire_type, tag_size) = read_tag(&bytes[offset..])?;
        offset += tag_size;

        if !matches!(wire_type, WireType::Varint) {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "Expected varint as TresOpNode field",
            ));
        }

        let (value, varint_size) = decode_varint_u32(&bytes[offset..])?;
        offset += varint_size;

        match field_number {
            1 => {
                op = match value {
                    0 => TresOperation::TernCond,
                    _ => return Err(Error::new(
                        ErrorKind::InvalidData,
                        format!("Unknown TresOp operation value: {}", value),
                    )),
                };
            },
            2 => {
                a_idx = value as usize;
            },
            3 => {
                b_idx = value as usize;
            },
            4 => {
                c_idx = value as usize;
            },
            _ => {
                return Err(Error::new(ErrorKind::InvalidData, "Unknown TresOpNode tag"));
            }
        }
    }

    nodes.push_noopt(Node::TresOp(op, a_idx, b_idx, c_idx));
    Ok(())
}

fn decode_constant_node<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    bytes: &[u8], nodes: &mut Nodes<T, NS>) -> Result<(), Error> {

    if bytes.is_empty() {
        return Err(Error::new(
            ErrorKind::UnexpectedEof,
            "Empty input buffer",
        ));
    }

    let (field_number, wire_type, tag_size) = read_tag(bytes)?;

    if field_number != 1 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "Expected field number 1 for ConstantNode",
        ));
    }

    if !matches!(wire_type, WireType::Len) {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "Expected length-delimited field for ConstantNode",
        ));
    }

    let bytes = &bytes[tag_size..];
    let (length, varint_size) = decode_varint_u32(bytes)?;
    let bytes = &bytes[varint_size..];
    if bytes.len() != length as usize {
        return Err(Error::new(
            ErrorKind::UnexpectedEof,
            "Incorrect ConstantNode field size",
        ));
    }

    let n = decode_big_le_bytes(bytes)?;
    let v = (&nodes.ff).parse_le_bytes(&n).map_err(|_| {
        Error::new(ErrorKind::InvalidData, "Invalid BigInt bytes")
    })?;
    nodes.const_node_idx_from_value(v);
    Ok(())
}

fn read_tag(bytes: &[u8]) -> Result<(u32, WireType, usize), Error> {
    let (tag, consumed) = decode_varint_u32(bytes)?;
    let field_number = tag >> 3;
    let wire_type = TryFrom::<u8>::try_from((tag & 0x7) as u8).unwrap();
    Ok((field_number, wire_type, consumed))
}


#[inline]
pub fn decode_varint_u32(bytes: &[u8]) -> Result<(u32, usize), Error> {
    // Fast-path optimization for empty slices
    if bytes.is_empty() {
        return Err(Error::new(
            ErrorKind::UnexpectedEof,
            "Empty input buffer",
        ));
    }

    // Fast-path for single-byte varints (very common case)
    let first_byte = bytes[0];
    if first_byte < 0x80 {
        return Ok((first_byte as u32, 1));
    }

    // We need at least 2 bytes now
    if bytes.len() < 2 {
        return Err(Error::new(
            ErrorKind::UnexpectedEof,
            "Incomplete varint in the input",
        ));
    }

    // Unrolled loop for the remaining bytes - faster than iterating
    let mut result: u32 = (first_byte & 0x7F) as u32;

    let second_byte = bytes[1];
    if second_byte < 0x80 {
        result |= (second_byte as u32) << 7;
        return Ok((result, 2));
    }

    if bytes.len() < 3 {
        return Err(Error::new(
            ErrorKind::UnexpectedEof,
            "Incomplete varint in the input",
        ));
    }

    result |= ((second_byte & 0x7F) as u32) << 7;

    let third_byte = bytes[2];
    if third_byte < 0x80 {
        result |= (third_byte as u32) << 14;
        return Ok((result, 3));
    }

    if bytes.len() < 4 {
        return Err(Error::new(
            ErrorKind::UnexpectedEof,
            "Incomplete varint in the input",
        ));
    }

    result |= ((third_byte & 0x7F) as u32) << 14;

    let fourth_byte = bytes[3];
    if fourth_byte < 0x80 {
        result |= (fourth_byte as u32) << 21;
        return Ok((result, 4));
    }

    if bytes.len() < 5 {
        return Err(Error::new(
            ErrorKind::UnexpectedEof,
            "Incomplete varint in the input",
        ));
    }

    result |= ((fourth_byte & 0x7F) as u32) << 21;

    let fifth_byte = bytes[4];
    // For u32, the fifth byte can only use 4 bits (plus the continuation bit)
    if fifth_byte > 0x0F {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "Varint value exceeds u32::MAX",
        ));
    }

    if fifth_byte < 0x80 {
        result |= (fifth_byte as u32) << 28;
        return Ok((result, 5));
    }

    // If we get here, the varint is invalid (too many continuation bits)
    Err(Error::new(
        ErrorKind::InvalidData,
        "Varint is too long for u32",
    ))
}
