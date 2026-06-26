use std::any::Any;
use std::cmp::Ordering;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::error::Error;
use std::fmt::Debug;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::Instant;
use ark_bn254::Fr;
use ark_ff::{BigInt, One, PrimeField, Zero};
use crate::field::{bn254_prime, Field, FieldOperations, FieldOps, M, U254};
use rand::{RngCore};
use ruint::aliases::U256;
use serde::{Deserialize, Serialize};
use memmap2::MmapMut;
use rand::prelude::ThreadRng;
use ruint::uint;
use tempfile::NamedTempFile;
use crate::progress_bar;

#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Operation {
    Mul,
    Div,
    Add,
    Sub,
    Pow,
    Idiv,
    Mod,
    Eq,
    Neq,
    Lt,
    Gt,
    Leq,
    Geq,
    Land,
    Lor,
    Shl,
    Shr,
    Bor,
    Band,
    Bxor,
}

impl Operation {
    pub fn eval(&self, a: U256, b: U256) -> U256 {
        use Operation::*;
        match self {
            Mul => a.mul_mod(b, M),
            Div => {
                if b == U256::ZERO {
                    // as we are simulating a circuit execution with signals
                    // values all equal to 0, just return 0 here in case of
                    // division by zero
                    U256::ZERO
                } else {
                    a.mul_mod(b.inv_mod(M).unwrap(), M)
                }
            },
            Add => a.add_mod(b, M),
            Sub => a.add_mod(M - b, M),
            Pow => a.pow_mod(b, M),
            Mod => {
                if b == U256::ZERO {
                    // Keep witness evaluation total, matching the zero-divisor convention
                    // used by Div/Idiv.
                    U256::ZERO
                } else {
                    a.div_rem(b).1
                }
            },
            Eq => U256::from(a == b),
            Neq => U256::from(a != b),
            Lt => u_lt(&a, &b),
            Gt => u_gt(&a, &b),
            Leq => u_lte(&a, &b),
            Geq => u_gte(&a, &b),
            Land => U256::from(a != U256::ZERO && b != U256::ZERO),
            Lor => U256::from(a != U256::ZERO || b != U256::ZERO),
            Shl => compute_shl_uint(a, b),
            Shr => compute_shr_uint(a, b),
            // TODO test with conner case when it is possible to get the number
            //      bigger then modulus
            Bor => a | b,
            Band => a & b,
            // TODO test with conner case when it is possible to get the number
            //      bigger then modulus
            Bxor => a ^ b,
            Idiv => if b == U256::ZERO { U256::ZERO } else { a / b },
        }
    }
}

impl From<&Operation> for crate::proto::DuoOp {
    fn from(v: &Operation) -> Self {
        match v {
            Operation::Mul => crate::proto::DuoOp::Mul,
            Operation::Div => crate::proto::DuoOp::Div,
            Operation::Add => crate::proto::DuoOp::Add,
            Operation::Sub => crate::proto::DuoOp::Sub,
            Operation::Pow => crate::proto::DuoOp::Pow,
            Operation::Idiv => crate::proto::DuoOp::Idiv,
            Operation::Mod => crate::proto::DuoOp::Mod,
            Operation::Eq => crate::proto::DuoOp::Eq,
            Operation::Neq => crate::proto::DuoOp::Neq,
            Operation::Lt => crate::proto::DuoOp::Lt,
            Operation::Gt => crate::proto::DuoOp::Gt,
            Operation::Leq => crate::proto::DuoOp::Leq,
            Operation::Geq => crate::proto::DuoOp::Geq,
            Operation::Land => crate::proto::DuoOp::Land,
            Operation::Lor => crate::proto::DuoOp::Lor,
            Operation::Shl => crate::proto::DuoOp::Shl,
            Operation::Shr => crate::proto::DuoOp::Shr,
            Operation::Bor => crate::proto::DuoOp::Bor,
            Operation::Band => crate::proto::DuoOp::Band,
            Operation::Bxor => crate::proto::DuoOp::Bxor,
        }
    }
}

impl TryFrom<u8> for Operation {
    type Error = String;

    fn try_from(op: u8) -> Result<Self, Self::Error> {
        match op {
            0 => Ok(Operation::Mul),
            1 => Ok(Operation::Div),
            2 => Ok(Operation::Add),
            3 => Ok(Operation::Sub),
            4 => Ok(Operation::Pow),
            5 => Ok(Operation::Idiv),
            6 => Ok(Operation::Mod),
            7 => Ok(Operation::Eq),
            8 => Ok(Operation::Neq),
            9 => Ok(Operation::Lt),
            10 => Ok(Operation::Gt),
            11 => Ok(Operation::Leq),
            12 => Ok(Operation::Geq),
            13 => Ok(Operation::Land),
            14 => Ok(Operation::Lor),
            15 => Ok(Operation::Shl),
            16 => Ok(Operation::Shr),
            17 => Ok(Operation::Bor),
            18 => Ok(Operation::Band),
            19 => Ok(Operation::Bxor),
            _ => Err(format!("Invalid operation: {}", op)),
        }
    }
}

impl From<&Operation> for u8 {
    fn from(val: &Operation) -> Self {
        match val {
            Operation::Mul => 0,
            Operation::Div => 1,
            Operation::Add => 2,
            Operation::Sub => 3,
            Operation::Pow => 4,
            Operation::Idiv => 5,
            Operation::Mod => 6,
            Operation::Eq => 7,
            Operation::Neq => 8,
            Operation::Lt => 9,
            Operation::Gt => 10,
            Operation::Leq => 11,
            Operation::Geq => 12,
            Operation::Land => 13,
            Operation::Lor => 14,
            Operation::Shl => 15,
            Operation::Shr => 16,
            Operation::Bor => 17,
            Operation::Band => 18,
            Operation::Bxor => 19,
        }
    }
}

#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy, Serialize, Deserialize)]
pub enum UnoOperation {
    Neg,
    Id, // identity - just return self
    Lnot,
    Bnot,
    Sqrt,
}

impl UnoOperation {
    pub fn eval(&self, a: U256) -> U256 {
        match self {
            UnoOperation::Neg => if a == U256::ZERO { U256::ZERO } else { M - a },
            UnoOperation::Id => a,
            UnoOperation::Lnot => if a == U256::ZERO {
                uint!(1_U256)
            } else {
                U256::ZERO
            },
            UnoOperation::Bnot => {
                let a = !a;
                let mask = !U256::ZERO >> M.leading_zeros();
                let a = a & mask;
                if a >= M { a - M } else { a }
            },
            UnoOperation::Sqrt => sqrt_mod_prime(a),
        }
    }
}

fn sqrt_mod_prime(a: U256) -> U256 {
    if a == U256::ZERO {
        return U256::ZERO;
    }
    let one = U256::from(1u64);
    let zero = U256::ZERO;
    let mut q = M - one;
    let mut s: u32 = 0;
    while q & one == zero {
        q >>= 1;
        s += 1;
    }

    let legendre = a.pow_mod((M - one) >> 1, M);
    if legendre != one {
        return zero;
    }

    if s == 1 {
        let exp = (q + one) >> 1;
        let mut r = a.pow_mod(exp, M);
        if r > (M >> 1) {
            r = M - r;
        }
        return r;
    }

    let mut z = U256::from(2u64);
    while z.pow_mod((M - one) >> 1, M) == one {
        z += one;
    }

    let mut m = s;
    let mut c = z.pow_mod(q, M);
    let mut t = a.pow_mod(q, M);
    let mut r = a.pow_mod((q + one) >> 1, M);

    while t != one {
        let mut t2i = t;
        let mut i: u32 = 0;
        while t2i != one {
            t2i = t2i.mul_mod(t2i, M);
            i += 1;
            if i == m {
                return zero;
            }
        }

        let mut b = c;
        for _ in 0..(m - i - 1) {
            b = b.mul_mod(b, M);
        }
        c = b.mul_mod(b, M);
        t = t.mul_mod(c, M);
        r = r.mul_mod(b, M);
        m = i;
    }

    if r > (M >> 1) {
        M - r
    } else {
        r
    }
}

impl From<&UnoOperation> for crate::proto::UnoOp {
    fn from(v: &UnoOperation) -> Self {
        match v {
            UnoOperation::Neg => crate::proto::UnoOp::Neg,
            UnoOperation::Id => crate::proto::UnoOp::Id,
            UnoOperation::Lnot => crate::proto::UnoOp::Lnot,
            UnoOperation::Bnot => crate::proto::UnoOp::Bnot,
            UnoOperation::Sqrt => crate::proto::UnoOp::Sqrt,
        }
    }
}

impl TryFrom<u8> for UnoOperation {
    type Error = String;
    fn try_from(op: u8) -> Result<Self, Self::Error> {
        match op {
            0 => Ok(UnoOperation::Neg),
            1 => Ok(UnoOperation::Id),
            2 => Ok(UnoOperation::Lnot),
            3 => Ok(UnoOperation::Bnot),
            4 => Ok(UnoOperation::Sqrt),
            _ => Err(format!("Invalid unary operation: {}", op)),
        }
    }
}

impl From<&UnoOperation> for u8 {
    fn from(val: &UnoOperation) -> Self {
        match val {
            UnoOperation::Neg => 0,
            UnoOperation::Id => 1,
            UnoOperation::Lnot => 2,
            UnoOperation::Bnot => 3,
            UnoOperation::Sqrt => 4,
        }
    }
}


#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy, Serialize, Deserialize)]
pub enum TresOperation {
    TernCond,
}

impl TresOperation {
    pub fn eval(&self, a: U256, b: U256, c: U256) -> U256 {
        match self {
            TresOperation::TernCond => if a == U256::ZERO { c } else { b },
        }
    }
}

impl From<&TresOperation> for crate::proto::TresOp {
    fn from(v: &TresOperation) -> Self {
        match v {
            TresOperation::TernCond => crate::proto::TresOp::TernCond,
        }
    }
}

impl TryFrom<u8> for TresOperation {
    type Error = String;
    fn try_from(op: u8) -> Result<Self, Self::Error> {
        match op {
            0 => Ok(TresOperation::TernCond),
            _ => Err(format!("Invalid ternary operation: {}", op)),
        }
    }
}

impl From<&TresOperation> for u8 {
    fn from(val: &TresOperation) -> Self {
        match val {
            TresOperation::TernCond => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Node {
    #[default]
    Unknown,
    Input(usize),
    Constant(usize),
    UnoOp(UnoOperation, usize),
    Op(Operation, usize, usize),
    TresOp(TresOperation, usize, usize, usize),
}

impl Node {
    const SIZE: usize = 3 * size_of::<usize>() + 2;

    fn from_bytes(buf: &[u8]) -> Node {
        match buf[0] {
            0 => Node::Unknown,
            1 => {
                let i = usize::from_le_bytes(buf[1..1+size_of::<usize>()].try_into().unwrap());
                Node::Input(i)
            }
            2 => {
                let i = usize::from_le_bytes(buf[1..1+size_of::<usize>()].try_into().unwrap());
                Node::Constant(i)
            }
            3 => {
                let op = UnoOperation::try_from(buf[1]).unwrap();
                let a = usize::from_le_bytes(buf[2..2+size_of::<usize>()].try_into().unwrap());
                Node::UnoOp(op, a)
            }
            4 => {
                let op = Operation::try_from(buf[1]).unwrap();
                let a = usize::from_le_bytes(buf[2..2+size_of::<usize>()].try_into().unwrap());
                let b = usize::from_le_bytes(buf[2+size_of::<usize>()..2+2*size_of::<usize>()].try_into().unwrap());
                Node::Op(op, a, b)
            }
            5 => {
                let op = TresOperation::try_from(buf[1]).unwrap();
                let a = usize::from_le_bytes(buf[2..2+size_of::<usize>()].try_into().unwrap());
                let b = usize::from_le_bytes(buf[2+size_of::<usize>()..2+2*size_of::<usize>()].try_into().unwrap());
                let c = usize::from_le_bytes(buf[2+2*size_of::<usize>()..2+3*size_of::<usize>()].try_into().unwrap());
                Node::TresOp(op, a, b, c)
            }
            _ => panic!("Invalid node type"),
        }
    }

    fn write_bytes(&self, to: &mut [u8]) {
        match self {
            Node::Unknown => {
                to[0] = 0;
            }
            Node::Input(i) => {
                to[0] = 1;
                to[1..1+size_of::<usize>()].copy_from_slice(&i.to_le_bytes());
            }
            Node::Constant(i) => {
                to[0] = 2;
                to[1..1+size_of::<usize>()].copy_from_slice(&i.to_le_bytes());
            }
            Node::UnoOp(op, a) => {
                to[0] = 3;
                to[1] = Into::<u8>::into(op);
                to[2..2+size_of::<usize>()].copy_from_slice(&a.to_le_bytes());
            }
            Node::Op(op, a, b) => {
                to[0] = 4;
                to[1] = Into::<u8>::into(op);
                to[2..2+size_of::<usize>()].copy_from_slice(&a.to_le_bytes());
                to[2+size_of::<usize>()..2+2*size_of::<usize>()].copy_from_slice(&b.to_le_bytes());
            }
            Node::TresOp(op, a, b, c) => {
                to[0] = 5;
                to[1] = Into::<u8>::into(op);
                to[2..2+size_of::<usize>()].copy_from_slice(&a.to_le_bytes());
                to[2+size_of::<usize>()..2+2*size_of::<usize>()].copy_from_slice(&b.to_le_bytes());
                to[2+2*size_of::<usize>()..2+3*size_of::<usize>()].copy_from_slice(&c.to_le_bytes());
            }
        }
    }

}

pub trait NodesInterface: Any {
    fn push_noopt(&mut self, n: Node) -> NodeIdx;
    fn push(&mut self, n: Node) -> NodeIdx;
    fn get_inputs_size(&self) -> usize;
    fn as_any(&self) -> &dyn Any;
}

pub trait NodesStorage {
    fn len(&self) -> usize;
    fn get(&self, idx: usize) -> Option<Node>;
    fn set(&mut self, idx: usize, n: Node);
    fn push(&mut self, n: Node);
    fn is_empty(&self) -> bool;
    fn retain<F>(&mut self, f: F)
    where
        F: FnMut() -> bool;
}

enum TempFile {
    Named(NamedTempFile),
    Unnamed(File),
}

impl TempFile {
    fn as_file(&self) -> &File {
        match self {
            TempFile::Named(file) => file.as_file(),
            TempFile::Unnamed(file) => file,
        }
    }
}

pub struct MMapNodes {
    tmp_dir_path: PathBuf,
    file: TempFile,
    mm: MmapMut,
    cap: usize,
    ln: usize,
}

impl MMapNodes {
    const init_size: usize = 1_000_000;

    pub fn new(named: bool, temp_dir: &Path) -> Self {
        Self::with_capacity(Self::init_size, named, temp_dir)
    }

    fn with_capacity(cap: usize, named: bool, temp_dir: &Path) -> Self {
        let cap = cap * Node::SIZE;
        let file = Self::create_file(temp_dir, named, cap);
        let mm = unsafe { MmapMut::map_mut(file.as_file()).unwrap() };
        MMapNodes {
            tmp_dir_path: temp_dir.to_path_buf(),
            file,
            mm,
            cap,
            ln: 0,
        }
    }

    fn grow(&mut self) {
        let mut inc = self.cap / 3;
        if inc < 1000 * Node::SIZE {
            inc = 1000 * Node::SIZE;
        }
        self.cap += inc;
        let new_size: u64 = self.cap.try_into().unwrap();
        self.file.as_file().set_len(new_size).unwrap();
        self.mm = unsafe { MmapMut::map_mut(self.file.as_file()).unwrap() };
    }

    fn create_file(in_dir: &Path, named: bool, size: usize) -> TempFile {
        if named {
            let file = NamedTempFile::new_in(in_dir).unwrap();
            println!(
                "Created node storage file: {}", file.path().to_str().unwrap());
            file.as_file().set_len(size.try_into().unwrap()).unwrap();
            TempFile::Named(file)
        } else {
            let file = tempfile::tempfile_in(in_dir).unwrap();
            file.set_len(size.try_into().unwrap()).unwrap();
            TempFile::Unnamed(file)
        }
    }
}

impl NodesStorage for MMapNodes {
    fn len(&self) -> usize {
        self.ln / Node::SIZE
    }

    fn get(&self, idx: usize) -> Option<Node> {
        if idx * Node::SIZE >= self.ln {
            return None;
        }
        let buf = &self.mm[idx * Node::SIZE..(idx + 1) * Node::SIZE];
        Some(Node::from_bytes(buf))
    }

    fn set(&mut self, idx: usize, n: Node) {
        if idx * Node::SIZE >= self.ln {
            panic!("Index out of bounds");
        }
        n.write_bytes(self.mm[idx * Node::SIZE..(idx + 1) * Node::SIZE].as_mut());
    }

    fn push(&mut self, n: Node) {
        if self.ln + Node::SIZE > self.cap {
            self.grow();
        }
        n.write_bytes(self.mm[self.ln..self.ln + Node::SIZE].as_mut());
        self.ln += Node::SIZE;
    }

    fn is_empty(&self) -> bool {
        self.ln == 0
    }

    fn retain<F>(&mut self, mut f: F)
    where
        F: FnMut() -> bool,
    {
        let named = match self.file {
            TempFile::Named(_) => true,
            TempFile::Unnamed(_) => false
        };
        let cap = self.ln;
        let file = Self::create_file(
            self.tmp_dir_path.as_path(), named, self.cap);
        let mut mm = unsafe { MmapMut::map_mut(file.as_file()).unwrap() };

        let mut dst: usize = 0;
        for i in 0..self.len() {
            if f() {
                mm[dst * Node::SIZE..(dst+1)*Node::SIZE]
                    .copy_from_slice(
                        self.mm[i * Node::SIZE..(i+1) * Node::SIZE]
                            .as_ref());
                dst += 1;
            }
        }

        self.mm = mm;
        self.file = file;
        self.cap = cap;
        self.ln = dst * Node::SIZE;
    }
}

pub struct VecNodes {
    nodes: Vec<Node>,
}

impl VecNodes {
    pub fn new() -> Self {
        VecNodes {
            nodes: Vec::new(),
        }
    }
}

impl Default for VecNodes {
    fn default() -> Self {
        Self::new()
    }
}

impl NodesStorage for VecNodes {
    fn len(&self) -> usize {
        self.nodes.len()
    }

    fn get(&self, idx: usize) -> Option<Node> {
        self.nodes.get(idx).copied()
    }

    fn set(&mut self, index: usize, n: Node) {
        self.nodes[index] = n;
    }

    fn push(&mut self, n: Node) {
        self.nodes.push(n);
    }

    fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    fn retain<F>(&mut self, mut f: F)
    where
        F: FnMut() -> bool,
    {
        self.nodes.retain(|_| f());
    }
}

pub struct Nodes<T: FieldOps, NS: NodesStorage> {
    prime_str: String,
    // TODO maybe remove pub
    pub ff: Field<T>,
    // TODO remove pub
    pub nodes: NS,
    pub constants: Vec<T>,
    // Mapping from a const value to a node index
    constants_idx: HashMap<T, usize>,
}

impl<T: FieldOps + 'static, NS: NodesStorage + 'static> Nodes<T, NS> {
    pub fn new(prime: T, prime_str: &str, nodes_storage: NS) -> Self {

        let ff = Field::new(prime);
        Nodes {
            ff,
            // nodes: NodesStorage::new(named_temp, temp_path),
            nodes: nodes_storage,
            constants: Vec::new(),
            constants_idx: HashMap::new(),
            prime_str: prime_str.to_string(),
        }
    }

    pub fn to_const_recursive(&self, idx: NodeIdx) -> Result<T, NodeConstErr> {
        let me = self.nodes.get(idx.0).ok_or(NodeConstErr::EmptyNode(idx))?;
        match me {
            Node::Unknown => panic!("Unknown node"),
            Node::Constant(const_idx) => Ok(self.constants[const_idx]),
            _ => { Err(NodeConstErr::InputSignal) }
        }
    }

    pub fn const_node_idx_from_value(&mut self, v: T) -> usize {
        match self.constants_idx.entry(v) {
            Entry::Occupied(e) => {
                *e.get()
            }
            Entry::Vacant(e) => {
                self.constants.push(v);
                self.nodes.push(Node::Constant(self.constants.len()-1));
                e.insert(self.nodes.len()-1);
                self.nodes.len()-1
            }
        }
    }

    fn rebuild_constants_index(&mut self) {
        self.constants_idx.clear();
        for i in 0..self.nodes.len() {
            let node = self.nodes.get(i).unwrap();
            if let Node::Constant(c_idx) = node {
                self.constants_idx.insert(self.constants[c_idx], i);
            }
        }
    }

    pub fn get(&self, idx: NodeIdx) -> Option<Node> {
        self.nodes.get(idx.0)
    }

    pub fn to_proto(
        &self,
        idx: usize) -> Result<crate::proto::node::Node, NodeConstErr> {

        let n = self.nodes.get(idx)
            .ok_or(NodeConstErr::EmptyNode(NodeIdx(idx)))?;
        match n {
            Node::Unknown => panic!("unknown node"),
            Node::Input(i) => {
                let idx: u32 = i.try_into().unwrap();
                Ok(
                    crate::proto::node::Node::Input (
                        crate::proto::InputNode { idx }))
            },
            Node::Constant(idx) => {
                let c = self.constants[idx];
                let i = crate::proto::BigUInt { value_le: c.to_le_bytes() };
                Ok(crate::proto::node::Node::Constant(
                    crate::proto::ConstantNode { value: Some(i) }))
            },
            Node::UnoOp(op, a) => Ok(
                crate::proto::node::Node::UnoOp(
                    crate::proto::UnoOpNode {
                        op: crate::proto::UnoOp::from(&op) as i32,
                        a_idx: a as u32 })
            ),
            Node::Op(op, a, b) => Ok(
                crate::proto::node::Node::DuoOp(
                    crate::proto::DuoOpNode {
                        op: crate::proto::DuoOp::from(&op) as i32,
                        a_idx: a as u32,
                        b_idx: b as u32 })),
            Node::TresOp(op, a, b, c) => Ok(
                crate::proto::node::Node::TresOp(
                    crate::proto::TresOpNode {
                        op: crate::proto::TresOp::from(&op) as i32,
                        a_idx: a as u32,
                        b_idx: b as u32,
                        c_idx: c as u32 })),
        }
    }

    pub fn push_proto(&mut self, n: &crate::proto::node::Node) {
        match n {
            crate::proto::node::Node::Input(n2) => {
                let idx: usize = n2.idx.try_into().unwrap();
                self.push_noopt(Node::Input(idx));
            },
            crate::proto::node::Node::Constant(n2) => {
                let c = (&self.ff)
                    .parse_le_bytes(&n2.value.as_ref().unwrap().value_le)
                    .unwrap();
                self.const_node_idx_from_value(c);
            },
            crate::proto::node::Node::UnoOp(n2) => {
                let op = crate::proto::UnoOp::try_from(n2.op).unwrap();
                self.push_noopt(Node::UnoOp(op.into(), n2.a_idx as usize));
            },
            crate::proto::node::Node::DuoOp(n2) => {
                let op = crate::proto::DuoOp::try_from(n2.op).unwrap();
                self.push_noopt(
                    Node::Op(op.into(), n2.a_idx as usize, n2.b_idx as usize));
            },
            crate::proto::node::Node::TresOp(n2) => {
                let op = crate::proto::TresOp::try_from(n2.op).unwrap();
                self.push_noopt(
                    Node::TresOp(
                        op.into(), n2.a_idx as usize, n2.b_idx as usize,
                        n2.c_idx as usize));
            },
        }
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn prime(&self) -> T {
        self.ff.prime
    }

    pub fn prime_str(&self) -> String {
        self.prime_str.clone()
    }
}

impl<T: FieldOps + 'static, NS: NodesStorage + 'static> NodesInterface for Nodes<T, NS> {
    // push without optimization
    fn push_noopt(&mut self, n: Node) -> NodeIdx {
        self.nodes.push(n);
        NodeIdx(self.nodes.len() - 1)
    }

    fn push(&mut self, n: Node) -> NodeIdx {
        match n {
            Node::Unknown => panic!("Unknown node"),
            Node::Constant(c_idx) => {
                let v = self.constants[c_idx];
                let idx = self.const_node_idx_from_value(v);
                NodeIdx(idx)
            },
            Node::UnoOp(op, a) => {
                if let Some(Node::Constant(a_idx)) = self.nodes.get(a) {
                    let v = (&self.ff).op_uno(op, self.constants[a_idx]);
                    let idx = self.const_node_idx_from_value(v);
                    NodeIdx(idx)
                } else {
                    self.push_noopt(n)
                }
            }
            Node::Op(op, a, b) => {
                if let (
                    Some(Node::Constant(a_idx)),
                    Some(Node::Constant(b_idx))) = (
                    self.nodes.get(a),
                    self.nodes.get(b)) {

                    let v = (&self.ff).op_duo(op, self.constants[a_idx], self.constants[b_idx]);
                    let idx = self.const_node_idx_from_value(v);
                    NodeIdx(idx)
                } else {
                    self.push_noopt(n)
                }
            }
            Node::TresOp(op, a, b, c) => {
                if let (
                    Some(Node::Constant(a_idx)),
                    Some(Node::Constant(b_idx)),
                    Some(Node::Constant(c_idx))) = (
                    self.nodes.get(a),
                    self.nodes.get(b),
                    self.nodes.get(c)) {

                    let v = (&self.ff).op_tres(
                        op, self.constants[a_idx], self.constants[b_idx],
                        self.constants[c_idx]);
                    let idx = self.const_node_idx_from_value(v);
                    NodeIdx(idx)
                } else {
                    self.push_noopt(n)
                }
            }
            Node::Input(_) => {
                self.push_noopt(n)
            },
        }
    }

    fn get_inputs_size(&self) -> usize {
        let mut start = false;
        let mut max_index = 0usize;
        for i in 0..self.nodes.len() {
            let node = self.nodes.get(i).unwrap();
            if let Node::Input(i) = node {
                if i > max_index {
                    max_index = i;
                }
                start = true;
            } else if start {
                break;
            }
        }
        max_index + 1
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
impl<T: FieldOps, NS: NodesStorage> PartialEq for Nodes<T, NS> {
    fn eq(&self, other: &Self) -> bool {
        if self.nodes.len() != other.nodes.len() {
            return false;
        }
        for i in 0..self.nodes.len() {
            let a = self.nodes.get(i).unwrap();
            let b = other.nodes.get(i).unwrap();
            let eq = match (a, b) {
                (Node::Unknown, Node::Unknown) => true,
                (Node::Input(a), Node::Input(b)) => a == b,
                (Node::Constant(a), Node::Constant(b)) => self.constants[a] == self.constants[b],
                (Node::UnoOp(a, b), Node::UnoOp(c, d)) => a == c && b == d,
                (Node::Op(a, b, c), Node::Op(d, e, f)) => a == d && b == e && c == f,
                (Node::TresOp(a, b, c, d), Node::TresOp(e, f, g, h)) => a == e && b == f && c == g && d == h,
                _ => false,
            };
            if !eq {
                return false;
            }
        }

        true
    }
}

#[cfg(test)]
impl<T: FieldOps, NS: NodesStorage> Debug for Nodes<T, NS> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Nodes {{")?;
        for i in 0..self.nodes.len() {
            let n = self.nodes.get(i).unwrap();
            if let Node::Constant(c_idx) = n {
                let bs = self.constants[c_idx].to_le_bytes();
                let n = U256::from_le_slice(&bs);
                writeln!(f, "    {}: Constant({})", i, n)?;
            } else {
                writeln!(f, "    {}: {:?}", i, n)?;
            }
        }
        writeln!(f, "}}")?;
        Ok(())
    }
}

#[derive(Debug, Copy, Clone)]
pub struct NodeIdx(pub usize);

impl From<usize> for NodeIdx {
    fn from(v: usize) -> Self {
        NodeIdx(v)
    }
}

#[derive(Debug)]
pub enum NodeConstErr {
    EmptyNode(NodeIdx),
    InputSignal,
    NotConst,
}

impl std::fmt::Display for NodeConstErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeConstErr::EmptyNode(idx) => {
                write!(f, "empty node at index {}", idx.0)
            }
            NodeConstErr::InputSignal => {
                write!(f, "input signal is not a constant")
            }
            NodeConstErr::NotConst => {
                write!(f, "node is not a constant")
            }
        }
    }
}

impl Error for NodeConstErr {}

fn compute_shl_uint(a: U256, b: U256) -> U256 {
    debug_assert!(b.lt(&U256::from(256)));
    let ls_limb = b.as_limbs()[0];
    a << ls_limb as usize
}

fn compute_shr_uint(a: U256, b: U256) -> U256 {
    debug_assert!(b.lt(&U256::from(256)));
    let ls_limb = b.as_limbs()[0];
    a >> ls_limb as usize
}

/// All references must be backwards.
fn assert_valid<NS: NodesStorage>(nodes: &NS) {
    for i in 0..nodes.len() {
        let node = nodes.get(i).unwrap();
        if let Node::Op(_, a, b) = node {
            assert!(a < i);
            assert!(b < i);
        } else if let Node::UnoOp(_, a) = node {
            assert!(a < i);
        } else if let Node::TresOp(_, a, b, c) = node {
            assert!(a < i);
            assert!(b < i);
            assert!(c < i);
        }
    }
}

pub fn optimize<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    nodes: &mut Nodes<T, NS>, outputs: &mut [usize]) {

    tree_shake(nodes, outputs);
    propagate(nodes);
    value_numbering(nodes, outputs);
    find_constants(nodes);
    tree_shake(nodes, outputs);
}

pub fn evaluate<T: FieldOps, F: FieldOperations<Type = T>, NS: NodesStorage>(
    ff: F, nodes: &NS, inputs: &[T], outputs: &[usize],
    constants: &[T]) -> Vec<T>
where Vec<T>: FromIterator<<F as FieldOperations>::Type>
{
    // assert_valid(nodes);

    let start = Instant::now();
    // Evaluate the graph.
    let mut values = Vec::with_capacity(nodes.len());
    for i in 0..nodes.len() {
        let node = nodes.get(i).unwrap();
        let value = match node {
            Node::Unknown => panic!("Unknown node"),
            Node::Constant(i) => constants[i],
            Node::Input(i) => inputs[i],
            Node::Op(op, a, b) => {
                ff.op_duo(op, values[a], values[b])
            },
            Node::UnoOp(op, a) => {
                ff.op_uno(op, values[a])
            },
            Node::TresOp(op, a, b, c) => {
                match op {
                    TresOperation::TernCond => {
                        if values[a].is_zero() { values[c] } else { values[b] }
                    },
                }
            },
        };
        values.push(value);
    }

    let r = outputs.iter().map(|&i| values[i]).collect();
    println!("generic typed graph calculated in {:?}", start.elapsed());
    r
}

/// Montgomery form `Fr` to its canonical `U254` residue in `[0, prime)`.
#[inline(always)]
fn to_canonical(v: Fr) -> U254 {
    U254::from_limbs(v.into_bigint().0)
}

/// `U254` to Montgomery form `Fr`, reducing modulo the prime first so the conversion is total:
/// `Fr::from_bigint` rejects any value `>= prime`. Callers that must preserve a non-canonical
/// raw representation should store it as [`Bn254Value::Raw`] instead. `prime` must be the
/// bn254 modulus; `evaluate_bn254` asserts that precondition.
#[inline(always)]
fn to_montgomery(v: U254, prime: U254) -> Fr {
    let v = if v < prime { v } else { v % prime };
    Fr::from_bigint(BigInt(v.into_limbs())).unwrap()
}

#[derive(Clone, Copy)]
enum Bn254Value {
    Mont(Fr),
    Raw(U254),
}

impl Bn254Value {
    #[inline(always)]
    fn from_raw(v: U254, prime: U254) -> Self {
        if v < prime {
            Bn254Value::Mont(to_montgomery(v, prime))
        } else {
            Bn254Value::Raw(v)
        }
    }

    #[inline(always)]
    fn from_mont(v: Fr) -> Self {
        Bn254Value::Mont(v)
    }

    #[inline(always)]
    fn raw(self) -> U254 {
        match self {
            Bn254Value::Mont(v) => to_canonical(v),
            Bn254Value::Raw(v) => v,
        }
    }

    #[inline(always)]
    fn mont(self, prime: U254) -> Fr {
        match self {
            Bn254Value::Mont(v) => v,
            Bn254Value::Raw(v) => to_montgomery(v, prime),
        }
    }

    #[inline(always)]
    fn is_zero(self) -> bool {
        match self {
            Bn254Value::Mont(v) => v.is_zero(),
            Bn254Value::Raw(v) => v.is_zero(),
        }
    }

    #[inline(always)]
    fn eq_raw(self, other: Self) -> bool {
        match (self, other) {
            (Bn254Value::Mont(x), Bn254Value::Mont(y)) => x == y,
            (x, y) => x.raw() == y.raw(),
        }
    }
}

#[inline(always)]
fn bn254_bool(v: bool) -> Bn254Value {
    Bn254Value::Mont(if v { Fr::one() } else { Fr::zero() })
}

#[inline(always)]
fn reduce_gt_prime(mut v: U254, prime: U254) -> U254 {
    // Stable shift semantics reduce only values strictly greater than the prime.
    if v > prime {
        v %= prime;
    }
    v
}

#[inline(always)]
fn reduce_ge_prime(mut v: U254, prime: U254) -> U254 {
    // Stable bitwise-not/or/xor semantics reduce values greater than or equal to the prime.
    if v >= prime {
        v %= prime;
    }
    v
}

#[inline(always)]
fn bn254_to_isize(v: U254, prime: U254, half_prime: U254) -> Option<isize> {
    if v > half_prime {
        TryInto::<isize>::try_into(prime - v).ok().map(|v| -v)
    } else {
        TryInto::<isize>::try_into(v).ok()
    }
}

#[inline(always)]
fn bn254_shl(lhs: U254, rhs: U254, prime: U254, half_prime: U254) -> U254 {
    match bn254_to_isize(rhs, prime, half_prime) {
        Some(r) => {
            let out = if r >= 0 {
                lhs << r as usize
            } else {
                lhs >> (-r as usize)
            };
            reduce_gt_prime(out, prime)
        },
        None => U254::from(0u64),
    }
}

#[inline(always)]
fn bn254_shr(lhs: U254, rhs: U254, prime: U254, half_prime: U254) -> U254 {
    match bn254_to_isize(rhs, prime, half_prime) {
        Some(r) => {
            let out = if r >= 0 {
                lhs >> r as usize
            } else {
                lhs << (-r as usize)
            };
            reduce_gt_prime(out, prime)
        },
        None => U254::from(0u64),
    }
}

#[inline(always)]
fn bn254_cmp(lhs: U254, rhs: U254, half_prime: U254) -> Ordering {
    let lhs_neg = half_prime < lhs;
    let rhs_neg = half_prime < rhs;

    if lhs_neg == rhs_neg {
        lhs.cmp(&rhs)
    } else if lhs_neg {
        Ordering::Less
    } else {
        Ordering::Greater
    }
}

#[inline(always)]
fn bn254_sqrt(v: Bn254Value, prime: U254, half_prime: U254) -> Bn254Value {
    use ark_ff::Field as ArkField;

    let x = v.mont(prime);
    if x.is_zero() {
        return Bn254Value::from_mont(x);
    }
    match x.sqrt() {
        Some(root) => {
            let canonical = to_canonical(root);
            if canonical > half_prime {
                Bn254Value::from_mont(-root)
            } else {
                Bn254Value::from_mont(root)
            }
        },
        None => Bn254Value::from_mont(Fr::zero()),
    }
}

/// bn254-specialized graph evaluator that keeps every node value resident in Montgomery form
/// (`Fr`) whenever its raw integer representation is canonical. Representation-level ops
/// (bitwise/shift/integer division/modulo) are evaluated on exact `U254` views and stored as
/// raw integers only when the stable evaluator's unreduced representation must remain
/// observable. By induction over the topological node order, `raw()` for every stored value is
/// byte-identical to the value produced by [`evaluate`], while the common all-field hot path
/// stays in Montgomery form.
pub fn evaluate_bn254<NS: NodesStorage>(
    ff: &Field<U254>, nodes: &NS, inputs: &[U254],
    outputs: &[usize], constants: &[U254]) -> Vec<U254>
{
    use ark_ff::Field as ArkField;

    let prime = ff.prime;
    let half_prime = prime >> 1;
    assert_eq!(prime, bn254_prime, "evaluate_bn254 requires the bn254 field");
    let zero = Fr::zero();

    let mut values: Vec<Bn254Value> = Vec::with_capacity(nodes.len());
    for i in 0..nodes.len() {
        let v: Bn254Value = match nodes.get(i).unwrap() {
            Node::Unknown => panic!("Unknown node"),
            Node::Constant(c) => Bn254Value::from_raw(constants[c], prime),
            Node::Input(k) => Bn254Value::from_raw(inputs[k], prime),
            Node::Op(op, a, b) => {
                let (x, y) = (values[a], values[b]);
                match (op, x, y) {
                    (Operation::Mul, x, y) => Bn254Value::from_mont(x.mont(prime) * y.mont(prime)),
                    (Operation::Add, x, y) => Bn254Value::from_mont(x.mont(prime) + y.mont(prime)),
                    (Operation::Sub, Bn254Value::Mont(x), Bn254Value::Mont(y)) => {
                        Bn254Value::from_mont(x - y)
                    },
                    (Operation::Sub, x, Bn254Value::Raw(y)) if y > prime => {
                        Bn254Value::from_raw(ff.op_duo(op, x.raw(), y), prime)
                    },
                    (Operation::Sub, x, y) => Bn254Value::from_mont(x.mont(prime) - y.mont(prime)),
                    (Operation::Div, x, Bn254Value::Mont(y)) => {
                        Bn254Value::from_mont(
                            if y == zero { zero } else { x.mont(prime) * y.inverse().unwrap() })
                    },
                    (Operation::Pow, x, y) => {
                        Bn254Value::from_mont(x.mont(prime).pow(y.raw().into_limbs()))
                    },
                    (Operation::Idiv, x, y) => {
                        let y = y.raw();
                        Bn254Value::from_raw(
                            if y.is_zero() { U254::from(0u64) } else { x.raw() / y },
                            prime)
                    },
                    (Operation::Mod, x, y) => {
                        let y = y.raw();
                        Bn254Value::from_raw(
                            if y.is_zero() { U254::from(0u64) } else { x.raw() % y },
                            prime)
                    },
                    (Operation::Eq, x, y) => bn254_bool(x.eq_raw(y)),
                    (Operation::Neq, x, y) => bn254_bool(!x.eq_raw(y)),
                    (Operation::Lt, x, y) => {
                        bn254_bool(bn254_cmp(x.raw(), y.raw(), half_prime) == Ordering::Less)
                    },
                    (Operation::Gt, x, y) => {
                        bn254_bool(bn254_cmp(x.raw(), y.raw(), half_prime) == Ordering::Greater)
                    },
                    (Operation::Leq, x, y) => {
                        bn254_bool(bn254_cmp(x.raw(), y.raw(), half_prime) != Ordering::Greater)
                    },
                    (Operation::Geq, x, y) => {
                        bn254_bool(bn254_cmp(x.raw(), y.raw(), half_prime) != Ordering::Less)
                    },
                    (Operation::Land, x, y) => bn254_bool(!x.is_zero() && !y.is_zero()),
                    (Operation::Lor, x, y) => bn254_bool(!x.is_zero() || !y.is_zero()),
                    (Operation::Shl, x, Bn254Value::Mont(y)) => {
                        Bn254Value::from_raw(bn254_shl(x.raw(), to_canonical(y), prime, half_prime), prime)
                    },
                    (Operation::Shr, x, Bn254Value::Mont(y)) => {
                        Bn254Value::from_raw(bn254_shr(x.raw(), to_canonical(y), prime, half_prime), prime)
                    },
                    (Operation::Bor, x, y) => {
                        Bn254Value::from_raw(reduce_ge_prime(x.raw() | y.raw(), prime), prime)
                    },
                    (Operation::Band, x, y) => Bn254Value::from_raw(x.raw() & y.raw(), prime),
                    (Operation::Bxor, x, y) => {
                        Bn254Value::from_raw(reduce_ge_prime(x.raw() ^ y.raw(), prime), prime)
                    },
                    _ => Bn254Value::from_raw(ff.op_duo(op, x.raw(), y.raw()), prime),
                }
            },
            Node::UnoOp(op, a) => {
                let x = values[a];
                match (op, x) {
                    (UnoOperation::Id, x) => x,
                    (UnoOperation::Neg, Bn254Value::Mont(x)) => Bn254Value::from_mont(-x),
                    (UnoOperation::Lnot, x) => bn254_bool(x.is_zero()),
                    (UnoOperation::Bnot, x) => {
                        Bn254Value::from_raw(reduce_ge_prime(!x.raw(), prime), prime)
                    },
                    (UnoOperation::Sqrt, x) => bn254_sqrt(x, prime, half_prime),
                    _ => Bn254Value::from_raw(ff.op_uno(op, x.raw()), prime),
                }
            },
            Node::TresOp(TresOperation::TernCond, a, b, c) => {
                if values[a].is_zero() { values[c] } else { values[b] }
            },
        };
        values.push(v);
    }

    outputs.iter().map(|&i| values[i].raw()).collect()
}

// pub fn evaluate_parallel(nodes: &[Node], inputs: &[U256], outputs: &[usize]) -> Vec<U256> {
//     let start = Instant::now();
//     let inputs: Arc<[U256]> = Arc::from(inputs);
//     println!("total nodes: {}", nodes.len());
//     let mut nodes_splitted = 0;
//     let sz = outputs.len() / 4;
//
//     let mut outputs2 = Vec::new();
//     let mut subgraphs = Vec::new();
//
//     for (i, chunk) in outputs.chunks(sz).enumerate() {
//         let mut nodes = Vec::from(nodes);
//         let mut chunk = Vec::from(chunk);
//         tree_shake(&mut nodes, &mut chunk);
//         nodes_splitted += nodes.len();
//         println!("chunk #{}: {} nodes", i, nodes.len());
//
//         outputs2.push(chunk);
//         subgraphs.push(nodes);
//     }
//     println!("total nodes splitted: {}", nodes_splitted);
//     println!("graph splitted in {:?}", start.elapsed());
//     // assert_valid(nodes);
//
//     let start = Instant::now();
//
//     let mut handles = Vec::new();
//     let threads_num = subgraphs.len();
//     let (tx, rx) = mpsc::channel();
//     for (i, (nodes, outputs)) in subgraphs.into_iter().zip(outputs2).enumerate() {
//         let inputs = Arc::clone(&inputs);
//         let tx = tx.clone();
//         let handle = thread::spawn(move || {
//             let mut values = Vec::with_capacity(nodes.len());
//             for &node in nodes.iter() {
//                 let value = match node {
//                     Node::Unknown => panic!("Unknown node"),
//                     Node::Constant(_) => todo!(),
//                     Node::Input(i) => inputs[i],
//                     Node::Op(op, a, b) => op.eval(values[a], values[b]),
//                     Node::UnoOp(op, a) => op.eval(values[a]),
//                     Node::TresOp(op, a, b, c) => op.eval(values[a], values[b], values[c]),
//                 };
//                 values.push(value);
//             }
//
//             let witness_signals: Vec<U256> = outputs.iter().map(|&i| values[i]).collect();
//             tx.send((i, witness_signals)).unwrap();
//         });
//         handles.push(handle);
//     }
//
//     let mut final_results = vec![Vec::new(); threads_num];
//
//     for handle in handles {
//         handle.join().unwrap();
//     }
//
//     for _ in 0..threads_num {
//         if let Ok((i, signals)) = rx.recv() {
//             final_results[i] = signals;
//         }
//     }
//
//     let r = final_results.into_iter().flatten().collect();
//     println!("graph calculated in parallel in {:?}", start.elapsed());
//
//     r
// }

/// Constant propagation
pub fn propagate<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    nodes: &mut Nodes<T, NS>) {

    assert_valid(&nodes.nodes);
    let mut constants = 0_usize;
    for i in 0..nodes.len() {
        let node = nodes.nodes.get(i).unwrap();
        if let Node::Op(op, a, b) = node {
            if let (
                Some(Node::Constant(va)),
                Some(Node::Constant(vb))) = (
                nodes.nodes.get(a), nodes.nodes.get(b)) {
                let v = (&nodes.ff).op_duo(
                    op, nodes.constants[va], nodes.constants[vb]);
                let node_idx = nodes.const_node_idx_from_value(v);
                let n = nodes.nodes.get(node_idx).unwrap();
                nodes.nodes.set(i, n);
                constants += 1;
            } else if a == b {
                // Not constant but equal
                use Operation::*;
                if let Some(c) = match op {
                    Eq | Leq | Geq => Some(true),
                    Neq | Lt | Gt => Some(false),
                    _ => None,
                } {
                    let v = T::from_bool(c);
                    let node_idx = nodes.const_node_idx_from_value(v);
                    let n = nodes.nodes.get(node_idx).unwrap();
                    nodes.nodes.set(i, n);
                    constants += 1;
                }
            }
        } else if let Node::UnoOp(op, a) = node {
            if let Some(Node::Constant(va)) = nodes.nodes.get(a) {
                let v = (&nodes.ff).op_uno(op, nodes.constants[va]);
                let node_idx = nodes.const_node_idx_from_value(v);
                let n = nodes.nodes.get(node_idx).unwrap();
                nodes.nodes.set(i, n);
                constants += 1;
            }
        } else if let Node::TresOp(op, a, b, c) = node {
            if let (
                Some(Node::Constant(va)), Some(Node::Constant(vb)),
                Some(Node::Constant(vc))) = (
                nodes.nodes.get(a), nodes.nodes.get(b), nodes.nodes.get(c)) {

                let v = (&nodes.ff).op_tres(
                    op, nodes.constants[va], nodes.constants[vb],
                    nodes.constants[vc]);
                let node_idx = nodes.const_node_idx_from_value(v);
                let n = nodes.nodes.get(node_idx).unwrap();
                nodes.nodes.set(i, n);
                constants += 1;
            }
        }
    }

    eprintln!("Propagated {constants} constants");
}

/// Remove unused nodes
pub fn tree_shake<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    nodes: &mut Nodes<T, NS>, outputs: &mut [usize]) {

    assert_valid(&nodes.nodes);

    println!("[tree shake start]");
    println!("  look for unused nodes");
    let pb_len = outputs.len() + nodes.nodes.len();
    let pb = progress_bar(pb_len);

    // Mark all nodes that are used.
    let mut used = vec![false; nodes.nodes.len()];
    for &i in outputs.iter() {
        used[i] = true;
        pb.inc(1);
    }

    // Work backwards from end as all references are backwards.
    for i in (0..nodes.nodes.len()).rev() {
        if used[i] {
            let node = nodes.nodes.get(i).unwrap();
            if let Node::Op(_, a, b) = node {
                used[a] = true;
                used[b] = true;
            }
            if let Node::UnoOp(_, a) = node {
                used[a] = true;
            }
            if let Node::TresOp(_, a, b, c) = node {
                used[a] = true;
                used[b] = true;
                used[c] = true;
            }
        }
        pb.inc(1);
    }

    pb.finish();

    // Remove unused nodes
    let n = nodes.nodes.len();
    let mut retain = used.iter();
    nodes.nodes.retain(|| *retain.next().unwrap());

    let removed = n - nodes.nodes.len();

    if removed > 0 {
        nodes.rebuild_constants_index();
    }

    println!("  renumber nodes");
    // Renumber references.
    let mut renumber = vec![None; n];
    let mut index = 0;
    for (i, &used) in used.iter().enumerate() {
        if used {
            renumber[i] = Some(index);
            index += 1;
        }
    }
    assert_eq!(index, nodes.nodes.len());
    for (&used, renumber) in used.iter().zip(renumber.iter()) {
        assert_eq!(used, renumber.is_some());
    }

    let pb_len = outputs.len() + nodes.nodes.len();
    let pb = progress_bar(pb_len);

    for i in 0..nodes.nodes.len() {
        match nodes.nodes.get(i) {
            Some(Node::UnoOp(op, a)) => {
                nodes.nodes.set(i, Node::UnoOp(op, renumber[a].unwrap()));
            }
            Some(Node::Op(op, a, b)) => {
                nodes.nodes.set(
                    i,
                    Node::Op(op, renumber[a].unwrap(), renumber[b].unwrap()));
            }
            Some(Node::TresOp(op, a, b, c)) => {
                nodes.nodes.set(
                    i,
                    Node::TresOp(
                        op, renumber[a].unwrap(), renumber[b].unwrap(),
                        renumber[c].unwrap()));
            }
            _ => (),
        }
        pb.inc(1);
    }

    for output in outputs.iter_mut() {
        *output = renumber[*output].unwrap();
        pb.inc(1);
    }

    pb.finish();

    println!("[tree shake end: removed {removed} unused nodes]");
}

fn rnd<T: FieldOps>(ff: &Field<T>, rng: &mut ThreadRng) -> T {
    let x = T::BITS.div_ceil(8);
    let mut bs = vec![0u8; x];
    rng.fill_bytes(&mut bs);

    let bits = T::BITS % 8;
    if bits != 0 {
        let mask = (1u16 << bits) - 1;
        let last_idx = bs.len() - 1;
        bs[last_idx] &= mask as u8;
    }

    ff.parse_le_bytes(&bs).unwrap()
}


/// Randomly evaluate the graph
fn random_eval<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    nodes: &mut Nodes<T, NS>) -> Vec<T> {

    let mut rng = rand::thread_rng();
    let mut values = Vec::with_capacity(nodes.len());
    let mut inputs = HashMap::new();
    let mut prfs = HashMap::new();
    let mut prfs_uno = HashMap::new();
    let mut prfs_tres = HashMap::new();
    for i in 0..nodes.nodes.len() {
        let node = nodes.nodes.get(i).unwrap();
        let value = match node {
            Node::Unknown => panic!("Unknown node"),

            Node::Constant(c_idx) => nodes.constants[c_idx],

            // Algebraic Ops are evaluated directly
            // Since the field is large, by Swartz-Zippel if
            // two values are the same then they are likely algebraically equal.
            Node::Op(op @ (Operation::Add | Operation::Sub | Operation::Mul), a, b) => {
                (&nodes.ff).op_duo(op, values[a], values[b])
            },

            // Input and non-algebraic ops are random functions
            // TODO: https://github.com/recmo/uint/issues/95 and use .gen_range(..M)
            Node::Input(i) => *inputs.entry(i)
                .or_insert_with(|| rnd(&nodes.ff, &mut rng)),
            Node::Op(op, a, b) => *prfs
                .entry((op, values[a], values[b]))
                .or_insert_with(|| rnd(&nodes.ff, &mut rng)),
            Node::UnoOp(op, a) => *prfs_uno
                .entry((op, values[a]))
                .or_insert_with(|| rnd(&nodes.ff, &mut rng)),
            Node::TresOp(op, a, b, c) => *prfs_tres
                .entry((op, values[a], values[b], values[c]))
                .or_insert_with(|| rnd(&nodes.ff, &mut rng)),
        };
        values.push(value);
    }
    values
}

/// Value numbering
pub fn value_numbering<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    nodes: &mut Nodes<T, NS>, outputs: &mut [usize]) {

    assert_valid(&nodes.nodes);

    // Evaluate the graph in random field elements.
    let values = random_eval(nodes);

    // Find all nodes with the same value.
    let mut value_map = HashMap::new();
    for (i, &value) in values.iter().enumerate() {
        value_map.entry(value).or_insert_with(Vec::new).push(i);
    }

    // For nodes that are the same, pick the first index.
    let mut renumber = Vec::with_capacity(nodes.len());
    for value in values {
        renumber.push(value_map[&value][0]);
    }

    for i in 0..nodes.nodes.len() {
        match nodes.nodes.get(i) {
            Some(Node::UnoOp(op, a)) => {
                nodes.nodes.set(i, Node::UnoOp(op, renumber[a]));
            }
            Some(Node::Op(op, a, b)) => {
                nodes.nodes.set(i, Node::Op(op, renumber[a], renumber[b]));
            }
            Some(Node::TresOp(op, a, b, c)) => {
                nodes.nodes.set(
                    i, Node::TresOp(op, renumber[a], renumber[b], renumber[c]));
            }
            _ => ()
        }
    }

    for output in outputs.iter_mut() {
        *output = renumber[*output];
    }

    eprintln!("Global value numbering applied");
}

/// Probabilistic constant determination
pub fn find_constants<T: FieldOps + 'static, NS: NodesStorage + 'static>(
    nodes: &mut Nodes<T, NS>) {

    assert_valid(&nodes.nodes);

    // Evaluate the graph in random field elements.
    let values_a = random_eval(nodes);
    let values_b = random_eval(nodes);

    // Find all nodes with the same value.
    let mut constants = 0;
    for i in 0..nodes.len() {
        if let Some(Node::Constant(_)) = nodes.nodes.get(i) {
            continue;
        }
        if values_a[i] == values_b[i] {
            let idx = nodes.const_node_idx_from_value(values_a[i]);
            let n = nodes.nodes.get(idx).unwrap();
            assert!(matches!(n, Node::Constant(_)));
            nodes.nodes.set(i, n);
            constants += 1;
        }
    }
    eprintln!("Found {} constants", constants);
}

// M / 2
const halfM: U256 = uint!(10944121435919637611123202872628637544274182200208017171849102093287904247808_U256);


fn u_gte(a: &U256, b: &U256) -> U256 {
    let a_neg = &halfM < a;
    let b_neg = &halfM < b;

    match (a_neg, b_neg) {
        (false, false) => U256::from(a >= b),
        (true, false) => uint!(0_U256),
        (false, true) => uint!(1_U256),
        (true, true) => U256::from(a >= b),
    }
}

fn u_lte(a: &U256, b: &U256) -> U256 {
    let a_neg = &halfM < a;
    let b_neg = &halfM < b;

    match (a_neg, b_neg) {
        (false, false) => U256::from(a <= b),
        (true, false) => uint!(1_U256),
        (false, true) => uint!(0_U256),
        (true, true) => U256::from(a <= b),
    }
}

fn u_gt(a: &U256, b: &U256) -> U256 {
    let a_neg = &halfM < a;
    let b_neg = &halfM < b;

    match (a_neg, b_neg) {
        (false, false) => U256::from(a > b),
        (true, false) => uint!(0_U256),
        (false, true) => uint!(1_U256),
        (true, true) => U256::from(a > b),
    }
}

fn u_lt(a: &U256, b: &U256) -> U256 {
    let a_neg = &halfM < a;
    let b_neg = &halfM < b;

    match (a_neg, b_neg) {
        (false, false) => U256::from(a < b),
        (true, false) => uint!(1_U256),
        (false, true) => uint!(0_U256),
        (true, true) => U256::from(a < b),
    }
}

#[cfg(test)]
mod tests {
    use std::ops::{Div};
    use super::*;
    use ruint::{uint};
    use crate::field::U254;

    #[test]
    fn test_ok() {
        let prime = U254::from_str_radix(
            "21888242871839275222246405745257275088548364400416034343698204186575808495617",
            10).unwrap();
        let ff = Field::new(prime);
        let mut rng = rand::thread_rng();
        let y = rnd(&ff, &mut rng);
        // println!("{}", rnd::<U254>());
        // let y = rng.gen::<[u8; 3]>();
        println!("{:?}", y);
    }

    #[test]
    fn test_div() {
        assert_eq!(
            Operation::Div.eval(U256::from(2u64), U256::from(3u64)),
            U256::from_str_radix("7296080957279758407415468581752425029516121466805344781232734728858602831873", 10).unwrap());

        assert_eq!(
            Operation::Div.eval(U256::from(6u64), U256::from(2u64)),
            U256::from_str_radix("3", 10).unwrap());

        assert_eq!(
            Operation::Div.eval(U256::from(7u64), U256::from(2u64)),
            U256::from_str_radix("10944121435919637611123202872628637544274182200208017171849102093287904247812", 10).unwrap());
    }

    #[test]
    fn test_idiv() {
        assert_eq!(
            Operation::Idiv.eval(U256::from(2u64), U256::from(3u64)),
            U256::from(0));

        assert_eq!(
            Operation::Idiv.eval(U256::from(6u64), U256::from(2u64)),
            U256::from(3));

        assert_eq!(
            Operation::Idiv.eval(U256::from(7u64), U256::from(2u64)),
            U256::from(3));
    }

    #[test]
    fn test_fr_mod() {
        assert_eq!(
            Operation::Mod.eval(U256::from(7u64), U256::from(0u64)),
            U256::from(0));

        assert_eq!(
            Operation::Mod.eval(U256::from(7u64), U256::from(2u64)),
            U256::from(1));

        assert_eq!(
            Operation::Mod.eval(U256::from(7u64), U256::from(9u64)),
            U256::from(7));
    }

    #[test]
    fn test_greater_then_module() {
        // println!("{}", Fr::MODULUS);
        // let f = Fr::from_str("21888242871839275222246405745257275088548364400416034343698204186575808495619").unwrap();
        // println!("[2] {}", f);
        // let mut i = f.into_bigint();
        // println!("[3] {}", i);
        // let j = i.add_with_carry(&Fr::MODULUS);
        // println!("[4] {}", i);
        // println!("[5] {}", j);
        // if i.cmp(&Fr::MODULUS).is_ge() {
        //     i.sub_with_borrow(&Fr::MODULUS);
        // }
        // let f2 = Fr::from_bigint(i).unwrap();
        // println!("[6] {}", f2);
        // let a= Fr::from(4u64);
        // let b= Fr::from(2u64);
        // let c = shl(a, b);
        // assert_eq!(c.cmp(&Fr::from(16u64)), Ordering::Equal)
    }

    #[test]
    fn test_u_gte() {
        let result = u_gte(&uint!(10_U256), &uint!(3_U256));
        assert_eq!(result, uint!(1_U256));

        let result = u_gte(&uint!(3_U256), &uint!(3_U256));
        assert_eq!(result, uint!(1_U256));

        let result = u_gte(&uint!(2_U256), &uint!(3_U256));
        assert_eq!(result, uint!(0_U256));

        // -1 >= 3 => 0
        let result = u_gte(
            &uint!(21888242871839275222246405745257275088548364400416034343698204186575808495616_U256),
            &uint!(3_U256));
        assert_eq!(result, uint!(0_U256));

        // -1 >= -2 => 1
        let result = u_gte(
            &uint!(21888242871839275222246405745257275088548364400416034343698204186575808495616_U256),
            &uint!(21888242871839275222246405745257275088548364400416034343698204186575808495615_U256));
        assert_eq!(result, uint!(1_U256));

        // -2 >= -1 => 0
        let result = u_gte(
            &uint!(21888242871839275222246405745257275088548364400416034343698204186575808495615_U256),
            &uint!(21888242871839275222246405745257275088548364400416034343698204186575808495616_U256));
        assert_eq!(result, uint!(0_U256));

        // -2 == -2 => 1
        let result = u_gte(
            &uint!(21888242871839275222246405745257275088548364400416034343698204186575808495615_U256),
            &uint!(21888242871839275222246405745257275088548364400416034343698204186575808495615_U256));
        assert_eq!(result, uint!(1_U256));
    }

    #[test]
    fn test_x() {
        let x = M.div(uint!(2_U256));

        println!("x: {:?}", x.as_limbs());
        println!("x: {}", M);
    }

    #[test]
    fn test_pow() {
        let a = uint!(21888242871839275222246405745257275088548364400416034343698204186575808495615_U256);
        let b = uint!(21888_U256);
        let c = Operation::Pow.eval(a, b);
        let want = uint!(6741803673964058984617537840767809723100020752467791363717299927390655464193_U256);
        assert_eq!(c, want);
    }

    #[test]
    fn test_bnot() {
        assert_eq!(
            uint!(7059779437489773633646340506914701874769131765994106666166191815402473914366_U256),
            UnoOperation::Bnot.eval(uint!(0_U256)));
        assert_eq!(
            uint!(7059779437489773633646340506914701874769131765994106666166191815400326430719_U256),
            UnoOperation::Bnot.eval(uint!(2147483647_U256)));
        assert_eq!(
            uint!(7059779437489773633646340506914701874769131765994106666166191815402473914367_U256),
            UnoOperation::Bnot.eval(uint!(21888242871839275222246405745257275088548364400416034343698204186575808495616_U256)));
        assert_eq!(
            uint!(7059779437489773633646340506914701874769131765994106666166191815401042258601_U256),
            UnoOperation::Bnot.eval(uint!(1431655765_U256)));
        assert_eq!(
            uint!(7059779437489773633646340506914701874769131765994106666166191815404191901285_U256),
            UnoOperation::Bnot.eval(uint!(21888242871839275222246405745257275088548364400416034343698204186574090508698_U256)));
        assert_eq!(
            uint!(0_U256),
            UnoOperation::Bnot.eval(uint!(115792089237316195423570985008687907853269984665640564039457584007913129639935_U256)));
        assert_eq!(
            uint!(19298681539552699237261830834781317975544997444273427339909597334652188273322_U256),
            UnoOperation::Bnot.eval(uint!(38597363079105398474523661669562635951089994888546854679819194669304376546645_U256)));
        assert_eq!(
            uint!(17368813385597429313535647751303186177990497699846084605918637601186969445990_U256),
            UnoOperation::Bnot.eval(uint!(69475253542389717254142591005212744711961990799384338423674550404747877783961_U256)));
        assert_eq!(
            uint!(16975279050329094783283862284904804026119806273934822715754654203603313563979_U256),
            UnoOperation::Bnot.eval(uint!(11972743258999954072608883967267172937197689892475318294109741798374968846004_U256)));
        assert_eq!(
            uint!(10364945975102880683525514432911402591886023268641012016029012611469420464237_U256),
            UnoOperation::Bnot.eval(uint!(18583076334226168172367231819260574371431472897769128993835383390508861945746_U256)));
        assert_eq!(
            uint!(4253782056457656234530291275605853130160190710592122558439987573692654305887_U256),
            UnoOperation::Bnot.eval(uint!(2805997381032117399116049231308848744608941055401984107726204241709819608479_U256)));
    }

    #[test]
    fn test_lnot() {
        assert_eq!(
            uint!(0_U256),
            UnoOperation::Lnot.eval(uint!(1_U256)));
        assert_eq!(
            uint!(1_U256),
            UnoOperation::Lnot.eval(uint!(0_U256)));
        assert_eq!(
            uint!(0_U256),
            UnoOperation::Lnot.eval(uint!(10944121435919637611123202872628637544274182200208017171849102093287904247808_U256)));
        assert_eq!(
            uint!(0_U256),
            UnoOperation::Lnot.eval(uint!(115792089237316195423570985008687907853269984665640564039457584007913129639935_U256)));
    }

    #[test]
    fn test_half() {
        // let h = M.div(U256::from(2));
        let h = M.wrapping_shr(1);
        type BN254 = ruint::Uint<254, 4>;

        let m = BN254::from_str_radix(
            "21888242871839275222246405745257275088548364400416034343698204186575808495617",
            10).unwrap();
        let a = BN254::from_str_radix(
            "18583076334226168172367231819260574371431472897769128993835383390508861945746",
            10).unwrap();
        let a = !a;
        // let mask = BN254::ZERO.not().shr(m.leading_zeros());
        // let a = a & mask;
        let a = if a >= m { a - m } else { a };

        let want = BN254::from_str_radix(
            "10364945975102880683525514432911402591886023268641012016029012611469420464237",
            10
        ).unwrap();
        assert_eq!(want, a);

        assert_eq!(h, halfM);
    }

    #[test]
    fn test_node_serialization() {
        let mut buf: [u8; 60] = [0xaa; 60];
        let x = Node::TresOp(TresOperation::TernCond, 1, 2, 3);
        x.write_bytes(&mut buf);

        let y = Node::from_bytes(&buf);
        assert_eq!(x, y);
        println!("{:?}", buf);
    }

    #[test]
    fn test_MMapNodes() {
        let x1 = Node::TresOp(TresOperation::TernCond, 1, 2, 3);
        let x2 = Node::Input(4);
        let mut nodes = MMapNodes::new(
            true, &std::env::temp_dir());
        nodes.push(x1);
        nodes.push(x2);
        let y1 = nodes.get(0).unwrap();
        assert_eq!(y1, x1);
        let y2 = nodes.get(1).unwrap();
        assert_eq!(y2, x2);

        assert!(nodes.get(3).is_none());

        assert_eq!(nodes.len(), 2);

        nodes.set(0, x2);
        let y2 = nodes.get(0).unwrap();
        assert_eq!(y2, x2);
        //
        // let mut v: Vec<Node> = Vec::new();
        // v.push(Node::Unknown);
        // let mut f: u64 = 1000000;
        // for i in 1..30 {
        //     f = f / 3 + f;
        //     println!("{}", indicatif::HumanCount(f))
        // }
    }

    #[test]
    fn test_MMapNodes_grow_named() {
        let mut nodes = MMapNodes::with_capacity(
            2, true, &std::env::temp_dir());
        assert_eq!(nodes.cap, 2 * Node::SIZE);
        assert_eq!(nodes.ln, 0);
        nodes.push(Node::TresOp(TresOperation::TernCond, 1, 2, 3));
        assert_eq!(nodes.cap, 2 * Node::SIZE);
        assert_eq!(nodes.ln, Node::SIZE);

        nodes.push(Node::TresOp(TresOperation::TernCond, 1, 2, 3));
        assert_eq!(nodes.cap, 2 * Node::SIZE);
        assert_eq!(nodes.ln, 2 * Node::SIZE);

        nodes.push(Node::TresOp(TresOperation::TernCond, 1, 2, 3));
        assert_eq!(nodes.cap, 1002 * Node::SIZE);
        assert_eq!(nodes.ln, 3 * Node::SIZE);
    }

    #[test]
    fn test_MMapNodes_grow_unnamed() {
        let mut nodes = MMapNodes::with_capacity(
            2, false, &std::env::temp_dir());
        assert_eq!(nodes.cap, 2 * Node::SIZE);
        assert_eq!(nodes.ln, 0);
        nodes.push(Node::TresOp(TresOperation::TernCond, 1, 2, 3));
        assert_eq!(nodes.cap, 2 * Node::SIZE);
        assert_eq!(nodes.ln, Node::SIZE);

        nodes.push(Node::TresOp(TresOperation::TernCond, 1, 2, 3));
        assert_eq!(nodes.cap, 2 * Node::SIZE);
        assert_eq!(nodes.ln, 2 * Node::SIZE);

        nodes.push(Node::TresOp(TresOperation::TernCond, 1, 2, 3));
        assert_eq!(nodes.cap, 1002 * Node::SIZE);
        assert_eq!(nodes.ln, 3 * Node::SIZE);
    }

    #[test]
    fn test_MMapNodes_retain() {
        let mut nodes = MMapNodes::new(
            true, &std::env::temp_dir());
        nodes.push(Node::TresOp(TresOperation::TernCond, 1, 2, 3));
        nodes.push(Node::TresOp(TresOperation::TernCond, 4, 5, 6));
        nodes.push(Node::TresOp(TresOperation::TernCond, 7, 8, 9));
        nodes.push(Node::TresOp(TresOperation::TernCond, 10, 11, 12));

        let used: Vec<bool> = vec![true, false, true, false];
        let mut ui = used.iter();
        nodes.retain(|| *ui.next().unwrap());
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes.cap, 4 * Node::SIZE);
        assert_eq!(nodes.ln, 2 * Node::SIZE);

        assert_eq!(
            nodes.get(0).unwrap(),
            Node::TresOp(TresOperation::TernCond, 1, 2, 3));
        assert_eq!(
            nodes.get(1).unwrap(),
            Node::TresOp(TresOperation::TernCond, 7, 8, 9));
    }

    mod bn254_montgomery {
        use super::*;
        use rand::rngs::StdRng;
        use rand::{Rng, SeedableRng};
        use crate::field::bn254_prime;

        /// Field-arithmetic and logical ops only: a graph built from these keeps results on the
        /// Montgomery fast path and must evaluate identically under both evaluators.
        const FIELD_OPS: [Operation; 13] = [
            Operation::Mul, Operation::Div, Operation::Add, Operation::Sub,
            Operation::Pow, Operation::Eq, Operation::Neq, Operation::Lt,
            Operation::Gt, Operation::Leq, Operation::Geq, Operation::Land,
            Operation::Lor,
        ];

        /// Representation-level binary ops.
        const REPRESENTATION_OPS: [Operation; 7] = [
            Operation::Idiv, Operation::Mod, Operation::Shl, Operation::Shr, Operation::Bor,
            Operation::Band, Operation::Bxor,
        ];

        const ALL_BINARY_OPS: [Operation; 20] = [
            Operation::Mul, Operation::Div, Operation::Add, Operation::Sub,
            Operation::Pow, Operation::Idiv, Operation::Mod, Operation::Eq,
            Operation::Neq, Operation::Lt, Operation::Gt, Operation::Leq,
            Operation::Geq, Operation::Land, Operation::Lor, Operation::Shl,
            Operation::Shr, Operation::Bor, Operation::Band, Operation::Bxor,
        ];

        const ALL_UNO_OPS: [UnoOperation; 5] = [
            UnoOperation::Neg, UnoOperation::Id, UnoOperation::Lnot,
            UnoOperation::Bnot, UnoOperation::Sqrt,
        ];

        /// Unary ops that keep a graph field-only (`Bnot` is representation-level).
        const FIELD_UNO_OPS: [UnoOperation; 4] = [
            UnoOperation::Neg, UnoOperation::Id, UnoOperation::Lnot, UnoOperation::Sqrt,
        ];

        /// Canonical operands chosen to exercise the edges of the value-semantic ops:
        /// zero, one, small ints, the 32-bit boundary, the half-prime sign boundary, and
        /// values just below the prime (the field's "negative" numbers).
        fn edge_values() -> Vec<U254> {
            let p = bn254_prime;
            vec![
                U254::from(0u64), U254::from(1u64), U254::from(2u64),
                U254::from(7u64), U254::from(256u64), U254::from(0xFFFF_FFFFu64),
                p >> 1, (p >> 1) + U254::from(1u64),
                p - U254::from(1u64), p - U254::from(2u64),
            ]
        }

        fn run_both(
            nodes: &VecNodes, inputs: &[U254], outputs: &[usize],
            constants: &[U254]) -> (Vec<U254>, Vec<U254>) {
            let ff = Field::new(bn254_prime);
            let base = evaluate(&ff, nodes, inputs, outputs, constants);
            let mont = evaluate_bn254(&ff, nodes, inputs, outputs, constants);
            (base, mont)
        }

        /// Every binary op, over a grid of canonical operand pairs, evaluated in isolation must
        /// match the baseline. This covers representation-level ops too: with canonical operands
        /// their results are canonical, so the two evaluators agree.
        #[test]
        fn binary_ops_match_baseline() {
            let values = edge_values();
            for op in ALL_BINARY_OPS {
                for &x in &values {
                    for &y in &values {
                        let mut nodes = VecNodes::default();
                        nodes.push(Node::Constant(0));
                        nodes.push(Node::Constant(1));
                        nodes.push(Node::Op(op, 0, 1));
                        let (base, mont) = run_both(&nodes, &[], &[2], &[x, y]);
                        assert_eq!(base, mont, "op {op:?} x={x} y={y}");
                    }
                }
            }
        }

        /// Every unary op over the grid of canonical operands must match the baseline.
        #[test]
        fn unary_ops_match_baseline() {
            let values = edge_values();
            for op in ALL_UNO_OPS {
                for &x in &values {
                    let mut nodes = VecNodes::default();
                    nodes.push(Node::Constant(0));
                    nodes.push(Node::UnoOp(op, 0));
                    let (base, mont) = run_both(&nodes, &[], &[1], &[x]);
                    assert_eq!(base, mont, "uno {op:?} x={x}");
                }
            }
        }

        /// The ternary conditional must select the same branch as the baseline.
        #[test]
        fn ternary_matches_baseline() {
            let values = edge_values();
            for &cond in &values {
                let mut nodes = VecNodes::default();
                nodes.push(Node::Constant(0));
                nodes.push(Node::Constant(1));
                nodes.push(Node::Constant(2));
                nodes.push(Node::TresOp(TresOperation::TernCond, 0, 1, 2));
                let (base, mont) =
                    run_both(&nodes, &[], &[3], &[cond, U254::from(11u64), U254::from(22u64)]);
                assert_eq!(base, mont, "terncond cond={cond}");
            }
        }

        /// Division, integer division, and modulo by zero return zero in both evaluators.
        #[test]
        fn by_zero_matches_baseline() {
            for op in [Operation::Div, Operation::Idiv, Operation::Mod] {
                let mut nodes = VecNodes::default();
                nodes.push(Node::Constant(0));
                nodes.push(Node::Constant(1));
                nodes.push(Node::Op(op, 0, 1));
                let (base, mont) =
                    run_both(&nodes, &[], &[2], &[U254::from(7u64), U254::from(0u64)]);
                assert_eq!(base, mont, "op {op:?} by zero");
                assert_eq!(mont[0], U254::from(0u64));
            }
        }

        #[test]
        fn mod_by_zero_constant_folds_to_zero() {
            let mut nodes = Nodes::new(bn254_prime, "bn128", VecNodes::new());
            let lhs = nodes.const_node_idx_from_value(U254::from(7u64));
            let zero = nodes.const_node_idx_from_value(U254::from(0u64));
            let folded = nodes.push(Node::Op(Operation::Mod, lhs, zero)).0;

            assert_eq!(folded, zero);
            assert_eq!(
                evaluate(&nodes.ff, &nodes.nodes, &[], &[folded], &nodes.constants),
                vec![U254::from(0u64)]);
        }

        /// `to_montgomery` reduces before converting, so a value that lands exactly on the
        /// prime maps to the field zero rather than panicking in `Fr::from_bigint`.
        #[test]
        fn boundary_reduces_before_convert() {
            let p = bn254_prime;
            assert_eq!(to_montgomery(p, p), to_montgomery(U254::from(0u64), p));
            assert_eq!(to_canonical(to_montgomery(p, p)), U254::from(0u64));
            assert_eq!(
                to_canonical(to_montgomery(p + U254::from(5u64), p)),
                U254::from(5u64));
            for v in edge_values() {
                assert_eq!(to_canonical(to_montgomery(v, p)), v);
            }
        }

        fn rand_canonical(rng: &mut StdRng) -> U254 {
            match rng.gen_range(0..6) {
                0 => U254::from(0u64),
                1 => U254::from(1u64),
                2 => U254::from(rng.gen::<u64>()),
                3 => bn254_prime - U254::from(1u64 + rng.gen::<u64>() % 1000),
                4 => bn254_prime >> 1,
                _ => {
                    let limbs = [
                        rng.gen::<u64>(), rng.gen::<u64>(), rng.gen::<u64>(),
                        rng.gen::<u64>() & 0x3FFF_FFFF_FFFF_FFFF,
                    ];
                    U254::from_limbs(limbs) % bn254_prime
                }
            }
        }

        /// Build a random graph. The op pools control which binary and unary ops the random
        /// nodes draw from, so the same generator builds both field-only graphs and
        /// representation-mixed graphs.
        fn random_graph(
            rng: &mut StdRng, binary_pool: &[Operation], uno_pool: &[UnoOperation],
        ) -> (VecNodes, Vec<U254>, Vec<U254>, Vec<usize>) {
            let n_consts = rng.gen_range(1..4);
            let n_inputs = rng.gen_range(1..4);
            let n_ops = rng.gen_range(4..40);

            let mut nodes = VecNodes::default();
            let constants: Vec<U254> = (0..n_consts).map(|_| rand_canonical(rng)).collect();
            let inputs: Vec<U254> = (0..n_inputs).map(|_| rand_canonical(rng)).collect();
            for c in 0..n_consts {
                nodes.push(Node::Constant(c));
            }
            for k in 0..n_inputs {
                nodes.push(Node::Input(k));
            }
            for _ in 0..n_ops {
                let len = nodes.len();
                let node = match rng.gen_range(0..3) {
                    0 => Node::Op(
                        binary_pool[rng.gen_range(0..binary_pool.len())],
                        rng.gen_range(0..len), rng.gen_range(0..len)),
                    1 => Node::UnoOp(
                        uno_pool[rng.gen_range(0..uno_pool.len())],
                        rng.gen_range(0..len)),
                    _ => Node::TresOp(
                        TresOperation::TernCond,
                        rng.gen_range(0..len), rng.gen_range(0..len), rng.gen_range(0..len)),
                };
                nodes.push(node);
            }

            let outputs: Vec<usize> = (0..nodes.len()).collect();
            (nodes, inputs, constants, outputs)
        }

        /// Field-only random graphs must reproduce the baseline witness exactly. Fuzzed over a
        /// broad space covering every field and logical op.
        #[test]
        fn field_only_graphs_match_baseline() {
            let mut rng = StdRng::seed_from_u64(0xB2254);
            for _ in 0..3000 {
                let (nodes, inputs, constants, outputs) =
                    random_graph(&mut rng, &FIELD_OPS, &FIELD_UNO_OPS);
                let (base, mont) = run_both(&nodes, &inputs, &outputs, &constants);
                assert_eq!(base, mont);
            }
        }

        /// Representation-level graphs are evaluated by the same universal bn254 evaluator.
        /// With canonical inputs, the evaluator should preserve byte-identical raw values while
        /// returning to Montgomery form for any canonical result.
        #[test]
        fn representation_graphs_match_baseline() {
            let mut rng = StdRng::seed_from_u64(0x9E3779B9);
            for _ in 0..3000 {
                let (nodes, inputs, constants, outputs) =
                    random_graph(&mut rng, &REPRESENTATION_OPS, &ALL_UNO_OPS);
                let (base, mont) = run_both(&nodes, &inputs, &outputs, &constants);
                assert_eq!(base, mont);
            }
        }

        #[test]
        fn shift_boundaries_match_baseline() {
            let p = bn254_prime;
            let cases = [
                U254::from(0u64),
                U254::from(1u64),
                U254::from(63u64),
                U254::from(64u64),
                U254::from(65u64),
                U254::from(127u64),
                U254::from(128u64),
                U254::from(129u64),
                U254::from(253u64),
                p - U254::from(1u64),
                p - U254::from(64u64),
            ];
            for op in [Operation::Shl, Operation::Shr] {
                for rhs in cases {
                    let mut nodes = VecNodes::default();
                    nodes.push(Node::Constant(0));
                    nodes.push(Node::Constant(1));
                    nodes.push(Node::Op(op, 0, 1));
                    let (base, mont) = run_both(
                        &nodes, &[], &[2],
                        &[U254::from(0x8000_0000_0000_0001u64), rhs]);
                    assert_eq!(base, mont, "op {op:?} rhs={rhs}");
                }
            }
        }

        #[test]
        fn shr_band_bit_extraction_chain_matches_baseline() {
            let p = bn254_prime;
            let mut nodes = VecNodes::default();
            let mut constants = vec![p - U254::from(1u64), U254::from(1u64)];
            nodes.push(Node::Constant(0));
            nodes.push(Node::Constant(1));
            let mut outputs = Vec::new();
            for bit in 0..32u64 {
                let shift_idx = constants.len();
                constants.push(U254::from(bit));
                nodes.push(Node::Constant(shift_idx));
                let shift_node = nodes.len();
                nodes.push(Node::Op(Operation::Shr, 0, shift_node - 1));
                let bit_node = nodes.len();
                nodes.push(Node::Op(Operation::Band, shift_node, 1));
                outputs.push(bit_node);
            }
            let (base, mont) = run_both(&nodes, &[], &outputs, &constants);
            assert_eq!(base, mont);
        }

        #[test]
        fn noncanonical_raw_value_is_preserved() {
            let p = bn254_prime;
            let mut nodes = VecNodes::default();
            nodes.push(Node::Input(0));
            nodes.push(Node::Constant(0));
            nodes.push(Node::Constant(1));
            nodes.push(Node::Op(Operation::Shl, 1, 2));
            nodes.push(Node::Op(Operation::Eq, 0, 3));
            nodes.push(Node::Op(Operation::Neq, 0, 3));

            let (base, mont) = run_both(
                &nodes, &[U254::from(0u64)], &[3, 4, 5],
                &[p, U254::from(0u64)]);
            assert_eq!(base, mont);
            assert_eq!(mont[0], p);
            assert_eq!(mont[1], U254::from(0u64));
            assert_eq!(mont[2], U254::from(1u64));
        }

        #[test]
        fn raw_rhs_subtraction_matches_baseline() {
            let p = bn254_prime;
            let mut nodes = VecNodes::default();
            nodes.push(Node::Constant(0));
            nodes.push(Node::Constant(1));
            nodes.push(Node::Op(Operation::Sub, 1, 0));

            let (base, mont) = run_both(
                &nodes, &[], &[2],
                &[p + U254::from(5u64), U254::from(7u64)]);
            assert_eq!(base, mont);
        }

        #[test]
        fn raw_values_feed_field_ops_match_baseline() {
            let p = bn254_prime;
            let cases = [
                (Operation::Mul, p, U254::from(7u64)),
                (Operation::Add, p, U254::from(7u64)),
                (Operation::Add, U254::from(7u64), p),
                (Operation::Sub, p + U254::from(5u64), U254::from(7u64)),
                (Operation::Sub, U254::from(7u64), p + U254::from(5u64)),
                (Operation::Pow, p + U254::from(5u64), U254::from(3u64)),
                (Operation::Eq, p, p),
                (Operation::Neq, p, U254::from(0u64)),
                (Operation::Lt, p + U254::from(5u64), U254::from(7u64)),
                (Operation::Geq, p + U254::from(5u64), U254::from(7u64)),
            ];
            for (op, lhs, rhs) in cases {
                let mut nodes = VecNodes::default();
                nodes.push(Node::Constant(0));
                nodes.push(Node::Constant(1));
                nodes.push(Node::Op(op, 0, 1));
                let (base, mont) = run_both(&nodes, &[], &[2], &[lhs, rhs]);
                assert_eq!(base, mont, "op {op:?} lhs={lhs} rhs={rhs}");
            }
        }

        #[test]
        fn raw_values_feed_unary_ops_match_baseline() {
            let p = bn254_prime;
            for op in ALL_UNO_OPS {
                for x in [p, p + U254::from(5u64)] {
                    let mut nodes = VecNodes::default();
                    nodes.push(Node::Constant(0));
                    nodes.push(Node::UnoOp(op, 0));
                    let (base, mont) = run_both(&nodes, &[], &[1], &[x]);
                    assert_eq!(base, mont, "uno {op:?} x={x}");
                }
            }
        }

        #[test]
        fn ternary_preserves_raw_selected_branch() {
            let p = bn254_prime;
            let mut nodes = VecNodes::default();
            nodes.push(Node::Constant(0));
            nodes.push(Node::Constant(1));
            nodes.push(Node::Constant(2));
            nodes.push(Node::TresOp(TresOperation::TernCond, 0, 1, 2));
            let (base, mont) = run_both(
                &nodes, &[], &[3],
                &[U254::from(0u64), U254::from(11u64), p]);
            assert_eq!(base, mont);
            assert_eq!(mont[0], p);
        }
    }
}
