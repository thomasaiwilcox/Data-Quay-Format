#!/usr/bin/env sh
set -eu

fail() {
    echo "m0 boundary check failed: $*" >&2
    exit 1
}

if grep -nE '^[[:space:]]*(arrow-(array|buffer|schema)|parquet|datafusion|cove-arrow)[[:space:]]*=' crates/cove-core/Cargo.toml; then
    fail "cove-core must not depend on Arrow, Parquet, DataFusion, or cove-arrow"
fi

if grep -RInE 'arrow_array|arrow_buffer|arrow_schema|parquet::|datafusion::|use[[:space:]]+datafusion' crates/cove-core/src; then
    fail "cove-core source must not import Arrow, Parquet, or DataFusion crates"
fi

if [ -e crates/cove-core/src/interop/arrow.rs ] || [ -e crates/cove-core/src/interop/parquet.rs ]; then
    fail "Arrow and Parquet interop source must live in cove-arrow, not cove-core"
fi

[ -f crates/cove-arrow/src/arrow.rs ] || fail "missing cove-arrow Arrow interop module"
[ -f crates/cove-arrow/src/parquet.rs ] || fail "missing cove-arrow Parquet interop module"

if grep -RInE '(arrow-(array|buffer|schema)[^"]*"54"|parquet[^"]*"54")' Cargo.toml crates/*/Cargo.toml; then
    fail "Arrow and Parquet consumers must be on the Arrow 58 line"
fi

if grep -RInE '^[[:space:]]*datafusion[[:space:]]*=' Cargo.toml crates/*/Cargo.toml | grep -v '^crates/cove-datafusion/Cargo.toml:'; then
    fail "DataFusion dependency must be isolated to cove-datafusion"
fi

if grep -RInE 'datafusion::|use[[:space:]]+datafusion' crates/*/src | grep -vE '^crates/cove-datafusion/src/(adapter_v53/|register\.rs:)'; then
    fail "DataFusion imports must stay in cove-datafusion adapter_v53 or register"
fi
