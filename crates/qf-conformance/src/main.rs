//! `qf-conformance` — Quay Format conformance corpus runner (Spec §70, §73, §75, §77).
//!
//! Walks a corpus directory of fixtures plus a JSON manifest that
//! maps each fixture to (a) the spec sections it exercises and (b) the
//! expected outcome (accept / reject-with-error-code). Prints a summary and
//! exits non-zero on any mismatch.
//!
//! Corpus layout:
//! ```text
//! conformance/
//!   manifest.jsonl
//!   accept/<fixture>
//!   reject/<fixture>
//! ```
//!
//! Manifest format (one JSON object per line):
//! ```json
//! {"path":"accept/min.quay","kind":"qf","expect":"accept","sections":["§9","§10"]}
//! {"path":"reject/bad_crc.quay","kind":"qf","expect":"reject","error_code":"QF_E_CHECKSUM_MISMATCH","sections":["§13"]}
//! ```

use std::{path::Path, process};

use qf_core::{
    artifact::{qfm::QfmFile, qfx::QfxFile},
    collation::CollationRegistry,
    digest::DigestManifest,
    domain::ColumnDomain,
    index::{
        aggregate::AggregateSynopsis, bloom::BloomFilterIndex, composite::CompositeIndex,
        exact_set::ExactSetIndex, inverted::InvertedMorselIndex, lookup::LookupIndex,
        topn::TopNSummary,
    },
    interop::lakehouse::LakehouseHints,
    io_hints::IoHints,
    kernel::KernelCapabilities,
    metadata::MetadataJson,
    page::PageIndex,
    profile::{
        qfe::{EngineMountPolicyV1, EngineProfileRegistry, ExecutionCodeDescriptorV1},
        qfh::HarborMountHintsV1,
        qfo::{ObjectTypeCatalog, TemporalSegmentIndex},
    },
    reader::{self, ValidationOptions},
    redaction::RedactionManifest,
    segment::{RowMorselDirectory, TableSegmentHeaderV1, TableSegmentIndex},
    sort::{ClusteringKeyEntryV1, SortKeyEntryV1},
    table::TableCatalog,
    QfError,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: qf-conformance <corpus-dir>");
        process::exit(2);
    }
    let corpus = Path::new(&args[1]);
    let manifest = corpus.join("manifest.jsonl");
    let manifest_bytes = match std::fs::read(&manifest) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cannot read manifest {}: {}", manifest.display(), e);
            process::exit(2);
        }
    };
    let text = String::from_utf8_lossy(&manifest_bytes);
    let mut total = 0usize;
    let mut passed = 0usize;
    for (lineno, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let entry = match parse_entry(line) {
            Some(e) => e,
            None => {
                eprintln!("manifest line {}: malformed", lineno + 1);
                continue;
            }
        };
        total += 1;
        let path = corpus.join(&entry.path);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("FAIL {} (read error: {})", entry.path, e);
                continue;
            }
        };
        let result = validate_fixture(&entry, &bytes);
        let ok = match (entry.expect.as_str(), &result) {
            ("accept", Ok(_)) => true,
            ("reject", Err(e)) => {
                if let Some(want) = &entry.error_code {
                    e.spec_code() == Some(want.as_str())
                } else if let Some(want) = &entry.error {
                    let dbg = format!("{:?}", e);
                    let disp = e.to_string();
                    dbg.contains(want) || disp.contains(want)
                } else {
                    true
                }
            }
            _ => false,
        };
        if ok {
            passed += 1;
            println!("PASS {}", entry.path);
        } else {
            let actual = match &result {
                Ok(()) => "accept".to_string(),
                Err(error) => format!("reject ({error})"),
            };
            println!(
                "FAIL {} (kind {}, expected {}, got {})",
                entry.path, entry.kind, entry.expect, actual
            );
        }
    }
    println!("\n{passed}/{total} fixtures passed");
    process::exit(if passed == total { 0 } else { 1 });
}

struct Entry {
    path: String,
    kind: String,
    expect: String,
    error_code: Option<String>,
    error: Option<String>,
    morsel_count: Option<u32>,
}

fn parse_entry(line: &str) -> Option<Entry> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let path = value.get("path")?.as_str()?.to_string();
    let kind = value
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or("qf")
        .to_string();
    let expect = value.get("expect")?.as_str()?.to_string();
    let error_code = value
        .get("error_code")
        .and_then(|v| v.as_str())
        .map(ToOwned::to_owned);
    let error = value
        .get("error")
        .and_then(|v| v.as_str())
        .map(ToOwned::to_owned);
    let morsel_count = value
        .get("morsel_count")
        .and_then(|v| v.as_u64())
        .and_then(|v| u32::try_from(v).ok());
    Some(Entry {
        path,
        kind,
        expect,
        error_code,
        error,
        morsel_count,
    })
}

fn validate_fixture(entry: &Entry, bytes: &[u8]) -> Result<(), QfError> {
    match entry.kind.as_str() {
        "qf" => reader::validate_bytes_with_options(
            bytes,
            ValidationOptions {
                semantic: true,
                verify_digests: false,
                allow_unknown_optional_extensions: true,
            },
        )
        .map(|_| ()),
        "qfx" => QfxFile::parse(bytes).map(|_| ()),
        "qfm" => QfmFile::parse(bytes).map(|_| ()),
        "metadata_json" => MetadataJson::parse(bytes).map(|_| ()),
        "collation_registry" => CollationRegistry::parse(bytes).map(|_| ()),
        "digest_manifest" => DigestManifest::parse(bytes).map(|_| ()),
        "redaction_manifest" => RedactionManifest::parse(bytes).map(|_| ()),
        "io_hints" => IoHints::parse(bytes).map(|_| ()),
        "lakehouse_hints" => LakehouseHints::parse(bytes).map(|_| ()),
        "kernel_capabilities" => KernelCapabilities::parse(bytes).map(|_| ()),
        "page_index" => PageIndex::parse(bytes).map(|_| ()),
        "column_domain" => ColumnDomain::parse(bytes).map(|_| ()),
        "table_catalog" => TableCatalog::parse(bytes).map(|_| ()),
        "table_segment_index" => TableSegmentIndex::parse(bytes).map(|_| ()),
        "table_segment_header" => TableSegmentHeaderV1::parse(bytes).map(|_| ()),
        "row_morsel_directory" => RowMorselDirectory::parse(
            bytes,
            entry.morsel_count.ok_or_else(|| {
                QfError::BadSection("row_morsel_directory fixture missing morsel_count".into())
            })?,
        )
        .map(|_| ()),
        "exact_set_index" => ExactSetIndex::parse(bytes).map(|_| ()),
        "bloom_index" => BloomFilterIndex::parse(bytes).map(|_| ()),
        "inverted_morsel_index" => InvertedMorselIndex::parse(bytes).map(|_| ()),
        "lookup_index" => LookupIndex::parse(bytes).map(|_| ()),
        "aggregate_synopsis" => AggregateSynopsis::parse(bytes).map(|_| ()),
        "composite_zone_index" => CompositeIndex::parse(bytes).map(|_| ()),
        "topn_summary" => TopNSummary::parse(bytes).map(|_| ()),
        "sort_key" => SortKeyEntryV1::parse(bytes).map(|_| ()),
        "clustering_key" => ClusteringKeyEntryV1::parse(bytes).map(|_| ()),
        "qfe_engine_registry" => EngineProfileRegistry::parse(bytes).map(|_| ()),
        "qfe_execution_code" => ExecutionCodeDescriptorV1::parse(bytes).map(|_| ()),
        "qfe_mount_policy" => EngineMountPolicyV1::parse(bytes).map(|_| ()),
        "qfh_mount_hints" => HarborMountHintsV1::parse(bytes).map(|_| ()),
        "qfo_object_catalog" => ObjectTypeCatalog::parse(bytes).map(|_| ()),
        "qfo_temporal_segment_index" => TemporalSegmentIndex::parse(bytes).map(|_| ()),
        other => Err(QfError::BadSection(format!(
            "unknown conformance fixture kind {other}"
        ))),
    }
}
