#!/usr/bin/env sh
set -eu

cargo fmt --check
sh scripts/check-m0-boundaries.sh
cargo test --workspace
cargo test -p cove-convert-parquet
cargo run -p cove-bench --bin cove-bench > /dev/null
cargo run -p cove-conformance --bin gen-corpus -- --check
cargo run -p cove-conformance --bin gen-capability-matrix -- --check
cargo run -p cove-conformance --bin cove-conformance -- conformance/