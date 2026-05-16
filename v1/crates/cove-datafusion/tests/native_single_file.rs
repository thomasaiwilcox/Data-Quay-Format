use std::{
    fmt, fs,
    ops::Range,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use async_trait::async_trait;
#[cfg(all(feature = "covm", feature = "covx"))]
use cove_core::artifact::covx::{CovxFile, CovxHeaderV1, CovxPostscriptV1, CovxReferencedFileV1};
#[cfg(feature = "covm")]
use cove_core::{
    artifact::covm::{CovmFile, CovmFileEntryV1, CovmHeaderV1, CovmPostscriptV1},
    constants::DigestAlgorithm,
};
use cove_core::{
    constants::{
        CoveEncodingKind, CoveLogicalType, CovePhysicalKind, PrimaryProfile, SectionKind,
        StorageClass, ValueTag, FEATURE_ENGINE_PROFILE, FEATURE_REDACTIONS,
    },
    dictionary::{FileDictionary, FileDictionaryHeaderV1, FileDictionaryIndexEntryV1},
    domain::ColumnDomain,
    index::{
        aggregate::{AggregateEntry, AggregateSynopsis, SynopsisAccuracy, SynopsisKind},
        composite::{
            CompositeIndex, CompositeTransformKind, CompositeZoneIndexHeaderV1,
            COMPOSITE_ZONE_INDEX_HEADER_LEN,
        },
        inverted::{
            InvertedEntry, InvertedKeyKind, InvertedMorselIndex, InvertedMorselIndexHeaderV1,
        },
        lookup::{
            LookupEntry, LookupIndex, LookupIndexHeaderV1, LookupIndexKind, LookupKeyKind,
            LookupUniqueness,
        },
        topn::{TopNDirection, TopNSummary, TOPN_ZONE_SUMMARY_LEN},
    },
    profile::cove_e::{
        EngineMountPolicyV1, EngineProfileEntryV1, EngineProfileRegistry,
        ExecutionCodeCanonicality, ExecutionCodeComparisonScope, ExecutionCodeDescriptorV1,
        ExecutionCodeKind, ExecutionCodeLifetime, FileCodeMappingKind, MissingValuePolicy,
        NullCodePolicy, ReverseLookupPolicy, StaleMappingPolicy,
    },
    redaction::{RedactionEntry, RedactionManifest},
    row_ref::RowRef,
    table::{ColumnEntry, TableCatalog, TableEntry},
    wire,
    writer::{ScanPageSpec, ScanProfileCoveWriter, ScanSegment, SectionPayload},
    zone_stats::{ZoneStatFlags, ZoneStats, ZoneStatsEntry, ZoneStatsSection},
};
#[cfg(feature = "covm")]
use cove_datafusion::register::{cove_table_from_covm_path, register_cove_covm};
use cove_datafusion::{
    bootstrap::{bootstrap_bytes, bootstrap_local_file, bootstrap_local_file_async},
    decode::{decode_local_dataset_scan_tasks, decode_scan},
    expr_lowering::{lower_filter, LowerExpr, LowerLiteral, LowerOperator},
    overlay::{CoveOverlaySnapshot, OverlayFile, OverlayFileIdentity, RowRange, RowVisibility},
    planner::{
        plan_scan, CovePredicate, FilterPlan, NullPredicateKind, NumericPredicateOp,
        PredicateLiteral,
    },
    range_reader::{coalesced_range_count, RangeCoalescingOptions},
    register::{
        cove_table_from_path, cove_table_from_path_async, register_cove_file,
        register_cove_file_async, register_cove_file_format, register_cove_file_with_options,
        register_cove_listing_table, register_cove_listing_table_with_options,
        register_cove_overlay_snapshot, CoveTableOptions, ExecutionCodePolicy,
    },
    task_graph::build_task_graph,
};
use datafusion::object_store::{
    memory::InMemory, path::Path, CopyOptions, GetOptions, GetResult, ListResult, MultipartUpload,
    ObjectMeta, ObjectStore, PutMultipartOptions, PutOptions, PutPayload, PutResult,
};
use datafusion::{
    arrow::{
        array::{
            Array, BinaryArray, BinaryViewArray, DictionaryArray, StringArray, StringViewArray,
        },
        datatypes::UInt32Type,
        util::pretty::pretty_format_batches,
    },
    assert_batches_eq,
    catalog::TableProvider,
    common::{stats::Precision, Column, ScalarValue},
    logical_expr::{BinaryExpr, Expr, Operator, TableProviderFilterPushDown},
    physical_plan::{execution_plan::collect as collect_physical_plan, ExecutionPlan},
    prelude::SessionContext,
};
use futures::stream::BoxStream;
use url::Url;

static NEXT_FILE_ID: AtomicU64 = AtomicU64::new(0);

async fn collect_sql_with_cove_metric(
    ctx: &SessionContext,
    sql: &str,
    metric_name: &str,
) -> (Vec<datafusion::arrow::record_batch::RecordBatch>, usize) {
    let dataframe = ctx.sql(sql).await.unwrap();
    let plan = dataframe.create_physical_plan().await.unwrap();
    let batches = collect_physical_plan(Arc::clone(&plan), ctx.task_ctx())
        .await
        .unwrap();
    (batches, execution_plan_metric_sum(&plan, metric_name))
}

fn execution_plan_metric_sum(plan: &Arc<dyn ExecutionPlan>, metric_name: &str) -> usize {
    let own = plan
        .metrics()
        .and_then(|metrics| metrics.sum_by_name(metric_name))
        .map(|metric| metric.as_usize())
        .unwrap_or(0);
    own + plan
        .children()
        .into_iter()
        .map(|child| execution_plan_metric_sum(child, metric_name))
        .sum::<usize>()
}

#[tokio::test]
async fn select_star_reads_single_file_multi_segment() {
    let path = write_temp_cove("events", primitive_events_file());
    let ctx = SessionContext::new();
    register_cove_file(&ctx, "events", &path).unwrap();

    let batches = ctx
        .sql("SELECT * FROM events")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    let expected = [
        "+----+-------+--------+",
        "| id | name  | active |",
        "+----+-------+--------+",
        "| 1  | alpha | true   |",
        "| 2  | beta  | false  |",
        "| 3  | gamma | true   |",
        "+----+-------+--------+",
    ];
    assert_batches_eq!(expected, &batches);
    fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn native_limit_pushdown_materializes_only_requested_rows() {
    let path = write_temp_cove("events_limit", primitive_events_file());
    let ctx = SessionContext::new();
    register_cove_file(&ctx, "events", &path).unwrap();

    let (full_batches, full_materialized) =
        collect_sql_with_cove_metric(&ctx, "SELECT id FROM events", "cove_rows_materialized").await;
    let full_expected = [
        "+----+", "| id |", "+----+", "| 1  |", "| 2  |", "| 3  |", "+----+",
    ];
    assert_batches_eq!(full_expected, &full_batches);
    assert_eq!(full_materialized, 3);
    let (_, full_buffered_partitions) = collect_sql_with_cove_metric(
        &ctx,
        "SELECT id FROM events",
        "cove_materialization_buffered_partitions",
    )
    .await;
    assert_eq!(full_buffered_partitions, 1);

    let (limit_batches, limit_materialized) = collect_sql_with_cove_metric(
        &ctx,
        "SELECT id FROM events LIMIT 1",
        "cove_rows_materialized",
    )
    .await;
    let limit_expected = ["+----+", "| id |", "+----+", "| 1  |", "+----+"];
    assert_batches_eq!(limit_expected, &limit_batches);
    assert_eq!(limit_materialized, 1);
    assert!(limit_materialized < full_materialized);
    let (_, limit_streaming_partitions) = collect_sql_with_cove_metric(
        &ctx,
        "SELECT id FROM events LIMIT 1",
        "cove_materialization_streaming_partitions",
    )
    .await;
    assert_eq!(limit_streaming_partitions, 1);

    fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn native_arrow_export_path_metrics_are_recorded() {
    let path = write_temp_cove("events_export_metrics", primitive_events_file());
    let ctx = SessionContext::new();
    register_cove_file(&ctx, "events", &path).unwrap();

    let (_, numcode_rows) = collect_sql_with_cove_metric(
        &ctx,
        "SELECT id, name, active FROM events",
        "cove_arrow_export_direct_numcode_rows",
    )
    .await;
    let (_, varbytes_rows) = collect_sql_with_cove_metric(
        &ctx,
        "SELECT id, name, active FROM events",
        "cove_arrow_export_direct_varbytes_rows",
    )
    .await;
    let (_, plainfixed_rows) = collect_sql_with_cove_metric(
        &ctx,
        "SELECT id, name, active FROM events",
        "cove_arrow_export_direct_plainfixed_rows",
    )
    .await;
    let (_, fallback_rows) = collect_sql_with_cove_metric(
        &ctx,
        "SELECT id, name, active FROM events",
        "cove_arrow_export_fallback_rows",
    )
    .await;

    assert_eq!(numcode_rows, 3);
    assert_eq!(varbytes_rows, 3);
    assert_eq!(plainfixed_rows, 3);
    assert_eq!(fallback_rows, 0);
    fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn native_materialization_mode_selection_is_explained() {
    let path = write_temp_cove("materialization_modes", topn_events_file());
    let ctx = SessionContext::new();
    register_cove_file(&ctx, "events", &path).unwrap();

    let explain = ctx
        .sql("EXPLAIN SELECT id FROM events")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let explain_text = pretty_format_batches(&explain).unwrap().to_string();
    assert!(
        explain_text.contains("materialization_mode=buffered"),
        "{explain_text}"
    );

    let explain = ctx
        .sql("EXPLAIN SELECT id FROM events LIMIT 1")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let explain_text = pretty_format_batches(&explain).unwrap().to_string();
    assert!(
        explain_text.contains("materialization_mode=streaming"),
        "{explain_text}"
    );

    let explain = ctx
        .sql("EXPLAIN SELECT id FROM events ORDER BY id DESC LIMIT 1")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let explain_text = pretty_format_batches(&explain).unwrap().to_string();
    assert!(explain_text.contains("topn_hint=Some"), "{explain_text}");
    assert!(
        explain_text.contains("materialization_mode=streaming"),
        "{explain_text}"
    );

    fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn async_bootstrap_and_registration_match_sync_helpers() {
    let path = write_temp_cove("async_parity", primitive_events_file());

    let sync_state = bootstrap_local_file(&path).unwrap();
    let async_state = bootstrap_local_file_async(&path).await.unwrap();
    assert_eq!(sync_state.table().row_count, async_state.table().row_count);
    assert_eq!(sync_state.schema().as_ref(), async_state.schema().as_ref());

    let sync_provider = cove_table_from_path(&path).unwrap();
    let async_provider = cove_table_from_path_async(&path).await.unwrap();
    assert_eq!(
        sync_provider.state().bootstrap_stats(),
        async_provider.state().bootstrap_stats()
    );

    let ctx = SessionContext::new();
    register_cove_file(&ctx, "events_sync", &path).unwrap();
    register_cove_file_async(&ctx, "events_async", &path)
        .await
        .unwrap();

    let sync_batches = ctx
        .sql("SELECT COUNT(*) AS rows FROM events_sync")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let async_batches = ctx
        .sql("SELECT COUNT(*) AS rows FROM events_async")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let expected = ["+------+", "| rows |", "+------+", "| 3    |", "+------+"];
    assert_batches_eq!(expected, &sync_batches);
    assert_batches_eq!(expected, &async_batches);

    fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn listing_registration_reads_multiple_cove_files() {
    let dir = make_temp_dir("listing_multi");
    fs::write(dir.join("part1.cove"), primitive_events_file()).unwrap();
    fs::write(dir.join("part2.cove"), primitive_events_file()).unwrap();

    let ctx = SessionContext::new();
    register_cove_listing_table(&ctx, "events", dir.to_str().unwrap())
        .await
        .unwrap();

    let batches = ctx
        .sql("SELECT id, name FROM events ORDER BY id, name")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    let expected = [
        "+----+-------+",
        "| id | name  |",
        "+----+-------+",
        "| 1  | alpha |",
        "| 1  | alpha |",
        "| 2  | beta  |",
        "| 2  | beta  |",
        "| 3  | gamma |",
        "| 3  | gamma |",
        "+----+-------+",
    ];
    assert_batches_eq!(expected, &batches);
    fs::remove_dir_all(dir).unwrap();
}

#[cfg(feature = "covm")]
#[tokio::test]
async fn covm_registration_reads_multiple_relative_files() {
    let dir = make_temp_dir("covm_multi");
    let nested = dir.join("nested");
    fs::create_dir_all(&nested).unwrap();
    let first = dir.join("part1.cove");
    let second = nested.join("part2.cove");
    fs::write(&first, primitive_events_file()).unwrap();
    fs::write(&second, primitive_events_file()).unwrap();
    let manifest = dir.join("dataset.covm");
    write_covm_manifest(
        &manifest,
        vec![
            covm_entry_for_path("part1.cove", &first),
            covm_entry_for_path("nested/part2.cove", &second),
        ],
    );

    let ctx = SessionContext::new();
    let provider = register_cove_covm(&ctx, "events", &manifest).unwrap();
    assert_eq!(provider.state().file_count(), 2);
    assert_eq!(provider.state().bootstrap_stats().files_validated, 2);

    let batches = ctx
        .sql("SELECT id, name FROM events ORDER BY id, name")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let expected = [
        "+----+-------+",
        "| id | name  |",
        "+----+-------+",
        "| 1  | alpha |",
        "| 1  | alpha |",
        "| 2  | beta  |",
        "| 2  | beta  |",
        "| 3  | gamma |",
        "| 3  | gamma |",
        "+----+-------+",
    ];
    assert_batches_eq!(expected, &batches);
    fs::remove_dir_all(dir).unwrap();
}

#[cfg(feature = "covm")]
#[tokio::test]
async fn covm_rejects_schema_mismatch() {
    let dir = make_temp_dir("covm_schema_mismatch");
    let first = dir.join("part1.cove");
    let second = dir.join("part2.cove");
    fs::write(&first, primitive_events_file()).unwrap();
    fs::write(&second, nullable_events_file()).unwrap();
    let manifest = dir.join("dataset.covm");
    write_covm_manifest(
        &manifest,
        vec![
            covm_entry_for_path("part1.cove", &first),
            covm_entry_for_path("part2.cove", &second),
        ],
    );

    let err = cove_table_from_covm_path(&manifest)
        .unwrap_err()
        .to_string();
    assert!(err.contains("schema mismatch"), "{err}");
    fs::remove_dir_all(dir).unwrap();
}

#[cfg(feature = "covm")]
#[tokio::test]
async fn stale_covm_entry_cannot_exclude_file() {
    let dir = make_temp_dir("covm_stale");
    let path = dir.join("part1.cove");
    fs::write(&path, primitive_events_file()).unwrap();
    let mut entry = covm_entry_for_path("part1.cove", &path);
    entry.footer_crc32c ^= 0x55AA_0011;
    let manifest = dir.join("dataset.covm");
    write_covm_manifest(&manifest, vec![entry]);

    let ctx = SessionContext::new();
    let provider = register_cove_covm(&ctx, "events", &manifest).unwrap();
    assert_eq!(provider.state().bootstrap_stats().covm_entries_stale, 1);
    let batches = ctx
        .sql("SELECT COUNT(*) AS rows FROM events")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let expected = ["+------+", "| rows |", "+------+", "| 3    |", "+------+"];
    assert_batches_eq!(expected, &batches);
    fs::remove_dir_all(dir).unwrap();
}

#[cfg(feature = "covm")]
#[tokio::test]
async fn covm_filecode_filters_resolve_per_file_dictionary() {
    let dir = make_temp_dir("covm_filecode");
    let first = dir.join("part1.cove");
    let second = dir.join("part2.cove");
    fs::write(&first, dictionary_items_file(sample_dictionary())).unwrap();
    fs::write(&second, dictionary_items_file(swapped_dictionary())).unwrap();
    let manifest = dir.join("dataset.covm");
    write_covm_manifest(
        &manifest,
        vec![
            covm_entry_for_path("part1.cove", &first),
            covm_entry_for_path("part2.cove", &second),
        ],
    );

    let ctx = SessionContext::new();
    register_cove_covm(&ctx, "items", &manifest).unwrap();
    let batches = ctx
        .sql("SELECT name FROM items WHERE name = 'red' ORDER BY name")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let expected = [
        "+------+", "| name |", "+------+", "| red  |", "| red  |", "+------+",
    ];
    assert_batches_eq!(expected, &batches);
    fs::remove_dir_all(dir).unwrap();
}

#[cfg(all(feature = "covm", feature = "covx"))]
#[tokio::test]
async fn covx_sibling_sidecar_validation_is_advisory() {
    let dir = make_temp_dir("covx_sidecar");
    let path = dir.join("part1.cove");
    fs::write(&path, primitive_events_file()).unwrap();
    let manifest = dir.join("dataset.covm");
    write_covm_manifest(&manifest, vec![covm_entry_for_path("part1.cove", &path)]);
    write_covx_sidecar(
        &PathBuf::from(format!("{}.covx", path.display())),
        vec![covx_entry_for_path(&path)],
    );

    let provider = cove_table_from_covm_path(&manifest).unwrap();
    assert_eq!(provider.state().bootstrap_stats().covx_sidecars_loaded, 1);

    let mut stale = covx_entry_for_path(&path);
    stale.file_len += 1;
    write_covx_sidecar(
        &PathBuf::from(format!("{}.covx", path.display())),
        vec![stale],
    );
    let ctx = SessionContext::new();
    let provider = register_cove_covm(&ctx, "events", &manifest).unwrap();
    assert_eq!(provider.state().bootstrap_stats().covx_sidecars_stale, 1);
    let batches = ctx
        .sql("SELECT COUNT(*) AS rows FROM events")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let expected = ["+------+", "| rows |", "+------+", "| 3    |", "+------+"];
    assert_batches_eq!(expected, &batches);
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn sql_external_table_stored_as_cove_works_after_format_registration() {
    let dir = make_temp_dir("sql_external");
    fs::write(dir.join("part1.cove"), primitive_events_file()).unwrap();

    let ctx = SessionContext::new();
    register_cove_file_format(&ctx).unwrap();
    ctx.sql(&format!(
        "CREATE EXTERNAL TABLE events STORED AS COVE LOCATION '{}'",
        dir.display()
    ))
    .await
    .unwrap();

    let batches = ctx
        .sql("SELECT name FROM events")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let expected = [
        "+-------+",
        "| name  |",
        "+-------+",
        "| alpha |",
        "| beta  |",
        "| gamma |",
        "+-------+",
    ];
    assert_batches_eq!(expected, &batches);
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn sql_external_table_rejects_cove_format_options() {
    let dir = make_temp_dir("sql_external_options");
    fs::write(dir.join("part1.cove"), primitive_events_file()).unwrap();

    let ctx = SessionContext::new();
    register_cove_file_format(&ctx).unwrap();
    let err = ctx
        .sql(&format!(
            "CREATE EXTERNAL TABLE events STORED AS COVE LOCATION '{}' OPTIONS ('cove.foo' 'bar')",
            dir.display()
        ))
        .await
        .unwrap_err()
        .to_string();

    assert!(
        err.contains("COVE DataFusion M2 does not support SQL format options"),
        "{err}"
    );
    assert!(err.contains("cove.foo"), "{err}");
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn listing_registration_rejects_schema_mismatch_and_empty_listing() {
    let mismatch = make_temp_dir("listing_mismatch");
    fs::write(mismatch.join("part1.cove"), primitive_events_file()).unwrap();
    fs::write(mismatch.join("part2.cove"), nullable_events_file()).unwrap();
    let ctx = SessionContext::new();
    let err = register_cove_listing_table(&ctx, "events", mismatch.to_str().unwrap())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("schema mismatch"), "{err}");
    fs::remove_dir_all(mismatch).unwrap();

    let empty = make_temp_dir("listing_empty");
    let err = register_cove_listing_table(&ctx, "empty_events", empty.to_str().unwrap())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("empty listing"), "{err}");
    fs::remove_dir_all(empty).unwrap();
}

#[tokio::test]
async fn listing_registration_rejects_multiple_tables_in_one_file() {
    let dir = make_temp_dir("listing_multi_table");
    fs::write(dir.join("bad.cove"), multiple_tables_file()).unwrap();
    let ctx = SessionContext::new();
    let err = register_cove_listing_table(&ctx, "bad", dir.to_str().unwrap())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("exactly one table"), "{err}");
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn compatibility_filters_are_residual_and_correct() {
    let dir = make_temp_dir("listing_filters");
    fs::write(dir.join("part1.cove"), nullable_events_file()).unwrap();
    let ctx = SessionContext::new();
    register_cove_listing_table(&ctx, "events", dir.to_str().unwrap())
        .await
        .unwrap();

    let batches = ctx
        .sql("SELECT id FROM events WHERE maybe IS NULL ORDER BY id")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let expected = ["+----+", "| id |", "+----+", "| 2  |", "| 3  |", "+----+"];
    assert_batches_eq!(expected, &batches);

    let explain = ctx
        .sql("EXPLAIN SELECT id FROM events WHERE maybe IS NULL")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let explain_text = pretty_format_batches(&explain).unwrap().to_string();
    assert!(explain_text.contains("FilterExec") || explain_text.contains("Filter"));
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn compatibility_uses_range_reads_and_projection_reads_fewer_bytes() {
    let projected = query_counting_store("SELECT name FROM events").await;
    let full = query_counting_store("SELECT * FROM events").await;

    assert_eq!(projected.full_gets, 0);
    assert_eq!(full.full_gets, 0);
    assert!(projected.range_gets > 0);
    assert!(full.range_gets > 0);
    assert!(
        projected.bytes_returned < full.bytes_returned,
        "projected={} full={}",
        projected.bytes_returned,
        full.bytes_returned
    );
}

#[tokio::test]
async fn compatibility_dictionary_output_is_option_aware() {
    let dir = make_temp_dir("listing_dictionary");
    fs::write(
        dir.join("part1.cove"),
        dictionary_items_file(sample_dictionary()),
    )
    .unwrap();
    fs::write(
        dir.join("part2.cove"),
        dictionary_items_file(swapped_dictionary()),
    )
    .unwrap();
    let ctx = SessionContext::new();
    register_cove_listing_table_with_options(
        &ctx,
        "items",
        dir.to_str().unwrap(),
        CoveTableOptions::default().with_arrow_dictionary_output(),
    )
    .await
    .unwrap();

    let batches = ctx
        .sql("SELECT name FROM items")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert!(batches.iter().all(|batch| batch
        .column(0)
        .as_any()
        .downcast_ref::<DictionaryArray<UInt32Type>>()
        .is_some()));

    let filtered = ctx
        .sql("SELECT name FROM items WHERE name = 'red' ORDER BY name")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let filtered_expected = [
        "+------+", "| name |", "+------+", "| red  |", "| red  |", "+------+",
    ];
    assert_batches_eq!(filtered_expected, &filtered);

    let grouped = ctx
        .sql("SELECT name, COUNT(*) AS n FROM items GROUP BY name ORDER BY name")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let grouped_expected = [
        "+------+---+",
        "| name | n |",
        "+------+---+",
        "| blue | 2 |",
        "| red  | 2 |",
        "+------+---+",
    ];
    assert_batches_eq!(grouped_expected, &grouped);

    let ordered = ctx
        .sql("SELECT name FROM items ORDER BY name")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let ordered_expected = [
        "+------+", "| name |", "+------+", "| blue |", "| blue |", "| red  |", "| red  |",
        "+------+",
    ];
    assert_batches_eq!(ordered_expected, &ordered);
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn select_projected_column_returns_only_projection() {
    let path = write_temp_cove("events_projection", primitive_events_file());
    let ctx = SessionContext::new();
    register_cove_file(&ctx, "events", &path).unwrap();

    let batches = ctx
        .sql("SELECT name FROM events")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    let expected = [
        "+-------+",
        "| name  |",
        "+-------+",
        "| alpha |",
        "| beta  |",
        "| gamma |",
        "+-------+",
    ];
    assert_batches_eq!(expected, &batches);
    assert!(batches.iter().all(|batch| batch.num_columns() == 1));
    fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn arrow_view_output_returns_view_arrays_and_preserves_values() {
    let path = write_temp_cove("arrow_view_output", primitive_events_file());
    let ctx = SessionContext::new();
    register_cove_file_with_options(
        &ctx,
        "events",
        &path,
        CoveTableOptions::default().with_arrow_view_output(),
    )
    .unwrap();

    let batches = ctx
        .sql("SELECT name FROM events WHERE name >= 'beta'")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(
        batches[0].schema().field(0).data_type(),
        &datafusion::arrow::datatypes::DataType::Utf8View
    );
    let names = batches
        .iter()
        .flat_map(|batch| {
            let array = batch
                .column(0)
                .as_any()
                .downcast_ref::<StringViewArray>()
                .unwrap();
            (0..array.len())
                .map(|row| array.value(row).to_string())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["beta", "gamma"]);
    fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn arrow_view_output_returns_binary_view_arrays() {
    let path = write_temp_cove("arrow_binary_view_output", binary_events_file());
    let ctx = SessionContext::new();
    register_cove_file_with_options(
        &ctx,
        "events",
        &path,
        CoveTableOptions::default().with_arrow_view_output(),
    )
    .unwrap();

    let batches = ctx
        .sql("SELECT payload FROM events")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(
        batches[0].schema().field(0).data_type(),
        &datafusion::arrow::datatypes::DataType::BinaryView
    );
    let payloads = batches
        .iter()
        .flat_map(|batch| {
            let array = batch
                .column(0)
                .as_any()
                .downcast_ref::<BinaryViewArray>()
                .unwrap();
            (0..array.len())
                .map(|row| array.value(row).to_vec())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        payloads,
        vec![b"short".to_vec(), b"long-binary-payload".to_vec()]
    );
    fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn arrow_view_output_supports_sort_group_and_topn() {
    let path = write_temp_cove("arrow_view_sort_group", primitive_events_file());
    let ctx = SessionContext::new();
    register_cove_file_with_options(
        &ctx,
        "events",
        &path,
        CoveTableOptions::default().with_arrow_view_output(),
    )
    .unwrap();

    let sorted = ctx
        .sql("SELECT name FROM events ORDER BY name DESC LIMIT 2")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(
        sorted[0].schema().field(0).data_type(),
        &datafusion::arrow::datatypes::DataType::Utf8View
    );
    let expected_sorted = [
        "+-------+",
        "| name  |",
        "+-------+",
        "| gamma |",
        "| beta  |",
        "+-------+",
    ];
    assert_batches_eq!(expected_sorted, &sorted);

    let grouped = ctx
        .sql("SELECT name, COUNT(*) AS n FROM events GROUP BY name ORDER BY name")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let expected_grouped = [
        "+-------+---+",
        "| name  | n |",
        "+-------+---+",
        "| alpha | 1 |",
        "| beta  | 1 |",
        "| gamma | 1 |",
        "+-------+---+",
    ];
    assert_batches_eq!(expected_grouped, &grouped);
    fs::remove_file(path).unwrap();
}

#[test]
fn decode_projection_pushdown_decodes_fewer_pages() {
    let state = bootstrap_bytes("events", primitive_events_file()).unwrap();
    let full_plan = plan_scan(&state, None, Vec::new()).unwrap();
    let projection = vec![1];
    let projected_plan = plan_scan(&state, Some(&projection), Vec::new()).unwrap();

    let full = decode_scan(&state, &full_plan).unwrap();
    let projected = decode_scan(&state, &projected_plan).unwrap();

    assert_eq!(full.stats.pages_decoded, 6);
    assert_eq!(projected.stats.pages_decoded, 2);
    assert_eq!(projected_plan.scan_projection, vec![1]);
    assert!(projected
        .batches
        .iter()
        .all(|batch| batch.num_columns() == 1));
}

#[test]
fn m6_task_graph_partitions_follow_target_morsel_option() {
    let state = cove_datafusion::bootstrap::bootstrap_bytes_with_options(
        "events",
        primitive_events_file(),
        CoveTableOptions::default().with_target_morsels_per_partition(1),
    )
    .unwrap();
    let plan = plan_scan(&state, None, Vec::new()).unwrap();
    let graph = build_task_graph(&state, &plan).unwrap();

    assert_eq!(graph.tasks.len(), 2);
    assert_eq!(graph.partitions.len(), 2);
    assert!(graph
        .partitions
        .iter()
        .all(|partition| partition.tasks.len() == 1));
}

#[tokio::test]
async fn m6_partitioned_native_scan_preserves_results_under_sort() {
    let path = write_temp_cove("m6_partitioned", primitive_events_file());
    let ctx = SessionContext::new();
    register_cove_file_with_options(
        &ctx,
        "events",
        &path,
        CoveTableOptions::default().with_target_morsels_per_partition(1),
    )
    .unwrap();

    let batches = ctx
        .sql("SELECT id, name FROM events ORDER BY id")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    let expected = [
        "+----+-------+",
        "| id | name  |",
        "+----+-------+",
        "| 1  | alpha |",
        "| 2  | beta  |",
        "| 3  | gamma |",
        "+----+-------+",
    ];
    assert_batches_eq!(expected, &batches);
    fs::remove_file(path).unwrap();
}

#[test]
fn m6_range_coalescing_thresholds_are_configurable() {
    let ranges = vec![0..8, 16..24, 4096..4104];
    let default_count = coalesced_range_count(&ranges, RangeCoalescingOptions::default()).unwrap();
    let tight_count = coalesced_range_count(
        &ranges,
        RangeCoalescingOptions {
            max_gap: 0,
            max_span: 1024 * 1024,
        },
    )
    .unwrap();

    assert_eq!(default_count, 1);
    assert_eq!(tight_count, 3);
}

#[tokio::test]
async fn projection_order_and_exact_filter_are_correct() {
    let path = write_temp_cove("events_filter", primitive_events_file());
    let ctx = SessionContext::new();
    register_cove_file(&ctx, "events", &path).unwrap();

    let batches = ctx
        .sql("SELECT name, id FROM events WHERE id > 1")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    let expected = [
        "+-------+----+",
        "| name  | id |",
        "+-------+----+",
        "| beta  | 2  |",
        "| gamma | 3  |",
        "+-------+----+",
    ];
    assert_batches_eq!(expected, &batches);

    let explain = ctx
        .sql("EXPLAIN SELECT name, id FROM events WHERE id > 1")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let explain_text = pretty_format_batches(&explain).unwrap().to_string();
    assert!(explain_text.contains("CoveExec"));
    assert!(!explain_text.contains("FilterExec"));
    assert!(explain_text.contains("scan_program="));
    assert!(explain_text.contains("exact_filters=1"));
    fs::remove_file(path).unwrap();
}

#[test]
fn filter_pushdown_classifies_supported_numeric_exact_and_null_inexact() {
    let path = write_temp_cove("nullable_classification", nullable_events_file());
    let provider = cove_table_from_path(&path).unwrap();
    let nullable_col = Expr::Column(Column::from_name("maybe"));
    let is_null = Expr::IsNull(Box::new(nullable_col.clone()));
    let is_not_null = Expr::IsNotNull(Box::new(nullable_col));
    let comparison = Expr::BinaryExpr(BinaryExpr::new(
        Box::new(Expr::Column(Column::from_name("id"))),
        Operator::Gt,
        Box::new(Expr::Literal(ScalarValue::Int64(Some(1)), None)),
    ));

    let support = provider
        .supports_filters_pushdown(&[&is_null, &is_not_null, &comparison])
        .unwrap();

    assert_eq!(
        support,
        vec![
            TableProviderFilterPushDown::Inexact,
            TableProviderFilterPushDown::Inexact,
            TableProviderFilterPushDown::Exact
        ]
    );
    assert!(support.contains(&TableProviderFilterPushDown::Exact));
    fs::remove_file(path).unwrap();
}

#[test]
fn filter_pushdown_classifies_varbytes_equality_exact() {
    let path = write_temp_cove("varbytes_classification", primitive_events_file());
    let provider = cove_table_from_path(&path).unwrap();
    let varbytes_equality = Expr::BinaryExpr(BinaryExpr::new(
        Box::new(Expr::Column(Column::from_name("name"))),
        Operator::Eq,
        Box::new(Expr::Literal(ScalarValue::Utf8(Some("beta".into())), None)),
    ));
    let varbytes_range = Expr::BinaryExpr(BinaryExpr::new(
        Box::new(Expr::Column(Column::from_name("name"))),
        Operator::GtEq,
        Box::new(Expr::Literal(ScalarValue::Utf8(Some("beta".into())), None)),
    ));

    let support = provider
        .supports_filters_pushdown(&[&varbytes_equality, &varbytes_range])
        .unwrap();

    assert_eq!(
        support,
        vec![
            TableProviderFilterPushDown::Exact,
            TableProviderFilterPushDown::Unsupported
        ]
    );
    fs::remove_file(path).unwrap();
}

#[test]
fn null_pruning_uses_page_indexes_without_materializing_predicate_columns() {
    let state = bootstrap_bytes("nullable", nullable_events_file()).unwrap();
    let projection = vec![0];
    let filter = FilterPlan::pruning_null(1, NullPredicateKind::IsNull, "maybe IS NULL");
    let plan = plan_scan(&state, Some(&projection), vec![filter]).unwrap();

    let decoded = decode_scan(&state, &plan).unwrap();

    assert_eq!(decoded.stats.predicate_pages_checked, 3);
    assert_eq!(decoded.stats.morsels_pruned, 1);
    assert_eq!(decoded.stats.pages_decoded, 2);
    assert!(decoded.batches.iter().all(|batch| batch.num_columns() == 1));
}

#[test]
fn numeric_row_selection_late_materializes_projected_columns() {
    let state = bootstrap_bytes("events", primitive_events_file()).unwrap();
    let full = decode_scan(&state, &plan_scan(&state, None, Vec::new()).unwrap()).unwrap();
    let projection = vec![1];
    let filter = FilterPlan::pruning_numeric(
        0,
        NumericPredicateOp::Gt,
        PredicateLiteral::Int64(2),
        "id > 2",
    );
    let plan = plan_scan(&state, Some(&projection), vec![filter]).unwrap();

    let decoded = decode_scan(&state, &plan).unwrap();

    let expected = [
        "+-------+",
        "| name  |",
        "+-------+",
        "| gamma |",
        "+-------+",
    ];
    assert_batches_eq!(expected, &decoded.batches);
    assert_eq!(decoded.stats.rows_selected, 1);
    assert_eq!(decoded.stats.rows_materialized, 1);
    assert!(decoded.stats.pages_decoded < full.stats.pages_decoded);
}

#[test]
fn varbytes_equality_late_materializes_projected_columns() {
    let state = bootstrap_bytes("events", primitive_events_file()).unwrap();
    let full = decode_scan(&state, &plan_scan(&state, None, Vec::new()).unwrap()).unwrap();
    let projection = vec![0];
    let filter = lower_filter(
        &state,
        &LowerExpr::Binary {
            left: Box::new(LowerExpr::Column("name".into())),
            op: LowerOperator::Eq,
            right: Box::new(LowerExpr::Literal(LowerLiteral::Utf8("beta".into()))),
        },
        "name = 'beta'",
    );
    assert!(matches!(
        filter.predicate,
        Some(CovePredicate::VarBytesEq { .. })
    ));
    let plan = plan_scan(&state, Some(&projection), vec![filter]).unwrap();

    let decoded = decode_scan(&state, &plan).unwrap();

    let expected = ["+----+", "| id |", "+----+", "| 2  |", "+----+"];
    assert_batches_eq!(expected, &decoded.batches);
    assert_eq!(decoded.stats.rows_selected, 1);
    assert_eq!(decoded.stats.rows_materialized, 1);
    assert_eq!(decoded.stats.residual_predicates, 0);
    assert_eq!(decoded.stats.exact_predicates, 1);
    assert!(decoded.stats.pages_decoded < full.stats.pages_decoded);
}

#[tokio::test]
async fn varbytes_equality_filter_is_pushed_down_exactly() {
    let path = write_temp_cove("events_varbytes_filter", primitive_events_file());
    let ctx = SessionContext::new();
    register_cove_file(&ctx, "events", &path).unwrap();

    let batches = ctx
        .sql("SELECT id FROM events WHERE name = 'beta'")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let expected = ["+----+", "| id |", "+----+", "| 2  |", "+----+"];
    assert_batches_eq!(expected, &batches);

    let explain = ctx
        .sql("EXPLAIN SELECT id FROM events WHERE name = 'beta'")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let explain_text = pretty_format_batches(&explain).unwrap().to_string();
    assert!(explain_text.contains("CoveExec"), "{explain_text}");
    assert!(!explain_text.contains("FilterExec"), "{explain_text}");
    assert!(explain_text.contains("exact_filters=1"), "{explain_text}");
    fs::remove_file(path).unwrap();
}

#[test]
fn absent_filecode_literal_selects_no_rows_without_page_decode() {
    let state = bootstrap_bytes("items", dictionary_items_file(sample_dictionary())).unwrap();
    let projection = vec![0];
    let filter = FilterPlan::pruning_file_code_in(0, Vec::new(), "name = 'green'");
    let plan = plan_scan(&state, Some(&projection), vec![filter]).unwrap();

    let decoded = decode_scan(&state, &plan).unwrap();

    assert!(decoded.batches.is_empty());
    assert_eq!(decoded.stats.pages_decoded, 0);
    assert_eq!(decoded.stats.rows_selected, 0);
    assert_eq!(decoded.stats.rows_materialized, 0);
}

#[test]
fn direct_decode_resolves_canonical_filecode_filters_for_single_file_state() {
    let state = bootstrap_bytes("items", dictionary_items_file_with_lookup_index()).unwrap();
    let projection = vec![1];
    let filter = lower_filter(
        &state,
        &LowerExpr::Binary {
            left: Box::new(LowerExpr::Column("name".into())),
            op: LowerOperator::Eq,
            right: Box::new(LowerExpr::Literal(LowerLiteral::Utf8("red".into()))),
        },
        "name = 'red'",
    );
    match filter.predicate.as_ref() {
        Some(CovePredicate::FileCodeIn {
            file_codes,
            canonical_values,
            ..
        }) => {
            assert!(file_codes.is_empty());
            assert_eq!(canonical_values.len(), 1);
        }
        other => panic!("expected FileCode predicate, got {other:?}"),
    }
    let plan = plan_scan(&state, Some(&projection), vec![filter]).unwrap();

    let decoded = decode_scan(&state, &plan).unwrap();

    let expected = [
        "+---------+",
        "| payload |",
        "+---------+",
        "| first   |",
        "+---------+",
    ];
    assert_batches_eq!(expected, &decoded.batches);
    assert_eq!(decoded.stats.lookup_index_hits, 1);
    assert_eq!(decoded.stats.index_rows_selected, 1);
}

#[test]
fn task_graph_execution_resolves_canonical_filecode_filters() {
    let state = bootstrap_bytes("items", dictionary_items_file_with_lookup_index()).unwrap();
    let projection = vec![1];
    let filter = lower_filter(
        &state,
        &LowerExpr::InList {
            expr: Box::new(LowerExpr::Column("name".into())),
            list: vec![LowerExpr::Literal(LowerLiteral::Utf8("red".into()))],
            negated: false,
        },
        "name IN ('red')",
    );
    let plan = plan_scan(&state, Some(&projection), vec![filter]).unwrap();
    let graph = build_task_graph(&state, &plan).unwrap();

    let decoded =
        decode_local_dataset_scan_tasks(&state, &plan, &graph.tasks, 0, graph.partitions.len())
            .unwrap();

    let expected = [
        "+---------+",
        "| payload |",
        "+---------+",
        "| first   |",
        "+---------+",
    ];
    assert_batches_eq!(expected, &decoded.batches);
    assert!(!graph.tasks.is_empty());
    assert_eq!(graph.tasks.len(), 1);
    assert_eq!(graph.tasks[0].row_selection.as_deref(), Some(&[0][..]));
    assert_eq!(decoded.stats.lookup_index_hits, 0);
    assert_eq!(decoded.stats.lookup_rowref_tasks, 1);
    assert_eq!(decoded.stats.selection_bitsets, 1);
}

#[test]
fn filecode_domain_pruning_skips_non_matching_morsels() {
    let state = bootstrap_bytes("items", dictionary_items_file_with_domain_stats()).unwrap();
    let full = decode_scan(&state, &plan_scan(&state, None, Vec::new()).unwrap()).unwrap();
    let projection = vec![1];
    let filter = FilterPlan::pruning_file_code_in(0, vec![0], "name = 'red'");
    let plan = plan_scan(&state, Some(&projection), vec![filter]).unwrap();

    let decoded = decode_scan(&state, &plan).unwrap();

    let expected = [
        "+---------+",
        "| payload |",
        "+---------+",
        "| first   |",
        "+---------+",
    ];
    assert_batches_eq!(expected, &decoded.batches);
    assert_eq!(decoded.stats.morsels_considered, 2);
    assert_eq!(decoded.stats.morsels_pruned, 1);
    assert_eq!(decoded.stats.rows_selected, 1);
    assert!(decoded.stats.pages_decoded < full.stats.pages_decoded);
}

#[test]
fn lookup_filecode_equality_selects_rows_before_predicate_page_decode() {
    let state = bootstrap_bytes("items", dictionary_items_file_with_lookup_index()).unwrap();
    let projection = vec![1];
    let filter = FilterPlan::pruning_file_code_in(0, vec![0], "name = 'red'");
    let plan = plan_scan(&state, Some(&projection), vec![filter]).unwrap();

    let decoded = decode_scan(&state, &plan).unwrap();

    let expected = [
        "+---------+",
        "| payload |",
        "+---------+",
        "| first   |",
        "+---------+",
    ];
    assert_batches_eq!(expected, &decoded.batches);
    assert_eq!(decoded.stats.lookup_index_hits, 1);
    assert_eq!(decoded.stats.index_rows_selected, 1);
    assert_eq!(decoded.stats.pages_decoded, 1);
}

#[test]
fn absent_lookup_key_prunes_without_page_decode() {
    let state = bootstrap_bytes("items", dictionary_items_file_with_lookup_index()).unwrap();
    let projection = vec![1];
    let filter = FilterPlan::pruning_file_code_in(0, vec![42], "name = 'green'");
    let plan = plan_scan(&state, Some(&projection), vec![filter]).unwrap();

    let decoded = decode_scan(&state, &plan).unwrap();

    assert!(decoded.batches.is_empty());
    assert_eq!(decoded.stats.pages_decoded, 0);
    assert_eq!(decoded.stats.morsels_pruned, 1);
}

#[test]
fn inverted_filecode_in_prunes_morsels_before_decode() {
    let state = bootstrap_bytes("items", dictionary_items_file_with_inverted_index()).unwrap();
    let projection = vec![1];
    let filter = FilterPlan::pruning_file_code_in(0, vec![0], "name IN ('red')");
    let plan = plan_scan(&state, Some(&projection), vec![filter]).unwrap();

    let decoded = decode_scan(&state, &plan).unwrap();

    let expected = [
        "+---------+",
        "| payload |",
        "+---------+",
        "| first   |",
        "+---------+",
    ];
    assert_batches_eq!(expected, &decoded.batches);
    assert_eq!(decoded.stats.morsels_considered, 2);
    assert_eq!(decoded.stats.morsels_pruned, 1);
}

#[test]
fn inverted_index_uses_file_global_morsel_ordinals() {
    let state = bootstrap_bytes(
        "items",
        dictionary_items_file_with_ambiguous_inverted_index(),
    )
    .unwrap();
    let projection = vec![1];
    let filter = FilterPlan::pruning_file_code_in(0, vec![1], "name IN ('blue')");
    let plan = plan_scan(&state, Some(&projection), vec![filter]).unwrap();

    let decoded = decode_scan(&state, &plan).unwrap();

    let expected = [
        "+---------+",
        "| payload |",
        "+---------+",
        "| second  |",
        "+---------+",
    ];
    assert_batches_eq!(expected, &decoded.batches);
    assert_eq!(decoded.stats.morsels_considered, 2);
    assert_eq!(decoded.stats.morsels_pruned, 1);
    assert_eq!(decoded.stats.pages_decoded, 2);
}

#[test]
fn lookup_numcode_equality_uses_exact_key_conversion() {
    let state = bootstrap_bytes("events", numeric_lookup_events_file()).unwrap();
    let projection = vec![1];
    let filter = FilterPlan::pruning_numeric(
        0,
        NumericPredicateOp::Eq,
        PredicateLiteral::Int64(2),
        "id = 2",
    );
    let plan = plan_scan(&state, Some(&projection), vec![filter]).unwrap();

    let decoded = decode_scan(&state, &plan).unwrap();

    let expected = [
        "+---------+",
        "| payload |",
        "+---------+",
        "| beta    |",
        "+---------+",
    ];
    assert_batches_eq!(expected, &decoded.batches);
    assert_eq!(decoded.stats.lookup_index_hits, 1);
    assert_eq!(decoded.stats.pages_decoded, 1);
}

#[tokio::test]
async fn inexact_null_filters_remain_residual_and_correct() {
    let path = write_temp_cove("nullable_residual", nullable_events_file());
    let ctx = SessionContext::new();
    register_cove_file(&ctx, "events", &path).unwrap();

    let is_null = ctx
        .sql("SELECT id FROM events WHERE maybe IS NULL ORDER BY id")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let expected_null = ["+----+", "| id |", "+----+", "| 2  |", "| 3  |", "+----+"];
    assert_batches_eq!(expected_null, &is_null);

    let is_not_null = ctx
        .sql("SELECT id FROM events WHERE maybe IS NOT NULL ORDER BY id")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let expected_not_null = ["+----+", "| id |", "+----+", "| 1  |", "| 4  |", "+----+"];
    assert_batches_eq!(expected_not_null, &is_not_null);

    let explain = ctx
        .sql("EXPLAIN SELECT id FROM events WHERE maybe IS NULL")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let explain_text = pretty_format_batches(&explain).unwrap().to_string();
    assert!(explain_text.contains("CoveExec"));
    assert!(explain_text.contains("FilterExec") || explain_text.contains("Filter"));
    fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn explain_select_star_mentions_cove_exec() {
    let path = write_temp_cove("events_explain", primitive_events_file());
    let ctx = SessionContext::new();
    register_cove_file(&ctx, "events", &path).unwrap();

    let batches = ctx
        .sql("EXPLAIN SELECT * FROM events")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let explain_text = pretty_format_batches(&batches).unwrap().to_string();

    assert!(explain_text.contains("CoveExec"));
    fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn filecode_dictionary_values_are_decoded() {
    let path = write_temp_cove("items", dictionary_items_file(sample_dictionary()));
    let ctx = SessionContext::new();
    register_cove_file(&ctx, "items", &path).unwrap();

    let (batches, decoded_fallback_rows) = collect_sql_with_cove_metric(
        &ctx,
        "SELECT name FROM items",
        "cove_filecode_dictionary_decoded_fallback_rows",
    )
    .await;

    let expected = [
        "+------+", "| name |", "+------+", "| red  |", "| blue |", "+------+",
    ];
    assert_batches_eq!(expected, &batches);
    assert_eq!(decoded_fallback_rows, 2);
    fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn filecode_dictionary_output_is_opt_in() {
    let path = write_temp_cove(
        "items_dictionary",
        dictionary_items_file(sample_dictionary()),
    );
    let ctx = SessionContext::new();
    register_cove_file_with_options(
        &ctx,
        "items",
        &path,
        CoveTableOptions::default().with_arrow_dictionary_output(),
    )
    .unwrap();

    let batches = ctx
        .sql("SELECT name FROM items")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    let array = batches[0].column(0);
    assert!(array
        .as_any()
        .downcast_ref::<DictionaryArray<UInt32Type>>()
        .is_some());
    let expected = [
        "+------+", "| name |", "+------+", "| red  |", "| blue |", "+------+",
    ];
    assert_batches_eq!(expected, &batches);

    let filtered = ctx
        .sql("SELECT name FROM items WHERE name = 'red'")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let filtered_expected = ["+------+", "| name |", "+------+", "| red  |", "+------+"];
    assert_batches_eq!(filtered_expected, &filtered);

    let grouped = ctx
        .sql("SELECT name, COUNT(*) AS n FROM items GROUP BY name ORDER BY name")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let grouped_expected = [
        "+------+---+",
        "| name | n |",
        "+------+---+",
        "| blue | 1 |",
        "| red  | 1 |",
        "+------+---+",
    ];
    assert_batches_eq!(grouped_expected, &grouped);
    fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn filecode_dictionary_output_uses_direct_key_export_and_value_cache() {
    let path = write_temp_cove(
        "items_dictionary_metrics",
        dictionary_items_file_with_domain_stats(),
    );
    let ctx = SessionContext::new();
    register_cove_file_with_options(
        &ctx,
        "items",
        &path,
        CoveTableOptions::default().with_arrow_dictionary_output(),
    )
    .unwrap();

    let (batches, key_rows) = collect_sql_with_cove_metric(
        &ctx,
        "SELECT name FROM items",
        "cove_filecode_dictionary_keys_rows",
    )
    .await;
    let expected = [
        "+------+", "| name |", "+------+", "| red  |", "| blue |", "+------+",
    ];
    assert_batches_eq!(expected, &batches);
    assert_eq!(key_rows, 2);

    let (_, value_bytes) = collect_sql_with_cove_metric(
        &ctx,
        "SELECT name FROM items",
        "cove_filecode_dictionary_values_bytes",
    )
    .await;
    let (_, cache_misses) = collect_sql_with_cove_metric(
        &ctx,
        "SELECT name FROM items",
        "cove_filecode_dictionary_value_cache_misses",
    )
    .await;
    let (_, cache_hits) = collect_sql_with_cove_metric(
        &ctx,
        "SELECT name FROM items",
        "cove_filecode_dictionary_value_cache_hits",
    )
    .await;

    assert!(value_bytes > 0);
    assert_eq!(cache_misses, 1);
    assert_eq!(cache_hits, 1);
    fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn filecode_dictionary_output_remaps_mixed_file_dictionary() {
    let path = write_temp_cove("mixed_dictionary", mixed_dictionary_items_file());
    let ctx = SessionContext::new();
    register_cove_file_with_options(
        &ctx,
        "items",
        &path,
        CoveTableOptions::default().with_arrow_dictionary_output(),
    )
    .unwrap();

    let (batches, remapped_rows) = collect_sql_with_cove_metric(
        &ctx,
        "SELECT name FROM items",
        "cove_filecode_dictionary_remapped_rows",
    )
    .await;
    let expected = [
        "+------+", "| name |", "+------+", "| red  |", "| blue |", "+------+",
    ];
    assert_batches_eq!(expected, &batches);
    assert_eq!(remapped_rows, 2);

    let dictionary = batches[0]
        .column(0)
        .as_any()
        .downcast_ref::<DictionaryArray<UInt32Type>>()
        .unwrap();
    assert_eq!(dictionary.keys().value(0), 0);
    assert_eq!(dictionary.keys().value(1), 1);
    let values = dictionary
        .values()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    assert_eq!(values.len(), 2);
    assert_eq!(values.value(0), "red");
    assert_eq!(values.value(1), "blue");

    let blob_batches = ctx
        .sql("SELECT blob FROM items")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let blob_dictionary = blob_batches[0]
        .column(0)
        .as_any()
        .downcast_ref::<DictionaryArray<UInt32Type>>()
        .unwrap();
    let blob_values = blob_dictionary
        .values()
        .as_any()
        .downcast_ref::<BinaryArray>()
        .unwrap();
    assert_eq!(blob_values.len(), 1);
    assert_eq!(blob_values.value(0), &[0xaa, 0xbb]);
    fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn filecode_dictionary_output_ignores_view_values_for_filecode_columns() {
    let path = write_temp_cove(
        "items_dictionary_view_options",
        dictionary_items_file(sample_dictionary()),
    );
    let ctx = SessionContext::new();
    register_cove_file_with_options(
        &ctx,
        "items",
        &path,
        CoveTableOptions::default()
            .with_arrow_dictionary_output()
            .with_arrow_view_output(),
    )
    .unwrap();

    let batches = ctx
        .sql("SELECT name FROM items")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let dictionary = batches[0]
        .column(0)
        .as_any()
        .downcast_ref::<DictionaryArray<UInt32Type>>()
        .unwrap();
    assert!(dictionary
        .values()
        .as_any()
        .downcast_ref::<StringArray>()
        .is_some());
    assert!(dictionary
        .values()
        .as_any()
        .downcast_ref::<StringViewArray>()
        .is_none());
    fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn unrelated_redacted_dictionary_entry_does_not_block_projection() {
    let path = write_temp_cove(
        "redacted_mixed_dictionary",
        redacted_mixed_dictionary_items_file(),
    );
    let ctx = SessionContext::new();
    register_cove_file_with_options(
        &ctx,
        "items",
        &path,
        CoveTableOptions::default().with_arrow_dictionary_output(),
    )
    .unwrap();

    let batches = ctx
        .sql("SELECT name FROM items")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let expected = ["+------+", "| name |", "+------+", "| red  |", "+------+"];
    assert_batches_eq!(expected, &batches);
    fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn filecode_dictionary_output_redacted_values_fail_projection() {
    let path = write_temp_cove(
        "redacted_dictionary",
        dictionary_items_file(redacted_dictionary()),
    );
    let ctx = SessionContext::new();
    register_cove_file_with_options(
        &ctx,
        "items",
        &path,
        CoveTableOptions::default().with_arrow_dictionary_output(),
    )
    .unwrap();

    let err = ctx
        .sql("SELECT name FROM items")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap_err()
        .to_string();

    assert!(err.contains("COVE_E_REDACTION_POLICY"), "{err}");
    fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn redacted_dictionary_value_fails_projection() {
    let path = write_temp_cove("redacted", dictionary_items_file(redacted_dictionary()));
    let ctx = SessionContext::new();
    register_cove_file(&ctx, "items", &path).unwrap();

    let err = ctx
        .sql("SELECT name FROM items")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap_err()
        .to_string();

    assert!(err.contains("COVE_E_REDACTION_POLICY"), "{err}");
    fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn redacted_filecode_count_column_does_not_use_metadata_fast_path() {
    let path = write_temp_cove(
        "redacted_count",
        dictionary_items_file(redacted_dictionary()),
    );
    let ctx = SessionContext::new();
    register_cove_file(&ctx, "items", &path).unwrap();

    let err = ctx
        .sql("SELECT COUNT(name) AS present FROM items")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap_err()
        .to_string();

    assert!(err.contains("COVE_E_REDACTION_POLICY"), "{err}");
    fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn filecode_zero_without_dictionary_is_not_null() {
    let path = write_temp_cove("filecode_zero", filecode_without_dictionary_file());
    let ctx = SessionContext::new();
    register_cove_file(&ctx, "items", &path).unwrap();

    let err = ctx
        .sql("SELECT name FROM items")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap_err()
        .to_string();

    assert!(err.contains("FileCode(0)"), "{err}");
    assert!(!err.to_ascii_lowercase().contains("null"), "{err}");
    fs::remove_file(path).unwrap();
}

#[test]
fn m4d_bootstrap_parses_aggregate_composite_and_topn_metadata() {
    let state = bootstrap_bytes("m4d_metadata", dictionary_items_file_with_m4d_metadata()).unwrap();

    assert_eq!(state.aggregate_entries_for(1).len(), 1);
    assert_eq!(state.composite_indexes().count(), 1);
    assert_eq!(state.topn_for(1).len(), 1);
}

#[tokio::test]
async fn m4d_metadata_count_star_rewrites_to_memtable() {
    let path = write_temp_cove("m4d_count_star", primitive_events_file());
    let ctx = SessionContext::new();
    register_cove_file(&ctx, "events", &path).unwrap();

    let batches = ctx
        .sql("SELECT COUNT(*) AS rows FROM events")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let expected = ["+------+", "| rows |", "+------+", "| 3    |", "+------+"];
    assert_batches_eq!(expected, &batches);

    let explain = ctx
        .sql("EXPLAIN SELECT COUNT(*) AS rows FROM events")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let explain_text = pretty_format_batches(&explain).unwrap().to_string();
    assert!(
        !explain_text.contains("CoveExec"),
        "metadata COUNT should not scan COVE data: {explain_text}"
    );
    fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn m4d_metadata_count_nullable_column_uses_exact_synopsis_only() {
    let exact_path = write_temp_cove(
        "m4d_count_nullable_exact",
        nullable_events_file_with_count(),
    );
    let ctx = SessionContext::new();
    register_cove_file(&ctx, "events", &exact_path).unwrap();

    let batches = ctx
        .sql("SELECT COUNT(maybe) AS present FROM events")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let expected = [
        "+---------+",
        "| present |",
        "+---------+",
        "| 2       |",
        "+---------+",
    ];
    assert_batches_eq!(expected, &batches);
    let explain = ctx
        .sql("EXPLAIN SELECT COUNT(maybe) AS present FROM events")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let explain_text = pretty_format_batches(&explain).unwrap().to_string();
    assert!(!explain_text.contains("CoveExec"), "{explain_text}");
    fs::remove_file(exact_path).unwrap();

    let fallback_path = write_temp_cove("m4d_count_nullable_fallback", nullable_events_file());
    let ctx = SessionContext::new();
    register_cove_file(&ctx, "events", &fallback_path).unwrap();
    let explain = ctx
        .sql("EXPLAIN SELECT COUNT(maybe) AS present FROM events")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let explain_text = pretty_format_batches(&explain).unwrap().to_string();
    assert!(explain_text.contains("CoveExec"), "{explain_text}");
    fs::remove_file(fallback_path).unwrap();
}

#[test]
fn m4d_composite_tuple_prunes_multi_column_filecode_filters() {
    let state = bootstrap_bytes("composite", composite_pairs_file()).unwrap();
    let projection = vec![2];
    let left = FilterPlan::pruning_file_code_in(0, vec![0], "left = 'red'");
    let right = FilterPlan::pruning_file_code_in(1, vec![1], "right = 'blue'");
    let plan = plan_scan(&state, Some(&projection), vec![left, right]).unwrap();

    let decoded = decode_scan(&state, &plan).unwrap();

    let expected = [
        "+---------+",
        "| payload |",
        "+---------+",
        "| hit     |",
        "+---------+",
    ];
    assert_batches_eq!(expected, &decoded.batches);
    assert_eq!(decoded.stats.morsels_considered, 2);
    assert_eq!(decoded.stats.morsels_pruned, 1);
}

#[tokio::test]
async fn m4d_topn_optimizer_adds_read_order_hint_without_removing_sort() {
    let path = write_temp_cove("m4d_topn", topn_events_file());
    let ctx = SessionContext::new();
    register_cove_file(&ctx, "events", &path).unwrap();

    let batches = ctx
        .sql("SELECT id FROM events ORDER BY id DESC LIMIT 1")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let expected = ["+----+", "| id |", "+----+", "| 9  |", "+----+"];
    assert_batches_eq!(expected, &batches);

    let explain = ctx
        .sql("EXPLAIN SELECT id FROM events ORDER BY id DESC LIMIT 1")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let explain_text = pretty_format_batches(&explain).unwrap().to_string();
    assert!(explain_text.contains("topn_hint=Some"), "{explain_text}");
    assert!(
        explain_text.contains("materialization_mode=streaming"),
        "{explain_text}"
    );
    assert!(
        explain_text.contains("SortExec") || explain_text.contains("Sort"),
        "{explain_text}"
    );
    fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn m4e_overlay_snapshot_applies_file_and_row_visibility() {
    let dir = make_temp_dir("m4e_overlay");
    let first = dir.join("part1.cove");
    let second = dir.join("part2.cove");
    fs::write(&first, primitive_events_file()).unwrap();
    fs::write(&second, primitive_events_file()).unwrap();
    let first_state = cove_table_from_path(&first).unwrap();
    let second_state = cove_table_from_path(&second).unwrap();
    let snapshot = CoveOverlaySnapshot {
        snapshot_id: "overlay-1".into(),
        files: vec![
            OverlayFile {
                uri: first.display().to_string().into(),
                expected_identity: Some(identity_for_state(first_state.state())),
                visibility: RowVisibility::DeletedRanges(vec![RowRange { start: 1, len: 1 }]),
            },
            OverlayFile {
                uri: second.display().to_string().into(),
                expected_identity: Some(identity_for_state(second_state.state())),
                visibility: RowVisibility::VisibleRanges(Vec::new()),
            },
        ],
    };

    let ctx = SessionContext::new();
    let provider =
        register_cove_overlay_snapshot(&ctx, "events", snapshot, CoveTableOptions::default())
            .unwrap();
    assert_eq!(provider.state().file_count(), 1);
    assert_eq!(provider.state().bootstrap_stats().overlay_files_hidden, 1);
    assert_eq!(provider.statistics().unwrap().num_rows, Precision::Exact(2));

    let batches = ctx
        .sql("SELECT id FROM events ORDER BY id")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let expected = ["+----+", "| id |", "+----+", "| 1  |", "| 3  |", "+----+"];
    assert_batches_eq!(expected, &batches);

    let count = ctx
        .sql("SELECT COUNT(*) AS rows FROM events")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let expected = ["+------+", "| rows |", "+------+", "| 2    |", "+------+"];
    assert_batches_eq!(expected, &count);
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn m4e_overlay_rejects_stale_identity_unless_file_is_hidden() {
    let path = write_temp_cove("m4e_overlay_stale", primitive_events_file());
    let mut identity = identity_for_state(cove_table_from_path(&path).unwrap().state());
    identity.footer_crc32c ^= 1;

    let visible = CoveOverlaySnapshot {
        snapshot_id: "overlay-stale".into(),
        files: vec![OverlayFile {
            uri: path.display().to_string().into(),
            expected_identity: Some(identity.clone()),
            visibility: RowVisibility::All,
        }],
    };
    let ctx = SessionContext::new();
    let err = register_cove_overlay_snapshot(&ctx, "events", visible, CoveTableOptions::default())
        .unwrap_err()
        .to_string();
    assert!(err.contains("overlay identity mismatch"), "{err}");

    let hidden = CoveOverlaySnapshot {
        snapshot_id: "overlay-hidden-stale".into(),
        files: vec![
            OverlayFile {
                uri: path.display().to_string().into(),
                expected_identity: Some(identity),
                visibility: RowVisibility::VisibleRanges(Vec::new()),
            },
            OverlayFile {
                uri: path.display().to_string().into(),
                expected_identity: None,
                visibility: RowVisibility::All,
            },
        ],
    };
    register_cove_overlay_snapshot(&ctx, "events_ok", hidden, CoveTableOptions::default()).unwrap();
    fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn m5_cove_e_metadata_survives_full_range_and_overlay_bootstrap() {
    let bytes = dictionary_items_file_with_lookup_and_cove_e(sample_dictionary(), true);
    let state = bootstrap_bytes("items_bytes", bytes.clone()).unwrap();
    assert_eq!(
        state.mounted().engine_metadata.execution_descriptors.len(),
        1
    );
    assert_eq!(
        state.mounted().engine_metadata.engine_mount_policies.len(),
        1
    );

    let path = write_temp_cove("m5_cove_e_range", bytes);
    let provider = cove_table_from_path(&path).unwrap();
    assert_eq!(
        provider.state().files()[0]
            .mounted()
            .engine_metadata
            .execution_descriptors
            .len(),
        1
    );

    let snapshot = CoveOverlaySnapshot {
        snapshot_id: "m5-overlay".into(),
        files: vec![OverlayFile {
            uri: path.display().to_string().into(),
            expected_identity: Some(identity_for_state(provider.state())),
            visibility: RowVisibility::All,
        }],
    };
    let ctx = SessionContext::new();
    let overlay =
        register_cove_overlay_snapshot(&ctx, "items", snapshot, CoveTableOptions::default())
            .unwrap();
    assert_eq!(
        overlay.state().files()[0]
            .mounted()
            .engine_metadata
            .execution_descriptors
            .len(),
        1
    );
    fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn m5_execution_code_policy_controls_unsupported_filecode_filters() {
    let path = write_temp_cove(
        "m5_unsupported_cove_e",
        dictionary_items_file_with_lookup_and_cove_e(sample_dictionary(), false),
    );
    let ctx = SessionContext::new();
    register_cove_file_with_options(
        &ctx,
        "items_disabled",
        &path,
        CoveTableOptions::default().with_execution_code_policy(ExecutionCodePolicy::Disabled),
    )
    .unwrap();
    let batches = ctx
        .sql("SELECT payload FROM items_disabled WHERE name = 'red'")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let expected = [
        "+---------+",
        "| payload |",
        "+---------+",
        "| first   |",
        "+---------+",
    ];
    assert_batches_eq!(expected, &batches);

    register_cove_file_with_options(
        &ctx,
        "items_required",
        &path,
        CoveTableOptions::default()
            .with_execution_code_policy(ExecutionCodePolicy::RequireSupported),
    )
    .unwrap();
    let err = ctx
        .sql("SELECT payload FROM items_required WHERE name = 'red'")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("COVE_E_BAD_ENGINE_PROFILE"), "{err}");
    fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn m5_metadata_filecode_count_uses_cove_metadata_exec() {
    let path = write_temp_cove(
        "m5_count_filecode",
        dictionary_items_file_with_lookup_and_cove_e(sample_dictionary(), true),
    );
    let ctx = SessionContext::new();
    register_cove_file(&ctx, "items", &path).unwrap();

    let batches = ctx
        .sql("SELECT COUNT(*) AS rows FROM items WHERE name = 'red'")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let expected = ["+------+", "| rows |", "+------+", "| 1    |", "+------+"];
    assert_batches_eq!(expected, &batches);

    let explain = ctx
        .sql("EXPLAIN SELECT COUNT(*) AS rows FROM items WHERE name = 'red'")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let explain_text = pretty_format_batches(&explain).unwrap().to_string();
    assert!(explain_text.contains("CoveMetadataExec"), "{explain_text}");
    assert!(!explain_text.contains("CoveExec"), "{explain_text}");
    fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn m5_metadata_filecode_group_by_counts_logical_values_across_files() {
    let dir = make_temp_dir("m5_group_overlay");
    let first = dir.join("part1.cove");
    let second = dir.join("part2.cove");
    fs::write(
        &first,
        dictionary_items_file_with_lookup_and_cove_e(sample_dictionary(), true),
    )
    .unwrap();
    fs::write(
        &second,
        dictionary_items_file_with_lookup_and_cove_e(swapped_dictionary(), true),
    )
    .unwrap();
    let first_state = cove_table_from_path(&first).unwrap();
    let second_state = cove_table_from_path(&second).unwrap();
    let snapshot = CoveOverlaySnapshot {
        snapshot_id: "m5-group-overlay".into(),
        files: vec![
            OverlayFile {
                uri: first.display().to_string().into(),
                expected_identity: Some(identity_for_state(first_state.state())),
                visibility: RowVisibility::All,
            },
            OverlayFile {
                uri: second.display().to_string().into(),
                expected_identity: Some(identity_for_state(second_state.state())),
                visibility: RowVisibility::All,
            },
        ],
    };
    let ctx = SessionContext::new();
    register_cove_overlay_snapshot(&ctx, "items", snapshot, CoveTableOptions::default()).unwrap();
    let batches = ctx
        .sql("SELECT name, COUNT(*) AS rows FROM items GROUP BY name ORDER BY name")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let expected = [
        "+------+------+",
        "| name | rows |",
        "+------+------+",
        "| blue | 2    |",
        "| red  | 2    |",
        "+------+------+",
    ];
    assert_batches_eq!(expected, &batches);

    let explain = ctx
        .sql("EXPLAIN SELECT name, COUNT(*) AS rows FROM items GROUP BY name")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let explain_text = pretty_format_batches(&explain).unwrap().to_string();
    assert!(explain_text.contains("CoveMetadataExec"), "{explain_text}");
    fs::remove_dir_all(dir).unwrap();
}

fn identity_for_state(state: &cove_datafusion::dataset_state::DatasetState) -> OverlayFileIdentity {
    OverlayFileIdentity {
        file_id: *state.file_id(),
        file_len: state.file_len(),
        footer_crc32c: state.footer_crc32c(),
        digest: None,
    }
}

fn aggregate_count_entry(
    table_id: u32,
    column_id: u32,
    row_count: u32,
    null_count: u32,
) -> AggregateEntry {
    AggregateEntry {
        table_id,
        segment_id: u32::MAX,
        morsel_id: u32::MAX,
        column_id,
        synopsis_kind: SynopsisKind::Count,
        key_kind: 0,
        accuracy: SynopsisAccuracy::Exact,
        flags: 0,
        row_count,
        null_count,
        payload_offset: 0,
        payload_length: 0,
        checksum: 0,
    }
}

fn dictionary_items_file_with_m4d_metadata() -> Vec<u8> {
    let catalog = dictionary_items_payload_catalog();
    let mut segment = ScanSegment::new(7, 0, 0, 2, 2);
    segment.set_column_pages(1, vec![filecode_page(2, filecodes(&[0, 1]))]);
    segment.set_column_pages(2, vec![varbytes_page(2, varbytes(&["first", "second"]))]);

    let mut writer = ScanProfileCoveWriter::new(catalog);
    writer.push_file_dictionary(&sample_dictionary());
    writer.push_aggregate_synopsis(&AggregateSynopsis {
        entries: vec![aggregate_count_entry(7, 1, 2, 0)],
    });
    writer.push_composite_zone_index(&composite_index(7, vec![1], vec![0, 0, 0]));
    writer.push_topn_summary(&topn_summary(7, 1, 0, 0, TopNDirection::Largest, 1));
    writer.push_segment(segment);
    writer.write().unwrap()
}

fn nullable_events_file_with_count() -> Vec<u8> {
    let catalog = TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 11,
            namespace: "public".into(),
            name: "events".into(),
            row_count: 4,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: vec![
                column(
                    1,
                    "id",
                    CoveLogicalType::Int64,
                    CovePhysicalKind::NumCode,
                    false,
                ),
                column(
                    2,
                    "maybe",
                    CoveLogicalType::Int64,
                    CovePhysicalKind::NumCode,
                    true,
                ),
            ],
        }],
    };

    let mut mixed = ScanSegment::new(11, 0, 0, 2, 2);
    mixed.set_column_pages(1, vec![numcode_page(2, numcode_i64(&[1, 2]))]);
    mixed.set_column_pages(2, vec![nullable_numcode_page(&[Some(10), None])]);

    let mut all_null = ScanSegment::new(11, 1, 2, 1, 2);
    all_null.set_column_pages(1, vec![numcode_page(1, numcode_i64(&[3]))]);
    all_null.set_column_pages(2, vec![nullable_numcode_page(&[None])]);

    let mut all_non_null = ScanSegment::new(11, 2, 3, 1, 2);
    all_non_null.set_column_pages(1, vec![numcode_page(1, numcode_i64(&[4]))]);
    all_non_null.set_column_pages(2, vec![nullable_numcode_page(&[Some(40)])]);

    let mut writer = ScanProfileCoveWriter::new(catalog);
    writer.push_aggregate_synopsis(&AggregateSynopsis {
        entries: vec![aggregate_count_entry(11, 2, 4, 2)],
    });
    writer.push_segment(mixed);
    writer.push_segment(all_null);
    writer.push_segment(all_non_null);
    writer.write().unwrap()
}

fn composite_pairs_file() -> Vec<u8> {
    let catalog = TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 17,
            namespace: "public".into(),
            name: "pairs".into(),
            row_count: 2,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: vec![
                column(
                    1,
                    "left",
                    CoveLogicalType::Utf8,
                    CovePhysicalKind::FileCode,
                    false,
                ),
                column(
                    2,
                    "right",
                    CoveLogicalType::Utf8,
                    CovePhysicalKind::FileCode,
                    false,
                ),
                column(
                    3,
                    "payload",
                    CoveLogicalType::Utf8,
                    CovePhysicalKind::VarBytes,
                    false,
                ),
            ],
        }],
    };
    let mut segment = ScanSegment::new(17, 0, 0, 2, 3);
    segment.morsel_row_count = 1;
    segment.set_column_pages(
        1,
        vec![
            filecode_page(1, filecodes(&[0])),
            filecode_page(1, filecodes(&[1])),
        ],
    );
    segment.set_column_pages(
        2,
        vec![
            filecode_page(1, filecodes(&[1])),
            filecode_page(1, filecodes(&[0])),
        ],
    );
    segment.set_column_pages(
        3,
        vec![
            varbytes_page(1, varbytes(&["hit"])),
            varbytes_page(1, varbytes(&["miss"])),
        ],
    );

    let mut writer = ScanProfileCoveWriter::new(catalog);
    writer.push_file_dictionary(&sample_dictionary());
    writer.push_composite_zone_index(&composite_index(17, vec![1, 2], vec![0, 1, 0, 0]));
    writer.push_segment(segment);
    writer.write().unwrap()
}

fn topn_events_file() -> Vec<u8> {
    let catalog = TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 19,
            namespace: "public".into(),
            name: "events".into(),
            row_count: 2,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: vec![column(
                1,
                "id",
                CoveLogicalType::Int64,
                CovePhysicalKind::NumCode,
                false,
            )],
        }],
    };
    let mut low = ScanSegment::new(19, 0, 0, 1, 1);
    low.set_column_pages(1, vec![numcode_page(1, numcode_i64(&[1]))]);
    let mut high = ScanSegment::new(19, 1, 1, 1, 1);
    high.set_column_pages(1, vec![numcode_page(1, numcode_i64(&[9]))]);

    let mut writer = ScanProfileCoveWriter::new(catalog);
    writer.push_topn_summary(&topn_summary(19, 1, 1, 0, TopNDirection::Largest, 9));
    writer.push_segment(low);
    writer.push_segment(high);
    writer.write().unwrap()
}

fn composite_index(
    table_id: u32,
    key_columns: Vec<u32>,
    tuple_entries: Vec<u32>,
) -> CompositeIndex {
    let mut entries = Vec::new();
    let tuple_width = key_columns.len() + 2;
    for tuple in tuple_entries.chunks_exact(tuple_width) {
        for code in &tuple[..key_columns.len()] {
            entries.extend_from_slice(&u64::from(*code).to_le_bytes());
        }
        entries.extend_from_slice(&tuple[key_columns.len()].to_le_bytes());
        entries.extend_from_slice(&tuple[key_columns.len() + 1].to_le_bytes());
    }
    CompositeIndex {
        header: CompositeZoneIndexHeaderV1 {
            table_id,
            key_column_count: key_columns.len() as u16,
            transform_kind: CompositeTransformKind::Tuple,
            flags: 0,
            zone_count: tuple_entries.len() as u32,
            key_columns_offset: COMPOSITE_ZONE_INDEX_HEADER_LEN as u64,
            entries_offset: 0,
            entries_length: 0,
            checksum: 0,
        },
        key_columns,
        entries,
    }
}

fn topn_summary(
    table_id: u32,
    column_id: u32,
    segment_id: u32,
    morsel_id: u32,
    direction: TopNDirection,
    value: u64,
) -> TopNSummary {
    let mut payload = Vec::new();
    payload.extend_from_slice(&value.to_le_bytes());
    payload.extend_from_slice(&1u64.to_le_bytes());
    TopNSummary {
        table_id,
        column_id,
        segment_id,
        morsel_id,
        direction,
        value_count: 1,
        flags: 0,
        payload_offset: TOPN_ZONE_SUMMARY_LEN as u64,
        payload_length: payload.len() as u64,
        checksum: 0,
        payload,
    }
}

fn primitive_events_file() -> Vec<u8> {
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
                column(
                    1,
                    "id",
                    CoveLogicalType::Int64,
                    CovePhysicalKind::NumCode,
                    false,
                ),
                column(
                    2,
                    "name",
                    CoveLogicalType::Utf8,
                    CovePhysicalKind::VarBytes,
                    false,
                ),
                column(
                    3,
                    "active",
                    CoveLogicalType::Bool,
                    CovePhysicalKind::Boolean,
                    false,
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
    writer.write().unwrap()
}

fn binary_events_file() -> Vec<u8> {
    let catalog = TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 41,
            namespace: "public".into(),
            name: "events".into(),
            row_count: 2,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: vec![column(
                1,
                "payload",
                CoveLogicalType::Binary,
                CovePhysicalKind::VarBytes,
                false,
            )],
        }],
    };
    let mut segment = ScanSegment::new(41, 0, 0, 2, 1);
    segment.set_column_pages(
        1,
        vec![varbytes_page(
            2,
            varbinary(&[b"short", b"long-binary-payload"]),
        )],
    );
    let mut writer = ScanProfileCoveWriter::new(catalog);
    writer.push_segment(segment);
    writer.write().unwrap()
}

fn nullable_events_file() -> Vec<u8> {
    let catalog = TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 11,
            namespace: "public".into(),
            name: "events".into(),
            row_count: 4,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: vec![
                column(
                    1,
                    "id",
                    CoveLogicalType::Int64,
                    CovePhysicalKind::NumCode,
                    false,
                ),
                column(
                    2,
                    "maybe",
                    CoveLogicalType::Int64,
                    CovePhysicalKind::NumCode,
                    true,
                ),
            ],
        }],
    };

    let mut mixed = ScanSegment::new(11, 0, 0, 2, 2);
    mixed.set_column_pages(1, vec![numcode_page(2, numcode_i64(&[1, 2]))]);
    mixed.set_column_pages(2, vec![nullable_numcode_page(&[Some(10), None])]);

    let mut all_null = ScanSegment::new(11, 1, 2, 1, 2);
    all_null.set_column_pages(1, vec![numcode_page(1, numcode_i64(&[3]))]);
    all_null.set_column_pages(2, vec![nullable_numcode_page(&[None])]);

    let mut all_non_null = ScanSegment::new(11, 2, 3, 1, 2);
    all_non_null.set_column_pages(1, vec![numcode_page(1, numcode_i64(&[4]))]);
    all_non_null.set_column_pages(2, vec![nullable_numcode_page(&[Some(40)])]);

    let mut writer = ScanProfileCoveWriter::new(catalog);
    writer.push_segment(mixed);
    writer.push_segment(all_null);
    writer.push_segment(all_non_null);
    writer.write().unwrap()
}

fn dictionary_items_file(dictionary: FileDictionary) -> Vec<u8> {
    let catalog = TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 7,
            namespace: "public".into(),
            name: "items".into(),
            row_count: 2,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: vec![column(
                1,
                "name",
                CoveLogicalType::Utf8,
                CovePhysicalKind::FileCode,
                false,
            )],
        }],
    };
    let mut segment = ScanSegment::new(7, 0, 0, 2, 1);
    segment.set_column_pages(1, vec![filecode_page(2, filecodes(&[0, 1]))]);
    let mut writer = ScanProfileCoveWriter::new(catalog);
    writer.push_file_dictionary(&dictionary);
    if has_redacted_entries(&dictionary) {
        writer.push_extra_section(redaction_manifest_section());
    }
    writer.push_segment(segment);
    writer.write().unwrap()
}

fn mixed_dictionary_items_file() -> Vec<u8> {
    let dictionary = FileDictionary {
        header: FileDictionaryHeaderV1 {
            entry_count: 3,
            flags: 0,
            index_entry_len: FileDictionaryHeaderV1::INDEX_ENTRY_LEN,
            value_hash_algorithm: 0,
            payload_length: 0,
            reserved: [0; 24],
        },
        entries: vec![
            inline_binary_entry(&[0xaa, 0xbb]),
            inline_utf8_entry("red"),
            inline_utf8_entry("blue"),
        ],
        payload: Vec::new(),
    };
    let catalog = TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 7,
            namespace: "public".into(),
            name: "items".into(),
            row_count: 2,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: vec![
                column(
                    1,
                    "name",
                    CoveLogicalType::Utf8,
                    CovePhysicalKind::FileCode,
                    false,
                ),
                column(
                    2,
                    "blob",
                    CoveLogicalType::Binary,
                    CovePhysicalKind::FileCode,
                    false,
                ),
            ],
        }],
    };
    let mut segment = ScanSegment::new(7, 0, 0, 2, 2);
    segment.set_column_pages(1, vec![filecode_page(2, filecodes(&[1, 2]))]);
    segment.set_column_pages(2, vec![filecode_page(2, filecodes(&[0, 0]))]);
    let mut writer = ScanProfileCoveWriter::new(catalog);
    writer.push_file_dictionary(&dictionary);
    writer.push_segment(segment);
    writer.write().unwrap()
}

fn redacted_mixed_dictionary_items_file() -> Vec<u8> {
    let dictionary = FileDictionary {
        header: FileDictionaryHeaderV1 {
            entry_count: 2,
            flags: 0,
            index_entry_len: FileDictionaryHeaderV1::INDEX_ENTRY_LEN,
            value_hash_algorithm: 0,
            payload_length: 0,
            reserved: [0; 24],
        },
        entries: vec![redacted_binary_entry(), inline_utf8_entry("red")],
        payload: Vec::new(),
    };
    let catalog = TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 7,
            namespace: "public".into(),
            name: "items".into(),
            row_count: 1,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: vec![column(
                1,
                "name",
                CoveLogicalType::Utf8,
                CovePhysicalKind::FileCode,
                false,
            )],
        }],
    };
    let mut segment = ScanSegment::new(7, 0, 0, 1, 1);
    segment.set_column_pages(1, vec![filecode_page(1, filecodes(&[1]))]);
    let mut writer = ScanProfileCoveWriter::new(catalog);
    writer.push_file_dictionary(&dictionary);
    writer.push_extra_section(redaction_manifest_section());
    writer.push_segment(segment);
    writer.write().unwrap()
}

fn dictionary_items_file_with_domain_stats() -> Vec<u8> {
    let dictionary = sample_dictionary();
    let catalog = TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 7,
            namespace: "public".into(),
            name: "items".into(),
            row_count: 2,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: vec![
                column(
                    1,
                    "name",
                    CoveLogicalType::Utf8,
                    CovePhysicalKind::FileCode,
                    false,
                ),
                column(
                    2,
                    "payload",
                    CoveLogicalType::Utf8,
                    CovePhysicalKind::VarBytes,
                    false,
                ),
            ],
        }],
    };
    let mut first = ScanSegment::new(7, 0, 0, 1, 2);
    first.set_column_pages(1, vec![filecode_page(1, filecodes(&[0]))]);
    first.set_column_pages(2, vec![varbytes_page(1, varbytes(&["first"]))]);
    let mut second = ScanSegment::new(7, 1, 1, 1, 2);
    second.set_column_pages(1, vec![filecode_page(1, filecodes(&[1]))]);
    second.set_column_pages(2, vec![varbytes_page(1, varbytes(&["second"]))]);

    let mut writer = ScanProfileCoveWriter::new(catalog);
    writer.push_file_dictionary(&dictionary);
    writer.push_extra_section(column_domain_section());
    writer.push_extra_section(filecode_zone_stats_section());
    writer.push_segment(first);
    writer.push_segment(second);
    writer.write().unwrap()
}

fn dictionary_items_file_with_lookup_index() -> Vec<u8> {
    let catalog = dictionary_items_payload_catalog();
    let mut segment = ScanSegment::new(7, 0, 0, 2, 2);
    segment.set_column_pages(1, vec![filecode_page(2, filecodes(&[0, 1]))]);
    segment.set_column_pages(2, vec![varbytes_page(2, varbytes(&["first", "second"]))]);

    let mut writer = ScanProfileCoveWriter::new(catalog);
    writer.push_file_dictionary(&sample_dictionary());
    writer.push_extra_section(lookup_index_section());
    writer.push_segment(segment);
    writer.write().unwrap()
}

fn dictionary_items_file_with_lookup_and_cove_e(
    dictionary: FileDictionary,
    supported_execution_code: bool,
) -> Vec<u8> {
    let catalog = dictionary_items_payload_catalog();
    let mut segment = ScanSegment::new(7, 0, 0, 2, 2);
    segment.set_column_pages(1, vec![filecode_page(2, filecodes(&[0, 1]))]);
    segment.set_column_pages(2, vec![varbytes_page(2, varbytes(&["first", "second"]))]);

    let mut writer = ScanProfileCoveWriter::new(catalog);
    writer.push_file_dictionary(&dictionary);
    writer.push_extra_section(lookup_index_section());
    for section in cove_e_sections(supported_execution_code) {
        writer.push_extra_section(section);
    }
    writer.push_segment(segment);
    writer.write().unwrap()
}

fn dictionary_items_file_with_inverted_index() -> Vec<u8> {
    let catalog = dictionary_items_payload_catalog();
    let mut segment = ScanSegment::new(7, 0, 0, 2, 2);
    segment.morsel_row_count = 1;
    segment.set_column_pages(
        1,
        vec![
            filecode_page(1, filecodes(&[0])),
            filecode_page(1, filecodes(&[1])),
        ],
    );
    segment.set_column_pages(
        2,
        vec![
            varbytes_page(1, varbytes(&["first"])),
            varbytes_page(1, varbytes(&["second"])),
        ],
    );

    let mut writer = ScanProfileCoveWriter::new(catalog);
    writer.push_file_dictionary(&sample_dictionary());
    writer.push_extra_section(inverted_index_section());
    writer.push_segment(segment);
    writer.write().unwrap()
}

fn dictionary_items_file_with_ambiguous_inverted_index() -> Vec<u8> {
    let catalog = dictionary_items_payload_catalog();
    let mut first = ScanSegment::new(7, 0, 0, 1, 2);
    first.morsel_row_count = 1;
    first.set_column_pages(1, vec![filecode_page(1, filecodes(&[0]))]);
    first.set_column_pages(2, vec![varbytes_page(1, varbytes(&["first"]))]);

    let mut second = ScanSegment::new(7, 1, 1, 1, 2);
    second.morsel_row_count = 1;
    second.set_column_pages(1, vec![filecode_page(1, filecodes(&[1]))]);
    second.set_column_pages(2, vec![varbytes_page(1, varbytes(&["second"]))]);

    let mut writer = ScanProfileCoveWriter::new(catalog);
    writer.push_file_dictionary(&sample_dictionary());
    writer.push_extra_section(ambiguous_inverted_index_section());
    writer.push_segment(first);
    writer.push_segment(second);
    writer.write().unwrap()
}

fn numeric_lookup_events_file() -> Vec<u8> {
    let catalog = TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 8,
            namespace: "public".into(),
            name: "events".into(),
            row_count: 3,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: vec![
                column(
                    1,
                    "id",
                    CoveLogicalType::Int64,
                    CovePhysicalKind::NumCode,
                    false,
                ),
                column(
                    2,
                    "payload",
                    CoveLogicalType::Utf8,
                    CovePhysicalKind::VarBytes,
                    false,
                ),
            ],
        }],
    };
    let mut segment = ScanSegment::new(8, 0, 0, 3, 2);
    segment.set_column_pages(1, vec![numcode_page(3, numcode_i64(&[1, 2, 3]))]);
    segment.set_column_pages(
        2,
        vec![varbytes_page(3, varbytes(&["alpha", "beta", "gamma"]))],
    );

    let mut writer = ScanProfileCoveWriter::new(catalog);
    writer.push_extra_section(numcode_lookup_index_section());
    writer.push_segment(segment);
    writer.write().unwrap()
}

fn dictionary_items_payload_catalog() -> TableCatalog {
    TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 7,
            namespace: "public".into(),
            name: "items".into(),
            row_count: 2,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: vec![
                column(
                    1,
                    "name",
                    CoveLogicalType::Utf8,
                    CovePhysicalKind::FileCode,
                    false,
                ),
                column(
                    2,
                    "payload",
                    CoveLogicalType::Utf8,
                    CovePhysicalKind::VarBytes,
                    false,
                ),
            ],
        }],
    }
}

fn filecode_without_dictionary_file() -> Vec<u8> {
    let catalog = TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 7,
            namespace: "public".into(),
            name: "items".into(),
            row_count: 1,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: vec![column(
                1,
                "name",
                CoveLogicalType::Utf8,
                CovePhysicalKind::FileCode,
                false,
            )],
        }],
    };
    let mut segment = ScanSegment::new(7, 0, 0, 1, 1);
    segment.set_column_pages(1, vec![filecode_page(1, filecodes(&[0]))]);
    let mut writer = ScanProfileCoveWriter::new(catalog);
    writer.push_segment(segment);
    writer.write().unwrap()
}

fn multiple_tables_file() -> Vec<u8> {
    let catalog = TableCatalog {
        flags: 0,
        tables: vec![
            TableEntry {
                table_id: 1,
                namespace: "public".into(),
                name: "first".into(),
                row_count: 0,
                primary_sort_key_count: 0,
                clustering_key_count: 0,
                flags: 0,
                columns: vec![column(
                    1,
                    "id",
                    CoveLogicalType::Int64,
                    CovePhysicalKind::NumCode,
                    false,
                )],
            },
            TableEntry {
                table_id: 2,
                namespace: "public".into(),
                name: "second".into(),
                row_count: 0,
                primary_sort_key_count: 0,
                clustering_key_count: 0,
                flags: 0,
                columns: vec![column(
                    1,
                    "id",
                    CoveLogicalType::Int64,
                    CovePhysicalKind::NumCode,
                    false,
                )],
            },
        ],
    };
    ScanProfileCoveWriter::new(catalog).write().unwrap()
}

fn column(
    column_id: u32,
    name: &str,
    logical: CoveLogicalType,
    physical: CovePhysicalKind,
    nullable: bool,
) -> ColumnEntry {
    ColumnEntry {
        column_id,
        name: name.into(),
        logical,
        physical,
        nullable,
        sort_order: 0,
        collation_id: 0,
        precision: 0,
        scale: 0,
        flags: 0,
    }
}

fn numcode_page(row_count: u32, payload: Vec<u8>) -> ScanPageSpec {
    ScanPageSpec::new(row_count, payload).with_encoding_root(CoveEncodingKind::NumCode as u32)
}

fn nullable_numcode_page(values: &[Option<i64>]) -> ScanPageSpec {
    let row_count = values.len() as u32;
    let mut null_bitmap = vec![0u8; values.len().div_ceil(8)];
    let mut non_null_count = 0u32;
    let mut payload_values = Vec::with_capacity(values.len() * 8);
    for (index, value) in values.iter().enumerate() {
        match value {
            Some(value) => {
                non_null_count += 1;
                payload_values.extend_from_slice(&(*value as u64).to_le_bytes());
            }
            None => {
                null_bitmap[index / 8] |= 1u8 << (index % 8);
                payload_values.extend_from_slice(&0u64.to_le_bytes());
            }
        }
    }
    let null_count = row_count - non_null_count;
    let mut payload = Vec::with_capacity(null_bitmap.len() + payload_values.len());
    if null_count != 0 {
        payload.extend_from_slice(&null_bitmap);
    }
    payload.extend_from_slice(&payload_values);
    ScanPageSpec::new(row_count, payload)
        .with_encoding_root(CoveEncodingKind::NumCode as u32)
        .with_counts(non_null_count, null_count)
}

fn varbytes_page(row_count: u32, payload: Vec<u8>) -> ScanPageSpec {
    ScanPageSpec::new(row_count, payload).with_encoding_root(CoveEncodingKind::VarBytes as u32)
}

fn bool_page(row_count: u32, payload: Vec<u8>) -> ScanPageSpec {
    ScanPageSpec::new(row_count, payload).with_encoding_root(CoveEncodingKind::PlainFixed as u32)
}

fn filecode_page(row_count: u32, payload: Vec<u8>) -> ScanPageSpec {
    ScanPageSpec::new(row_count, payload).with_encoding_root(CoveEncodingKind::FileCode as u32)
}

fn has_redacted_entries(dictionary: &FileDictionary) -> bool {
    dictionary
        .entries
        .iter()
        .any(|entry| entry.storage_class == StorageClass::Redacted as u8)
}

fn redaction_manifest_section() -> SectionPayload {
    let manifest = RedactionManifest {
        entries: vec![RedactionEntry {
            redaction_id: 1,
            section_id: 2,
            local_ref: 0,
            reason_code: 1,
            policy_id: b"test/redacted".to_vec(),
            audit_ref: b"native_single_file".to_vec(),
            created_at_us: 0,
        }],
    };
    SectionPayload {
        section_kind: SectionKind::RedactionManifest as u16,
        profile: PrimaryProfile::Mixed as u8,
        flags: 0,
        item_count: 1,
        row_count: 0,
        compression: 0,
        alignment_log2: 0,
        required_features: FEATURE_REDACTIONS,
        optional_features: 0,
        data: manifest.serialize().unwrap(),
    }
}

fn column_domain_section() -> SectionPayload {
    let domain = ColumnDomain::from_sorted_present_codes(
        &[0, 1],
        2,
        7,
        1,
        CoveLogicalType::Utf8 as u16,
        0,
        0,
    )
    .unwrap();
    SectionPayload {
        section_kind: SectionKind::ColumnDomain as u16,
        profile: PrimaryProfile::TableScan as u8,
        flags: 0,
        item_count: 1,
        row_count: 0,
        compression: 0,
        alignment_log2: 0,
        required_features: 0,
        optional_features: 0,
        data: domain.serialize().unwrap(),
    }
}

fn filecode_zone_stats_section() -> SectionPayload {
    let entries = vec![
        filecode_zone_stats_entry(0, 0),
        filecode_zone_stats_entry(1, 1),
    ];
    let section = ZoneStatsSection { entries };
    SectionPayload {
        section_kind: SectionKind::ZoneStats as u16,
        profile: PrimaryProfile::TableScan as u8,
        flags: 0,
        item_count: 2,
        row_count: 2,
        compression: 0,
        alignment_log2: 0,
        required_features: 0,
        optional_features: 0,
        data: section.serialize().unwrap(),
    }
}

fn lookup_index_section() -> SectionPayload {
    let index = LookupIndex {
        header: LookupIndexHeaderV1 {
            table_id: 7,
            column_id: 1,
            key_kind: LookupKeyKind::FileCode,
            index_kind: LookupIndexKind::SparseSorted,
            uniqueness: LookupUniqueness::NonUnique,
            flags: 0,
            entry_count: 0,
            entries_offset: 0,
            entries_length: 0,
            rowref_offset: 0,
            rowref_length: 0,
            checksum: 0,
        },
        entries: vec![
            LookupEntry {
                key: 0,
                rows: vec![RowRef {
                    table_id: 7,
                    segment_id: 0,
                    morsel_id: 0,
                    row_in_morsel: 0,
                }],
            },
            LookupEntry {
                key: 1,
                rows: vec![RowRef {
                    table_id: 7,
                    segment_id: 0,
                    morsel_id: 0,
                    row_in_morsel: 1,
                }],
            },
        ],
    };
    SectionPayload {
        section_kind: SectionKind::LookupIndex as u16,
        profile: PrimaryProfile::ArchiveAcceleration as u8,
        flags: 0,
        item_count: 2,
        row_count: 2,
        compression: 0,
        alignment_log2: 0,
        required_features: 0,
        optional_features: 0,
        data: index.serialize().unwrap(),
    }
}

fn numcode_lookup_index_section() -> SectionPayload {
    let index = LookupIndex {
        header: LookupIndexHeaderV1 {
            table_id: 8,
            column_id: 1,
            key_kind: LookupKeyKind::NumCode,
            index_kind: LookupIndexKind::SparseSorted,
            uniqueness: LookupUniqueness::NonUnique,
            flags: 0,
            entry_count: 0,
            entries_offset: 0,
            entries_length: 0,
            rowref_offset: 0,
            rowref_length: 0,
            checksum: 0,
        },
        entries: vec![LookupEntry {
            key: 2,
            rows: vec![RowRef {
                table_id: 8,
                segment_id: 0,
                morsel_id: 0,
                row_in_morsel: 1,
            }],
        }],
    };
    SectionPayload {
        section_kind: SectionKind::LookupIndex as u16,
        profile: PrimaryProfile::ArchiveAcceleration as u8,
        flags: 0,
        item_count: 1,
        row_count: 3,
        compression: 0,
        alignment_log2: 0,
        required_features: 0,
        optional_features: 0,
        data: index.serialize().unwrap(),
    }
}

fn cove_e_sections(supported_execution_code: bool) -> Vec<SectionPayload> {
    let descriptor = ExecutionCodeDescriptorV1 {
        descriptor_id: 1,
        code_kind: if supported_execution_code {
            ExecutionCodeKind::DictionaryKey
        } else {
            ExecutionCodeKind::OpaqueBytes
        },
        code_width_bits: if supported_execution_code { 32 } else { 128 },
        byte_order: 0,
        lifetime: ExecutionCodeLifetime::Scan,
        comparison_scope: ExecutionCodeComparisonScope::File,
        canonicality: ExecutionCodeCanonicality::Transient,
        null_code_policy: NullCodePolicy::NullBitmapOnly,
        flags: 0,
        scope_ref: 0,
        code_space_ref: 0,
        checksum: 0,
    };
    let policy = EngineMountPolicyV1 {
        policy_id: 2,
        filecode_mapping_kind: FileCodeMappingKind::MapToArrowDictionary,
        missing_value_policy: MissingValuePolicy::DecodeValueOnly,
        stale_mapping_policy: StaleMappingPolicy::IgnoreIfOptional,
        reverse_lookup_policy: ReverseLookupPolicy::BuildFromDictionary,
        flags: 0,
        dictionary_digest_ref: 0,
        code_space_ref: 0,
        cache_key_ref: 0,
        private_payload_ref: 0,
        checksum: 0,
    };
    let registry = EngineProfileRegistry {
        flags: 0,
        profiles: vec![EngineProfileEntryV1 {
            profile_id: 3,
            namespace: "org.apache.datafusion".into(),
            profile_name: "arrow-dictionary".into(),
            version_major: 1,
            version_minor: 0,
            required_features: 0,
            optional_features: 0,
            execution_descriptor_ref: 1,
            mount_policy_ref: 2,
            private_payload_ref: 0,
            checksum: 0,
        }],
    };
    vec![
        cove_e_section(
            SectionKind::EngineProfileRegistry,
            1,
            registry.serialize().unwrap(),
        ),
        cove_e_section(
            SectionKind::ExecutionCodeDescriptor,
            1,
            descriptor.serialize().to_vec(),
        ),
        cove_e_section(
            SectionKind::EngineMountPolicy,
            1,
            policy.serialize().to_vec(),
        ),
    ]
}

fn cove_e_section(kind: SectionKind, item_count: u64, data: Vec<u8>) -> SectionPayload {
    SectionPayload {
        section_kind: kind as u16,
        profile: PrimaryProfile::EngineExecution as u8,
        flags: 0,
        item_count,
        row_count: 0,
        compression: 0,
        alignment_log2: 0,
        required_features: FEATURE_ENGINE_PROFILE,
        optional_features: 0,
        data,
    }
}

fn inverted_index_section() -> SectionPayload {
    let index = InvertedMorselIndex {
        header: InvertedMorselIndexHeaderV1 {
            table_id: 7,
            column_id: 1,
            key_kind: InvertedKeyKind::FileCode,
            flags: 0,
            representation: 0,
            reserved: 0,
            entry_count: 0,
            entries_offset: 0,
            bitmap_data_offset: 0,
            checksum: 0,
        },
        entries: vec![InvertedEntry {
            key: 0,
            morsel_bitmap_offset: 0,
            morsel_bitmap_length: 1,
            row_bitmap_offset: 0,
            row_bitmap_length: 0,
        }],
        bitmap_data: vec![0b0000_0001],
    };
    SectionPayload {
        section_kind: SectionKind::InvertedMorselIndex as u16,
        profile: PrimaryProfile::ArchiveAcceleration as u8,
        flags: 0,
        item_count: 1,
        row_count: 2,
        compression: 0,
        alignment_log2: 0,
        required_features: 0,
        optional_features: 0,
        data: index.serialize(),
    }
}

fn ambiguous_inverted_index_section() -> SectionPayload {
    let index = InvertedMorselIndex {
        header: InvertedMorselIndexHeaderV1 {
            table_id: 7,
            column_id: 1,
            key_kind: InvertedKeyKind::FileCode,
            flags: 0,
            representation: 0,
            reserved: 0,
            entry_count: 0,
            entries_offset: 0,
            bitmap_data_offset: 0,
            checksum: 0,
        },
        entries: vec![
            InvertedEntry {
                key: 0,
                morsel_bitmap_offset: 0,
                morsel_bitmap_length: 1,
                row_bitmap_offset: 0,
                row_bitmap_length: 0,
            },
            InvertedEntry {
                key: 1,
                morsel_bitmap_offset: 1,
                morsel_bitmap_length: 1,
                row_bitmap_offset: 0,
                row_bitmap_length: 0,
            },
        ],
        bitmap_data: vec![0b0000_0001, 0b0000_0010],
    };
    SectionPayload {
        section_kind: SectionKind::InvertedMorselIndex as u16,
        profile: PrimaryProfile::ArchiveAcceleration as u8,
        flags: 0,
        item_count: 2,
        row_count: 2,
        compression: 0,
        alignment_log2: 0,
        required_features: 0,
        optional_features: 0,
        data: index.serialize(),
    }
}

fn filecode_zone_stats_entry(segment_id: u32, rank: u32) -> ZoneStatsEntry {
    ZoneStatsEntry {
        table_id: 7,
        segment_id,
        morsel_id: 0,
        column_id: 1,
        non_null_count: 1,
        distinct_count: 1,
        run_count: 1,
        stats: ZoneStats {
            scope: cove_core::zone_stats::ZoneScope::Morsel,
            row_count: 1,
            null_count: 0,
            min: None,
            max: None,
            flags: ZoneStatFlags::HAS_DOMAIN_RANGE | ZoneStatFlags::CONSTANT,
        },
        min_domain_rank: rank,
        max_domain_rank: rank,
        exact_set_ref: u32::MAX,
        bloom_ref: u32::MAX,
    }
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

fn varbinary(values: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for value in values {
        out.extend_from_slice(&(value.len() as u32).to_le_bytes());
        out.extend_from_slice(value);
    }
    out
}

fn bools(values: &[bool]) -> Vec<u8> {
    values.iter().map(|value| u8::from(*value)).collect()
}

fn filecodes(values: &[u32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn sample_dictionary() -> FileDictionary {
    FileDictionary {
        header: FileDictionaryHeaderV1 {
            entry_count: 2,
            flags: 0,
            index_entry_len: FileDictionaryHeaderV1::INDEX_ENTRY_LEN,
            value_hash_algorithm: 0,
            payload_length: 0,
            reserved: [0; 24],
        },
        entries: vec![inline_utf8_entry("red"), inline_utf8_entry("blue")],
        payload: Vec::new(),
    }
}

fn redacted_dictionary() -> FileDictionary {
    FileDictionary {
        header: FileDictionaryHeaderV1 {
            entry_count: 2,
            flags: 0,
            index_entry_len: FileDictionaryHeaderV1::INDEX_ENTRY_LEN,
            value_hash_algorithm: 0,
            payload_length: 0,
            reserved: [0; 24],
        },
        entries: vec![redacted_utf8_entry(), inline_utf8_entry("blue")],
        payload: Vec::new(),
    }
}

fn swapped_dictionary() -> FileDictionary {
    FileDictionary {
        header: FileDictionaryHeaderV1 {
            entry_count: 2,
            flags: 0,
            index_entry_len: FileDictionaryHeaderV1::INDEX_ENTRY_LEN,
            value_hash_algorithm: 0,
            payload_length: 0,
            reserved: [0; 24],
        },
        entries: vec![inline_utf8_entry("blue"), inline_utf8_entry("red")],
        payload: Vec::new(),
    }
}

fn inline_utf8_entry(value: &str) -> FileDictionaryIndexEntryV1 {
    let canonical = canonical_utf8(value);
    let mut inline_data = [0u8; 16];
    inline_data[..canonical.len()].copy_from_slice(&canonical);
    FileDictionaryIndexEntryV1 {
        value_tag: ValueTag::Utf8 as u16,
        storage_class: StorageClass::Inline as u8,
        flags: 0,
        inline_len: canonical.len() as u8,
        reserved0: [0; 3],
        inline_data,
        payload_offset: 0,
        payload_length: 0,
        canonical_hash64: 0,
        reserved1: 0,
    }
}

fn inline_binary_entry(value: &[u8]) -> FileDictionaryIndexEntryV1 {
    let mut canonical = wire::encode_u64_leb128(value.len() as u64);
    canonical.extend_from_slice(value);
    let mut inline_data = [0u8; 16];
    inline_data[..canonical.len()].copy_from_slice(&canonical);
    FileDictionaryIndexEntryV1 {
        value_tag: ValueTag::Binary as u16,
        storage_class: StorageClass::Inline as u8,
        flags: 0,
        inline_len: canonical.len() as u8,
        reserved0: [0; 3],
        inline_data,
        payload_offset: 0,
        payload_length: 0,
        canonical_hash64: 0,
        reserved1: 0,
    }
}

fn redacted_utf8_entry() -> FileDictionaryIndexEntryV1 {
    FileDictionaryIndexEntryV1 {
        value_tag: ValueTag::Utf8 as u16,
        storage_class: StorageClass::Redacted as u8,
        flags: 0,
        inline_len: 0,
        reserved0: [0; 3],
        inline_data: [0; 16],
        payload_offset: 0,
        payload_length: 0,
        canonical_hash64: 0,
        reserved1: 0,
    }
}

fn redacted_binary_entry() -> FileDictionaryIndexEntryV1 {
    FileDictionaryIndexEntryV1 {
        value_tag: ValueTag::Binary as u16,
        storage_class: StorageClass::Redacted as u8,
        flags: 0,
        inline_len: 0,
        reserved0: [0; 3],
        inline_data: [0; 16],
        payload_offset: 0,
        payload_length: 0,
        canonical_hash64: 0,
        reserved1: 0,
    }
}

fn canonical_utf8(value: &str) -> Vec<u8> {
    let mut canonical = wire::encode_u64_leb128(value.len() as u64);
    canonical.extend_from_slice(value.as_bytes());
    canonical
}

fn write_temp_cove(label: &str, bytes: Vec<u8>) -> PathBuf {
    let id = NEXT_FILE_ID.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "cove-datafusion-{label}-{}-{id}.cove",
        std::process::id()
    ));
    fs::write(&path, bytes).unwrap();
    path
}

#[cfg(feature = "covm")]
fn write_covm_manifest(path: &std::path::Path, files: Vec<CovmFileEntryV1>) {
    let manifest = CovmFile {
        header: CovmHeaderV1::new([0xC0; 16], 1, files.len() as u32, 0),
        files,
        postscript: CovmPostscriptV1 {
            header_offset: 0,
            header_len: 0,
            entries_offset: 0,
            entries_len: 0,
            file_len: 0,
            flags: 0,
            checksum: 0,
        },
    };
    fs::write(path, manifest.serialize().unwrap()).unwrap();
}

#[cfg(feature = "covm")]
fn covm_entry_for_path(uri: &str, path: &std::path::Path) -> CovmFileEntryV1 {
    let state = bootstrap_local_file(path).unwrap();
    CovmFileEntryV1 {
        file_id: *state.file_id(),
        uri: uri.to_string(),
        file_len: state.file_len(),
        footer_crc32c: state.footer_crc32c(),
        digest_algorithm: DigestAlgorithm::None as u16,
        digest: Vec::new(),
        row_count: state.table().row_count,
        segment_count: state.segments().len() as u32,
        file_stats_ref: u32::MAX,
        file_exact_set_ref: u32::MAX,
        flags: 0,
    }
}

#[cfg(all(feature = "covm", feature = "covx"))]
fn write_covx_sidecar(path: &std::path::Path, referenced_files: Vec<CovxReferencedFileV1>) {
    let sidecar = CovxFile {
        header: CovxHeaderV1::new([0xC1; 16], referenced_files.len() as u32, 0),
        referenced_files,
        postscript: CovxPostscriptV1 {
            header_offset: 0,
            header_len: 0,
            entries_offset: 0,
            entries_len: 0,
            file_len: 0,
            flags: 0,
            checksum: 0,
        },
    };
    fs::write(path, sidecar.serialize().unwrap()).unwrap();
}

#[cfg(all(feature = "covm", feature = "covx"))]
fn covx_entry_for_path(path: &std::path::Path) -> CovxReferencedFileV1 {
    let state = bootstrap_local_file(path).unwrap();
    CovxReferencedFileV1 {
        file_id: *state.file_id(),
        file_len: state.file_len(),
        footer_crc32c: state.footer_crc32c(),
        digest_algorithm: DigestAlgorithm::None as u16,
        digest: Vec::new(),
    }
}

fn make_temp_dir(label: &str) -> PathBuf {
    let id = NEXT_FILE_ID.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "cove-datafusion-{label}-{}-{id}",
        std::process::id()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

#[derive(Debug, Clone, Copy)]
struct QueryCounts {
    full_gets: usize,
    range_gets: usize,
    bytes_returned: usize,
}

async fn query_counting_store(sql: &str) -> QueryCounts {
    let inner = Arc::new(InMemory::new());
    inner
        .put_opts(
            &Path::from("dataset/part1.cove"),
            primitive_events_file().into(),
            PutOptions::default(),
        )
        .await
        .unwrap();
    let store = Arc::new(CountingObjectStore::new(inner));
    let ctx = SessionContext::new();
    ctx.register_object_store(
        &Url::parse("cove-test://bucket").unwrap(),
        store.clone() as Arc<dyn ObjectStore>,
    );
    register_cove_listing_table(&ctx, "events", "cove-test://bucket/dataset/")
        .await
        .unwrap();
    let batches = ctx.sql(sql).await.unwrap().collect().await.unwrap();
    assert!(!batches.is_empty());
    store.counts()
}

#[derive(Debug)]
struct CountingObjectStore {
    inner: Arc<dyn ObjectStore>,
    full_gets: std::sync::atomic::AtomicUsize,
    range_gets: std::sync::atomic::AtomicUsize,
    bytes_returned: std::sync::atomic::AtomicUsize,
}

impl CountingObjectStore {
    fn new(inner: Arc<dyn ObjectStore>) -> Self {
        Self {
            inner,
            full_gets: std::sync::atomic::AtomicUsize::new(0),
            range_gets: std::sync::atomic::AtomicUsize::new(0),
            bytes_returned: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn counts(&self) -> QueryCounts {
        QueryCounts {
            full_gets: self.full_gets.load(Ordering::SeqCst),
            range_gets: self.range_gets.load(Ordering::SeqCst),
            bytes_returned: self.bytes_returned.load(Ordering::SeqCst),
        }
    }
}

impl fmt::Display for CountingObjectStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CountingObjectStore")
    }
}

#[async_trait]
impl ObjectStore for CountingObjectStore {
    async fn put_opts(
        &self,
        location: &Path,
        payload: PutPayload,
        opts: PutOptions,
    ) -> datafusion::object_store::Result<PutResult> {
        self.inner.put_opts(location, payload, opts).await
    }

    async fn put_multipart_opts(
        &self,
        location: &Path,
        opts: PutMultipartOptions,
    ) -> datafusion::object_store::Result<Box<dyn MultipartUpload>> {
        self.inner.put_multipart_opts(location, opts).await
    }

    async fn get_opts(
        &self,
        location: &Path,
        options: GetOptions,
    ) -> datafusion::object_store::Result<GetResult> {
        self.full_gets.fetch_add(1, Ordering::SeqCst);
        self.inner.get_opts(location, options).await
    }

    async fn get_ranges(
        &self,
        location: &Path,
        ranges: &[Range<u64>],
    ) -> datafusion::object_store::Result<Vec<bytes::Bytes>> {
        self.range_gets.fetch_add(ranges.len(), Ordering::SeqCst);
        let chunks = self.inner.get_ranges(location, ranges).await?;
        let bytes = chunks.iter().map(|chunk| chunk.len()).sum::<usize>();
        self.bytes_returned.fetch_add(bytes, Ordering::SeqCst);
        Ok(chunks)
    }

    fn delete_stream(
        &self,
        locations: BoxStream<'static, datafusion::object_store::Result<Path>>,
    ) -> BoxStream<'static, datafusion::object_store::Result<Path>> {
        self.inner.delete_stream(locations)
    }

    fn list(
        &self,
        prefix: Option<&Path>,
    ) -> BoxStream<'static, datafusion::object_store::Result<ObjectMeta>> {
        self.inner.list(prefix)
    }

    async fn list_with_delimiter(
        &self,
        prefix: Option<&Path>,
    ) -> datafusion::object_store::Result<ListResult> {
        self.inner.list_with_delimiter(prefix).await
    }

    async fn copy_opts(
        &self,
        from: &Path,
        to: &Path,
        options: CopyOptions,
    ) -> datafusion::object_store::Result<()> {
        self.inner.copy_opts(from, to, options).await
    }
}
