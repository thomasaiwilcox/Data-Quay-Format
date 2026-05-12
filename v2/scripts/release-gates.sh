#!/usr/bin/env sh
set -eu

cargo fmt --check
sh scripts/check-m0-boundaries.sh
cargo test --workspace
cargo run -p cove-codec --bin cove-codec-validate -- conformance/codecs/codec_descriptor_valid.bin > /dev/null
cargo run -p cove-codec --bin cove-codec-validate -- conformance/codecs/registered_encoding_envelope_valid.bin > /dev/null
cargo run -p cove-layout --bin cove-layout-inspect -- conformance/layout/layout_plan_valid.bin > /dev/null
cargo run -p cove-layout --bin cove-layout-inspect -- conformance/layout/scan_split_index_valid.bin > /dev/null
cargo run -p cove-layout --bin cove-layout-inspect -- conformance/zerocopy/zero_copy_map_valid.bin > /dev/null
cargo run -p cove-runtime --bin cove-runtime-inspect -- conformance/runtime/runtime_hint_valid.bin > /dev/null
cargo run -p cove-coverage --bin cove-coverage-inspect -- conformance/coverage/coverage_set_valid.bin > /dev/null
cargo run -p cove-coverage --bin cove-coverage-inspect -- conformance/coverage/provider_registry_valid.bin > /dev/null
cargo run -p cove-coverage --bin cove-coverage-inspect -- conformance/coverage/coverage_proof_record_valid.bin > /dev/null
cargo run -p cove-coverage --bin cove-coverage-inspect -- conformance/coverage/predicate_normal_form_valid.bin > /dev/null
cargo run -p cove-coverage --bin cove-coverage-inspect -- conformance/coverage/interval_predicate_valid.bin > /dev/null
cargo run -p cove-coverage --bin cove-coverage-inspect -- conformance/coverage/coverage_plan_candidate_valid.bin > /dev/null
cargo run -p cove-index --bin cove-index-inspect -- conformance/covi/empty_valid.covi > /dev/null
cargo run -p cove-index --bin cove-index-inspect -- conformance/covi/single_section_valid.covi > /dev/null
cargo run -p cove-index --bin cove-index-inspect -- conformance/covi/index_capability_valid.bin > /dev/null
cargo run -p cove-index --bin cove-index-inspect -- conformance/covi/index_only_capability_valid.bin > /dev/null
cargo run -p cove-cache --bin cove-cache-inspect -- conformance/cache/cache_valid.bin > /dev/null
cargo test -p cove-convert-parquet
cargo run -p cove-bench --bin cove-bench > /dev/null
cargo run -p cove-conformance --bin gen-corpus -- --check
cargo run -p cove-conformance --bin gen-capability-matrix -- --check
cargo run -p cove-conformance --bin cove-conformance -- conformance/
