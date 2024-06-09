#!/usr/bin/env bash

set -eu

ptau_url="https://storage.googleapis.com/zkevm/ptau/powersOfTau28_hez_final_08.ptau"

required_commands=(circom snarkjs curl cargo node cmp)

RED='\033[0;31m'
NC='\033[0m' # No Color

for cmd in "${required_commands[@]}"; do
  if ! command -v "$cmd" &> /dev/null; then
    echo -e "${RED}\`$cmd\` command could not be found${NC}"
    exit 1
  fi
done

workdir="$(pwd)/test_working_dir"

if [ ! -d "$workdir" ]; then
  echo "Creating working directory $workdir"
  mkdir "$workdir"
fi

ptau_path="${workdir}/$(basename $ptau_url)"
if [ ! -f "$ptau_path" ]; then
  echo "Downloading $ptau_url to $ptau_path"
  curl -L "$ptau_url" -o "$ptau_path"
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
echo "script dir ${script_dir}"

circomlib_path="${script_dir}/circomlib/circuits"
if [ ! -d "$circomlib_path" ]; then
  echo -e "${RED}circomlib not found at $circomlib_path${NC}"
  exit 1
fi

for circuit_path in "${script_dir}"/test_circuits/*.circom; do
  circuit_path=$(realpath "$circuit_path")
  echo "Running $circuit_path"
  circuit_name="$(basename "$circuit_path")" && circuit_name="${circuit_name%%.*}"
  inputs_path="$(dirname "$circuit_path")/${circuit_name}_inputs.json"
  if [ ! -f "$inputs_path" ]; then
    echo -e "${RED}Inputs file not found at $inputs_path${NC}"
    exit 1
  fi
  circuit_graph_path="${workdir}/${circuit_name}_graph.bin"
  witness_path="${workdir}/${circuit_name}.wtns"
  proof_path="${workdir}/${circuit_name}_proof.json"
  public_signals_path="${workdir}/${circuit_name}_public.json"

  # run commands from the project directory
  pushd "${script_dir}" > /dev/null
  cargo run --package witness --bin build-circuit "$circuit_path" "$circuit_graph_path" -l "$circomlib_path"
  cargo run --package witness --bin calc-witness "$circuit_graph_path" "$inputs_path" "$witness_path"
  popd > /dev/null

  # run commands from the working directory
  pushd "$workdir" > /dev/null

  circom -l "${circomlib_path}" --r1cs --wasm "$circuit_path"
  node "${circuit_name}"_js/generate_witness.js "${circuit_name}"_js/"${circuit_name}".wasm "${inputs_path}" "${witness_path}2"
  snarkjs wej "${witness_path}" "${witness_path}.json"
  snarkjs wej "${witness_path}2" "${witness_path}2.json"

  snarkjs wtns check "${circuit_name}".r1cs "${witness_path}"
  snarkjs wtns check "${circuit_name}".r1cs "${witness_path}2"
  if ! cmp -s "${witness_path}" "${witness_path}2"; then
    echo -e "${RED}Witnesses do not match${NC}"
    exit 1
  fi

  snarkjs groth16 setup "${circuit_name}".r1cs "$ptau_path" "${circuit_name}"_0000.zkey
  ENTROPY1=$(head -c 64 /dev/urandom | od -An -tx1 -v | tr -d ' \n')
  snarkjs zkey contribute "${circuit_name}"_0000.zkey "${circuit_name}"_final.zkey --name="1st Contribution" -v -e="$ENTROPY1"
  snarkjs zkey verify "${circuit_name}".r1cs "$ptau_path" "${circuit_name}"_final.zkey
  snarkjs zkey export verificationkey "${circuit_name}"_final.zkey "${circuit_name}"_verification_key.json
  # export witness as text ints
  # snarkjs wej witness.wtns witness.json

  snarkjs groth16 prove "${circuit_name}"_final.zkey "${witness_path}" "${proof_path}" "${public_signals_path}"
  snarkjs groth16 verify "${circuit_name}"_verification_key.json "${public_signals_path}" "${proof_path}"

  popd > /dev/null
done