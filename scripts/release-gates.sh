#!/usr/bin/env sh
set -eu

cargo fmt --check
cargo test --workspace
cargo run -p qf-conformance --bin gen-corpus -- --check
cargo run -p qf-conformance --bin gen-capability-matrix -- --check
cargo run -p qf-conformance --bin qf-conformance -- conformance/