#!/usr/bin/env sh
set -eu

cargo fmt --check
cargo test --workspace
cargo run -p cove-conformance --bin gen-corpus -- --check
cargo run -p cove-conformance --bin gen-capability-matrix -- --check
cargo run -p cove-conformance --bin cove-conformance -- conformance/