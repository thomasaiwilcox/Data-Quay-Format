//! `cove-conformance` — Cove Format conformance corpus runner (Spec §70, §73, §75, §77).
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
//! {"path":"accept/min.cove","kind":"cove","expect":"accept","sections":["§9","§10"]}
//! {"path":"reject/bad_crc.cove","kind":"cove","expect":"reject","error_code":"COVE_E_CHECKSUM_MISMATCH","sections":["§13"]}
//! ```

use std::{borrow::Cow, collections::BTreeSet, path::Path, process};

use arrow_array::{Array, BinaryArray, BooleanArray, Int32Array, StringArray, UInt64Array};
use serde_json::{json, Value};

use cove_core::{
    array::{CoveArrayValue, EncodedArray},
    artifact::{covemap::CovemapFile, covm::CovmFile, covx::CovxFile},
    checksum,
    collation::CollationRegistry,
    compression::{column_page_payload, encode_page_payload, section_payload},
    constants::{
        CompressionCodec, CoveEncodingKind, CoveLogicalType, CovePhysicalKind, SectionKind,
    },
    dictionary::FileDictionary,
    digest::DigestManifest,
    domain::ColumnDomain,
    encoding::{
        assert_parity,
        bit_packed::{BitPacked, BitPackedPayload},
        constant::{Constant, ConstantPayload},
        delta::{Delta, DeltaPayload},
        frame_of_reference::{ForPayload, FrameOfReference},
        local_codebook::{LocalCodebook, LocalCodebookPayload},
        nested::{ListLayout, MapLayout, StructLayout},
        patched_base::{PatchedBase, PatchedBasePayload},
        plain::{PlainFixed, PlainFixedPayload, PlainVarint, PlainVarintPayload},
        rle::{Rle, RlePayload},
        run_end::{RunEnd, RunEndPayload},
        sparse::{Sparse, SparsePayload},
        Encoding,
    },
    index::{
        aggregate::AggregateSynopsis,
        bloom::BloomFilterIndex,
        composite::CompositeIndex,
        exact_set::{
            ExactSetGranularity, ExactSetIndex, ExactSetIndexHeaderV1, ExactSetKeyKind,
            ExactSetRepresentation,
        },
        inverted::InvertedMorselIndex,
        lookup::LookupIndex,
        topn::TopNSummary,
    },
    interop::{
        arrow::{arrow_validity_to_cove_null, cove_null_to_arrow_validity, encoded_array_to_arrow},
        lakehouse::LakehouseHints,
        parquet::{
            convert_parquet_bytes, decode_materialized_page_values, ParquetConversionOptions,
        },
    },
    io_hints::IoHints,
    kernel::KernelCapabilities,
    metadata::MetadataJson,
    page::{ColumnPageIndex, ColumnPageIndexEntryV1, PageIndex},
    profile::{
        cove_e::{
            CodeSpaceDescriptorV1, EngineMountPolicyV1, EngineProfileRegistry,
            ExecutionCodeDescriptorV1, ExecutionScopeDescriptorV1,
        },
        cove_h::HarborMountHintsV1,
        cove_o::{ObjectTypeCatalog, TemporalSegmentIndex},
    },
    pruning::{
        explain_aggregate_synopsis, explain_bloom_membership, explain_composite_zone,
        explain_file_code_equality, explain_inverted_morsel_lookup, explain_is_not_null,
        explain_is_null, explain_lookup_index_point, explain_numcode_range,
        explain_resolved_domain_rank_range, PruningEvidence, PruningExplanation,
    },
    reader::{self, ValidationOptions},
    redaction::RedactionManifest,
    row_ref::RowRef,
    segment::{RowMorselDirectory, TableSegmentHeaderV1, TableSegmentIndex, TableSegmentPayloadV1},
    sort::{ClusteringKeyEntryV1, SortKeyEntryV1},
    table::TableCatalog,
    wire::{
        decode_u64_leb128, encode_u64_leb128, parse_bool_strict, zigzag_decode_i64,
        zigzag_encode_i64,
    },
    zone_stats::{
        NumericStatValue, StatKind, StatScalar, ZoneScope, ZoneStatFlags, ZoneStats, ZoneStatsEntry,
    },
    CoveError,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: cove-conformance <corpus-dir>");
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
        let result = validate_fixture(&entry, corpus, &bytes);
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
    raw: Value,
}

fn parse_entry(line: &str) -> Option<Entry> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let path = value.get("path")?.as_str()?.to_string();
    let kind = value
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or("cove")
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
        raw: value,
    })
}

fn validate_fixture(entry: &Entry, corpus: &Path, bytes: &[u8]) -> Result<(), CoveError> {
    match entry.kind.as_str() {
        "cove" => reader::validate_bytes_with_options(
            bytes,
            ValidationOptions {
                semantic: true,
                verify_digests: false,
                allow_unknown_optional_extensions: true,
            },
        )
        .map(|_| ()),
        "covemap" => CovemapFile::parse(bytes).map(|_| ()),
        "covx" => CovxFile::parse(bytes).map(|_| ()),
        "covm" => CovmFile::parse(bytes).map(|_| ()),
        "metadata_json" => MetadataJson::parse(bytes).map(|_| ()),
        "encoding_case" => validate_encoding_fixture(bytes),
        "encoded_array_decode_case" => validate_encoded_array_decode_fixture(bytes),
        "nested_case" => validate_nested_fixture(bytes),
        "arrow_bitmap_case" => validate_arrow_bitmap_fixture(bytes),
        "arrow_export_case" => validate_arrow_export_fixture(bytes),
        "parquet_conversion_case" => validate_parquet_conversion_fixture(entry, bytes),
        "error_surface_case" => validate_error_surface_fixture(bytes),
        "suite_contract_case" => validate_suite_contract_fixture(corpus, bytes),
        "file_dictionary" => validate_file_dictionary_fixture(bytes),
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
                CoveError::BadSection("row_morsel_directory fixture missing morsel_count".into())
            })?,
        )
        .map(|_| ()),
        "exact_set_index" => ExactSetIndex::parse(bytes).map(|_| ()),
        "bloom_index" => BloomFilterIndex::parse(bytes).map(|_| ()),
        "inverted_morsel_index" => InvertedMorselIndex::parse(bytes).map(|_| ()),
        "lookup_index" => LookupIndex::parse(bytes).map(|_| ()),
        "row_ref" => RowRef::decode(bytes).map(|_| ()),
        "aggregate_synopsis" => AggregateSynopsis::parse(bytes).map(|_| ()),
        "composite_zone_index" => CompositeIndex::parse(bytes).map(|_| ()),
        "topn_summary" => TopNSummary::parse(bytes).map(|_| ()),
        "sort_key" => SortKeyEntryV1::parse(bytes).map(|_| ()),
        "clustering_key" => ClusteringKeyEntryV1::parse(bytes).map(|_| ()),
        "cove_e_engine_registry" => EngineProfileRegistry::parse(bytes).map(|_| ()),
        "cove_e_execution_code" => ExecutionCodeDescriptorV1::parse(bytes).map(|_| ()),
        "cove_e_execution_scope" => ExecutionScopeDescriptorV1::parse(bytes).map(|_| ()),
        "cove_e_code_space" => CodeSpaceDescriptorV1::parse(bytes).map(|_| ()),
        "cove_e_mount_policy" => EngineMountPolicyV1::parse(bytes).map(|_| ()),
        "cove_h_mount_hints" => HarborMountHintsV1::parse(bytes).map(|_| ()),
        "cove_o_object_catalog" => ObjectTypeCatalog::parse(bytes).map(|_| ()),
        "cove_o_temporal_segment_index" => TemporalSegmentIndex::parse(bytes).map(|_| ()),
        "pruning_case" => validate_pruning_fixture(bytes),
        "page_codec_case" => validate_page_codec_fixture(bytes),
        "wire_primitive_case" => validate_wire_primitive_fixture(bytes),
        other => Err(CoveError::BadSection(format!(
            "unknown conformance fixture kind {other}"
        ))),
    }
}

fn validate_suite_contract_fixture(corpus: &Path, bytes: &[u8]) -> Result<(), CoveError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|error| {
        CoveError::BadSection(format!("invalid suite-contract fixture json: {error}"))
    })?;
    let op = value
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| CoveError::BadSection("suite-contract fixture missing op".into()))?;

    match op {
        "manifest_sections_present" => validate_suite_manifest_contract(corpus, &value),
        "release_gate_contains" => validate_release_gate_contract(corpus, &value),
        "workspace_members_present" => validate_workspace_contract(corpus, &value),
        other => Err(CoveError::BadSection(format!(
            "unsupported suite-contract op {other}"
        ))),
    }
}

fn validate_suite_manifest_contract(corpus: &Path, value: &Value) -> Result<(), CoveError> {
    let manifest_path = corpus.join("manifest.jsonl");
    let manifest = std::fs::read_to_string(&manifest_path).map_err(|error| {
        CoveError::BadSection(format!(
            "cannot read suite manifest {}: {error}",
            manifest_path.display()
        ))
    })?;
    let required_sections = parse_fixture_string_vector(value.get("sections"), "sections")?;
    let minimum_accept = value
        .get("minimum_accept")
        .and_then(Value::as_u64)
        .unwrap_or(1) as usize;
    let minimum_reject = value
        .get("minimum_reject")
        .and_then(Value::as_u64)
        .unwrap_or(1) as usize;

    let mut seen_sections = BTreeSet::new();
    let mut accept_count = 0usize;
    let mut reject_count = 0usize;
    for (line_number, line) in manifest.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let entry: Value = serde_json::from_str(line).map_err(|error| {
            CoveError::BadSection(format!(
                "invalid manifest line {} for suite contract: {error}",
                line_number + 1
            ))
        })?;
        match entry.get("expect").and_then(Value::as_str) {
            Some("accept") => accept_count += 1,
            Some("reject") => reject_count += 1,
            _ => {}
        }
        if let Some(sections) = entry.get("sections").and_then(Value::as_array) {
            for section in sections.iter().filter_map(Value::as_str) {
                seen_sections.insert(section.to_string());
            }
        }
    }

    if accept_count < minimum_accept {
        return Err(CoveError::BadSection(format!(
            "suite contract requires at least {minimum_accept} accept fixtures, found {accept_count}"
        )));
    }
    if reject_count < minimum_reject {
        return Err(CoveError::BadSection(format!(
            "suite contract requires at least {minimum_reject} reject fixtures, found {reject_count}"
        )));
    }
    for section in required_sections {
        let matched = seen_sections
            .iter()
            .any(|seen| seen == &section || seen.starts_with(&format!("{section}.")));
        if !matched {
            return Err(CoveError::BadSection(format!(
                "suite contract missing manifest coverage for {section}"
            )));
        }
    }

    Ok(())
}

fn validate_release_gate_contract(corpus: &Path, value: &Value) -> Result<(), CoveError> {
    let repo_root = corpus.parent().ok_or_else(|| {
        CoveError::BadSection("cannot locate repository root from conformance corpus".into())
    })?;
    let gate_path = repo_root.join("scripts/release-gates.sh");
    let contents = std::fs::read_to_string(&gate_path).map_err(|error| {
        CoveError::BadSection(format!(
            "cannot read release-gate script {}: {error}",
            gate_path.display()
        ))
    })?;
    for needle in parse_fixture_string_vector(value.get("needles"), "needles")? {
        if !contents.contains(&needle) {
            return Err(CoveError::BadSection(format!(
                "release-gate script missing required command: {needle}"
            )));
        }
    }
    Ok(())
}

fn validate_workspace_contract(corpus: &Path, value: &Value) -> Result<(), CoveError> {
    let repo_root = corpus.parent().ok_or_else(|| {
        CoveError::BadSection("cannot locate repository root from conformance corpus".into())
    })?;
    let cargo_toml_path = repo_root.join("Cargo.toml");
    let cargo_toml = std::fs::read_to_string(&cargo_toml_path).map_err(|error| {
        CoveError::BadSection(format!(
            "cannot read workspace manifest {}: {error}",
            cargo_toml_path.display()
        ))
    })?;
    for member in parse_fixture_string_vector(value.get("members"), "members")? {
        let needle = format!("\"{member}\"");
        if !cargo_toml.contains(&needle) {
            return Err(CoveError::BadSection(format!(
                "workspace manifest missing required member {member}"
            )));
        }
    }
    Ok(())
}

fn parse_fixture_string_vector(
    value: Option<&Value>,
    field: &str,
) -> Result<Vec<String>, CoveError> {
    let values = value
        .and_then(Value::as_array)
        .ok_or_else(|| CoveError::BadSection(format!("fixture missing {field}")))?;
    let mut out = Vec::with_capacity(values.len());
    for (index, item) in values.iter().enumerate() {
        let string = item.as_str().ok_or_else(|| {
            CoveError::BadSection(format!("fixture field {field}[{index}] is not a string"))
        })?;
        out.push(string.to_string());
    }
    Ok(out)
}

fn validate_error_surface_fixture(bytes: &[u8]) -> Result<(), CoveError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|error| {
        CoveError::BadSection(format!("invalid error-surface fixture json: {error}"))
    })?;
    let code = value
        .get("code")
        .and_then(Value::as_str)
        .ok_or_else(|| CoveError::BadSection("error-surface fixture missing code".into()))?;
    let error = synthetic_error_surface_error(code).ok_or_else(|| {
        CoveError::BadSection(format!("unsupported error-surface fixture code {code}"))
    })?;
    if error.spec_code() != Some(code) {
        return Err(CoveError::BadSection(format!(
            "error-surface fixture code {code} does not match spec_code()"
        )));
    }
    if !error.to_string().contains(code) {
        return Err(CoveError::BadSection(format!(
            "error-surface fixture code {code} is not present in display output"
        )));
    }
    Err(error)
}

fn synthetic_error_surface_error(code: &str) -> Option<CoveError> {
    match code {
        "COVE_E_BAD_VERSION" => Some(CoveError::BadVersion),
        "COVE_E_ARITH_OVERFLOW" => Some(CoveError::ArithOverflow),
        "COVE_E_DICT_MISS" => Some(CoveError::DictMiss),
        "COVE_E_BAD_FILECODE" => Some(CoveError::BadFileCode),
        "COVE_E_BAD_NUMCODE" => Some(CoveError::BadNumCode),
        "COVE_E_BAD_EXTENSION" => Some(CoveError::BadExtension),
        "COVE_E_EXECUTION_CODE_MAP" => Some(CoveError::ExecutionCodeMap),
        "COVE_E_HARBOR_MOUNT_LEASE" => Some(CoveError::HarborMountLease),
        "COVE_E_NOT_SELF_CONTAINED" => Some(CoveError::NotSelfContained),
        "COVE_E_REDACTION_POLICY" => Some(CoveError::RedactionPolicy),
        "COVE_E_SIDECAR_STALE" => Some(CoveError::SidecarStale),
        "COVE_E_MAP_INVALID" => Some(CoveError::MapInvalid),
        "COVE_E_MAP_FUNCTION_UNDECLARED" => Some(CoveError::MapFunctionUndeclared),
        "COVE_E_MAP_IDENTITY_CONFLICT" => Some(CoveError::MapIdentityConflict),
        "COVE_E_MAP_SOURCE_STALE" => Some(CoveError::MapSourceStale),
        "COVE_E_MAP_EVIDENCE_INVALID" => Some(CoveError::MapEvidenceInvalid),
        _ => None,
    }
}

fn validate_file_dictionary_fixture(bytes: &[u8]) -> Result<(), CoveError> {
    if bytes.len() < 4 {
        return Err(CoveError::BufferTooShort);
    }
    let index_len = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
    let split = 4usize
        .checked_add(index_len)
        .ok_or(CoveError::ArithOverflow)?;
    if split > bytes.len() {
        return Err(CoveError::OffsetRange);
    }
    FileDictionary::parse(&bytes[4..split], &bytes[split..]).map(|_| ())
}

fn validate_encoding_fixture(bytes: &[u8]) -> Result<(), CoveError> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|err| CoveError::BadSection(format!("invalid encoding fixture json: {err}")))?;
    let encoding = value
        .get("encoding")
        .and_then(Value::as_str)
        .ok_or_else(|| CoveError::BadSection("encoding fixture missing encoding".into()))?;
    let payload = parse_fixture_byte_vector(value.get("payload"), "payload")?;
    let expected_values = parse_fixture_i64_vector(value.get("expect_values"), "expect_values")?;

    match encoding {
        "constant" => validate_encoding_payload::<Constant, _, _>(
            &payload,
            &expected_values,
            ConstantPayload::parse,
        ),
        "rle" => {
            validate_encoding_payload::<Rle, _, _>(&payload, &expected_values, RlePayload::parse)
        }
        "run_end" => validate_encoding_payload::<RunEnd, _, _>(
            &payload,
            &expected_values,
            RunEndPayload::parse,
        ),
        "plain_fixed" => validate_encoding_payload::<PlainFixed, _, _>(
            &payload,
            &expected_values,
            PlainFixedPayload::parse,
        ),
        "plain_varint" => validate_encoding_payload::<PlainVarint, _, _>(
            &payload,
            &expected_values,
            PlainVarintPayload::parse,
        ),
        "bit_packed" => validate_encoding_payload::<BitPacked, _, _>(
            &payload,
            &expected_values,
            BitPackedPayload::parse,
        ),
        "delta" => validate_encoding_payload::<Delta, _, _>(
            &payload,
            &expected_values,
            DeltaPayload::parse,
        ),
        "frame_of_reference" => validate_encoding_payload::<FrameOfReference, _, _>(
            &payload,
            &expected_values,
            ForPayload::parse,
        ),
        "local_codebook" => validate_encoding_payload::<LocalCodebook, _, _>(
            &payload,
            &expected_values,
            LocalCodebookPayload::parse,
        ),
        "patched_base" => validate_encoding_payload::<PatchedBase, _, _>(
            &payload,
            &expected_values,
            PatchedBasePayload::parse,
        ),
        "sparse" => validate_encoding_payload::<Sparse, _, _>(
            &payload,
            &expected_values,
            SparsePayload::parse,
        ),
        other => Err(CoveError::BadSection(format!(
            "unsupported encoding fixture kind {other}"
        ))),
    }
}

fn validate_nested_fixture(bytes: &[u8]) -> Result<(), CoveError> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|err| CoveError::BadSection(format!("invalid nested fixture json: {err}")))?;
    let layout = value
        .get("layout")
        .and_then(Value::as_str)
        .ok_or_else(|| CoveError::BadSection("nested fixture missing layout".into()))?;

    match layout {
        "list" => {
            let offsets = parse_fixture_u32_vector(value.get("offsets"), "offsets")?;
            let child_row_count = value
                .get("child_row_count")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| {
                    CoveError::BadSection("nested list fixture missing child_row_count".into())
                })?;
            ListLayout { offsets }.validate_child_count(child_row_count)
        }
        "struct" => {
            let field_row_counts =
                parse_fixture_u64_vector(value.get("field_row_counts"), "field_row_counts")?;
            let parent_row_count = value
                .get("parent_row_count")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    CoveError::BadSection("nested struct fixture missing parent_row_count".into())
                })?;
            let parent_null_handling_declared = value
                .get("parent_null_handling_declared")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            StructLayout { field_row_counts }
                .validate_parent_row_count(parent_row_count, parent_null_handling_declared)
        }
        "map" => {
            let offsets = parse_fixture_u32_vector(value.get("offsets"), "offsets")?;
            let key_row_count = value
                .get("key_row_count")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| {
                    CoveError::BadSection("nested map fixture missing key_row_count".into())
                })?;
            let value_row_count = value
                .get("value_row_count")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| {
                    CoveError::BadSection("nested map fixture missing value_row_count".into())
                })?;
            let keys_are_scalar = value
                .get("keys_are_scalar")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let allow_duplicate_keys = value
                .get("allow_duplicate_keys")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let canonical_keys =
                parse_fixture_string_bytes(value.get("canonical_keys"), "canonical_keys")?;
            MapLayout {
                offsets,
                key_row_count,
                value_row_count,
                keys_are_scalar,
                allow_duplicate_keys,
                canonical_keys,
            }
            .validate()
        }
        other => Err(CoveError::BadSection(format!(
            "unsupported nested fixture layout {other}"
        ))),
    }
}

fn validate_encoding_payload<E, P, F>(
    payload: &[u8],
    expected_values: &[i64],
    parse: F,
) -> Result<(), CoveError>
where
    E: Encoding<Payload = P>,
    F: FnOnce(&[u8]) -> Result<P, CoveError>,
{
    let payload = parse(payload)?;
    assert_parity::<E>(&payload)?;
    let actual = E::canonical_decode(&payload)?;
    if actual != expected_values {
        return Err(CoveError::BadSection(format!(
            "encoding fixture mismatch: expected {:?}, got {:?}",
            expected_values, actual
        )));
    }
    Ok(())
}

fn validate_encoded_array_decode_fixture(bytes: &[u8]) -> Result<(), CoveError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|err| {
        CoveError::BadSection(format!("invalid encoded_array fixture json: {err}"))
    })?;
    let fixture = fixture_encoded_array(&value)?;
    let array = fixture.as_array();
    let expected = value
        .get("expect")
        .and_then(Value::as_array)
        .ok_or_else(|| CoveError::BadSection("encoded_array fixture missing expect".into()))?;
    if expected.len() as u64 != array.row_count {
        return Err(CoveError::BadSection(
            "encoded_array fixture expect length must match row_count".into(),
        ));
    }

    let actual = array
        .decode_all_rows()?
        .into_iter()
        .map(|value| array_value_to_json(array.logical, value))
        .collect::<Result<Vec<_>, _>>()?;
    if actual != *expected {
        return Err(CoveError::BadSection(format!(
            "encoded_array fixture mismatch: expected {expected:?}, got {actual:?}"
        )));
    }
    Ok(())
}

fn validate_arrow_export_fixture(bytes: &[u8]) -> Result<(), CoveError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|err| {
        CoveError::BadSection(format!("invalid arrow export fixture json: {err}"))
    })?;
    let fixture = fixture_encoded_array(&value)?;
    let array = fixture.as_array();
    let arrow = encoded_array_to_arrow(&array)?;
    let expected_type = value
        .get("expect_type")
        .and_then(Value::as_str)
        .ok_or_else(|| CoveError::BadSection("arrow export fixture missing expect_type".into()))?;
    let actual_type = format!("{:?}", arrow.data_type());
    if actual_type != expected_type {
        return Err(CoveError::BadSection(format!(
            "arrow export data type mismatch: expected {expected_type}, got {actual_type}"
        )));
    }
    let expected = value
        .get("expect")
        .and_then(Value::as_array)
        .ok_or_else(|| CoveError::BadSection("arrow export fixture missing expect".into()))?;
    let actual = arrow_array_to_json(expected_type, arrow.as_ref())?;
    if actual != *expected {
        return Err(CoveError::BadSection(format!(
            "arrow export fixture mismatch: expected {expected:?}, got {actual:?}"
        )));
    }
    Ok(())
}

struct EncodedArrayFixture {
    logical: CoveLogicalType,
    physical: CovePhysicalKind,
    encoding: CoveEncodingKind,
    row_count: u64,
    payload: Vec<u8>,
}

impl EncodedArrayFixture {
    fn as_array(&self) -> EncodedArray<'_> {
        EncodedArray::new(
            self.logical,
            self.physical,
            self.row_count,
            self.encoding,
            None,
            &self.payload,
            None,
        )
    }
}

fn fixture_encoded_array(value: &Value) -> Result<EncodedArrayFixture, CoveError> {
    let logical = parse_logical_type(
        value
            .get("logical")
            .and_then(Value::as_str)
            .ok_or_else(|| CoveError::BadSection("array fixture missing logical".into()))?,
    )?;
    let physical = parse_physical_kind(
        value
            .get("physical")
            .and_then(Value::as_str)
            .ok_or_else(|| CoveError::BadSection("array fixture missing physical".into()))?,
    )?;
    let encoding = parse_encoding_kind(
        value
            .get("encoding")
            .and_then(Value::as_str)
            .ok_or_else(|| CoveError::BadSection("array fixture missing encoding".into()))?,
    )?;
    let row_count = value
        .get("row_count")
        .and_then(Value::as_u64)
        .ok_or_else(|| CoveError::BadSection("array fixture missing row_count".into()))?;
    let payload = value
        .get("payload")
        .and_then(Value::as_array)
        .ok_or_else(|| CoveError::BadSection("array fixture missing payload".into()))?;
    let payload = payload
        .iter()
        .map(|item| {
            item.as_u64()
                .and_then(|value| u8::try_from(value).ok())
                .ok_or_else(|| CoveError::BadSection("array fixture payload must be bytes".into()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(EncodedArrayFixture {
        logical,
        physical,
        encoding,
        row_count,
        payload,
    })
}

fn array_value_to_json(
    logical: CoveLogicalType,
    value: CoveArrayValue<'_>,
) -> Result<Value, CoveError> {
    match value {
        CoveArrayValue::Null => Ok(Value::Null),
        CoveArrayValue::ValidityBit(value) | CoveArrayValue::Boolean(value) => Ok(json!(value)),
        CoveArrayValue::Int64(value) => Ok(json!(value)),
        CoveArrayValue::NumCode(value) | CoveArrayValue::Varint(value) => Ok(json!(value)),
        CoveArrayValue::FileCode(value) => Ok(json!(value)),
        CoveArrayValue::Bytes(bytes) => bytes_value_to_json(logical, bytes),
        CoveArrayValue::OwnedBytes(bytes) => bytes_value_to_json(logical, &bytes),
        CoveArrayValue::DictValue(_) => Err(CoveError::BadSection(
            "array decode conformance fixtures do not use dictionaries".into(),
        )),
    }
}

fn bytes_value_to_json(logical: CoveLogicalType, bytes: &[u8]) -> Result<Value, CoveError> {
    match logical {
        CoveLogicalType::Utf8 | CoveLogicalType::Json => Ok(json!(std::str::from_utf8(bytes)
            .map_err(|err| CoveError::BadSection(format!("invalid UTF-8 value: {err}")))?)),
        CoveLogicalType::Binary | CoveLogicalType::Uuid => Ok(json!(hex_encode(bytes))),
        _ => Ok(json!(hex_encode(bytes))),
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn arrow_array_to_json(expected_type: &str, array: &dyn Array) -> Result<Vec<Value>, CoveError> {
    match expected_type {
        "Boolean" => {
            let values = downcast_arrow_array::<BooleanArray>(array, expected_type)?;
            Ok((0..values.len())
                .map(|row| {
                    if values.is_null(row) {
                        Value::Null
                    } else {
                        json!(values.value(row))
                    }
                })
                .collect())
        }
        "Int32" => {
            let values = downcast_arrow_array::<Int32Array>(array, expected_type)?;
            Ok((0..values.len())
                .map(|row| {
                    if values.is_null(row) {
                        Value::Null
                    } else {
                        json!(values.value(row))
                    }
                })
                .collect())
        }
        "UInt64" => {
            let values = downcast_arrow_array::<UInt64Array>(array, expected_type)?;
            Ok((0..values.len())
                .map(|row| {
                    if values.is_null(row) {
                        Value::Null
                    } else {
                        json!(values.value(row))
                    }
                })
                .collect())
        }
        "Utf8" => {
            let values = downcast_arrow_array::<StringArray>(array, expected_type)?;
            Ok((0..values.len())
                .map(|row| {
                    if values.is_null(row) {
                        Value::Null
                    } else {
                        json!(values.value(row))
                    }
                })
                .collect())
        }
        "Binary" => {
            let values = downcast_arrow_array::<BinaryArray>(array, expected_type)?;
            Ok((0..values.len())
                .map(|row| {
                    if values.is_null(row) {
                        Value::Null
                    } else {
                        json!(hex_encode(values.value(row)))
                    }
                })
                .collect())
        }
        other => Err(CoveError::BadSection(format!(
            "unsupported arrow export fixture type {other}"
        ))),
    }
}

fn downcast_arrow_array<'a, T: 'static>(
    array: &'a dyn Array,
    expected_type: &str,
) -> Result<&'a T, CoveError> {
    array.as_any().downcast_ref::<T>().ok_or_else(|| {
        CoveError::BadSection(format!(
            "arrow export fixture expected {expected_type} array"
        ))
    })
}

fn parse_logical_type(value: &str) -> Result<CoveLogicalType, CoveError> {
    match value {
        "Bool" => Ok(CoveLogicalType::Bool),
        "Int32" => Ok(CoveLogicalType::Int32),
        "Int64" => Ok(CoveLogicalType::Int64),
        "UInt64" => Ok(CoveLogicalType::UInt64),
        "Utf8" => Ok(CoveLogicalType::Utf8),
        "Binary" => Ok(CoveLogicalType::Binary),
        other => Err(CoveError::BadSection(format!(
            "unsupported array fixture logical type {other}"
        ))),
    }
}

fn parse_physical_kind(value: &str) -> Result<CovePhysicalKind, CoveError> {
    match value {
        "Boolean" => Ok(CovePhysicalKind::Boolean),
        "FixedBytes" => Ok(CovePhysicalKind::FixedBytes),
        "NumCode" => Ok(CovePhysicalKind::NumCode),
        "VarBytes" => Ok(CovePhysicalKind::VarBytes),
        other => Err(CoveError::BadSection(format!(
            "unsupported array fixture physical kind {other}"
        ))),
    }
}

fn parse_encoding_kind(value: &str) -> Result<CoveEncodingKind, CoveError> {
    match value {
        "PlainFixed" => Ok(CoveEncodingKind::PlainFixed),
        "NumCode" => Ok(CoveEncodingKind::NumCode),
        "VarBytes" => Ok(CoveEncodingKind::VarBytes),
        "Rle" => Ok(CoveEncodingKind::Rle),
        "LocalCodebook" => Ok(CoveEncodingKind::LocalCodebook),
        other => Err(CoveError::BadSection(format!(
            "unsupported array fixture encoding kind {other}"
        ))),
    }
}

fn validate_arrow_bitmap_fixture(bytes: &[u8]) -> Result<(), CoveError> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|err| CoveError::BadSection(format!("invalid arrow fixture json: {err}")))?;
    let op = value
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| CoveError::BadSection("arrow fixture missing op".into()))?;
    let row_count = value
        .get("row_count")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| CoveError::BadSection("arrow fixture missing row_count".into()))?;
    let input = parse_fixture_byte_vector(value.get("input"), "input")?;
    let expected = parse_fixture_byte_vector(value.get("expect"), "expect")?;

    let actual = match op {
        "cove_to_arrow" => cove_null_to_arrow_validity(&input, row_count)?,
        "arrow_to_cove" => arrow_validity_to_cove_null(&input, row_count)?,
        other => {
            return Err(CoveError::BadSection(format!(
                "unsupported arrow fixture op {other}"
            )))
        }
    };

    if actual != expected {
        return Err(CoveError::BadSection(format!(
            "arrow fixture mismatch: expected {:?}, got {:?}",
            expected, actual
        )));
    }
    Ok(())
}

fn validate_parquet_conversion_fixture(entry: &Entry, bytes: &[u8]) -> Result<(), CoveError> {
    let mut options = ParquetConversionOptions::default();
    if let Some(table_name) = entry.raw.get("table_name").and_then(Value::as_str) {
        options.table_name = table_name.to_string();
    }
    if let Some(namespace) = entry.raw.get("namespace").and_then(Value::as_str) {
        options.namespace = namespace.to_string();
    }
    if let Some(morsel_row_count) = entry.raw.get("morsel_row_count").and_then(Value::as_u64) {
        options.morsel_row_count = u32::try_from(morsel_row_count)
            .map_err(|_| CoveError::BadSection("invalid morsel_row_count".into()))?;
    }

    let result = convert_parquet_bytes(bytes, &options)?;
    let validation = reader::validate_bytes_with_options(
        &result.cove_bytes,
        ValidationOptions {
            semantic: true,
            verify_digests: false,
            allow_unknown_optional_extensions: true,
        },
    )?;

    if let Some(expected_row_count) = entry.raw.get("expected_row_count").and_then(Value::as_u64) {
        if result.report.row_count != expected_row_count {
            return Err(CoveError::BadSection(format!(
                "parquet conversion row_count mismatch: expected {expected_row_count}, got {}",
                result.report.row_count
            )));
        }
    }

    let table_catalog_payload = section_payload_by_kind(
        &result.cove_bytes,
        &validation.validated,
        SectionKind::TableCatalog,
    )?;
    let table_catalog = TableCatalog::parse(table_catalog_payload.as_ref())?;
    let table = table_catalog
        .tables
        .first()
        .ok_or_else(|| CoveError::BadSection("converted parquet file is missing a table".into()))?;

    if table.name != options.table_name {
        return Err(CoveError::BadSection(format!(
            "parquet conversion table name mismatch: expected {}, got {}",
            options.table_name, table.name
        )));
    }
    if table.namespace != options.namespace {
        return Err(CoveError::BadSection(format!(
            "parquet conversion namespace mismatch: expected {}, got {}",
            options.namespace, table.namespace
        )));
    }

    let segment_payload = section_payload_by_kind(
        &result.cove_bytes,
        &validation.validated,
        SectionKind::TableSegmentData,
    )?;
    let segment = TableSegmentPayloadV1::parse(segment_payload.as_ref())?;

    let expected_columns = entry
        .raw
        .get("expected_columns")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            CoveError::BadSection("parquet conversion fixture missing expected_columns".into())
        })?;
    if expected_columns.len() != table.columns.len() {
        return Err(CoveError::BadSection(format!(
            "parquet conversion column_count mismatch: expected {}, got {}",
            expected_columns.len(),
            table.columns.len()
        )));
    }

    for (expected, column) in expected_columns.iter().zip(table.columns.iter()) {
        if let Some(expected_name) = expected.get("name").and_then(Value::as_str) {
            if column.name != expected_name {
                return Err(CoveError::BadSection(format!(
                    "parquet conversion column name mismatch: expected {expected_name}, got {}",
                    column.name
                )));
            }
        }
        if let Some(expected_logical) = expected.get("logical").and_then(Value::as_str) {
            if format!("{:?}", column.logical) != expected_logical {
                return Err(CoveError::BadSection(format!(
                    "parquet conversion logical mismatch for column {}: expected {expected_logical}, got {:?}",
                    column.name, column.logical
                )));
            }
        }
        if let Some(expected_physical) = expected.get("physical").and_then(Value::as_str) {
            if format!("{:?}", column.physical) != expected_physical {
                return Err(CoveError::BadSection(format!(
                    "parquet conversion physical mismatch for column {}: expected {expected_physical}, got {:?}",
                    column.name, column.physical
                )));
            }
        }
        let expected_values = expected
            .get("values")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                CoveError::BadSection(format!(
                    "parquet conversion fixture missing values for column {}",
                    column.name
                ))
            })?;
        let actual_values =
            decode_segment_column_values(segment_payload.as_ref(), &segment, column)?;
        let actual_json = actual_values
            .into_iter()
            .map(|value| value.to_json_value())
            .collect::<Vec<_>>();
        if actual_json != *expected_values {
            return Err(CoveError::BadSection(format!(
                "parquet conversion values mismatch for column {}: expected {:?}, got {:?}",
                column.name, expected_values, actual_json
            )));
        }
    }

    Ok(())
}

fn section_payload_by_kind<'a>(
    data: &'a [u8],
    validated: &'a reader::ValidatedCoveFile,
    kind: SectionKind,
) -> Result<Cow<'a, [u8]>, CoveError> {
    let entry = validated
        .footer
        .sections
        .iter()
        .find(|entry| entry.section_kind == kind as u16)
        .ok_or_else(|| CoveError::BadSection(format!("missing section {kind:?}")))?;
    section_payload(data, entry)
}

fn decode_segment_column_values(
    segment_bytes: &[u8],
    segment: &TableSegmentPayloadV1,
    column: &cove_core::table::ColumnEntry,
) -> Result<Vec<cove_core::interop::parquet::ParquetScalarValue>, CoveError> {
    let column_directory = segment
        .columns
        .iter()
        .find(|entry| entry.column_id == column.column_id)
        .ok_or_else(|| {
            CoveError::BadSection(format!(
                "missing segment column directory for column {}",
                column.column_id
            ))
        })?;
    let page_index = ColumnPageIndex::parse(
        &segment_bytes[column_directory.page_index_offset as usize
            ..(column_directory.page_index_offset + column_directory.page_index_length) as usize],
    )?;
    let mut out = Vec::new();
    for page in &page_index.entries {
        let page_wire = &segment_bytes
            [page.page_offset as usize..(page.page_offset + page.page_length) as usize];
        let payload = column_page_payload(page_wire, page)?;
        out.extend(decode_materialized_page_values(
            column,
            page.row_count,
            payload.as_ref(),
        )?);
    }
    Ok(out)
}

fn parse_fixture_byte_vector(value: Option<&Value>, field: &str) -> Result<Vec<u8>, CoveError> {
    let items = value
        .and_then(Value::as_array)
        .ok_or_else(|| CoveError::BadSection(format!("fixture missing {field} byte array")))?;
    items
        .iter()
        .map(|item| {
            item.as_u64()
                .and_then(|value| u8::try_from(value).ok())
                .ok_or_else(|| {
                    CoveError::BadSection(format!(
                        "fixture field {field} must contain only byte values"
                    ))
                })
        })
        .collect()
}

fn parse_fixture_i64_vector(value: Option<&Value>, field: &str) -> Result<Vec<i64>, CoveError> {
    let items = value
        .and_then(Value::as_array)
        .ok_or_else(|| CoveError::BadSection(format!("fixture missing {field} i64 array")))?;
    items
        .iter()
        .map(|item| {
            item.as_i64().ok_or_else(|| {
                CoveError::BadSection(format!(
                    "fixture field {field} must contain only i64 values"
                ))
            })
        })
        .collect()
}

fn parse_fixture_u32_vector(value: Option<&Value>, field: &str) -> Result<Vec<u32>, CoveError> {
    let items = value
        .and_then(Value::as_array)
        .ok_or_else(|| CoveError::BadSection(format!("fixture missing {field} u32 array")))?;
    items
        .iter()
        .map(|item| {
            item.as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| {
                    CoveError::BadSection(format!(
                        "fixture field {field} must contain only u32 values"
                    ))
                })
        })
        .collect()
}

fn parse_fixture_u64_vector(value: Option<&Value>, field: &str) -> Result<Vec<u64>, CoveError> {
    let items = value
        .and_then(Value::as_array)
        .ok_or_else(|| CoveError::BadSection(format!("fixture missing {field} u64 array")))?;
    items
        .iter()
        .map(|item| {
            item.as_u64().ok_or_else(|| {
                CoveError::BadSection(format!(
                    "fixture field {field} must contain only u64 values"
                ))
            })
        })
        .collect()
}

fn parse_fixture_string_bytes(
    value: Option<&Value>,
    field: &str,
) -> Result<Vec<Vec<u8>>, CoveError> {
    let items = value
        .and_then(Value::as_array)
        .ok_or_else(|| CoveError::BadSection(format!("fixture missing {field} string array")))?;
    items
        .iter()
        .map(|item| {
            item.as_str()
                .map(|item| item.as_bytes().to_vec())
                .ok_or_else(|| {
                    CoveError::BadSection(format!(
                        "fixture field {field} must contain only string values"
                    ))
                })
        })
        .collect()
}

#[derive(Debug)]
struct PruningColumnFixture {
    column_id: u32,
    zone_stats: Option<ZoneStatsEntry>,
    domain: Option<ColumnDomain>,
    exact_set: Option<ExactSetIndex>,
    bloom: Option<BloomFilterIndex>,
    bloom_fail_open: bool,
    inverted: Option<InvertedMorselIndex>,
    inverted_fail_open: bool,
    lookup: Option<LookupIndex>,
    lookup_fail_open: bool,
    composite: Option<CompositeIndex>,
    composite_fail_open: bool,
    composite_matches_bindings: bool,
    aggregate: Option<AggregateSynopsis>,
    aggregate_fail_open: bool,
    aggregate_proves_no_match: bool,
}

/// Spec §10 — wire-format primitives (varint LEB128, ZigZag, strict bool).
///
/// Fixture shape:
/// ```json
/// { "op": "varint_round_trip",   "value": <u64>,  "expect_bytes": [u8...] }
/// { "op": "varint_decode_reject", "input": [u8...], "reason": "..." }
/// { "op": "zigzag_round_trip",   "value": <i64>,  "expect_zigzag": <u64> }
/// { "op": "bool_strict",         "byte": <u8>, "expect": <bool> }
/// { "op": "bool_strict_reject",  "byte": <u8> }
/// ```
fn validate_wire_primitive_fixture(bytes: &[u8]) -> Result<(), CoveError> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|err| CoveError::BadSection(format!("invalid wire fixture json: {err}")))?;
    let op = value
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| CoveError::BadSection("wire fixture missing op".into()))?;
    match op {
        "varint_round_trip" => {
            let n = value
                .get("value")
                .and_then(Value::as_u64)
                .ok_or_else(|| CoveError::BadSection("varint fixture missing value".into()))?;
            let expected = parse_fixture_byte_vector(value.get("expect_bytes"), "expect_bytes")?;
            let actual = encode_u64_leb128(n);
            if actual != expected {
                return Err(CoveError::BadSection(format!(
                    "varint encode mismatch for {n}: expected {:?}, got {:?}",
                    expected, actual
                )));
            }
            let (decoded, used) = decode_u64_leb128(&actual)?;
            if decoded != n || used != actual.len() {
                return Err(CoveError::BadSection(format!(
                    "varint round-trip mismatch for {n}: decoded={decoded}, used={used}, len={}",
                    actual.len()
                )));
            }
            Ok(())
        }
        "varint_decode_reject" => {
            let input = parse_fixture_byte_vector(value.get("input"), "input")?;
            if decode_u64_leb128(&input).is_ok() {
                return Err(CoveError::BadSection(
                    "varint_decode_reject input was accepted".into(),
                ));
            }
            Ok(())
        }
        "zigzag_round_trip" => {
            let n = value
                .get("value")
                .and_then(Value::as_i64)
                .ok_or_else(|| CoveError::BadSection("zigzag fixture missing value".into()))?;
            let expected = value
                .get("expect_zigzag")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    CoveError::BadSection("zigzag fixture missing expect_zigzag".into())
                })?;
            let encoded = zigzag_encode_i64(n);
            if encoded != expected {
                return Err(CoveError::BadSection(format!(
                    "zigzag encode mismatch for {n}: expected {expected}, got {encoded}"
                )));
            }
            if zigzag_decode_i64(encoded) != n {
                return Err(CoveError::BadSection(format!(
                    "zigzag decode mismatch for {n}: got {}",
                    zigzag_decode_i64(encoded)
                )));
            }
            Ok(())
        }
        "bool_strict" => {
            let byte =
                value.get("byte").and_then(Value::as_u64).ok_or_else(|| {
                    CoveError::BadSection("bool_strict fixture missing byte".into())
                })? as u8;
            let expected = value
                .get("expect")
                .and_then(Value::as_bool)
                .ok_or_else(|| {
                    CoveError::BadSection("bool_strict fixture missing expect".into())
                })?;
            let actual = parse_bool_strict(byte)?;
            if actual != expected {
                return Err(CoveError::BadSection(format!(
                    "bool_strict mismatch: expected {expected}, got {actual}"
                )));
            }
            Ok(())
        }
        "bool_strict_reject" => {
            let byte = value.get("byte").and_then(Value::as_u64).ok_or_else(|| {
                CoveError::BadSection("bool_strict_reject fixture missing byte".into())
            })? as u8;
            if parse_bool_strict(byte).is_ok() {
                return Err(CoveError::BadSection(format!(
                    "bool_strict_reject byte {byte} was accepted"
                )));
            }
            Ok(())
        }
        other => Err(CoveError::BadSection(format!(
            "wire_primitive_case unknown op {other:?}"
        ))),
    }
}

/// Spec §66 / §27 — exercise page-level compression and validation.
///
/// Fixture shape:
/// ```json
/// {
///   "codec": "none" | "lz4" | "zstd",
///   "payload": "<utf-8 string used as the uncompressed page bytes>",
///   "expect": "round_trip" | "parse_reject" | "decode_reject",
///   // optional overrides applied before serializing the entry:
///   "page_length_override":         <u64?>,
///   "uncompressed_length_override": <u64?>,
///   "flags_override":               <u32?>,
///   "row_count_override":           <u32?>,
///   "non_null_count_override":      <u32?>,
///   "null_count_override":          <u32?>,
///   "encoding_root_override":       <u32?>,
///   "page_offset_override":         <u64?>,
///   // optional wire-byte mutation applied before column_page_payload:
///   "truncate_wire_bytes":          <usize?>
/// }
/// ```
///
/// `round_trip`     — encode payload, parse the entry, decode wire bytes,
///                    assert decoded == payload.
/// `parse_reject`   — apply overrides, expect `ColumnPageIndexEntryV1::parse`
///                    to reject (Spec §27.2 invariants + §66 codec rules).
/// `decode_reject`  — entry parses cleanly but `column_page_payload` rejects
///                    the wire bytes (Spec §66 robustness against truncation
///                    or length mismatch).
fn validate_page_codec_fixture(bytes: &[u8]) -> Result<(), CoveError> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|err| CoveError::BadSection(format!("invalid page_codec fixture json: {err}")))?;
    let codec = match value.get("codec").and_then(Value::as_str) {
        Some("none") => CompressionCodec::None,
        Some("lz4") => CompressionCodec::Lz4,
        Some("zstd") => CompressionCodec::Zstd,
        other => {
            return Err(CoveError::BadSection(format!(
                "page_codec fixture has unknown codec {other:?}"
            )))
        }
    };
    let payload = value
        .get("payload")
        .and_then(Value::as_str)
        .ok_or_else(|| CoveError::BadSection("page_codec fixture missing payload".into()))?
        .as_bytes()
        .to_vec();
    let expect = value
        .get("expect")
        .and_then(Value::as_str)
        .ok_or_else(|| CoveError::BadSection("page_codec fixture missing expect".into()))?;

    let wire = encode_page_payload(&payload, codec)?;
    let page_length = value
        .get("page_length_override")
        .and_then(Value::as_u64)
        .unwrap_or(wire.len() as u64);
    let uncompressed_length = value
        .get("uncompressed_length_override")
        .and_then(Value::as_u64)
        .unwrap_or(payload.len() as u64);
    let flags = value
        .get("flags_override")
        .and_then(Value::as_u64)
        .map(|raw| raw as u32)
        .unwrap_or(codec as u32);
    let row_count = value
        .get("row_count_override")
        .and_then(Value::as_u64)
        .unwrap_or(1) as u32;
    let non_null_count = value
        .get("non_null_count_override")
        .and_then(Value::as_u64)
        .unwrap_or(row_count as u64) as u32;
    let null_count = value
        .get("null_count_override")
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;
    let encoding_root = value
        .get("encoding_root_override")
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;
    let page_offset = value
        .get("page_offset_override")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    let entry = ColumnPageIndexEntryV1 {
        column_id: 1,
        morsel_id: 0,
        row_count,
        non_null_count,
        null_count,
        encoding_root,
        page_offset,
        page_length,
        uncompressed_length,
        stats_ref: 0,
        flags,
        checksum: checksum::crc32c(&wire),
    };
    let serialized = entry.serialize();
    let parsed = ColumnPageIndexEntryV1::parse(&serialized);

    match expect {
        "parse_reject" => {
            if parsed.is_ok() {
                return Err(CoveError::BadSection(
                    "page_codec parse_reject fixture parsed successfully".into(),
                ));
            }
            Ok(())
        }
        "round_trip" => {
            let parsed = parsed?;
            let decoded = column_page_payload(&wire, &parsed)?;
            if &*decoded != payload.as_slice() {
                return Err(CoveError::BadSection(
                    "page_codec round_trip decoded payload mismatch".into(),
                ));
            }
            Ok(())
        }
        "decode_reject" => {
            let parsed = parsed?;
            let mut wire = wire.clone();
            if let Some(truncate_to) = value.get("truncate_wire_bytes").and_then(Value::as_u64) {
                wire.truncate(truncate_to as usize);
            }
            // Re-stamp page_length to match the (possibly truncated) wire so
            // that the §66 codec dispatch is what surfaces the rejection,
            // not the surface-length check.
            let mut entry = parsed;
            entry.page_length = wire.len() as u64;
            if column_page_payload(&wire, &entry).is_ok() {
                return Err(CoveError::BadSection(
                    "page_codec decode_reject fixture decoded successfully".into(),
                ));
            }
            Ok(())
        }
        other => Err(CoveError::BadSection(format!(
            "page_codec fixture unknown expect kind {other:?}"
        ))),
    }
}

fn validate_pruning_fixture(bytes: &[u8]) -> Result<(), CoveError> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|err| CoveError::BadSection(format!("invalid pruning fixture json: {err}")))?;
    let columns = parse_pruning_columns(value.get("columns"))?;
    let predicate = value
        .get("predicate")
        .ok_or_else(|| CoveError::BadSection("pruning fixture missing predicate".into()))?;
    let explanation = evaluate_pruning_predicate(predicate, &columns)?;

    let expected_outcome =
        parse_expected_outcome(value.get("expect_outcome").ok_or_else(|| {
            CoveError::BadSection("pruning fixture missing expect_outcome".into())
        })?)?;
    if explanation.final_outcome != expected_outcome {
        return Err(CoveError::BadSection(format!(
            "pruning outcome mismatch: expected {:?}, got {:?}",
            expected_outcome, explanation.final_outcome
        )));
    }

    if let Some(expected) = value.get("expect_evidence") {
        let expected = expected
            .as_array()
            .ok_or_else(|| {
                CoveError::BadSection("expect_evidence must be an array of strings".into())
            })?
            .iter()
            .map(|item| {
                item.as_str().ok_or_else(|| {
                    CoveError::BadSection("expect_evidence entries must be strings".into())
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let actual = explanation
            .steps
            .iter()
            .map(|step| pruning_evidence_name(step.evidence))
            .collect::<Vec<_>>();
        if actual != expected {
            return Err(CoveError::BadSection(format!(
                "pruning evidence mismatch: expected {:?}, got {:?}",
                expected, actual
            )));
        }
    }

    Ok(())
}

fn parse_pruning_columns(value: Option<&Value>) -> Result<Vec<PruningColumnFixture>, CoveError> {
    let Some(columns) = value else {
        return Ok(Vec::new());
    };
    let columns = columns
        .as_array()
        .ok_or_else(|| CoveError::BadSection("pruning fixture columns must be an array".into()))?;
    columns
        .iter()
        .map(parse_pruning_column)
        .collect::<Result<Vec<_>, _>>()
}

fn parse_pruning_column(value: &Value) -> Result<PruningColumnFixture, CoveError> {
    let column_id = value
        .get("column_id")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| CoveError::BadSection("pruning column missing column_id".into()))?;

    Ok(PruningColumnFixture {
        column_id,
        zone_stats: value
            .get("zone_stats")
            .map(|zone_stats| parse_pruning_zone_stats(zone_stats, column_id))
            .transpose()?,
        domain: value
            .get("column_domain")
            .map(|domain| parse_pruning_domain(domain, column_id))
            .transpose()?,
        exact_set: value
            .get("exact_set")
            .map(|exact_set| parse_pruning_exact_set(exact_set, column_id))
            .transpose()?,
        bloom: value
            .get("bloom")
            .map(|bloom| parse_pruning_bloom(bloom, column_id))
            .transpose()?,
        bloom_fail_open: value
            .get("bloom")
            .and_then(|bloom| bloom.get("fail_open"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        inverted: value
            .get("inverted")
            .map(|inverted| parse_pruning_inverted(inverted, column_id))
            .transpose()?,
        inverted_fail_open: value
            .get("inverted")
            .and_then(|inverted| inverted.get("fail_open"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        lookup: value
            .get("lookup")
            .map(|lookup| parse_pruning_lookup(lookup, column_id))
            .transpose()?,
        lookup_fail_open: value
            .get("lookup")
            .and_then(|lookup| lookup.get("fail_open"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        composite: value
            .get("composite")
            .map(|_| composite_index_stub(column_id)),
        composite_fail_open: value
            .get("composite")
            .and_then(|composite| composite.get("fail_open"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        composite_matches_bindings: value
            .get("composite")
            .and_then(|composite| composite.get("matches_bindings"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        aggregate: value.get("aggregate").map(|_| AggregateSynopsis::default()),
        aggregate_fail_open: value
            .get("aggregate")
            .and_then(|aggregate| aggregate.get("fail_open"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        aggregate_proves_no_match: value
            .get("aggregate")
            .and_then(|aggregate| aggregate.get("proves_no_match"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn parse_pruning_zone_stats(value: &Value, column_id: u32) -> Result<ZoneStatsEntry, CoveError> {
    let row_count = value
        .get("row_count")
        .and_then(Value::as_u64)
        .ok_or_else(|| CoveError::BadSection("zone_stats missing row_count".into()))?;
    let null_count = value
        .get("null_count")
        .and_then(Value::as_u64)
        .ok_or_else(|| CoveError::BadSection("zone_stats missing null_count".into()))?;
    let min_domain_rank = value
        .get("min_domain_rank")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(0);
    let max_domain_rank = value
        .get("max_domain_rank")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(0);
    let flags = parse_zone_stat_flags(value.get("flags"))?;
    let mut min = value
        .get("min")
        .map(|scalar| parse_pruning_stat_scalar(scalar, "zone_stats min"))
        .transpose()?;
    let mut max = value
        .get("max")
        .map(|scalar| parse_pruning_stat_scalar(scalar, "zone_stats max"))
        .transpose()?;
    if flags.contains(ZoneStatFlags::MINMAX_TRUNCATED) {
        if let Some(min) = min.as_mut() {
            min.truncated = true;
        }
        if let Some(max) = max.as_mut() {
            max.truncated = true;
        }
    }

    let entry = ZoneStatsEntry {
        table_id: 1,
        segment_id: 0,
        morsel_id: u32::MAX,
        column_id,
        non_null_count: u32::try_from(row_count.checked_sub(null_count).ok_or_else(|| {
            CoveError::BadSection("zone_stats null_count exceeds row_count".into())
        })?)
        .map_err(|_| CoveError::BadSection("zone_stats non_null_count overflows u32".into()))?,
        distinct_count: 0,
        run_count: 0,
        stats: ZoneStats {
            scope: ZoneScope::Segment,
            row_count,
            null_count,
            min,
            max,
            flags,
        },
        min_domain_rank,
        max_domain_rank,
        exact_set_ref: 0,
        bloom_ref: 0,
    };
    entry.validate()?;
    Ok(entry)
}

fn parse_zone_stat_flags(value: Option<&Value>) -> Result<ZoneStatFlags, CoveError> {
    let mut flags = ZoneStatFlags::empty();
    let Some(value) = value else {
        return Ok(flags);
    };
    let items = value.as_array().ok_or_else(|| {
        CoveError::BadSection("zone_stats flags must be an array of strings".into())
    })?;
    for item in items {
        match item.as_str().ok_or_else(|| {
            CoveError::BadSection("zone_stats flags entries must be strings".into())
        })? {
            "has_min_max" => flags = flags | ZoneStatFlags::HAS_MIN_MAX,
            "has_domain_range" => flags = flags | ZoneStatFlags::HAS_DOMAIN_RANGE,
            "constant" => flags = flags | ZoneStatFlags::CONSTANT,
            "has_nan" => flags = flags | ZoneStatFlags::HAS_NAN,
            "minmax_truncated" => flags = flags | ZoneStatFlags::MINMAX_TRUNCATED,
            other => {
                return Err(CoveError::BadSection(format!(
                    "unsupported pruning zone_stats flag {other}"
                )))
            }
        }
    }
    Ok(flags)
}

fn parse_pruning_domain(value: &Value, column_id: u32) -> Result<ColumnDomain, CoveError> {
    let sorted_file_codes = value
        .get("sorted_file_codes")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            CoveError::BadSection("column_domain missing sorted_file_codes array".into())
        })?
        .iter()
        .map(|item| {
            item.as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| {
                    CoveError::BadSection(
                        "column_domain sorted_file_codes entries must be u32 values".into(),
                    )
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let dictionary_entry_count = value
        .get("dictionary_entry_count")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or_else(|| {
            sorted_file_codes
                .iter()
                .copied()
                .max()
                .unwrap_or(0)
                .saturating_add(1)
        });
    let safe = value.get("safe").and_then(Value::as_bool).unwrap_or(true);

    let mut domain = ColumnDomain::from_sorted_present_codes(
        &sorted_file_codes,
        dictionary_entry_count,
        1,
        column_id,
        0,
        0,
        0,
    )?;
    if !safe && !domain.sorted_file_codes.is_empty() {
        let first_code = domain.sorted_file_codes[0] as usize;
        let replacement = domain.sorted_file_codes.len() as u32 - 1;
        domain.file_code_to_rank[first_code] = replacement;
    }
    Ok(domain)
}

fn parse_pruning_exact_set(value: &Value, column_id: u32) -> Result<ExactSetIndex, CoveError> {
    let keys = value
        .get("keys")
        .and_then(Value::as_array)
        .ok_or_else(|| CoveError::BadSection("exact_set missing keys array".into()))?
        .iter()
        .map(|item| {
            item.as_u64().ok_or_else(|| {
                CoveError::BadSection("exact_set keys entries must be u64 values".into())
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ExactSetIndex {
        header: ExactSetIndexHeaderV1 {
            table_id: 1,
            column_id,
            granularity: ExactSetGranularity::Segment,
            key_kind: ExactSetKeyKind::FileCode,
            representation: ExactSetRepresentation::SortedList,
            flags: 0,
            entry_count: keys.len() as u32,
            data_offset: 0,
            data_length: 0,
            checksum: 0,
        },
        keys,
        data: Vec::new(),
    })
}

fn parse_pruning_bloom(value: &Value, column_id: u32) -> Result<BloomFilterIndex, CoveError> {
    use cove_core::index::bloom::{
        BloomAlgorithm, BloomGranularity, BloomHashDomain, BloomIndexHeaderV1,
        BLOOM_INDEX_HEADER_LEN,
    };
    let bit_count = value
        .get("bit_count")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(64);
    let mut bloom = BloomFilterIndex {
        header: BloomIndexHeaderV1 {
            table_id: 1,
            column_id,
            granularity: BloomGranularity::Segment,
            hash_domain: BloomHashDomain::CanonicalValueHash,
            algorithm: BloomAlgorithm::SplitBlock,
            flags: 0,
            target_fpr_ppm: 10_000,
            filter_count: 1,
            data_offset: BLOOM_INDEX_HEADER_LEN as u64,
            data_length: bit_count as u64,
            checksum: 0,
        },
        hash_count: 4,
        bits: vec![0u8; bit_count],
    };
    if let Some(values) = value.get("values").and_then(Value::as_array) {
        for entry in values {
            let bytes = parse_pruning_byte_string(entry, "bloom values entry")?;
            bloom.insert(&bytes);
        }
    }
    Ok(bloom)
}

fn parse_pruning_inverted(value: &Value, column_id: u32) -> Result<InvertedMorselIndex, CoveError> {
    use cove_core::index::inverted::{
        InvertedEntry, InvertedKeyKind, InvertedMorselIndexHeaderV1,
        INVERTED_MORSEL_INDEX_HEADER_LEN,
    };
    let mut keys: Vec<u64> = value
        .get("keys")
        .and_then(Value::as_array)
        .ok_or_else(|| CoveError::BadSection("inverted missing keys array".into()))?
        .iter()
        .map(|item| {
            item.as_u64().ok_or_else(|| {
                CoveError::BadSection("inverted keys entries must be u64 values".into())
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    keys.sort_unstable();
    keys.dedup();
    Ok(InvertedMorselIndex {
        header: InvertedMorselIndexHeaderV1 {
            table_id: 1,
            column_id,
            key_kind: InvertedKeyKind::FileCode,
            flags: 0,
            representation: 0,
            reserved: 0,
            entry_count: keys.len() as u32,
            entries_offset: INVERTED_MORSEL_INDEX_HEADER_LEN as u64,
            bitmap_data_offset: INVERTED_MORSEL_INDEX_HEADER_LEN as u64,
            checksum: 0,
        },
        entries: keys
            .into_iter()
            .map(|key| InvertedEntry {
                key,
                morsel_bitmap_offset: 0,
                morsel_bitmap_length: 0,
                row_bitmap_offset: 0,
                row_bitmap_length: 0,
            })
            .collect(),
        bitmap_data: Vec::new(),
    })
}

fn parse_pruning_lookup(value: &Value, column_id: u32) -> Result<LookupIndex, CoveError> {
    use cove_core::index::lookup::{
        LookupEntry, LookupIndexHeaderV1, LookupIndexKind, LookupKeyKind, LookupUniqueness,
    };
    let mut keys: Vec<u64> = value
        .get("keys")
        .and_then(Value::as_array)
        .ok_or_else(|| CoveError::BadSection("lookup missing keys array".into()))?
        .iter()
        .map(|item| {
            item.as_u64().ok_or_else(|| {
                CoveError::BadSection("lookup keys entries must be u64 values".into())
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    keys.sort_unstable();
    keys.dedup();
    Ok(LookupIndex {
        header: LookupIndexHeaderV1 {
            table_id: 1,
            column_id,
            key_kind: LookupKeyKind::FileCode,
            index_kind: LookupIndexKind::SparseSorted,
            uniqueness: LookupUniqueness::Unique,
            flags: 0,
            entry_count: keys.len() as u64,
            entries_offset: 0,
            entries_length: 0,
            rowref_offset: 0,
            rowref_length: 0,
            checksum: 0,
        },
        entries: keys
            .into_iter()
            .map(|key| LookupEntry {
                key,
                rows: vec![RowRef {
                    table_id: 1,
                    segment_id: 0,
                    morsel_id: 0,
                    row_in_morsel: 0,
                }],
            })
            .collect(),
    })
}

fn composite_index_stub(column_id: u32) -> CompositeIndex {
    use cove_core::index::composite::{
        CompositeTransformKind, CompositeZoneIndexHeaderV1, COMPOSITE_ZONE_INDEX_HEADER_LEN,
    };
    CompositeIndex {
        header: CompositeZoneIndexHeaderV1 {
            table_id: 1,
            key_column_count: 1,
            transform_kind: CompositeTransformKind::Tuple,
            flags: 0,
            zone_count: 1,
            key_columns_offset: COMPOSITE_ZONE_INDEX_HEADER_LEN as u64,
            entries_offset: (COMPOSITE_ZONE_INDEX_HEADER_LEN + 4) as u64,
            entries_length: 0,
            checksum: 0,
        },
        key_columns: vec![column_id],
        entries: Vec::new(),
    }
}

fn parse_pruning_byte_string(value: &Value, field: &str) -> Result<Vec<u8>, CoveError> {
    if let Some(text) = value.as_str() {
        return Ok(text.as_bytes().to_vec());
    }
    if let Some(items) = value.as_array() {
        return items
            .iter()
            .map(|item| {
                item.as_u64()
                    .and_then(|value| u8::try_from(value).ok())
                    .ok_or_else(|| {
                        CoveError::BadSection(format!("{field} byte array entries must be u8"))
                    })
            })
            .collect();
    }
    Err(CoveError::BadSection(format!(
        "{field} must be a string or u8 array"
    )))
}

fn parse_pruning_stat_scalar(value: &Value, field: &str) -> Result<StatScalar, CoveError> {
    let kind_name = value
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| CoveError::BadSection(format!("{field} missing kind")))?;
    let kind = parse_pruning_stat_kind(kind_name)?;
    let raw_value = value
        .get("value")
        .ok_or_else(|| CoveError::BadSection(format!("{field} missing value")))?;
    let bytes = match kind {
        StatKind::Int64 => parse_json_i64(raw_value, field)?.to_le_bytes().to_vec(),
        StatKind::UInt64 => parse_json_u64(raw_value, field)?.to_le_bytes().to_vec(),
        StatKind::Float64Bits => parse_json_f64(raw_value, field)?
            .to_bits()
            .to_le_bytes()
            .to_vec(),
        StatKind::Decimal128 => parse_json_i128(raw_value, field)?.to_le_bytes().to_vec(),
        StatKind::TimestampMicros => parse_json_i64(raw_value, field)?.to_le_bytes().to_vec(),
        StatKind::TimestampNanos => parse_json_i64(raw_value, field)?.to_le_bytes().to_vec(),
        StatKind::DateDays => parse_json_i32(raw_value, field)?.to_le_bytes().to_vec(),
        StatKind::None | StatKind::FixedBytes => {
            return Err(CoveError::BadSection(format!(
                "{field} uses unsupported pruning stat kind {kind_name}"
            )))
        }
    };

    Ok(StatScalar {
        kind,
        bytes,
        truncated: value
            .get("truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn parse_pruning_numeric_bound(value: &Value, field: &str) -> Result<NumericStatValue, CoveError> {
    parse_pruning_stat_scalar(value, field)?
        .numeric_value()
        .ok_or_else(|| {
            CoveError::BadSection(format!("{field} must decode to a numeric stat value"))
        })
}

fn parse_pruning_stat_kind(kind: &str) -> Result<StatKind, CoveError> {
    match kind {
        "int64" => Ok(StatKind::Int64),
        "uint64" => Ok(StatKind::UInt64),
        "float64" | "float64_bits" => Ok(StatKind::Float64Bits),
        "decimal128" => Ok(StatKind::Decimal128),
        "timestamp_micros" => Ok(StatKind::TimestampMicros),
        "timestamp_nanos" => Ok(StatKind::TimestampNanos),
        "date_days" => Ok(StatKind::DateDays),
        other => Err(CoveError::BadSection(format!(
            "unsupported pruning stat kind {other}"
        ))),
    }
}

fn parse_json_i32(value: &Value, field: &str) -> Result<i32, CoveError> {
    let parsed = parse_json_i64(value, field)?;
    i32::try_from(parsed).map_err(|_| CoveError::BadSection(format!("{field} must fit in i32")))
}

fn parse_json_i64(value: &Value, field: &str) -> Result<i64, CoveError> {
    if let Some(value) = value.as_i64() {
        return Ok(value);
    }
    if let Some(value) = value.as_u64() {
        return i64::try_from(value)
            .map_err(|_| CoveError::BadSection(format!("{field} must fit in i64")));
    }
    if let Some(value) = value.as_str() {
        return value.parse::<i64>().map_err(|_| {
            CoveError::BadSection(format!("{field} must be an i64-compatible value"))
        });
    }
    Err(CoveError::BadSection(format!(
        "{field} must be an integer value"
    )))
}

fn parse_json_u64(value: &Value, field: &str) -> Result<u64, CoveError> {
    if let Some(value) = value.as_u64() {
        return Ok(value);
    }
    if let Some(value) = value.as_str() {
        return value
            .parse::<u64>()
            .map_err(|_| CoveError::BadSection(format!("{field} must be a u64-compatible value")));
    }
    Err(CoveError::BadSection(format!(
        "{field} must be an unsigned integer value"
    )))
}

fn parse_json_i128(value: &Value, field: &str) -> Result<i128, CoveError> {
    if let Some(value) = value.as_i64() {
        return Ok(value as i128);
    }
    if let Some(value) = value.as_u64() {
        return Ok(value as i128);
    }
    if let Some(value) = value.as_str() {
        return value.parse::<i128>().map_err(|_| {
            CoveError::BadSection(format!("{field} must be an i128-compatible value"))
        });
    }
    Err(CoveError::BadSection(format!(
        "{field} must be an integer value"
    )))
}

fn parse_json_f64(value: &Value, field: &str) -> Result<f64, CoveError> {
    if let Some(value) = value.as_f64() {
        return Ok(value);
    }
    if let Some(value) = value.as_str() {
        return value.parse::<f64>().map_err(|_| {
            CoveError::BadSection(format!("{field} must be an f64-compatible value"))
        });
    }
    Err(CoveError::BadSection(format!(
        "{field} must be a numeric value"
    )))
}

fn evaluate_pruning_predicate(
    predicate: &Value,
    columns: &[PruningColumnFixture],
) -> Result<PruningExplanation, CoveError> {
    let op = predicate
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| CoveError::BadSection("pruning predicate missing op".into()))?;
    match op {
        "is_null" => {
            let column = pruning_column(columns, predicate_column_id(predicate)?);
            Ok(explain_is_null(
                column.and_then(|column| column.zone_stats.as_ref()),
            ))
        }
        "is_not_null" => {
            let column = pruning_column(columns, predicate_column_id(predicate)?);
            Ok(explain_is_not_null(
                column.and_then(|column| column.zone_stats.as_ref()),
            ))
        }
        "file_code_eq" => {
            let column = pruning_column(columns, predicate_column_id(predicate)?);
            let file_code = predicate
                .get("file_code")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| CoveError::BadSection("file_code_eq missing file_code".into()))?;
            Ok(explain_file_code_equality(
                file_code,
                column.and_then(|column| column.zone_stats.as_ref()),
                column.and_then(|column| column.domain.as_ref()),
                column.and_then(|column| column.exact_set.as_ref()),
            ))
        }
        "domain_rank_range" => {
            let column = pruning_column(columns, predicate_column_id(predicate)?);
            let min_rank = predicate
                .get("min_rank")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| {
                    CoveError::BadSection("domain_rank_range missing min_rank".into())
                })?;
            let max_rank = predicate
                .get("max_rank")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| {
                    CoveError::BadSection("domain_rank_range missing max_rank".into())
                })?;
            Ok(explain_resolved_domain_rank_range(
                min_rank,
                max_rank,
                column.and_then(|column| column.zone_stats.as_ref()),
                column.and_then(|column| column.domain.as_ref()),
            ))
        }
        "numcode_range" => {
            let column = pruning_column(columns, predicate_column_id(predicate)?);
            let lower_bound = predicate
                .get("lower")
                .map(|value| parse_pruning_numeric_bound(value, "numcode_range lower"))
                .transpose()?;
            let upper_bound = predicate
                .get("upper")
                .map(|value| parse_pruning_numeric_bound(value, "numcode_range upper"))
                .transpose()?;
            if lower_bound.is_none() && upper_bound.is_none() {
                return Err(CoveError::BadSection(
                    "numcode_range must declare at least one bound".into(),
                ));
            }
            Ok(explain_numcode_range(
                lower_bound,
                predicate
                    .get("lower_inclusive")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                upper_bound,
                predicate
                    .get("upper_inclusive")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                column.and_then(|column| column.zone_stats.as_ref()),
            ))
        }
        "and" => fold_pruning_operands(predicate, columns, |left, right| left.and(right)),
        "or" => fold_pruning_operands(predicate, columns, |left, right| left.or(right)),
        "not" => {
            let operand = predicate
                .get("operand")
                .ok_or_else(|| CoveError::BadSection("not predicate missing operand".into()))?;
            Ok(evaluate_pruning_predicate(operand, columns)?.not())
        }
        "bloom_membership" => {
            let column = pruning_column(columns, predicate_column_id(predicate)?);
            let value = predicate
                .get("value")
                .ok_or_else(|| CoveError::BadSection("bloom_membership missing value".into()))?;
            let bytes = parse_pruning_byte_string(value, "bloom_membership value")?;
            Ok(explain_bloom_membership(
                &bytes,
                column.and_then(|column| column.bloom.as_ref()),
                column.map(|column| column.bloom_fail_open).unwrap_or(false),
            ))
        }
        "inverted_lookup" => {
            let column = pruning_column(columns, predicate_column_id(predicate)?);
            let key = predicate
                .get("key")
                .and_then(Value::as_u64)
                .ok_or_else(|| CoveError::BadSection("inverted_lookup missing key".into()))?;
            Ok(explain_inverted_morsel_lookup(
                key,
                column.and_then(|column| column.inverted.as_ref()),
                column
                    .map(|column| column.inverted_fail_open)
                    .unwrap_or(false),
            ))
        }
        "lookup_point" => {
            let column = pruning_column(columns, predicate_column_id(predicate)?);
            let key = predicate
                .get("key")
                .and_then(Value::as_u64)
                .ok_or_else(|| CoveError::BadSection("lookup_point missing key".into()))?;
            Ok(explain_lookup_index_point(
                key,
                column.and_then(|column| column.lookup.as_ref()),
                column
                    .map(|column| column.lookup_fail_open)
                    .unwrap_or(false),
            ))
        }
        "composite_zone" => {
            let column = pruning_column(columns, predicate_column_id(predicate)?);
            Ok(explain_composite_zone(
                column.and_then(|column| column.composite.as_ref()),
                column
                    .map(|column| column.composite_fail_open)
                    .unwrap_or(false),
                column
                    .map(|column| column.composite_matches_bindings)
                    .unwrap_or(false),
            ))
        }
        "aggregate_synopsis" => {
            let column = pruning_column(columns, predicate_column_id(predicate)?);
            Ok(explain_aggregate_synopsis(
                column.and_then(|column| column.aggregate.as_ref()),
                column
                    .map(|column| column.aggregate_fail_open)
                    .unwrap_or(false),
                column
                    .map(|column| column.aggregate_proves_no_match)
                    .unwrap_or(false),
            ))
        }
        "reorder_invariant_and" => evaluate_reorder_invariant(predicate, columns, |a, b| a.and(b)),
        "reorder_invariant_or" => evaluate_reorder_invariant(predicate, columns, |a, b| a.or(b)),
        other => Err(CoveError::BadSection(format!(
            "unsupported pruning predicate op {other}"
        ))),
    }
}

fn fold_pruning_operands<F>(
    predicate: &Value,
    columns: &[PruningColumnFixture],
    combine: F,
) -> Result<PruningExplanation, CoveError>
where
    F: Fn(PruningExplanation, PruningExplanation) -> PruningExplanation,
{
    let operands = predicate
        .get("operands")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            CoveError::BadSection("compound pruning predicate missing operands".into())
        })?;
    let mut operands = operands.iter();
    let first = operands.next().ok_or_else(|| {
        CoveError::BadSection("compound pruning predicate must have at least one operand".into())
    })?;
    let mut explanation = evaluate_pruning_predicate(first, columns)?;
    for operand in operands {
        explanation = combine(explanation, evaluate_pruning_predicate(operand, columns)?);
    }
    Ok(explanation)
}

/// Spec §37.5: prove that AND/OR predicates are commutative under reordering.
///
/// Evaluate the operand list in the declared order to produce the canonical
/// explanation, then re-evaluate every other permutation and assert each
/// yields the same `final_outcome`. The runner returns the canonical
/// explanation so the caller can still assert outcome and evidence trace.
fn evaluate_reorder_invariant<F>(
    predicate: &Value,
    columns: &[PruningColumnFixture],
    combine: F,
) -> Result<PruningExplanation, CoveError>
where
    F: Fn(PruningExplanation, PruningExplanation) -> PruningExplanation,
{
    let operand_values: Vec<&Value> = predicate
        .get("operands")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            CoveError::BadSection("reorder_invariant predicate missing operands".into())
        })?
        .iter()
        .collect();
    if operand_values.is_empty() {
        return Err(CoveError::BadSection(
            "reorder_invariant predicate must have at least one operand".into(),
        ));
    }
    let canonical = fold_in_order(&operand_values, columns, &combine)?;
    let mut indices: Vec<usize> = (0..operand_values.len()).collect();
    let mut permutation = indices.clone();
    while next_permutation(&mut permutation) {
        let permuted: Vec<&Value> = permutation.iter().map(|i| operand_values[*i]).collect();
        let alternative = fold_in_order(&permuted, columns, &combine)?;
        if alternative.final_outcome != canonical.final_outcome {
            return Err(CoveError::BadSection(format!(
                "reorder_invariant outcome diverged under permutation {:?}: expected {:?}, got {:?}",
                permutation, canonical.final_outcome, alternative.final_outcome
            )));
        }
        indices.clone_from(&permutation);
    }
    let _ = indices;
    Ok(canonical)
}

fn fold_in_order<F>(
    operands: &[&Value],
    columns: &[PruningColumnFixture],
    combine: &F,
) -> Result<PruningExplanation, CoveError>
where
    F: Fn(PruningExplanation, PruningExplanation) -> PruningExplanation,
{
    let mut iter = operands.iter();
    let first = iter.next().ok_or_else(|| {
        CoveError::BadSection("fold_in_order requires at least one operand".into())
    })?;
    let mut explanation = evaluate_pruning_predicate(first, columns)?;
    for operand in iter {
        explanation = combine(explanation, evaluate_pruning_predicate(operand, columns)?);
    }
    Ok(explanation)
}

/// Lexicographic next-permutation; returns false when no further permutation
/// exists (the slice has been left in the smallest order).
fn next_permutation(slice: &mut [usize]) -> bool {
    if slice.len() < 2 {
        return false;
    }
    let mut i = slice.len() - 1;
    while i > 0 && slice[i - 1] >= slice[i] {
        i -= 1;
    }
    if i == 0 {
        slice.reverse();
        return false;
    }
    let pivot = i - 1;
    let mut j = slice.len() - 1;
    while slice[j] <= slice[pivot] {
        j -= 1;
    }
    slice.swap(pivot, j);
    slice[i..].reverse();
    true
}

fn predicate_column_id(predicate: &Value) -> Result<u32, CoveError> {
    predicate
        .get("column_id")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| CoveError::BadSection("pruning predicate missing column_id".into()))
}

fn pruning_column(
    columns: &[PruningColumnFixture],
    column_id: u32,
) -> Option<&PruningColumnFixture> {
    columns.iter().find(|column| column.column_id == column_id)
}

fn parse_expected_outcome(
    value: &Value,
) -> Result<cove_core::predicate::PredicateZoneOutcome, CoveError> {
    match value
        .as_str()
        .ok_or_else(|| CoveError::BadSection("expect_outcome must be a string".into()))?
    {
        "all_match" => Ok(cove_core::predicate::PredicateZoneOutcome::AllMatch),
        "no_match" => Ok(cove_core::predicate::PredicateZoneOutcome::NoMatch),
        "some_match" => Ok(cove_core::predicate::PredicateZoneOutcome::SomeMatch),
        "unknown" => Ok(cove_core::predicate::PredicateZoneOutcome::Unknown),
        other => Err(CoveError::BadSection(format!(
            "unsupported pruning expect_outcome {other}"
        ))),
    }
}

fn pruning_evidence_name(evidence: PruningEvidence) -> &'static str {
    match evidence {
        PruningEvidence::NoMetadata => "NoMetadata",
        PruningEvidence::ZoneStats => "ZoneStats",
        PruningEvidence::ColumnDomain => "ColumnDomain",
        PruningEvidence::ExactSet => "ExactSet",
        PruningEvidence::BloomFilter => "BloomFilter",
        PruningEvidence::InvertedIndex => "InvertedIndex",
        PruningEvidence::CompositeIndex => "CompositeIndex",
        PruningEvidence::AggregateSynopsis => "AggregateSynopsis",
        PruningEvidence::TopNSummary => "TopNSummary",
        PruningEvidence::FallbackToScan => "FallbackToScan",
    }
}
