use std::cmp::Ordering;

use ark_bn254::Fr;
use ark_ff::{BigInt, Field as ArkField, One, PrimeField, Zero};

use super::{Node, NodesStorage, Operation, TresOperation, UnoOperation};
use crate::field::{bn254_prime, Field, FieldOperations, U254};

/// Montgomery form `Fr` to its canonical `U254` residue in `[0, prime)`.
#[inline(always)]
pub(super) fn to_canonical(v: Fr) -> U254 {
    U254::from_limbs(v.into_bigint().0)
}

/// `U254` to Montgomery form `Fr`, reducing modulo the prime first so the conversion is total:
/// `Fr::from_bigint` rejects any value `>= prime`. Callers that must preserve a non-canonical
/// raw representation should store it as [`Bn254Value::Raw`] instead. `prime` must be the
/// bn254 modulus; `evaluate_bn254` asserts that precondition.
#[inline(always)]
pub(super) fn to_montgomery(v: U254, prime: U254) -> Fr {
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
/// byte-identical to the value produced by [`super::evaluate`], while the common all-field hot
/// path stays in Montgomery form.
pub fn evaluate_bn254<NS: NodesStorage>(
    ff: &Field<U254>, nodes: &NS, inputs: &[U254],
    outputs: &[usize], constants: &[U254]) -> Vec<U254>
{
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

#[cfg(test)]
mod tests {
    use super::{evaluate_bn254, to_canonical, to_montgomery};
    use super::super::{
        evaluate, Node, Nodes, NodesInterface, NodesStorage, Operation, TresOperation,
        UnoOperation, VecNodes,
    };
    use crate::field::{bn254_prime, Field, U254};
    use rand::rngs::StdRng;
    use rand::{Rng, SeedableRng};

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
