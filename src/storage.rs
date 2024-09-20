use std::io::{Write, Read};
use ark_bn254::Fr;
use ark_ff::{PrimeField};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use prost::Message;
use crate::graph::{Operation, TresOperation, UnoOperation};
use crate::InputSignalsInfo;

// format of the wtns.graph file:
// + magic line: wtns.graph.001
// + 4 bytes unsigned LE 32-bit integer: number of nodes
// + series of protobuf serialized nodes. Each node prefixed by varint length
// + protobuf serialized GraphMetadata
// + 8 bytes unsigned LE 64-bit integer: offset of GraphMetadata message

const WITNESSCALC_GRAPH_MAGIC: &[u8] = b"wtns.graph.001";

const MAX_VARINT_LENGTH: usize = 10;

impl From<crate::proto::Node> for crate::graph::Node {
    fn from(value: crate::proto::Node) -> Self {
        match value.node.unwrap() {
            crate::proto::node::Node::Input(input_node) => {
                crate::graph::Node::Input(input_node.idx as usize)
            }
            crate::proto::node::Node::Constant(constant_node) => {
                let i = constant_node.value.unwrap();
                crate::graph::Node::MontConstant(Fr::from_le_bytes_mod_order(i.value_le.as_slice()))
            }
            crate::proto::node::Node::UnoOp(uno_op_node) => {
                let op = crate::proto::UnoOp::try_from(uno_op_node.op).unwrap();
                crate::graph::Node::UnoOp(op.into(), uno_op_node.a_idx as usize)
            }
            crate::proto::node::Node::DuoOp(duo_op_node) => {
                let op = crate::proto::DuoOp::try_from(duo_op_node.op).unwrap();
                crate::graph::Node::Op(
                    op.into(), duo_op_node.a_idx as usize,
                    duo_op_node.b_idx as usize)
            }
            crate::proto::node::Node::TresOp(tres_op_node) => {
                let op = crate::proto::TresOp::try_from(tres_op_node.op).unwrap();
                crate::graph::Node::TresOp(
                    op.into(), tres_op_node.a_idx as usize,
                    tres_op_node.b_idx as usize, tres_op_node.c_idx as usize)
            }
        }
    }
}

impl From<&crate::graph::Node> for crate::proto::node::Node {
    fn from(node: &crate::graph::Node) -> Self {
        match node {
            crate::graph::Node::Input(i) => {
                crate::proto::node::Node::Input (crate::proto::InputNode {
                    idx: i.clone() as u32
                })
            }
            crate::graph::Node::Constant(_) => {
                panic!("We are not supposed to write Constant to the witnesscalc graph. All Constant should be converted to MontConstant.");
            }
            crate::graph::Node::UnoOp(op, a) => {
                let op = crate::proto::UnoOp::from(op);
                crate::proto::node::Node::UnoOp(
                    crate::proto::UnoOpNode {
                        op: op as i32,
                        a_idx: a.clone() as u32 })
            }
            crate::graph::Node::Op(op, a, b) => {
                crate::proto::node::Node::DuoOp(
                    crate::proto::DuoOpNode {
                        op: crate::proto::DuoOp::from(op) as i32,
                        a_idx: a.clone() as u32,
                        b_idx: b.clone() as u32 })
            }
            crate::graph::Node::TresOp(op, a, b, c) => {
                crate::proto::node::Node::TresOp(
                    crate::proto::TresOpNode {
                        op: crate::proto::TresOp::from(op) as i32,
                        a_idx: a.clone() as u32,
                        b_idx: b.clone() as u32,
                        c_idx: c.clone() as u32 })
            }
            crate::graph::Node::MontConstant(c) => {
                let bi = Into::<num_bigint::BigUint>::into(c.clone());
                let i = crate::proto::BigUInt { value_le: bi.to_bytes_le() };
                crate::proto::node::Node::Constant(
                    crate::proto::ConstantNode { value: Some(i) })
            }
        }
    }
}

impl From<crate::proto::UnoOp> for UnoOperation {
    fn from(value: crate::proto::UnoOp) -> Self {
        match value {
            crate::proto::UnoOp::Neg => UnoOperation::Neg,
            crate::proto::UnoOp::Id => UnoOperation::Id,
        }
    }
}

impl From<crate::proto::DuoOp> for Operation {
    fn from(value: crate::proto::DuoOp) -> Self {
        match value {
            crate::proto::DuoOp::Mul => Operation::Mul,
            crate::proto::DuoOp::Div => Operation::Div,
            crate::proto::DuoOp::Add => Operation::Add,
            crate::proto::DuoOp::Sub => Operation::Sub,
            crate::proto::DuoOp::Pow => Operation::Pow,
            crate::proto::DuoOp::Idiv => Operation::Idiv,
            crate::proto::DuoOp::Mod => Operation::Mod,
            crate::proto::DuoOp::Eq => Operation::Eq,
            crate::proto::DuoOp::Neq => Operation::Neq,
            crate::proto::DuoOp::Lt => Operation::Lt,
            crate::proto::DuoOp::Gt => Operation::Gt,
            crate::proto::DuoOp::Leq => Operation::Leq,
            crate::proto::DuoOp::Geq => Operation::Geq,
            crate::proto::DuoOp::Land => Operation::Land,
            crate::proto::DuoOp::Lor => Operation::Lor,
            crate::proto::DuoOp::Shl => Operation::Shl,
            crate::proto::DuoOp::Shr => Operation::Shr,
            crate::proto::DuoOp::Bor => Operation::Bor,
            crate::proto::DuoOp::Band => Operation::Band,
            crate::proto::DuoOp::Bxor => Operation::Bxor,
        }
    }
}

impl From<crate::proto::TresOp> for TresOperation {
    fn from(value: crate::proto::TresOp) -> Self {
        match value {
            crate::proto::TresOp::TernCond => TresOperation::TernCond,
        }
    }
}

pub fn serialize_witnesscalc_graph<T: Write>(
    mut w: T, nodes: &Vec<crate::graph::Node>, witness_signals: &Vec<usize>,
    input_signals: &InputSignalsInfo) -> std::io::Result<()> {

    let mut ptr = 0usize;
    w.write_all(WITNESSCALC_GRAPH_MAGIC).unwrap();
    ptr += WITNESSCALC_GRAPH_MAGIC.len();

    w.write_u64::<LittleEndian>(nodes.len() as u64)?;
    ptr += 8;

    let metadata = crate::proto::GraphMetadata {
        witness_signals: witness_signals.iter().map(|x| *x as u32).collect::<Vec<u32>>(),
        inputs: input_signals.iter().map(|(k, v)| {
            let sig = crate::proto::SignalDescription {
                offset: v.0 as u32,
                len: v.1 as u32 };
            (k.clone(), sig)
        }).collect()
    };

    // capacity of buf should be enough to hold the largest message + 10 bytes
    // of varint length
    let mut buf =
        Vec::with_capacity(metadata.encoded_len() + MAX_VARINT_LENGTH);

    for node in nodes {
        let node_pb = crate::proto::Node{
            node: Some(crate::proto::node::Node::from(node)),
        };

        assert_eq!(buf.len(), 0);
        node_pb.encode_length_delimited(&mut buf)?;
        ptr += buf.len();

        w.write_all(&buf)?;
        buf.clear();
    }

    metadata.encode_length_delimited(&mut buf)?;
    w.write_all(&buf)?;
    buf.clear();

    w.write_u64::<LittleEndian>(ptr as u64)?;

    Ok(())
}

fn read_message_length<R: Read>(rw: &mut WriteBackReader<R>) -> std::io::Result<usize> {
    let mut buf = [0u8; MAX_VARINT_LENGTH];
    rw.read(&mut buf)?;

    let n = prost::decode_length_delimiter(buf.as_ref())?;

    let lnln = prost::length_delimiter_len(n);

    if lnln < buf.len() {
        rw.write(&buf[lnln..])?;
    }

    Ok(n)
}

fn read_message<R: Read, M: Message + std::default::Default>(rw: &mut WriteBackReader<R>) -> std::io::Result<M> {
    let ln = read_message_length(rw)?;
    let mut buf = vec![0u8; ln];
    let bytes_read = rw.read(&mut buf)?;
    if bytes_read != ln {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof, "Unexpected EOF"));
    }

    let msg = prost::Message::decode(&buf[..])?;

    Ok(msg)
}

pub fn deserialize_witnesscalc_graph(
    r: impl Read) -> std::io::Result<(Vec<crate::graph::Node>, Vec<usize>, InputSignalsInfo)> {

    let mut br = WriteBackReader::new(r);
    let mut magic = [0u8; WITNESSCALC_GRAPH_MAGIC.len()];

    br.read_exact(&mut magic)?;

    if !magic.eq(WITNESSCALC_GRAPH_MAGIC) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData, "Invalid magic"));
    }

    let mut nodes = Vec::new();
    let nodes_num = br.read_u64::<LittleEndian>()?;
    for _ in 0..nodes_num {
        let n: crate::proto::Node = read_message(&mut br)?;
        let n2: crate::graph::Node = n.into();
        nodes.push(n2);
    }

    let md: crate::proto::GraphMetadata = read_message(&mut br)?;

    let witness_signals = md.witness_signals
        .iter()
        .map(|x| *x as usize)
        .collect::<Vec<usize>>();

    let input_signals = md.inputs.iter()
        .map(|(k, v)| {
            (k.clone(), (v.offset as usize, v.len as usize))
        })
        .collect::<InputSignalsInfo>();

    Ok((nodes, witness_signals, input_signals))
}

struct WriteBackReader<R: Read> {
    reader: R,
    buffer: Vec<u8>,
}

impl <R: Read> WriteBackReader<R> {
    fn new(reader: R) -> Self {
        WriteBackReader {
            reader,
            buffer: Vec::new(),
        }
    }
}

impl<R: Read> Read for WriteBackReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.len() == 0 {
            return Ok(0)
        }

        let mut n = 0usize;

        if !self.buffer.is_empty() {
            n = std::cmp::min(buf.len(), self.buffer.len());
            self.buffer[self.buffer.len()-n..]
                .iter()
                .rev()
                .enumerate()
                .for_each(|(i, x)| {
                    buf[i] = *x;
                });
            self.buffer.truncate(self.buffer.len() - n);
        }

        while n < buf.len() {
            let m = self.reader.read(&mut buf[n..])?;
            if m == 0 {
                break;
            }
            n += m;
        }

        Ok(n)
    }
}

impl<R: Read> Write for WriteBackReader<R> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.buffer.reserve(buf.len());
        self.buffer.extend(buf.iter().rev());
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use crate::graph::{Operation, TresOperation, UnoOperation};
    use core::str::FromStr;
    use super::*;

    #[test]
    fn test_read_message() {
        let mut buf = Vec::new();
        let n1 = crate::proto::Node {
            node: Some(crate::proto::node::Node::Input(
                crate::proto::InputNode { idx: 1 }))
        };
        n1.encode_length_delimited(&mut buf).unwrap();

        let n2 = crate::proto::Node {
            node: Some(crate::proto::node::Node::Input(
                crate::proto::InputNode { idx: 2 }))
        };
        n2.encode_length_delimited(&mut buf).unwrap();

        let mut reader = std::io::Cursor::new(&buf);

        let mut rw = WriteBackReader::new(&mut reader);

        let got_n1: crate::proto::Node = read_message(&mut rw).unwrap();
        assert!(n1.eq(&got_n1));

        let got_n2: crate::proto::Node = read_message(&mut rw).unwrap();
        assert!(n2.eq(&got_n2));

        assert_eq!(reader.position(), buf.len() as u64);
    }

    #[test]
    fn test_read_message_variant() {
        let nodes = vec![
            crate::proto::Node {
                node: Some(
                    crate::proto::node::Node::from(&crate::graph::Node::Input(0))
                )
            },
            crate::proto::Node {
                node: Some(
                    crate::proto::node::Node::from(
                        &crate::graph::Node::MontConstant(
                            Fr::from_str("1").unwrap()))
                )
            },
            crate::proto::Node {
                node: Some(
                    crate::proto::node::Node::from(&crate::graph::Node::UnoOp(UnoOperation::Id, 4))
                )
            },
            crate::proto::Node {
                node: Some(
                    crate::proto::node::Node::from(&crate::graph::Node::Op(Operation::Mul, 5, 6))
                )
            },
            crate::proto::Node {
                node: Some(
                    crate::proto::node::Node::from(&crate::graph::Node::TresOp(TresOperation::TernCond, 7, 8, 9))
                )
            },
        ];

        let mut buf = Vec::new();
        for n in &nodes {
            n.encode_length_delimited(&mut buf).unwrap();
        }

        let mut nodes_got: Vec<crate::proto::Node> = Vec::new();
        let mut reader = std::io::Cursor::new(&buf);
        let mut rw = WriteBackReader::new(&mut reader);
        for _ in 0..nodes.len() {
            nodes_got.push(read_message(&mut rw).unwrap());
        }

        assert_eq!(nodes, nodes_got);
    }

    #[test]
    fn test_write_back_reader() {
        let data = [1u8, 2, 3, 4, 5, 6];
        let mut r = WriteBackReader::new(std::io::Cursor::new(&data));

        let buf = &mut [0u8; 5];
        r.read(buf).unwrap();
        assert_eq!(buf, &[1, 2, 3, 4, 5]);

        // return [4, 5] to reader
        r.write(&buf[3..]).unwrap();
        // return [2, 3] to reader
        r.write(&buf[1..3]).unwrap();

        buf.fill(0);

        // read 3 bytes, expect [2, 3, 4] after returns
        let mut n = r.read(&mut buf[..3]).unwrap();
        assert_eq!(n, 3);
        assert_eq!(buf, &[2, 3, 4, 0, 0]);

        buf.fill(0);

        // read everything left in reader
        n = r.read(buf).unwrap();
        assert_eq!(n, 2);
        assert_eq!(buf, &[5, 6, 0, 0, 0]);
    }

    #[test]
    fn test_deserialize_inputs() {
        let nodes = vec![
            crate::graph::Node::Input(0),
            crate::graph::Node::MontConstant(Fr::from_str("1").unwrap()),
            crate::graph::Node::UnoOp(UnoOperation::Id, 4),
            crate::graph::Node::Op(Operation::Mul, 5, 6),
            crate::graph::Node::TresOp(TresOperation::TernCond, 7, 8, 9),
        ];

        let witness_signals = vec![4, 1];

        let mut input_signals: InputSignalsInfo = HashMap::new();
        input_signals.insert("sig1".to_string(), (1, 3));
        input_signals.insert("sig2".to_string(), (5, 1));

        let mut tmp = Vec::new();
        serialize_witnesscalc_graph(&mut tmp, &nodes, &witness_signals, &input_signals).unwrap();

        let mut reader = std::io::Cursor::new(&tmp);

        let (nodes_res, witness_signals_res, input_signals_res) =
            deserialize_witnesscalc_graph(&mut reader).unwrap();

        assert_eq!(nodes, nodes_res);
        assert_eq!(input_signals, input_signals_res);
        assert_eq!(witness_signals, witness_signals_res);

        let metadata_start = LittleEndian::read_u64(&tmp[tmp.len() - 8..]);

        let mt_reader = std::io::Cursor::new(&tmp[metadata_start as usize..]);
        let mut rw = WriteBackReader::new(mt_reader);
        let metadata: crate::proto::GraphMetadata = read_message(&mut rw).unwrap();

        let metadata_want = crate::proto::GraphMetadata {
            witness_signals: vec![4, 1],
            inputs: input_signals.iter().map(|(k, v)| {
                (k.clone(), crate::proto::SignalDescription {
                    offset: v.0 as u32,
                    len: v.1 as u32
                })
            }).collect()
        };

        assert_eq!(metadata, metadata_want);
    }
}