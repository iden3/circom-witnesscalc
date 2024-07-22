# 🏎️ circom-witness-rs

## Description

This crate provides a fast witness generator for Circom circuits, serving as a drop-in replacement for Circom's witness generator. It was created in response to the slow performance of Circom's WASM generator for larger circuits, which also necessitates a WASM runtime, often a cumbersome requirement. The native C++ generator, though faster, depends on x86 assembly for field operations, rendering it impractical for use on other platforms (e.g., cross-compiling to ARM for mobile devices).

`circom-witness-rs` comes with two modes:

1. Build a Circom circuit using `build-circuit` command. As a result, you will get a binary file of graph operations to calculate the witness for a circuit.
2. Using the generated bin file and inputs for the circuit, generate a witness using `calc-witness` command or a library function.

## Usage

See this [example project](https://github.com/philsippl/semaphore-witness-example) for Semaphore with more details on building. 

See `semaphore-rs` for an [example at runtime](https://github.com/worldcoin/semaphore-rs/blob/62f556bdc1a2a25021dcccc97af4dfa522ab5789/src/protocol/mod.rs#L161-L163).

All of those example were used with `circom compiler 2.1.6` ([dcf7d68](https://github.com/iden3/circom/tree/dcf7d687a81c6d9b3e3840181fd83cdaf5f4ac05)). Using a different version of circom might cause issues due to different c++ code being generated.

## Benchmarks

### [semaphore-rs](https://github.com/worldcoin/semaphore-rs/tree/main)
**TLDR: For semaphore circuit (depth 30) `circom-witness-rs` is ~25x faster than wasm and ~10x faster than native c++ version.**
```
cargo bench --bench=criterion --features=bench,depth_30
```

With `circom-witness-rs`:q
```
witness_30              time:   [993.84 µs 996.62 µs 999.42 µs]
```

With wasm witness generator from [`circom-compat`](https://github.com/arkworks-rs/circom-compat/blob/master/src/witness/witness_calculator.rs):
```
witness_30              time:   [24.630 ms 24.693 ms 24.759 ms]
```

With native c++ witness generator from circom: `9.640ms`

As a nice side effect of the graph optimizations, the binary size is also reduced heavily. In the example of Semaphore the binary size is reduced from `1.3MB` (`semaphore.wasm`) to `350KB` (`graph.bin`). 

## Unimplemented features

There are still quite a few missing operations that need to be implemented. The list of supported and unsupported operations can be found here. Support for the missing operations is very straighfoward and will be added in the future.
https://github.com/philsippl/circom-witness-rs/blob/e889cedde49a8929812b825aede55d9668118302/src/generate.rs#L61-L89

## Build witness from intermediate representation

To create a circuit graph file from a Circom 2 program, run the following command:

```shell
cargo run --package witness --bin build-circuit <path_to_circuit.circom> <path_to_circuit_graph.bin> [-l <path_to_circom_libs/>]* [-i <inputs_file.json>] [-print-unoptimized]
```

Optional flags:

* `-l <path_to_circom_libs/>` - Path to the circomlib directory. This flag can be used multiple times.
* `-i <inputs_file.json>` - Path to the inputs file. If provided, the inputs will be used to generate the witness. Otherwise, inputs will be set to 0.
* `-print-unoptimized` - Evaluate the graph with provided or default inputs and print it to stdout (useful for debugging).

## Calculate witness from circuit graph created on previous step

> Note: In the inputs file all values of signals should be arrays. Even if they have a single value.

To generate a witness file from a circuit graph and inputs, run the following command.

```shell
cargo run --package witness --bin calc-witness <path_to_circuit_graph.bin> <path_to_inputs.json> <path_to_output_witness.wtns>
```

## Run circuits tests

To run circuits tests, first get the iden3/circomlib repository

```shell
git clone git@github.com:iden3/circomlib.git
```
circom snarkjs curl cargo node cmp
Also, you need to have the following commands installed: `circom`, `snarkjs`,
`curl`, `cargo`, `node` and `cmp`.

Now run the `test_circuits.sh` script.

```shell
./test_circuits.sh
```