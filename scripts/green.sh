#!/usr/bin/env bash
# The merge gate: format, lints, tests. Last stdout line is the verdict.
set -uo pipefail
cd "$(dirname "$0")/.."
mkdir -p .green
exec > >(tee .green/last.log) 2>&1

phase() {
  local name=$1
  shift
  echo "== $name"
  "$@" || { echo "RED phase=$name"; exit 1; }
}

if [ ! -f Cargo.toml ]; then
  echo "GREEN passed=0"
  exit 0
fi
phase fmt cargo fmt --check
phase clippy cargo clippy --workspace --all-targets --locked -- -D warnings
echo "== test"
cargo test --workspace --locked 2>&1 | tee .green/test.log || { echo "RED phase=test"; exit 1; }
passed=$(grep -o '[0-9]* passed' .green/test.log | awk '{s+=$1} END {print s+0}')
echo "GREEN passed=$passed"
