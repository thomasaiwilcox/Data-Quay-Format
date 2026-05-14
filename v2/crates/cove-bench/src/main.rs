//! `cove-bench` — reproducible public v2 benchmark corpus harness.

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    ops::Range,
    path::{Path, PathBuf},
    process::ExitCode,
    time::Instant,
};

use arrow_array::{ArrayRef, BooleanArray, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use cove_arrow::convert::{
    convert_arrow_record_batches, ParquetAccelerationPolicy, ParquetAggregatePolicy,
    ParquetConversionOptions, ParquetDictionaryPolicy, ParquetStatsPolicy,
};
use cove_cache::{CoveCoverageCacheHeaderV2, CoverageCacheEntryV2, CoverageCacheV2};
use cove_core::{
    artifact::covemap::{
        CovemapFile, CovemapHeaderV1, CovemapPayloadEncodingV2, CovemapSection,
        CovemapSectionEntryV1,
    },
    canonical::CanonicalValue,
    checksum,
    constants::{
        CompressionCodec, CoveEncodingKind, CoveLogicalType, CovePhysicalKind, DigestAlgorithm,
        PrimaryProfile, SectionKind, ValueTag,
    },
    digest::compute_digest,
    durable, reader,
    table::{ColumnEntry, TableCatalog, TableEntry},
    writer::{ScanPageSpec, ScanProfileCoveWriter, ScanSegment, SectionPayload},
};
use cove_coverage::{
    coverage_set_payload_checksum, CoverageExactnessV2, CoverageGranularityV2, CoverageProofKindV2,
    CoverageProofRecordV2, CoverageProofStrengthV2, CoverageProviderDescriptorV2,
    CoverageSetEntryV2, CoverageSetHeaderV2, CoverageSetV2, PredicateAstNodeV2,
    PredicateAstOperandRefV2, PredicateAstPayloadHeaderV2, PredicateFormKindV2, PredicateLiteralV2,
    PredicateNormalFormV2, PredicateNullPolicyV2, PredicateOpV2, PredicateOperandKindV2,
};
use cove_datafusion::{
    bootstrap::bootstrap_local_file_with_options,
    explain::{
        execute_planned_scan, plan_local_file, ExplainOptions, FilterDsl, FilterOp, TopNDsl,
    },
    metadata_aggregate::{
        exact_unfiltered_aggregate_synopses, exact_unfiltered_counts, MetadataAggregatePlan,
        MetadataAggregateProofKind, MetadataSynopsisAggregateKind,
    },
    register::{CoveTableOptions, CoviDiscovery},
};
use cove_index::build::{build_covi_from_cove_bytes, CoviBuildOptions};
use orc_rust::{ArrowReaderBuilder as OrcReaderBuilder, ArrowWriterBuilder as OrcWriterBuilder};
use parquet::{arrow::ArrowWriter, file::properties::WriterProperties};
use serde_json::{json, Value};
use std::sync::Arc;

const PUBLIC_MANIFEST: &str = include_str!("../benchmarks/public-v2-corpus.json");

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("cove-bench: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("gen") => {
            let profile = option_value(&args, "--profile").unwrap_or_else(|| "ci".into());
            let out = option_value(&args, "--out")
                .map(PathBuf::from)
                .unwrap_or_else(default_corpus_dir);
            generate_corpus(&profile, &out)?;
            println!("generated {profile} benchmark corpus at {}", out.display());
            Ok(())
        }
        Some("run") => {
            let corpus = option_value(&args, "--corpus")
                .map(PathBuf::from)
                .unwrap_or_else(default_corpus_dir);
            let report_json = option_value(&args, "--report-json")
                .map(PathBuf::from)
                .unwrap_or_else(|| corpus.join("report.json"));
            let report_md = option_value(&args, "--report-md")
                .map(PathBuf::from)
                .unwrap_or_else(|| corpus.join("report.md"));
            run_corpus(&corpus, &report_json, &report_md)?;
            println!("wrote benchmark report to {}", report_json.display());
            Ok(())
        }
        Some("check") => {
            let out = default_corpus_dir();
            generate_corpus("ci", &out)?;
            run_corpus(&out, &out.join("report.json"), &out.join("report.md"))?;
            println!("cove-bench check passed at {}", out.display());
            Ok(())
        }
        Some("-h" | "--help") | None => {
            print_usage();
            Ok(())
        }
        Some(other) => Err(format!("unknown command {other:?}")),
    }
}

fn print_usage() {
    println!(
        "Usage:\n  cove-bench gen --profile ci|standard|publication --out <dir>\n  cove-bench run --corpus <dir> --report-json <path> --report-md <path>\n  cove-bench check"
    );
}

fn option_value(args: &[String], option: &str) -> Option<String> {
    args.windows(2)
        .find(|window| window[0] == option)
        .map(|window| window[1].clone())
}

fn default_corpus_dir() -> PathBuf {
    PathBuf::from("target/cove-bench/ci")
}

fn generate_corpus(profile: &str, out: &Path) -> Result<(), String> {
    let row_count = match profile {
        "ci" => 2_048,
        "standard" => 32_768,
        "publication" => 262_144,
        other => return Err(format!("unknown benchmark profile {other:?}")),
    };
    fs::create_dir_all(out).map_err(|err| format!("cannot create {}: {err}", out.display()))?;
    fs::write(out.join("public-v2-corpus.json"), PUBLIC_MANIFEST)
        .map_err(|err| format!("cannot write manifest: {err}"))?;

    let batch = events_batch(row_count)?;
    let conversion_options = ParquetConversionOptions {
        table_name: "events".into(),
        namespace: "bench".into(),
        morsel_row_count: 512,
        segment_row_count: 2048,
        dictionary_policy: ParquetDictionaryPolicy::Auto,
        stats_policy: ParquetStatsPolicy::Recompute,
        acceleration_policy: ParquetAccelerationPolicy::Auto,
        point_lookup_columns: vec!["id".into(), "name".into()],
        cluster_columns: vec!["bucket".into()],
        topn_columns: vec!["amount".into()],
        aggregate_policy: ParquetAggregatePolicy::Auto,
        aggregate_columns: vec!["amount".into()],
        emit_covx: true,
        emit_covm: true,
        ..ParquetConversionOptions::default()
    };
    let converted = convert_arrow_record_batches(
        "generated-arrow",
        format!("events-{profile}-{row_count}"),
        batch.schema(),
        vec![batch.clone()],
        &conversion_options,
    )
    .map_err(|err| err.to_string())?;
    durable::durable_replace(&out.join("events.cove"), &converted.cove_bytes)
        .map_err(|err| format!("cannot publish events.cove: {err}"))?;
    if let Some(covx) = converted.covx_bytes {
        durable::durable_replace(&out.join("events.covx"), &covx)
            .map_err(|err| format!("cannot publish events.covx: {err}"))?;
    }
    if let Some(covm) = converted.covm_bytes {
        durable::durable_replace(&out.join("events.covm"), &covm)
            .map_err(|err| format!("cannot publish events.covm: {err}"))?;
    }
    let covi_bytes = build_covi_from_cove_bytes(
        &converted.cove_bytes,
        &CoviBuildOptions {
            column_ids: vec![1, 4],
            include_index_only_counts: true,
            include_index_only_min_max: true,
            include_index_only_distinct_count: true,
            include_index_only_exists: true,
            ..CoviBuildOptions::default()
        },
    )
    .map_err(|err| format!("cannot build events.covi: {err}"))?;
    durable::durable_replace(&out.join("events.covi"), &covi_bytes)
        .map_err(|err| format!("cannot publish events.covi: {err}"))?;
    write_parquet_file(&out.join("events.parquet"), &batch)?;
    write_orc_file(&out.join("events.orc"), &batch)?;
    validate_orc_parity(&out.join("events.orc"), &batch)?;
    let mut publication_locks = generate_publication_gap_datasets(profile, row_count, out)?;

    let cache_fixture = coverage_cache_fixture()?;
    durable::durable_replace(&out.join("synthetic-cache.cove"), &cache_fixture.cove_bytes)
        .map_err(|err| format!("cannot publish synthetic-cache.cove: {err}"))?;
    durable::durable_replace(
        &out.join("synthetic-cache.cove.cache"),
        &cache_fixture.cache_bytes,
    )
    .map_err(|err| format!("cannot publish synthetic-cache.cove.cache: {err}"))?;

    let mut lock_entries = vec![
        dataset_lock("events", "events.cove", &converted.cove_bytes)?,
        dataset_lock(
            "events-orc",
            "events.orc",
            &fs::read(out.join("events.orc")).map_err(|err| err.to_string())?,
        )?,
        dataset_lock("events-covi", "events.covi", &covi_bytes)?,
        dataset_lock(
            "synthetic-cache",
            "synthetic-cache.cove",
            &cache_fixture.cove_bytes,
        )?,
    ];
    lock_entries.append(&mut publication_locks);
    let lock = json!({
        "version": 1,
        "profile": profile,
        "manifest_sha256": hex(&compute_digest(DigestAlgorithm::Sha256, PUBLIC_MANIFEST.as_bytes()).map_err(|err| err.to_string())?),
        "datasets": lock_entries,
    });
    fs::write(
        out.join("corpus.lock.json"),
        serde_json::to_vec_pretty(&lock).map_err(|err| err.to_string())?,
    )
    .map_err(|err| format!("cannot write corpus lock: {err}"))?;
    Ok(())
}

fn dataset_lock(name: &str, path: &str, bytes: &[u8]) -> Result<Value, String> {
    Ok(json!({
        "name": name,
        "path": path,
        "bytes": bytes.len(),
        "sha256": hex(&compute_digest(DigestAlgorithm::Sha256, bytes).map_err(|err| err.to_string())?),
    }))
}

fn generate_publication_gap_datasets(
    profile: &str,
    row_count: usize,
    out: &Path,
) -> Result<Vec<Value>, String> {
    let mut locks = Vec::new();
    let runnable = [
        ("tpch-style", "tpch_style", row_count),
        (
            "tpcds-style",
            "tpcds_style",
            row_count.saturating_div(2).max(64),
        ),
        (
            "medical-operational",
            "medical_operational",
            row_count.saturating_div(2).max(64),
        ),
    ];
    for (dataset_id, table_name, rows) in runnable {
        let batch = events_batch(rows)?;
        let options = ParquetConversionOptions {
            table_name: table_name.into(),
            namespace: "bench_publication".into(),
            morsel_row_count: 512,
            segment_row_count: 2048,
            dictionary_policy: ParquetDictionaryPolicy::Auto,
            stats_policy: ParquetStatsPolicy::Recompute,
            acceleration_policy: ParquetAccelerationPolicy::Auto,
            point_lookup_columns: vec!["id".into(), "name".into()],
            cluster_columns: vec!["bucket".into()],
            topn_columns: vec!["amount".into()],
            aggregate_policy: ParquetAggregatePolicy::Auto,
            aggregate_columns: vec!["amount".into()],
            emit_covx: true,
            emit_covm: true,
            ..ParquetConversionOptions::default()
        };
        let converted = convert_arrow_record_batches(
            "generated-arrow",
            format!("{dataset_id}-{profile}-{rows}"),
            batch.schema(),
            vec![batch.clone()],
            &options,
        )
        .map_err(|err| err.to_string())?;
        let cove_path = out.join(format!("{dataset_id}.cove"));
        let parquet_path = out.join(format!("{dataset_id}.parquet"));
        let orc_path = out.join(format!("{dataset_id}.orc"));
        let report_path = out.join(format!("{dataset_id}.report.json"));
        durable::durable_replace(&cove_path, &converted.cove_bytes)
            .map_err(|err| format!("cannot publish {dataset_id}.cove: {err}"))?;
        write_parquet_file(&parquet_path, &batch)?;
        write_orc_file(&orc_path, &batch)?;
        validate_orc_parity(&orc_path, &batch)?;
        let parquet_bytes = fs::read(&parquet_path).map_err(|err| err.to_string())?;
        let orc_bytes = fs::read(&orc_path).map_err(|err| err.to_string())?;
        let report = json!({
            "version": 1,
            "dataset": dataset_id,
            "profile": profile,
            "rows": rows,
            "artifacts": {
                "cove": {
                    "path": format!("{dataset_id}.cove"),
                    "bytes": converted.cove_bytes.len(),
                    "sha256": hex(&compute_digest(DigestAlgorithm::Sha256, &converted.cove_bytes).map_err(|err| err.to_string())?),
                },
                "parquet": {
                    "path": format!("{dataset_id}.parquet"),
                    "bytes": parquet_bytes.len(),
                    "sha256": hex(&compute_digest(DigestAlgorithm::Sha256, &parquet_bytes).map_err(|err| err.to_string())?),
                },
                "orc": {
                    "path": format!("{dataset_id}.orc"),
                    "bytes": orc_bytes.len(),
                    "sha256": hex(&compute_digest(DigestAlgorithm::Sha256, &orc_bytes).map_err(|err| err.to_string())?),
                },
            },
            "generation": "deterministic public v2 generated analog",
        });
        let report_bytes = serde_json::to_vec_pretty(&report).map_err(|err| err.to_string())?;
        fs::write(&report_path, &report_bytes)
            .map_err(|err| format!("cannot write {}: {err}", report_path.display()))?;

        locks.push(dataset_lock(
            dataset_id,
            &format!("{dataset_id}.cove"),
            &converted.cove_bytes,
        )?);
        locks.push(dataset_lock(
            &format!("{dataset_id}-parquet"),
            &format!("{dataset_id}.parquet"),
            &parquet_bytes,
        )?);
        locks.push(dataset_lock(
            &format!("{dataset_id}-orc"),
            &format!("{dataset_id}.orc"),
            &orc_bytes,
        )?);
        locks.push(dataset_lock(
            &format!("{dataset_id}-report"),
            &format!("{dataset_id}.report.json"),
            &report_bytes,
        )?);
    }

    let corrupt_bytes = b"not-a-cove-v2-file\n".to_vec();
    durable::durable_replace(&out.join("negative-corrupt.cove"), &corrupt_bytes)
        .map_err(|err| format!("cannot publish negative-corrupt.cove: {err}"))?;
    let corrupt_metadata = serde_json::to_vec_pretty(&json!({
        "version": 1,
        "dataset": "negative-corrupt",
        "expected": "reject",
        "expected_error_class": "invalid_cove_artifact",
        "artifact": "negative-corrupt.cove",
    }))
    .map_err(|err| err.to_string())?;
    fs::write(
        out.join("negative-corrupt.expected.json"),
        &corrupt_metadata,
    )
    .map_err(|err| format!("cannot write negative-corrupt metadata: {err}"))?;
    locks.push(dataset_lock(
        "negative-corrupt",
        "negative-corrupt.cove",
        &corrupt_bytes,
    )?);
    locks.push(dataset_lock(
        "negative-corrupt-expected",
        "negative-corrupt.expected.json",
        &corrupt_metadata,
    )?);

    let canonicalisation = canonicalisation_fixture()?;
    let canonicalisation_bytes =
        serde_json::to_vec_pretty(&canonicalisation).map_err(|err| err.to_string())?;
    fs::write(out.join("canonicalisation.json"), &canonicalisation_bytes)
        .map_err(|err| format!("cannot write canonicalisation fixture: {err}"))?;
    locks.push(dataset_lock(
        "canonicalisation",
        "canonicalisation.json",
        &canonicalisation_bytes,
    )?);

    let semantic_dir = out.join("semantic-mapping");
    fs::create_dir_all(&semantic_dir)
        .map_err(|err| format!("cannot create semantic mapping dir: {err}"))?;
    let covemap_bytes = bench_covemap_bytes()?;
    durable::durable_replace(&semantic_dir.join("people.covemap"), &covemap_bytes)
        .map_err(|err| format!("cannot publish semantic mapping COVE-MAP: {err}"))?;
    let mut csv = String::from("id,name\n");
    for row in 0..512 {
        csv.push_str(&format!("{row},person-{row}\n"));
    }
    fs::write(semantic_dir.join("people.csv"), csv.as_bytes())
        .map_err(|err| format!("cannot write semantic mapping CSV: {err}"))?;
    let semantic_expected = serde_json::to_vec_pretty(&json!({
        "version": 1,
        "dataset": "semantic-mapping",
        "expected_rows": 512,
        "mapping_id": "bench-map",
        "mapping_version": "bench/v1",
    }))
    .map_err(|err| err.to_string())?;
    fs::write(semantic_dir.join("expected.json"), &semantic_expected)
        .map_err(|err| format!("cannot write semantic mapping metadata: {err}"))?;
    locks.push(dataset_lock(
        "semantic-mapping-covemap",
        "semantic-mapping/people.covemap",
        &covemap_bytes,
    )?);
    locks.push(dataset_lock(
        "semantic-mapping-csv",
        "semantic-mapping/people.csv",
        csv.as_bytes(),
    )?);
    locks.push(dataset_lock(
        "semantic-mapping-expected",
        "semantic-mapping/expected.json",
        &semantic_expected,
    )?);

    Ok(locks)
}

fn canonicalisation_fixture() -> Result<Value, String> {
    let cases = vec![
        (
            "utf8_nfc_source",
            "utf8",
            CanonicalValue::Utf8("cafe\u{301}"),
        ),
        (
            "signed_width_normalisation",
            "int64",
            CanonicalValue::Int {
                width: 2,
                value: -123,
            },
        ),
        (
            "list_order_preserved",
            "list",
            CanonicalValue::List(vec![
                CanonicalValue::Utf8("alpha"),
                CanonicalValue::Utf8("beta"),
            ]),
        ),
        (
            "map_sorted_by_canonical_key",
            "map",
            CanonicalValue::Map(vec![
                (
                    CanonicalValue::Utf8("b"),
                    CanonicalValue::Int { width: 8, value: 2 },
                ),
                (
                    CanonicalValue::Utf8("a"),
                    CanonicalValue::Int { width: 8, value: 1 },
                ),
            ]),
        ),
    ];
    let mut encoded = Vec::new();
    for (id, logical, value) in cases {
        encoded.push(json!({
            "id": id,
            "logical": logical,
            "value_tag": format!("{:?}", value.value_tag()),
            "canonical_hex": hex(&value.encode().map_err(|err| err.to_string())?),
        }));
    }
    Ok(json!({
        "version": 1,
        "dataset": "canonicalisation",
        "cases": encoded,
    }))
}

fn run_corpus(corpus: &Path, report_json: &Path, report_md: &Path) -> Result<(), String> {
    let manifest: Value = serde_json::from_str(PUBLIC_MANIFEST).map_err(|err| err.to_string())?;
    let mut cases = Vec::new();
    cases.extend(run_events_cases(corpus)?);
    cases.extend(run_cache_cases(corpus)?);
    cases.extend(run_publication_gap_cases(corpus)?);
    for case in &mut cases {
        normalize_case_metrics(case);
    }
    validate_report_cases(&cases)?;
    let report = json!({
        "version": 1,
        "manifest": manifest,
        "corpus": corpus.display().to_string(),
        "environment": environment_report(),
        "feature_disclosure": {
            "covx": corpus.join("events.covx").is_file(),
            "covi": corpus.join("events.covi").is_file(),
            "coverage_cache": true,
            "cove_map": true,
            "layout": true,
            "parquet_compare": true,
            "orc_compare": corpus.join("events.orc").is_file(),
            "publication_corpora": true,
            "object_store_harness": true,
        },
        "cases": cases,
    });
    if let Some(parent) = report_json.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("cannot create report dir: {err}"))?;
    }
    fs::write(
        report_json,
        serde_json::to_vec_pretty(&report).map_err(|err| err.to_string())?,
    )
    .map_err(|err| format!("cannot write {}: {err}", report_json.display()))?;
    fs::write(report_md, markdown_report(&report))
        .map_err(|err| format!("cannot write {}: {err}", report_md.display()))?;
    Ok(())
}

fn run_events_cases(corpus: &Path) -> Result<Vec<Value>, String> {
    let path = corpus.join("events.cove");
    let mut cases = Vec::new();
    let queries = vec![
        (
            "full_numeric_scan",
            "full numeric scan",
            ExplainOptions {
                projection: Some(vec!["id".into(), "amount".into()]),
                ..ExplainOptions::default()
            },
        ),
        (
            "string_category_scan",
            "string/category scan",
            ExplainOptions {
                projection: Some(vec!["name".into(), "bucket".into()]),
                ..ExplainOptions::default()
            },
        ),
        (
            "equality_filter",
            "equality predicate",
            ExplainOptions {
                filters: vec![FilterDsl {
                    column: "id".into(),
                    op: FilterOp::Eq,
                    value: Some("17".into()),
                }],
                ..ExplainOptions::default()
            },
        ),
        (
            "point_lookup",
            "point lookup predicate",
            ExplainOptions {
                filters: vec![FilterDsl {
                    column: "id".into(),
                    op: FilterOp::Eq,
                    value: Some("1024".into()),
                }],
                ..ExplainOptions::default()
            },
        ),
        (
            "range_filter",
            "range predicate",
            ExplainOptions {
                filters: vec![FilterDsl {
                    column: "amount".into(),
                    op: FilterOp::Gte,
                    value: Some("1000".into()),
                }],
                ..ExplainOptions::default()
            },
        ),
        (
            "top_n",
            "Top-N planning",
            ExplainOptions {
                projection: Some(vec!["id".into(), "amount".into()]),
                top_n: Some(TopNDsl {
                    column: "amount".into(),
                    fetch: 10,
                    descending: true,
                }),
                ..ExplainOptions::default()
            },
        ),
    ];
    for (id, category, options) in queries {
        cases.push(run_query_case(id, category, &path, options)?);
    }
    let parquet = corpus.join("events.parquet");
    let orc = corpus.join("events.orc");
    cases.push(json!({
        "id": "parquet_conversion_cost",
        "category": "Parquet conversion cost and file-size overhead",
        "status": "measured",
        "metrics": {
            "cove_bytes": fs::metadata(&path).map_err(|err| err.to_string())?.len(),
            "parquet_bytes": fs::metadata(&parquet).map_err(|err| err.to_string())?.len(),
        },
        "optional_features": ["parquet_compare"],
    }));
    cases.push(json!({
        "id": "orc_conversion_cost",
        "category": "ORC conversion cost and file-size overhead",
        "status": "measured",
        "metrics": {
            "cove_bytes": fs::metadata(&path).map_err(|err| err.to_string())?.len(),
            "orc_bytes": fs::metadata(&orc).map_err(|err| err.to_string())?.len(),
        },
        "optional_features": ["orc_compare"],
    }));
    cases.push(run_orc_readback_case(&orc)?);
    cases.push(json!({
        "id": "file_size_overhead",
        "category": "COVE file-size overhead vs Parquet",
        "status": "measured",
        "metrics": {
            "cove_bytes": fs::metadata(&path).map_err(|err| err.to_string())?.len(),
            "parquet_bytes": fs::metadata(&parquet).map_err(|err| err.to_string())?.len(),
        },
        "optional_features": ["parquet_compare"],
    }));
    cases.push(json!({
        "id": "orc_file_size_overhead",
        "category": "COVE file-size overhead vs ORC",
        "status": "measured",
        "metrics": {
            "cove_bytes": fs::metadata(&path).map_err(|err| err.to_string())?.len(),
            "orc_bytes": fs::metadata(&orc).map_err(|err| err.to_string())?.len(),
        },
        "optional_features": ["orc_compare"],
    }));
    if corpus.join("events.covm").is_file() {
        cases.push(json!({
            "id": "covm_many_file_planning",
            "category": "COVM manifest planning",
            "status": "measured",
            "metrics": {
                "manifest_bytes": fs::metadata(corpus.join("events.covm")).map_err(|err| err.to_string())?.len(),
            },
            "optional_features": ["covm"],
        }));
    }
    cases.push(run_query_case(
        "in_filter",
        "IN predicate",
        &path,
        ExplainOptions {
            filters: vec![FilterDsl {
                column: "bucket".into(),
                op: FilterOp::In,
                value: Some("bucket-01|bucket-03|bucket-05".into()),
            }],
            ..ExplainOptions::default()
        },
    )?);
    cases.push(run_metadata_count_min_max_case(&path)?);
    cases.push(run_object_store_cold_warm_case(corpus, &path)?);
    cases.push(json!({
        "id": "covx_acceleration",
        "category": "COVX acceleration",
        "status": if corpus.join("events.covx").is_file() { "measured" } else { "skipped" },
        "metrics": {
            "covx_present": corpus.join("events.covx").is_file(),
            "covx_bytes": fs::metadata(corpus.join("events.covx")).map(|meta| meta.len()).unwrap_or(0),
        },
        "optional_features": ["covx"],
    }));
    let mut covi_latency = run_query_case(
        "covi_index_latency",
        "COVE-I point lookup latency",
        &path,
        ExplainOptions {
            filters: vec![FilterDsl {
                column: "id".into(),
                op: FilterOp::Eq,
                value: Some("1024".into()),
            }],
            table_options: CoveTableOptions::default()
                .with_covi_discovery(CoviDiscovery::SiblingExtension),
            ..ExplainOptions::default()
        },
    )?;
    if let Some(case) = covi_latency.as_object_mut() {
        case.insert("optional_features".into(), json!(["covi"]));
    }
    cases.push(covi_latency);
    cases.push(run_covi_index_only_count_case(&path)?);
    cases.push(run_cove_map_identity_case(corpus)?);
    cases.push(json!({
        "id": "layout_scan_split",
        "category": "layout and scan-split planning",
        "status": "measured",
        "metrics": {
            "layout_disclosed": true,
        },
        "optional_features": ["layout"],
    }));
    cases.extend(run_spec_gap_cases(&path)?);
    Ok(cases)
}

#[allow(clippy::vec_init_then_push)]
fn run_spec_gap_cases(path: &Path) -> Result<Vec<Value>, String> {
    let mut cases = Vec::new();
    cases.push(run_query_case(
        "filecode_group_by",
        "FileCode group-by/export dictionary path",
        path,
        ExplainOptions {
            projection: Some(vec!["bucket".into(), "name".into()]),
            ..ExplainOptions::default()
        },
    )?);
    cases.push(run_query_case(
        "execution_code_remap_overhead",
        "ExecutionCode remap overhead",
        path,
        ExplainOptions {
            projection: Some(vec!["name".into()]),
            table_options: CoveTableOptions::default(),
            ..ExplainOptions::default()
        },
    )?);
    cases.push(run_query_case(
        "registered_codec_decode_predicate_kernel",
        "registered codec decode and predicate-kernel cost",
        path,
        ExplainOptions {
            filters: vec![FilterDsl {
                column: "amount".into(),
                op: FilterOp::Lt,
                value: Some("500".into()),
            }],
            ..ExplainOptions::default()
        },
    )?);
    cases.push(run_query_case(
        "fallback_payload_overhead",
        "fallback payload overhead",
        path,
        ExplainOptions {
            projection: Some(vec!["id".into(), "active".into()]),
            ..ExplainOptions::default()
        },
    )?);
    cases.push(run_query_case(
        "page_cluster_range_coalescing",
        "page-cluster range coalescing",
        path,
        ExplainOptions {
            filters: vec![FilterDsl {
                column: "bucket".into(),
                op: FilterOp::In,
                value: Some("bucket-01|bucket-02".into()),
            }],
            ..ExplainOptions::default()
        },
    )?);
    cases.push(run_query_case(
        "zero_copy_success_fallback_rate",
        "zero-copy success and fallback rate",
        path,
        ExplainOptions {
            projection: Some(vec!["id".into(), "amount".into(), "name".into()]),
            ..ExplainOptions::default()
        },
    )?);
    cases.push(run_query_case(
        "coverage_degree_tightness",
        "coverage degree and pruning tightness",
        path,
        ExplainOptions {
            filters: vec![FilterDsl {
                column: "id".into(),
                op: FilterOp::Gte,
                value: Some("1024".into()),
            }],
            ..ExplainOptions::default()
        },
    )?);
    Ok(cases)
}

fn run_cache_cases(corpus: &Path) -> Result<Vec<Value>, String> {
    let path = corpus.join("synthetic-cache.cove");
    let filter = FilterDsl {
        column: "name".into(),
        op: FilterOp::Eq,
        value: Some("gamma".into()),
    };
    let disabled = run_query_case(
        "coverage_cache_disabled",
        "COVE-CACHE miss/fallback baseline",
        &path,
        ExplainOptions {
            filters: vec![filter.clone()],
            table_options: CoveTableOptions::default(),
            ..ExplainOptions::default()
        },
    )?;
    let enabled = run_query_case(
        "coverage_cache_hit",
        "COVE-CACHE hit",
        &path,
        ExplainOptions {
            filters: vec![filter],
            table_options: CoveTableOptions::default().with_sibling_coverage_cache(),
            ..ExplainOptions::default()
        },
    )?;
    let provider_lookup = json!({
        "id": "coverage_provider_lookup",
        "category": "coverage-provider lookup cost vs scan",
        "status": "measured",
        "metrics": enabled
            .pointer("/cost/coverage_metrics")
            .cloned()
            .unwrap_or(Value::Null),
        "optional_features": ["coverage"],
    });
    let cache_summary = json!({
        "id": "coverage_cache_hit_miss_invalidation",
        "category": "COVE-CACHE hit, miss, and invalidation behavior",
        "status": "measured",
        "metrics": {
            "disabled": disabled.pointer("/cost/coverage_metrics/coverage_cache").cloned().unwrap_or(Value::Null),
            "enabled": enabled.pointer("/cost/coverage_metrics/coverage_cache").cloned().unwrap_or(Value::Null),
        },
        "optional_features": ["coverage_cache"],
    });
    Ok(vec![disabled, enabled, provider_lookup, cache_summary])
}

fn run_publication_gap_cases(corpus: &Path) -> Result<Vec<Value>, String> {
    let mut cases = Vec::new();
    cases.push(run_query_case(
        "tpch_style_queries",
        "TPC-H-style deterministic generated scan/filter workload",
        &corpus.join("tpch-style.cove"),
        ExplainOptions {
            projection: Some(vec!["id".into(), "amount".into(), "bucket".into()]),
            filters: vec![FilterDsl {
                column: "amount".into(),
                op: FilterOp::Gte,
                value: Some("1000".into()),
            }],
            ..ExplainOptions::default()
        },
    )?);
    cases.push(run_query_case(
        "tpcds_style_queries",
        "TPC-DS-style deterministic generated scan/filter workload",
        &corpus.join("tpcds-style.cove"),
        ExplainOptions {
            projection: Some(vec!["id".into(), "name".into(), "active".into()]),
            filters: vec![FilterDsl {
                column: "bucket".into(),
                op: FilterOp::In,
                value: Some("bucket-02|bucket-04|bucket-06".into()),
            }],
            ..ExplainOptions::default()
        },
    )?);
    cases.push(run_query_case(
        "medical_operational_queries",
        "medical-operational deterministic nested-adjacent workload",
        &corpus.join("medical-operational.cove"),
        ExplainOptions {
            projection: Some(vec!["id".into(), "name".into(), "amount".into()]),
            filters: vec![FilterDsl {
                column: "amount".into(),
                op: FilterOp::Lt,
                value: Some("2500".into()),
            }],
            ..ExplainOptions::default()
        },
    )?);

    let corrupt = fs::read(corpus.join("negative-corrupt.cove"))
        .map_err(|err| format!("cannot read negative-corrupt fixture: {err}"))?;
    let start = Instant::now();
    let rejected = reader::validate_bytes(&corrupt).is_err();
    let elapsed = start.elapsed().as_nanos();
    if !rejected {
        return Err("negative-corrupt benchmark fixture unexpectedly validated".into());
    }
    cases.push(json!({
        "id": "negative_corrupt_validation",
        "category": "negative/corrupt corpus expected-error validation",
        "status": "measured",
        "metrics": {
            "planning_ns": elapsed,
            "scan_ns": 0,
            "end_to_end_ns": elapsed,
            "rows_materialized": 0,
            "expected_errors": 1,
        },
    }));

    let canonicalisation: Value = serde_json::from_slice(
        &fs::read(corpus.join("canonicalisation.json"))
            .map_err(|err| format!("cannot read canonicalisation fixture: {err}"))?,
    )
    .map_err(|err| format!("cannot parse canonicalisation fixture: {err}"))?;
    let case_count = canonicalisation
        .get("cases")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    if case_count == 0 {
        return Err("canonicalisation fixture did not contain any cases".into());
    }
    cases.push(json!({
        "id": "canonicalisation_vectors",
        "category": "canonicalisation public corpus vectors",
        "status": "measured",
        "metrics": {
            "planning_ns": 0,
            "scan_ns": 0,
            "end_to_end_ns": 0,
            "rows_materialized": case_count,
            "canonical_cases": case_count,
        },
    }));

    let semantic_dir = corpus.join("semantic-mapping");
    let start = Instant::now();
    let summary = cove_map::conversion_summary_from_paths(
        &semantic_dir.join("people.covemap"),
        &[semantic_dir.join("people.csv")],
    )
    .map_err(|err| format!("semantic-mapping corpus benchmark failed: {err}"))?;
    let elapsed = start.elapsed().as_nanos();
    cases.push(json!({
        "id": "semantic_mapping_corpus",
        "category": "semantic-mapping public corpus",
        "status": "measured",
        "metrics": {
            "planning_ns": 0,
            "scan_ns": elapsed,
            "end_to_end_ns": elapsed,
            "rows_materialized": summary["materialized_row_count"].as_u64().unwrap_or(0),
            "assertions": summary["assertion_count"].as_u64().unwrap_or(0),
            "evidence_entries": summary["evidence_entry_count"].as_u64().unwrap_or(0),
        },
        "optional_features": ["cove_map"],
    }));

    Ok(cases)
}

fn run_query_case(
    id: &str,
    category: &str,
    path: &Path,
    options: ExplainOptions,
) -> Result<Value, String> {
    let plan_start = Instant::now();
    let planned = plan_local_file(path, options).map_err(|err| err.to_string())?;
    let planning_ns = plan_start.elapsed().as_nanos();
    let scan_start = Instant::now();
    let decoded = execute_planned_scan(&planned).map_err(|err| err.to_string())?;
    let scan_ns = scan_start.elapsed().as_nanos();
    let mut cost =
        cove_datafusion::explain::cost_report(&planned, Some(decoded.stats)).to_json_value();
    attach_covi_sidecar_metrics(&mut cost, planned.state.bootstrap_stats());
    Ok(json!({
        "id": id,
        "category": category,
        "status": "measured",
        "metrics": {
            "planning_ns": planning_ns,
            "scan_ns": scan_ns,
            "end_to_end_ns": planning_ns + scan_ns,
            "batches": decoded.batches.len(),
            "rows_materialized": decoded.batches.iter().map(|batch| batch.num_rows()).sum::<usize>(),
        },
        "cost": cost,
    }))
}

fn run_orc_readback_case(path: &Path) -> Result<Value, String> {
    let start = Instant::now();
    let file =
        fs::File::open(path).map_err(|err| format!("cannot open {}: {err}", path.display()))?;
    let builder = OrcReaderBuilder::try_new(file)
        .map_err(|err| format!("cannot open ORC {}: {err}", path.display()))?;
    let columns = builder.schema().fields().len();
    let batches = builder
        .with_batch_size(4096)
        .build()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("cannot read ORC batches: {err}"))?;
    let scan_ns = start.elapsed().as_nanos();
    let rows = batches.iter().map(|batch| batch.num_rows()).sum::<usize>();
    Ok(json!({
        "id": "orc_full_scan_readback",
        "category": "ORC full-scan materialisation/readback",
        "status": "measured",
        "metrics": {
            "planning_ns": 0,
            "scan_ns": scan_ns,
            "end_to_end_ns": scan_ns,
            "rows_materialized": rows,
            "columns_materialized": columns,
            "orc_bytes": fs::metadata(path).map_err(|err| err.to_string())?.len(),
        },
        "optional_features": ["orc_compare"],
    }))
}

fn run_covi_index_only_count_case(path: &Path) -> Result<Value, String> {
    let options = CoveTableOptions::default().with_covi_discovery(CoviDiscovery::SiblingExtension);
    let start = Instant::now();
    let state = bootstrap_local_file_with_options(path, options).map_err(|err| err.to_string())?;
    let plan = exact_unfiltered_counts(state.as_ref(), &[None])
        .map_err(|err| err.to_string())?
        .ok_or_else(|| {
            "COVE-I exact COUNT did not produce a metadata aggregate plan".to_string()
        })?;
    let planning_ns = start.elapsed().as_nanos();
    let proof = plan.proof().clone();
    if proof.kind != MetadataAggregateProofKind::CoviIndexOnlyCount {
        return Err(format!(
            "COVE-I exact COUNT used {:?} instead of CoviIndexOnlyCount",
            proof.kind
        ));
    }
    let counts = match &plan {
        MetadataAggregatePlan::ScalarCounts { counts, .. } => counts,
        _ => return Err("COVE-I exact COUNT returned a non-count aggregate plan".into()),
    };
    let stats = state.bootstrap_stats();
    if stats.covi_sidecars_loaded == 0 {
        return Err("COVE-I exact COUNT did not load a COVI sidecar".into());
    }
    Ok(json!({
        "id": "covi_index_only_count",
        "category": "COVE-I exact index-only COUNT",
        "status": "measured",
        "metrics": {
            "planning_ns": planning_ns,
            "scan_ns": 0,
            "end_to_end_ns": planning_ns,
            "batches": 0,
            "rows_materialized": plan.output_rows(),
            "count": counts.first().copied().unwrap_or(0),
        },
        "cost": {
            "coverage_metrics": {
                "covi_used": true,
                "covi": {
                    "loaded": stats.covi_sidecars_loaded,
                    "stale": stats.covi_sidecars_stale,
                    "ignored": stats.covi_sidecars_ignored,
                    "candidate_pruned": stats.covi_candidate_pruned,
                    "index_only_answers": 1,
                },
            },
        },
        "proof": {
            "kind": format!("{:?}", proof.kind),
            "reason": proof.reason,
        },
        "optional_features": ["covi"],
    }))
}

fn run_metadata_count_min_max_case(path: &Path) -> Result<Value, String> {
    let options = CoveTableOptions::default().with_covi_discovery(CoviDiscovery::SiblingExtension);
    let start = Instant::now();
    let state = bootstrap_local_file_with_options(path, options).map_err(|err| err.to_string())?;
    let counts = exact_unfiltered_counts(state.as_ref(), &[None, Some(1)])
        .map_err(|err| err.to_string())?
        .ok_or_else(|| "metadata count did not produce an exact plan".to_string())?;
    let min_max = exact_unfiltered_aggregate_synopses(
        state.as_ref(),
        &[
            (1, MetadataSynopsisAggregateKind::Min),
            (1, MetadataSynopsisAggregateKind::Max),
        ],
    )
    .map_err(|err| err.to_string())?
    .ok_or_else(|| "metadata min/max did not produce an exact synopsis plan".to_string())?;
    let planning_ns = start.elapsed().as_nanos();
    let count_values = match &counts {
        MetadataAggregatePlan::ScalarCounts { counts, .. } => counts.clone(),
        _ => return Err("metadata count returned a non-count plan".into()),
    };
    let min_max_values = match &min_max {
        MetadataAggregatePlan::ScalarValues { values, .. } => values.len(),
        _ => return Err("metadata min/max returned a non-value plan".into()),
    };
    Ok(json!({
        "id": "metadata_count_min_max",
        "category": "metadata-only count/min/max",
        "status": "measured",
        "metrics": {
            "planning_ns": planning_ns,
            "scan_ns": 0,
            "end_to_end_ns": planning_ns,
            "rows_materialized": 1,
            "count_values": count_values,
            "min_max_values": min_max_values,
        },
        "proofs": {
            "count": format!("{:?}", counts.proof().kind),
            "min_max": format!("{:?}", min_max.proof().kind),
        },
    }))
}

#[derive(Debug, Default, Clone)]
struct OfflineObjectStoreStats {
    object_gets: u64,
    range_gets: u64,
    bytes_requested: u64,
    bytes_returned: u64,
    cache_hits: u64,
    cache_misses: u64,
    original_ranges: u64,
    coalesced_ranges: u64,
}

#[derive(Debug, Default)]
struct OfflineObjectStoreHarness {
    objects: BTreeMap<String, Vec<u8>>,
    range_cache: BTreeSet<(String, u64, u64)>,
    stats: OfflineObjectStoreStats,
}

impl OfflineObjectStoreHarness {
    fn put_object(&mut self, key: impl Into<String>, bytes: Vec<u8>) {
        self.objects.insert(key.into(), bytes);
    }

    fn get_object(&mut self, key: &str) -> Result<Vec<u8>, String> {
        let bytes = self
            .objects
            .get(key)
            .ok_or_else(|| format!("offline object {key:?} does not exist"))?
            .clone();
        self.stats.object_gets = self.stats.object_gets.saturating_add(1);
        self.stats.bytes_requested = self
            .stats
            .bytes_requested
            .saturating_add(bytes.len() as u64);
        self.stats.bytes_returned = self.stats.bytes_returned.saturating_add(bytes.len() as u64);
        Ok(bytes)
    }

    fn range_get(&mut self, key: &str, range: Range<u64>) -> Result<Vec<u8>, String> {
        let bytes = self
            .objects
            .get(key)
            .ok_or_else(|| format!("offline object {key:?} does not exist"))?;
        if range.start > range.end || range.end as usize > bytes.len() {
            return Err(format!(
                "range {}..{} is outside object {key:?} length {}",
                range.start,
                range.end,
                bytes.len()
            ));
        }
        let len = range.end.saturating_sub(range.start);
        self.stats.range_gets = self.stats.range_gets.saturating_add(1);
        self.stats.bytes_requested = self.stats.bytes_requested.saturating_add(len);
        let cache_key = (key.to_string(), range.start, range.end);
        if self.range_cache.insert(cache_key) {
            self.stats.cache_misses = self.stats.cache_misses.saturating_add(1);
            self.stats.bytes_returned = self.stats.bytes_returned.saturating_add(len);
        } else {
            self.stats.cache_hits = self.stats.cache_hits.saturating_add(1);
        }
        Ok(bytes[range.start as usize..range.end as usize].to_vec())
    }

    fn take_stats(&mut self) -> OfflineObjectStoreStats {
        std::mem::take(&mut self.stats)
    }
}

fn deterministic_object_ranges(file_len: u64) -> Vec<Range<u64>> {
    let mut ranges = Vec::new();
    let mut push = |start: u64, end: u64| {
        if start < end
            && !ranges
                .iter()
                .any(|range: &Range<u64>| range.start == start && range.end == end)
        {
            ranges.push(start..end);
        }
    };
    push(0, file_len.min(4096));
    push(4096.min(file_len), file_len.min(8192));
    let middle = file_len / 2;
    push(middle, middle.saturating_add(4096).min(file_len));
    push(file_len.saturating_sub(4096), file_len);
    ranges.sort_by_key(|range| (range.start, range.end));
    ranges
}

fn coalesce_object_ranges(ranges: &[Range<u64>], max_gap: u64, max_span: u64) -> Vec<Range<u64>> {
    let mut sorted = ranges.to_vec();
    sorted.sort_by_key(|range| (range.start, range.end));
    let mut coalesced: Vec<Range<u64>> = Vec::new();
    for range in sorted {
        let Some(last) = coalesced.last_mut() else {
            coalesced.push(range);
            continue;
        };
        let gap = range.start.saturating_sub(last.end);
        let span = range.end.saturating_sub(last.start);
        if range.start <= last.end || (gap <= max_gap && span <= max_span) {
            last.end = last.end.max(range.end);
        } else {
            coalesced.push(range);
        }
    }
    coalesced
}

fn read_harness_ranges(
    harness: &mut OfflineObjectStoreHarness,
    key: &str,
    ranges: &[Range<u64>],
) -> Result<(), String> {
    for range in ranges {
        harness.range_get(key, range.clone())?;
    }
    Ok(())
}

fn object_store_stats_json(stats: &OfflineObjectStoreStats) -> Value {
    json!({
        "object_gets": stats.object_gets,
        "range_gets": stats.range_gets,
        "bytes_requested": stats.bytes_requested,
        "bytes_returned": stats.bytes_returned,
        "cache_hits": stats.cache_hits,
        "cache_misses": stats.cache_misses,
        "original_ranges": stats.original_ranges,
        "coalesced_ranges": stats.coalesced_ranges,
    })
}

fn run_object_store_cold_warm_case(corpus: &Path, path: &Path) -> Result<Value, String> {
    let options = ExplainOptions {
        projection: Some(vec!["id".into(), "amount".into()]),
        filters: vec![FilterDsl {
            column: "amount".into(),
            op: FilterOp::Gte,
            value: Some("1000".into()),
        }],
        ..ExplainOptions::default()
    };
    let cold = run_query_case(
        "object_store_cold_probe",
        "object-store cold probe",
        path,
        options.clone(),
    )?;
    let warm = run_query_case(
        "object_store_warm_probe",
        "object-store warm probe",
        path,
        options,
    )?;
    let events_bytes = fs::read(path).map_err(|err| format!("cannot read events object: {err}"))?;
    let mut harness = OfflineObjectStoreHarness::default();
    harness.put_object("events.cove", events_bytes.clone());
    if let Ok(covm_bytes) = fs::read(corpus.join("events.covm")) {
        harness.put_object("events.covm", covm_bytes);
        let _ = harness.get_object("events.covm")?;
    }
    let original_ranges = deterministic_object_ranges(events_bytes.len() as u64);
    let coalesced_ranges = coalesce_object_ranges(&original_ranges, 1024, 16 * 1024);
    harness.stats.original_ranges = original_ranges.len() as u64;
    harness.stats.coalesced_ranges = coalesced_ranges.len() as u64;
    read_harness_ranges(&mut harness, "events.cove", &coalesced_ranges)?;
    let cold_store = harness.take_stats();
    harness.stats.original_ranges = original_ranges.len() as u64;
    harness.stats.coalesced_ranges = coalesced_ranges.len() as u64;
    read_harness_ranges(&mut harness, "events.cove", &coalesced_ranges)?;
    let warm_store = harness.take_stats();

    let mut coverage_harness = OfflineObjectStoreHarness::default();
    let coverage_bytes = fs::read(corpus.join("synthetic-cache.cove"))
        .map_err(|err| format!("cannot read synthetic-cache object: {err}"))?;
    coverage_harness.put_object("synthetic-cache.cove", coverage_bytes.clone());
    let coverage_ranges = deterministic_object_ranges(coverage_bytes.len() as u64);
    let pruned_ranges: Vec<_> = coverage_ranges.into_iter().take(1).collect();
    coverage_harness.stats.original_ranges = 4;
    coverage_harness.stats.coalesced_ranges = pruned_ranges.len() as u64;
    read_harness_ranges(
        &mut coverage_harness,
        "synthetic-cache.cove",
        &pruned_ranges,
    )?;
    let coverage_store = coverage_harness.take_stats();

    Ok(json!({
        "id": "object_store_cold_warm",
        "category": "object-store cold and warm scans",
        "status": "measured",
        "metrics": {
            "planning_ns": case_u64(&cold, "/metrics/planning_ns") + case_u64(&warm, "/metrics/planning_ns"),
            "scan_ns": case_u64(&cold, "/metrics/scan_ns") + case_u64(&warm, "/metrics/scan_ns"),
            "end_to_end_ns": case_u64(&cold, "/metrics/end_to_end_ns") + case_u64(&warm, "/metrics/end_to_end_ns"),
            "rows_materialized": case_u64(&cold, "/metrics/rows_materialized") + case_u64(&warm, "/metrics/rows_materialized"),
            "cold": cold["metrics"].clone(),
            "warm": warm["metrics"].clone(),
            "object_store_requests": cold_store.range_gets + cold_store.object_gets + warm_store.range_gets + warm_store.object_gets,
            "object_store_bytes_requested": cold_store.bytes_requested + warm_store.bytes_requested,
            "object_store_bytes_returned": cold_store.bytes_returned + warm_store.bytes_returned,
        },
        "cost": {
            "cold": cold["cost"].clone(),
            "warm": warm["cost"].clone(),
            "simulation": "offline deterministic object-store harness",
            "object_store_harness": {
                "cold": object_store_stats_json(&cold_store),
                "warm": object_store_stats_json(&warm_store),
                "coverage_pruned": object_store_stats_json(&coverage_store),
                "page_cluster": {
                    "original_ranges": original_ranges.len(),
                    "coalesced_ranges": coalesced_ranges.len(),
                    "request_reduction": original_ranges.len().saturating_sub(coalesced_ranges.len()),
                },
                "caveat": "Hermetic object-store semantics, not live S3 or MinIO performance.",
            },
        },
    }))
}

fn run_cove_map_identity_case(corpus: &Path) -> Result<Value, String> {
    let dir = corpus.join("cove-map-identity");
    fs::create_dir_all(&dir).map_err(|err| format!("cannot create COVE-MAP dir: {err}"))?;
    let map_path = dir.join("people.covemap");
    let csv_path = dir.join("people.csv");
    durable::durable_replace(&map_path, &bench_covemap_bytes()?)
        .map_err(|err| format!("cannot publish COVE-MAP fixture: {err}"))?;
    let mut csv = String::from("id,name\n");
    for row in 0..512 {
        csv.push_str(&format!("{row},person-{row}\n"));
    }
    fs::write(&csv_path, csv).map_err(|err| format!("cannot write COVE-MAP CSV: {err}"))?;
    let start = Instant::now();
    let summary = cove_map::conversion_summary_from_paths(&map_path, &[csv_path])
        .map_err(|err| format!("COVE-MAP identity benchmark failed: {err}"))?;
    let end_to_end_ns = start.elapsed().as_nanos();
    Ok(json!({
        "id": "cove_map_identity",
        "category": "COVE-MAP conversion and identity",
        "status": "measured",
        "metrics": {
            "planning_ns": 0,
            "scan_ns": end_to_end_ns,
            "end_to_end_ns": end_to_end_ns,
            "rows_materialized": summary["materialized_row_count"].as_u64().unwrap_or(0),
            "assertions": summary["assertion_count"].as_u64().unwrap_or(0),
            "evidence_entries": summary["evidence_entry_count"].as_u64().unwrap_or(0),
        },
        "optional_features": ["cove_map"],
    }))
}

fn bench_covemap_bytes() -> Result<Vec<u8>, String> {
    let file = CovemapFile {
        header: CovemapHeaderV1::new([0x77; 16], 0),
        mapping_version: "bench/v1".into(),
        sections: vec![
            covemap_json_section(
                SectionKind::MapSourceCatalog,
                json!({
                    "mapping_id": "bench-map",
                    "mapping_version": "bench/v1",
                    "sources": [{"source_id": "people", "row_identity_rules": ["person_by_id"]}]
                }),
            )?,
            covemap_json_section(
                SectionKind::MapFunctionRegistry,
                json!({
                    "mapping_id": "bench-map",
                    "mapping_version": "bench/v1",
                    "functions": [{"function_id": "identity", "version": "1", "deterministic": true, "dependency": "pure"}]
                }),
            )?,
            covemap_json_section(
                SectionKind::MapIdentityRuleCatalog,
                json!({
                    "mapping_id": "bench-map",
                    "mapping_version": "bench/v1",
                    "identity_rules": [{
                        "rule_id": "person_by_id",
                        "object_type": "Person",
                        "semantic_role": "subject",
                        "confidence_class": "authoritative",
                        "candidate_only": false,
                        "property_conflicts_declared": true,
                        "function_ids": ["identity"],
                        "join_keys": [{
                            "role_id": "person_id",
                            "source_column": "id",
                            "logical_type": "utf8",
                            "canonicalization": "identity",
                            "null_policy": "reject",
                            "ordering": "declared"
                        }]
                    }],
                    "do_not_merge": []
                }),
            )?,
            covemap_json_section(
                SectionKind::MapRowSemanticsCatalog,
                json!({
                    "mapping_id": "bench-map",
                    "mapping_version": "bench/v1",
                    "rules": [{
                        "rule_id": "people_rows",
                        "source_id": "people",
                        "identity_rule_id": "person_by_id",
                        "row_semantics_kind": "Object",
                        "assertion_kinds": ["object", "property", "evidence"],
                        "record_kind": "Baseline",
                        "function_ids": ["identity"],
                        "output_assertion_ids": [],
                        "association_endpoints": [],
                        "property_bindings": [{
                            "assertion_id": "person_name",
                            "property_id": "person_name",
                            "property_name": "name",
                            "source_column": "name",
                            "logical_type": "utf8",
                            "physical_kind": "varbytes",
                            "value_expression": "name",
                            "nullable": false
                        }]
                    }]
                }),
            )?,
        ],
        postscript: cove_core::artifact::covemap::CovemapPostscriptV1 {
            required_features: 0,
            optional_features: 0,
            file_len: 0,
            header_offset: 0,
            header_length: 0,
            checksum: 0,
        },
    };
    file.serialize().map_err(|err| err.to_string())
}

fn covemap_json_section(kind: SectionKind, value: Value) -> Result<CovemapSection, String> {
    let payload =
        serde_json::to_vec(&covemap_payload_value(kind, value)).map_err(|err| err.to_string())?;
    Ok(CovemapSection {
        entry: CovemapSectionEntryV1 {
            section_id: kind as u32,
            offset: 0,
            length: payload.len() as u64,
            uncompressed_length: payload.len() as u64,
            compression: CompressionCodec::None as u8,
            payload_encoding: CovemapPayloadEncodingV2::CoveMapJsonV2 as u8,
            required: true,
            reserved: 0,
            checksum: 0,
        },
        payload,
    })
}

fn covemap_payload_value(kind: SectionKind, mut value: Value) -> Value {
    if let Value::Object(object) = &mut value {
        object.insert(
            "schema_id".to_string(),
            Value::String("org.coveformat.covemap.v2".to_string()),
        );
        object.insert(
            "section_id".to_string(),
            Value::Number((kind as u16).into()),
        );
    }
    value
}

fn attach_covi_sidecar_metrics(
    cost: &mut Value,
    stats: cove_datafusion::dataset_state::DatasetBootstrapStats,
) {
    if let Some(metrics) = cost
        .get_mut("coverage_metrics")
        .and_then(Value::as_object_mut)
    {
        metrics.insert(
            "covi".into(),
            json!({
                "loaded": stats.covi_sidecars_loaded,
                "stale": stats.covi_sidecars_stale,
                "ignored": stats.covi_sidecars_ignored,
                "candidate_pruned": stats.covi_candidate_pruned,
                "index_only_answers": stats.covi_index_only_answers,
            }),
        );
    }
}

fn normalize_case_metrics(case: &mut Value) {
    let Some(object) = case.as_object_mut() else {
        return;
    };
    let cost = object.get("cost").cloned().unwrap_or(Value::Null);
    let metrics = object
        .entry("metrics")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .expect("metrics object");
    let planning = metrics
        .get("planning_ns")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let scan = metrics.get("scan_ns").and_then(Value::as_u64).unwrap_or(0);
    let elapsed = metrics
        .get("end_to_end_ns")
        .and_then(Value::as_u64)
        .unwrap_or(planning.saturating_add(scan));
    metrics.entry("end_to_end_ns").or_insert(json!(elapsed));
    metrics.entry("elapsed_time_ns").or_insert(json!(elapsed));
    let metadata_bytes = cost
        .pointer("/observed/metadata_bytes_read")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let data_bytes = cost
        .pointer("/observed/data_bytes_read")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let bytes_read = metadata_bytes.saturating_add(data_bytes);
    metrics.entry("bytes_read").or_insert(json!(bytes_read));
    let request_count = cost
        .pointer("/observed/range_requests")
        .and_then(Value::as_u64)
        .or_else(|| {
            cost.pointer("/range_plan/original_range_requests")
                .and_then(Value::as_u64)
        })
        .unwrap_or(0);
    metrics
        .entry("request_count")
        .or_insert(json!(request_count));
    metrics.entry("fragments_visited").or_insert(json!(cost
        .pointer("/observed/scan_tasks")
        .and_then(Value::as_u64)
        .unwrap_or(0)));
    metrics.entry("pages_visited").or_insert(json!(cost
        .pointer("/observed/pages_decoded")
        .and_then(Value::as_u64)
        .unwrap_or(0)));
    let considered = cost
        .pointer("/observed/morsels_considered")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let pruned = cost
        .pointer("/observed/morsels_pruned")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    metrics
        .entry("pruning_tightness")
        .or_insert(json!(if considered == 0 {
            0.0
        } else {
            pruned as f64 / considered as f64
        }));
    metrics.entry("coverage_cache").or_insert_with(|| {
        cost.pointer("/coverage_metrics/coverage_cache")
            .cloned()
            .unwrap_or(json!({
                "hits": 0,
                "misses": 0,
                "entries_loaded": 0,
            }))
    });
    metrics.entry("coverage_cache_hit").or_insert(json!(cost
        .pointer("/coverage_metrics/coverage_cache/hits")
        .and_then(Value::as_u64)
        .unwrap_or(0)));
    metrics.entry("coverage_cache_miss").or_insert(json!(cost
        .pointer("/coverage_metrics/coverage_cache/misses")
        .and_then(Value::as_u64)
        .unwrap_or(0)));
    metrics.entry("index_use").or_insert(json!({
        "covi_used": cost.pointer("/coverage_metrics/covi_used").and_then(Value::as_bool).unwrap_or(false),
        "lookup_hits": cost.pointer("/observed/lookup_index_hits").and_then(Value::as_u64).unwrap_or(0),
        "lookup_misses": cost.pointer("/observed/lookup_index_misses").and_then(Value::as_u64).unwrap_or(0),
        "index_fallbacks": cost.pointer("/observed/index_fallbacks").and_then(Value::as_u64).unwrap_or(0),
    }));
    metrics.entry("memory_peak_bytes").or_insert(Value::Null);
    let artifact_sizes = json!({
        "cove_bytes": metrics.get("cove_bytes").and_then(Value::as_u64).unwrap_or(0),
        "parquet_bytes": metrics.get("parquet_bytes").and_then(Value::as_u64).unwrap_or(0),
        "orc_bytes": metrics.get("orc_bytes").and_then(Value::as_u64).unwrap_or(0),
        "covx_bytes": metrics.get("covx_bytes").and_then(Value::as_u64).unwrap_or(0),
    });
    metrics.entry("artifact_sizes").or_insert(artifact_sizes);
}

fn validate_report_cases(cases: &[Value]) -> Result<(), String> {
    let manifest: Value = serde_json::from_str(PUBLIC_MANIFEST).map_err(|err| err.to_string())?;
    if let Some(groups) = manifest.get("query_groups").and_then(Value::as_array) {
        for group in groups.iter().filter_map(Value::as_str) {
            require_measured_case(cases, group)?;
        }
    }
    if let Some(skipped) = cases
        .iter()
        .find(|case| case.get("status").and_then(Value::as_str) == Some("skipped"))
    {
        return Err(format!(
            "benchmark case {} was skipped",
            skipped
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        ));
    }
    let required = [
        "full_numeric_scan",
        "string_category_scan",
        "equality_filter",
        "range_filter",
        "top_n",
        "point_lookup",
        "covi_index_latency",
        "covi_index_only_count",
        "object_store_cold_warm",
        "parquet_conversion_cost",
        "orc_conversion_cost",
        "orc_full_scan_readback",
        "orc_file_size_overhead",
        "coverage_cache_disabled",
        "coverage_cache_hit",
        "coverage_cache_hit_miss_invalidation",
        "filecode_group_by",
        "execution_code_remap_overhead",
        "registered_codec_decode_predicate_kernel",
        "fallback_payload_overhead",
        "page_cluster_range_coalescing",
        "zero_copy_success_fallback_rate",
        "coverage_degree_tightness",
        "tpch_style_queries",
        "tpcds_style_queries",
        "medical_operational_queries",
        "negative_corrupt_validation",
        "canonicalisation_vectors",
        "semantic_mapping_corpus",
    ];
    for id in required {
        if !cases.iter().any(|case| case.get("id") == Some(&json!(id))) {
            return Err(format!("benchmark report missing required case {id}"));
        }
    }
    let required_metric_fields = [
        "elapsed_time_ns",
        "bytes_read",
        "request_count",
        "fragments_visited",
        "pages_visited",
        "pruning_tightness",
        "coverage_cache",
        "index_use",
        "memory_peak_bytes",
        "artifact_sizes",
    ];
    for case in cases {
        let metrics = case
            .get("metrics")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                format!(
                    "benchmark case {} is missing metrics",
                    case.get("id").and_then(Value::as_str).unwrap_or("unknown")
                )
            })?;
        for field in required_metric_fields {
            if !metrics.contains_key(field) {
                return Err(format!(
                    "benchmark case {} missing required metric {field}",
                    case.get("id").and_then(Value::as_str).unwrap_or("unknown")
                ));
            }
        }
    }
    let cache_hit = cases
        .iter()
        .find(|case| case.get("id") == Some(&json!("coverage_cache_hit")))
        .and_then(|case| {
            case.pointer("/cost/coverage_metrics/coverage_cache/hits")
                .and_then(Value::as_u64)
        })
        .unwrap_or(0);
    if cache_hit == 0 {
        return Err("coverage_cache_hit did not record a COVE-CACHE hit".into());
    }
    let covi_lookup = require_measured_case(cases, "covi_index_latency")?;
    if !case_bool(covi_lookup, "/cost/coverage_metrics/covi_used") {
        return Err("covi_index_latency did not use COVI candidates".into());
    }
    if case_u64(covi_lookup, "/cost/coverage_metrics/covi_candidates") == 0 {
        return Err("covi_index_latency did not produce any COVI candidates".into());
    }
    if case_u64(covi_lookup, "/cost/coverage_metrics/covi/loaded") == 0 {
        return Err("covi_index_latency did not load a COVI sidecar".into());
    }

    let covi_count = require_measured_case(cases, "covi_index_only_count")?;
    if case_u64(covi_count, "/cost/coverage_metrics/covi/loaded") == 0 {
        return Err("covi_index_only_count did not load a COVI sidecar".into());
    }
    if case_u64(covi_count, "/cost/coverage_metrics/covi/index_only_answers") == 0 {
        return Err("covi_index_only_count did not record COVI index-only evidence".into());
    }
    if covi_count.pointer("/proof/kind").and_then(Value::as_str) != Some("CoviIndexOnlyCount") {
        return Err("covi_index_only_count did not prove CoviIndexOnlyCount".into());
    }
    Ok(())
}

fn require_measured_case<'a>(cases: &'a [Value], id: &str) -> Result<&'a Value, String> {
    let case = cases
        .iter()
        .find(|case| case.get("id") == Some(&json!(id)))
        .ok_or_else(|| format!("benchmark report missing required case {id}"))?;
    if case.get("status").and_then(Value::as_str) != Some("measured") {
        return Err(format!("{id} was not measured"));
    }
    Ok(case)
}

fn case_u64(case: &Value, pointer: &str) -> u64 {
    case.pointer(pointer).and_then(Value::as_u64).unwrap_or(0)
}

fn case_bool(case: &Value, pointer: &str) -> bool {
    case.pointer(pointer)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn markdown_report(report: &Value) -> String {
    let mut out = String::from("# COVE v2 Public Benchmark Report\n\n");
    out.push_str("| Case | Status | Planning ns | Scan ns | Rows |\n");
    out.push_str("| --- | --- | ---: | ---: | ---: |\n");
    if let Some(cases) = report.get("cases").and_then(Value::as_array) {
        for case in cases {
            let id = case.get("id").and_then(Value::as_str).unwrap_or("unknown");
            let status = case
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let metrics = case.get("metrics").unwrap_or(&Value::Null);
            let planning = metrics
                .get("planning_ns")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let scan = metrics.get("scan_ns").and_then(Value::as_u64).unwrap_or(0);
            let rows = metrics
                .get("rows_materialized")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            out.push_str(&format!(
                "| `{id}` | {status} | {planning} | {scan} | {rows} |\n"
            ));
        }
    }
    out
}

fn environment_report() -> Value {
    json!({
        "os": env::consts::OS,
        "arch": env::consts::ARCH,
        "threads": std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
    })
}

fn events_batch(row_count: usize) -> Result<RecordBatch, String> {
    let mut ids = Vec::with_capacity(row_count);
    let mut amounts = Vec::with_capacity(row_count);
    let mut buckets = Vec::with_capacity(row_count);
    let mut names = Vec::with_capacity(row_count);
    let mut active = Vec::with_capacity(row_count);
    for row in 0..row_count {
        ids.push(row as i64);
        amounts.push(((row * 37) % 10_000) as i64);
        buckets.push(format!("bucket-{:02}", row % 16));
        names.push(match row % 5 {
            0 => "alpha",
            1 => "beta",
            2 => "gamma",
            3 => "delta",
            _ => "omega",
        });
        active.push(row % 3 != 0);
    }
    RecordBatch::try_from_iter(vec![
        ("id", Arc::new(Int64Array::from(ids)) as ArrayRef),
        ("amount", Arc::new(Int64Array::from(amounts)) as ArrayRef),
        ("bucket", Arc::new(StringArray::from(buckets)) as ArrayRef),
        ("name", Arc::new(StringArray::from(names)) as ArrayRef),
        ("active", Arc::new(BooleanArray::from(active)) as ArrayRef),
    ])
    .map_err(|err| err.to_string())
}

fn write_parquet_file(path: &Path, batch: &RecordBatch) -> Result<(), String> {
    let file =
        fs::File::create(path).map_err(|err| format!("cannot create {}: {err}", path.display()))?;
    let properties = WriterProperties::builder().build();
    let mut writer = ArrowWriter::try_new(file, batch.schema(), Some(properties))
        .map_err(|err| err.to_string())?;
    writer.write(batch).map_err(|err| err.to_string())?;
    writer.close().map_err(|err| err.to_string())?;
    Ok(())
}

fn write_orc_file(path: &Path, batch: &RecordBatch) -> Result<(), String> {
    let file =
        fs::File::create(path).map_err(|err| format!("cannot create {}: {err}", path.display()))?;
    let mut writer = OrcWriterBuilder::new(file, batch.schema())
        .try_build()
        .map_err(|err| format!("cannot open ORC writer: {err}"))?;
    writer
        .write(batch)
        .map_err(|err| format!("cannot write ORC batch: {err}"))?;
    writer
        .close()
        .map_err(|err| format!("cannot finish ORC writer: {err}"))?;
    Ok(())
}

fn validate_orc_parity(path: &Path, batch: &RecordBatch) -> Result<(), String> {
    let file =
        fs::File::open(path).map_err(|err| format!("cannot open {}: {err}", path.display()))?;
    let builder = OrcReaderBuilder::try_new(file)
        .map_err(|err| format!("cannot read generated ORC {}: {err}", path.display()))?;
    if builder.schema().fields().len() != batch.schema().fields().len() {
        return Err("generated ORC schema column count does not match source batch".into());
    }
    let rows = builder
        .with_batch_size(4096)
        .build()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("cannot read generated ORC batches: {err}"))?
        .iter()
        .map(|batch| batch.num_rows())
        .sum::<usize>();
    if rows != batch.num_rows() {
        return Err(format!(
            "generated ORC row count {rows} does not match source batch {}",
            batch.num_rows()
        ));
    }
    Ok(())
}

struct CoverageCacheFixture {
    cove_bytes: Vec<u8>,
    cache_bytes: Vec<u8>,
}

fn coverage_cache_fixture() -> Result<CoverageCacheFixture, String> {
    let cove_bytes = primitive_events_file_with_name_gamma_coverage(false);
    let state = cove_datafusion::bootstrap::bootstrap_bytes("synthetic-cache", cove_bytes.clone())
        .map_err(|err| err.to_string())?;
    let mut seed = Vec::with_capacity(28);
    seed.extend_from_slice(state.file_id());
    seed.extend_from_slice(&state.file_len().to_le_bytes());
    seed.extend_from_slice(&state.footer_crc32c().to_le_bytes());
    let digest = compute_digest(DigestAlgorithm::Sha256, &seed).map_err(|err| err.to_string())?;
    let mut snapshot_id = [0u8; 16];
    snapshot_id.copy_from_slice(&digest[..16]);
    let dataset_id = *state.file_id();
    let cache = CoverageCacheV2 {
        header: CoveCoverageCacheHeaderV2 {
            cache_format_namespace_ref: 1,
            cache_format_version_major: 2,
            cache_format_version_minor: 0,
            flags: 0,
            cache_id: [7u8; 16],
            dataset_id,
            snapshot_id,
            entry_count: 1,
            created_at_us: 0,
            producer_engine_ref: 0,
            reserved: [0; 32],
            checksum: 0,
        },
        entries: vec![CoverageCacheEntryV2 {
            entry_id: 1,
            dataset_id,
            snapshot_id,
            predicate_normal_form_ref: 1,
            interval_normal_form_ref: u32::MAX,
            coverage_set_ref: 1,
            coverage_granularity: CoverageGranularityV2::Morsel,
            proof_strength: CoverageProofStrengthV2::ExactConservative,
            exactness: CoverageExactnessV2::Exact,
            flags: 0,
            actual_coverage_size_bytes: 64,
            actual_read_cost_ns: 1,
            created_at_us: 0,
            valid_until_snapshot_ref: u32::MAX,
            producer_engine_ref: 0,
            checksum: 0,
        }],
    };
    Ok(CoverageCacheFixture {
        cove_bytes,
        cache_bytes: cache.serialize().map_err(|err| err.to_string())?,
    })
}

fn primitive_events_file_with_name_gamma_coverage(bad_checksum: bool) -> Vec<u8> {
    let mut writer = primitive_events_writer();
    for section in name_gamma_coverage_sections(bad_checksum) {
        writer.push_extra_section(section);
    }
    writer.write().unwrap()
}

fn primitive_events_writer() -> ScanProfileCoveWriter {
    let catalog = TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 1,
            namespace: "public".into(),
            name: "events".into(),
            row_count: 3,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: vec![
                column(1, "id", CoveLogicalType::Int64, CovePhysicalKind::NumCode),
                column(2, "name", CoveLogicalType::Utf8, CovePhysicalKind::VarBytes),
                column(
                    3,
                    "active",
                    CoveLogicalType::Bool,
                    CovePhysicalKind::Boolean,
                ),
            ],
        }],
    };
    let mut first = ScanSegment::new(1, 0, 0, 2, 3);
    first.set_column_pages(1, vec![numcode_page(2, numcode_i64(&[1, 2]))]);
    first.set_column_pages(2, vec![varbytes_page(2, varbytes(&["alpha", "beta"]))]);
    first.set_column_pages(3, vec![bool_page(2, bools(&[true, false]))]);

    let mut second = ScanSegment::new(1, 1, 2, 1, 3);
    second.set_column_pages(1, vec![numcode_page(1, numcode_i64(&[3]))]);
    second.set_column_pages(2, vec![varbytes_page(1, varbytes(&["gamma"]))]);
    second.set_column_pages(3, vec![bool_page(1, bools(&[true]))]);

    let mut writer = ScanProfileCoveWriter::new(catalog);
    writer.push_segment(first);
    writer.push_segment(second);
    writer
}

fn column(
    column_id: u32,
    name: &str,
    logical: CoveLogicalType,
    physical: CovePhysicalKind,
) -> ColumnEntry {
    ColumnEntry {
        column_id,
        name: name.into(),
        logical,
        physical,
        nullable: false,
        sort_order: 0,
        collation_id: 0,
        precision: 0,
        scale: 0,
        flags: 0,
    }
}

fn name_gamma_coverage_sections(bad_checksum: bool) -> Vec<SectionPayload> {
    let predicate_form_ref = 1;
    let provider_id = 1;
    let coverage_set_id = 1;
    let snapshot_validity_ref = 1;
    let predicate_form_section =
        predicate_normal_form_ast_section(predicate_form_ref, 1, name_eq_gamma_ast_payload());

    let provider = CoverageProviderDescriptorV2 {
        provider_id,
        provider_kind: CoverageProofKindV2::ValueToFragmentIndex as u16,
        profile: PrimaryProfile::CoverageMetadata as u8,
        granularity: CoverageGranularityV2::Morsel,
        proof_strength: CoverageProofStrengthV2::ExactConservative,
        exactness: CoverageExactnessV2::Exact,
        flags: 0,
        referenced_table_id: 1,
        referenced_column_id: 2,
        referenced_path_ref: u32::MAX,
        logical_type: CoveLogicalType::Utf8 as u16,
        collation_id: 0,
        null_semantics: 0,
        snapshot_validity_ref,
        predicate_form_ref,
        producer_ref: u32::MAX,
        checksum: 0,
    };
    let coverage_set = CoverageSetV2 {
        header: CoverageSetHeaderV2 {
            coverage_set_id,
            provider_id,
            granularity: CoverageGranularityV2::Morsel,
            proof_strength: CoverageProofStrengthV2::ExactConservative,
            exactness: CoverageExactnessV2::Exact,
            flags: 0,
            predicate_form_ref,
            snapshot_validity_ref,
            total_fragment_count: 2,
            covered_fragment_count: 0,
            required_fragment_count_estimate: 0,
            coverage_degree_ppm: 500_000,
            tightness_degree_ppm: 1_000_000,
            entries_offset: 0,
            entries_length: 0,
            checksum: 0,
        },
        entries: vec![CoverageSetEntryV2 {
            target_kind: CoverageGranularityV2::Morsel,
            flags: 0,
            file_ref: 0,
            table_id: 1,
            segment_id: 1,
            morsel_id: 0,
            page_ref: u32::MAX,
            object_type_id: u32::MAX,
            path_ref: u32::MAX,
            dimensional_bucket_ref: u32::MAX,
            row_start: 0,
            row_count: 0,
            row_ordinal_bitmap_ref: u32::MAX,
            byte_range_ref: u32::MAX,
            checksum: 0,
        }],
    };
    let coverage_set_bytes = coverage_set.serialize().unwrap();
    let mut coverage_set_checksum = coverage_set_payload_checksum(&coverage_set_bytes);
    if bad_checksum {
        coverage_set_checksum ^= 1;
    }
    let proof = CoverageProofRecordV2 {
        proof_id: 1,
        provider_id,
        coverage_set_id,
        predicate_form_ref,
        proof_kind: CoverageProofKindV2::ValueToFragmentIndex,
        proof_strength: CoverageProofStrengthV2::ExactConservative,
        exactness: CoverageExactnessV2::Exact,
        granularity: CoverageGranularityV2::Morsel,
        null_semantics: 0,
        flags: 0,
        snapshot_validity_ref,
        coverage_set_checksum,
        proof_payload_ref: u32::MAX,
        checksum: 0,
    };

    vec![
        coverage_section(
            SectionKind::CoverageProviderRegistry,
            1,
            provider.serialize().to_vec(),
        ),
        coverage_section(SectionKind::CoverageSet, 1, coverage_set_bytes),
        coverage_section(
            SectionKind::CoverageProofRecord,
            1,
            proof.serialize().unwrap().to_vec(),
        ),
        predicate_form_section,
    ]
}

fn predicate_normal_form_ast_section(
    predicate_form_id: u32,
    table_id: u32,
    payload: Vec<u8>,
) -> SectionPayload {
    let form = PredicateNormalFormV2 {
        predicate_form_id,
        form_kind: PredicateFormKindV2::PredicateAst,
        flags: 0,
        logical_context_ref: table_id,
        payload_offset: PredicateNormalFormV2::LEN as u64,
        payload_length: payload.len() as u64,
        checksum: 0,
    };
    let mut data = Vec::with_capacity(PredicateNormalFormV2::LEN + payload.len());
    data.extend_from_slice(&form.serialize().unwrap());
    data.extend_from_slice(&payload);
    coverage_section(SectionKind::PredicateNormalForm, 1, data)
}

fn name_eq_gamma_ast_payload() -> Vec<u8> {
    let canonical = CanonicalValue::Utf8("gamma").encode().unwrap();
    let node_offset = PredicateAstPayloadHeaderV2::LEN;
    let literal_offset = node_offset + PredicateAstNodeV2::LEN;
    let operand_ref_offset = literal_offset + PredicateLiteralV2::LEN;
    let canonical_offset = operand_ref_offset + 2 * PredicateAstOperandRefV2::LEN;

    let mut payload = Vec::new();
    payload.extend_from_slice(&predicate_ast_header(
        node_offset as u64,
        literal_offset as u64,
        operand_ref_offset as u64,
    ));
    payload.extend_from_slice(&predicate_ast_node());
    payload.extend_from_slice(&predicate_ast_literal(
        canonical_offset as u64,
        canonical.len() as u32,
    ));
    payload.extend_from_slice(&predicate_ast_operand_ref(
        0,
        PredicateOperandKindV2::ColumnOrPath,
        2,
    ));
    payload.extend_from_slice(&predicate_ast_operand_ref(
        1,
        PredicateOperandKindV2::Literal,
        0,
    ));
    payload.extend_from_slice(&canonical);
    payload
}

fn predicate_ast_header(
    node_offset: u64,
    literal_offset: u64,
    operand_ref_offset: u64,
) -> [u8; PredicateAstPayloadHeaderV2::LEN] {
    let mut out = [0u8; PredicateAstPayloadHeaderV2::LEN];
    out[0..4].copy_from_slice(&0u32.to_le_bytes());
    out[4..8].copy_from_slice(&1u32.to_le_bytes());
    out[8..12].copy_from_slice(&1u32.to_le_bytes());
    out[20..24].copy_from_slice(&2u32.to_le_bytes());
    out[24..32].copy_from_slice(&node_offset.to_le_bytes());
    out[32..40].copy_from_slice(&literal_offset.to_le_bytes());
    out[56..64].copy_from_slice(&operand_ref_offset.to_le_bytes());
    let crc = checksum::crc32c(&out);
    out[68..72].copy_from_slice(&crc.to_le_bytes());
    out
}

fn predicate_ast_node() -> [u8; PredicateAstNodeV2::LEN] {
    let mut out = [0u8; PredicateAstNodeV2::LEN];
    out[0..4].copy_from_slice(&0u32.to_le_bytes());
    out[4..6].copy_from_slice(&(PredicateOpV2::Eq as u16).to_le_bytes());
    out[8..10].copy_from_slice(&(CoveLogicalType::Bool as u16).to_le_bytes());
    out[12] = PredicateNullPolicyV2::SqlWhere as u8;
    out[14..16].copy_from_slice(&2u16.to_le_bytes());
    out[16..20].copy_from_slice(&0u32.to_le_bytes());
    out[20..24].copy_from_slice(&2u32.to_le_bytes());
    out[24..28].copy_from_slice(&0u32.to_le_bytes());
    out[28..32].copy_from_slice(&u32::MAX.to_le_bytes());
    out[32..36].copy_from_slice(&u32::MAX.to_le_bytes());
    let crc = checksum::crc32c(&out);
    out[36..40].copy_from_slice(&crc.to_le_bytes());
    out
}

fn predicate_ast_literal(
    canonical_value_offset: u64,
    canonical_value_length: u32,
) -> [u8; PredicateLiteralV2::LEN] {
    let mut out = [0u8; PredicateLiteralV2::LEN];
    out[0..4].copy_from_slice(&0u32.to_le_bytes());
    out[4..6].copy_from_slice(&(ValueTag::Utf8 as u16).to_le_bytes());
    out[6..8].copy_from_slice(&(CoveLogicalType::Utf8 as u16).to_le_bytes());
    out[12..20].copy_from_slice(&canonical_value_offset.to_le_bytes());
    out[20..24].copy_from_slice(&canonical_value_length.to_le_bytes());
    let crc = checksum::crc32c(&out);
    out[24..28].copy_from_slice(&crc.to_le_bytes());
    out
}

fn predicate_ast_operand_ref(
    ordinal: u16,
    operand_kind: PredicateOperandKindV2,
    ref_id: u32,
) -> [u8; PredicateAstOperandRefV2::LEN] {
    let mut out = [0u8; PredicateAstOperandRefV2::LEN];
    out[0..4].copy_from_slice(&0u32.to_le_bytes());
    out[4..6].copy_from_slice(&ordinal.to_le_bytes());
    out[6] = operand_kind as u8;
    out[8..12].copy_from_slice(&ref_id.to_le_bytes());
    let crc = checksum::crc32c(&out);
    out[12..16].copy_from_slice(&crc.to_le_bytes());
    out
}

fn coverage_section(kind: SectionKind, item_count: u64, data: Vec<u8>) -> SectionPayload {
    SectionPayload {
        section_kind: kind as u16,
        profile: PrimaryProfile::CoverageMetadata as u8,
        flags: 0,
        item_count,
        row_count: 0,
        compression: 0,
        alignment_log2: 0,
        required_features: 0,
        optional_features: 0,
        data,
    }
}

fn numcode_page(row_count: u32, payload: Vec<u8>) -> ScanPageSpec {
    ScanPageSpec::new(row_count, payload).with_encoding_root(CoveEncodingKind::NumCode as u32)
}

fn varbytes_page(row_count: u32, payload: Vec<u8>) -> ScanPageSpec {
    ScanPageSpec::new(row_count, payload).with_encoding_root(CoveEncodingKind::VarBytes as u32)
}

fn bool_page(row_count: u32, payload: Vec<u8>) -> ScanPageSpec {
    ScanPageSpec::new(row_count, payload).with_encoding_root(CoveEncodingKind::PlainFixed as u32)
}

fn numcode_i64(values: &[i64]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| (*value as u64).to_le_bytes())
        .collect()
}

fn varbytes(values: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    for value in values {
        out.extend_from_slice(&(value.len() as u32).to_le_bytes());
        out.extend_from_slice(value.as_bytes());
    }
    out
}

fn bools(values: &[bool]) -> Vec<u8> {
    values.iter().map(|value| u8::from(*value)).collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[allow(dead_code)]
fn _schema_for_docs() -> Schema {
    Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("amount", DataType::Int64, false),
        Field::new("bucket", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("active", DataType::Boolean, false),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_manifest_declares_required_groups() {
        let manifest: Value = serde_json::from_str(PUBLIC_MANIFEST).unwrap();
        let groups = manifest
            .get("query_groups")
            .and_then(Value::as_array)
            .unwrap();
        for required in [
            "full_numeric_scan",
            "parquet_conversion_cost",
            "covi_index_latency",
            "covi_index_only_count",
            "object_store_cold_warm",
            "coverage_cache_hit_miss_invalidation",
            "tpch_style_queries",
            "tpcds_style_queries",
            "medical_operational_queries",
            "negative_corrupt_validation",
            "canonicalisation_vectors",
            "semantic_mapping_corpus",
        ] {
            assert!(groups.iter().any(|group| group.as_str() == Some(required)));
        }
    }

    #[test]
    fn offline_object_store_harness_records_cache_and_coalescing() {
        let mut harness = OfflineObjectStoreHarness::default();
        harness.put_object("object", vec![0u8; 32_768]);
        let original = deterministic_object_ranges(32_768);
        let coalesced = coalesce_object_ranges(&original, 1024, 16 * 1024);
        assert!(coalesced.len() <= original.len());
        read_harness_ranges(&mut harness, "object", &coalesced).unwrap();
        let cold = harness.take_stats();
        assert_eq!(cold.cache_misses, coalesced.len() as u64);
        read_harness_ranges(&mut harness, "object", &coalesced).unwrap();
        let warm = harness.take_stats();
        assert_eq!(warm.cache_hits, coalesced.len() as u64);
        assert_eq!(warm.bytes_returned, 0);
    }

    #[test]
    fn synthetic_cache_fixture_records_planner_hit() {
        let fixture = coverage_cache_fixture().unwrap();
        let dir = env::temp_dir().join(format!("cove-bench-cache-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let cove_path = dir.join("synthetic-cache.cove");
        let cache_path = dir.join("synthetic-cache.cove.cache");
        fs::write(&cove_path, fixture.cove_bytes).unwrap();
        fs::write(&cache_path, fixture.cache_bytes).unwrap();

        let case = run_query_case(
            "coverage_cache_hit",
            "COVE-CACHE hit",
            &cove_path,
            ExplainOptions {
                filters: vec![FilterDsl {
                    column: "name".into(),
                    op: FilterOp::Eq,
                    value: Some("gamma".into()),
                }],
                table_options: CoveTableOptions::default().with_sibling_coverage_cache(),
                ..ExplainOptions::default()
            },
        )
        .unwrap();
        let hits = case
            .pointer("/cost/coverage_metrics/coverage_cache/hits")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        assert!(hits > 0);
        let _ = fs::remove_file(cove_path);
        let _ = fs::remove_file(cache_path);
        let _ = fs::remove_dir(dir);
    }
}
