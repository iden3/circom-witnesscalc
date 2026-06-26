use std::collections::{BTreeMap, HashMap};
#[cfg(test)]
use std::fmt::{Debug, Formatter};
use std::io::{Error, ErrorKind, Read, Write};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use num_bigint::BigUint;
use prost::Message;
use ruint::aliases::U256;
use crate::field::{FieldOps, Field};
use crate::graph::{Nodes, NodesStorage, Operation, TresOperation, UnoOperation};
use crate::proto::SignalDescription;
use crate::proto::vm::{IoDef, IoDefs};
use crate::vm::{Function, Template, validate_compiled_bytecode};
use crate::vm2;

fn write_signal<W: Write>(w: &mut W, signal: &vm2::Signal) -> std::io::Result<()> {
    match signal {
        vm2::Signal::Ff(dims) => {
            w.write_u8(0)?; // Signal type: Ff
            w.write_u32::<LittleEndian>(dims.len() as u32)?;
            for dim in dims {
                w.write_u32::<LittleEndian>(*dim as u32)?;
            }
        }
        vm2::Signal::Bus(type_idx, dims) => {
            w.write_u8(1)?; // Signal type: Bus
            w.write_u32::<LittleEndian>(*type_idx as u32)?;
            w.write_u32::<LittleEndian>(dims.len() as u32)?;
            for dim in dims {
                w.write_u32::<LittleEndian>(*dim as u32)?;
            }
        }
    }
    Ok(())
}

fn read_signal<R: Read>(r: &mut R) -> std::io::Result<vm2::Signal> {
    let signal_type = r.read_u8()?;
    match signal_type {
        0 => {
            // Ff signal
            let num_dims = r.read_u32::<LittleEndian>()? as usize;
            let mut dims = Vec::with_capacity(num_dims);
            for _ in 0..num_dims {
                dims.push(r.read_u32::<LittleEndian>()? as usize);
            }
            Ok(vm2::Signal::Ff(dims))
        }
        1 => {
            // Bus signal
            let type_idx = r.read_u32::<LittleEndian>()? as usize;
            let num_dims = r.read_u32::<LittleEndian>()? as usize;
            let mut dims = Vec::with_capacity(num_dims);
            for _ in 0..num_dims {
                dims.push(r.read_u32::<LittleEndian>()? as usize);
            }
            Ok(vm2::Signal::Bus(type_idx, dims))
        }
        _ => Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid signal type"))
    }
}

fn write_string_vec<W: Write>(w: &mut W, vec: &Vec<String>) -> std::io::Result<()> {
    let count = vec.len();

    // Calculate total_bytes: 4 (count) + sum of (4 + value.len()) for each entry
    let total_bytes: u32 = 4 + vec.iter()
        .map(|v| 4 + v.len() as u32)
        .sum::<u32>();

    // Write: total_bytes, count, then each (value_len, value_bytes)
    w.write_u32::<LittleEndian>(total_bytes)?;
    w.write_u32::<LittleEndian>(count as u32)?;

    for value in vec {
        w.write_u32::<LittleEndian>(value.len() as u32)?;
        if !value.is_empty() {
            w.write_all(value.as_bytes())?;
        }
    }

    Ok(())
}

fn read_string_vec<R: Read>(r: &mut R) -> std::io::Result<Vec<String>> {
    let total_bytes = r.read_u32::<LittleEndian>()?;
    let count = r.read_u32::<LittleEndian>()? as usize;

    let mut vec = Vec::with_capacity(count);
    let mut bytes_read = 4u32; // count field

    for _ in 0..count {
        let value_len = r.read_u32::<LittleEndian>()? as usize;
        bytes_read += 4;

        if value_len > 0 {
            let mut value_bytes = vec![0u8; value_len];
            r.read_exact(&mut value_bytes)?;
            bytes_read += value_len as u32;

            let value = String::from_utf8(value_bytes)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData,
                    format!("Invalid UTF-8 in variable name: {}", e)))?;
            vec.push(value);
        } else {
            vec.push(String::new());
        }
    }

    // Validate that we read exactly total_bytes
    if bytes_read != total_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Variable names data corruption: expected {} bytes, read {} bytes",
                    total_bytes, bytes_read)
        ));
    }

    Ok(vec)
}

// format of the wtns.graph file:
// + magic line: wtns.graph.001
// + 4 bytes unsigned LE 32-bit integer: number of nodes
// + series of protobuf serialized nodes. Each node prefixed by varint length
// + protobuf serialized GraphMetadata
// + 8 bytes unsigned LE 64-bit integer: offset of GraphMetadata message

pub mod proto_deserializer;

pub(crate) const WITNESSCALC_GRAPH_MAGIC_001: &[u8] = b"wtns.graph.001";
pub(crate) const WITNESSCALC_GRAPH_MAGIC_002: &[u8] = b"wtns.graph.002";
const WITNESSCALC_VM_MAGIC: &[u8] = b"wtns.vm.001";
pub(crate) const WITNESSCALC_CVM_MAGIC: &[u8] = b"wtns.cvm.001";

const MAX_VARINT_LENGTH: usize = 10;

impl From<crate::proto::UnoOp> for UnoOperation {
    fn from(value: crate::proto::UnoOp) -> Self {
        match value {
            crate::proto::UnoOp::Neg => UnoOperation::Neg,
            crate::proto::UnoOp::Id => UnoOperation::Id,
            crate::proto::UnoOp::Lnot => UnoOperation::Lnot,
            crate::proto::UnoOp::Bnot => UnoOperation::Bnot,
            crate::proto::UnoOp::Sqrt => UnoOperation::Sqrt,
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

pub fn serialize_witnesscalc_graph<W, T, NS>(
    mut w: W, nodes: &Nodes<T, NS>, witness_signals: &[usize],
    input_infos: &[vm2::InputInfo], types: &[vm2::Type]) -> std::io::Result<()>
    where
        W: Write,
        T: FieldOps + 'static,
        NS: NodesStorage + 'static {

    let mut ptr = 0usize;
    w.write_all(WITNESSCALC_GRAPH_MAGIC_002).unwrap();
    ptr += WITNESSCALC_GRAPH_MAGIC_002.len();

    w.write_u64::<LittleEndian>(nodes.nodes.len() as u64)?;
    ptr += 8;

    let metadata = crate::proto::GraphMetadata {
        witness_signals: witness_signals.iter().map(|x| *x as u32).collect::<Vec<u32>>(),
        // empty value, this value is used in 001 version, superseded by input_signal_info
        inputs: HashMap::new(),
        prime: Some(crate::proto::BigUInt {
            value_le: nodes.prime().to_le_bytes()
        }),
        prime_str: nodes.prime_str(),
        input_signal_info: serialize_input_signal_info(input_infos, types)?,
    };

    // capacity of buf should be enough to hold the largest message + 10 bytes
    // of varint length
    let mut buf =
        Vec::with_capacity(metadata.encoded_len() + MAX_VARINT_LENGTH);

    for i in 0..nodes.len() {
        let node = nodes.to_proto(i).unwrap();
        let node_pb = crate::proto::Node{ node: Some(node) };

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

pub type InputList = Vec<(String, usize, usize)>;

pub struct IODef {
    pub code: usize,
    pub offset: usize,
    pub lengths: Vec<usize>,
}

pub type InputOutputList = Vec<IODef>;

pub type TemplateInstanceIOMap = BTreeMap<usize, InputOutputList>;

pub struct CompiledCircuit {
    pub main_template_id: usize,
    pub templates: Vec<Template>,
    pub functions: Vec<Function>,
    pub signals_num: usize,
    pub constants: Vec<U256>,
    pub inputs: InputList,
    pub witness_signals: Vec<usize>,
    pub io_map: TemplateInstanceIOMap,
}

#[cfg(test)]
impl Debug for CompiledCircuit {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledCircuit")
            .field("main_template_id", &self.main_template_id)
            .field("templates", &self.templates)
            .field("functions", &self.functions)
            .field("signals_num", &self.signals_num)
            .field("constants", &self.constants.iter().map(|x| x.to_string()).collect::<Vec<String>>())
            .field("inputs", &self.inputs)
            .field("witness_signals", &self.witness_signals)
            .field(
                "io_map",
                &self.io_map.iter()
                    .map( |(&x, y)|
                        (
                            x,
                            y.iter().map(|z| (z.code, z.offset, z.lengths.clone())).collect::<Vec<(usize, usize, Vec<usize>)>>(),
                        )
                    )
                    .collect::<Vec<(usize, Vec<(usize, usize, Vec<usize>)>)>>()
            )
            .finish()
    }
}

pub fn serialize_witnesscalc_vm(
    mut w: impl Write, cs: &CompiledCircuit) -> std::io::Result<()> {

    let inputs_desc = cs.inputs.iter()
        .map(|(name, offset, len)| {
            (
                name.clone(),
                SignalDescription {
                    offset: TryInto::<u32>::try_into(*offset)
                        .expect("signal offset is too large"),
                    len: TryInto::<u32>::try_into(*len)
                        .expect("signals length is too large"),
                },
            )
        }).collect::<HashMap<String, SignalDescription>>();

    w.write_all(WITNESSCALC_VM_MAGIC).unwrap();

    let io_map = cs.io_map.iter()
        .map(|(tmpl_idx, io_list)| {
            (
                TryInto::<u32>::try_into(*tmpl_idx)
                    .expect("io_map template index is too large"),
                IoDefs{
                    io_defs: io_list.iter()
                        .map(|x| IoDef{
                            code: x.code.try_into()
                                .expect("signal code is too large"),
                            offset: x.offset.try_into()
                                .expect("signal offset is too large"),
                            lengths: x.lengths.iter()
                                .map(|l| TryInto::<u32>::try_into(*l)
                                    .expect("signal length is too large"))
                                .collect::<Vec<u32>>(),
                        })
                        .collect()
                }
            )
        })
        .collect();

    let md = crate::proto::vm::VmMd {
        main_template_id: cs.main_template_id.try_into()
            .expect("main template id too large"),
        templates_num: TryInto::<u32>::try_into(cs.templates.len())
            .expect("too many templates"),
        functions_num: TryInto::<u32>::try_into(cs.functions.len())
            .expect("too many functions"),
        signals_num: TryInto::<u32>::try_into(cs.signals_num)
            .expect("too many signals"),
        constants_num: TryInto::<u32>::try_into(cs.constants.len())
            .expect("too many constants"),
        inputs: inputs_desc,
        witness_signals: cs.witness_signals.iter()
            .map(|x| TryInto::<u32>::try_into(*x)
                .expect("witness signal index is too large"))
            .collect(),
        io_map,
    };

    let mut buf = Vec::new();
    md.encode_length_delimited(&mut buf)?;
    w.write_all(&buf)?;
    buf.clear();

    for tmpl in &cs.templates {
        let tmpl_pb: crate::proto::vm::Template = tmpl.try_into().unwrap();
        assert_eq!(buf.len(), 0);
        tmpl_pb.encode_length_delimited(&mut buf)?;
        w.write_all(&buf)?;
        buf.clear();
    }

    for func in &cs.functions {
        let func_pb: crate::proto::vm::Function = func.try_into().unwrap();
        assert_eq!(buf.len(), 0);
        func_pb.encode_length_delimited(&mut buf)?;
        w.write_all(&buf)?;
        buf.clear();
    }

    for c in &cs.constants {
        w.write_all(c.as_le_slice())?;
    }

    Ok(())
}

fn read_message_length<R: Read>(rw: &mut WriteBackReader<R>) -> std::io::Result<usize> {
    let mut buf = [0u8; MAX_VARINT_LENGTH];
    let bytes_read = rw.read(&mut buf)?;
    if bytes_read == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof, "Unexpected EOF"));
    }

    let len_delimiter = prost::decode_length_delimiter(buf.as_ref())?;

    let lnln = prost::length_delimiter_len(len_delimiter);

    if lnln < bytes_read {
        rw.write_all(&buf[lnln..bytes_read])?;
    }

    Ok(len_delimiter)
}

fn read_message<R: Read, M: Message + Default>(rw: &mut WriteBackReader<R>) -> std::io::Result<M> {
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

pub fn deserialize_witnesscalc_vm(
    mut r: impl Read) -> std::io::Result<CompiledCircuit>{

    let mut br = WriteBackReader::new(&mut r);
    let mut magic = [0u8; WITNESSCALC_VM_MAGIC.len()];

    br.read_exact(&mut magic)?;

    if !magic.eq(WITNESSCALC_VM_MAGIC) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "vm file does not look like a witnesscalc vm file"));
    };

    let md: crate::proto::vm::VmMd = read_message(&mut br)?;

    let mut templates: Vec<Template> = Vec::with_capacity(md.templates_num as usize);
    for template_idx in 0..md.templates_num {
        let tmpl: crate::proto::vm::Template = read_message(&mut br)?;
        templates.push(Template::try_from(&tmpl)
            .map_err(|err| Error::new(
                ErrorKind::InvalidData,
                format!("template {} is invalid: {}", template_idx, err)))?);
    }

    let mut functions: Vec<Function> = Vec::with_capacity(md.functions_num as usize);
    for function_idx in 0..md.functions_num {
        let func: crate::proto::vm::Function = read_message(&mut br)?;
        functions.push(Function::try_from(&func)
            .map_err(|err| Error::new(
                ErrorKind::InvalidData,
                format!("function {} is invalid: {}", function_idx, err)))?);
    }

    let mut constants = Vec::with_capacity(md.constants_num as usize);
    for _ in 0 .. md.constants_num {
        let mut buf = [0u8; 32];
        br.read_exact(&mut buf)?;
        let c = U256::from_le_slice(&buf);
        constants.push(c);
    }

    let main_template_id = md.main_template_id.try_into()
        .map_err(|err| Error::new(
            ErrorKind::InvalidData,
            format!("main template id is invalid: {}", err)))?;
    let signals_num = md.signals_num.try_into()
        .map_err(|err| Error::new(
            ErrorKind::InvalidData,
            format!("signals number is invalid: {}", err)))?;
    let inputs: InputList = md.inputs.iter()
        .map(|(sig_name, sig_desc)| {
            Ok((
                sig_name.clone(),
                TryInto::<usize>::try_into(sig_desc.offset)
                    .map_err(|err| Error::new(
                        ErrorKind::InvalidData,
                        format!("input signal offset is invalid: {}", err)))?,
                TryInto::<usize>::try_into(sig_desc.len)
                    .map_err(|err| Error::new(
                        ErrorKind::InvalidData,
                        format!("input signal length is invalid: {}", err)))?,
            ))
        })
        .collect::<std::io::Result<_>>()?;
    let witness_signals: Vec<usize> = md.witness_signals.iter()
        .map(|x| TryInto::<usize>::try_into(*x)
            .map_err(|err| Error::new(
                ErrorKind::InvalidData,
                format!("witness signal index is invalid: {}", err))))
        .collect::<std::io::Result<_>>()?;
    let io_map: TemplateInstanceIOMap = md.io_map
        .iter()
        .map(|(tmpl_id, io_defs)| {
            Ok((
                TryInto::<usize>::try_into(*tmpl_id)
                    .map_err(|err| Error::new(
                        ErrorKind::InvalidData,
                        format!("template index is invalid: {}", err)))?,
                io_defs.io_defs.iter()
                    .map(|d| {
                        Ok(IODef {
                            code: d.code.try_into()
                                .map_err(|err| Error::new(
                                    ErrorKind::InvalidData,
                                    format!("signal code is invalid: {}", err)))?,
                            offset: d.offset.try_into()
                                .map_err(|err| Error::new(
                                    ErrorKind::InvalidData,
                                    format!("signal offset is invalid: {}", err)))?,
                            lengths: d.lengths.iter()
                                .map(|l| TryInto::<usize>::try_into(*l)
                                    .map_err(|err| Error::new(
                                        ErrorKind::InvalidData,
                                        format!("signal length is invalid: {}", err))))
                                .collect::<std::io::Result<_>>()?,
                        })
                    })
                    .collect::<std::io::Result<_>>()?,
            ))
        })
        .collect::<std::io::Result<_>>()?;

    validate_legacy_signal_ranges(
        signals_num, &inputs, &witness_signals, &io_map, templates.len())?;
    validate_compiled_bytecode(
        &templates, &functions, &io_map, constants.len(), main_template_id)
        .map_err(|err| Error::new(ErrorKind::InvalidData, err))?;

    Ok(CompiledCircuit {
        main_template_id,
        templates,
        functions,
        signals_num,
        constants,
        inputs,
        witness_signals,
        io_map,
    })
}

fn validate_legacy_signal_ranges(
    signals_num: usize, inputs: &InputList, witness_signals: &[usize],
    io_map: &TemplateInstanceIOMap, templates_len: usize) -> std::io::Result<()> {

    for (idx, witness_idx) in witness_signals.iter().enumerate() {
        if *witness_idx >= signals_num {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!(
                    "witness signal {} is out of bounds: {} >= {}",
                    idx, witness_idx, signals_num)));
        }
    }

    for (name, offset, len) in inputs {
        let end = offset.checked_add(*len)
            .ok_or_else(|| Error::new(
                ErrorKind::InvalidData,
                format!("input signal {} span overflows", name)))?;
        if end > signals_num {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!(
                    "input signal {} span {}..{} exceeds signals {}",
                    name, offset, end, signals_num)));
        }
    }

    for (template_id, io_defs) in io_map {
        if *template_id >= templates_len {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!(
                    "io_map template id {} is out of bounds (templates: {})",
                    template_id, templates_len)));
        }
        for (idx, io_def) in io_defs.iter().enumerate() {
            let len = io_def.lengths.iter().try_fold(1usize, |acc, len| {
                acc.checked_mul(*len)
                    .ok_or_else(|| Error::new(
                        ErrorKind::InvalidData,
                        format!(
                            "io_map template {} definition {} length overflows",
                            template_id, idx)))
            })?;
            let end = io_def.offset.checked_add(len)
                .ok_or_else(|| Error::new(
                    ErrorKind::InvalidData,
                    format!(
                        "io_map template {} definition {} span overflows",
                        template_id, idx)))?;
            if end > signals_num {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    format!(
                        "io_map template {} definition {} span {}..{} exceeds signals {}",
                        template_id, idx, io_def.offset, end, signals_num)));
            }
        }
    }

    Ok(())
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
        if buf.is_empty() {
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

pub fn init_input_signals(
    inputs_desc: &InputList,
    inputs: &HashMap<String, Vec<U256>>,
    signals: &mut [Option<U256>],
) {
    signals[0] = Some(U256::from(1u64));

    for (name, offset, len) in inputs_desc {
        match inputs.get(name) {
            Some(values) => {
                if values.len() != *len {
                    panic!(
                        "input signal {} has different length in inputs file, want {}, actual {}",
                        name, len, values.len());
                }
                for (i, v) in values.iter().enumerate() {
                    signals[*offset + i] = Some(*v);
                }
            }
            None => {
                panic!("input signal {} is not found in inputs file", name);
            }
        }
    }
}

pub fn serialize_input_infos<W: Write>(
    w: &mut W,
    input_infos: &[vm2::InputInfo],
) -> std::io::Result<()> {
    w.write_u32::<LittleEndian>(input_infos.len() as u32)?;
    for input_info in input_infos {
        w.write_u32::<LittleEndian>(input_info.name.len() as u32)?;
        w.write_all(input_info.name.as_bytes())?;

        w.write_u32::<LittleEndian>(input_info.offset as u32)?;

        w.write_u32::<LittleEndian>(input_info.lengths.len() as u32)?;
        for length in &input_info.lengths {
            w.write_u32::<LittleEndian>(*length as u32)?;
        }

        match &input_info.type_id {
            Some(type_id) => {
                w.write_u8(1)?;
                w.write_u32::<LittleEndian>(type_id.len() as u32)?;
                w.write_all(type_id.as_bytes())?;
            }
            None => {
                w.write_u8(0)?;
            }
        }
    }
    Ok(())
}

pub fn serialize_types<W: Write>(w: &mut W, types: &[vm2::Type]) -> std::io::Result<()> {
    w.write_u32::<LittleEndian>(types.len() as u32)?;
    for typ in types {
        w.write_u32::<LittleEndian>(typ.name.len() as u32)?;
        w.write_all(typ.name.as_bytes())?;

        w.write_u32::<LittleEndian>(typ.fields.len() as u32)?;
        for field in &typ.fields {
            w.write_u32::<LittleEndian>(field.name.len() as u32)?;
            w.write_all(field.name.as_bytes())?;

            match &field.kind {
                vm2::TypeFieldKind::Ff => {
                    w.write_u8(0)?;
                }
                vm2::TypeFieldKind::Bus(bus_index) => {
                    w.write_u8(1)?;
                    w.write_u32::<LittleEndian>(*bus_index as u32)?;
                }
            }

            w.write_u32::<LittleEndian>(field.offset as u32)?;
            w.write_u32::<LittleEndian>(field.base_type_size as u32)?;

            w.write_u32::<LittleEndian>(field.dims.len() as u32)?;
            for dim in &field.dims {
                w.write_u32::<LittleEndian>(*dim as u32)?;
            }
        }
    }
    Ok(())
}

pub fn serialize_input_signal_info(
    input_infos: &[vm2::InputInfo],
    types: &[vm2::Type],
) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    serialize_input_infos(&mut buf, input_infos)?;
    serialize_types(&mut buf, types)?;
    Ok(buf)
}

pub fn deserialize_input_infos<R: Read>(r: &mut R) -> std::io::Result<Vec<vm2::InputInfo>> {
    let num_input_infos = r.read_u32::<LittleEndian>()? as usize;
    let mut input_infos = Vec::with_capacity(num_input_infos);

    for _ in 0..num_input_infos {
        let name_len = r.read_u32::<LittleEndian>()? as usize;
        let mut name_bytes = vec![0u8; name_len];
        r.read_exact(&mut name_bytes)?;
        let name = String::from_utf8(name_bytes)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid UTF-8 in input name"))?;

        let offset = r.read_u32::<LittleEndian>()? as usize;

        let num_lengths = r.read_u32::<LittleEndian>()? as usize;
        let mut lengths = Vec::with_capacity(num_lengths);
        for _ in 0..num_lengths {
            lengths.push(r.read_u32::<LittleEndian>()? as usize);
        }

        let has_type_id = r.read_u8()?;
        let type_id = if has_type_id == 1 {
            let type_id_len = r.read_u32::<LittleEndian>()? as usize;
            let mut type_id_bytes = vec![0u8; type_id_len];
            r.read_exact(&mut type_id_bytes)?;
            Some(String::from_utf8(type_id_bytes)
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid UTF-8 in type_id"))?)
        } else {
            None
        };

        input_infos.push(vm2::InputInfo {
            name,
            offset,
            lengths,
            type_id,
        });
    }

    Ok(input_infos)
}

pub fn deserialize_types<R: Read>(r: &mut R) -> std::io::Result<Vec<vm2::Type>> {
    let num_types = r.read_u32::<LittleEndian>()? as usize;
    let mut types = Vec::with_capacity(num_types);

    for _ in 0..num_types {
        let name_len = r.read_u32::<LittleEndian>()? as usize;
        let mut name_bytes = vec![0u8; name_len];
        r.read_exact(&mut name_bytes)?;
        let name = String::from_utf8(name_bytes)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid UTF-8 in type name"))?;

        let num_fields = r.read_u32::<LittleEndian>()? as usize;
        let mut fields = Vec::with_capacity(num_fields);

        for _ in 0..num_fields {
            let field_name_len = r.read_u32::<LittleEndian>()? as usize;
            let mut field_name_bytes = vec![0u8; field_name_len];
            r.read_exact(&mut field_name_bytes)?;
            let field_name = String::from_utf8(field_name_bytes)
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid UTF-8 in field name"))?;

            let kind_tag = r.read_u8()?;
            let kind = match kind_tag {
                0 => vm2::TypeFieldKind::Ff,
                1 => {
                    let bus_index = r.read_u32::<LittleEndian>()? as usize;
                    vm2::TypeFieldKind::Bus(bus_index)
                }
                _ => return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid type field kind")),
            };

            let offset = r.read_u32::<LittleEndian>()? as usize;
            let base_type_size = r.read_u32::<LittleEndian>()? as usize;

            let num_dims = r.read_u32::<LittleEndian>()? as usize;
            let mut dims = Vec::with_capacity(num_dims);
            for _ in 0..num_dims {
                dims.push(r.read_u32::<LittleEndian>()? as usize);
            }

            fields.push(vm2::TypeField {
                name: field_name,
                kind,
                offset,
                base_type_size,
                dims,
            });
        }

        types.push(vm2::Type {
            name,
            fields,
        });
    }

    Ok(types)
}

pub fn deserialize_input_signal_info(data: &[u8]) -> std::io::Result<(Vec<vm2::InputInfo>, Vec<vm2::Type>)> {
    let mut cursor = std::io::Cursor::new(data);
    let input_infos = deserialize_input_infos(&mut cursor)?;
    let types = deserialize_types(&mut cursor)?;
    Ok((input_infos, types))
}

pub fn serialize_witnesscalc_vm2<T: FieldOps>(
    mut w: impl Write, circuit: &vm2::Circuit<T>) -> std::io::Result<()> {

    w.write_all(WITNESSCALC_CVM_MAGIC)?;

    // Write field (prime) - first write the length, then the bytes
    let prime_bytes = circuit.field.prime.to_le_bytes();
    w.write_u8(prime_bytes.len().try_into().map_err(|_| {
        Error::new(
            ErrorKind::InvalidData,
            "Field prime is too large, cannot serialize")
    })?)?;
    w.write_all(&prime_bytes)?;

    // Write main_template_id
    w.write_u32::<LittleEndian>(circuit.main_template_id as u32)?;

    // Write number of templates
    w.write_u32::<LittleEndian>(circuit.templates.len() as u32)?;

    // Write templates
    for template in &circuit.templates {
        // Write template name length and name
        w.write_u32::<LittleEndian>(template.name.len() as u32)?;
        w.write_all(template.name.as_bytes())?;

        // Write code length and code
        w.write_u32::<LittleEndian>(template.code.len() as u32)?;
        w.write_all(&template.code)?;

        // Write template metadata
        w.write_u32::<LittleEndian>(template.signals_num as u32)?;
        w.write_u32::<LittleEndian>(template.number_of_inputs as u32)?;

        // Write components
        w.write_u32::<LittleEndian>(template.components.len() as u32)?;
        for component in &template.components {
            match component {
                Some(idx) => {
                    w.write_u8(1)?; // Has value
                    w.write_u32::<LittleEndian>(*idx as u32)?;
                }
                None => {
                    w.write_u8(0)?; // No value
                }
            }
        }
        
        w.write_u32::<LittleEndian>(template.inputs.len() as u32)?;
        for signal in &template.inputs {
            write_signal(&mut w, signal)?;
        }
        
        w.write_u32::<LittleEndian>(template.outputs.len() as u32)?;
        for signal in &template.outputs {
            write_signal(&mut w, signal)?;
        }

        // Write variable name maps
        write_string_vec(&mut w, &template.ff_variable_names)?;
        write_string_vec(&mut w, &template.i64_variable_names)?;
    }

    // Write number of functions
    w.write_u32::<LittleEndian>(circuit.functions.len() as u32)?;

    // Write functions (same format as templates)
    for function in &circuit.functions {
        w.write_u32::<LittleEndian>(function.name.len() as u32)?;
        w.write_all(function.name.as_bytes())?;

        w.write_u32::<LittleEndian>(function.code.len() as u32)?;
        w.write_all(&function.code)?;

        // Write variable name maps
        write_string_vec(&mut w, &function.ff_variable_names)?;
        write_string_vec(&mut w, &function.i64_variable_names)?;
    }

    // Write witness
    w.write_u32::<LittleEndian>(circuit.witness.len() as u32)?;
    for signal_idx in &circuit.witness {
        w.write_u32::<LittleEndian>(*signal_idx as u32)?;
    }

    // Write signals_num
    w.write_u32::<LittleEndian>(circuit.signals_num as u32)?;

    serialize_input_infos(&mut w, &circuit.input_infos)?;
    serialize_types(&mut w, &circuit.types)?;

    Ok(())
}

pub fn read_witnesscalc_vm2_header(
    mut r: impl Read) -> std::io::Result<BigUint> {

    let mut magic = [0u8; WITNESSCALC_CVM_MAGIC.len()];
    r.read_exact(&mut magic)?;

    if !magic.eq(WITNESSCALC_CVM_MAGIC) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "cvm file does not look like a witnesscalc cvm file"));
    }

    // Read field (prime) - first read the length, then the bytes
    let prime_len = r.read_u8()? as usize;
    let mut prime_bytes = vec![0u8; prime_len];
    r.read_exact(&mut prime_bytes)?;

    Ok(BigUint::from_bytes_le(&prime_bytes))
}

pub fn deserialize_witnesscalc_vm2_body<T: FieldOps>(
    mut r: impl Read, field: Field<T>) -> std::io::Result<vm2::Circuit<T>> {

    // Read main_template_id
    let main_template_id = r.read_u32::<LittleEndian>()? as usize;

    // Read templates
    let num_templates = r.read_u32::<LittleEndian>()? as usize;
    let mut templates = Vec::with_capacity(num_templates);

    for _ in 0..num_templates {
        // Read template name
        let name_len = r.read_u32::<LittleEndian>()? as usize;
        let mut name_bytes = vec![0u8; name_len];
        r.read_exact(&mut name_bytes)?;
        let name = String::from_utf8(name_bytes)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid UTF-8 in template name"))?;

        // Read code
        let code_len = r.read_u32::<LittleEndian>()? as usize;
        let mut code = vec![0u8; code_len];
        r.read_exact(&mut code)?;

        // Read template metadata
        let signals_num = r.read_u32::<LittleEndian>()? as usize;
        let number_of_inputs = r.read_u32::<LittleEndian>()? as usize;

        // Read components
        let num_components = r.read_u32::<LittleEndian>()? as usize;
        let mut components = Vec::with_capacity(num_components);
        for _ in 0..num_components {
            let has_value = r.read_u8()?;
            if has_value == 1 {
                let idx = r.read_u32::<LittleEndian>()? as usize;
                components.push(Some(idx));
            } else {
                components.push(None);
            }
        }

        let num_inputs = r.read_u32::<LittleEndian>()? as usize;
        let mut inputs = Vec::with_capacity(num_inputs);
        for _ in 0..num_inputs {
            inputs.push(read_signal(&mut r)?);
        }
        
        let num_outputs = r.read_u32::<LittleEndian>()? as usize;
        let mut outputs = Vec::with_capacity(num_outputs);
        for _ in 0..num_outputs {
            outputs.push(read_signal(&mut r)?);
        }

        // Read variable name maps
        let ff_variable_names = read_string_vec(&mut r)?;
        let i64_variable_names = read_string_vec(&mut r)?;

        templates.push(vm2::Template {
            name,
            code,
            signals_num,
            number_of_inputs,
            components,
            inputs,
            outputs,
            ff_variable_names,
            i64_variable_names,
        });
    }

    // Read functions
    let num_functions = r.read_u32::<LittleEndian>()? as usize;
    let mut functions = Vec::with_capacity(num_functions);
    let mut function_registry = HashMap::new();

    for idx in 0..num_functions {
        // Read function name
        let name_len = r.read_u32::<LittleEndian>()? as usize;
        let mut name_bytes = vec![0u8; name_len];
        r.read_exact(&mut name_bytes)?;
        let name = String::from_utf8(name_bytes)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid UTF-8 in function name"))?;

        // Add to function registry
        function_registry.insert(name.clone(), idx);

        // Read code
        let code_len = r.read_u32::<LittleEndian>()? as usize;
        let mut code = vec![0u8; code_len];
        r.read_exact(&mut code)?;

        // Read variable name maps
        let ff_variable_names = read_string_vec(&mut r)?;
        let i64_variable_names = read_string_vec(&mut r)?;

        functions.push(vm2::Function {
            name,
            code,
            ff_variable_names,
            i64_variable_names,
        });
    }

    // Read witness
    let num_witness = r.read_u32::<LittleEndian>()? as usize;
    let mut witness = Vec::with_capacity(num_witness);
    for _ in 0..num_witness {
        witness.push(r.read_u32::<LittleEndian>()? as usize);
    }

    // Read signals_num
    let signals_num = r.read_u32::<LittleEndian>()? as usize;

    let input_infos = deserialize_input_infos(&mut r)?;
    let types = deserialize_types(&mut r)?;

    Ok(vm2::Circuit {
        main_template_id,
        templates,
        functions,
        function_registry,
        field,
        witness,
        signals_num,
        input_infos,
        types,
    })
}

#[cfg(test)]
mod tests {
    use num_traits::Num;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::mem::size_of;
    use std::rc::Rc;
    use crate::graph::{Node, NodesInterface, Operation, TresOperation, UnoOperation, VecNodes};
    use byteorder::ByteOrder;
    use crate::vm::{build_component, execute, ComponentTmpl, OpCode};
    use crate::field::{bn254_prime, FieldOperations, U254, U64};
    use crate::InputSignalsInfo;
    use crate::storage::proto_deserializer::{deserialize_witnesscalc_graph_from_bytes, InputInfo};
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
        let prime = U254::from_str_radix(
            "21888242871839275222246405745257275088548364400416034343698204186575808495617",
            10).unwrap();
        let node_storage = VecNodes::new();
        let mut nodes = Nodes::new(
            prime, "bn128", node_storage);
        nodes.push(Node::Input(0));
        let c = (&nodes.ff).parse_str("1").unwrap();
        nodes.const_node_idx_from_value(c);
        nodes.push(Node::UnoOp(UnoOperation::Id, 1));
        nodes.push(Node::Op(Operation::Add, 1, 2));
        nodes.push(Node::TresOp(TresOperation::TernCond, 1, 2, 2));
        let mut nodes_pb = Vec::new();
        let mut buf = Vec::new();
        for i in 0..nodes.len() {
            let n = nodes.to_proto(i).unwrap();
            let node_pb = crate::proto::Node{ node: Some(n) };
            node_pb.encode_length_delimited(&mut buf).unwrap();
            nodes_pb.push(node_pb);
        }
        let mut nodes_got: Vec<crate::proto::Node> = Vec::new();
        let mut reader = std::io::Cursor::new(&buf);
        let mut rw = WriteBackReader::new(&mut reader);
        for _ in 0..nodes.len() {
            nodes_got.push(read_message(&mut rw).unwrap());
        }
        assert_eq!(nodes_pb, nodes_got);
    }

    #[test]
    fn test_write_back_reader() {
        let data = [1u8, 2, 3, 4, 5, 6];
        let mut r = WriteBackReader::new(std::io::Cursor::new(&data));

        let buf = &mut [0u8; 5];
        r.read_exact(buf).unwrap();
        assert_eq!(buf, &[1, 2, 3, 4, 5]);

        // return [4, 5] to reader
        r.write_all(&buf[3..]).unwrap();
        // return [2, 3] to reader
        r.write_all(&buf[1..3]).unwrap();

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
        let prime = <U254 as FieldOps>::from_str("21888242871839275222246405745257275088548364400416034343698204186575808495617").unwrap();
        let node_storage = VecNodes::new();
        let mut nodes = Nodes::new(
            prime, "bn128", node_storage);
        nodes.push(Node::Input(0));
        let c = (&nodes.ff).parse_str("1").unwrap();
        nodes.const_node_idx_from_value(c);
        let c = (&nodes.ff).parse_str("0").unwrap();
        nodes.const_node_idx_from_value(c);
        nodes.push_noopt(Node::UnoOp(UnoOperation::Id, 0));
        nodes.push_noopt(Node::Op(Operation::Mul, 1, 2));
        nodes.push_noopt(Node::TresOp(TresOperation::TernCond, 1, 2, 3));

        let witness_signals = vec![4, 1];

        let input_infos = vec![
            vm2::InputInfo {
                name: "sig1".to_string(),
                offset: 1,
                lengths: vec![3],
                type_id: None,
            },
            vm2::InputInfo {
                name: "sig2".to_string(),
                offset: 4,
                lengths: vec![1],
                type_id: None,
            },
        ];
        let types: Vec<vm2::Type> = vec![];

        // Build expected InputSignalsInfo for verification
        let mut input_signals: InputSignalsInfo = HashMap::new();
        input_signals.insert("sig1".to_string(), (1, 3));
        input_signals.insert("sig2".to_string(), (4, 1));

        let mut tmp = Vec::new();
        serialize_witnesscalc_graph(&mut tmp, &nodes, &witness_signals, &input_infos, &types).unwrap();

        let (nodes_res, witness_signals_res, inputs_info_res) =
            deserialize_witnesscalc_graph_from_bytes(&tmp).unwrap();
        match nodes_res.as_any().downcast_ref::<Nodes<U254, VecNodes>>() {
            Some(t) => {
                assert_eq!(&nodes, t);
            },
            None => panic!(),
        }

        assert_eq!(witness_signals, witness_signals_res);
        let want_inputs_info = InputInfo::V2 {
            input_info: input_infos.clone(),
            types: types.clone(),
        };
        assert_eq!(want_inputs_info, inputs_info_res);

        let metadata_start = LittleEndian::read_u64(&tmp[tmp.len() - 8..]);

        let mt_reader = std::io::Cursor::new(&tmp[metadata_start as usize..]);
        let mut rw = WriteBackReader::new(mt_reader);
        let metadata: crate::proto::GraphMetadata = read_message(&mut rw).unwrap();

        let prime = nodes.prime();
        let prime_bytes: Vec<u8> = <U254 as FieldOps>::to_le_bytes(&prime);
        let expected_blob = serialize_input_signal_info(&input_infos, &types).unwrap();
        let metadata_want = crate::proto::GraphMetadata {
            witness_signals: vec![4, 1],
            inputs: HashMap::new(),
            prime: Some(crate::proto::BigUInt {
                value_le: prime_bytes,
            }),
            prime_str: nodes.prime_str(),
            input_signal_info: expected_blob,
        };

        assert_eq!(metadata, metadata_want);
    }

    #[test]
    fn test_serialization() {

        let mut buf: Vec<u8> = Vec::new();

        let io_map = TemplateInstanceIOMap::new();

        let cs = CompiledCircuit {
            main_template_id: 0,
            templates: vec![
                Template{
                    name: "tmpl1".to_string(),
                    code: vec![OpCode::NoOp as u8],
                    line_numbers: vec![10],
                    components: vec![
                        ComponentTmpl{
                            symbol: "sym1".to_string(),
                            sub_cmp_idx: 0,
                            number_of_cmp: 1,
                            name_subcomponent: "sub1".to_string(),
                            signal_offset: 3,
                            signal_offset_jump: 4,
                            template_id: 1,
                            has_inputs: true,
                        },
                    ],
                    var_stack_depth: 4,
                    number_of_inputs: 5,
                },
                Template{
                    name: "tmpl2".to_string(),
                    code: vec![OpCode::NoOp as u8],
                    line_numbers: vec![100],
                    components: vec![],
                    var_stack_depth: 40,
                    number_of_inputs: 50,
                },
            ],
            functions: vec![
                Function{
                    name: "func1".to_string(),
                    symbol: "sym1".to_string(),
                    code: vec![OpCode::FnReturn as u8, 0, 0, 0, 0],
                    line_numbers: vec![10, 20, 30, 40, 50],
                },
            ],
            signals_num: 20,
            constants: vec![U256::from(100500)],
            inputs: vec![("inp1".to_string(), 5, 10)],
            witness_signals: vec![1, 2, 3],
            io_map,
        };
        serialize_witnesscalc_vm(&mut buf, &cs).unwrap();

        let cs2 = deserialize_witnesscalc_vm(&buf[..]).unwrap();

        // println!("{:?}", cs);
        // println!("{:?}", cs2);

        assert_eq!(format!("{:?}", cs), format!("{:?}", cs2));
    }

    fn minimal_legacy_circuit(
        code: Vec<u8>, components: Vec<ComponentTmpl>,
        functions: Vec<Function>) -> CompiledCircuit {

        CompiledCircuit {
            main_template_id: 0,
            templates: vec![Template {
                name: "main".to_string(),
                code,
                line_numbers: vec![],
                components,
                var_stack_depth: 0,
                number_of_inputs: 0,
            }],
            functions,
            signals_num: 1,
            constants: vec![],
            inputs: vec![],
            witness_signals: vec![],
            io_map: TemplateInstanceIOMap::new(),
        }
    }

    fn mapped_get_subsignal_code(cmp_idx: u32) -> Vec<u8> {
        let mut code = vec![OpCode::Push4 as u8];
        code.extend(cmp_idx.to_le_bytes());
        code.push(OpCode::GetSubSignal as u8);
        code.extend(1_u32.to_le_bytes());
        code.push(0b1000_0000);
        code.extend(0_u32.to_le_bytes());
        code.extend(0_u32.to_le_bytes());
        code
    }

    fn heterogeneous_mapped_legacy_circuit(cmp_idx: u32) -> CompiledCircuit {
        let components = vec![
            ComponentTmpl {
                symbol: "a".to_string(),
                sub_cmp_idx: 0,
                number_of_cmp: 1,
                name_subcomponent: "a".to_string(),
                signal_offset: 0,
                signal_offset_jump: 0,
                template_id: 1,
                has_inputs: false,
            },
            ComponentTmpl {
                symbol: "b".to_string(),
                sub_cmp_idx: 1,
                number_of_cmp: 1,
                name_subcomponent: "b".to_string(),
                signal_offset: 1,
                signal_offset_jump: 0,
                template_id: 2,
                has_inputs: false,
            },
        ];
        let mut circuit = minimal_legacy_circuit(
            mapped_get_subsignal_code(cmp_idx),
            components,
            vec![]);
        circuit.templates.push(Template {
            name: "child_a".to_string(),
            code: vec![OpCode::NoOp as u8],
            line_numbers: vec![],
            components: vec![],
            var_stack_depth: 0,
            number_of_inputs: 0,
        });
        circuit.templates.push(Template {
            name: "child_b".to_string(),
            code: vec![OpCode::NoOp as u8],
            line_numbers: vec![],
            components: vec![],
            var_stack_depth: 0,
            number_of_inputs: 0,
        });
        circuit.signals_num = 3;
        circuit.io_map.insert(1, vec![IODef {
            code: 0,
            offset: 0,
            lengths: vec![],
        }]);
        circuit
    }

    fn deserialize_legacy_err(circuit: &CompiledCircuit) -> String {
        let mut artifact = Vec::new();
        serialize_witnesscalc_vm(&mut artifact, circuit).unwrap();
        deserialize_witnesscalc_vm(&artifact[..])
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn deserialize_vm_rejects_invalid_legacy_opcode() {
        let circuit = minimal_legacy_circuit(vec![12], vec![], vec![]);
        let err = deserialize_legacy_err(&circuit);
        assert!(err.contains("invalid opcode byte"), "got: {err}");
    }

    #[test]
    fn deserialize_vm_rejects_truncated_legacy_immediate() {
        let circuit = minimal_legacy_circuit(
            vec![OpCode::Push4 as u8],
            vec![],
            vec![]);
        let err = deserialize_legacy_err(&circuit);
        assert!(err.contains("truncated operand"), "got: {err}");
    }

    #[test]
    fn deserialize_vm_rejects_legacy_cmp_call_out_of_range() {
        let mut code = vec![OpCode::CmpCall as u8];
        code.extend(0_u32.to_le_bytes());
        let circuit = minimal_legacy_circuit(code, vec![], vec![]);
        let err = deserialize_legacy_err(&circuit);
        assert!(err.contains("CmpCall target"), "got: {err}");
    }

    #[test]
    fn deserialize_vm_rejects_legacy_function_call_out_of_range() {
        let mut code = vec![OpCode::FnCall as u8];
        code.extend(0_u32.to_le_bytes());
        code.extend(0_u32.to_le_bytes());
        code.extend(0_u32.to_le_bytes());
        let circuit = minimal_legacy_circuit(code, vec![], vec![]);
        let err = deserialize_legacy_err(&circuit);
        assert!(err.contains("FnCall target"), "got: {err}");
    }

    #[test]
    fn deserialize_vm_rejects_legacy_constant_out_of_range() {
        let mut code = vec![OpCode::GetConstant8 as u8];
        code.extend(0_usize.to_le_bytes());
        let circuit = minimal_legacy_circuit(code, vec![], vec![]);
        let err = deserialize_legacy_err(&circuit);
        assert!(err.contains("GetConstant8 target"), "got: {err}");
    }

    #[test]
    fn deserialize_vm_rejects_legacy_jump_into_immediate() {
        let mut code = vec![OpCode::Push8 as u8];
        code.extend(0_usize.to_le_bytes());
        code.push(OpCode::Jump as u8);
        let offset = 1_i32 - (code.len() as i32 + size_of::<i32>() as i32);
        code.extend(offset.to_le_bytes());

        let circuit = minimal_legacy_circuit(code, vec![], vec![]);
        let err = deserialize_legacy_err(&circuit);
        assert!(err.contains("targeting non-instruction byte"), "got: {err}");
    }

    #[test]
    fn deserialize_vm_rejects_legacy_stack_underflow() {
        let circuit = minimal_legacy_circuit(vec![OpCode::OpMul as u8], vec![], vec![]);
        let err = deserialize_legacy_err(&circuit);
        assert!(err.contains("stack values available"), "got: {err}");
    }

    #[test]
    fn deserialize_vm_rejects_legacy_function_fallthrough() {
        let function = Function {
            name: "f".to_string(),
            symbol: "f".to_string(),
            code: vec![OpCode::NoOp as u8],
            line_numbers: vec![],
        };
        let circuit = minimal_legacy_circuit(vec![OpCode::NoOp as u8], vec![], vec![function]);
        let err = deserialize_legacy_err(&circuit);
        assert!(err.contains("falls through without FnReturn"), "got: {err}");
    }

    #[test]
    fn deserialize_vm_rejects_legacy_branch_stack_underflow() {
        let mut code = vec![OpCode::Push8 as u8];
        code.extend(0_usize.to_le_bytes());
        code.push(OpCode::JumpIfFalse as u8);
        code.extend(9_i32.to_le_bytes());
        code.push(OpCode::Push8 as u8);
        code.extend(1_usize.to_le_bytes());
        code.push(OpCode::Push8 as u8);
        code.extend(3_usize.to_le_bytes());
        code.push(OpCode::OpMul as u8);

        let circuit = minimal_legacy_circuit(code, vec![], vec![]);
        let err = deserialize_legacy_err(&circuit);
        assert!(err.contains("inconsistent stack depths"), "got: {err}");
    }

    #[test]
    fn deserialize_vm_rejects_legacy_function_return_mismatch() {
        let mut code = vec![OpCode::FnCall as u8];
        code.extend(0_u32.to_le_bytes());
        code.extend(0_u32.to_le_bytes());
        code.extend(1_u32.to_le_bytes());

        let function = Function {
            name: "f".to_string(),
            symbol: "f".to_string(),
            code: vec![OpCode::FnReturn as u8, 0, 0, 0, 0],
            line_numbers: vec![],
        };
        let circuit = minimal_legacy_circuit(code, vec![], vec![function]);
        let err = deserialize_legacy_err(&circuit);
        assert!(err.contains("FnCall return count"), "got: {err}");
    }

    #[test]
    fn deserialize_vm_rejects_legacy_mapped_signal_without_io_map() {
        let mut code = vec![OpCode::Push4 as u8];
        code.extend(0_u32.to_le_bytes());
        code.push(OpCode::GetSubSignal as u8);
        code.extend(1_u32.to_le_bytes());
        code.push(0b1000_0000);
        code.extend(0_u32.to_le_bytes());
        code.extend(0_u32.to_le_bytes());

        let circuit = minimal_legacy_circuit(code, vec![], vec![]);
        let err = deserialize_legacy_err(&circuit);
        assert!(err.contains("mapped signal access"), "got: {err}");
    }

    #[test]
    fn deserialize_vm_accepts_legacy_heterogeneous_mapped_signal_candidate() {
        let circuit = heterogeneous_mapped_legacy_circuit(0);
        let mut artifact = Vec::new();
        serialize_witnesscalc_vm(&mut artifact, &circuit).unwrap();

        deserialize_witnesscalc_vm(&artifact[..]).unwrap();
    }

    #[test]
    fn execute_vm_rejects_legacy_mapped_signal_without_selected_io_map() {
        let circuit = heterogeneous_mapped_legacy_circuit(1);
        let mut artifact = Vec::new();
        serialize_witnesscalc_vm(&mut artifact, &circuit).unwrap();
        let circuit = deserialize_witnesscalc_vm(&artifact[..]).unwrap();
        let component = Rc::new(RefCell::new(build_component(
            &circuit.templates,
            circuit.main_template_id,
            1,
        )));
        let mut signals = vec![None; circuit.signals_num];

        let err = execute(
            component,
            &circuit.templates,
            &circuit.functions,
            &circuit.constants,
            &mut signals,
            &circuit.io_map,
            None,
        )
        .unwrap_err();

        assert!(err.contains("template not found in io_map: 2"), "got: {err}");
    }

    #[test]
    fn deserialize_vm_rejects_legacy_witness_out_of_range() {
        let mut circuit = minimal_legacy_circuit(vec![OpCode::NoOp as u8], vec![], vec![]);
        circuit.witness_signals = vec![1];
        let err = deserialize_legacy_err(&circuit);
        assert!(err.contains("witness signal"), "got: {err}");
    }

    #[test]
    fn deserialize_vm_rejects_legacy_input_span_out_of_range() {
        let mut circuit = minimal_legacy_circuit(vec![OpCode::NoOp as u8], vec![], vec![]);
        circuit.inputs = vec![("a".to_string(), 1, 1)];
        let err = deserialize_legacy_err(&circuit);
        assert!(err.contains("input signal a span"), "got: {err}");
    }

    #[test]
    fn test_vm2_serialization() {
        let prime = BigUint::from_str_radix(
            "21888242871839275222246405745257275088548364400416034343698204186575808495617",
            10).unwrap();
        let ff = Field::new(bn254_prime);

        let circuit = vm2::Circuit {
            main_template_id: 0,
            templates: vec![
                vm2::Template {
                    name: "main".to_string(),
                    code: vec![1, 2, 3, 4, 5],
                    signals_num: 5,
                    number_of_inputs: 2,
                    components: vec![Some(1), None, Some(2)],
                    inputs: vec![vm2::Signal::Ff(vec![]), vm2::Signal::Bus(0, vec![2])],
                    outputs: vec![vm2::Signal::Ff(vec![3])],
                    ff_variable_names: vec![],
                    i64_variable_names: vec![],
                },
                vm2::Template {
                    name: "sub_template".to_string(),
                    code: vec![10, 20, 30],
                    signals_num: 3,
                    number_of_inputs: 1,
                    components: vec![],
                    inputs: vec![vm2::Signal::Ff(vec![])],
                    outputs: vec![vm2::Signal::Ff(vec![])],
                    ff_variable_names: vec![],
                    i64_variable_names: vec![],
                },
            ],
            functions: vec![
                vm2::Function {
                    name: "add".to_string(),
                    code: vec![100, 101, 102],
                    ff_variable_names: vec![],
                    i64_variable_names: vec![],
                },
                vm2::Function {
                    name: "mul".to_string(),
                    code: vec![200, 201],
                    ff_variable_names: vec![],
                    i64_variable_names: vec![],
                },
            ],
            function_registry: HashMap::from([
                ("add".to_string(), 0),
                ("mul".to_string(), 1),
            ]),
            field: ff.clone(),
            witness: vec![0, 1, 2, 3],
            signals_num: 10,
            input_infos: vec![
                vm2::InputInfo {
                    name: "input1".to_string(),
                    offset: 1,
                    lengths: vec![1],
                    type_id: None,
                },
                vm2::InputInfo {
                    name: "input2".to_string(),
                    offset: 2,
                    lengths: vec![2, 3],
                    type_id: Some("MyType".to_string()),
                },
            ],
            types: vec![
                vm2::Type {
                    name: "MyType".to_string(),
                    fields: vec![
                        vm2::TypeField {
                            name: "field1".to_string(),
                            kind: vm2::TypeFieldKind::Ff,
                            offset: 0,
                            base_type_size: 1,
                            dims: vec![],
                        },
                        vm2::TypeField {
                            name: "field2".to_string(),
                            kind: vm2::TypeFieldKind::Bus(0), // Index 0 in types vector
                            offset: 1,
                            base_type_size: 5,
                            dims: vec![2, 3],
                        },
                    ],
                },
            ],
        };
        
        // Serialize
        let mut buf = Vec::new();
        serialize_witnesscalc_vm2(&mut buf, &circuit).unwrap();
        
        // Read header
        let mut reader = std::io::Cursor::new(&buf);
        let prime_read = read_witnesscalc_vm2_header(&mut reader).unwrap();
        assert_eq!(prime_read, prime);
        
        // Deserialize
        let circuit2 = deserialize_witnesscalc_vm2_body(&mut reader, ff).unwrap();
        
        // Verify all fields
        assert_eq!(circuit.main_template_id, circuit2.main_template_id);
        assert_eq!(circuit.signals_num, circuit2.signals_num);
        assert_eq!(circuit.witness, circuit2.witness);
        assert_eq!(circuit.templates.len(), circuit2.templates.len());
        assert_eq!(circuit.functions.len(), circuit2.functions.len());
        assert_eq!(circuit.function_registry, circuit2.function_registry);
        assert_eq!(circuit.input_infos.len(), circuit2.input_infos.len());
        assert_eq!(circuit.types.len(), circuit2.types.len());
        
        // Verify templates
        for (t1, t2) in circuit.templates.iter().zip(circuit2.templates.iter()) {
            assert_eq!(t1.name, t2.name);
            assert_eq!(t1.code, t2.code);
            assert_eq!(t1.ff_variable_names, t2.ff_variable_names);
            assert_eq!(t1.i64_variable_names, t2.i64_variable_names);
            assert_eq!(t1.signals_num, t2.signals_num);
            assert_eq!(t1.number_of_inputs, t2.number_of_inputs);
            assert_eq!(t1.components, t2.components);
        }

        // Verify functions
        for (f1, f2) in circuit.functions.iter().zip(circuit2.functions.iter()) {
            assert_eq!(f1.name, f2.name);
            assert_eq!(f1.code, f2.code);
            assert_eq!(f1.ff_variable_names, f2.ff_variable_names);
            assert_eq!(f1.i64_variable_names, f2.i64_variable_names);
        }
        
        // Verify input_infos
        for (i1, i2) in circuit.input_infos.iter().zip(circuit2.input_infos.iter()) {
            assert_eq!(i1.name, i2.name);
            assert_eq!(i1.offset, i2.offset);
            assert_eq!(i1.lengths, i2.lengths);
            assert_eq!(i1.type_id, i2.type_id);
        }
        
        // Verify types
        for (t1, t2) in circuit.types.iter().zip(circuit2.types.iter()) {
            assert_eq!(t1.name, t2.name);
            assert_eq!(t1.fields.len(), t2.fields.len());
            
            for (f1, f2) in t1.fields.iter().zip(t2.fields.iter()) {
                assert_eq!(f1.name, f2.name);
                match (&f1.kind, &f2.kind) {
                    (vm2::TypeFieldKind::Ff, vm2::TypeFieldKind::Ff) => {},
                    (vm2::TypeFieldKind::Bus(b1), vm2::TypeFieldKind::Bus(b2)) => {
                        assert_eq!(b1, b2);
                    },
                    _ => panic!("Type field kind mismatch"),
                }
                assert_eq!(f1.offset, f2.offset);
                assert_eq!(f1.base_type_size, f2.base_type_size);
                assert_eq!(f1.dims, f2.dims);
            }
        }
    }

    #[test]
    fn test_vm2_serialization_empty() {
        let prime = BigUint::from(17u64);
        let ff = Field::new(U64::new(17u64));
        
        let circuit = vm2::Circuit {
            main_template_id: 5,
            templates: vec![],
            functions: vec![],
            function_registry: HashMap::new(),
            field: ff.clone(),
            witness: vec![],
            signals_num: 0,
            input_infos: vec![],
            types: vec![],
        };
        
        // Serialize
        let mut buf = Vec::new();
        serialize_witnesscalc_vm2(&mut buf, &circuit).unwrap();
        
        // Read header
        let mut reader = std::io::Cursor::new(&buf);
        let prime_read = read_witnesscalc_vm2_header(&mut reader).unwrap();
        assert_eq!(prime_read, prime);
        
        // Deserialize
        let circuit2 = deserialize_witnesscalc_vm2_body(&mut reader, ff).unwrap();
        
        assert_eq!(circuit.main_template_id, circuit2.main_template_id);
        assert_eq!(circuit.templates.len(), 0);
        assert_eq!(circuit2.templates.len(), 0);
        assert_eq!(circuit.functions.len(), 0);
        assert_eq!(circuit2.functions.len(), 0);
        assert_eq!(circuit.function_registry.len(), 0);
        assert_eq!(circuit2.function_registry.len(), 0);
        assert_eq!(reader.position(), buf.len() as u64);
    }

    #[test]
    fn test_vm2_header_invalid_magic() {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"invalid.magic");
        
        let mut reader = std::io::Cursor::new(&buf);
        let result = read_witnesscalc_vm2_header(&mut reader);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cvm file does not look like a witnesscalc cvm file"));
    }

    #[test]
    fn test_vm2_serialization_utf8_names() {
        let prime = BigUint::from(101u64);
        let ff = Field::new(U64::new(101u64));

        let circuit = vm2::Circuit {
            main_template_id: 0,
            templates: vec![
                vm2::Template {
                    name: "模板名称".to_string(), // Chinese characters
                    code: vec![1, 2, 3],
                    signals_num: 1,
                    number_of_inputs: 1,
                    components: vec![],
                    inputs: vec![vm2::Signal::Ff(vec![])],
                    outputs: vec![vm2::Signal::Ff(vec![])],
                    ff_variable_names: vec![],
                    i64_variable_names: vec![],
                },
            ],
            functions: vec![
                vm2::Function {
                    name: "función_española".to_string(), // Spanish characters
                    code: vec![4, 5, 6],
                    ff_variable_names: vec![],
                    i64_variable_names: vec![],
                },
            ],
            function_registry: HashMap::from([
                ("función_española".to_string(), 0),
            ]),
            field: ff.clone(),
            witness: vec![],
            signals_num: 1,
            input_infos: vec![
                vm2::InputInfo {
                    name: "вход".to_string(), // Russian characters
                    offset: 0,
                    lengths: vec![1],
                    type_id: Some("τύπος".to_string()), // Greek characters
                },
            ],
            types: vec![
                vm2::Type {
                    name: "τύπος".to_string(),
                    fields: vec![
                        vm2::TypeField {
                            name: "フィールド".to_string(), // Japanese characters
                            kind: vm2::TypeFieldKind::Bus(0), // Index 0 in types vector
                            offset: 0,
                            base_type_size: 1,
                            dims: vec![],
                        },
                    ],
                },
            ],
        };
        
        // Serialize
        let mut buf = Vec::new();
        serialize_witnesscalc_vm2(&mut buf, &circuit).unwrap();
        
        // Deserialize
        let mut reader = std::io::Cursor::new(&buf);
        let prime_read = read_witnesscalc_vm2_header(&mut reader).unwrap();
        assert_eq!(prime_read, prime);

        let circuit2 = deserialize_witnesscalc_vm2_body(&mut reader, ff).unwrap();

        // Verify UTF-8 names are preserved
        assert_eq!(circuit.templates[0].name, circuit2.templates[0].name);
        assert_eq!(circuit.functions[0].name, circuit2.functions[0].name);
        assert_eq!(circuit.input_infos[0].name, circuit2.input_infos[0].name);
        assert_eq!(circuit.input_infos[0].type_id, circuit2.input_infos[0].type_id);
        assert_eq!(circuit.types[0].name, circuit2.types[0].name);
        assert_eq!(circuit.types[0].fields[0].name, circuit2.types[0].fields[0].name);
        
        if let vm2::TypeFieldKind::Bus(bus1) = &circuit.types[0].fields[0].kind {
            if let vm2::TypeFieldKind::Bus(bus2) = &circuit2.types[0].fields[0].kind {
                assert_eq!(bus1, bus2);
            } else {
                panic!("Expected Bus type");
            }
        }
    }

    #[test]
    fn test_vm2_serialization_template_variable_names() {
        let prime = BigUint::from(101u64);
        let ff = Field::new(U64::new(101u64));

        // Create template with populated variable name vectors
        let ff_variable_names = vec![
            "signal_a".to_string(),
            "signal_b".to_string(),
            "temp_result".to_string(),
        ];

        let i64_variable_names = vec![
            "counter".to_string(),
            "loop_idx".to_string(),
        ];

        let circuit = vm2::Circuit {
            main_template_id: 0,
            templates: vec![
                vm2::Template {
                    name: "TestTemplate".to_string(),
                    code: vec![1, 2, 3],
                    signals_num: 5,
                    number_of_inputs: 1,
                    components: vec![],
                    inputs: vec![vm2::Signal::Ff(vec![])],
                    outputs: vec![vm2::Signal::Ff(vec![])],
                    ff_variable_names: ff_variable_names.clone(),
                    i64_variable_names: i64_variable_names.clone(),
                },
            ],
            functions: vec![],
            function_registry: HashMap::new(),
            field: ff.clone(),
            witness: vec![],
            signals_num: 5,
            input_infos: vec![],
            types: vec![],
        };

        // Serialize
        let mut buf = Vec::new();
        serialize_witnesscalc_vm2(&mut buf, &circuit).unwrap();

        // Deserialize
        let mut reader = std::io::Cursor::new(&buf);
        let prime_read = read_witnesscalc_vm2_header(&mut reader).unwrap();
        assert_eq!(prime_read, prime);

        let circuit2 = deserialize_witnesscalc_vm2_body(&mut reader, ff).unwrap();

        // Verify variable names are preserved
        assert_eq!(circuit.templates[0].ff_variable_names, circuit2.templates[0].ff_variable_names,
            "ff_variable_names should be preserved during serialization");
        assert_eq!(circuit.templates[0].i64_variable_names, circuit2.templates[0].i64_variable_names,
            "i64_variable_names should be preserved during serialization");
    }

    #[test]
    fn test_vm2_serialization_function_variable_names() {
        let prime = BigUint::from(101u64);
        let ff = Field::new(U64::new(101u64));

        // Create function with populated variable name vectors
        let ff_variable_names = vec![
            "param_x".to_string(),
            "param_y".to_string(),
            "result".to_string(),
        ];

        let i64_variable_names = vec![
            "iteration".to_string(),
        ];

        let circuit = vm2::Circuit {
            main_template_id: 0,
            templates: vec![],
            functions: vec![
                vm2::Function {
                    name: "TestFunction".to_string(),
                    code: vec![10, 20, 30],
                    ff_variable_names: ff_variable_names.clone(),
                    i64_variable_names: i64_variable_names.clone(),
                },
            ],
            function_registry: HashMap::from([
                ("TestFunction".to_string(), 0),
            ]),
            field: ff.clone(),
            witness: vec![],
            signals_num: 0,
            input_infos: vec![],
            types: vec![],
        };

        // Serialize
        let mut buf = Vec::new();
        serialize_witnesscalc_vm2(&mut buf, &circuit).unwrap();

        // Deserialize
        let mut reader = std::io::Cursor::new(&buf);
        let prime_read = read_witnesscalc_vm2_header(&mut reader).unwrap();
        assert_eq!(prime_read, prime);

        let circuit2 = deserialize_witnesscalc_vm2_body(&mut reader, ff).unwrap();

        // Verify variable names are preserved
        assert_eq!(circuit.functions[0].ff_variable_names, circuit2.functions[0].ff_variable_names,
            "ff_variable_names should be preserved during serialization");
        assert_eq!(circuit.functions[0].i64_variable_names, circuit2.functions[0].i64_variable_names,
            "i64_variable_names should be preserved during serialization");
    }
}
