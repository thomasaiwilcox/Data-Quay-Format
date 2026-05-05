//! Generates the Spec §70 capability matrix from facts in qf-core.
//!
//! Run with `cargo run -p qf-conformance --bin gen-capability-matrix`.
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
            capability: "QF header",
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
            corpus: "partial",
            notes: "wire.rs, checksum.rs; fixture coverage is bootstrap-focused",
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
            validated: "partial",
            written: "partial",
            corpus: "no",
            notes: "dictionary.rs; semantic dictionary parsing exists",
        },
        Row {
            section: "§17",
            capability: "Canonical value encoding",
            modeled: "yes",
            parsed: "n/a",
            validated: "unit",
            written: "n/a",
            corpus: "no",
            notes: "canonical.rs",
        },
        Row {
            section: "§20",
            capability: "Encoding cascades",
            modeled: "yes",
            parsed: "partial",
            validated: "unit",
            written: "no",
            corpus: "no",
            notes: "encoding/* helpers; deterministic parity harness; writer integration pending",
        },
        Row {
            section: "§21",
            capability: "Kernel capability declarations",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "no",
            corpus: "yes",
            notes: "kernel.rs; accept/reject corpus",
        },
        Row {
            section: "§22",
            capability: "Collations",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "no",
            corpus: "yes",
            notes: "collation.rs; accept/reject registry corpus",
        },
        Row {
            section: "§23",
            capability: "ColumnDomain",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "helper",
            corpus: "yes",
            notes: "domain.rs; accept/reject corpus plus qf-validate negative integration",
        },
        Row {
            section: "§24",
            capability: "Table catalog",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "table.rs; ScanProfileQfWriter emits it; accept/reject corpus",
        },
        Row {
            section: "§25",
            capability: "Segment index/header",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "segment.rs; ScanProfileQfWriter emits index and data header; corpus covered",
        },
        Row {
            section: "§26",
            capability: "Morsel directory",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            corpus: "yes",
            notes: "segment.rs; ScanProfileQfWriter emits row morsels; corpus covered",
        },
        Row {
            section: "§27",
            capability: "Page entry + CRC",
            modeled: "yes",
            parsed: "yes",
            validated: "unit",
            written: "no",
            corpus: "yes",
            notes: "page.rs; accept/reject page-index corpus; scan writer page payload integration pending",
        },
        Row {
            section: "§28",
            capability: "Zone statistics",
            modeled: "yes",
            parsed: "no",
            validated: "unit",
            written: "no",
            corpus: "no",
            notes: "zone_stats.rs helper-level validation",
        },
        Row {
            section: "§29",
            capability: "Predicate truth tables",
            modeled: "yes",
            parsed: "n/a",
            validated: "unit",
            written: "n/a",
            corpus: "no",
            notes: "predicate.rs",
        },
        Row {
            section: "§30",
            capability: "Exact-set index",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "no",
            corpus: "yes",
            notes: "index/exact_set.rs; accept/reject corpus",
        },
        Row {
            section: "§31",
            capability: "Bloom-filter index",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "no",
            corpus: "yes",
            notes: "index/bloom.rs; accept/reject corpus",
        },
        Row {
            section: "§32",
            capability: "Inverted morsel index",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "no",
            corpus: "yes",
            notes: "index/inverted.rs; accept/reject corpus",
        },
        Row {
            section: "§33",
            capability: "Lookup index (RowRef)",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "no",
            corpus: "yes",
            notes: "index/lookup.rs; accept/reject corpus",
        },
        Row {
            section: "§34",
            capability: "Aggregate synopsis",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "no",
            corpus: "yes",
            notes: "index/aggregate.rs; accept/reject corpus",
        },
        Row {
            section: "§35",
            capability: "Composite zone index",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "no",
            corpus: "yes",
            notes: "index/composite.rs; accept/reject corpus",
        },
        Row {
            section: "§36",
            capability: "Top-N summary",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "no",
            corpus: "yes",
            notes: "index/topn.rs; accept/reject corpus",
        },
        Row {
            section: "§37",
            capability: "Pruning evidence + explain",
            modeled: "yes",
            parsed: "n/a",
            validated: "unit",
            written: "n/a",
            corpus: "no",
            notes: "pruning.rs",
        },
        Row {
            section: "§40",
            capability: "ExecutionCode descriptors",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "helper",
            corpus: "yes",
            notes: "profile/qfe.rs; descriptor/registry/policy plus required/optional corpus",
        },
        Row {
            section: "§44",
            capability: "Harbor mount hints",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "helper",
            corpus: "yes",
            notes: "profile/qfh.rs; mount-hints plus required/optional corpus",
        },
        Row {
            section: "§49",
            capability: "Arrow null↔validity inversion",
            modeled: "yes",
            parsed: "n/a",
            validated: "unit",
            written: "n/a",
            corpus: "no",
            notes: "interop/arrow.rs",
        },
        Row {
            section: "§50",
            capability: "Lakehouse hints",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "no",
            corpus: "yes",
            notes: "interop/lakehouse.rs; accept/reject corpus",
        },
        Row {
            section: "§51",
            capability: "Parquet conversion",
            modeled: "plan",
            parsed: "no",
            validated: "no",
            written: "no",
            corpus: "no",
            notes: "interop/parquet.rs is design scaffolding only",
        },
        Row {
            section: "§52",
            capability: "Nested layouts",
            modeled: "yes",
            parsed: "partial",
            validated: "unit",
            written: "no",
            corpus: "no",
            notes: "encoding/nested.rs",
        },
        Row {
            section: "§53",
            capability: "Sort + clustering keys",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "helper",
            corpus: "yes",
            notes: "sort.rs fixed-size §53 entries; accept/reject corpus",
        },
        Row {
            section: "§54",
            capability: "RowRef",
            modeled: "yes",
            parsed: "yes",
            validated: "unit",
            written: "yes",
            corpus: "partial",
            notes: "row_ref.rs; exercised through lookup-index corpus",
        },
        Row {
            section: "§56",
            capability: "QF-O object type catalog",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "helper",
            corpus: "yes",
            notes: "profile/qfo.rs; accept/reject catalog plus required/optional corpus",
        },
        Row {
            section: "§57",
            capability: "QF-O temporal segment index",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "helper",
            corpus: "yes",
            notes: "profile/qfo.rs; accept/reject temporal-index corpus",
        },
        Row {
            section: "§58",
            capability: "QF-O lex row order",
            modeled: "yes",
            parsed: "helper",
            validated: "unit",
            written: "n/a",
            corpus: "no",
            notes: "profile/qfo.rs helper; temporal data parser pending",
        },
        Row {
            section: "§60",
            capability: "QF-O self-containment",
            modeled: "yes",
            parsed: "helper",
            validated: "unit",
            written: "n/a",
            corpus: "no",
            notes: "profile/qfo.rs helper; prev_ref wire integration pending",
        },
        Row {
            section: "§63",
            capability: "Trust chain",
            modeled: "yes",
            parsed: "no",
            validated: "unit",
            written: "no",
            corpus: "no",
            notes: "trust_chain.rs helper; TrustManifest reader integration pending",
        },
        Row {
            section: "§64",
            capability: "Redaction manifest",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "no",
            corpus: "yes",
            notes: "redaction.rs; accept/reject corpus",
        },
        Row {
            section: "§65",
            capability: "Digest manifest",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "no",
            corpus: "yes",
            notes: "digest.rs; accept/reject manifest corpus plus --verify-digests coverage",
        },
        Row {
            section: "§66",
            capability: "Compression",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "partial",
            corpus: "no",
            notes: "compression.rs feature-gated; writer emits uncompressed",
        },
        Row {
            section: "§67",
            capability: "I/O hints",
            modeled: "yes",
            parsed: "helper",
            validated: "unit",
            written: "no",
            corpus: "yes",
            notes: "io_hints.rs; accept/reject corpus",
        },
        Row {
            section: "§68",
            capability: "QFX sidecar",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "helper",
            corpus: "yes",
            notes: "artifact/qfx.rs; accept/reject artifact corpus",
        },
        Row {
            section: "§69",
            capability: "QFM manifest",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "helper",
            corpus: "yes",
            notes: "artifact/qfm.rs; accept/reject manifest corpus",
        },
        Row {
            section: "§71",
            capability: "Writer profiles",
            modeled: "yes",
            parsed: "n/a",
            validated: "yes",
            written: "partial",
            corpus: "partial",
            notes: "Minimal writer plus ScanProfileQfWriter; generated QF-T fixture; page payload writer pending",
        },
        Row {
            section: "§72",
            capability: "Validation model",
            modeled: "yes",
            parsed: "n/a",
            validated: "partial",
            written: "n/a",
            corpus: "yes",
            notes: "reader stages + qf-validate; required/optional profile corpus; QF-O temporal data invariants pending",
        },
        Row {
            section: "§74",
            capability: "Durable replace",
            modeled: "yes",
            parsed: "n/a",
            validated: "unit",
            written: "yes",
            corpus: "no",
            notes: "durable.rs",
        },
        Row {
            section: "§75",
            capability: "Error-code surface",
            modeled: "yes",
            parsed: "n/a",
            validated: "unit",
            written: "n/a",
            corpus: "partial",
            notes: "error.rs; bootstrap reject fixtures",
        },
        Row {
            section: "§78",
            capability: "Conformance/benchmark suite",
            modeled: "yes",
            parsed: "n/a",
            validated: "partial",
            written: "partial",
            corpus: "partial",
            notes: "78-fixture qf-conformance corpus, qf-bench smoke, and deterministic robustness harness",
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
    out.push_str("# QF v1.0 Capability Matrix (Spec §70)\n\n");
    out.push_str("Generated by `cargo run -p qf-conformance --bin gen-capability-matrix`.\n\n");
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
            "{} is not up to date; run cargo run -p qf-conformance --bin gen-capability-matrix",
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
