//! A/B benchmark of the canonical-ruint [`evaluate`] against the Montgomery-resident
//! [`evaluate_bn254`] on two synthetic graphs: a field-only arithmetic hash shape
//! (Poseidon/Merkle-like), and a mixed graph that interleaves `Shr`/`Band` bit extraction with
//! field multiplies and adds. Run with `cargo bench --bench evaluate`.
//!
//! The win comes from the hot field arithmetic: a multiply becomes a single Montgomery
//! multiply and add/sub/neg are conversion-free. On x86, building with
//! `RUSTFLAGS="-C target-cpu=x86-64-v3"` (or `native`) enables ark-ff's ADX/MULX assembly and
//! widens the gap further; it is inert on arm64.

use circom_witnesscalc::field::{bn254_prime, Field, U254};
use circom_witnesscalc::graph::{
    evaluate, evaluate_bn254, Node, NodesStorage, Operation, VecNodes,
};
use criterion::{black_box, criterion_group, criterion_main, Criterion};

/// Build a field-only graph of `n` nodes: two inputs, two constants, then a chain where each
/// node combines the previous two. The op mix is multiply-dominated to mirror an arithmetic
/// hash, whose cost is concentrated in S-box and MDS-matrix multiplies, with periodic adds
/// and subtracts standing in for the linear layers.
fn build_field_graph(n: usize) -> (VecNodes, Vec<U254>, Vec<U254>, Vec<usize>) {
    let mut nodes = VecNodes::default();
    nodes.push(Node::Input(0));
    nodes.push(Node::Input(1));
    nodes.push(Node::Constant(0));
    nodes.push(Node::Constant(1));
    for i in 4..n {
        let op = match i % 4 {
            2 => Operation::Add,
            3 => Operation::Sub,
            _ => Operation::Mul,
        };
        nodes.push(Node::Op(op, i - 1, i - 2));
    }
    let inputs = vec![U254::from(123u64), U254::from(456u64)];
    let constants = vec![U254::from(7u64), U254::from(13u64)];
    let outputs: Vec<usize> = (n - 16..n).collect();
    (nodes, inputs, constants, outputs)
}

/// Build a graph with alternating bit extraction and field arithmetic. This exercises the
/// evaluator paths that materialize raw `U254` views for `Shr`/`Band`, then return to
/// Montgomery-resident field ops for `Add`/`Mul`.
fn build_mixed_graph(rounds: usize) -> (VecNodes, Vec<U254>, Vec<U254>, Vec<usize>) {
    let mut nodes = VecNodes::default();
    nodes.push(Node::Input(0));
    nodes.push(Node::Input(1));
    nodes.push(Node::Constant(0));
    let one = 2;
    let mut acc = 0;
    let mut prev = 1;
    for _ in 0..rounds {
        let shifted = nodes.len();
        nodes.push(Node::Op(Operation::Shr, prev, one));
        let bit = nodes.len();
        nodes.push(Node::Op(Operation::Band, shifted, one));
        let mixed = nodes.len();
        nodes.push(Node::Op(Operation::Add, acc, bit));
        let product = nodes.len();
        nodes.push(Node::Op(Operation::Mul, mixed, prev));
        acc = product;
        prev = mixed;
    }
    let inputs = vec![U254::from(123u64), U254::from(456u64)];
    let constants = vec![U254::from(1u64)];
    let outputs: Vec<usize> = (nodes.len() - 16..nodes.len()).collect();
    (nodes, inputs, constants, outputs)
}

fn bench_graph(
    c: &mut Criterion,
    ff: &Field<U254>,
    name: &str,
    graph: &(VecNodes, Vec<U254>, Vec<U254>, Vec<usize>),
) {
    let (nodes, inputs, constants, outputs) = graph;
    assert_eq!(
        evaluate(ff, nodes, inputs, outputs, constants),
        evaluate_bn254(ff, nodes, inputs, outputs, constants)
    );

    let mut group = c.benchmark_group(name);
    group.bench_function("canonical-ruint", |b| {
        b.iter(|| evaluate(ff, nodes, black_box(inputs), outputs, constants));
    });
    group.bench_function("montgomery-resident", |b| {
        b.iter(|| evaluate_bn254(ff, nodes, black_box(inputs), outputs, constants));
    });
    group.finish();
}

fn bench_evaluate(c: &mut Criterion) {
    let ff = Field::new(bn254_prime);
    bench_graph(
        c,
        &ff,
        "evaluate/300k-field-nodes",
        &build_field_graph(300_000),
    );
    bench_graph(
        c,
        &ff,
        "evaluate/300k-mixed-shr-band-nodes",
        &build_mixed_graph(75_000),
    );
}

criterion_group!(benches, bench_evaluate);
criterion_main!(benches);
