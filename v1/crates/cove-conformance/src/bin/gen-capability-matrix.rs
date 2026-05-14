//! Generates the Spec §71 capability matrix from facts in cove-core.
//!
//! Run with `cargo run -p cove-conformance --bin gen-capability-matrix`.
//! Output is a markdown table written to `conformance/capability_matrix.md`.
//!
//! The matrix is evidence based. A section is not treated as broadly
//! supported just because a helper type or parser exists; support is split
//! into modeled, parsed, validated, written, and corpus evidence.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
};

use serde_json::Value;

struct Row {
    section: &'static str,
    capability: &'static str,
    modeled: &'static str,
    parsed: &'static str,
    validated: &'static str,
    written: &'static str,
    notes: &'static str,
}

#[derive(Debug, Clone, Copy)]
struct CorpusRequirement {
    min_accept: usize,
    min_reject: usize,
    reject_exemption: Option<&'static str>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct CorpusEvidence {
    accept: usize,
    reject: usize,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            section: "§10",
            capability: "COVE header",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "header.rs; accept/reject bootstrap fixtures",
        },
        Row {
            section: "§8",
            capability: "Wire primitives",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "wire.rs, checksum.rs; bootstrap fixtures plus `wire_primitive_case` corpus covering varint LEB128 boundary round-trips, malformed/overflow rejection, ZigZag i64 boundaries, and strict bool accept/reject",
        },
        Row {
            section: "§12",
            capability: "Postscript",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "postscript.rs; bootstrap validation checks footer pointer, file length, feature echo, and tail magic",
        },
        Row {
            section: "§13",
            capability: "Footer and section directory",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "footer.rs plus reader.rs validate section ordering, ranges, CRCs, profiles, codecs, and metadata",
        },
        Row {
            section: "§15",
            capability: "Metadata JSON",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "metadata.rs; accept/reject JSON corpus",
        },
        Row {
            section: "§16",
            capability: "File dictionary",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "dictionary.rs; standalone dictionary corpus plus semantic reader integration; header serializer round-trips with parser",
        },
        Row {
            section: "§17",
            capability: "Canonical value encoding",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "canonical.rs `validate_canonical_payload` is a structural parser that consumes encoded payloads, paired with `CanonicalValue::encode` and the recursive `canonicalize_*` helpers; round-trip exercised by spec_17 unit tests and the dictionary corpus",
        },
        Row {
            section: "§19",
            capability: "Logical/physical compatibility",
            modeled: "yes",
            parsed: "n/a",
            validated: "yes",
            written: "yes",
            notes: "types.rs validates strict/default pairs plus explicit Bool-as-NumCode declarations through table, segment, and COVE-O property flags; full-file accept/reject corpus covers declared and missing Bool NumCode cases",
        },
        Row {
            section: "§20",
            capability: "Encoding cascades",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "encoding_case corpus covers constant, LocalCodebook (BitPacked/RLE child cascades), RLE, run-end, plain, bit-packed, delta, FoR, patched-base, and sparse; `ScanProfileCoveWriter` emits compressed page payloads through `encode_page_payload`, exercised by writer tests plus `accept/cove_t_local_codebook_lz4.cove`",
        },
        Row {
            section: "§21",
            capability: "Kernel capability declarations",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "kernel.rs parses §21 field-based capability entries with reserved-byte validation; accept/reject corpus; `KernelCapabilities::serialize` round-trips with the parser",
        },
        Row {
            section: "§22",
            capability: "Collations",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "collation.rs; accept/reject registry corpus; `CollationRegistry::serialize` round-trips with parser",
        },
        Row {
            section: "§23",
            capability: "ColumnDomain",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "domain.rs; accept/reject corpus plus cove-validate negative integration; `ColumnDomain::serialize` round-trips with parser",
        },
        Row {
            section: "§24",
            capability: "Table catalog",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "table.rs; ScanProfileCoveWriter emits it; accept/reject corpus",
        },
        Row {
            section: "§25",
            capability: "Segment index/header",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "segment.rs; ScanProfileCoveWriter emits index and data header; corpus covered",
        },
        Row {
            section: "§26",
            capability: "Morsel directory",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "segment.rs; ScanProfileCoveWriter emits row morsels; corpus covered",
        },
        Row {
            section: "§27",
            capability: "Page entry + CRC",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "page.rs plus segment.rs; standalone page-index corpus and ScanProfileCoveWriter emit validated page-index/data regions",
        },
        Row {
            section: "§28",
            capability: "Zone statistics",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "zone_stats.rs parser plus semantic reader integration; full-file accept/reject corpus and CLI coverage; `ZoneStatsEntry::serialize` / `ZoneStatsSection::serialize` round-trip with the parser",
        },
        Row {
            section: "§29",
            capability: "Predicate truth tables",
            modeled: "yes",
            parsed: "n/a",
            validated: "yes",
            written: "n/a",
            notes: "predicate.rs plus pruning_case corpus for AND/OR/NOT outcome composition",
        },
        Row {
            section: "§30",
            capability: "Exact-set index",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "index/exact_set.rs; accept/reject corpus; `ExactSetIndex::serialize` round-trips with parser",
        },
        Row {
            section: "§31",
            capability: "Bloom-filter index",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "index/bloom.rs; accept/reject corpus plus pruning_case bloom_membership accept and fail-open fixtures; `BloomFilterIndex::serialize` round-trips with parser",
        },
        Row {
            section: "§32",
            capability: "Inverted morsel index",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "index/inverted.rs; accept/reject corpus plus pruning_case inverted_lookup accept and fail-open fixtures; `InvertedMorselIndex::serialize` round-trips with parser",
        },
        Row {
            section: "§33",
            capability: "Lookup index (RowRef)",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "index/lookup.rs; accept/reject corpus plus pruning_case lookup_point accept and fail-open fixtures; `LookupIndex::serialize` round-trips with parser",
        },
        Row {
            section: "§34",
            capability: "Aggregate synopsis",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "index/aggregate.rs; accept/reject corpus plus pruning_case aggregate_synopsis accept and fail-open fixtures; `AggregateSynopsis::serialize` round-trips with parser",
        },
        Row {
            section: "§35",
            capability: "Composite zone index",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "index/composite.rs; accept/reject corpus plus pruning_case composite_zone accept and fail-open fixtures; `CompositeIndex::serialize` round-trips with parser",
        },
        Row {
            section: "§36",
            capability: "Top-N summary",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "index/topn.rs; accept/reject corpus; `TopNSummary::serialize` round-trips with parser",
        },
        Row {
            section: "§37",
            capability: "Pruning evidence + explain",
            modeled: "yes",
            parsed: "n/a",
            validated: "yes",
            written: "n/a",
            notes: "pruning.rs leaf proofs for null, FileCode equality, FileCode domain-range, typed NumCode range, bloom membership, inverted/lookup point, aggregate synopsis, and composite zone with §73 fail-open fallback evidence and §37.5 AND/OR reorder-invariance proofs backed by pruning_case corpus",
        },
        Row {
            section: "§40",
            capability: "ExecutionCode descriptors",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "profile/cove_e.rs plus reader.rs; descriptor/registry/scope/code-space/policy and required/optional full-file corpus; `ExecutionCodeDescriptorV1::serialize` round-trips with parser",
        },
        Row {
            section: "§41",
            capability: "Execution scope descriptors",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "profile/cove_e.rs plus reader.rs; standalone and integrated full-file scope descriptor corpus; `ExecutionScopeDescriptorV1::serialize` round-trips with parser",
        },
        Row {
            section: "§42",
            capability: "Code-space descriptors",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "profile/cove_e.rs plus reader.rs; standalone and integrated full-file code-space corpus; `CodeSpaceDescriptorV1::serialize` round-trips with parser",
        },
        Row {
            section: "§43",
            capability: "Engine mount policy",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "profile/cove_e.rs plus reader.rs; mount-policy plus required/optional full-file bundle corpus; `EngineMountPolicyV1::serialize` round-trips with parser",
        },
        Row {
            section: "§44",
            capability: "Harbor mount hints",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "profile/cove_h.rs parses hints; mount.rs exposes Harbor-specific mount_cove_h_file that reuses or rebuilds FileCode-to-Harbor maps; corpus covers hint parsing plus mount map rebuild/reuse",
        },
        Row {
            section: "§45",
            capability: "Extension registry",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "extensions.rs; extension_registry corpus covers valid optional registry, bad CRC, reserved header flags, trailing bytes, and required unknown extension rejection",
        },
        Row {
            section: "§46",
            capability: "Custom logical types",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "ExtensionLogicalTypeV1 parser/serializer plus validation context; corpus covers portable Arrow extension descriptor and invalid collation reference rejection",
        },
        Row {
            section: "§47",
            capability: "Custom indexes/synopses",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "ExtensionIndexDescriptorV1 parser/serializer; corpus covers valid false-negative non-skipping descriptors and rejects descriptors that claim proof capability while allowing false negatives",
        },
        Row {
            section: "§49",
            capability: "Arrow interop and export",
            modeled: "yes",
            parsed: "n/a",
            validated: "yes",
            written: "yes",
            notes: "interop/arrow.rs covers null↔validity inversion, strict/reporting EncodedArray-to-Arrow export, UUID FixedSizeBinary, JSON extension-or-lossy enforcement, decimal precision/scale context, and fidelity diagnostics; arrow_bitmap_case and arrow_export_case corpus cover both surfaces",
        },
        Row {
            section: "§50",
            capability: "Lakehouse hints",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "interop/lakehouse.rs parses descriptive hints and visibility overlay references, with guard helpers for physical pruning, candidate filtering, and visible exactness/aggregate restrictions; full external catalog filtering remains out of scope",
        },
        Row {
            section: "§51",
            capability: "Parquet conversion",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "interop/parquet.rs plus cove-convert-parquet CLI convert primitive/temporal/utf8/binary/nested JSON-fallback parquet batches into COVE-T scan-profile files with machine-readable reports; parquet_conversion_case corpus covers primitive, nullable, and nested fallback accept cases",
        },
        Row {
            section: "§52",
            capability: "Nested layouts",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "encoding/nested.rs payload parsers plus ScanProfileCoveWriter explicit nested page specs, semantic TableSegmentData validation, nested_case JSON fixtures, and accept/reject nested .cove files for list/struct/map invariants",
        },
        Row {
            section: "§53",
            capability: "Sort + clustering keys",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "sort.rs fixed-size §53 entries; accept/reject corpus; `SortKey::serialize` round-trips with parser",
        },
        Row {
            section: "§54",
            capability: "RowRef",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "row_ref.rs; standalone accept/reject corpus plus lookup-index integration",
        },
        Row {
            section: "§56",
            capability: "COVE-O object type catalog",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "profile/cove_o.rs; accept/reject catalog plus required/optional corpus; ObjectTypeCatalog serializer round-trips with parser",
        },
        Row {
            section: "§57",
            capability: "COVE-O temporal segment index",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "profile/cove_o.rs; accept/reject temporal-index corpus; TemporalSegmentIndex serializer round-trips with parser",
        },
        Row {
            section: "§58",
            capability: "COVE-O temporal row order",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "profile/cove_o.rs temporal segment parser plus semantic reader integration; full-file accept/reject corpus covers lexicographic order and CSN nondecreasing row-order enforcement",
        },
        Row {
            section: "§60",
            capability: "COVE-O self-containment",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "profile/cove_o.rs prev_ref parsing plus semantic reader integration; full-file accept/reject corpus; segment serializer enforces prev_ref on round-trip",
        },
        Row {
            section: "§61",
            capability: "COVE-O property columns",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "profile/cove_o.rs parses temporal property column directories, page indexes, and payload metadata; cove_map_convert_case validates generated COVE-O property pages and association readback metadata",
        },
        Row {
            section: "§62",
            capability: "COVE-O temporal bloom index",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "TemporalBloomIndex parser/serializer; corpus covers valid filter ranges, bad entry CRC, out-of-range filter payloads, and inverted time buckets rejected as bad optional indexes",
        },
        Row {
            section: "§63",
            capability: "Trust chain",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "TrustManifest parser plus trust_chain.rs reader integration; full-file accept/reject corpus; `TrustManifest::serialize` round-trips with parser",
        },
        Row {
            section: "§64",
            capability: "Redaction manifest",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "redaction.rs; accept/reject corpus; `RedactionManifest::serialize` round-trips with parser",
        },
        Row {
            section: "§65",
            capability: "Digest manifest",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "digest.rs parses §65 header/entry wire format with header checksum and range-based verification; accept/reject manifest corpus plus --verify-digests coverage; `DigestManifest::serialize` round-trips with parser",
        },
        Row {
            section: "§66",
            capability: "Compression",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "compression.rs plus writer.rs and page.rs; section LZ4/Zstd payloads, page-level codec dispatch with codec/length/reserved-bit invariants, and `page_codec_case` round-trip plus rejection corpus",
        },
        Row {
            section: "§67",
            capability: "I/O hints",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "io_hints.rs parses the six-field §67 `CoveIoHintV1`; accept/reject corpus; `IoHints::encode` round-trips with parser",
        },
        Row {
            section: "§68",
            capability: "COVX sidecar",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "artifact/covx.rs; accept/reject artifact corpus plus mount-time freshness/fallback fixtures; `CovxFile::serialize` round-trips with parser",
        },
        Row {
            section: "§69",
            capability: "COVM manifest",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "artifact/covm.rs; accept/reject manifest corpus plus mount-time freshness/fallback fixtures; `CovmFile::serialize` round-trips with parser",
        },
        Row {
            section: "§70",
            capability: "COVEMAP artifact framing",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "artifact/covemap.rs; accept/reject artifact corpus; `CovemapFile::serialize` round-trips with parser",
        },
        Row {
            section: "§70.2",
            capability: "COVE-MAP source catalog and replay fingerprints",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "profile/cove_map.rs parses source catalogs; cove-map rejects undeclared, duplicate, missing, and stale replay sources; corpus covers valid execution and missing-source rejection",
        },
        Row {
            section: "§70.3",
            capability: "COVE-MAP row semantics",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "profile/cove_map.rs parses row semantic rules and validates source/identity references; generated execution mapping and conversion corpus exercise object/property/association rules",
        },
        Row {
            section: "§70.5",
            capability: "COVE-MAP semantic join keys",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "cove-map builds canonical length-delimited join key tuples from declared components/functions; execution corpus covers multi-rule identity planning",
        },
        Row {
            section: "§70.6",
            capability: "COVE-MAP deterministic identity resolution",
            modeled: "yes",
            parsed: "n/a",
            validated: "yes",
            written: "yes",
            notes: "cove-map resolves deterministic identity components, applies do-not-merge constraints, and emits equivalence/evidence records; conversion corpus executes the path",
        },
        Row {
            section: "§70.8",
            capability: "COVE-MAP property conflict resolution",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "profile/cove_map.rs parses declared conflict policies; cove-map groups property candidates by destination GOID/property, rejects unresolved conflicts, applies source-priority winners, and records suppressed evidence",
        },
        Row {
            section: "§70.9",
            capability: "COVE-MAP association roles and validity readback",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "profile/cove_map.rs parses association endpoint roles, cardinality, and validity expressions; cove-map materializes stable association properties and projection readback exposes them",
        },
        Row {
            section: "§70.10",
            capability: "COVE-MAP projection readback",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "profile/cove_map.rs expanded ProjectionCatalog schema plus cove_map_project_case corpus executing cove-map library projection over generated mapping/source fixtures; legacy projection previews fail closed",
        },
        Row {
            section: "§70.12",
            capability: "COVE-MAP provenance and evidence",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "profile/cove_map.rs validates evidence source/rule/assertion references; cove-map materializes evidence indexes and conversion corpus checks generated evidence counts",
        },
        Row {
            section: "§70.13",
            capability: "COVE-MAP deterministic function registry",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "profile/cove_map.rs rejects undeclared or nondeterministic functions; cove-map conversion executes declared canonicalization functions",
        },
        Row {
            section: "§70.14",
            capability: "COVE-MAP governance reconciliation metadata",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "profile/cove_map.rs parses source sensitivity labels/ranks and access policies; cove-map emits effective governance metadata by default and rejects mixed sensitivity when requested",
        },
        Row {
            section: "§72.8",
            capability: "COVE-MAP object/association conversion",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "cove_map_convert_case corpus executes cove-map library conversion, checks object/association/report/evidence counts, and semantically validates generated COVE-O output",
        },
        Row {
            section: "§72",
            capability: "Writer profiles",
            modeled: "yes",
            parsed: "n/a",
            validated: "yes",
            written: "yes",
            notes: "Minimal writer plus ScanProfileCoveWriter; generated COVE-T scan fixture now includes column/page regions",
        },
        Row {
            section: "§73",
            capability: "Validation model",
            modeled: "yes",
            parsed: "n/a",
            validated: "yes",
            written: "n/a",
            notes: "reader stages + cove-validate; required/optional profile corpus; shared, COVE-T, and COVE-O semantic invariants exercised end to end",
        },
        Row {
            section: "§74",
            capability: "Recovery and failure behavior",
            modeled: "yes",
            parsed: "n/a",
            validated: "yes",
            written: "n/a",
            notes: "reader fail-closed structural corruption, optional profile/index ignore paths, unknown optional feature acceptance, required feature rejection, and MAP/stale/sidecar error fixtures are tagged as recovery evidence",
        },
        Row {
            section: "§75",
            capability: "Durable replace",
            modeled: "yes",
            parsed: "n/a",
            validated: "yes",
            written: "yes",
            notes: "durable.rs plus writer.rs and COVE-family CLIs; durable_publish_case corpus validates publication through durable_replace",
        },
        Row {
            section: "§76",
            capability: "Error-code surface",
            modeled: "yes",
            parsed: "n/a",
            validated: "yes",
            written: "n/a",
            notes: "error.rs spec_code()/ALL_SPEC_CODES, cove-validate JSON output, and generated reject fixtures including error_surface_case coverage",
        },
        Row {
            section: "§77",
            capability: "Compatibility",
            modeled: "yes",
            parsed: "n/a",
            validated: "yes",
            written: "n/a",
            notes: "registry.rs compatibility_rules plus unknown optional/required feature fixtures and optional/required profile behavior corpus",
        },
        Row {
            section: "§78",
            capability: "Conformance requirements",
            modeled: "yes",
            parsed: "n/a",
            validated: "yes",
            written: "n/a",
            notes: "conformance runner validates bootstrap, structural, profile, writer, recovery, and COVE-MAP aware requirements through generated manifest evidence",
        },
        Row {
            section: "§79",
            capability: "Open conformance/benchmark suite",
            modeled: "yes",
            parsed: "n/a",
            validated: "yes",
            written: "yes",
            notes: "suite_contract_case corpus, CLI smoke tests including cove-convert-parquet, release-gate bench/conformance smoke, and deterministic robustness harness",
        },
    ]
}

fn gate_met(value: &str) -> bool {
    matches!(value, "yes" | "n/a")
}

fn non_corpus_gates_met(row: &Row) -> bool {
    gate_met(row.modeled)
        && gate_met(row.parsed)
        && gate_met(row.validated)
        && gate_met(row.written)
}

fn row_gate_met(row: &Row, corpus_status: &str) -> bool {
    non_corpus_gates_met(row) && corpus_status == "yes"
}

fn corpus_requirement(row: &Row) -> CorpusRequirement {
    let reject_exemption = match row.section {
        "§8" => Some("wire negative cases are executable fixture operations inside accept corpus rows"),
        "§29" => Some("predicate truth-table failures are asserted as expected outcomes inside pruning fixtures"),
        "§37" => Some("fail-open and negative pruning paths are asserted inside pruning fixtures"),
        "§51" => Some("conversion fixtures are positive CLI/library conversions without a stable negative corpus surface"),
        "§61" => Some("property-column coverage is validated through generated COVE-O outputs"),
        "§70.3" => Some("row-semantic failures are covered through referenced MAP validation rows"),
        "§70.5" => Some("join-key behavior is validated through deterministic execution fixtures and unit tests"),
        "§70.6" => Some("identity-resolution negative behavior is covered by do-not-merge unit tests"),
        "§70.9" => Some("association readback is a positive preservation contract"),
        "§70.10" => Some("projection readback is a positive preservation contract"),
        "§70.12" => Some("evidence rejection is covered by referenced MAP validation rows"),
        "§70.13" => Some("function-registry rejection is covered by COVE_E_MAP_FUNCTION_UNDECLARED fixtures"),
        "§75" => Some("durable replace is a positive publication contract"),
        "§78" => Some("suite requirements are self-checking accept fixtures"),
        "§79" => Some("open suite packaging is checked by suite-contract accept fixtures"),
        _ => None,
    };
    if row.section == "§76" {
        return CorpusRequirement {
            min_accept: 0,
            min_reject: 1,
            reject_exemption: Some("error-code surface is demonstrated by reject fixtures"),
        };
    }
    CorpusRequirement {
        min_accept: 1,
        min_reject: if reject_exemption.is_some() { 0 } else { 1 },
        reject_exemption,
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;

    fn test_row(section: &'static str) -> Row {
        Row {
            section,
            capability: "Test capability",
            modeled: "yes",
            parsed: "yes",
            validated: "yes",
            written: "yes",
            notes: "test notes",
        }
    }

    #[test]
    fn section_prefix_matching_handles_parent_and_child_sections() {
        assert!(section_matches("§70.8", "§70"));
        assert!(section_matches("§70.8", "§70.8"));
        assert!(!section_matches("§71", "§70"));
        assert!(!section_matches("§700", "§70"));
    }

    #[test]
    fn insufficient_manifest_evidence_is_not_fully_gated() {
        let rows = vec![test_row("§70.8")];
        let evidence = collect_manifest_evidence(
            &rows,
            r#"{"path":"accept/x.json","kind":"case","expect":"accept","sections":["§70.8"]}"#,
        );
        let row = &rows[0];
        let status = corpus_status(evidence[row.section], corpus_requirement(row));
        assert_eq!(status, "partial");
        assert!(!row_gate_met(row, status));
    }

    #[test]
    fn reject_exemptions_are_required_and_rendered() {
        let row = test_row("§70.9");
        let requirement = corpus_requirement(&row);
        assert_eq!(requirement.min_reject, 0);
        assert!(requirement.reject_exemption.is_some());
        let note = evidence_note(
            &row,
            CorpusEvidence {
                accept: 1,
                reject: 0,
            },
            requirement,
        );
        assert!(note.contains("corpus accept=1 reject=0"));
        assert!(note.contains("reject exemption:"));
        assert_eq!(
            corpus_status(
                CorpusEvidence {
                    accept: 1,
                    reject: 0,
                },
                requirement
            ),
            "yes"
        );
    }
}

fn section_matches(fixture_section: &str, row_section: &str) -> bool {
    fixture_section == row_section || fixture_section.starts_with(&format!("{row_section}."))
}

fn collect_manifest_evidence(
    rows: &[Row],
    manifest_text: &str,
) -> BTreeMap<String, CorpusEvidence> {
    let mut evidence = rows
        .iter()
        .map(|row| (row.section.to_string(), CorpusEvidence::default()))
        .collect::<BTreeMap<_, _>>();
    for (line_number, line) in manifest_text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let entry: Value = serde_json::from_str(line).unwrap_or_else(|err| {
            panic!(
                "invalid conformance manifest line {}: {err}",
                line_number + 1
            )
        });
        let expect = entry.get("expect").and_then(Value::as_str);
        let sections = entry
            .get("sections")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        for row in rows {
            let matched = sections
                .iter()
                .filter_map(Value::as_str)
                .any(|section| section_matches(section, row.section));
            if !matched {
                continue;
            }
            let row_evidence = evidence.get_mut(row.section).expect("row evidence exists");
            match expect {
                Some("accept") => row_evidence.accept += 1,
                Some("reject") => row_evidence.reject += 1,
                _ => {}
            }
        }
    }
    evidence
}

fn corpus_status(evidence: CorpusEvidence, requirement: CorpusRequirement) -> &'static str {
    if evidence.accept >= requirement.min_accept && evidence.reject >= requirement.min_reject {
        "yes"
    } else {
        "partial"
    }
}

fn evidence_note(row: &Row, evidence: CorpusEvidence, requirement: CorpusRequirement) -> String {
    let mut note = format!(
        "{}; corpus accept={} reject={}",
        row.notes, evidence.accept, evidence.reject
    );
    if let Some(exemption) = requirement.reject_exemption {
        note.push_str("; reject exemption: ");
        note.push_str(exemption);
    }
    note
}

fn check_mode() -> bool {
    std::env::args().any(|arg| arg == "--check")
}

fn spec_sections(spec_text: &str) -> BTreeSet<String> {
    spec_text
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim_start_matches('#').trim_start();
            let number = trimmed.split_whitespace().next()?;
            let number = number.strip_suffix('.').unwrap_or(number);
            if number.chars().all(|ch| ch.is_ascii_digit() || ch == '.') {
                Some(format!("§{number}"))
            } else {
                None
            }
        })
        .collect()
}

fn validate_row_sections_exist(rows: &[Row], spec_text: &str) {
    let sections = spec_sections(spec_text);
    let missing = rows
        .iter()
        .filter(|row| !sections.contains(row.section))
        .map(|row| format!("{} {}", row.section, row.capability))
        .collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "capability matrix references section ids missing from Spec.md: {}",
        missing.join(", ")
    );
}

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("conformance");
    let spec_path = root.parent().unwrap().join("Spec.md");
    let spec_text = fs::read_to_string(&spec_path)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", spec_path.display()));
    let manifest_path = root.join("manifest.jsonl");
    let manifest_text = fs::read_to_string(&manifest_path)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", manifest_path.display()));
    fs::create_dir_all(&root).unwrap();
    let path = root.join("capability_matrix.md");

    let mut out = String::new();
    out.push_str("# COVE v1.0 Capability Matrix (Spec §71)\n\n");
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
    validate_row_sections_exist(&rows, &spec_text);
    let evidence_by_section = collect_manifest_evidence(&rows, &manifest_text);
    let mut missing_corpus_evidence = Vec::new();
    for r in &rows {
        total += 1;
        let evidence = evidence_by_section
            .get(r.section)
            .copied()
            .unwrap_or_default();
        let requirement = corpus_requirement(r);
        let corpus = corpus_status(evidence, requirement);
        if row_gate_met(r, corpus) {
            fully_gated += 1;
        }
        if check_mode() && non_corpus_gates_met(r) && corpus != "yes" {
            missing_corpus_evidence.push(format!(
                "{} {} needs accept>={} reject>={} but has accept={} reject={}",
                r.section,
                r.capability,
                requirement.min_accept,
                requirement.min_reject,
                evidence.accept,
                evidence.reject
            ));
        }
        let notes = evidence_note(r, evidence, requirement);
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
            r.section, r.capability, r.modeled, r.parsed, r.validated, r.written, corpus, notes,
        ));
    }
    out.push_str(&format!(
        "\n**Fully gated capabilities:** {fully_gated} / {total}\n"
    ));
    out.push_str("\n**Intentionally contextual or indirectly tracked sections:** §1-§7, §11, §14, §18, §38-§39, §48, §55, §59, §71, §80-§81. These sections define terminology, invariants, registries, or suite process rather than a single reference-code capability row.\n");

    if check_mode() {
        assert!(
            missing_corpus_evidence.is_empty(),
            "fully modeled capability rows are missing manifest evidence:\n{}",
            missing_corpus_evidence.join("\n")
        );
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
