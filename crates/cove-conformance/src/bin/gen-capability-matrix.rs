//! Generates the Spec §70 capability matrix from facts in cove-core.
//!
//! Run with `cargo run -p cove-conformance --bin gen-capability-matrix`.
//! Output is a markdown table written to `conformance/capability_matrix.md`.
//!
//! The matrix is evidence based. A section is not treated as broadly
//! supported just because a helper type or parser exists; support is split
//! into modeled, parsed, validated, written, and corpus evidence.

use std::{fs, path::PathBuf};

struct Row {
    section: &'static str,
    capability: &'static str,
    modeled: &'static str,
    parsed: &'static str,
    validated: &'static str,
    written: &'static str,
    corpus: &'static str,
    notes: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            section: "§9",
            capability: "COVE header",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "header.rs; accept/reject bootstrap fixtures",
        },
        Row {
            section: "§10",
            capability: "Wire primitives",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "wire.rs, checksum.rs; bootstrap fixtures plus `wire_primitive_case` corpus covering varint LEB128 boundary round-trips, malformed/overflow rejection, ZigZag i64 boundaries, and strict bool accept/reject",
        },
        Row {
            section: "§13",
            capability: "Postscript + footer",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "postscript.rs, footer.rs",
        },
        Row {
            section: "§15",
            capability: "Metadata JSON",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "metadata.rs; accept/reject JSON corpus",
        },
        Row {
            section: "§16",
            capability: "File dictionary",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "dictionary.rs; standalone dictionary corpus plus semantic reader integration; header serializer round-trips with parser",
        },
        Row {
            section: "§17",
            capability: "Canonical value encoding",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "canonical.rs `validate_canonical_payload` is a structural parser that consumes encoded payloads, paired with `CanonicalValue::encode` and the recursive `canonicalize_*` helpers; round-trip exercised by spec_17 unit tests and the dictionary corpus",
        },
        Row {
            section: "§20",
            capability: "Encoding cascades",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "encoding_case corpus covers constant, LocalCodebook (BitPacked/RLE child cascades), RLE, run-end, plain, bit-packed, delta, FoR, patched-base, and sparse; `ScanProfileCoveWriter` emits compressed page payloads through `encode_page_payload`, exercised by writer tests plus `accept/cove_t_local_codebook_lz4.cove`",
        },
        Row {
            section: "§21",
            capability: "Kernel capability declarations",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "kernel.rs; accept/reject corpus; `KernelCapabilities::serialize` round-trips with the parser",
        },
        Row {
            section: "§22",
            capability: "Collations",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "collation.rs; accept/reject registry corpus; `CollationRegistry::serialize` round-trips with parser",
        },
        Row {
            section: "§23",
            capability: "ColumnDomain",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "domain.rs; accept/reject corpus plus cove-validate negative integration; `ColumnDomain::serialize` round-trips with parser",
        },
        Row {
            section: "§24",
            capability: "Table catalog",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "table.rs; ScanProfileCoveWriter emits it; accept/reject corpus",
        },
        Row {
            section: "§25",
            capability: "Segment index/header",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "segment.rs; ScanProfileCoveWriter emits index and data header; corpus covered",
        },
        Row {
            section: "§26",
            capability: "Morsel directory",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "segment.rs; ScanProfileCoveWriter emits row morsels; corpus covered",
        },
        Row {
            section: "§27",
            capability: "Page entry + CRC",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "page.rs plus segment.rs; standalone page-index corpus and ScanProfileCoveWriter emit validated page-index/data regions",
        },
        Row {
            section: "§28",
            capability: "Zone statistics",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "zone_stats.rs parser plus semantic reader integration; full-file accept/reject corpus and CLI coverage; `ZoneStatsEntry::serialize` / `ZoneStatsSection::serialize` round-trip with the parser",
        },
        Row {
            section: "§29",
            capability: "Predicate truth tables",
            modeled: "yes",
            parsed: "n/a",
            validated: "yes",
            written: "n/a",
            corpus: "yes",
            notes: "predicate.rs plus pruning_case corpus for AND/OR/NOT outcome composition",
        },
        Row {
            section: "§30",
            capability: "Exact-set index",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "index/exact_set.rs; accept/reject corpus; `ExactSetIndex::serialize` round-trips with parser",
        },
        Row {
            section: "§31",
            capability: "Bloom-filter index",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "index/bloom.rs; accept/reject corpus plus pruning_case bloom_membership accept and fail-open fixtures; `BloomFilterIndex::serialize` round-trips with parser",
        },
        Row {
            section: "§32",
            capability: "Inverted morsel index",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "index/inverted.rs; accept/reject corpus plus pruning_case inverted_lookup accept and fail-open fixtures; `InvertedMorselIndex::serialize` round-trips with parser",
        },
        Row {
            section: "§33",
            capability: "Lookup index (RowRef)",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "index/lookup.rs; accept/reject corpus plus pruning_case lookup_point accept and fail-open fixtures; `LookupIndex::serialize` round-trips with parser",
        },
        Row {
            section: "§34",
            capability: "Aggregate synopsis",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "index/aggregate.rs; accept/reject corpus plus pruning_case aggregate_synopsis accept and fail-open fixtures; `AggregateSynopsis::serialize` round-trips with parser",
        },
        Row {
            section: "§35",
            capability: "Composite zone index",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "index/composite.rs; accept/reject corpus plus pruning_case composite_zone accept and fail-open fixtures; `CompositeIndex::serialize` round-trips with parser",
        },
        Row {
            section: "§36",
            capability: "Top-N summary",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "index/topn.rs; accept/reject corpus; `TopNSummary::serialize` round-trips with parser",
        },
        Row {
            section: "§37",
            capability: "Pruning evidence + explain",
            modeled: "yes",
            parsed: "n/a",
            validated: "yes",
            written: "n/a",
            corpus: "yes",
            notes: "pruning.rs leaf proofs for null, FileCode equality, FileCode domain-range, typed NumCode range, bloom membership, inverted/lookup point, aggregate synopsis, and composite zone with §73 fail-open fallback evidence and §37.5 AND/OR reorder-invariance proofs backed by pruning_case corpus",
        },
        Row {
            section: "§40",
            capability: "ExecutionCode descriptors",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "profile/cove_e.rs plus reader.rs; descriptor/registry/scope/code-space/policy and required/optional full-file corpus; `ExecutionCodeDescriptorV1::serialize` round-trips with parser",
        },
        Row {
            section: "§41",
            capability: "Execution scope descriptors",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "profile/cove_e.rs plus reader.rs; standalone and integrated full-file scope descriptor corpus; `ExecutionScopeDescriptorV1::serialize` round-trips with parser",
        },
        Row {
            section: "§42",
            capability: "Code-space descriptors",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "profile/cove_e.rs plus reader.rs; standalone and integrated full-file code-space corpus; `CodeSpaceDescriptorV1::serialize` round-trips with parser",
        },
        Row {
            section: "§43",
            capability: "Engine mount policy",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "profile/cove_e.rs plus reader.rs; mount-policy plus required/optional full-file bundle corpus; `EngineMountPolicyV1::serialize` round-trips with parser",
        },
        Row {
            section: "§44",
            capability: "Harbor mount hints",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "profile/cove_h.rs; mount-hints plus required/optional corpus; `HarborMountHintsV1::serialize` round-trips with parser",
        },
        Row {
            section: "§49",
            capability: "Arrow interop and export",
            modeled: "yes",
            parsed: "n/a",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "interop/arrow.rs covers null↔validity inversion plus EncodedArray-to-Arrow scalar/export helpers; arrow_bitmap_case and arrow_export_case corpus cover both surfaces",
        },
        Row {
            section: "§50",
            capability: "Lakehouse hints",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "interop/lakehouse.rs; accept/reject corpus; `LakehouseHints::serialize` round-trips with parser",
        },
        Row {
            section: "§51",
            capability: "Parquet conversion",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "interop/parquet.rs plus cove-convert-parquet CLI convert primitive/temporal/utf8/binary parquet batches into COVE-T scan-profile files with machine-readable reports; parquet_conversion_case corpus covers accept plus null/nested reject cases",
        },
        Row {
            section: "§52",
            capability: "Nested layouts",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "encoding/nested.rs payload parsers plus ScanProfileCoveWriter explicit nested page specs, semantic TableSegmentData validation, nested_case JSON fixtures, and accept/reject nested .cove files for list/struct/map invariants",
        },
        Row {
            section: "§53",
            capability: "Sort + clustering keys",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "sort.rs fixed-size §53 entries; accept/reject corpus; `SortKey::serialize` round-trips with parser",
        },
        Row {
            section: "§54",
            capability: "RowRef",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "row_ref.rs; standalone accept/reject corpus plus lookup-index integration",
        },
        Row {
            section: "§56",
            capability: "COVE-O object type catalog",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "profile/cove_o.rs; accept/reject catalog plus required/optional corpus; ObjectTypeCatalog serializer round-trips with parser",
        },
        Row {
            section: "§57",
            capability: "COVE-O temporal segment index",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "profile/cove_o.rs; accept/reject temporal-index corpus; TemporalSegmentIndex serializer round-trips with parser",
        },
        Row {
            section: "§58",
            capability: "COVE-O lex row order",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "profile/cove_o.rs temporal segment parser plus semantic reader integration; full-file accept/reject corpus; segment serializer enforces lex order on round-trip",
        },
        Row {
            section: "§60",
            capability: "COVE-O self-containment",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "profile/cove_o.rs prev_ref parsing plus semantic reader integration; full-file accept/reject corpus; segment serializer enforces prev_ref on round-trip",
        },
        Row {
            section: "§63",
            capability: "Trust chain",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "TrustManifest parser plus trust_chain.rs reader integration; full-file accept/reject corpus; `TrustManifest::serialize` round-trips with parser",
        },
        Row {
            section: "§64",
            capability: "Redaction manifest",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "redaction.rs; accept/reject corpus; `RedactionManifest::serialize` round-trips with parser",
        },
        Row {
            section: "§65",
            capability: "Digest manifest",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "digest.rs; accept/reject manifest corpus plus --verify-digests coverage; `DigestManifest::serialize` round-trips with parser",
        },
        Row {
            section: "§66",
            capability: "Compression",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "compression.rs plus writer.rs and page.rs; section LZ4/Zstd payloads, page-level codec dispatch with codec/length/reserved-bit invariants, and `page_codec_case` round-trip plus rejection corpus",
        },
        Row {
            section: "§67",
            capability: "I/O hints",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "io_hints.rs; accept/reject corpus; `IoHints::encode` round-trips with parser",
        },
        Row {
            section: "§68",
            capability: "COVX sidecar",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "artifact/covx.rs; accept/reject artifact corpus; `CovxFile::serialize` round-trips with parser",
        },
        Row {
            section: "§69",
            capability: "COVM manifest",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "artifact/covm.rs; accept/reject manifest corpus; `CovmFile::serialize` round-trips with parser",
        },
        Row {
            section: "§70",
            capability: "COVEMAP artifact framing",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "artifact/covemap.rs; accept/reject artifact corpus; `CovemapFile::serialize` round-trips with parser",
        },
        Row {
            section: "§71",
            capability: "Writer profiles",
            modeled: "yes",
            parsed: "n/a",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "Minimal writer plus ScanProfileCoveWriter; generated COVE-T scan fixture now includes column/page regions",
        },
        Row {
            section: "§72",
            capability: "Validation model",
            modeled: "yes",
            parsed: "n/a",
            validated: "yes",
            written: "n/a",
            corpus: "yes",
            notes: "reader stages + cove-validate; required/optional profile corpus; shared, COVE-T, and COVE-O semantic invariants exercised end to end",
        },
        Row {
            section: "§74",
            capability: "Durable replace",
            modeled: "yes",
            parsed: "n/a",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "durable.rs plus writer.rs; tempdir failure-injection coverage and writer-facing publish API",
        },
        Row {
            section: "§75",
            capability: "Error-code surface",
            modeled: "yes",
            parsed: "n/a",
            validated: "yes",
            written: "n/a",
            corpus: "yes",
            notes: "error.rs spec_code()/ALL_SPEC_CODES, cove-validate JSON output, and generated reject fixtures including error_surface_case coverage",
        },
        Row {
            section: "§78",
            capability: "Conformance/benchmark suite",
            modeled: "yes",
            parsed: "n/a",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "suite_contract_case corpus, CLI smoke tests including cove-convert-parquet, release-gate bench/conformance smoke, and deterministic robustness harness",
        },
    ]
}

fn gate_met(value: &str) -> bool {
    matches!(value, "yes" | "n/a")
}

fn row_gate_met(row: &Row) -> bool {
    gate_met(row.modeled)
        && gate_met(row.parsed)
        && gate_met(row.validated)
        && gate_met(row.written)
        && row.corpus == "yes"
}

fn check_mode() -> bool {
    std::env::args().any(|arg| arg == "--check")
}

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("conformance");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("capability_matrix.md");

    let mut out = String::new();
    out.push_str("# COVE v1.0 Capability Matrix (Spec §70)\n\n");
    out.push_str("Generated by `cargo run -p cove-conformance --bin gen-capability-matrix`.\n\n");
    out.push_str("Evidence key: `yes` = implemented and exercised, `partial` = incomplete, `helper` = helper/writer helper only, `unit` = unit-test-only evidence, `plan` = design scaffold, `n/a` = not applicable.\n\n");
    out.push_str(
        "| Spec | Capability | Modeled | Parsed | Validated | Written | Corpus | Notes |\n",
    );
    out.push_str(
        "|------|------------|---------|--------|-----------|---------|--------|-------|\n",
    );
    let mut total = 0usize;
    let mut fully_gated = 0usize;
    let rows = rows();
    for r in &rows {
        total += 1;
        if row_gate_met(r) {
            fully_gated += 1;
        }
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
            r.section, r.capability, r.modeled, r.parsed, r.validated, r.written, r.corpus, r.notes,
        ));
    }
    out.push_str(&format!(
        "\n**Fully gated capabilities:** {fully_gated} / {total}\n"
    ));

    if check_mode() {
        let existing = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("cannot read {} during --check: {err}", path.display()));
        assert_eq!(
            existing,
            out,
            "{} is not up to date; run cargo run -p cove-conformance --bin gen-capability-matrix",
            path.display()
        );
        println!(
            "{} is up to date ({fully_gated}/{total} fully gated capabilities)",
            path.display()
        );
    } else {
        fs::write(&path, &out).unwrap();
        println!(
            "wrote {} ({fully_gated}/{total} fully gated capabilities)",
            path.display()
        );
    }
}
