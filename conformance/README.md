# QF Conformance Corpus

This directory contains binary fixtures and a `manifest.jsonl` that maps each
fixture to the Spec §1–§79 sections it exercises. The corpus includes complete
`.quay` files plus parser-focused payload and artifact fixtures for structures
that are not always meaningful as standalone QF files. Run the corpus with:

```sh
cargo run -p qf-conformance --bin qf-conformance -- conformance/
```

Each manifest line is one JSON object:
- `path`     — relative path from this directory
- `kind`     — parser to run; omitted means `qf`. Current generated kinds are:
    `qf`, `qfx`, `qfm`, `metadata_json`, `collation_registry`,
    `digest_manifest`, `redaction_manifest`, `io_hints`, `lakehouse_hints`,
    `kernel_capabilities`, `page_index`, `column_domain`, `table_catalog`,
    `table_segment_index`, `table_segment_header`, `row_morsel_directory`,
    `exact_set_index`, `bloom_index`, `inverted_morsel_index`, `lookup_index`,
    `aggregate_synopsis`, `composite_zone_index`, `topn_summary`, `sort_key`,
    `clustering_key`, `qfe_engine_registry`, `qfe_execution_code`,
    `qfe_mount_policy`, `qfh_mount_hints`, `qfo_object_catalog`, and
    `qfo_temporal_segment_index`
- `expect`   — `"accept"` or `"reject"`
- `error_code` — (preferred when `expect=reject`) stable Spec §75 error code
- `error`    — optional fallback substring match for ad hoc cases
- `sections` — list of `"§N.M"` markers from `Spec.md`
- `morsel_count` — required only for `row_morsel_directory` fixtures

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
cargo run -p qf-conformance --bin gen-corpus -- --check
cargo run -p qf-conformance --bin gen-capability-matrix -- --check
```

A capability should only be treated as release-grade when the relevant columns
are `yes` and the corpus contains both positive and negative evidence where the
spec defines failure behavior.
