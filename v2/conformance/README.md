# COVE Conformance Corpus

This directory contains binary fixtures and a `manifest.jsonl` that maps each
fixture to the Spec §1–§81 sections it exercises. The corpus includes complete
`.cove` files plus parser-focused payload and artifact fixtures for structures
that are not always meaningful as standalone COVE files. Run the corpus with:

```sh
cargo run -p cove-conformance --bin cove-conformance -- conformance/
```

Each manifest line is one JSON object:
- `path`     — relative path from this directory
- `kind`     — parser to run; omitted means `cove`. Current generated kinds are:
    `cove`, `covx`, `covm`, `metadata_json`, `file_dictionary`, `collation_registry`,
    `digest_manifest`, `redaction_manifest`, `io_hints`, `lakehouse_hints`,
    `encoding_case`,
    `nested_case`,
    `arrow_bitmap_case`, `suite_contract_case`, `kernel_capabilities`, `page_index`, `column_domain`,
    `table_catalog`, `table_segment_index`, `table_segment_header`,
    `row_morsel_directory`, `row_ref`, `exact_set_index`, `bloom_index`,
    `inverted_morsel_index`, `lookup_index`, `aggregate_synopsis`,
    `composite_zone_index`, `topn_summary`, `sort_key`, `clustering_key`,
        `pruning_case`, `error_surface_case`, `cove_e_engine_registry`, `cove_e_execution_code`,
    `cove_e_execution_scope`, `cove_e_code_space`, `cove_e_mount_policy`,
    `cove_h_mount_hints`, `cove_o_object_catalog`,
    `cove_o_temporal_segment_index`, `cove_o_temporal_bloom_index`,
    `extension_registry`, `extension_logical_type`, `extension_index_descriptor`,
    `durable_publish_case`, `sidecar_freshness_case`, `cove_map_convert_case`,
    `cove_map_project_case`, and `arrow_view_materialization_case`
- `expect`   — `"accept"` or `"reject"`
- `error_code` — (preferred when `expect=reject`) stable Spec §76 error code
    Fixtures with `error_code` are automatically tagged as `§76` evidence by `gen-corpus`.
- `error`    — optional fallback substring match for ad hoc cases
- `sections` — list of `"§N.M"` markers from `Spec.md`
- `morsel_count` — required only for `row_morsel_directory` fixtures

`suite_contract_case` fixtures are repo-level meta checks used for Spec §§78-79. They
verify the generated manifest breadth plus the release-gate and workspace binary
contract that keeps the conformance and benchmark suite executable.

## Capability Evidence

`capability_matrix.md` is generated and evidence-based. A section is not marked
as broadly supported just because a Rust type or helper exists. The matrix
separates five evidence levels:

- `Modeled` — the repo has a type, enum, helper, or design scaffold.
- `Parsed` — the on-disk wire form is parsed from bytes.
- `Validated` — semantic or structural invariants are checked.
- `Written` — a writer can emit the structure.
- `Corpus` — accept/reject fixtures exercise the behavior through the
    conformance runner.

Generated artifacts can be checked without rewriting files:

```sh
cargo run -p cove-conformance --bin gen-corpus -- --check
cargo run -p cove-conformance --bin gen-capability-matrix -- --check
```

A capability should only be treated as release-grade when the relevant columns
are `yes` and the corpus contains both positive and negative evidence where the
spec defines failure behavior.
