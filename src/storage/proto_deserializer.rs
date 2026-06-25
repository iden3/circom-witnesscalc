use std::io::{Cursor, Error, ErrorKind, Read};
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

    let nodes_num = read_u64_le(bytes, idx, "Graph artifact missing node count")?;
    idx += 8;

    // Layout: [magic][u64 node count][node messages][metadata][u64 metadata offset].
    // The trailing u64 records where the metadata message begins.
    const MISSING_METADATA_PTR: &str = "Graph artifact missing trailing metadata pointer";
    if bytes.len() - idx < 8 {
        return Err(Error::new(ErrorKind::UnexpectedEof, MISSING_METADATA_PTR));
    }

    let metadata_ptr_slot = bytes.len() - 8;
    let metadata_offset = read_u64_le(bytes, metadata_ptr_slot, MISSING_METADATA_PTR)?;
    let metadata_offset: usize = metadata_offset.try_into().map_err(|_| {
        Error::new(
            ErrorKind::InvalidData,
            format!("Metadata offset {} cannot fit usize", metadata_offset),
        )
    })?;
    if metadata_offset < idx || metadata_offset > metadata_ptr_slot {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!("Metadata offset {} is outside graph payload", metadata_offset),
        ));
    }

    let metadata_bytes = checked_range(
        bytes,
        metadata_offset,
        metadata_ptr_slot - metadata_offset,
        "Graph metadata bytes are truncated",
    )?;
    let r = Cursor::new(metadata_bytes);
    let mut br = WriteBackReader::new(r);
    let md: crate::proto::GraphMetadata = read_message(&mut br)?;
    let mut trailing = [0u8; 1];
    if br.read(&mut trailing)? != 0 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "Graph metadata has trailing bytes",
        ));
    }

    let (prime, curve_name) = if let Some(prime) = &md.prime {
        (
            <U254 as FieldOps>::from_le_bytes(prime.value_le.as_slice()).map_err(|err| {
                Error::new(
                    ErrorKind::InvalidData,
                    format!("Invalid metadata prime: {}", err),
                )
            })?,
            md.prime_str.as_str(),
        )
    } else {
        (
            U254::from_str(
                "21888242871839275222246405745257275088548364400416034343698204186575808495617")
                .unwrap(),
            "bn128"
        )
    };

    let outer_nodes: Box<dyn NodesInterface> = match prime.bit_len() {
        64 => {
            let prime = U64::from_le_bytes(
                &<U254 as FieldOps>::to_le_bytes(&prime))
                .map_err(|err| {
                    Error::new(
                        ErrorKind::InvalidData,
                        format!("Invalid 64-bit metadata prime: {}", err),
                    )
                })?;
            let node_storage = VecNodes::new();
            let mut nodes = Nodes::new(
                prime, curve_name, node_storage);
            decode_graph_nodes(bytes, idx, nodes_num, metadata_offset, &mut nodes)?;
            Box::new(nodes)
        }
        254 => {
            let node_storage = VecNodes::new();
            let mut nodes = Nodes::new(
                prime, curve_name, node_storage);
            decode_graph_nodes(bytes, idx, nodes_num, metadata_offset, &mut nodes)?;
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
        .map(|x| usize::try_from(*x).map_err(|_| {
            Error::new(ErrorKind::InvalidData, "Witness signal index does not fit usize")
        }))
        .collect::<Result<Vec<usize>, Error>>()?;

    let inputs_info = if bytes.starts_with(WITNESSCALC_GRAPH_MAGIC_001) {
        let inputs = md.inputs.iter()
            .map(|(k, v)| {
                let offset = usize::try_from(v.offset).map_err(|_| {
                    Error::new(ErrorKind::InvalidData, "Input offset does not fit usize")
                })?;
                let len = usize::try_from(v.len).map_err(|_| {
                    Error::new(ErrorKind::InvalidData, "Input length does not fit usize")
                })?;
                Ok((k.clone(), (offset, len)))
            })
            .collect::<Result<InputSignalsInfo, Error>>()?;
        InputInfo::V1(inputs)
    } else if bytes.starts_with(WITNESSCALC_GRAPH_MAGIC_002) {
        let (input_info, types) = deserialize_input_signal_info(&md.input_signal_info)?;
        InputInfo::V2 { input_info, types }
    } else {
        unreachable!("magic bytes already validated at start of function")
    };

    Ok((outer_nodes, witness_signals, inputs_info))
}

fn read_u64_le(bytes: &[u8], offset: usize, context: &str) -> Result<u64, Error> {
    let bytes = checked_range(bytes, offset, 8, context)?;
    let mut buf = [0u8; 8];
    buf.copy_from_slice(bytes);
    Ok(u64::from_le_bytes(buf))
}

fn checked_range<'a>(
    bytes: &'a [u8],
    offset: usize,
    len: usize,
    context: &str,
) -> Result<&'a [u8], Error> {
    let end = offset.checked_add(len).ok_or_else(|| {
        Error::new(ErrorKind::InvalidData, "Byte range length overflows usize")
    })?;
    bytes.get(offset..end).ok_or_else(|| Error::new(ErrorKind::UnexpectedEof, context))
}

fn decode_graph_nodes<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    bytes: &[u8],
    mut idx: usize,
    nodes_num: u64,
    metadata_offset: usize,
    nodes: &mut Nodes<T, NS>,
) -> Result<(), Error> {
    for _ in 0..nodes_num {
        if idx > metadata_offset {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "Node cursor moved past metadata offset",
            ));
        }

        let (msg_len, int_len) = decode_varint_u32(&bytes[idx..metadata_offset])?;
        idx += int_len;
        let msg_len = msg_len as usize;
        let msg_end = idx.checked_add(msg_len).ok_or_else(|| {
            Error::new(ErrorKind::InvalidData, "Node length overflows usize")
        })?;
        // Nodes sit before the metadata, so msg_end must stay within it. A node
        // that runs past the whole artifact is truncation (UnexpectedEof); one
        // that fits the buffer but spills into the metadata is malformed
        // (InvalidData).
        if msg_end > bytes.len() {
            return Err(Error::new(
                ErrorKind::UnexpectedEof,
                "Node length extends past graph artifact",
            ));
        }
        if msg_end > metadata_offset {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "Node length extends past metadata offset",
            ));
        }

        let node_bytes = checked_range(
            bytes,
            idx,
            msg_len,
            "Node bytes are truncated",
        )?;
        let nodes_before = nodes.len();
        decode_node(node_bytes, nodes)?;
        if nodes.len() != nodes_before + 1 {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "Decoded node record did not produce exactly one graph node",
            ));
        }
        idx = msg_end;
    }

    if idx != metadata_offset {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!(
                "Decoded node bytes ended at {} but metadata starts at {}",
                idx, metadata_offset),
        ));
    }

    Ok(())
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
        _ => Err(Error::new(
            ErrorKind::InvalidData,
            format!("Found unknown node field number {}", field_number),
        )),
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
    let wire_type = (tag & 0x7) as u8;
    let wire_type = WireType::try_from(wire_type).map_err(|_| {
        Error::new(
            ErrorKind::InvalidData,
            format!("Invalid protobuf wire type {}", wire_type),
        )
    })?;
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
