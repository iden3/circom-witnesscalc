use std::{
    collections::HashMap,
    ops::{BitAnd, Shl, Shr},
};
use std::collections::HashSet;

use crate::field::M;
use ark_bn254::Fr;
use ark_ff::{BigInt, Field, PrimeField, BigInteger, Zero, One};
use rand::Rng;
use ruint::aliases::U256;
use serde::{Deserialize, Serialize};

use ark_serialize::{CanonicalDeserialize, CanonicalSerialize, Compress, Validate};

fn ark_se<S, A: CanonicalSerialize>(a: &A, s: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let mut bytes = vec![];
    a.serialize_with_mode(&mut bytes, Compress::Yes)
        .map_err(serde::ser::Error::custom)?;
    s.serialize_bytes(&bytes)
}

fn ark_de<'de, D, A: CanonicalDeserialize>(data: D) -> Result<A, D::Error>
where
    D: serde::de::Deserializer<'de>,
{
    let s: Vec<u8> = serde::de::Deserialize::deserialize(data)?;
    let a = A::deserialize_with_mode(s.as_slice(), Compress::Yes, Validate::Yes);
    a.map_err(serde::de::Error::custom)
}

#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Operation {
    Mul,
    MMul,
    Add,
    Sub,
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
    Band,
    Neg,
    Div,
    Idiv,
    TernCond,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Node {
    Input(usize),
    Constant(U256),
    #[serde(serialize_with = "ark_se", deserialize_with = "ark_de")]
    MontConstant(Fr),
    UnoOp(Operation, usize),
    Op(Operation, usize, usize),
    TresOp(Operation, usize, usize, usize),
}

impl Operation {
    pub fn eval(&self, a: U256, b: U256) -> U256 {
        use Operation::*;
        match self {
            Add => a.add_mod(b, M),
            Sub => a.add_mod(M - b, M),
            Mul => a.mul_mod(b, M),
            Eq => U256::from(a == b),
            Neq => U256::from(a != b),
            Lt => U256::from(a < b),
            Gt => U256::from(a > b),
            Leq => U256::from(a <= b),
            Geq => U256::from(a >= b),
            Land => U256::from(a != U256::ZERO && b != U256::ZERO),
            Lor => U256::from(a != U256::ZERO || b != U256::ZERO),
            Shl => compute_shl_uint(a, b),
            Shr => compute_shr_uint(a, b),
            Band => a.bitand(b),
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
            Idiv => a / b,
            _ => unimplemented!("operator {:?} not implemented", self),
        }
    }

    pub fn eval_uno(&self, a: U256) -> U256 {
        match self {
            Operation::Neg => if a == U256::ZERO { U256::ZERO } else { M - a },
            _ => unimplemented!("operator {:?} not implemented for UNO operation", self),
        }
    }

    pub fn eval_tres(&self, a: U256, b: U256, c: U256) -> U256 {
        match self {
            Operation::TernCond => if a == U256::ZERO { c } else { b },
            _ => unimplemented!("operator {:?} not implemented for TRES operation", self),
        }
    }

    pub fn eval_fr(&self, a: Fr, b: Fr) -> Fr {
        use Operation::*;
        match self {
            Add => a + b,
            Sub => a - b,
            Mul => a * b,
            Shr => shr(a, b),
            Band => bit_and(a, b),
            Div => if b.is_zero() { Fr::zero() } else { a / b },
            // We always should return something on the circuit execution.
            // So in case of division by 0 we would return 0. And the proof
            // should be invalid in the end.
            Neq => {
                match a.cmp(&b) {
                    std::cmp::Ordering::Equal => Fr::zero(),
                    _ => Fr::one(),
                }
            },
            _ => unimplemented!("operator {:?} not implemented for Montgomery", self),
        }
    }

    pub fn eval_fr_uno(&self, a: Fr) -> Fr {
        match self {
            Operation::Neg => if a.is_zero() { Fr::zero() } else {
                let mut x = Fr::MODULUS;
                x.sub_with_borrow(&a.into_bigint());
                Fr::from_bigint(x).unwrap()
            },
            _ => unimplemented!("operator {:?} not implemented for UNO operation", self),
        }
    }

    pub fn eval_fr_tres(&self, a: Fr, b: Fr, c: Fr) -> Fr {
        match self {
            Operation::TernCond => if a.is_zero() { c } else { b },
            _ => unimplemented!("operator {:?} not implemented for TRES operation", self),
        }
    }
}

fn compute_shl_uint(a: U256, b: U256) -> U256 {
    debug_assert!(b.lt(&U256::from(256)));
    let ls_limb = b.as_limbs()[0];
    a.shl(ls_limb as usize)
}

fn compute_shr_uint(a: U256, b: U256) -> U256 {
    debug_assert!(b.lt(&U256::from(256)));
    let ls_limb = b.as_limbs()[0];
    a.shr(ls_limb as usize)
}

/// All references must be backwards.
fn assert_valid(nodes: &[Node]) {
    for (i, &node) in nodes.iter().enumerate() {
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

pub fn optimize(nodes: &mut Vec<Node>, outputs: &mut [usize]) {
    tree_shake(nodes, outputs);
    propagate(nodes);
    value_numbering(nodes, outputs);
    constants(nodes);
    tree_shake(nodes, outputs);
    montgomery_form(nodes);
}

pub fn evaluate(nodes: &[Node], inputs: &[U256], outputs: &[usize]) -> Vec<U256> {
    // assert_valid(nodes);

    // Evaluate the graph.
    let mut values = Vec::with_capacity(nodes.len());
    for (_, &node) in nodes.iter().enumerate() {
        let value = match node {
            Node::Constant(c) => Fr::new(c.into()),
            Node::MontConstant(c) => c,
            Node::Input(i) => Fr::new(inputs[i].into()),
            Node::Op(op, a, b) => op.eval_fr(values[a], values[b]),
            Node::UnoOp(op, a) => op.eval_fr_uno(values[a]),
            Node::TresOp(op, a, b, c) => op.eval_fr_tres(values[a], values[b], values[c]),
        };
        values.push(value);
    }

    // Convert from Montgomery form and return the outputs.
    let mut out = vec![U256::ZERO; outputs.len()];
    for i in 0..outputs.len() {
        out[i] = U256::try_from(values[outputs[i]].into_bigint()).unwrap();
    }

    // Trace the calculation of the signal
    // println!("output 1 signal {}", outputs[1]);
    // trace_signal(outputs[1], nodes, &values);

    out
}

fn trace_signal_with_seen(i: usize, nodes: &[Node], values: &Vec<Fr>,
                          seen: &mut HashSet<usize>) {

    if seen.contains(&i) {
        println!("at [{}]: cycle detected", i);
        return;
    }

    seen.insert(i);

    match nodes[i] {
        Node::Input(a) => {
            println!("at [{}]: input({}): {}", i, a, values[i].to_string());
        },
        Node::Constant(a) => {
            println!("at [{}]: constant {}", i, a.to_string());
        },
        Node::MontConstant(a) => {
            println!("at [{}]: montgomery constant {}", i, a.into_bigint().to_string());
        },
        Node::Op(op, a, b) => {
            println!("at [{}]: operation {:?} between [{}] ({}) and [{}] ({}): {}",
                     i, op, a, values[a].to_string(), b, values[b].to_string(),
                     values[i].to_string());
            trace_signal_with_seen(a, nodes, values, seen);
            trace_signal_with_seen(b, nodes, values, seen);
        },
        Node::UnoOp(op, a) => {
            println!("at [{}]: unary operation {:?} on [{}] ({}): {}",
                     i, op, a, values[a].to_string(), values[i].to_string());
            trace_signal_with_seen(a, nodes, values, seen);
        },
        Node::TresOp(op, a, b, c) => {
            println!(
                "at [{}]: tres operation {:?} on [{}] ({}), [{}] ({}) and [{}] ({}): {}",
                i, op, a, values[a].to_string(), b, values[b].to_string(),
                c, values[c].to_string(), values[i].to_string());
            trace_signal_with_seen(a, nodes, values, seen);
        },
    }
}

pub fn trace_signal(i: usize, nodes: &[Node], values: &Vec<Fr>) {
    let mut seen: HashSet<usize> = HashSet::new();
    trace_signal_with_seen(i, &nodes, &values, &mut seen);
}

/// Constant propagation
pub fn propagate(nodes: &mut [Node]) {
    assert_valid(nodes);
    let mut constants = 0_usize;
    for i in 0..nodes.len() {
        if let Node::Op(op, a, b) = nodes[i] {
            if let (Node::Constant(va), Node::Constant(vb)) = (nodes[a], nodes[b]) {
                nodes[i] = Node::Constant(op.eval(va, vb));
                constants += 1;
            } else if a == b {
                // Not constant but equal
                use Operation::*;
                if let Some(c) = match op {
                    Eq | Leq | Geq => Some(true),
                    Neq | Lt | Gt => Some(false),
                    _ => None,
                } {
                    nodes[i] = Node::Constant(U256::from(c));
                    constants += 1;
                }
            }
        } else if let Node::UnoOp(op, a) = nodes[i] {
            if let Node::Constant(va) = nodes[a] {
                nodes[i] = Node::Constant(op.eval_uno(va));
                constants += 1;
            }
        } else if let Node::TresOp(op, a, b, c) = nodes[i] {
            if let (Node::Constant(va), Node::Constant(vb), Node::Constant(vc)) = (nodes[a], nodes[b], nodes[c]) {
                nodes[i] = Node::Constant(op.eval_tres(va, vb, vc));
                constants += 1;
            }
        }
    }

    eprintln!("Propagated {constants} constants");
}

/// Remove unused nodes
pub fn tree_shake(nodes: &mut Vec<Node>, outputs: &mut [usize]) {
    assert_valid(nodes);

    // Mark all nodes that are used.
    let mut used = vec![false; nodes.len()];
    for &i in outputs.iter() {
        used[i] = true;
    }

    // Work backwards from end as all references are backwards.
    for i in (0..nodes.len()).rev() {
        if used[i] {
            if let Node::Op(_, a, b) = nodes[i] {
                used[a] = true;
                used[b] = true;
            }
            if let Node::UnoOp(_, a) = nodes[i] {
                used[a] = true;
            }
            if let Node::TresOp(_, a, b, c) = nodes[i] {
                used[a] = true;
                used[b] = true;
                used[c] = true;
            }
        }
    }

    // Remove unused nodes
    let n = nodes.len();
    let mut retain = used.iter();
    nodes.retain(|_| *retain.next().unwrap());
    let removed = n - nodes.len();

    // Renumber references.
    let mut renumber = vec![None; n];
    let mut index = 0;
    for (i, &used) in used.iter().enumerate() {
        if used {
            renumber[i] = Some(index);
            index += 1;
        }
    }
    assert_eq!(index, nodes.len());
    for (&used, renumber) in used.iter().zip(renumber.iter()) {
        assert_eq!(used, renumber.is_some());
    }

    // Renumber references.
    for node in nodes.iter_mut() {
        if let Node::Op(_, a, b) = node {
            *a = renumber[*a].unwrap();
            *b = renumber[*b].unwrap();
        }
        if let Node::UnoOp(_, a) = node {
            *a = renumber[*a].unwrap();
        }
        if let Node::TresOp(_, a, b, c) = node {
            *a = renumber[*a].unwrap();
            *b = renumber[*b].unwrap();
            *c = renumber[*c].unwrap();
        }
    }
    for output in outputs.iter_mut() {
        *output = renumber[*output].unwrap();
    }

    eprintln!("Removed {removed} unused nodes");
}

/// Randomly evaluate the graph
fn random_eval(nodes: &mut Vec<Node>) -> Vec<U256> {
    let mut rng = rand::thread_rng();
    let mut values = Vec::with_capacity(nodes.len());
    let mut inputs = HashMap::new();
    let mut prfs = HashMap::new();
    let mut prfs_uno = HashMap::new();
    let mut prfs_tres = HashMap::new();
    for node in nodes.iter() {
        use Operation::*;
        let value = match node {
            // Constants evaluate to themselves
            Node::Constant(c) => *c,

            Node::MontConstant(c) => unimplemented!("should not be used"),

            // Algebraic Ops are evaluated directly
            // Since the field is large, by Swartz-Zippel if
            // two values are the same then they are likely algebraically equal.
            Node::Op(op @ (Add | Sub | Mul), a, b) => op.eval(values[*a], values[*b]),

            // Input and non-algebraic ops are random functions
            // TODO: https://github.com/recmo/uint/issues/95 and use .gen_range(..M)
            Node::Input(i) => *inputs.entry(*i).or_insert_with(|| rng.gen::<U256>() % M),
            Node::Op(op, a, b) => *prfs
                .entry((*op, values[*a], values[*b]))
                .or_insert_with(|| rng.gen::<U256>() % M),
            Node::UnoOp(op, a) => *prfs_uno
                .entry((*op, values[*a]))
                .or_insert_with(|| rng.gen::<U256>() % M),
            Node::TresOp(op, a, b, c) => *prfs_tres
                .entry((*op, values[*a], values[*b], values[*c]))
                .or_insert_with(|| rng.gen::<U256>() % M),
        };
        values.push(value);
    }
    values
}

/// Value numbering
pub fn value_numbering(nodes: &mut Vec<Node>, outputs: &mut [usize]) {
    assert_valid(nodes);

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

    // Renumber references.
    for node in nodes.iter_mut() {
        if let Node::Op(_, a, b) = node {
            *a = renumber[*a];
            *b = renumber[*b];
        }
        if let Node::UnoOp(_, a) = node {
            *a = renumber[*a];
        }
        if let Node::TresOp(_, a, b, c) = node {
            *a = renumber[*a];
            *b = renumber[*b];
            *c = renumber[*c];
        }
    }
    for output in outputs.iter_mut() {
        *output = renumber[*output];
    }

    eprintln!("Global value numbering applied");
}

/// Probabilistic constant determination
pub fn constants(nodes: &mut Vec<Node>) {
    assert_valid(nodes);

    // Evaluate the graph in random field elements.
    let values_a = random_eval(nodes);
    let values_b = random_eval(nodes);

    // Find all nodes with the same value.
    let mut constants = 0;
    for i in 0..nodes.len() {
        if let Node::Constant(_) = nodes[i] {
            continue;
        }
        if values_a[i] == values_b[i] {
            nodes[i] = Node::Constant(values_a[i]);
            constants += 1;
        }
    }
    eprintln!("Found {} constants", constants);
}

/// Convert to Montgomery form
pub fn montgomery_form(nodes: &mut [Node]) {
    for node in nodes.iter_mut() {
        use Node::*;
        use Operation::*;
        match node {
            Constant(c) => *node = MontConstant(Fr::new((*c).into())),
            MontConstant(..) => (),
            Input(..) => (),
            Op(Add | Sub | Mul | Shr | Band | Div | Neq, ..) => (),
            Op(op, ..) => unimplemented!("Operators Montgomery form: {:?}", op),
            UnoOp(Neg, ..) => (),
            UnoOp(op, ..) => unimplemented!("Operators Montgomery form UNO: {:?}", op),
            TresOp(TernCond, ..) => (),
            TresOp(op, ..) => unimplemented!("Operators Montgomery form TRES: {:?}", op),
        }
    }
    eprintln!("Converted to Montgomery form");
}

fn shr(a: Fr, b: Fr) -> Fr {
    if b.is_zero() {
        return a;
    }

    match b.cmp(&Fr::from(254u64)) {
        std::cmp::Ordering::Equal  => {return Fr::zero()},
        std::cmp::Ordering::Greater => {return Fr::zero()},
        _ => (),
    };

    let mut n = b.into_bigint().to_bytes_le()[0];
    let mut result = a.into_bigint();
    let c = result.as_mut();
    while n >= 64 {
        for i in 0..3 {
            c[i as usize] = c[(i + 1) as usize];
        }
        c[3] = 0;
        n -= 64;
    }

    if n == 0 {
        return Fr::from_bigint(result).unwrap();
    }

    let mask:u64 = (1<<n) - 1;
    let mut carrier: u64 = c[3] & mask;
    c[3] >>= n;
    for i in (0..3).rev() {
        let new_carrier = c[i] & mask;
        c[i] = (c[i] >> n) | (carrier << (64 - n));
        carrier = new_carrier;
    }
    Fr::from_bigint(result).unwrap()
}

fn bit_and(a: Fr, b: Fr) -> Fr {
    let a = a.into_bigint();
    let b = b.into_bigint();
    let mut c: [u64; 4] = [0; 4];
    for i in 0..4 {
        c[i] = a.0[i] & b.0[i];
    }
    let mut d: BigInt<4> = BigInt::new(c);
    if d > Fr::MODULUS {
        d.sub_with_borrow(&Fr::MODULUS);
    }

    Fr::from_bigint(d).unwrap()
}