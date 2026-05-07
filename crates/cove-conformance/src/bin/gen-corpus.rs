//! Generates the conformance corpus referenced by `conformance/manifest.jsonl`.
//! Run with `cargo run -p cove-conformance --bin gen-corpus`.
//!
//! Each fixture maps to one or more Spec §76 error codes; the manifest is
//! written alongside the binaries so the generator stays the source of truth.

use std::{collections::BTreeSet, fs, io::Cursor, path::PathBuf, sync::Arc};

use arrow_array::{
    builder::{Int32Builder, ListBuilder},
    ArrayRef, BinaryArray, BooleanArray, Date32Array, Float64Array, Int64Array, RecordBatch,
    StringArray, TimestampMicrosecondArray,
};
use parquet::arrow::ArrowWriter;

use cove_core::{
    artifact::{
        covemap::{
            CovemapFile, CovemapHeaderV1, CovemapPostscriptV1, CovemapSection,
            CovemapSectionEntryV1, COVEMAP_HEADER_LEN, COVEMAP_POSTSCRIPT_LEN,
            COVEMAP_POSTSCRIPT_TAIL_SIZE,
        },
        covm::{CovmFile, CovmFileEntryV1, CovmHeaderV1, CovmPostscriptV1},
        covx::{CovxFile, CovxHeaderV1, CovxPostscriptV1, CovxReferencedFileV1},
    },
    canonical::{CanonicalField, CanonicalValue},
    checksum,
    constants::{
        CompressionCodec, CoveEncodingKind, CoveLogicalType, CovePhysicalKind, DigestAlgorithm,
        PrimaryProfile, SectionKind, StorageClass, ValueTag, FEATURE_CODEC_LZ4, FEATURE_CODEC_ZSTD,
        FEATURE_COLUMN_DOMAINS, FEATURE_ENGINE_PROFILE, FEATURE_FILE_DICTIONARY,
        FEATURE_HARBOR_PROFILE, FEATURE_OBJECT_PROFILE, FEATURE_PAGE_PAYLOAD_ELISION,
        FEATURE_SEMANTIC_MAP, FEATURE_TABLE_PROFILE, FEATURE_TRUST_CHAIN,
    },
    dictionary::{FileDictionaryHeaderV1, FileDictionaryIndexEntryV1},
    digest::{compute_digest, DigestEntry, DigestManifest, DigestScope, DigestTargetKind},
    domain::{ColumnDomain, ColumnDomainHeaderV1, COLUMN_DOMAIN_HEADER_LEN},
    encoding::{
        bit_packed::BitPackedPayload,
        constant::ConstantPayload,
        delta::DeltaPayload,
        frame_of_reference::ForPayload,
        local_codebook::{LocalCodebookPayload, LocalCodebookValues, LocalIndexPayload},
        nested::{
            ListLayout, ListLayoutPayload, MapLayout, MapLayoutPayload, StructLayout,
            StructLayoutPayload,
        },
        plain::{PlainFixedPayload, PlainVarintPayload},
        rle::RlePayload,
    },
    extensions::{
        ExtensionFalseNegativePolicy, ExtensionIndexDescriptorV1, ExtensionKind,
        ExtensionLogicalTypeV1, ExtensionProofCapability, ExtensionRegistry,
        ExtensionRegistryEntry,
    },
    footer::{CoveFooterHeaderV1, CoveSectionEntryV1, FOOTER_HEADER_SIZE, SECTION_ENTRY_SIZE},
    header::{CoveHeaderV1, HEADER_SIZE},
    index::{
        aggregate::{AggregateEntry, SynopsisAccuracy, SynopsisKind},
        bloom::{
            BloomAlgorithm, BloomGranularity, BloomHashDomain, BloomIndexHeaderV1,
            BLOOM_INDEX_HEADER_LEN,
        },
        composite::{
            CompositeTransformKind, CompositeZoneIndexHeaderV1, COMPOSITE_ZONE_INDEX_HEADER_LEN,
        },
        exact_set::{
            ExactSetGranularity, ExactSetIndexHeaderV1, ExactSetKeyKind, ExactSetRepresentation,
            EXACT_SET_HEADER_LEN,
        },
        inverted::{
            InvertedEntry, InvertedKeyKind, InvertedMorselIndexHeaderV1, INVERTED_MORSEL_ENTRY_LEN,
            INVERTED_MORSEL_INDEX_HEADER_LEN,
        },
        lookup::{
            LookupIndexHeaderV1, LookupIndexKind, LookupKeyKind, LookupUniqueness,
            LOOKUP_INDEX_HEADER_LEN,
        },
        topn::{TopNDirection, TopNSummary, TOPN_ZONE_SUMMARY_LEN},
    },
    interop::lakehouse::{LakehouseHints, LakehouseVisibilityOverlayRef},
    io_hints::defaults_object_store,
    kernel::{KernelCapabilities, KernelCapabilityEntry},
    page::{
        ColumnPageIndexEntryV1, COLUMN_PAGE_INDEX_ENTRY_LEN, PAGE_FLAG_ALL_NON_NULL,
        PAGE_FLAG_ALL_NULL, PAGE_FLAG_STATS_ONLY_CONSTANT, PAGE_FLAG_VALUE_STREAM_ELIDED,
    },
    postscript::{CovePostscriptV1, POSTSCRIPT_SIZE, POSTSCRIPT_TOTAL_SIZE},
    profile::{
        cove_e::{
            CodeSpaceDescriptorV1, EngineMountPolicyV1, EngineProfileEntryV1,
            EngineProfileRegistry, ExecutionCodeCanonicality, ExecutionCodeComparisonScope,
            ExecutionCodeDescriptorV1, ExecutionCodeKind, ExecutionCodeLifetime,
            ExecutionScopeDescriptorV1, ExecutionScopeKind, FileCodeMappingKind,
            MissingValuePolicy, NullCodePolicy, ReverseLookupPolicy, StaleMappingPolicy,
        },
        cove_h::HarborMountHintsV1,
        cove_o::{
            CoveRecordRefV1, ObjectTypeCatalog, ObjectTypeEntryV1, PropertyEntryV1, RecordKind,
            TemporalBloomEntryV1, TemporalBloomIndex, TemporalRowEntryV1, TemporalSegmentData,
            TemporalSegmentHeaderV1, TemporalSegmentIndex, TemporalSegmentIndexEntryV1,
            TEMPORAL_BLOOM_ENTRY_LEN, TEMPORAL_ROW_ENTRY_LEN, TEMPORAL_SEGMENT_HEADER_LEN,
        },
    },
    reader,
    row_ref::RowRef,
    segment::{
        RowMorselDirectory, RowMorselEntryV1, TableColumnDirectoryEntryV1, TableSegmentHeaderV1,
        TableSegmentIndex, TableSegmentIndexEntryV1, TableSegmentPayloadV1,
        TABLE_COLUMN_DIRECTORY_ENTRY_LEN, TABLE_SEGMENT_HEADER_LEN,
    },
    sort::{ClusteringKeyEntryV1, ClusteringStrength, NullOrder, SortDirection, SortKeyEntryV1},
    table::{ColumnEntry, TableCatalog, TableEntry, COLUMN_FLAG_BOOL_DECLARED_NUMERIC},
    writer::{MinimalCoveWriter, ScanPageSpec, ScanProfileCoveWriter, ScanSegment, SectionPayload},
    zone_stats::{
        StatKind, StatScalar, ZoneScope, ZoneStatFlags, ZoneStats, ZoneStatsEntry,
        ZoneStatsSection, STAT_SCALAR_ENCODED_LEN, ZONE_STATS_ENTRY_LEN,
    },
    CoveError,
};
use serde_json::{json, Value};

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("conformance");
    fs::create_dir_all(root.join("accept")).unwrap();
    fs::create_dir_all(root.join("reject")).unwrap();

    let mut entries = Vec::new();

    // accept/min_empty: structurally valid empty COVE-T file.
    let bytes = MinimalCoveWriter::write_empty_file().unwrap();
    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/min_empty.cove",
            "cove",
            "accept",
            None,
            &["§9", "§10", "§12", "§13", "§72.1"],
        ),
        bytes.clone(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_t_scan_table.cove",
            "cove",
            "accept",
            None,
            &["§24", "§25", "§26", "§27", "§72.2", "§72.3", "§73"],
        ),
        cove_t_scan_table_file(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_t_bool_numcode_declared.cove",
            "cove",
            "accept",
            None,
            &["§19", "§24", "§25", "§73"],
        ),
        cove_t_bool_numcode_file(true),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_t_payload_elision_stats_only_all_null_valid.cove",
            "cove",
            "accept",
            None,
            &["§27.2", "§72.2", "§73"],
        ),
        cove_t_payload_elision_stats_only_all_null_file(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_t_payload_elision_stats_only_all_non_null_valid.cove",
            "cove",
            "accept",
            None,
            &["§27.2", "§28", "§72.2", "§73"],
        ),
        cove_t_payload_elision_stats_only_all_non_null_file(Some(valid_constant_page_stats())),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_t_payload_elision_value_stream_mixed_constant.cove",
            "cove",
            "accept",
            None,
            &["§20.6", "§27.2", "§72.2", "§73"],
        ),
        cove_t_payload_elision_value_stream_mixed_constant_file(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_t_payload_elision_value_stream_wrong_root.cove",
            "cove",
            "reject",
            Some("COVE_E_PAGE_CORRUPT"),
            &["§20.6", "§27.2", "§73"],
        ),
        cove_t_payload_elision_value_stream_wrong_root_file(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_t_payload_elision_value_stream_missing_bitmap.cove",
            "cove",
            "reject",
            Some("COVE_E_PAGE_CORRUPT"),
            &["§20.6", "§27.2", "§73"],
        ),
        cove_t_payload_elision_value_stream_missing_bitmap_file(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_t_payload_elision_value_stream_missing_feature.cove",
            "cove",
            "reject",
            Some("COVE_E_BAD_SECTION"),
            &["§20.6", "§27.2", "§72.2"],
        ),
        cove_t_payload_elision_value_stream_missing_feature_file(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_t_payload_elision_missing_feature.cove",
            "cove",
            "reject",
            Some("COVE_E_BAD_SECTION"),
            &["§27.2", "§72.2"],
        ),
        cove_t_payload_elision_missing_feature_file(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_t_stats_only_all_non_null_missing_stats.cove",
            "cove",
            "reject",
            Some("COVE_E_PAGE_CORRUPT"),
            &["§27.2", "§28", "§73"],
        ),
        cove_t_payload_elision_stats_only_all_non_null_file(None),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_t_stats_only_all_non_null_missing_constant_flag.cove",
            "cove",
            "reject",
            Some("COVE_E_PAGE_CORRUPT"),
            &["§27.2", "§28", "§73"],
        ),
        cove_t_payload_elision_stats_only_all_non_null_file(Some(constant_page_stats_with_flags(
            ZoneStatFlags::HAS_MIN_MAX,
        ))),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_t_stats_only_all_non_null_wrong_scope.cove",
            "cove",
            "reject",
            Some("COVE_E_PAGE_CORRUPT"),
            &["§27.2", "§28", "§73"],
        ),
        cove_t_payload_elision_stats_only_all_non_null_file(
            Some(wrong_scope_constant_page_stats()),
        ),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_t_stats_only_all_non_null_float32_stats.cove",
            "cove",
            "reject",
            Some("COVE_E_PAGE_CORRUPT"),
            &["§27.2", "§28", "§73"],
        ),
        cove_t_payload_elision_stats_only_all_non_null_float32_file(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_t_numcode_page_short_values.cove",
            "cove",
            "reject",
            Some("COVE_E_PAGE_CORRUPT"),
            &["§27.3", "§73"],
        ),
        cove_t_numcode_page_short_values_file(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_t_local_codebook_lz4.cove",
            "cove",
            "accept",
            None,
            &["§20", "§25", "§27", "§66", "§72.3"],
        ),
        cove_t_local_codebook_lz4_file(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_t_nested_list_valid.cove",
            "cove",
            "accept",
            None,
            &["§25", "§27", "§52", "§72.3"],
        ),
        cove_t_nested_list_valid_file(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_t_nested_struct_valid.cove",
            "cove",
            "accept",
            None,
            &["§25", "§27", "§52", "§72.3"],
        ),
        cove_t_nested_struct_valid_file(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_t_nested_map_valid.cove",
            "cove",
            "accept",
            None,
            &["§25", "§27", "§52", "§72.3"],
        ),
        cove_t_nested_map_valid_file(),
    );

    let mut parquet_accept = fixture(
        "accept/parquet_primitives_valid.parquet",
        "parquet_conversion_case",
        "accept",
        None,
        &["§24", "§25", "§27", "§51", "§72.3"],
    );
    parquet_accept["table_name"] = json!("parquet_demo");
    parquet_accept["namespace"] = json!("interop");
    parquet_accept["expected_row_count"] = json!(3u64);
    parquet_accept["expected_columns"] = json!([
        {
            "name": "active",
            "logical": "Bool",
            "physical": "Boolean",
            "values": [true, false, true]
        },
        {
            "name": "id",
            "logical": "Int64",
            "physical": "NumCode",
            "values": [10, 20, 30]
        },
        {
            "name": "score",
            "logical": "Float64",
            "physical": "NumCode",
            "values": [1.5, 2.0, 3.25]
        },
        {
            "name": "city",
            "logical": "Utf8",
            "physical": "VarBytes",
            "values": ["sea", "lon", "par"]
        },
        {
            "name": "blob",
            "logical": "Binary",
            "physical": "VarBytes",
            "values": ["6161", "6262", "6363"]
        },
        {
            "name": "event_date",
            "logical": "DateDays",
            "physical": "NumCode",
            "values": [19000, 19001, 19002]
        },
        {
            "name": "ts_us",
            "logical": "TimestampMicros",
            "physical": "NumCode",
            "values": [1000, 2000, 3000]
        }
    ]);
    write_fixture(
        &root,
        &mut entries,
        parquet_accept,
        parquet_primitives_valid_file(),
    );

    let mut parquet_nullable = fixture(
        "accept/parquet_nullable_valid.parquet",
        "parquet_conversion_case",
        "accept",
        None,
        &["§6.6", "§24", "§25", "§27", "§51", "§72.3"],
    );
    parquet_nullable["table_name"] = json!("parquet_nullable");
    parquet_nullable["namespace"] = json!("interop");
    parquet_nullable["expected_row_count"] = json!(3u64);
    parquet_nullable["expected_columns"] = json!([
        {
            "name": "id",
            "logical": "Int64",
            "physical": "NumCode",
            "values": [1, null, 3]
        }
    ]);
    write_fixture(
        &root,
        &mut entries,
        parquet_nullable,
        parquet_nullable_valid_file(),
    );

    let mut parquet_nested = fixture(
        "accept/parquet_nested_json_fallback.parquet",
        "parquet_conversion_case",
        "accept",
        None,
        &["§51", "§52", "§72.3"],
    );
    parquet_nested["table_name"] = json!("parquet_nested_json");
    parquet_nested["namespace"] = json!("interop");
    parquet_nested["expected_row_count"] = json!(3u64);
    parquet_nested["expected_columns"] = json!([
        {
            "name": "tags",
            "logical": "Json",
            "physical": "VarBytes",
            "values": [[1, 2], [], [3]]
        }
    ]);
    write_fixture(
        &root,
        &mut entries,
        parquet_nested,
        parquet_nested_unsupported_file(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/column_domain_valid.bin",
            "column_domain",
            "accept",
            None,
            &["§23"],
        ),
        valid_column_domain_payload(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/table_catalog_valid.bin",
            "table_catalog",
            "accept",
            None,
            &["§24"],
        ),
        valid_table_catalog().serialize().unwrap(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/table_segment_index_valid.bin",
            "table_segment_index",
            "accept",
            None,
            &["§25"],
        ),
        valid_table_segment_index().serialize().unwrap(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/table_segment_header_valid.bin",
            "table_segment_header",
            "accept",
            None,
            &["§25"],
        ),
        valid_table_segment_header().serialize().to_vec(),
    );

    let row_morsel_valid = fixture(
        "accept/row_morsel_directory_valid.bin",
        "row_morsel_directory",
        "accept",
        None,
        &["§26"],
    );
    write_fixture(
        &root,
        &mut entries,
        with_morsel_count(row_morsel_valid, 2),
        valid_row_morsel_directory().serialize(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/sort_key_valid.bin",
            "sort_key",
            "accept",
            None,
            &["§53"],
        ),
        valid_sort_key().serialize().to_vec(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/clustering_key_valid.bin",
            "clustering_key",
            "accept",
            None,
            &["§53"],
        ),
        valid_clustering_key().serialize().to_vec(),
    );

    let mut intermediate_clustering_key = valid_clustering_key();
    intermediate_clustering_key.clustering_strength = ClusteringStrength(9);
    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/clustering_key_intermediate_strength.bin",
            "clustering_key",
            "accept",
            None,
            &["§53"],
        ),
        intermediate_clustering_key.serialize().to_vec(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/row_ref_valid.bin",
            "row_ref",
            "accept",
            None,
            &["§54"],
        ),
        RowRef {
            table_id: 1,
            segment_id: 2,
            morsel_id: 3,
            row_in_morsel: 4,
        }
        .encode()
        .to_vec(),
    );

    let covx_bytes = valid_covx_file();
    write_fixture(
        &root,
        &mut entries,
        fixture("accept/covx_valid.covx", "covx", "accept", None, &["§68"]),
        covx_bytes.clone(),
    );

    let covm_bytes = valid_covm_file();
    write_fixture(
        &root,
        &mut entries,
        fixture("accept/covm_valid.covm", "covm", "accept", None, &["§69"]),
        covm_bytes.clone(),
    );

    for (path, case) in [
        (
            "accept/sidecar_freshness_valid.json",
            SidecarFreshnessCase::Valid,
        ),
        (
            "accept/sidecar_freshness_file_id_stale.json",
            SidecarFreshnessCase::FileId,
        ),
        (
            "accept/sidecar_freshness_file_len_stale.json",
            SidecarFreshnessCase::FileLen,
        ),
        (
            "accept/sidecar_freshness_footer_crc_stale.json",
            SidecarFreshnessCase::FooterCrc,
        ),
        (
            "accept/sidecar_freshness_digest_stale.json",
            SidecarFreshnessCase::Digest,
        ),
        (
            "accept/sidecar_freshness_corrupt_ignored.json",
            SidecarFreshnessCase::Corrupt,
        ),
    ] {
        write_fixture(
            &root,
            &mut entries,
            fixture(
                path,
                "sidecar_freshness_case",
                "accept",
                None,
                &["§48", "§68", "§69"],
            ),
            sidecar_freshness_payload(case),
        );
    }

    let covemap_bytes = valid_covemap_file();
    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/covemap_valid.covemap",
            "covemap",
            "accept",
            None,
            &["§70"],
        ),
        covemap_bytes.clone(),
    );

    let mut covemap_unknown_required = covemap_bytes.clone();
    rewrite_covemap_feature_bits(
        &mut covemap_unknown_required,
        FEATURE_SEMANTIC_MAP | (1u64 << 63),
        0,
    );
    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/covemap_unknown_required_feature.covemap",
            "covemap",
            "reject",
            Some("COVE_E_UNKNOWN_REQUIRED_FEATURE"),
            &["§70", "§74", "§77", "§76"],
        ),
        covemap_unknown_required,
    );

    let mut covemap_missing_semantic_map = covemap_bytes.clone();
    rewrite_covemap_feature_bits(&mut covemap_missing_semantic_map, 0, 0);
    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/covemap_missing_semantic_map_feature.covemap",
            "covemap",
            "reject",
            Some("COVE_E_BAD_SECTION"),
            &["§11", "§70", "§76"],
        ),
        covemap_missing_semantic_map,
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/covemap_lz4_missing_feature.covemap",
            "covemap",
            "reject",
            Some("COVE_E_BAD_SECTION"),
            &["§66", "§70", "§76"],
        ),
        covemap_lz4_missing_feature_file(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/metadata_json_valid.json",
            "metadata_json",
            "accept",
            None,
            &["§15"],
        ),
        br#"{"producer":"cove-conformance","purpose":"metadata fixture"}"#.to_vec(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/encoding_constant_valid.json",
            "encoding_case",
            "accept",
            None,
            &["§20"],
        ),
        encoding_fixture_bytes(json!({
            "encoding": "constant",
            "payload": ConstantPayload { value: -42, row_count: 5 }.encode().to_vec(),
            "expect_values": [-42, -42, -42, -42, -42]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/encoding_rle_valid.json",
            "encoding_case",
            "accept",
            None,
            &["§20"],
        ),
        encoding_fixture_bytes(json!({
            "encoding": "rle",
            "payload": RlePayload { runs: vec![(1, 3), (2, 1), (1, 2)] }.encode(),
            "expect_values": [1, 1, 1, 2, 1, 1]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/encoding_run_end_valid.json",
            "encoding_case",
            "accept",
            None,
            &["§20"],
        ),
        encoding_fixture_bytes(json!({
            "encoding": "run_end",
            "payload": run_end_payload_bytes(&[10, 20, 30], &[2, 5, 6]),
            "expect_values": [10, 10, 20, 20, 20, 30]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/encoding_plain_fixed_valid.json",
            "encoding_case",
            "accept",
            None,
            &["§20"],
        ),
        encoding_fixture_bytes(json!({
            "encoding": "plain_fixed",
            "payload": PlainFixedPayload { values: vec![1, -2, 3, -4] }.encode(),
            "expect_values": [1, -2, 3, -4]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/encoding_plain_varint_valid.json",
            "encoding_case",
            "accept",
            None,
            &["§20"],
        ),
        encoding_fixture_bytes(json!({
            "encoding": "plain_varint",
            "payload": PlainVarintPayload { values: vec![0, -1, 1, -2, 2] }.encode(),
            "expect_values": [0, -1, 1, -2, 2]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/encoding_bit_packed_valid.json",
            "encoding_case",
            "accept",
            None,
            &["§20"],
        ),
        encoding_fixture_bytes(json!({
            "encoding": "bit_packed",
            "payload": BitPackedPayload::pack(&[0, 1, 2, 3, 4, 5, 6, 7, 0, 7, 4], 3).unwrap().encode(),
            "expect_values": [0, 1, 2, 3, 4, 5, 6, 7, 0, 7, 4]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/encoding_delta_valid.json",
            "encoding_case",
            "accept",
            None,
            &["§20"],
        ),
        encoding_fixture_bytes(json!({
            "encoding": "delta",
            "payload": DeltaPayload { base: 100, deltas: vec![1, 2, -3, 5] }.encode(),
            "expect_values": [100, 101, 103, 100, 105]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/encoding_frame_of_reference_valid.json",
            "encoding_case",
            "accept",
            None,
            &["§20"],
        ),
        encoding_fixture_bytes(json!({
            "encoding": "frame_of_reference",
            "payload": ForPayload { reference: 1_000_000, offsets: vec![0, 1, -2, 3, 4] }.encode(),
            "expect_values": [1_000_000, 1_000_001, 999_998, 1_000_003, 1_000_004]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/encoding_patched_base_valid.json",
            "encoding_case",
            "accept",
            None,
            &["§20"],
        ),
        encoding_fixture_bytes(json!({
            "encoding": "patched_base",
            "payload": patched_base_payload_bytes(&[0, 0, 0, 0], &[(1, 10), (3, 20)]),
            "expect_values": [0, 10, 0, 20]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/encoding_sparse_valid.json",
            "encoding_case",
            "accept",
            None,
            &["§20"],
        ),
        encoding_fixture_bytes(json!({
            "encoding": "sparse",
            "payload": sparse_payload_bytes(5, 0, &[(1, 42), (4, -7)]),
            "expect_values": [0, 42, 0, 0, -7]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/encoding_local_codebook_bit_packed_valid.json",
            "encoding_case",
            "accept",
            None,
            &["§20"],
        ),
        encoding_fixture_bytes(json!({
            "encoding": "local_codebook",
            "payload": LocalCodebookPayload {
                values: LocalCodebookValues::FileCode(vec![100, 200, 300]),
                indexes: LocalIndexPayload::BitPacked(
                    BitPackedPayload::pack(&[0, 1, 2, 1, 0], 2).unwrap(),
                ),
            }
            .encode(),
            "expect_values": [100, 200, 300, 200, 100]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/encoding_local_codebook_rle_valid.json",
            "encoding_case",
            "accept",
            None,
            &["§20"],
        ),
        encoding_fixture_bytes(json!({
            "encoding": "local_codebook",
            "payload": LocalCodebookPayload {
                values: LocalCodebookValues::NumCode(vec![7, 9]),
                indexes: LocalIndexPayload::Rle(RlePayload {
                    runs: vec![(0, 3), (1, 1), (0, 2)],
                }),
            }
            .encode(),
            "expect_values": [7, 7, 7, 9, 7, 7]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/encoded_array_decode_rle_valid.json",
            "encoded_array_decode_case",
            "accept",
            None,
            &["§20", "§72.3"],
        ),
        encoding_fixture_bytes(json!({
            "logical": "Int64",
            "physical": "FixedBytes",
            "encoding": "Rle",
            "row_count": 4,
            "payload": RlePayload { runs: vec![(-2, 2), (9, 2)] }.encode(),
            "expect": [-2, -2, 9, 9]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/encoded_array_decode_local_codebook_varbytes_valid.json",
            "encoded_array_decode_case",
            "accept",
            None,
            &["§20", "§72.3"],
        ),
        encoding_fixture_bytes(json!({
            "logical": "Utf8",
            "physical": "VarBytes",
            "encoding": "LocalCodebook",
            "row_count": 3,
            "payload": LocalCodebookPayload {
                values: LocalCodebookValues::VarBytes(vec![b"red".to_vec(), b"blue".to_vec()]),
                indexes: LocalIndexPayload::Rle(RlePayload {
                    runs: vec![(0, 1), (1, 2)],
                }),
            }
            .encode(),
            "expect": ["red", "blue", "blue"]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/arrow_export_utf8_valid.json",
            "arrow_export_case",
            "accept",
            None,
            &["§49", "§20", "§72.3"],
        ),
        encoding_fixture_bytes(json!({
            "logical": "Utf8",
            "physical": "VarBytes",
            "encoding": "VarBytes",
            "row_count": 2,
            "payload": varbytes_payload(&[b"hi".as_ref(), b"there".as_ref()]),
            "expect_type": "Utf8",
            "expect": ["hi", "there"]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/arrow_export_json_requires_report.json",
            "arrow_export_case",
            "reject",
            None,
            &["§49", "§20", "§76"],
        ),
        encoding_fixture_bytes(json!({
            "logical": "Json",
            "physical": "VarBytes",
            "encoding": "VarBytes",
            "row_count": 1,
            "payload": varbytes_payload(&[br#"{"a":1}"#.as_ref()]),
            "expect_type": "Utf8",
            "expect": ["{\"a\":1}"]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/nested_list_valid.json",
            "nested_case",
            "accept",
            None,
            &["§52"],
        ),
        nested_fixture_bytes(json!({
            "layout": "list",
            "offsets": [0, 2, 2, 5],
            "child_row_count": 5
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/nested_struct_valid.json",
            "nested_case",
            "accept",
            None,
            &["§52"],
        ),
        nested_fixture_bytes(json!({
            "layout": "struct",
            "field_row_counts": [3, 3, 3],
            "parent_row_count": 3,
            "parent_null_handling_declared": true
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/nested_map_valid.json",
            "nested_case",
            "accept",
            None,
            &["§52"],
        ),
        nested_fixture_bytes(json!({
            "layout": "map",
            "offsets": [0, 2, 3],
            "key_row_count": 3,
            "value_row_count": 3,
            "keys_are_scalar": true,
            "allow_duplicate_keys": false,
            "canonical_keys": ["a", "b", "a"]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/file_dictionary_valid.bin",
            "file_dictionary",
            "accept",
            None,
            &["§16", "§17"],
        ),
        valid_file_dictionary_fixture_payload().unwrap(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/collation_registry_valid.bin",
            "collation_registry",
            "accept",
            None,
            &["§22"],
        ),
        collation_registry_payload(&[("utf8-bytewise", b""), ("signed-numeric", b"")]),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/page_index_valid.bin",
            "page_index",
            "accept",
            None,
            &["§27"],
        ),
        page_index_payload(4, 1, CoveEncodingKind::PlainFixed as u16),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_t_zone_stats_valid.cove",
            "cove",
            "accept",
            None,
            &["§28", "§73"],
        ),
        semantic_profile_cove_file(
            PrimaryProfile::TableScan,
            FEATURE_TABLE_PROFILE,
            0,
            vec![SectionPayload {
                section_kind: SectionKind::ZoneStats as u16,
                profile: PrimaryProfile::TableScan as u8,
                flags: 0,
                item_count: 1,
                row_count: 10,
                compression: 0,
                alignment_log2: 0,
                required_features: FEATURE_TABLE_PROFILE,
                optional_features: 0,
                data: valid_zone_stats_payload(),
            }],
        ),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/pruning_null_is_null_all.json",
            "pruning_case",
            "accept",
            None,
            &["§37.4"],
        ),
        pruning_fixture_bytes(json!({
            "columns": [
                {
                    "column_id": 7,
                    "zone_stats": {
                        "row_count": 10,
                        "null_count": 10
                    }
                }
            ],
            "predicate": {
                "op": "is_null",
                "column_id": 7
            },
            "expect_outcome": "all_match",
            "expect_evidence": ["ZoneStats"]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/pruning_file_code_eq_exact_set_no.json",
            "pruning_case",
            "accept",
            None,
            &["§37.1"],
        ),
        pruning_fixture_bytes(json!({
            "columns": [
                {
                    "column_id": 7,
                    "exact_set": {
                        "keys": [1, 4, 7]
                    }
                }
            ],
            "predicate": {
                "op": "file_code_eq",
                "column_id": 7,
                "file_code": 3
            },
            "expect_outcome": "no_match",
            "expect_evidence": ["ExactSet"]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/pruning_file_code_eq_constant_yes.json",
            "pruning_case",
            "accept",
            None,
            &["§37.1"],
        ),
        pruning_fixture_bytes(json!({
            "columns": [
                {
                    "column_id": 7,
                    "zone_stats": {
                        "row_count": 5,
                        "null_count": 0,
                        "flags": ["has_domain_range", "constant"],
                        "min_domain_rank": 1,
                        "max_domain_rank": 1
                    },
                    "column_domain": {
                        "sorted_file_codes": [1, 3, 4, 7],
                        "dictionary_entry_count": 8
                    }
                }
            ],
            "predicate": {
                "op": "file_code_eq",
                "column_id": 7,
                "file_code": 3
            },
            "expect_outcome": "all_match",
            "expect_evidence": ["ColumnDomain"]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/pruning_domain_rank_range_overlap.json",
            "pruning_case",
            "accept",
            None,
            &["§37.2"],
        ),
        pruning_fixture_bytes(json!({
            "columns": [
                {
                    "column_id": 7,
                    "zone_stats": {
                        "row_count": 8,
                        "null_count": 0,
                        "flags": ["has_domain_range"],
                        "min_domain_rank": 1,
                        "max_domain_rank": 2
                    },
                    "column_domain": {
                        "sorted_file_codes": [1, 3, 4, 7],
                        "dictionary_entry_count": 8
                    }
                }
            ],
            "predicate": {
                "op": "domain_rank_range",
                "column_id": 7,
                "min_rank": 2,
                "max_rank": 3
            },
            "expect_outcome": "some_match",
            "expect_evidence": ["ColumnDomain"]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/pruning_domain_rank_range_unsafe_domain.json",
            "pruning_case",
            "accept",
            None,
            &["§37.2", "§73"],
        ),
        pruning_fixture_bytes(json!({
            "columns": [
                {
                    "column_id": 7,
                    "zone_stats": {
                        "row_count": 8,
                        "null_count": 0,
                        "flags": ["has_domain_range"],
                        "min_domain_rank": 1,
                        "max_domain_rank": 2
                    },
                    "column_domain": {
                        "sorted_file_codes": [1, 3, 4, 7],
                        "dictionary_entry_count": 8,
                        "safe": false
                    }
                }
            ],
            "predicate": {
                "op": "domain_rank_range",
                "column_id": 7,
                "min_rank": 1,
                "max_rank": 2
            },
            "expect_outcome": "unknown",
            "expect_evidence": ["FallbackToScan"]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/pruning_truth_table_and.json",
            "pruning_case",
            "accept",
            None,
            &["§29", "§37.2", "§37.4"],
        ),
        pruning_fixture_bytes(json!({
            "columns": [
                {
                    "column_id": 7,
                    "zone_stats": {
                        "row_count": 10,
                        "null_count": 0
                    }
                },
                {
                    "column_id": 8,
                    "zone_stats": {
                        "row_count": 8,
                        "null_count": 0,
                        "flags": ["has_domain_range"],
                        "min_domain_rank": 1,
                        "max_domain_rank": 2
                    },
                    "column_domain": {
                        "sorted_file_codes": [1, 3, 4, 7],
                        "dictionary_entry_count": 8
                    }
                }
            ],
            "predicate": {
                "op": "and",
                "operands": [
                    {
                        "op": "is_not_null",
                        "column_id": 7
                    },
                    {
                        "op": "domain_rank_range",
                        "column_id": 8,
                        "min_rank": 2,
                        "max_rank": 3
                    }
                ]
            },
            "expect_outcome": "some_match",
            "expect_evidence": ["ZoneStats", "ColumnDomain"]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/pruning_truth_table_or.json",
            "pruning_case",
            "accept",
            None,
            &["§29", "§37.1", "§37.4"],
        ),
        pruning_fixture_bytes(json!({
            "columns": [
                {
                    "column_id": 7,
                    "exact_set": {
                        "keys": [1, 4, 7]
                    }
                },
                {
                    "column_id": 8,
                    "zone_stats": {
                        "row_count": 6,
                        "null_count": 2
                    }
                }
            ],
            "predicate": {
                "op": "or",
                "operands": [
                    {
                        "op": "file_code_eq",
                        "column_id": 7,
                        "file_code": 3
                    },
                    {
                        "op": "is_null",
                        "column_id": 8
                    }
                ]
            },
            "expect_outcome": "some_match",
            "expect_evidence": ["ExactSet", "ZoneStats"]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/pruning_truth_table_not.json",
            "pruning_case",
            "accept",
            None,
            &["§29", "§37.1"],
        ),
        pruning_fixture_bytes(json!({
            "columns": [
                {
                    "column_id": 7,
                    "exact_set": {
                        "keys": [1, 4, 7]
                    }
                }
            ],
            "predicate": {
                "op": "not",
                "operand": {
                    "op": "file_code_eq",
                    "column_id": 7,
                    "file_code": 3
                }
            },
            "expect_outcome": "all_match",
            "expect_evidence": ["ExactSet"]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/pruning_numcode_range_all.json",
            "pruning_case",
            "accept",
            None,
            &["§29", "§37.3"],
        ),
        pruning_fixture_bytes(json!({
            "columns": [
                {
                    "column_id": 9,
                    "zone_stats": {
                        "row_count": 8,
                        "null_count": 0,
                        "flags": ["has_min_max"],
                        "min": { "kind": "int64", "value": 22 },
                        "max": { "kind": "int64", "value": 51 }
                    }
                }
            ],
            "predicate": {
                "op": "numcode_range",
                "column_id": 9,
                "lower": { "kind": "int64", "value": 18 },
                "upper": { "kind": "int64", "value": 65 }
            },
            "expect_outcome": "all_match",
            "expect_evidence": ["ZoneStats"]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/pruning_numcode_range_no.json",
            "pruning_case",
            "accept",
            None,
            &["§29", "§37.3"],
        ),
        pruning_fixture_bytes(json!({
            "columns": [
                {
                    "column_id": 9,
                    "zone_stats": {
                        "row_count": 8,
                        "null_count": 0,
                        "flags": ["has_min_max"],
                        "min": { "kind": "int64", "value": 22 },
                        "max": { "kind": "int64", "value": 51 }
                    }
                }
            ],
            "predicate": {
                "op": "numcode_range",
                "column_id": 9,
                "lower": { "kind": "int64", "value": 70 },
                "upper": { "kind": "int64", "value": 90 }
            },
            "expect_outcome": "no_match",
            "expect_evidence": ["ZoneStats"]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/pruning_numcode_range_overlap.json",
            "pruning_case",
            "accept",
            None,
            &["§29", "§37.3"],
        ),
        pruning_fixture_bytes(json!({
            "columns": [
                {
                    "column_id": 9,
                    "zone_stats": {
                        "row_count": 8,
                        "null_count": 0,
                        "flags": ["has_min_max"],
                        "min": { "kind": "int64", "value": 22 },
                        "max": { "kind": "int64", "value": 51 }
                    }
                }
            ],
            "predicate": {
                "op": "numcode_range",
                "column_id": 9,
                "lower": { "kind": "int64", "value": 40 },
                "upper": { "kind": "int64", "value": 90 }
            },
            "expect_outcome": "some_match",
            "expect_evidence": ["ZoneStats"]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/pruning_numcode_range_nan_unknown.json",
            "pruning_case",
            "accept",
            None,
            &["§28", "§37.3"],
        ),
        pruning_fixture_bytes(json!({
            "columns": [
                {
                    "column_id": 9,
                    "zone_stats": {
                        "row_count": 8,
                        "null_count": 0,
                        "flags": ["has_min_max", "has_nan"],
                        "min": { "kind": "float64", "value": 1.0 },
                        "max": { "kind": "float64", "value": 2.0 }
                    }
                }
            ],
            "predicate": {
                "op": "numcode_range",
                "column_id": 9,
                "lower": { "kind": "float64", "value": 0.0 },
                "upper": { "kind": "float64", "value": 3.0 }
            },
            "expect_outcome": "unknown",
            "expect_evidence": ["FallbackToScan"]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/pruning_numcode_range_truncated_unknown.json",
            "pruning_case",
            "accept",
            None,
            &["§28", "§37.3"],
        ),
        pruning_fixture_bytes(json!({
            "columns": [
                {
                    "column_id": 9,
                    "zone_stats": {
                        "row_count": 8,
                        "null_count": 0,
                        "flags": ["has_min_max", "minmax_truncated"],
                        "min": { "kind": "int64", "value": 1 },
                        "max": { "kind": "int64", "value": 2 }
                    }
                }
            ],
            "predicate": {
                "op": "numcode_range",
                "column_id": 9,
                "lower": { "kind": "int64", "value": 0 },
                "upper": { "kind": "int64", "value": 3 }
            },
            "expect_outcome": "unknown",
            "expect_evidence": ["FallbackToScan"]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/pruning_bloom_membership_no.json",
            "pruning_case",
            "accept",
            None,
            &["§31", "§37.1"],
        ),
        pruning_fixture_bytes(json!({
            "columns": [
                {
                    "column_id": 11,
                    "bloom": { "values": ["alpha", "beta", "gamma"], "bit_count": 64 }
                }
            ],
            "predicate": { "op": "bloom_membership", "column_id": 11, "value": "delta" },
            "expect_outcome": "no_match",
            "expect_evidence": ["BloomFilter"]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/pruning_bloom_membership_fallback.json",
            "pruning_case",
            "accept",
            None,
            &["§31", "§73"],
        ),
        pruning_fixture_bytes(json!({
            "columns": [
                {
                    "column_id": 11,
                    "bloom": { "values": ["alpha"], "bit_count": 64, "fail_open": true }
                }
            ],
            "predicate": { "op": "bloom_membership", "column_id": 11, "value": "alpha" },
            "expect_outcome": "unknown",
            "expect_evidence": ["FallbackToScan"]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/pruning_inverted_lookup_no.json",
            "pruning_case",
            "accept",
            None,
            &["§32", "§37.1"],
        ),
        pruning_fixture_bytes(json!({
            "columns": [
                { "column_id": 12, "inverted": { "keys": [3, 5, 7] } }
            ],
            "predicate": { "op": "inverted_lookup", "column_id": 12, "key": 4 },
            "expect_outcome": "no_match",
            "expect_evidence": ["InvertedIndex"]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/pruning_inverted_lookup_fallback.json",
            "pruning_case",
            "accept",
            None,
            &["§32", "§73"],
        ),
        pruning_fixture_bytes(json!({
            "columns": [
                { "column_id": 12, "inverted": { "keys": [3], "fail_open": true } }
            ],
            "predicate": { "op": "inverted_lookup", "column_id": 12, "key": 3 },
            "expect_outcome": "unknown",
            "expect_evidence": ["FallbackToScan"]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/pruning_lookup_point_no.json",
            "pruning_case",
            "accept",
            None,
            &["§33", "§37.1"],
        ),
        pruning_fixture_bytes(json!({
            "columns": [
                { "column_id": 13, "lookup": { "keys": [10, 20, 30] } }
            ],
            "predicate": { "op": "lookup_point", "column_id": 13, "key": 15 },
            "expect_outcome": "no_match",
            "expect_evidence": ["InvertedIndex"]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/pruning_lookup_point_fallback.json",
            "pruning_case",
            "accept",
            None,
            &["§33", "§73"],
        ),
        pruning_fixture_bytes(json!({
            "columns": [
                { "column_id": 13, "lookup": { "keys": [10], "fail_open": true } }
            ],
            "predicate": { "op": "lookup_point", "column_id": 13, "key": 10 },
            "expect_outcome": "unknown",
            "expect_evidence": ["FallbackToScan"]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/pruning_aggregate_synopsis_no.json",
            "pruning_case",
            "accept",
            None,
            &["§34", "§37.1"],
        ),
        pruning_fixture_bytes(json!({
            "columns": [
                { "column_id": 14, "aggregate": { "proves_no_match": true } }
            ],
            "predicate": { "op": "aggregate_synopsis", "column_id": 14 },
            "expect_outcome": "no_match",
            "expect_evidence": ["AggregateSynopsis"]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/pruning_aggregate_synopsis_fallback.json",
            "pruning_case",
            "accept",
            None,
            &["§34", "§73"],
        ),
        pruning_fixture_bytes(json!({
            "columns": [
                { "column_id": 14, "aggregate": { "proves_no_match": true, "fail_open": true } }
            ],
            "predicate": { "op": "aggregate_synopsis", "column_id": 14 },
            "expect_outcome": "unknown",
            "expect_evidence": ["FallbackToScan"]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/pruning_composite_zone_no.json",
            "pruning_case",
            "accept",
            None,
            &["§35", "§37.1"],
        ),
        pruning_fixture_bytes(json!({
            "columns": [
                { "column_id": 15, "composite": { "matches_bindings": false } }
            ],
            "predicate": { "op": "composite_zone", "column_id": 15 },
            "expect_outcome": "no_match",
            "expect_evidence": ["CompositeIndex"]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/pruning_composite_zone_fallback.json",
            "pruning_case",
            "accept",
            None,
            &["§35", "§73"],
        ),
        pruning_fixture_bytes(json!({
            "columns": [
                { "column_id": 15, "composite": { "matches_bindings": true, "fail_open": true } }
            ],
            "predicate": { "op": "composite_zone", "column_id": 15 },
            "expect_outcome": "unknown",
            "expect_evidence": ["FallbackToScan"]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/pruning_reorder_invariant_and.json",
            "pruning_case",
            "accept",
            None,
            &["§37.5"],
        ),
        pruning_fixture_bytes(json!({
            "columns": [
                {
                    "column_id": 21,
                    "zone_stats": { "row_count": 4, "null_count": 0, "flags": [] }
                },
                {
                    "column_id": 22,
                    "zone_stats": {
                        "row_count": 4,
                        "null_count": 0,
                        "flags": ["has_min_max"],
                        "min": { "kind": "int64", "value": 10 },
                        "max": { "kind": "int64", "value": 20 }
                    }
                },
                {
                    "column_id": 23,
                    "exact_set": { "keys": [1, 2, 3] }
                }
            ],
            "predicate": {
                "op": "reorder_invariant_and",
                "operands": [
                    { "op": "is_not_null", "column_id": 21 },
                    {
                        "op": "numcode_range",
                        "column_id": 22,
                        "lower": { "kind": "int64", "value": 8 },
                        "upper": { "kind": "int64", "value": 25 }
                    },
                    { "op": "file_code_eq", "column_id": 23, "file_code": 7 }
                ]
            },
            "expect_outcome": "no_match"
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/pruning_reorder_invariant_or.json",
            "pruning_case",
            "accept",
            None,
            &["§37.5"],
        ),
        pruning_fixture_bytes(json!({
            "columns": [
                {
                    "column_id": 31,
                    "zone_stats": { "row_count": 6, "null_count": 0, "flags": [] }
                },
                {
                    "column_id": 32,
                    "zone_stats": { "row_count": 6, "null_count": 6, "flags": [] }
                },
                {
                    "column_id": 33,
                    "exact_set": { "keys": [1, 2, 3] }
                }
            ],
            "predicate": {
                "op": "reorder_invariant_or",
                "operands": [
                    { "op": "is_not_null", "column_id": 31 },
                    { "op": "is_null", "column_id": 32 },
                    { "op": "file_code_eq", "column_id": 33, "file_code": 99 }
                ]
            },
            "expect_outcome": "all_match"
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/page_codec_none_round_trip.json",
            "page_codec_case",
            "accept",
            None,
            &["§27", "§66"],
        ),
        page_codec_fixture_bytes(json!({
            "codec": "none",
            "payload": "uncompressed page bytes",
            "expect": "round_trip"
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/page_codec_lz4_round_trip.json",
            "page_codec_case",
            "accept",
            None,
            &["§27", "§66"],
        ),
        page_codec_fixture_bytes(json!({
            "codec": "lz4",
            "payload": "Cove page-level LZ4 round trip Cove page-level LZ4 round trip",
            "expect": "round_trip"
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/page_codec_zstd_round_trip.json",
            "page_codec_case",
            "accept",
            None,
            &["§27", "§66"],
        ),
        page_codec_fixture_bytes(json!({
            "codec": "zstd",
            "payload": "Cove page-level Zstd round trip Cove page-level Zstd round trip",
            "expect": "round_trip"
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/page_codec_none_length_mismatch_rejected.json",
            "page_codec_case",
            "accept",
            None,
            &["§13.2", "§27.2", "§66"],
        ),
        page_codec_fixture_bytes(json!({
            "codec": "none",
            "payload": "abcdef",
            // codec=None requires uncompressed_length == page_length.
            "uncompressed_length_override": 99,
            "expect": "parse_reject"
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/page_codec_unknown_codec_rejected.json",
            "page_codec_case",
            "accept",
            None,
            &["§27.2", "§66"],
        ),
        page_codec_fixture_bytes(json!({
            "codec": "none",
            "payload": "abcdef",
            // 0xFF is not a known CompressionCodec value.
            "flags_override": 0xFFu64,
            "expect": "parse_reject"
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/page_codec_reserved_flag_bits_rejected.json",
            "page_codec_case",
            "accept",
            None,
            &["§27.2", "§66"],
        ),
        page_codec_fixture_bytes(json!({
            "codec": "none",
            "payload": "abcdef",
            // Reserved bits above the codec byte must be zero in v1.0.
            "flags_override": 0x0000_1000u64,
            "expect": "parse_reject"
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/page_codec_stats_only_constant_all_null_round_trip.json",
            "page_codec_case",
            "accept",
            None,
            &["§27.2", "§66"],
        ),
        page_codec_fixture_bytes(json!({
            "codec": "none",
            "payload": "",
            "flags_override": 0x0000_0300u64,
            "row_count_override": 1u64,
            "non_null_count_override": 0u64,
            "null_count_override": 1u64,
            "encoding_root_override": 0xFFFF_FFFFu64,
            "page_offset_override": 0u64,
            "page_length_override": 0u64,
            "uncompressed_length_override": 0u64,
            "expect": "round_trip"
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/page_codec_stats_only_constant_requires_empty_payload.json",
            "page_codec_case",
            "accept",
            None,
            &["§27.2", "§66"],
        ),
        page_codec_fixture_bytes(json!({
            "codec": "none",
            "payload": "abcdef",
            "flags_override": 0x0000_0300u64,
            "row_count_override": 1u64,
            "non_null_count_override": 0u64,
            "null_count_override": 1u64,
            "encoding_root_override": 0xFFFF_FFFFu64,
            "page_offset_override": 0u64,
            "expect": "parse_reject"
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/page_codec_lz4_truncated_rejected.json",
            "page_codec_case",
            "accept",
            None,
            &["§27", "§66"],
        ),
        page_codec_fixture_bytes(json!({
            "codec": "lz4",
            "payload": "Cove page-level LZ4 corruption sentinel sentinel sentinel",
            "truncate_wire_bytes": 4,
            "expect": "decode_reject"
        })),
    );

    // §8 — wire-format primitives (varint LEB128, ZigZag, strict bool).
    let wire_fixtures: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "accept/wire_varint_zero.json",
            json!({ "op": "varint_round_trip", "value": 0u64, "expect_bytes": [0u8] }),
            vec!["§8"],
        ),
        (
            "accept/wire_varint_127.json",
            json!({ "op": "varint_round_trip", "value": 127u64, "expect_bytes": [0x7fu8] }),
            vec!["§8"],
        ),
        (
            "accept/wire_varint_128.json",
            json!({ "op": "varint_round_trip", "value": 128u64, "expect_bytes": [0x80u8, 0x01u8] }),
            vec!["§8"],
        ),
        (
            "accept/wire_varint_u32_max.json",
            json!({
                "op": "varint_round_trip",
                "value": 0xFFFF_FFFFu64,
                "expect_bytes": [0xffu8, 0xffu8, 0xffu8, 0xffu8, 0x0fu8]
            }),
            vec!["§8"],
        ),
        (
            "accept/wire_varint_u64_max.json",
            json!({
                "op": "varint_round_trip",
                "value": u64::MAX,
                "expect_bytes": [0xffu8, 0xffu8, 0xffu8, 0xffu8, 0xffu8, 0xffu8, 0xffu8, 0xffu8, 0xffu8, 0x01u8]
            }),
            vec!["§8"],
        ),
        (
            "accept/wire_varint_truncated_rejected.json",
            json!({
                "op": "varint_decode_reject",
                "input": [0x80u8],
                "reason": "continuation bit set but no following byte"
            }),
            vec!["§8"],
        ),
        (
            "accept/wire_varint_overlong_rejected.json",
            json!({
                "op": "varint_decode_reject",
                "input": [0x80u8, 0x80u8, 0x80u8, 0x80u8, 0x80u8, 0x80u8, 0x80u8, 0x80u8, 0x80u8, 0x80u8, 0x01u8],
                "reason": "11-byte varint overflows u64"
            }),
            vec!["§8"],
        ),
        (
            "accept/wire_varint_10byte_overflow_rejected.json",
            json!({
                "op": "varint_decode_reject",
                "input": [0x80u8, 0x80u8, 0x80u8, 0x80u8, 0x80u8, 0x80u8, 0x80u8, 0x80u8, 0x80u8, 0x02u8],
                "reason": "10th-byte high bits would shift past bit 63"
            }),
            vec!["§8"],
        ),
        (
            "accept/wire_zigzag_zero.json",
            json!({ "op": "zigzag_round_trip", "value": 0i64, "expect_zigzag": 0u64 }),
            vec!["§8"],
        ),
        (
            "accept/wire_zigzag_negative_one.json",
            json!({ "op": "zigzag_round_trip", "value": -1i64, "expect_zigzag": 1u64 }),
            vec!["§8"],
        ),
        (
            "accept/wire_zigzag_positive_one.json",
            json!({ "op": "zigzag_round_trip", "value": 1i64, "expect_zigzag": 2u64 }),
            vec!["§8"],
        ),
        (
            "accept/wire_zigzag_i64_min.json",
            json!({ "op": "zigzag_round_trip", "value": i64::MIN, "expect_zigzag": u64::MAX }),
            vec!["§8"],
        ),
        (
            "accept/wire_zigzag_i64_max.json",
            json!({
                "op": "zigzag_round_trip",
                "value": i64::MAX,
                "expect_zigzag": (u64::MAX - 1)
            }),
            vec!["§8"],
        ),
        (
            "accept/wire_bool_strict_false.json",
            json!({ "op": "bool_strict", "byte": 0u8, "expect": false }),
            vec!["§8"],
        ),
        (
            "accept/wire_bool_strict_true.json",
            json!({ "op": "bool_strict", "byte": 1u8, "expect": true }),
            vec!["§8"],
        ),
        (
            "accept/wire_bool_strict_two_rejected.json",
            json!({ "op": "bool_strict_reject", "byte": 2u8 }),
            vec!["§8"],
        ),
        (
            "accept/wire_bool_strict_high_bit_rejected.json",
            json!({ "op": "bool_strict_reject", "byte": 0xffu8 }),
            vec!["§8"],
        ),
    ];
    for (path, body, sections) in wire_fixtures {
        let section_refs: Vec<&str> = sections;
        write_fixture(
            &root,
            &mut entries,
            fixture(path, "wire_primitive_case", "accept", None, &section_refs),
            page_codec_fixture_bytes(body),
        );
    }

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/digest_manifest_valid.bin",
            "digest_manifest",
            "accept",
            None,
            &["§65"],
        ),
        digest_manifest_payload(7, DigestAlgorithm::Sha256, b"payload").unwrap(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/redaction_manifest_valid.bin",
            "redaction_manifest",
            "accept",
            None,
            &["§64"],
        ),
        redaction_manifest_payload(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/io_hints_valid.bin",
            "io_hints",
            "accept",
            None,
            &["§67"],
        ),
        defaults_object_store().encode().to_vec(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/lakehouse_hints_valid.bin",
            "lakehouse_hints",
            "accept",
            None,
            &["§50"],
        ),
        lakehouse_hints_payload("catalog://cove", "generated"),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/lakehouse_overlay_guard_valid.bin",
            "lakehouse_overlay_guard_case",
            "accept",
            None,
            &["§50"],
        ),
        lakehouse_overlay_guard_payload(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/arrow_bitmap_cove_to_arrow_valid.json",
            "arrow_bitmap_case",
            "accept",
            None,
            &["§49"],
        ),
        arrow_bitmap_fixture_bytes(json!({
            "op": "cove_to_arrow",
            "row_count": 8,
            "input": [10],
            "expect": [245]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/arrow_bitmap_arrow_to_cove_partial_valid.json",
            "arrow_bitmap_case",
            "accept",
            None,
            &["§49"],
        ),
        arrow_bitmap_fixture_bytes(json!({
            "op": "arrow_to_cove",
            "row_count": 4,
            "input": [5],
            "expect": [10]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/arrow_bitmap_cove_short.json",
            "arrow_bitmap_case",
            "reject",
            Some("COVE_E_OFFSET_RANGE"),
            &["§49"],
        ),
        arrow_bitmap_fixture_bytes(json!({
            "op": "cove_to_arrow",
            "row_count": 1,
            "input": [],
            "expect": []
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/arrow_bitmap_arrow_short.json",
            "arrow_bitmap_case",
            "reject",
            Some("COVE_E_OFFSET_RANGE"),
            &["§49"],
        ),
        arrow_bitmap_fixture_bytes(json!({
            "op": "arrow_to_cove",
            "row_count": 1,
            "input": [],
            "expect": []
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/kernel_capabilities_valid.bin",
            "kernel_capabilities",
            "accept",
            None,
            &["§21"],
        ),
        kernel_capabilities_payload(CoveEncodingKind::Rle as u16),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/exact_set_index_valid.bin",
            "exact_set_index",
            "accept",
            None,
            &["§30"],
        ),
        exact_set_index_payload(&[2, 5, 9]),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/bloom_index_valid.bin",
            "bloom_index",
            "accept",
            None,
            &["§31"],
        ),
        bloom_index_payload(1, 64),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/inverted_morsel_index_valid.bin",
            "inverted_morsel_index",
            "accept",
            None,
            &["§32"],
        ),
        inverted_index_payload(&[5]),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/lookup_index_valid.bin",
            "lookup_index",
            "accept",
            None,
            &["§33", "§54"],
        ),
        lookup_index_payload(&[RowRef {
            table_id: 1,
            segment_id: 0,
            morsel_id: 0,
            row_in_morsel: 2,
        }]),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/aggregate_synopsis_valid.bin",
            "aggregate_synopsis",
            "accept",
            None,
            &["§34"],
        ),
        aggregate_synopsis_payload(123),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/composite_zone_index_valid.bin",
            "composite_zone_index",
            "accept",
            None,
            &["§35"],
        ),
        composite_index_payload(1),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/topn_summary_valid.bin",
            "topn_summary",
            "accept",
            None,
            &["§36"],
        ),
        topn_summary_payload(&[(1, 100), (2, 50)]),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_e_engine_registry_valid.bin",
            "cove_e_engine_registry",
            "accept",
            None,
            &["§39"],
        ),
        engine_registry_payload(&["org.example"]).unwrap(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_e_execution_code_valid.bin",
            "cove_e_execution_code",
            "accept",
            None,
            &["§40"],
        ),
        valid_execution_descriptor().serialize().to_vec(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_e_execution_scope_valid.bin",
            "cove_e_execution_scope",
            "accept",
            None,
            &["§41"],
        ),
        valid_execution_scope_descriptor().serialize().unwrap(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_e_code_space_valid.bin",
            "cove_e_code_space",
            "accept",
            None,
            &["§42"],
        ),
        valid_code_space_descriptor().serialize().unwrap(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_e_mount_policy_valid.bin",
            "cove_e_mount_policy",
            "accept",
            None,
            &["§43"],
        ),
        valid_mount_policy().serialize().to_vec(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_h_mount_hints_valid.bin",
            "cove_h_mount_hints",
            "accept",
            None,
            &["§44"],
        ),
        valid_harbor_mount_hints().serialize().to_vec(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_h_mount_rebuild_reuse.cove",
            "cove_h_mount_case",
            "accept",
            None,
            &["§44", "§48", "§73"],
        ),
        cove_h_mount_case_file(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_o_object_catalog_valid.bin",
            "cove_o_object_catalog",
            "accept",
            None,
            &["§56", "§61"],
        ),
        valid_object_catalog().serialize().unwrap(),
    );
    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_o_object_catalog_old_layout.bin",
            "cove_o_object_catalog",
            "reject",
            Some("COVE_E_OFFSET_RANGE"),
            &["§56", "§76"],
        ),
        old_layout_object_catalog_bytes(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_o_temporal_segment_index_valid.bin",
            "cove_o_temporal_segment_index",
            "accept",
            None,
            &["§57"],
        ),
        valid_temporal_segment_index().serialize().unwrap(),
    );

    let valid_temporal_rows = valid_temporal_rows();
    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_o_temporal_valid.cove",
            "cove",
            "accept",
            None,
            &["§58", "§60", "§73"],
        ),
        semantic_profile_cove_file(
            PrimaryProfile::ObjectTemporal,
            FEATURE_OBJECT_PROFILE,
            0,
            vec![
                cove_o_object_catalog_section(),
                cove_o_temporal_segment_index_section(&[(5, &valid_temporal_rows)]),
                cove_o_temporal_segment_data_section(5, &valid_temporal_rows),
            ],
        ),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_o_trust_manifest_valid.cove",
            "cove",
            "accept",
            None,
            &["§63", "§73"],
        ),
        semantic_profile_cove_file(
            PrimaryProfile::ObjectTemporal,
            FEATURE_OBJECT_PROFILE | FEATURE_TRUST_CHAIN,
            0,
            vec![
                cove_o_object_catalog_section(),
                cove_o_temporal_segment_index_section(&[(5, &valid_temporal_rows)]),
                cove_o_temporal_segment_data_section(5, &valid_temporal_rows),
                SectionPayload {
                    section_kind: SectionKind::TrustManifest as u16,
                    profile: PrimaryProfile::ObjectTemporal as u8,
                    flags: 0,
                    item_count: valid_temporal_rows.len() as u64,
                    row_count: valid_temporal_rows.len() as u64,
                    compression: 0,
                    alignment_log2: 0,
                    required_features: FEATURE_TRUST_CHAIN,
                    optional_features: 0,
                    data: trust_manifest_payload(5, &valid_temporal_rows),
                },
            ],
        ),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/extension_registry_valid.bin",
            "extension_registry",
            "accept",
            None,
            &["§45"],
        ),
        extension_registry_valid_payload(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/extension_registry_bad_crc.bin",
            "extension_registry",
            "reject",
            Some("COVE_E_CHECKSUM_MISMATCH"),
            &["§45"],
        ),
        extension_registry_bad_crc_payload(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/extension_registry_reserved.bin",
            "extension_registry",
            "reject",
            Some("COVE_E_BAD_SECTION"),
            &["§45"],
        ),
        extension_registry_reserved_payload(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/extension_registry_trailing.bin",
            "extension_registry",
            "reject",
            Some("COVE_E_BAD_EXTENSION"),
            &["§45"],
        ),
        extension_registry_trailing_payload(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/extension_registry_required_unknown.bin",
            "extension_registry",
            "reject",
            Some("COVE_E_BAD_EXTENSION"),
            &["§45", "§77"],
        ),
        extension_registry_required_unknown_payload(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/extension_registry_physical_no_fallback.bin",
            "extension_registry",
            "reject",
            Some("COVE_E_BAD_EXTENSION"),
            &["§45", "§76"],
        ),
        extension_registry_optional_no_fallback_payload(ExtensionKind::PhysicalKind),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/extension_registry_encoding_no_fallback.bin",
            "extension_registry",
            "reject",
            Some("COVE_E_BAD_EXTENSION"),
            &["§45", "§76"],
        ),
        extension_registry_optional_no_fallback_payload(ExtensionKind::Encoding),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/extension_registry_compression_no_fallback.bin",
            "extension_registry",
            "reject",
            Some("COVE_E_BAD_EXTENSION"),
            &["§45", "§76"],
        ),
        extension_registry_optional_no_fallback_payload(ExtensionKind::CompressionCodec),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/extension_logical_type_patient_id.bin",
            "extension_logical_type",
            "accept",
            None,
            &["§46"],
        ),
        extension_logical_type_payload(0),
    );

    write_fixture(
        &root,
        &mut entries,
        with_collation_count(
            fixture(
                "reject/extension_logical_type_bad_collation.bin",
                "extension_logical_type",
                "reject",
                Some("COVE_E_BAD_EXTENSION"),
                &["§46"],
            ),
            1,
        ),
        extension_logical_type_payload(2),
    );

    write_fixture(
        &root,
        &mut entries,
        with_expect_can_skip(
            fixture(
                "accept/extension_index_false_negative_non_skipping.bin",
                "extension_index_descriptor",
                "accept",
                None,
                &["§47"],
            ),
            false,
        ),
        extension_index_descriptor_payload(
            ExtensionProofCapability::None,
            ExtensionFalseNegativePolicy::MayHaveFalseNegatives,
        ),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/extension_index_false_negative_proof_claim.bin",
            "extension_index_descriptor",
            "reject",
            Some("COVE_E_BAD_EXTENSION"),
            &["§47"],
        ),
        extension_index_descriptor_payload(
            ExtensionProofCapability::DefinitelyNo,
            ExtensionFalseNegativePolicy::MayHaveFalseNegatives,
        ),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_o_temporal_bloom_valid.bin",
            "cove_o_temporal_bloom_index",
            "accept",
            None,
            &["§62"],
        ),
        temporal_bloom_payload(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_o_temporal_bloom_bad_crc.bin",
            "cove_o_temporal_bloom_index",
            "reject",
            Some("COVE_E_CHECKSUM_MISMATCH"),
            &["§62"],
        ),
        temporal_bloom_bad_crc_payload(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_o_temporal_bloom_filter_oob.bin",
            "cove_o_temporal_bloom_index",
            "reject",
            Some("COVE_E_OFFSET_RANGE"),
            &["§62"],
        ),
        temporal_bloom_filter_oob_payload(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_o_temporal_bloom_inverted_bucket.bin",
            "cove_o_temporal_bloom_index",
            "reject",
            Some("COVE_E_BAD_INDEX"),
            &["§62"],
        ),
        temporal_bloom_inverted_bucket_payload(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/durable_publish_replace.json",
            "durable_publish_case",
            "accept",
            None,
            &["§75"],
        ),
        suite_contract_fixture_bytes(json!({
            "case_id": "replace",
            "payload": "durable-cove-candidate"
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_unknown_optional_feature.cove",
            "cove",
            "accept",
            None,
            &["§74", "§77"],
        ),
        cove_with_unknown_optional_feature(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_e_optional_bad_descriptor.cove",
            "cove",
            "accept",
            None,
            &["§40", "§74", "§77"],
        ),
        profile_cove_file(
            0,
            FEATURE_ENGINE_PROFILE,
            SectionKind::ExecutionCodeDescriptor,
            PrimaryProfile::EngineExecution,
            0,
            FEATURE_ENGINE_PROFILE,
            invalid_execution_descriptor_payload(),
        ),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_e_lz4_valid.cove",
            "cove",
            "accept",
            None,
            &["§40", "§66", "§73"],
        ),
        compressed_profile_cove_file(
            FEATURE_ENGINE_PROFILE,
            FEATURE_CODEC_LZ4,
            SectionKind::ExecutionCodeDescriptor,
            PrimaryProfile::EngineExecution,
            FEATURE_ENGINE_PROFILE,
            FEATURE_CODEC_LZ4,
            CompressionCodec::Lz4,
            valid_execution_descriptor().serialize().to_vec(),
        ),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_e_zstd_valid.cove",
            "cove",
            "accept",
            None,
            &["§40", "§66", "§73"],
        ),
        compressed_profile_cove_file(
            FEATURE_ENGINE_PROFILE,
            FEATURE_CODEC_ZSTD,
            SectionKind::ExecutionCodeDescriptor,
            PrimaryProfile::EngineExecution,
            FEATURE_ENGINE_PROFILE,
            FEATURE_CODEC_ZSTD,
            CompressionCodec::Zstd,
            valid_execution_descriptor().serialize().to_vec(),
        ),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_e_profile_bundle_valid.cove",
            "cove",
            "accept",
            None,
            &["§39", "§40", "§41", "§42", "§43", "§73"],
        ),
        cove_e_profile_bundle_file(true, false),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_e_optional_bad_refs.cove",
            "cove",
            "accept",
            None,
            &["§39", "§40", "§41", "§42", "§43", "§74", "§77"],
        ),
        cove_e_profile_bundle_file(false, true),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_h_optional_bad_hints.cove",
            "cove",
            "accept",
            None,
            &["§44", "§74", "§77"],
        ),
        profile_cove_file(
            0,
            FEATURE_HARBOR_PROFILE,
            SectionKind::HarborMountHints,
            PrimaryProfile::HarborExecution,
            0,
            FEATURE_HARBOR_PROFILE,
            invalid_harbor_mount_hints_payload(),
        ),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_map_valid.cove",
            "cove",
            "accept",
            None,
            &["§70", "§73.6"],
        ),
        cove_map_valid_file(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_map_invalid.cove",
            "cove",
            "reject",
            Some("COVE_E_MAP_INVALID"),
            &["§70", "§73.6"],
        ),
        cove_map_invalid_file(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_map_function_undeclared.cove",
            "cove",
            "reject",
            Some("COVE_E_MAP_FUNCTION_UNDECLARED"),
            &["§70", "§73.6"],
        ),
        cove_map_function_undeclared_file(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_map_identity_conflict.cove",
            "cove",
            "reject",
            Some("COVE_E_MAP_IDENTITY_CONFLICT"),
            &["§70", "§73.6"],
        ),
        cove_map_identity_conflict_file(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_map_source_stale.cove",
            "cove",
            "reject",
            Some("COVE_E_MAP_SOURCE_STALE"),
            &["§70", "§73.6"],
        ),
        cove_map_source_stale_file(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_map_evidence_invalid.cove",
            "cove",
            "reject",
            Some("COVE_E_MAP_EVIDENCE_INVALID"),
            &["§70", "§73.6"],
        ),
        cove_map_evidence_invalid_file(),
    );

    write_cove_map_execution_cases(&root, &mut entries);

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_o_optional_bad_catalog.cove",
            "cove",
            "accept",
            None,
            &["§56", "§74", "§77"],
        ),
        profile_cove_file(
            0,
            FEATURE_OBJECT_PROFILE,
            SectionKind::ObjectTypeCatalog,
            PrimaryProfile::ObjectTemporal,
            0,
            FEATURE_OBJECT_PROFILE,
            invalid_object_catalog().serialize().unwrap(),
        ),
    );

    // reject/truncated_magic: clip the trailing magic bytes.
    let mut clipped = bytes.clone();
    let n = clipped.len();
    clipped.truncate(n - 4);
    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/truncated_magic.cove",
            "cove",
            "reject",
            Some("COVE_E_BAD_MAGIC"),
            &["§12", "§74", "§76"],
        ),
        clipped,
    );

    // reject/short_file: clearly too-short file.
    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/short_file.cove",
            "cove",
            "reject",
            Some("COVE_E_OFFSET_RANGE"),
            &["§12", "§74", "§76"],
        ),
        b"COV".to_vec(),
    );

    // reject/header_magic_swapped: header magic bytes corrupted.
    let mut hdr_bad = bytes.clone();
    hdr_bad[0..4].copy_from_slice(b"XXXX");
    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/header_magic_swapped.cove",
            "cove",
            "reject",
            Some("COVE_E_CHECKSUM_MISMATCH"),
            &["§9", "§10", "§74", "§76"],
        ),
        hdr_bad,
    );

    // reject/footer_crc_flipped: bit-flip inside the footer payload so the
    // postscript's footer CRC no longer matches the footer bytes.
    let mut crc_bad = bytes.clone();
    let ps = CovePostscriptV1::parse_from_tail(&bytes).unwrap();
    let footer_offset = ps.footer.offset as usize;
    crc_bad[footer_offset] ^= 0xFF;
    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/footer_crc_flipped.cove",
            "cove",
            "reject",
            Some("COVE_E_CHECKSUM_MISMATCH"),
            &["§13", "§74", "§76"],
        ),
        crc_bad,
    );

    // reject/empty_file: zero bytes.
    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/empty_file.cove",
            "cove",
            "reject",
            Some("COVE_E_OFFSET_RANGE"),
            &["§12", "§74", "§76"],
        ),
        Vec::new(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_t_bad_column_domain.cove",
            "cove",
            "reject",
            Some("COVE_E_BAD_DOMAIN"),
            &["§23", "§73", "§76"],
        ),
        cove_file_with_section(
            FEATURE_TABLE_PROFILE | FEATURE_COLUMN_DOMAINS,
            SectionKind::ColumnDomain,
            PrimaryProfile::TableScan,
            FEATURE_COLUMN_DOMAINS,
            invalid_column_domain_payload(),
        ),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_t_bad_zone_stats.cove",
            "cove",
            "reject",
            Some("COVE_E_BAD_STATS"),
            &["§28", "§73", "§76"],
        ),
        semantic_profile_cove_file(
            PrimaryProfile::TableScan,
            FEATURE_TABLE_PROFILE,
            0,
            vec![SectionPayload {
                section_kind: SectionKind::ZoneStats as u16,
                profile: PrimaryProfile::TableScan as u8,
                flags: 0,
                item_count: 1,
                row_count: 10,
                compression: 0,
                alignment_log2: 0,
                required_features: FEATURE_TABLE_PROFILE,
                optional_features: 0,
                data: invalid_zone_stats_payload(),
            }],
        ),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_t_duplicate_table_id.cove",
            "cove",
            "reject",
            Some("COVE_E_BAD_SCHEMA"),
            &["§24", "§73", "§76"],
        ),
        cove_file_with_section(
            FEATURE_TABLE_PROFILE,
            SectionKind::TableCatalog,
            PrimaryProfile::TableScan,
            0,
            duplicate_table_catalog().serialize().unwrap(),
        ),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_t_bool_numcode_missing_declaration.cove",
            "cove",
            "reject",
            Some("COVE_E_BAD_LOGICAL_PHYSICAL_PAIR"),
            &["§19", "§24", "§73", "§76"],
        ),
        cove_t_bool_numcode_file(false),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_t_bad_segment_gap.cove",
            "cove",
            "reject",
            Some("COVE_E_SEGMENT_CORRUPT"),
            &["§25", "§73", "§76"],
        ),
        cove_file_with_section(
            FEATURE_TABLE_PROFILE,
            SectionKind::TableSegmentIndex,
            PrimaryProfile::TableScan,
            0,
            gap_table_segment_index().serialize().unwrap(),
        ),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_t_nested_list_bad_child_count.cove",
            "cove",
            "reject",
            Some("COVE_E_PAGE_CORRUPT"),
            &["§27", "§52", "§73", "§76"],
        ),
        cove_t_nested_list_bad_child_count_file(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_t_nested_struct_missing_null_handling.cove",
            "cove",
            "reject",
            Some("COVE_E_PAGE_CORRUPT"),
            &["§27", "§52", "§73", "§76"],
        ),
        cove_t_nested_struct_missing_null_handling_file(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_t_nested_map_duplicate_keys.cove",
            "cove",
            "reject",
            Some("COVE_E_PAGE_CORRUPT"),
            &["§27", "§52", "§73", "§76"],
        ),
        cove_t_nested_map_duplicate_keys_file(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/column_domain_duplicate.bin",
            "column_domain",
            "reject",
            Some("COVE_E_BAD_DOMAIN"),
            &["§23", "§76"],
        ),
        invalid_column_domain_payload(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/table_catalog_bad_pair.bin",
            "table_catalog",
            "reject",
            Some("COVE_E_BAD_LOGICAL_PHYSICAL_PAIR"),
            &["§24", "§76"],
        ),
        bad_pair_table_catalog().serialize().unwrap(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/table_segment_index_gap.bin",
            "table_segment_index",
            "reject",
            Some("COVE_E_SEGMENT_CORRUPT"),
            &["§25", "§76"],
        ),
        gap_table_segment_index().serialize().unwrap(),
    );

    let mut bad_segment_header = valid_table_segment_header().serialize().to_vec();
    bad_segment_header[68] ^= 0xFF;
    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/table_segment_header_bad_crc.bin",
            "table_segment_header",
            "reject",
            Some("COVE_E_CHECKSUM_MISMATCH"),
            &["§25", "§76"],
        ),
        bad_segment_header,
    );

    let row_morsel_gap = fixture(
        "reject/row_morsel_directory_gap.bin",
        "row_morsel_directory",
        "reject",
        Some("COVE_E_SEGMENT_CORRUPT"),
        &["§26", "§76"],
    );
    write_fixture(
        &root,
        &mut entries,
        with_morsel_count(row_morsel_gap, 2),
        gap_row_morsel_directory().serialize(),
    );

    let mut bad_sort_key = valid_sort_key().serialize().to_vec();
    bad_sort_key[4] = 9;
    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/sort_key_bad_direction.bin",
            "sort_key",
            "reject",
            Some("COVE_E_BAD_SECTION"),
            &["§53", "§76"],
        ),
        bad_sort_key,
    );

    let mut covx_bad = covx_bytes;

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/row_ref_truncated.bin",
            "row_ref",
            "reject",
            Some("COVE_E_OFFSET_RANGE"),
            &["§54"],
        ),
        vec![0u8; 4],
    );
    covx_bad[82] ^= 0xFF;
    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/covx_header_crc_flipped.covx",
            "covx",
            "reject",
            Some("COVE_E_CHECKSUM_MISMATCH"),
            &["§68", "§76"],
        ),
        covx_bad,
    );

    let mut covm_bad = covm_bytes;
    covm_bad[78] ^= 0xFF;
    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/covm_header_crc_flipped.covm",
            "covm",
            "reject",
            Some("COVE_E_CHECKSUM_MISMATCH"),
            &["§69", "§76"],
        ),
        covm_bad,
    );

    let mut covemap_bad = covemap_bytes;
    covemap_bad[94] ^= 0xFF;
    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/covemap_header_crc_flipped.covemap",
            "covemap",
            "reject",
            Some("COVE_E_CHECKSUM_MISMATCH"),
            &["§70", "§76"],
        ),
        covemap_bad,
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/metadata_json_invalid.json",
            "metadata_json",
            "reject",
            Some("COVE_E_BAD_SECTION"),
            &["§15", "§76"],
        ),
        b"{not-json".to_vec(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/file_dictionary_bad_utf8_len.bin",
            "file_dictionary",
            "reject",
            Some("COVE_E_BAD_SECTION"),
            &["§16", "§17", "§76"],
        ),
        invalid_file_dictionary_bad_utf8_len_payload(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/file_dictionary_bad_map_duplicate.bin",
            "file_dictionary",
            "reject",
            Some("COVE_E_BAD_SECTION"),
            &["§16", "§17", "§76"],
        ),
        invalid_file_dictionary_bad_map_duplicate_payload().unwrap(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/file_dictionary_redacted_null.bin",
            "file_dictionary",
            "reject",
            Some("COVE_E_BAD_SECTION"),
            &["§16", "§76"],
        ),
        invalid_file_dictionary_redacted_null_payload().unwrap(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/collation_registry_bad_utf8.bin",
            "collation_registry",
            "reject",
            Some("COVE_E_BAD_SECTION"),
            &["§22", "§76"],
        ),
        collation_registry_bad_utf8_payload(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/page_index_bad_null_count.bin",
            "page_index",
            "reject",
            Some("COVE_E_PAGE_CORRUPT"),
            &["§27", "§76"],
        ),
        page_index_payload(4, 5, CoveEncodingKind::PlainFixed as u16),
    );

    let mut constant_bad_row_count = [0u8; ConstantPayload::ENCODED_LEN];
    constant_bad_row_count[0..8].copy_from_slice(&5i64.to_le_bytes());
    constant_bad_row_count[8..16].copy_from_slice(&u64::MAX.to_le_bytes());
    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/encoding_constant_bad_row_count.json",
            "encoding_case",
            "reject",
            Some("COVE_E_PAGE_CORRUPT"),
            &["§20"],
        ),
        encoding_fixture_bytes(json!({
            "encoding": "constant",
            "payload": constant_bad_row_count.to_vec(),
            "expect_values": []
        })),
    );

    let mut rle_zero_length = Vec::new();
    rle_zero_length.extend_from_slice(&1u32.to_le_bytes());
    rle_zero_length.extend_from_slice(&0i64.to_le_bytes());
    rle_zero_length.extend_from_slice(&0u32.to_le_bytes());
    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/encoding_rle_zero_length.json",
            "encoding_case",
            "reject",
            Some("COVE_E_PAGE_CORRUPT"),
            &["§20"],
        ),
        encoding_fixture_bytes(json!({
            "encoding": "rle",
            "payload": rle_zero_length,
            "expect_values": []
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/encoding_run_end_bad_order.json",
            "encoding_case",
            "reject",
            Some("COVE_E_PAGE_CORRUPT"),
            &["§20"],
        ),
        encoding_fixture_bytes(json!({
            "encoding": "run_end",
            "payload": run_end_payload_bytes(&[1, 2], &[5, 5]),
            "expect_values": []
        })),
    );

    let plain_fixed_valid = PlainFixedPayload {
        values: vec![1, -2, 3, -4],
    }
    .encode();
    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/encoding_plain_fixed_truncated.json",
            "encoding_case",
            "reject",
            Some("COVE_E_OFFSET_RANGE"),
            &["§20"],
        ),
        encoding_fixture_bytes(json!({
            "encoding": "plain_fixed",
            "payload": plain_fixed_valid[..plain_fixed_valid.len() - 1].to_vec(),
            "expect_values": []
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/encoding_plain_varint_truncated.json",
            "encoding_case",
            "reject",
            Some("COVE_E_OFFSET_RANGE"),
            &["§20"],
        ),
        encoding_fixture_bytes(json!({
            "encoding": "plain_varint",
            "payload": 1u32.to_le_bytes().to_vec(),
            "expect_values": []
        })),
    );

    let mut bit_packed_bad_width = Vec::new();
    bit_packed_bad_width.push(0u8);
    bit_packed_bad_width.extend_from_slice(&1u32.to_le_bytes());
    bit_packed_bad_width.extend_from_slice(&0u32.to_le_bytes());
    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/encoding_bit_packed_bad_width.json",
            "encoding_case",
            "reject",
            Some("COVE_E_PAGE_CORRUPT"),
            &["§20"],
        ),
        encoding_fixture_bytes(json!({
            "encoding": "bit_packed",
            "payload": bit_packed_bad_width,
            "expect_values": []
        })),
    );

    let mut delta_truncated = Vec::new();
    delta_truncated.extend_from_slice(&5i64.to_le_bytes());
    delta_truncated.extend_from_slice(&1u32.to_le_bytes());

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/encoding_delta_truncated.json",
            "encoding_case",
            "reject",
            Some("COVE_E_OFFSET_RANGE"),
            &["§20"],
        ),
        encoding_fixture_bytes(json!({
            "encoding": "delta",
            "payload": delta_truncated,
            "expect_values": []
        })),
    );

    let mut for_truncated = Vec::new();
    for_truncated.extend_from_slice(&7i64.to_le_bytes());
    for_truncated.extend_from_slice(&1u32.to_le_bytes());

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/encoding_for_truncated.json",
            "encoding_case",
            "reject",
            Some("COVE_E_OFFSET_RANGE"),
            &["§20"],
        ),
        encoding_fixture_bytes(json!({
            "encoding": "frame_of_reference",
            "payload": for_truncated,
            "expect_values": []
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/encoding_patched_base_duplicate_patch.json",
            "encoding_case",
            "reject",
            Some("COVE_E_PAGE_CORRUPT"),
            &["§20"],
        ),
        encoding_fixture_bytes(json!({
            "encoding": "patched_base",
            "payload": patched_base_payload_bytes(&[0, 0], &[(1, 1), (1, 2)]),
            "expect_values": []
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/encoding_sparse_out_of_range.json",
            "encoding_case",
            "reject",
            Some("COVE_E_PAGE_CORRUPT"),
            &["§20"],
        ),
        encoding_fixture_bytes(json!({
            "encoding": "sparse",
            "payload": sparse_payload_bytes(5, 0, &[(5, 1)]),
            "expect_values": []
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/encoding_local_codebook_bad_local_index.json",
            "encoding_case",
            "reject",
            Some("COVE_E_PAGE_CORRUPT"),
            &["§20"],
        ),
        encoding_fixture_bytes(json!({
            "encoding": "local_codebook",
            "payload": LocalCodebookPayload {
                values: LocalCodebookValues::FileCode(vec![42]),
                indexes: LocalIndexPayload::BitPacked(
                    BitPackedPayload::pack(&[0, 1], 1).unwrap(),
                ),
            }
            .encode(),
            "expect_values": []
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/nested_list_bad_child_count.json",
            "nested_case",
            "reject",
            Some("COVE_E_PAGE_CORRUPT"),
            &["§52"],
        ),
        nested_fixture_bytes(json!({
            "layout": "list",
            "offsets": [0, 2, 2, 5],
            "child_row_count": 4
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/nested_struct_missing_null_handling.json",
            "nested_case",
            "reject",
            Some("COVE_E_PAGE_CORRUPT"),
            &["§52"],
        ),
        nested_fixture_bytes(json!({
            "layout": "struct",
            "field_row_counts": [3, 3],
            "parent_row_count": 3,
            "parent_null_handling_declared": false
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/nested_map_duplicate_keys.json",
            "nested_case",
            "reject",
            Some("COVE_E_PAGE_CORRUPT"),
            &["§52"],
        ),
        nested_fixture_bytes(json!({
            "layout": "map",
            "offsets": [0, 2],
            "key_row_count": 2,
            "value_row_count": 2,
            "keys_are_scalar": true,
            "allow_duplicate_keys": false,
            "canonical_keys": ["a", "a"]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/nested_map_non_scalar_key.json",
            "nested_case",
            "reject",
            Some("COVE_E_PAGE_CORRUPT"),
            &["§52"],
        ),
        nested_fixture_bytes(json!({
            "layout": "map",
            "offsets": [0, 1],
            "key_row_count": 1,
            "value_row_count": 1,
            "keys_are_scalar": false,
            "allow_duplicate_keys": false,
            "canonical_keys": ["a"]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/nested_map_child_count_mismatch.json",
            "nested_case",
            "reject",
            Some("COVE_E_PAGE_CORRUPT"),
            &["§52"],
        ),
        nested_fixture_bytes(json!({
            "layout": "map",
            "offsets": [0, 2],
            "key_row_count": 2,
            "value_row_count": 1,
            "keys_are_scalar": true,
            "allow_duplicate_keys": false,
            "canonical_keys": ["a", "b"]
        })),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/digest_manifest_wrong_len.bin",
            "digest_manifest",
            "reject",
            Some("COVE_E_BAD_SECTION"),
            &["§65", "§76"],
        ),
        digest_manifest_wrong_len_payload(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/digest_manifest_bad_checksum.bin",
            "digest_manifest",
            "reject",
            Some("COVE_E_CHECKSUM_MISMATCH"),
            &["§65", "§76"],
        ),
        digest_manifest_bad_checksum_payload(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/redaction_manifest_truncated.bin",
            "redaction_manifest",
            "reject",
            Some("COVE_E_OFFSET_RANGE"),
            &["§64", "§76"],
        ),
        1u32.to_le_bytes().to_vec(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/io_hints_truncated.bin",
            "io_hints",
            "reject",
            Some("COVE_E_OFFSET_RANGE"),
            &["§67", "§76"],
        ),
        vec![0; 8],
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/io_hints_legacy_12_byte_layout.bin",
            "io_hints",
            "reject",
            Some("COVE_E_OFFSET_RANGE"),
            &["§67", "§76"],
        ),
        vec![0; 12],
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/lakehouse_hints_bad_utf8.bin",
            "lakehouse_hints",
            "reject",
            Some("COVE_E_BAD_SECTION"),
            &["§50", "§76"],
        ),
        lakehouse_hints_bad_utf8_payload(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/kernel_capabilities_unknown_encoding.bin",
            "kernel_capabilities",
            "reject",
            Some("COVE_E_BAD_SECTION"),
            &["§21", "§76"],
        ),
        kernel_capabilities_payload(0xfffe),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/kernel_capabilities_reserved.bin",
            "kernel_capabilities",
            "reject",
            Some("COVE_E_BAD_SECTION"),
            &["§21", "§76"],
        ),
        kernel_capabilities_reserved_payload(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/kernel_capabilities_trailing.bin",
            "kernel_capabilities",
            "reject",
            Some("COVE_E_BAD_SECTION"),
            &["§21", "§76"],
        ),
        kernel_capabilities_trailing_payload(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/kernel_capabilities_truncated.bin",
            "kernel_capabilities",
            "reject",
            Some("COVE_E_OFFSET_RANGE"),
            &["§21", "§76"],
        ),
        vec![1, 0, 0, 0, CoveEncodingKind::Rle as u8],
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/exact_set_index_unsorted.bin",
            "exact_set_index",
            "reject",
            Some("COVE_E_BAD_INDEX"),
            &["§30", "§76"],
        ),
        exact_set_index_payload(&[5, 2]),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/bloom_index_zero_filter_count.bin",
            "bloom_index",
            "reject",
            Some("COVE_E_BAD_INDEX"),
            &["§31", "§76"],
        ),
        bloom_index_payload(0, 64),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/inverted_morsel_index_unsorted.bin",
            "inverted_morsel_index",
            "reject",
            Some("COVE_E_BAD_INDEX"),
            &["§32", "§76"],
        ),
        inverted_index_payload(&[7, 5]),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/lookup_index_unsorted.bin",
            "lookup_index",
            "reject",
            Some("COVE_E_BAD_INDEX"),
            &["§33", "§76"],
        ),
        lookup_index_unsorted_payload(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/aggregate_synopsis_unknown_kind.bin",
            "aggregate_synopsis",
            "reject",
            Some("COVE_E_BAD_INDEX"),
            &["§34", "§76"],
        ),
        aggregate_synopsis_unknown_kind_payload(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/composite_zone_index_zero_key_columns.bin",
            "composite_zone_index",
            "reject",
            Some("COVE_E_BAD_INDEX"),
            &["§35", "§76"],
        ),
        composite_index_payload(0),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/topn_summary_bad_direction.bin",
            "topn_summary",
            "reject",
            Some("COVE_E_BAD_INDEX"),
            &["§36", "§76"],
        ),
        topn_summary_bad_direction_payload(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_e_engine_registry_duplicate_namespace.bin",
            "cove_e_engine_registry",
            "reject",
            Some("COVE_E_BAD_ENGINE_PROFILE"),
            &["§39", "§76"],
        ),
        engine_registry_payload(&["org.example", "org.example"]).unwrap(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_e_execution_code_bad_kind.bin",
            "cove_e_execution_code",
            "reject",
            Some("COVE_E_BAD_ENGINE_PROFILE"),
            &["§40", "§76"],
        ),
        invalid_execution_descriptor_payload(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_e_execution_scope_bad_kind.bin",
            "cove_e_execution_scope",
            "reject",
            Some("COVE_E_BAD_ENGINE_PROFILE"),
            &["§41", "§76"],
        ),
        invalid_execution_scope_descriptor_payload(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_e_code_space_bad_utf8.bin",
            "cove_e_code_space",
            "reject",
            Some("COVE_E_BAD_SECTION"),
            &["§42", "§76"],
        ),
        invalid_code_space_descriptor_payload(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_e_mount_policy_bad_mapping.bin",
            "cove_e_mount_policy",
            "reject",
            Some("COVE_E_BAD_ENGINE_PROFILE"),
            &["§43", "§76"],
        ),
        invalid_mount_policy_payload(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_h_mount_hints_reserved.bin",
            "cove_h_mount_hints",
            "reject",
            Some("COVE_E_BAD_SECTION"),
            &["§44", "§76"],
        ),
        invalid_harbor_mount_hints_payload(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_o_object_catalog_duplicate_property.bin",
            "cove_o_object_catalog",
            "reject",
            Some("COVE_E_BAD_SCHEMA"),
            &["§56", "§76"],
        ),
        invalid_object_catalog().serialize().unwrap(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_o_temporal_segment_index_bad_counts.bin",
            "cove_o_temporal_segment_index",
            "reject",
            Some("COVE_E_BAD_SCHEMA"),
            &["§57", "§76"],
        ),
        invalid_temporal_segment_index().serialize().unwrap(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_unknown_required_feature.cove",
            "cove",
            "reject",
            Some("COVE_E_UNKNOWN_REQUIRED_FEATURE"),
            &["§74", "§77", "§76"],
        ),
        cove_with_unknown_required_feature(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_e_required_bad_descriptor.cove",
            "cove",
            "reject",
            Some("COVE_E_BAD_ENGINE_PROFILE"),
            &["§40", "§74", "§77", "§76"],
        ),
        profile_cove_file(
            FEATURE_ENGINE_PROFILE,
            0,
            SectionKind::ExecutionCodeDescriptor,
            PrimaryProfile::EngineExecution,
            FEATURE_ENGINE_PROFILE,
            0,
            invalid_execution_descriptor_payload(),
        ),
    );

    let mut lz4_missing_feature = compressed_profile_cove_file(
        FEATURE_ENGINE_PROFILE,
        FEATURE_CODEC_LZ4,
        SectionKind::ExecutionCodeDescriptor,
        PrimaryProfile::EngineExecution,
        FEATURE_ENGINE_PROFILE,
        FEATURE_CODEC_LZ4,
        CompressionCodec::Lz4,
        valid_execution_descriptor().serialize().to_vec(),
    );
    rewrite_cove_feature_bits(&mut lz4_missing_feature, FEATURE_ENGINE_PROFILE, 0);
    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_lz4_missing_feature.cove",
            "cove",
            "reject",
            Some("COVE_E_BAD_SECTION"),
            &["§66", "§73", "§76"],
        ),
        lz4_missing_feature,
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_e_required_bad_refs.cove",
            "cove",
            "reject",
            Some("COVE_E_BAD_ENGINE_PROFILE"),
            &["§39", "§40", "§41", "§42", "§43", "§73", "§76"],
        ),
        cove_e_profile_bundle_file(true, true),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_h_required_bad_hints.cove",
            "cove",
            "reject",
            Some("COVE_E_BAD_SECTION"),
            &["§44", "§74", "§77", "§76"],
        ),
        profile_cove_file(
            FEATURE_HARBOR_PROFILE,
            0,
            SectionKind::HarborMountHints,
            PrimaryProfile::HarborExecution,
            FEATURE_HARBOR_PROFILE,
            0,
            invalid_harbor_mount_hints_payload(),
        ),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_o_required_bad_catalog.cove",
            "cove",
            "reject",
            Some("COVE_E_BAD_SCHEMA"),
            &["§56", "§74", "§77", "§76"],
        ),
        profile_cove_file(
            FEATURE_OBJECT_PROFILE,
            0,
            SectionKind::ObjectTypeCatalog,
            PrimaryProfile::ObjectTemporal,
            FEATURE_OBJECT_PROFILE,
            0,
            invalid_object_catalog().serialize().unwrap(),
        ),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_o_temporal_bad_order.cove",
            "cove",
            "reject",
            Some("COVE_E_BAD_SCHEMA"),
            &["§58", "§73", "§76"],
        ),
        semantic_profile_cove_file(PrimaryProfile::ObjectTemporal, FEATURE_OBJECT_PROFILE, 0, {
            let bad_order_rows = [
                valid_temporal_rows[1].clone(),
                valid_temporal_rows[0].clone(),
            ];
            vec![
                cove_o_object_catalog_section(),
                cove_o_temporal_segment_index_section(&[(5, &bad_order_rows)]),
                cove_o_temporal_segment_data_section(5, &bad_order_rows),
            ]
        }),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_o_temporal_csn_decreases.cove",
            "cove",
            "reject",
            Some("COVE_E_BAD_SCHEMA"),
            &["§58", "§73", "§76"],
        ),
        semantic_profile_cove_file(PrimaryProfile::ObjectTemporal, FEATURE_OBJECT_PROFILE, 0, {
            let mut bad_csn_rows = valid_temporal_rows.clone();
            bad_csn_rows[0].timestamp_us = 10;
            bad_csn_rows[0].csn = 100;
            bad_csn_rows[1].timestamp_us = 20;
            bad_csn_rows[1].csn = 50;
            vec![
                cove_o_object_catalog_section(),
                cove_o_temporal_segment_index_section(&[(5, &bad_csn_rows)]),
                cove_o_temporal_segment_data_section(5, &bad_csn_rows),
            ]
        }),
    );

    let mut bad_prev_rows = valid_temporal_rows.clone();
    bad_prev_rows[0].prev_ref = Some(CoveRecordRefV1 {
        segment_id: 5,
        row_index: 1,
        target_kind: 0,
    });
    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_o_temporal_bad_prev_ref.cove",
            "cove",
            "reject",
            Some("COVE_E_REF_INVALID"),
            &["§60", "§73", "§76"],
        ),
        semantic_profile_cove_file(
            PrimaryProfile::ObjectTemporal,
            FEATURE_OBJECT_PROFILE,
            0,
            vec![
                cove_o_object_catalog_section(),
                cove_o_temporal_segment_index_section(&[(5, &bad_prev_rows)]),
                cove_o_temporal_segment_data_section(5, &bad_prev_rows),
            ],
        ),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_o_property_elision_missing_feature.cove",
            "cove",
            "reject",
            Some("COVE_E_BAD_SECTION"),
            &["§61", "§66", "§74", "§76"],
        ),
        cove_o_property_stats_only_file(
            FEATURE_OBJECT_PROFILE,
            PAGE_FLAG_STATS_ONLY_CONSTANT | PAGE_FLAG_ALL_NULL,
            0,
            valid_temporal_rows.len() as u32,
        ),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_o_property_stats_only_all_non_null_missing_stats.cove",
            "cove",
            "reject",
            Some("COVE_E_PAGE_CORRUPT"),
            &["§61", "§66", "§76"],
        ),
        cove_o_property_stats_only_file(
            FEATURE_OBJECT_PROFILE | FEATURE_PAGE_PAYLOAD_ELISION,
            PAGE_FLAG_STATS_ONLY_CONSTANT | PAGE_FLAG_ALL_NON_NULL,
            valid_temporal_rows.len() as u32,
            0,
        ),
    );

    let mut bad_trust_manifest = trust_manifest_payload(5, &valid_temporal_rows);
    *bad_trust_manifest.last_mut().unwrap() ^= 0xFF;
    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_o_trust_manifest_bad_digest.cove",
            "cove",
            "reject",
            Some("COVE_E_DIGEST_MISMATCH"),
            &["§63", "§73", "§76"],
        ),
        semantic_profile_cove_file(
            PrimaryProfile::ObjectTemporal,
            FEATURE_OBJECT_PROFILE | FEATURE_TRUST_CHAIN,
            0,
            vec![
                cove_o_object_catalog_section(),
                cove_o_temporal_segment_index_section(&[(5, &valid_temporal_rows)]),
                cove_o_temporal_segment_data_section(5, &valid_temporal_rows),
                SectionPayload {
                    section_kind: SectionKind::TrustManifest as u16,
                    profile: PrimaryProfile::ObjectTemporal as u8,
                    flags: 0,
                    item_count: valid_temporal_rows.len() as u64,
                    row_count: valid_temporal_rows.len() as u64,
                    compression: 0,
                    alignment_log2: 0,
                    required_features: FEATURE_TRUST_CHAIN,
                    optional_features: 0,
                    data: bad_trust_manifest,
                },
            ],
        ),
    );

    for (path, code) in [
        (
            "reject/error_surface_bad_version.json",
            "COVE_E_BAD_VERSION",
        ),
        (
            "reject/error_surface_arith_overflow.json",
            "COVE_E_ARITH_OVERFLOW",
        ),
        ("reject/error_surface_dict_miss.json", "COVE_E_DICT_MISS"),
        (
            "reject/error_surface_bad_filecode.json",
            "COVE_E_BAD_FILECODE",
        ),
        (
            "reject/error_surface_bad_numcode.json",
            "COVE_E_BAD_NUMCODE",
        ),
        (
            "reject/error_surface_bad_extension.json",
            "COVE_E_BAD_EXTENSION",
        ),
        (
            "reject/error_surface_execution_code_map.json",
            "COVE_E_EXECUTION_CODE_MAP",
        ),
        (
            "reject/error_surface_harbor_mount_lease.json",
            "COVE_E_HARBOR_MOUNT_LEASE",
        ),
        (
            "reject/error_surface_not_self_contained.json",
            "COVE_E_NOT_SELF_CONTAINED",
        ),
        (
            "reject/error_surface_redaction_policy.json",
            "COVE_E_REDACTION_POLICY",
        ),
        (
            "reject/error_surface_sidecar_stale.json",
            "COVE_E_SIDECAR_STALE",
        ),
        (
            "reject/error_surface_map_invalid.json",
            "COVE_E_MAP_INVALID",
        ),
        (
            "reject/error_surface_map_function_undeclared.json",
            "COVE_E_MAP_FUNCTION_UNDECLARED",
        ),
        (
            "reject/error_surface_map_identity_conflict.json",
            "COVE_E_MAP_IDENTITY_CONFLICT",
        ),
        (
            "reject/error_surface_map_source_stale.json",
            "COVE_E_MAP_SOURCE_STALE",
        ),
        (
            "reject/error_surface_map_evidence_invalid.json",
            "COVE_E_MAP_EVIDENCE_INVALID",
        ),
    ] {
        write_fixture(
            &root,
            &mut entries,
            fixture(path, "error_surface_case", "reject", Some(code), &["§76"]),
            error_surface_fixture_bytes(json!({ "code": code })),
        );
    }

    for (path, value) in [
        (
            "accept/suite_manifest_contract.json",
            json!({
                "op": "manifest_sections_present",
                "sections": ["§8", "§10", "§12", "§20", "§37", "§45", "§46", "§47", "§51", "§61", "§62", "§70.2", "§70.3", "§70.5", "§70.6", "§70.8", "§70.9", "§70.10", "§70.12", "§70.13", "§70.14", "§72.8", "§74", "§75", "§76", "§77", "§78", "§79"],
                "minimum_accept": 1,
                "minimum_reject": 1,
            }),
        ),
        (
            "accept/suite_release_gates_contract.json",
            json!({
                "op": "release_gate_contains",
                "needles": [
                    "cargo fmt --check",
                    "cargo test --workspace",
                    "cargo test -p cove-convert-parquet",
                    "cargo run -p cove-bench --bin cove-bench > /dev/null",
                    "cargo run -p cove-conformance --bin gen-corpus -- --check",
                    "cargo run -p cove-conformance --bin gen-capability-matrix -- --check",
                    "cargo run -p cove-conformance --bin cove-conformance -- conformance/"
                ],
            }),
        ),
        (
            "accept/suite_workspace_contract.json",
            json!({
                "op": "workspace_members_present",
                "members": [
                    "crates/cove-core",
                    "crates/cove-validate",
                    "crates/cove-inspect",
                    "crates/cove-dump",
                    "crates/cove-convert-parquet",
                    "crates/cove-conformance",
                    "crates/cove-map",
                    "crates/cove-bench"
                ],
            }),
        ),
    ] {
        write_fixture(
            &root,
            &mut entries,
            fixture(path, "suite_contract_case", "accept", None, &["§78", "§79"]),
            suite_contract_fixture_bytes(value),
        );
    }

    assert_error_code_coverage(&entries);

    let manifest = root.join("manifest.jsonl");
    let manifest_content = entries
        .iter()
        .map(|entry| serde_json::to_string(entry).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    if check_mode() {
        let existing = fs::read(&manifest).unwrap_or_else(|err| {
            panic!("cannot read {} during --check: {err}", manifest.display())
        });
        assert_eq!(
            existing,
            manifest_content.as_bytes(),
            "{} is not up to date; run cargo run -p cove-conformance --bin gen-corpus",
            manifest.display()
        );
        println!(
            "conformance corpus is up to date ({} fixtures in {})",
            entries.len(),
            root.display()
        );
    } else {
        fs::write(&manifest, manifest_content).unwrap();

        println!("wrote {} fixtures to {}", entries.len(), root.display());
    }
}

fn check_mode() -> bool {
    std::env::args().any(|arg| arg == "--check")
}

fn fixture(
    path: &str,
    kind: &str,
    expect: &str,
    error_code: Option<&str>,
    sections: &[&str],
) -> Value {
    let mut sections = sections.to_vec();
    if error_code.is_some() && !sections.contains(&"§76") {
        sections.push("§76");
    }
    let mut value = json!({
        "path": path,
        "kind": kind,
        "expect": expect,
        "sections": sections,
    });
    if let Some(code) = error_code {
        value["error_code"] = json!(code);
    }
    value
}

fn with_morsel_count(mut value: Value, morsel_count: u32) -> Value {
    value["morsel_count"] = json!(morsel_count);
    value
}

fn with_collation_count(mut value: Value, collation_count: usize) -> Value {
    value["collation_count"] = json!(collation_count);
    value
}

fn with_expect_can_skip(mut value: Value, expected: bool) -> Value {
    value["expect_can_skip"] = json!(expected);
    value
}

fn arrow_bitmap_fixture_bytes(value: Value) -> Vec<u8> {
    serde_json::to_vec_pretty(&value).unwrap()
}

fn encoding_fixture_bytes(value: Value) -> Vec<u8> {
    serde_json::to_vec_pretty(&value).unwrap()
}

fn error_surface_fixture_bytes(value: Value) -> Vec<u8> {
    serde_json::to_vec_pretty(&value).unwrap()
}

fn suite_contract_fixture_bytes(value: Value) -> Vec<u8> {
    serde_json::to_vec_pretty(&value).unwrap()
}

fn nested_fixture_bytes(value: Value) -> Vec<u8> {
    serde_json::to_vec_pretty(&value).unwrap()
}

fn pruning_fixture_bytes(value: Value) -> Vec<u8> {
    serde_json::to_vec_pretty(&value).unwrap()
}

fn page_codec_fixture_bytes(value: Value) -> Vec<u8> {
    serde_json::to_vec_pretty(&value).unwrap()
}

fn map_payload_bytes(value: Value) -> Vec<u8> {
    serde_json::to_vec_pretty(&value).unwrap()
}

fn extension_registry_entry(
    extension_kind: ExtensionKind,
    required_feature_bit: u64,
    optional_feature_bit: u64,
    payload_ref: u32,
) -> ExtensionRegistryEntry {
    ExtensionRegistryEntry {
        extension_id: 7,
        namespace: b"org.example".to_vec(),
        name: b"patient-id".to_vec(),
        version_major: 1,
        version_minor: 0,
        extension_kind,
        required_feature_bit,
        optional_feature_bit,
        fallback_kind: 0,
        fallback_ref: 0,
        payload_ref,
        checksum: 0,
    }
}

fn extension_registry_valid_payload() -> Vec<u8> {
    ExtensionRegistry {
        flags: 0,
        entries: vec![extension_registry_entry(
            ExtensionKind::VendorMetadata,
            0,
            1 << 20,
            0,
        )],
    }
    .serialize()
    .unwrap()
}

fn extension_registry_bad_crc_payload() -> Vec<u8> {
    let mut bytes = extension_registry_valid_payload();
    *bytes.last_mut().unwrap() ^= 0xFF;
    bytes
}

fn extension_registry_reserved_payload() -> Vec<u8> {
    let mut bytes = ExtensionRegistry {
        flags: 0,
        entries: Vec::new(),
    }
    .serialize()
    .unwrap();
    bytes[4] = 1;
    bytes
}

fn extension_registry_trailing_payload() -> Vec<u8> {
    let mut bytes = ExtensionRegistry {
        flags: 0,
        entries: Vec::new(),
    }
    .serialize()
    .unwrap();
    bytes.push(0);
    bytes
}

fn extension_registry_required_unknown_payload() -> Vec<u8> {
    ExtensionRegistry {
        flags: 0,
        entries: vec![extension_registry_entry(
            ExtensionKind::VendorMetadata,
            1 << 20,
            0,
            0,
        )],
    }
    .serialize()
    .unwrap()
}

fn extension_registry_optional_no_fallback_payload(kind: ExtensionKind) -> Vec<u8> {
    ExtensionRegistry {
        flags: 0,
        entries: vec![extension_registry_entry(kind, 0, 1 << 20, 0)],
    }
    .serialize()
    .unwrap()
}

fn extension_logical_type_payload(collation_id: u16) -> Vec<u8> {
    ExtensionLogicalTypeV1 {
        extension_id: 7,
        base_logical_type: CoveLogicalType::Utf8,
        canonical_value_tag: ValueTag::Utf8,
        collation_id,
        flags: 0,
        arrow_extension_name: "org.example.patient-id".into(),
        metadata_payload_ref: 0,
    }
    .serialize()
    .unwrap()
}

fn extension_index_descriptor_payload(
    proof_capability: ExtensionProofCapability,
    false_negative_policy: ExtensionFalseNegativePolicy,
) -> Vec<u8> {
    ExtensionIndexDescriptorV1 {
        extension_id: 7,
        index_kind: 100,
        key_column_count: 1,
        proof_capability,
        false_negative_policy,
        flags: 0,
        payload_ref: 0,
    }
    .serialize()
    .to_vec()
}

fn temporal_bloom_payload() -> Vec<u8> {
    TemporalBloomIndex {
        flags: 0,
        entries: vec![TemporalBloomEntryV1 {
            segment_id: 5,
            time_bucket_start_us: 1_700_000_000_000_000,
            time_bucket_end_us: 1_700_000_060_000_000,
            filter_offset: 0,
            filter_length: 0,
            checksum: 0,
        }],
    }
    .serialize(&[vec![0xA5, 0x5A, 0xC3, 0x3C]])
    .unwrap()
}

fn temporal_bloom_bad_crc_payload() -> Vec<u8> {
    let mut bytes = temporal_bloom_payload();
    bytes[8 + TEMPORAL_BLOOM_ENTRY_LEN - 1] ^= 0xFF;
    bytes
}

fn temporal_bloom_filter_oob_payload() -> Vec<u8> {
    let mut bytes = temporal_bloom_payload();
    let pos = 8usize;
    let bad_offset = (bytes.len() as u64) + 8;
    bytes[pos + 20..pos + 28].copy_from_slice(&bad_offset.to_le_bytes());
    rewrite_temporal_bloom_entry_crc(&mut bytes);
    bytes
}

fn temporal_bloom_inverted_bucket_payload() -> Vec<u8> {
    let mut bytes = temporal_bloom_payload();
    let pos = 8usize;
    bytes[pos + 4..pos + 12].copy_from_slice(&20i64.to_le_bytes());
    bytes[pos + 12..pos + 20].copy_from_slice(&10i64.to_le_bytes());
    rewrite_temporal_bloom_entry_crc(&mut bytes);
    bytes
}

fn rewrite_temporal_bloom_entry_crc(bytes: &mut [u8]) {
    let pos = 8usize;
    let mut entry = [0u8; TEMPORAL_BLOOM_ENTRY_LEN];
    entry.copy_from_slice(&bytes[pos..pos + TEMPORAL_BLOOM_ENTRY_LEN]);
    entry[36..40].fill(0);
    let crc = checksum::crc32c(&entry);
    bytes[pos + 36..pos + 40].copy_from_slice(&crc.to_le_bytes());
}

fn parquet_primitives_valid_file() -> Vec<u8> {
    let batch = RecordBatch::try_from_iter(vec![
        (
            "active",
            Arc::new(BooleanArray::from(vec![true, false, true])) as ArrayRef,
        ),
        (
            "id",
            Arc::new(Int64Array::from(vec![10, 20, 30])) as ArrayRef,
        ),
        (
            "score",
            Arc::new(Float64Array::from(vec![1.5, 2.0, 3.25])) as ArrayRef,
        ),
        (
            "city",
            Arc::new(StringArray::from(vec!["sea", "lon", "par"])) as ArrayRef,
        ),
        (
            "blob",
            Arc::new(BinaryArray::from(vec![
                b"aa".as_ref(),
                b"bb".as_ref(),
                b"cc".as_ref(),
            ])) as ArrayRef,
        ),
        (
            "event_date",
            Arc::new(Date32Array::from(vec![19000, 19001, 19002])) as ArrayRef,
        ),
        (
            "ts_us",
            Arc::new(TimestampMicrosecondArray::from(vec![1000, 2000, 3000])) as ArrayRef,
        ),
    ])
    .unwrap();
    parquet_file_bytes(&batch)
}

fn parquet_nullable_valid_file() -> Vec<u8> {
    let batch = RecordBatch::try_from_iter(vec![(
        "id",
        Arc::new(Int64Array::from(vec![Some(1), None, Some(3)])) as ArrayRef,
    )])
    .unwrap();
    parquet_file_bytes(&batch)
}

fn parquet_nested_unsupported_file() -> Vec<u8> {
    let mut builder = ListBuilder::new(Int32Builder::new());
    builder.values().append_value(1);
    builder.values().append_value(2);
    builder.append(true);
    builder.append(true);
    builder.values().append_value(3);
    builder.append(true);
    let batch =
        RecordBatch::try_from_iter(vec![("tags", Arc::new(builder.finish()) as ArrayRef)]).unwrap();
    parquet_file_bytes(&batch)
}

fn parquet_file_bytes(batch: &RecordBatch) -> Vec<u8> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut writer = ArrowWriter::try_new(&mut cursor, batch.schema(), None).unwrap();
        writer.write(batch).unwrap();
        writer.close().unwrap();
    }
    cursor.into_inner()
}

fn run_end_payload_bytes(values: &[i64], run_ends: &[u32]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(values.len() as u32).to_le_bytes());
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
    for run_end in run_ends {
        out.extend_from_slice(&run_end.to_le_bytes());
    }
    out
}

fn sparse_payload_bytes(row_count: u32, fill: i64, overrides: &[(u32, i64)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&row_count.to_le_bytes());
    out.extend_from_slice(&fill.to_le_bytes());
    out.extend_from_slice(&(overrides.len() as u32).to_le_bytes());
    for (position, value) in overrides {
        out.extend_from_slice(&position.to_le_bytes());
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

fn patched_base_payload_bytes(base: &[i64], patches: &[(u32, i64)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(base.len() as u32).to_le_bytes());
    for value in base {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&(patches.len() as u32).to_le_bytes());
    for (position, value) in patches {
        out.extend_from_slice(&position.to_le_bytes());
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

fn varbytes_payload(values: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for value in values {
        out.extend_from_slice(&(value.len() as u32).to_le_bytes());
        out.extend_from_slice(value);
    }
    out
}

fn assert_error_code_coverage(entries: &[Value]) {
    let covered = entries
        .iter()
        .filter_map(|entry| entry.get("error_code").and_then(Value::as_str))
        .collect::<BTreeSet<_>>();
    let missing = CoveError::ALL_SPEC_CODES
        .iter()
        .copied()
        .filter(|code| !covered.contains(code))
        .collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "manifest is missing Spec §76 error_code coverage for: {}",
        missing.join(", ")
    );
}

fn write_fixture(root: &PathBuf, entries: &mut Vec<Value>, entry: Value, bytes: Vec<u8>) {
    let path = entry["path"].as_str().unwrap();
    let full_path = root.join(path);
    if check_mode() {
        let existing = fs::read(&full_path).unwrap_or_else(|err| {
            panic!("cannot read {} during --check: {err}", full_path.display())
        });
        assert_eq!(
            existing,
            bytes,
            "{} is not up to date; run cargo run -p cove-conformance --bin gen-corpus",
            full_path.display()
        );
    } else {
        fs::write(full_path, bytes).unwrap();
    }
    entries.push(entry);
}

fn write_auxiliary_file(root: &PathBuf, path: &str, bytes: &[u8]) {
    let full_path = root.join(path);
    if check_mode() {
        let existing = fs::read(&full_path).unwrap_or_else(|err| {
            panic!("cannot read {} during --check: {err}", full_path.display())
        });
        assert_eq!(
            existing,
            bytes,
            "{} is not up to date; run cargo run -p cove-conformance --bin gen-corpus",
            full_path.display()
        );
    } else {
        fs::write(full_path, bytes).unwrap();
    }
}

fn cove_file_with_section(
    required_features: u64,
    section_kind: SectionKind,
    profile: PrimaryProfile,
    section_required_features: u64,
    data: Vec<u8>,
) -> Vec<u8> {
    profile_cove_file(
        required_features,
        0,
        section_kind,
        profile,
        section_required_features,
        0,
        data,
    )
}

fn semantic_profile_cove_file(
    primary_profile: PrimaryProfile,
    required_features: u64,
    optional_features: u64,
    sections: Vec<SectionPayload>,
) -> Vec<u8> {
    let mut writer = MinimalCoveWriter::new();
    writer.primary_profile = primary_profile as u8;
    writer.required_features = required_features;
    writer.optional_features = optional_features;
    writer.sections = sections;
    writer.write().unwrap()
}

fn profile_cove_file(
    required_features: u64,
    optional_features: u64,
    section_kind: SectionKind,
    profile: PrimaryProfile,
    section_required_features: u64,
    section_optional_features: u64,
    data: Vec<u8>,
) -> Vec<u8> {
    semantic_profile_cove_file(
        PrimaryProfile::Mixed,
        required_features,
        optional_features,
        vec![SectionPayload {
            section_kind: section_kind as u16,
            profile: profile as u8,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: section_required_features,
            optional_features: section_optional_features,
            data,
        }],
    )
}

fn compressed_profile_cove_file(
    required_features: u64,
    optional_features: u64,
    section_kind: SectionKind,
    profile: PrimaryProfile,
    section_required_features: u64,
    section_optional_features: u64,
    compression: CompressionCodec,
    data: Vec<u8>,
) -> Vec<u8> {
    semantic_profile_cove_file(
        PrimaryProfile::Mixed,
        required_features,
        optional_features,
        vec![SectionPayload {
            section_kind: section_kind as u16,
            profile: profile as u8,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: compression as u8,
            alignment_log2: 0,
            required_features: section_required_features,
            optional_features: section_optional_features,
            data,
        }],
    )
}

fn cove_with_unknown_optional_feature() -> Vec<u8> {
    let mut writer = MinimalCoveWriter::new();
    writer.optional_features = 1u64 << 63;
    writer.write().unwrap()
}

fn cove_with_unknown_required_feature() -> Vec<u8> {
    let writer = MinimalCoveWriter::new();
    let mut bytes = writer.write().unwrap();
    rewrite_cove_feature_bits(&mut bytes, FEATURE_TABLE_PROFILE | (1u64 << 63), 0);
    bytes
}

fn rewrite_cove_feature_bits(bytes: &mut [u8], required_features: u64, optional_features: u64) {
    bytes[16..24].copy_from_slice(&required_features.to_le_bytes());
    bytes[24..32].copy_from_slice(&optional_features.to_le_bytes());
    bytes[124..128].fill(0);
    let header_crc = checksum::crc32c(&bytes[..HEADER_SIZE]);
    bytes[124..128].copy_from_slice(&header_crc.to_le_bytes());

    let tail_start = bytes.len() - POSTSCRIPT_TOTAL_SIZE;
    bytes[tail_start..tail_start + 8].copy_from_slice(&required_features.to_le_bytes());
    bytes[tail_start + 8..tail_start + 16].copy_from_slice(&optional_features.to_le_bytes());
    bytes[tail_start + 60..tail_start + 64].fill(0);
    let postscript_crc = checksum::crc32c(&bytes[tail_start..tail_start + POSTSCRIPT_SIZE]);
    bytes[tail_start + 60..tail_start + 64].copy_from_slice(&postscript_crc.to_le_bytes());
}

fn collation_registry_payload(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = (entries.len() as u32).to_le_bytes().to_vec();
    for (name, metadata) in entries {
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&(metadata.len() as u16).to_le_bytes());
        out.extend_from_slice(metadata);
    }
    out
}

fn collation_registry_bad_utf8_payload() -> Vec<u8> {
    let mut out = 1u32.to_le_bytes().to_vec();
    out.extend_from_slice(&1u16.to_le_bytes());
    out.push(0xff);
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

fn page_index_payload(row_count: u32, null_count: u32, encoding: u16) -> Vec<u8> {
    let mut out = 1u32.to_le_bytes().to_vec();
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&row_count.to_le_bytes());
    out.extend_from_slice(&null_count.to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes());
    out.extend_from_slice(&encoding.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out
}

fn zone_stat_scalar(value: &[u8]) -> [u8; STAT_SCALAR_ENCODED_LEN] {
    let mut out = [0u8; STAT_SCALAR_ENCODED_LEN];
    out[0] = 1;
    out[2..4].copy_from_slice(&(value.len() as u16).to_le_bytes());
    out[4..4 + value.len()].copy_from_slice(value);
    out
}

fn zone_stats_payload(row_count: u32, null_count: u32, non_null_count: u32) -> Vec<u8> {
    let mut out = [0u8; ZONE_STATS_ENTRY_LEN];
    out[0..4].copy_from_slice(&1u32.to_le_bytes());
    out[4..8].copy_from_slice(&2u32.to_le_bytes());
    out[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    out[12..16].copy_from_slice(&3u32.to_le_bytes());
    out[16..20].copy_from_slice(&row_count.to_le_bytes());
    out[20..24].copy_from_slice(&null_count.to_le_bytes());
    out[24..28].copy_from_slice(&non_null_count.to_le_bytes());
    out[28..32].copy_from_slice(&5u32.to_le_bytes());
    out[32..36].copy_from_slice(&2u32.to_le_bytes());
    out[36..40].copy_from_slice(&ZoneStatFlags::HAS_MIN_MAX.bits().to_le_bytes());
    out[40..60].copy_from_slice(&zone_stat_scalar(&1i64.to_le_bytes()));
    out[60..80].copy_from_slice(&zone_stat_scalar(&9i64.to_le_bytes()));
    out.to_vec()
}

fn valid_zone_stats_payload() -> Vec<u8> {
    zone_stats_payload(10, 2, 8)
}

fn invalid_zone_stats_payload() -> Vec<u8> {
    zone_stats_payload(10, 2, 7)
}

fn digest_manifest_payload(
    section_id: u32,
    algorithm: DigestAlgorithm,
    payload: &[u8],
) -> Result<Vec<u8>, cove_core::CoveError> {
    let digest = compute_digest(algorithm, payload)?;
    DigestManifest {
        algorithm,
        scope: DigestScope::Section,
        root_digest: [0; 32],
        entries: vec![DigestEntry {
            target_kind: DigestTargetKind::Section,
            section_id,
            local_id: 0,
            offset: 0,
            length: payload.len() as u64,
            digest,
        }],
    }
    .serialize()
}

fn digest_manifest_wrong_len_payload() -> Vec<u8> {
    let mut out = DigestManifest {
        algorithm: DigestAlgorithm::Sha256,
        scope: DigestScope::Section,
        root_digest: [0; 32],
        entries: vec![DigestEntry {
            target_kind: DigestTargetKind::Section,
            section_id: 7,
            local_id: 0,
            offset: 0,
            length: 4,
            digest: vec![0u8; 32],
        }],
    }
    .serialize()
    .unwrap();
    let digest_len_pos = cove_core::digest::DIGEST_MANIFEST_HEADER_LEN + 2;
    out[digest_len_pos..digest_len_pos + 2].copy_from_slice(&4u16.to_le_bytes());
    out.truncate(cove_core::digest::DIGEST_MANIFEST_HEADER_LEN + 32 + 4);
    out[16..24].copy_from_slice(&(36u64).to_le_bytes());
    out[56..60].fill(0);
    let crc = checksum::crc32c(&out[..cove_core::digest::DIGEST_MANIFEST_HEADER_LEN]);
    out[56..60].copy_from_slice(&crc.to_le_bytes());
    out
}

fn digest_manifest_bad_checksum_payload() -> Vec<u8> {
    let mut out =
        digest_manifest_payload(7, DigestAlgorithm::Sha256, b"payload").expect("digest manifest");
    out[0] ^= 0xFF;
    out
}

fn redaction_manifest_payload() -> Vec<u8> {
    let mut out = 1u32.to_le_bytes().to_vec();
    out.extend_from_slice(&7u64.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&42u64.to_le_bytes());
    out.extend_from_slice(&17u16.to_le_bytes());
    out.extend_from_slice(&11u16.to_le_bytes());
    out.extend_from_slice(b"policy/gdpr");
    out.extend_from_slice(&9u16.to_le_bytes());
    out.extend_from_slice(b"ticket-42");
    out.extend_from_slice(&1_700_000_000_000_000i64.to_le_bytes());
    out
}

fn lakehouse_hints_payload(catalog: &str, provenance: &str) -> Vec<u8> {
    let mut out = vec![0u8; 32];
    out.extend_from_slice(&1u32.to_le_bytes());
    write_len_prefixed(&mut out, b"date");
    write_len_prefixed(&mut out, b"2026-05-04");
    out.push(0);
    write_len_prefixed(&mut out, catalog.as_bytes());
    write_len_prefixed(&mut out, provenance.as_bytes());
    out.extend_from_slice(&[0u8; 32]);
    out
}

fn lakehouse_overlay_guard_payload() -> Vec<u8> {
    LakehouseHints {
        schema_fingerprint: [0x11; 32],
        partition_values: vec![("date".into(), "2026-05-04".into())],
        source_snapshot: Some(123),
        sequence_number: Some(456),
        catalog_identifier: "catalog://cove".into(),
        provenance: "generated".into(),
        conversion_digest: [0x22; 32],
        visibility_overlay: Some(LakehouseVisibilityOverlayRef {
            overlay_kind: 1,
            file_id: Some([0x33; 16]),
            file_len: Some(4096),
            footer_crc32c: Some(0x1234_5678),
            digest: Some([0x44; 32]),
            reference: "s3://bucket/deletes.dv".into(),
        }),
    }
    .serialize()
    .unwrap()
}

fn lakehouse_hints_bad_utf8_payload() -> Vec<u8> {
    let mut out = vec![0u8; 32];
    out.extend_from_slice(&0u32.to_le_bytes());
    out.push(0);
    write_len_prefixed(&mut out, &[0xff]);
    write_len_prefixed(&mut out, b"");
    out.extend_from_slice(&[0u8; 32]);
    out
}

struct DictionaryFixtureEntry {
    value_tag: ValueTag,
    storage_class: StorageClass,
    canonical_bytes: Vec<u8>,
}

fn valid_file_dictionary_fixture_payload() -> Result<Vec<u8>, cove_core::CoveError> {
    dictionary_fixture_payload(&[
        DictionaryFixtureEntry {
            value_tag: ValueTag::Utf8,
            storage_class: StorageClass::Inline,
            canonical_bytes: CanonicalValue::Utf8("active").encode()?,
        },
        DictionaryFixtureEntry {
            value_tag: ValueTag::DateDays,
            storage_class: StorageClass::Inline,
            canonical_bytes: CanonicalValue::DateDays(12).encode()?,
        },
        DictionaryFixtureEntry {
            value_tag: ValueTag::List,
            storage_class: StorageClass::Inline,
            canonical_bytes: CanonicalValue::List(vec![
                CanonicalValue::Bool(true),
                CanonicalValue::Utf8("x"),
            ])
            .encode()?,
        },
        DictionaryFixtureEntry {
            value_tag: ValueTag::Struct,
            storage_class: StorageClass::Inline,
            canonical_bytes: CanonicalValue::Struct(vec![
                CanonicalField {
                    field_id: 7,
                    value: CanonicalValue::Bool(false),
                },
                CanonicalField {
                    field_id: 1,
                    value: CanonicalValue::Int { width: 8, value: 9 },
                },
            ])
            .encode()?,
        },
        DictionaryFixtureEntry {
            value_tag: ValueTag::Map,
            storage_class: StorageClass::Inline,
            canonical_bytes: CanonicalValue::Map(vec![
                (CanonicalValue::Utf8("a"), CanonicalValue::Utf8("1")),
                (CanonicalValue::Utf8("b"), CanonicalValue::Utf8("2")),
            ])
            .encode()?,
        },
        DictionaryFixtureEntry {
            value_tag: ValueTag::Utf8,
            storage_class: StorageClass::Payload,
            canonical_bytes: CanonicalValue::Utf8("this is a payload-only dictionary value")
                .encode()?,
        },
        DictionaryFixtureEntry {
            value_tag: ValueTag::Utf8,
            storage_class: StorageClass::Redacted,
            canonical_bytes: Vec::new(),
        },
    ])
}

fn invalid_file_dictionary_bad_utf8_len_payload() -> Vec<u8> {
    dictionary_fixture_payload_unchecked(&[DictionaryFixtureEntry {
        value_tag: ValueTag::Utf8,
        storage_class: StorageClass::Inline,
        canonical_bytes: vec![5, b'a', b'b', b'c'],
    }])
}

fn invalid_file_dictionary_bad_map_duplicate_payload() -> Result<Vec<u8>, cove_core::CoveError> {
    let key = tagged_canonical_bytes(&CanonicalValue::Utf8("dup"))?;
    let value1 = tagged_canonical_bytes(&CanonicalValue::Utf8("v1"))?;
    let value2 = tagged_canonical_bytes(&CanonicalValue::Utf8("v2"))?;
    let mut map = cove_core::wire::encode_u64_leb128(2);
    map.extend_from_slice(&key);
    map.extend_from_slice(&value1);
    map.extend_from_slice(&key);
    map.extend_from_slice(&value2);
    Ok(dictionary_fixture_payload_unchecked(&[
        DictionaryFixtureEntry {
            value_tag: ValueTag::Map,
            storage_class: StorageClass::Payload,
            canonical_bytes: map,
        },
    ]))
}

fn invalid_file_dictionary_redacted_null_payload() -> Result<Vec<u8>, cove_core::CoveError> {
    dictionary_fixture_payload(&[DictionaryFixtureEntry {
        value_tag: ValueTag::Null,
        storage_class: StorageClass::Redacted,
        canonical_bytes: Vec::new(),
    }])
}

fn tagged_canonical_bytes(value: &CanonicalValue<'_>) -> Result<Vec<u8>, cove_core::CoveError> {
    let mut out = cove_core::wire::encode_u64_leb128(value.value_tag() as u64);
    out.extend_from_slice(&value.encode()?);
    Ok(out)
}

fn dictionary_fixture_payload(
    entries: &[DictionaryFixtureEntry],
) -> Result<Vec<u8>, cove_core::CoveError> {
    Ok(dictionary_fixture_payload_unchecked(entries))
}

fn dictionary_fixture_payload_unchecked(entries: &[DictionaryFixtureEntry]) -> Vec<u8> {
    let (index, payload) = dictionary_fixture_index_and_payload(entries);
    let mut out = Vec::with_capacity(4 + index.len() + payload.len());
    out.extend_from_slice(&(index.len() as u32).to_le_bytes());
    out.extend_from_slice(&index);
    out.extend_from_slice(&payload);
    out
}

fn dictionary_fixture_index_and_payload(entries: &[DictionaryFixtureEntry]) -> (Vec<u8>, Vec<u8>) {
    let mut index_entries = Vec::with_capacity(entries.len());
    let mut payload = Vec::new();
    for entry in entries {
        let mut inline_data = [0u8; 16];
        let (inline_len, payload_offset, payload_length) = match entry.storage_class {
            StorageClass::Inline => {
                assert!(entry.canonical_bytes.len() <= inline_data.len());
                inline_data[..entry.canonical_bytes.len()].copy_from_slice(&entry.canonical_bytes);
                (entry.canonical_bytes.len() as u8, 0, 0)
            }
            StorageClass::Payload => {
                let payload_offset = payload.len() as u64;
                payload.extend_from_slice(&entry.canonical_bytes);
                (0, payload_offset, entry.canonical_bytes.len() as u32)
            }
            StorageClass::Redacted => (0, 0, 0),
            _ => panic!("future storage class is not supported by conformance fixtures"),
        };
        index_entries.push(FileDictionaryIndexEntryV1 {
            value_tag: entry.value_tag as u16,
            storage_class: entry.storage_class as u8,
            flags: 0,
            inline_len,
            reserved0: [0; 3],
            inline_data,
            payload_offset,
            payload_length,
            canonical_hash64: 0,
            reserved1: 0,
        });
    }

    let header = FileDictionaryHeaderV1 {
        entry_count: entries.len() as u32,
        flags: 0,
        index_entry_len: FileDictionaryHeaderV1::INDEX_ENTRY_LEN,
        value_hash_algorithm: 0,
        payload_length: payload.len() as u64,
        reserved: [0; 24],
    };
    let mut index = header.serialize().to_vec();
    for entry in index_entries {
        index.extend_from_slice(&entry.serialize());
    }
    (index, payload)
}

fn write_len_prefixed(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
    out.extend_from_slice(bytes);
}

fn kernel_capabilities_payload(encoding: u16) -> Vec<u8> {
    let mut out = 1u32.to_le_bytes().to_vec();
    out.extend_from_slice(&encoding.to_le_bytes());
    out.extend_from_slice(&[
        1, // supports_eq
        1, // supports_in
        1, // supports_range
        1, // supports_is_null
        1, // supports_count
        1, // supports_min_max
        1, // supports_selection_decode
        0, // supports_direct_executioncode_remap
        2, // decode_cost_class
        3, // predicate_cost_class
        0, 0, 0, 0, 0, 0, // reserved
    ]);
    out
}

fn kernel_capabilities_payload_from_entry(encoding: CoveEncodingKind) -> Vec<u8> {
    KernelCapabilities {
        entries: vec![KernelCapabilityEntry {
            encoding,
            supports_eq: 1,
            supports_in: 1,
            supports_range: 1,
            supports_is_null: 1,
            supports_count: 1,
            supports_min_max: 1,
            supports_selection_decode: 1,
            supports_direct_executioncode_remap: 0,
            decode_cost_class: 2,
            predicate_cost_class: 3,
            reserved: [0; 6],
        }],
    }
    .serialize()
}

fn kernel_capabilities_reserved_payload() -> Vec<u8> {
    let mut bytes = kernel_capabilities_payload_from_entry(CoveEncodingKind::Rle);
    *bytes.last_mut().unwrap() = 1;
    bytes
}

fn kernel_capabilities_trailing_payload() -> Vec<u8> {
    let mut bytes = kernel_capabilities_payload_from_entry(CoveEncodingKind::Rle);
    bytes.push(0);
    bytes
}

fn exact_set_index_payload(codes: &[u64]) -> Vec<u8> {
    let mut data = Vec::new();
    for code in codes {
        data.extend_from_slice(&code.to_le_bytes());
    }
    let header = ExactSetIndexHeaderV1 {
        table_id: 1,
        column_id: 1,
        granularity: ExactSetGranularity::Morsel,
        key_kind: ExactSetKeyKind::FileCode,
        representation: ExactSetRepresentation::SortedList,
        flags: 0,
        entry_count: codes.len() as u32,
        data_offset: EXACT_SET_HEADER_LEN as u64,
        data_length: data.len() as u64,
        checksum: 0,
    };
    let mut out = header.serialize().to_vec();
    out.extend_from_slice(&data);
    out
}

fn bloom_index_payload(filter_count: u32, byte_len: u32) -> Vec<u8> {
    let header = BloomIndexHeaderV1 {
        table_id: 1,
        column_id: 1,
        granularity: BloomGranularity::Morsel,
        hash_domain: BloomHashDomain::FileCode,
        algorithm: BloomAlgorithm::SplitBlock,
        flags: 0,
        target_fpr_ppm: 10_000,
        filter_count,
        data_offset: BLOOM_INDEX_HEADER_LEN as u64,
        data_length: byte_len as u64,
        checksum: 0,
    };
    let mut out = header.serialize().to_vec();
    out.extend(std::iter::repeat(0u8).take(byte_len as usize));
    out
}

fn inverted_index_payload(keys: &[u64]) -> Vec<u8> {
    let bitmap_offset = INVERTED_MORSEL_INDEX_HEADER_LEN + keys.len() * INVERTED_MORSEL_ENTRY_LEN;
    let header = InvertedMorselIndexHeaderV1 {
        table_id: 1,
        column_id: 1,
        key_kind: InvertedKeyKind::FileCode,
        flags: 0,
        representation: 0,
        reserved: 0,
        entry_count: keys.len() as u32,
        entries_offset: INVERTED_MORSEL_INDEX_HEADER_LEN as u64,
        bitmap_data_offset: bitmap_offset as u64,
        checksum: 0,
    };
    let mut out = header.serialize().to_vec();
    for (idx, key) in keys.iter().enumerate() {
        let entry = InvertedEntry {
            key: *key,
            morsel_bitmap_offset: idx as u64,
            morsel_bitmap_length: 1,
            row_bitmap_offset: 0,
            row_bitmap_length: 0,
        };
        out.extend_from_slice(&entry.serialize());
    }
    out.extend(std::iter::repeat(0xff).take(keys.len().max(1)));
    out
}

fn lookup_index_payload(rows: &[RowRef]) -> Vec<u8> {
    lookup_index_payload_for_entries(&[(10, rows)])
}

fn lookup_index_unsorted_payload() -> Vec<u8> {
    let row = RowRef {
        table_id: 1,
        segment_id: 0,
        morsel_id: 0,
        row_in_morsel: 0,
    };
    lookup_index_payload_for_entries(&[(10, &[row]), (5, &[row])])
}

fn lookup_index_payload_for_entries(entries: &[(u64, &[RowRef])]) -> Vec<u8> {
    let mut entry_bytes = Vec::new();
    let mut rowref_bytes = Vec::new();
    let mut rowref_start = 0u32;
    for (key, rows) in entries {
        entry_bytes.extend_from_slice(&key.to_le_bytes());
        entry_bytes.extend_from_slice(&rowref_start.to_le_bytes());
        entry_bytes.extend_from_slice(&(rows.len() as u32).to_le_bytes());
        for row in *rows {
            rowref_bytes.extend_from_slice(&row.encode());
        }
        rowref_start += rows.len() as u32;
    }
    let rowref_offset = LOOKUP_INDEX_HEADER_LEN + entry_bytes.len();
    let header = LookupIndexHeaderV1 {
        table_id: 1,
        column_id: 1,
        key_kind: LookupKeyKind::FileCode,
        index_kind: LookupIndexKind::SparseSorted,
        uniqueness: LookupUniqueness::NonUnique,
        flags: 0,
        entry_count: entries.len() as u64,
        entries_offset: LOOKUP_INDEX_HEADER_LEN as u64,
        entries_length: entry_bytes.len() as u64,
        rowref_offset: rowref_offset as u64,
        rowref_length: rowref_bytes.len() as u64,
        checksum: 0,
    };
    let mut out = header.serialize().to_vec();
    out.extend_from_slice(&entry_bytes);
    out.extend_from_slice(&rowref_bytes);
    out
}

fn aggregate_synopsis_payload(count: u64) -> Vec<u8> {
    AggregateEntry {
        table_id: 1,
        segment_id: 0,
        morsel_id: u32::MAX,
        column_id: 1,
        synopsis_kind: SynopsisKind::Count,
        key_kind: 0,
        accuracy: SynopsisAccuracy::Exact,
        flags: 0,
        row_count: count as u32,
        null_count: 0,
        payload_offset: 0,
        payload_length: 0,
        checksum: 0,
    }
    .serialize()
    .to_vec()
}

fn aggregate_synopsis_unknown_kind_payload() -> Vec<u8> {
    let mut out = aggregate_synopsis_payload(1);
    out[16] = 99;
    out[44..48].fill(0);
    let crc = checksum::crc32c(&out);
    out[44..48].copy_from_slice(&crc.to_le_bytes());
    out
}

fn composite_index_payload(key_column_count: u8) -> Vec<u8> {
    let mut key_column_bytes = Vec::new();
    for column_id in 0..key_column_count {
        key_column_bytes.extend_from_slice(&(column_id as u32 + 1).to_le_bytes());
    }
    let entry_bytes = if key_column_count == 0 {
        Vec::new()
    } else {
        vec![0xA5; 8]
    };
    let entries_offset = COMPOSITE_ZONE_INDEX_HEADER_LEN + key_column_bytes.len();
    let header = CompositeZoneIndexHeaderV1 {
        table_id: 1,
        key_column_count: key_column_count as u16,
        transform_kind: CompositeTransformKind::Tuple,
        flags: 0,
        zone_count: if key_column_count == 0 { 0 } else { 1 },
        key_columns_offset: COMPOSITE_ZONE_INDEX_HEADER_LEN as u64,
        entries_offset: entries_offset as u64,
        entries_length: entry_bytes.len() as u64,
        checksum: 0,
    };
    let mut out = header.serialize().to_vec();
    out.extend_from_slice(&key_column_bytes);
    out.extend_from_slice(&entry_bytes);
    out
}

fn topn_summary_payload(entries: &[(u64, u64)]) -> Vec<u8> {
    let mut payload = Vec::new();
    for (code, frequency) in entries {
        payload.extend_from_slice(&code.to_le_bytes());
        payload.extend_from_slice(&frequency.to_le_bytes());
    }
    let summary = TopNSummary {
        table_id: 1,
        column_id: 1,
        segment_id: 0,
        morsel_id: 0,
        direction: TopNDirection::Largest,
        value_count: entries.len() as u16,
        flags: 0,
        payload_offset: TOPN_ZONE_SUMMARY_LEN as u64,
        payload_length: payload.len() as u64,
        checksum: 0,
        payload,
    };
    let mut out = summary.serialize_header().to_vec();
    out.extend_from_slice(&summary.payload);
    out
}

fn topn_summary_bad_direction_payload() -> Vec<u8> {
    let mut out = topn_summary_payload(&[(1, 100)]);
    out[16] = 99;
    out[36..40].fill(0);
    let crc = checksum::crc32c(&out[..TOPN_ZONE_SUMMARY_LEN]);
    out[36..40].copy_from_slice(&crc.to_le_bytes());
    out
}

fn engine_registry_payload(namespaces: &[&str]) -> Result<Vec<u8>, cove_core::CoveError> {
    let profiles = namespaces
        .iter()
        .enumerate()
        .map(|(idx, namespace)| EngineProfileEntryV1 {
            profile_id: idx as u32 + 1,
            namespace: (*namespace).into(),
            profile_name: "engine-dictionary-code".into(),
            version_major: 1,
            version_minor: 0,
            required_features: 0,
            optional_features: 0,
            execution_descriptor_ref: 2,
            mount_policy_ref: 3,
            private_payload_ref: 0,
            checksum: 0,
        })
        .collect();
    EngineProfileRegistry { flags: 0, profiles }.serialize()
}

fn valid_execution_descriptor() -> ExecutionCodeDescriptorV1 {
    ExecutionCodeDescriptorV1 {
        descriptor_id: 1,
        code_kind: ExecutionCodeKind::DictionaryKey,
        code_width_bits: 32,
        byte_order: 0,
        lifetime: ExecutionCodeLifetime::Scan,
        comparison_scope: ExecutionCodeComparisonScope::File,
        canonicality: ExecutionCodeCanonicality::Transient,
        null_code_policy: NullCodePolicy::NullBitmapOnly,
        flags: 0,
        scope_ref: 0,
        code_space_ref: 0,
        checksum: 0,
    }
}

fn valid_execution_scope_descriptor() -> ExecutionScopeDescriptorV1 {
    ExecutionScopeDescriptorV1 {
        scope_id: 2,
        scope_kind: ExecutionScopeKind::Catalog,
        flags: 0,
        stable_id: b"catalog/main".to_vec(),
        display_name: "main catalog".into(),
        private_payload_ref: 0,
    }
}

fn invalid_execution_scope_descriptor_payload() -> Vec<u8> {
    let mut bytes = valid_execution_scope_descriptor().serialize().unwrap();
    bytes[4..6].copy_from_slice(&99u16.to_le_bytes());
    bytes
}

fn valid_code_space_descriptor() -> CodeSpaceDescriptorV1 {
    CodeSpaceDescriptorV1 {
        code_space_id: 3,
        namespace: "org.example.engine".into(),
        stable_id: b"space-1".to_vec(),
        epoch: 7,
        flags: 0,
        private_payload_ref: 0,
    }
}

fn invalid_code_space_descriptor_payload() -> Vec<u8> {
    let mut bytes = valid_code_space_descriptor().serialize().unwrap();
    bytes[4..6].copy_from_slice(&1u16.to_le_bytes());
    bytes[6] = 0xff;
    bytes
}

fn invalid_execution_descriptor_payload() -> Vec<u8> {
    let mut bytes = valid_execution_descriptor().serialize().to_vec();
    bytes[4] = 42;
    bytes[24..28].fill(0);
    let crc = checksum::crc32c(&bytes);
    bytes[24..28].copy_from_slice(&crc.to_le_bytes());
    bytes
}

fn valid_mount_policy() -> EngineMountPolicyV1 {
    EngineMountPolicyV1 {
        policy_id: 1,
        filecode_mapping_kind: FileCodeMappingKind::MapToExecutionCode,
        missing_value_policy: MissingValuePolicy::DecodeValueOnly,
        stale_mapping_policy: StaleMappingPolicy::IgnoreIfOptional,
        reverse_lookup_policy: ReverseLookupPolicy::BuildFromDictionary,
        flags: 0,
        dictionary_digest_ref: 0,
        code_space_ref: 2,
        cache_key_ref: 0,
        private_payload_ref: 0,
        checksum: 0,
    }
}

fn engine_registry_payload_with_refs(
    execution_descriptor_ref: u32,
    mount_policy_ref: u32,
) -> Result<Vec<u8>, cove_core::CoveError> {
    EngineProfileRegistry {
        flags: 0,
        profiles: vec![EngineProfileEntryV1 {
            profile_id: 1,
            namespace: "org.example".into(),
            profile_name: "engine-dictionary-code".into(),
            version_major: 1,
            version_minor: 0,
            required_features: 0,
            optional_features: 0,
            execution_descriptor_ref,
            mount_policy_ref,
            private_payload_ref: 0,
            checksum: 0,
        }],
    }
    .serialize()
}

fn valid_execution_descriptor_with_refs(
    descriptor_id: u32,
    scope_ref: u32,
    code_space_ref: u32,
) -> ExecutionCodeDescriptorV1 {
    ExecutionCodeDescriptorV1 {
        descriptor_id,
        code_kind: ExecutionCodeKind::DictionaryKey,
        code_width_bits: 32,
        byte_order: 0,
        lifetime: ExecutionCodeLifetime::Scan,
        comparison_scope: ExecutionCodeComparisonScope::File,
        canonicality: ExecutionCodeCanonicality::Transient,
        null_code_policy: NullCodePolicy::NullBitmapOnly,
        flags: 0,
        scope_ref,
        code_space_ref,
        checksum: 0,
    }
}

fn valid_mount_policy_with_refs(policy_id: u32, code_space_ref: u32) -> EngineMountPolicyV1 {
    EngineMountPolicyV1 {
        policy_id,
        filecode_mapping_kind: FileCodeMappingKind::MapToExecutionCode,
        missing_value_policy: MissingValuePolicy::DecodeValueOnly,
        stale_mapping_policy: StaleMappingPolicy::IgnoreIfOptional,
        reverse_lookup_policy: ReverseLookupPolicy::BuildFromDictionary,
        flags: 0,
        dictionary_digest_ref: 0,
        code_space_ref,
        cache_key_ref: 0,
        private_payload_ref: 0,
        checksum: 0,
    }
}

fn valid_execution_scope_descriptor_with_id(scope_id: u32) -> ExecutionScopeDescriptorV1 {
    ExecutionScopeDescriptorV1 {
        scope_id,
        scope_kind: ExecutionScopeKind::Catalog,
        flags: 0,
        stable_id: b"catalog/main".to_vec(),
        display_name: "main catalog".into(),
        private_payload_ref: 0,
    }
}

fn valid_code_space_descriptor_with_id(code_space_id: u32) -> CodeSpaceDescriptorV1 {
    CodeSpaceDescriptorV1 {
        code_space_id,
        namespace: "org.example.engine".into(),
        stable_id: b"space-1".to_vec(),
        epoch: 7,
        flags: 0,
        private_payload_ref: 0,
    }
}

fn cove_e_profile_bundle_file(required: bool, dangling_scope_ref: bool) -> Vec<u8> {
    let file_required_features = if required { FEATURE_ENGINE_PROFILE } else { 0 };
    let file_optional_features = if required { 0 } else { FEATURE_ENGINE_PROFILE };
    let section_required_features = if required { FEATURE_ENGINE_PROFILE } else { 0 };
    let section_optional_features = if required { 0 } else { FEATURE_ENGINE_PROFILE };
    let scope_id = 31;
    let code_space_id = 41;
    let scope_ref = if dangling_scope_ref { 99 } else { scope_id };
    semantic_profile_cove_file(
        PrimaryProfile::Mixed,
        file_required_features,
        file_optional_features,
        vec![
            SectionPayload {
                section_kind: SectionKind::EngineProfileRegistry as u16,
                profile: PrimaryProfile::EngineExecution as u8,
                flags: 0,
                item_count: 1,
                row_count: 0,
                compression: 0,
                alignment_log2: 0,
                required_features: section_required_features,
                optional_features: section_optional_features,
                data: engine_registry_payload_with_refs(11, 21).unwrap(),
            },
            SectionPayload {
                section_kind: SectionKind::ExecutionCodeDescriptor as u16,
                profile: PrimaryProfile::EngineExecution as u8,
                flags: 0,
                item_count: 1,
                row_count: 0,
                compression: 0,
                alignment_log2: 0,
                required_features: section_required_features,
                optional_features: section_optional_features,
                data: valid_execution_descriptor_with_refs(11, scope_ref, code_space_id)
                    .serialize()
                    .to_vec(),
            },
            SectionPayload {
                section_kind: SectionKind::ExecutionScopeDescriptor as u16,
                profile: PrimaryProfile::EngineExecution as u8,
                flags: 0,
                item_count: 1,
                row_count: 0,
                compression: 0,
                alignment_log2: 0,
                required_features: section_required_features,
                optional_features: section_optional_features,
                data: valid_execution_scope_descriptor_with_id(scope_id)
                    .serialize()
                    .unwrap(),
            },
            SectionPayload {
                section_kind: SectionKind::CodeSpaceDescriptor as u16,
                profile: PrimaryProfile::EngineExecution as u8,
                flags: 0,
                item_count: 1,
                row_count: 0,
                compression: 0,
                alignment_log2: 0,
                required_features: section_required_features,
                optional_features: section_optional_features,
                data: valid_code_space_descriptor_with_id(code_space_id)
                    .serialize()
                    .unwrap(),
            },
            SectionPayload {
                section_kind: SectionKind::EngineMountPolicy as u16,
                profile: PrimaryProfile::EngineExecution as u8,
                flags: 0,
                item_count: 1,
                row_count: 0,
                compression: 0,
                alignment_log2: 0,
                required_features: section_required_features,
                optional_features: section_optional_features,
                data: valid_mount_policy_with_refs(21, code_space_id)
                    .serialize()
                    .to_vec(),
            },
        ],
    )
}

fn invalid_mount_policy_payload() -> Vec<u8> {
    let mut bytes = valid_mount_policy().serialize().to_vec();
    bytes[4] = 42;
    bytes[28..32].fill(0);
    let crc = checksum::crc32c(&bytes);
    bytes[28..32].copy_from_slice(&crc.to_le_bytes());
    bytes
}

fn valid_harbor_mount_hints() -> HarborMountHintsV1 {
    HarborMountHintsV1 {
        harbor_profile_version_major: 1,
        harbor_profile_version_minor: 0,
        tenant_scope_ref: 1,
        code_space_ref: 2,
        lease_epoch: 3,
        dictionary_digest_ref: 0,
        catalog_digest_ref: 0,
        mount_cache_policy: 0,
        reserved: [0; 7],
        private_payload_ref: 0,
        checksum: 0,
    }
}

fn cove_h_mount_case_file() -> Vec<u8> {
    let dictionary_entries = [
        DictionaryFixtureEntry {
            value_tag: ValueTag::Utf8,
            storage_class: StorageClass::Inline,
            canonical_bytes: CanonicalValue::Utf8("red").encode().unwrap(),
        },
        DictionaryFixtureEntry {
            value_tag: ValueTag::Utf8,
            storage_class: StorageClass::Inline,
            canonical_bytes: CanonicalValue::Utf8("blue").encode().unwrap(),
        },
    ];
    let dictionary = dictionary_fixture_index_and_payload(&dictionary_entries);
    let catalog = TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 7,
            namespace: "public".into(),
            name: "items".into(),
            row_count: 0,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: vec![ColumnEntry {
                column_id: 1,
                name: "name".into(),
                logical: CoveLogicalType::Utf8,
                physical: CovePhysicalKind::FileCode,
                nullable: false,
                sort_order: 0,
                collation_id: 0,
                precision: 0,
                scale: 0,
                flags: 0,
            }],
        }],
    };
    semantic_profile_cove_file(
        PrimaryProfile::HarborExecution,
        FEATURE_TABLE_PROFILE | FEATURE_FILE_DICTIONARY | FEATURE_HARBOR_PROFILE,
        0,
        vec![
            SectionPayload {
                section_kind: SectionKind::FileDictionaryIndex as u16,
                profile: PrimaryProfile::Mixed as u8,
                flags: 0,
                item_count: dictionary_entries.len() as u64,
                row_count: 0,
                compression: 0,
                alignment_log2: 0,
                required_features: FEATURE_FILE_DICTIONARY,
                optional_features: 0,
                data: dictionary.0,
            },
            SectionPayload {
                section_kind: SectionKind::FileDictionaryPayload as u16,
                profile: PrimaryProfile::Mixed as u8,
                flags: 0,
                item_count: 1,
                row_count: 0,
                compression: 0,
                alignment_log2: 0,
                required_features: FEATURE_FILE_DICTIONARY,
                optional_features: 0,
                data: dictionary.1,
            },
            SectionPayload {
                section_kind: SectionKind::TableCatalog as u16,
                profile: PrimaryProfile::TableScan as u8,
                flags: 0,
                item_count: 1,
                row_count: 0,
                compression: 0,
                alignment_log2: 0,
                required_features: FEATURE_TABLE_PROFILE,
                optional_features: 0,
                data: catalog.serialize().unwrap(),
            },
            SectionPayload {
                section_kind: SectionKind::HarborMountHints as u16,
                profile: PrimaryProfile::HarborExecution as u8,
                flags: 0,
                item_count: 1,
                row_count: 0,
                compression: 0,
                alignment_log2: 0,
                required_features: FEATURE_HARBOR_PROFILE,
                optional_features: 0,
                data: valid_harbor_mount_hints().serialize().to_vec(),
            },
        ],
    )
}

fn invalid_harbor_mount_hints_payload() -> Vec<u8> {
    let mut data = valid_harbor_mount_hints().serialize().to_vec();
    data[29] = 1;
    data
}

fn valid_object_catalog() -> ObjectTypeCatalog {
    ObjectTypeCatalog {
        flags: 0,
        types: vec![ObjectTypeEntryV1 {
            object_type_id: 1,
            type_name: "Thing".into(),
            flags: cove_core::profile::cove_o::OBJECT_TYPE_FLAG_ENTITY_OBJECT,
            properties: vec![PropertyEntryV1 {
                property_id: 1,
                property_name: "active".into(),
                logical_type: CoveLogicalType::Bool,
                physical_kind: CovePhysicalKind::Boolean,
                nullable: false,
                collation_id: 0,
                flags: 0,
            }],
        }],
    }
}

fn invalid_object_catalog() -> ObjectTypeCatalog {
    let mut catalog = valid_object_catalog();
    let property = catalog.types[0].properties[0].clone();
    catalog.types[0].properties.push(property);
    catalog
}

fn old_layout_object_catalog_bytes() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&5u16.to_le_bytes());
    out.extend_from_slice(b"Thing");
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

fn valid_temporal_segment_index() -> TemporalSegmentIndex {
    TemporalSegmentIndex {
        flags: 0,
        entries: vec![temporal_segment_entry_for_rows(5, &valid_temporal_rows())],
    }
}

fn cove_o_object_catalog_section() -> SectionPayload {
    let catalog = valid_object_catalog();
    SectionPayload {
        section_kind: SectionKind::ObjectTypeCatalog as u16,
        profile: PrimaryProfile::ObjectTemporal as u8,
        flags: 0,
        item_count: catalog.types.len() as u64,
        row_count: 0,
        compression: 0,
        alignment_log2: 0,
        required_features: FEATURE_OBJECT_PROFILE,
        optional_features: 0,
        data: catalog.serialize().unwrap(),
    }
}

fn cove_o_temporal_segment_index_section(
    segments: &[(u32, &[TemporalRowEntryV1])],
) -> SectionPayload {
    let index = TemporalSegmentIndex {
        flags: 0,
        entries: segments
            .iter()
            .map(|(segment_id, rows)| temporal_segment_entry_for_rows(*segment_id, rows))
            .collect(),
    };
    SectionPayload {
        section_kind: SectionKind::TemporalSegmentIndex as u16,
        profile: PrimaryProfile::ObjectTemporal as u8,
        flags: 0,
        item_count: index.entries.len() as u64,
        row_count: segments.iter().map(|(_, rows)| rows.len() as u64).sum(),
        compression: 0,
        alignment_log2: 0,
        required_features: FEATURE_OBJECT_PROFILE,
        optional_features: 0,
        data: index.serialize().unwrap(),
    }
}

fn cove_o_temporal_segment_data_section(
    segment_id: u32,
    rows: &[TemporalRowEntryV1],
) -> SectionPayload {
    SectionPayload {
        section_kind: SectionKind::TemporalSegmentData as u16,
        profile: PrimaryProfile::ObjectTemporal as u8,
        flags: 0,
        item_count: 1,
        row_count: rows.len() as u64,
        compression: 0,
        alignment_log2: 0,
        required_features: FEATURE_OBJECT_PROFILE,
        optional_features: 0,
        data: temporal_segment_data_payload(segment_id, rows),
    }
}

fn cove_o_property_stats_only_file(
    required_features: u64,
    page_flags: u32,
    non_null_count: u32,
    null_count: u32,
) -> Vec<u8> {
    let rows = valid_temporal_rows();
    let segment_payload = temporal_segment_data_payload_with_property_stats_only(
        5,
        &rows,
        page_flags,
        non_null_count,
        null_count,
    );
    let mut index_entry = temporal_segment_entry_for_rows(5, &rows);
    index_entry.length = segment_payload.len() as u64;
    let index = TemporalSegmentIndex {
        flags: 0,
        entries: vec![index_entry],
    };
    semantic_profile_cove_file(
        PrimaryProfile::ObjectTemporal,
        required_features,
        0,
        vec![
            cove_o_object_catalog_section(),
            SectionPayload {
                section_kind: SectionKind::TemporalSegmentIndex as u16,
                profile: PrimaryProfile::ObjectTemporal as u8,
                flags: 0,
                item_count: 1,
                row_count: rows.len() as u64,
                compression: 0,
                alignment_log2: 0,
                required_features: FEATURE_OBJECT_PROFILE,
                optional_features: 0,
                data: index.serialize().unwrap(),
            },
            SectionPayload {
                section_kind: SectionKind::TemporalSegmentData as u16,
                profile: PrimaryProfile::ObjectTemporal as u8,
                flags: 0,
                item_count: 1,
                row_count: rows.len() as u64,
                compression: 0,
                alignment_log2: 0,
                required_features: FEATURE_OBJECT_PROFILE | FEATURE_PAGE_PAYLOAD_ELISION,
                optional_features: 0,
                data: segment_payload,
            },
        ],
    )
}

fn valid_temporal_rows() -> Vec<TemporalRowEntryV1> {
    vec![
        TemporalRowEntryV1 {
            timestamp_us: 10,
            csn: 1,
            branch_key: 0,
            goid: [0; 16],
            record_id: [0; 16],
            record_kind: RecordKind::Delta,
            prev_ref: None,
        },
        TemporalRowEntryV1 {
            timestamp_us: 20,
            csn: 2,
            branch_key: 0,
            goid: [1; 16],
            record_id: [1; 16],
            record_kind: RecordKind::Snapshot,
            prev_ref: Some(CoveRecordRefV1 {
                segment_id: 5,
                row_index: 0,
                target_kind: 0,
            }),
        },
    ]
}

fn temporal_segment_data_payload(segment_id: u32, rows: &[TemporalRowEntryV1]) -> Vec<u8> {
    let row_directory_offset = TEMPORAL_SEGMENT_HEADER_LEN as u64;
    let row_bytes = (rows.len() * TEMPORAL_ROW_ENTRY_LEN) as u64;
    let row_end = row_directory_offset + row_bytes;
    let payload = TemporalSegmentData {
        header: TemporalSegmentHeaderV1 {
            segment_id,
            object_type_id: 1,
            time_range_start_us: rows.first().map(|row| row.timestamp_us).unwrap_or(0),
            time_range_end_us: rows.last().map(|row| row.timestamp_us).unwrap_or(0),
            csn_min: rows.first().map(|row| row.csn).unwrap_or(0),
            csn_max: rows.last().map(|row| row.csn).unwrap_or(0),
            row_count: rows.len() as u32,
            morsel_count: u32::from(!rows.is_empty()),
            morsel_row_count: if rows.is_empty() {
                0
            } else {
                rows.len() as u32
            },
            column_count: 0,
            row_directory_offset,
            column_directory_offset: row_end,
            page_index_offset: row_end,
            data_offset: row_end,
            flags: 0,
            checksum: 0,
        },
        rows: rows.to_vec(),
        property_columns: Vec::new(),
    };
    let mut out = payload.header.serialize().to_vec();
    for row in &payload.rows {
        out.extend_from_slice(&row.serialize());
    }
    out
}

fn temporal_segment_data_payload_with_property_stats_only(
    segment_id: u32,
    rows: &[TemporalRowEntryV1],
    page_flags: u32,
    non_null_count: u32,
    null_count: u32,
) -> Vec<u8> {
    let row_directory_offset = TEMPORAL_SEGMENT_HEADER_LEN as u64;
    let row_bytes = (rows.len() * TEMPORAL_ROW_ENTRY_LEN) as u64;
    let row_end = row_directory_offset + row_bytes;
    let column_directory_offset = row_end;
    let page_index_offset = column_directory_offset + TABLE_COLUMN_DIRECTORY_ENTRY_LEN as u64;
    let page_index_length = COLUMN_PAGE_INDEX_ENTRY_LEN as u64;
    let data_offset = page_index_offset + page_index_length;
    let header = TemporalSegmentHeaderV1 {
        segment_id,
        object_type_id: 1,
        time_range_start_us: rows.first().map(|row| row.timestamp_us).unwrap_or(0),
        time_range_end_us: rows.last().map(|row| row.timestamp_us).unwrap_or(0),
        csn_min: rows.first().map(|row| row.csn).unwrap_or(0),
        csn_max: rows.last().map(|row| row.csn).unwrap_or(0),
        row_count: rows.len() as u32,
        morsel_count: u32::from(!rows.is_empty()),
        morsel_row_count: if rows.is_empty() {
            0
        } else {
            rows.len() as u32
        },
        column_count: 1,
        row_directory_offset,
        column_directory_offset,
        page_index_offset,
        data_offset,
        flags: 0,
        checksum: 0,
    };
    let directory = TableColumnDirectoryEntryV1 {
        column_id: 1,
        logical_type: CoveLogicalType::Bool,
        physical_kind: CovePhysicalKind::Boolean,
        flags: 0,
        page_index_offset,
        page_index_length,
        data_offset,
        data_length: 0,
        stats_ref: u32::MAX,
        domain_ref: u32::MAX,
        checksum: 0,
    };
    let page = ColumnPageIndexEntryV1 {
        column_id: 1,
        morsel_id: 0,
        row_count: rows.len() as u32,
        non_null_count,
        null_count,
        encoding_root: u32::MAX,
        page_offset: 0,
        page_length: 0,
        uncompressed_length: 0,
        stats_ref: 0,
        flags: page_flags,
        checksum: checksum::crc32c(&[]),
    };
    let mut out = header.serialize().to_vec();
    for row in rows {
        out.extend_from_slice(&row.serialize());
    }
    out.extend_from_slice(&directory.serialize());
    out.extend_from_slice(&page.serialize());
    out
}

fn trust_manifest_payload(segment_id: u32, rows: &[TemporalRowEntryV1]) -> Vec<u8> {
    let mut out = (rows.len() as u32).to_le_bytes().to_vec();
    let mut prev = [0u8; 32];
    for (row_index, row) in rows.iter().enumerate() {
        out.extend_from_slice(&segment_id.to_le_bytes());
        out.extend_from_slice(&(row_index as u32).to_le_bytes());
        prev = cove_core::trust_chain::chain(&prev, &row.trust_payload()).unwrap();
        out.extend_from_slice(&prev);
    }
    out
}

fn invalid_temporal_segment_index() -> TemporalSegmentIndex {
    TemporalSegmentIndex {
        flags: 0,
        entries: vec![temporal_segment_entry(1, 2, 2, 0, 0, 1)],
    }
}

fn temporal_segment_entry_for_rows(
    segment_id: u32,
    rows: &[TemporalRowEntryV1],
) -> TemporalSegmentIndexEntryV1 {
    let (delta_count, snapshot_count, baseline_count, tombstone_count) =
        temporal_row_kind_counts(rows);
    TemporalSegmentIndexEntryV1 {
        segment_id,
        object_type_id: 1,
        time_range_start_us: rows.first().map(|row| row.timestamp_us).unwrap_or(0),
        time_range_end_us: rows.last().map(|row| row.timestamp_us).unwrap_or(0),
        csn_min: rows.first().map(|row| row.csn).unwrap_or(0),
        csn_max: rows.last().map(|row| row.csn).unwrap_or(0),
        row_count: rows.len() as u32,
        delta_count,
        snapshot_count,
        baseline_count,
        tombstone_count,
        min_goid: rows.iter().map(|row| row.goid).min().unwrap_or([0; 16]),
        max_goid: rows.iter().map(|row| row.goid).max().unwrap_or([0; 16]),
        offset: 0,
        length: temporal_segment_data_payload(segment_id, rows).len() as u64,
        checksum: 0,
    }
}

fn temporal_row_kind_counts(rows: &[TemporalRowEntryV1]) -> (u32, u32, u32, u32) {
    let mut delta_count = 0;
    let mut snapshot_count = 0;
    let mut baseline_count = 0;
    let mut tombstone_count = 0;
    for row in rows {
        match row.record_kind {
            RecordKind::Delta => delta_count += 1,
            RecordKind::Snapshot => snapshot_count += 1,
            RecordKind::Baseline => baseline_count += 1,
            RecordKind::Tombstone => tombstone_count += 1,
            RecordKind::ReservedLegacyMaterializedDelta => {}
            _ => {}
        }
    }
    (delta_count, snapshot_count, baseline_count, tombstone_count)
}

fn temporal_segment_entry(
    segment_id: u32,
    row_count: u32,
    delta_count: u32,
    snapshot_count: u32,
    baseline_count: u32,
    tombstone_count: u32,
) -> TemporalSegmentIndexEntryV1 {
    TemporalSegmentIndexEntryV1 {
        segment_id,
        object_type_id: 1,
        time_range_start_us: 10,
        time_range_end_us: 20,
        csn_min: 1,
        csn_max: 2,
        row_count,
        delta_count,
        snapshot_count,
        baseline_count,
        tombstone_count,
        min_goid: [0; 16],
        max_goid: [1; 16],
        offset: 128,
        length: 4096,
        checksum: 0,
    }
}

fn cove_t_scan_table_file() -> Vec<u8> {
    let mut writer = ScanProfileCoveWriter::new(valid_table_catalog());
    writer.push_segment(ScanSegment::new(1, 0, 0, 10, 1));
    writer.write().unwrap()
}

fn cove_t_bool_numcode_file(declared_numeric: bool) -> Vec<u8> {
    let catalog = TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 1,
            namespace: "public".into(),
            name: "flags".into(),
            row_count: if declared_numeric { 3 } else { 0 },
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: vec![ColumnEntry {
                column_id: 1,
                name: "active_code".into(),
                logical: CoveLogicalType::Bool,
                physical: CovePhysicalKind::NumCode,
                nullable: false,
                sort_order: 0,
                collation_id: 0,
                precision: 0,
                scale: 0,
                flags: if declared_numeric {
                    COLUMN_FLAG_BOOL_DECLARED_NUMERIC
                } else {
                    0
                },
            }],
        }],
    };
    if declared_numeric {
        let mut writer = ScanProfileCoveWriter::new(catalog);
        writer.push_segment(ScanSegment::new(1, 0, 0, 3, 1));
        writer.write().unwrap()
    } else {
        let mut writer = MinimalCoveWriter::new();
        writer.primary_profile = PrimaryProfile::TableScan as u8;
        writer.required_features = FEATURE_TABLE_PROFILE;
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::TableCatalog as u16,
            profile: PrimaryProfile::TableScan as u8,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: FEATURE_TABLE_PROFILE,
            optional_features: 0,
            data: catalog.serialize().unwrap(),
        });
        writer.write().unwrap()
    }
}

fn cove_t_payload_elision_stats_only_all_null_file() -> Vec<u8> {
    let catalog = TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 1,
            namespace: "public".into(),
            name: "events".into(),
            row_count: 6,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: vec![ColumnEntry {
                column_id: 1,
                name: "status_code".into(),
                logical: CoveLogicalType::UInt32,
                physical: CovePhysicalKind::NumCode,
                nullable: true,
                sort_order: 0,
                collation_id: 0,
                precision: 0,
                scale: 0,
                flags: 0,
            }],
        }],
    };
    let mut segment = ScanSegment::new(1, 0, 0, 6, 1);
    segment.set_column_pages(
        1,
        vec![ScanPageSpec::new(6, Vec::new())
            .with_counts(0, 6)
            .with_encoding_root(u32::MAX)
            .with_flags(PAGE_FLAG_STATS_ONLY_CONSTANT | PAGE_FLAG_ALL_NULL)],
    );
    let mut writer = ScanProfileCoveWriter::new(catalog);
    writer.push_segment(segment);
    writer.write().unwrap()
}

fn stats_only_constant_catalog(
    logical: CoveLogicalType,
    physical: CovePhysicalKind,
) -> TableCatalog {
    TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 1,
            namespace: "public".into(),
            name: "events".into(),
            row_count: 6,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: vec![ColumnEntry {
                column_id: 1,
                name: "status_code".into(),
                logical,
                physical,
                nullable: false,
                sort_order: 0,
                collation_id: 0,
                precision: 0,
                scale: 0,
                flags: 0,
            }],
        }],
    }
}

fn cove_t_payload_elision_stats_only_all_non_null_file(stats: Option<ZoneStatsEntry>) -> Vec<u8> {
    let mut segment = ScanSegment::new(1, 0, 0, 6, 1);
    segment.set_column_pages(
        1,
        vec![ScanPageSpec::new(6, Vec::new())
            .with_counts(6, 0)
            .with_encoding_root(u32::MAX)
            .with_flags(PAGE_FLAG_STATS_ONLY_CONSTANT | PAGE_FLAG_ALL_NON_NULL)],
    );
    let mut writer = ScanProfileCoveWriter::new(value_stream_elision_catalog());
    writer.push_segment(segment);
    if let Some(stats) = stats {
        writer
            .push_zone_stats(&ZoneStatsSection {
                entries: vec![stats],
            })
            .unwrap();
    }
    writer.write().unwrap()
}

fn cove_t_payload_elision_stats_only_all_non_null_float32_file() -> Vec<u8> {
    let scalar = StatScalar {
        kind: StatKind::Float64Bits,
        bytes: 1.0f64.to_bits().to_le_bytes().to_vec(),
        truncated: false,
    };
    let mut stats = valid_constant_page_stats();
    stats.stats.min = Some(scalar.clone());
    stats.stats.max = Some(scalar);
    let mut segment = ScanSegment::new(1, 0, 0, 6, 1);
    segment.set_column_pages(
        1,
        vec![ScanPageSpec::new(6, Vec::new())
            .with_counts(6, 0)
            .with_encoding_root(u32::MAX)
            .with_flags(PAGE_FLAG_STATS_ONLY_CONSTANT | PAGE_FLAG_ALL_NON_NULL)],
    );
    let mut writer = ScanProfileCoveWriter::new(stats_only_constant_catalog(
        CoveLogicalType::Float32,
        CovePhysicalKind::NumCode,
    ));
    writer.push_segment(segment);
    writer
        .push_zone_stats(&ZoneStatsSection {
            entries: vec![stats],
        })
        .unwrap();
    writer.write().unwrap()
}

fn cove_t_payload_elision_value_stream_mixed_constant_file() -> Vec<u8> {
    value_stream_elided_file(CoveEncodingKind::Constant)
}

fn cove_t_payload_elision_value_stream_wrong_root_file() -> Vec<u8> {
    value_stream_elided_file(CoveEncodingKind::NumCode)
}

fn cove_t_payload_elision_value_stream_missing_bitmap_file() -> Vec<u8> {
    let bytes = value_stream_elided_file_without_nulls();
    rewrite_first_segment_page(bytes, |page| {
        page.non_null_count = 4;
        page.null_count = 2;
        page.flags |= PAGE_FLAG_VALUE_STREAM_ELIDED;
    })
}

fn cove_t_payload_elision_value_stream_missing_feature_file() -> Vec<u8> {
    clear_required_feature(
        cove_t_payload_elision_value_stream_mixed_constant_file(),
        FEATURE_PAGE_PAYLOAD_ELISION,
    )
}

fn value_stream_elided_file(encoding: CoveEncodingKind) -> Vec<u8> {
    let mut payload = vec![0b0010_0100];
    payload.extend_from_slice(
        &ConstantPayload {
            value: 42,
            row_count: 6,
        }
        .encode(),
    );
    let mut segment = ScanSegment::new(1, 0, 0, 6, 1);
    segment.set_column_pages(
        1,
        vec![ScanPageSpec::new(6, payload)
            .with_counts(4, 2)
            .with_encoding_root(encoding as u32)
            .with_flags(PAGE_FLAG_VALUE_STREAM_ELIDED)],
    );
    let mut writer = ScanProfileCoveWriter::new(value_stream_elision_catalog());
    writer.push_segment(segment);
    writer.write().unwrap()
}

fn value_stream_elided_file_without_nulls() -> Vec<u8> {
    let payload = ConstantPayload {
        value: 42,
        row_count: 6,
    }
    .encode()
    .to_vec();
    let mut segment = ScanSegment::new(1, 0, 0, 6, 1);
    segment.set_column_pages(
        1,
        vec![ScanPageSpec::new(6, payload)
            .with_counts(6, 0)
            .with_encoding_root(CoveEncodingKind::Constant as u32)
            .with_flags(PAGE_FLAG_VALUE_STREAM_ELIDED)],
    );
    let mut writer = ScanProfileCoveWriter::new(stats_only_constant_catalog(
        CoveLogicalType::Int64,
        CovePhysicalKind::NumCode,
    ));
    writer.push_segment(segment);
    writer.write().unwrap()
}

fn value_stream_elision_catalog() -> TableCatalog {
    let mut catalog =
        stats_only_constant_catalog(CoveLogicalType::Int64, CovePhysicalKind::NumCode);
    catalog.tables[0].columns[0].nullable = true;
    catalog
}

fn cove_t_numcode_page_short_values_file() -> Vec<u8> {
    let mut segment = ScanSegment::new(1, 0, 0, 6, 1);
    segment.set_column_pages(
        1,
        vec![ScanPageSpec::new(6, 7u64.to_le_bytes().to_vec())
            .with_counts(6, 0)
            .with_encoding_root(CoveEncodingKind::NumCode as u32)],
    );
    let mut writer = ScanProfileCoveWriter::new(stats_only_constant_catalog(
        CoveLogicalType::Int64,
        CovePhysicalKind::NumCode,
    ));
    writer.push_segment(segment);
    writer.write().unwrap()
}

fn valid_constant_page_stats() -> ZoneStatsEntry {
    constant_page_stats_with_flags(ZoneStatFlags::HAS_MIN_MAX | ZoneStatFlags::CONSTANT)
}

fn constant_page_stats_with_flags(flags: ZoneStatFlags) -> ZoneStatsEntry {
    let scalar = StatScalar {
        kind: StatKind::Int64,
        bytes: 42i64.to_le_bytes().to_vec(),
        truncated: false,
    };
    ZoneStatsEntry {
        table_id: 1,
        segment_id: 0,
        morsel_id: 0,
        column_id: 1,
        non_null_count: 6,
        distinct_count: 1,
        run_count: 1,
        stats: ZoneStats {
            scope: ZoneScope::Morsel,
            row_count: 6,
            null_count: 0,
            min: Some(scalar.clone()),
            max: Some(scalar),
            flags,
        },
        min_domain_rank: 0,
        max_domain_rank: 0,
        exact_set_ref: u32::MAX,
        bloom_ref: u32::MAX,
    }
}

fn wrong_scope_constant_page_stats() -> ZoneStatsEntry {
    let mut stats = valid_constant_page_stats();
    stats.morsel_id = 1;
    stats
}

fn cove_t_payload_elision_missing_feature_file() -> Vec<u8> {
    clear_required_feature(
        cove_t_payload_elision_stats_only_all_null_file(),
        FEATURE_PAGE_PAYLOAD_ELISION,
    )
}

fn write_cove_map_execution_cases(root: &PathBuf, entries: &mut Vec<Value>) {
    let map_path = "accept/cove_map_execution.covemap";
    let source_path = "accept/people.csv";
    write_fixture(
        root,
        entries,
        fixture(
            map_path,
            "covemap",
            "accept",
            None,
            &[
                "§70.2", "§70.3", "§70.5", "§70.6", "§70.9", "§70.10", "§70.12", "§70.13", "§72.8",
                "§73.6",
            ],
        ),
        cove_map_execution_file(),
    );
    write_auxiliary_file(root, source_path, cove_map_execution_source_bytes());

    let candidate_map_path = "accept/cove_map_candidate_identity.covemap";
    write_fixture(
        root,
        entries,
        fixture(
            candidate_map_path,
            "covemap",
            "accept",
            None,
            &["§70.3", "§70.4", "§70.6", "§72.8", "§73.6"],
        ),
        cove_map_candidate_identity_file(),
    );

    let candidate_map = root.join(candidate_map_path);
    let candidate_sources = vec![root.join(source_path)];
    let candidate_summary =
        cove_map::conversion_summary_from_paths(&candidate_map, &candidate_sources).unwrap();
    let candidate_report = candidate_summary
        .get("report")
        .cloned()
        .unwrap_or(Value::Null);
    write_fixture(
        root,
        entries,
        fixture(
            "accept/cove_map_candidate_identity_case.json",
            "cove_map_convert_case",
            "accept",
            None,
            &["§70.4", "§70.6", "§72.8", "§73.6"],
        ),
        suite_contract_fixture_bytes(json!({
            "mapping": candidate_map_path,
            "sources": [source_path],
            "expected_conversion": {
                "object_count": candidate_report["object_count"],
                "association_count": candidate_report["association_count"],
                "candidate_match_count": candidate_report["candidate_match_count"],
            },
            "expected_conversion_summary": {
                "materialized_row_count": candidate_summary["materialized_row_count"],
                "evidence_entry_count": candidate_summary["evidence_entry_count"],
                "assertion_count": candidate_summary["assertion_count"],
            }
        })),
    );

    let association_only_map_path = "accept/cove_map_association_only.covemap";
    write_fixture(
        root,
        entries,
        fixture(
            association_only_map_path,
            "covemap",
            "accept",
            None,
            &["§70.3", "§70.9", "§72.8", "§73.6"],
        ),
        cove_map_association_only_file(),
    );
    let association_only_summary = cove_map::conversion_summary_from_paths(
        &root.join(association_only_map_path),
        &[root.join(source_path)],
    )
    .unwrap();
    let association_only_report = association_only_summary
        .get("report")
        .cloned()
        .unwrap_or(Value::Null);
    write_fixture(
        root,
        entries,
        fixture(
            "accept/cove_map_association_only_case.json",
            "cove_map_convert_case",
            "accept",
            None,
            &["§70.3", "§70.9", "§72.8", "§73.6"],
        ),
        suite_contract_fixture_bytes(json!({
            "mapping": association_only_map_path,
            "sources": [source_path],
            "expected_conversion": {
                "object_count": association_only_report["object_count"],
                "association_count": association_only_report["association_count"],
            },
            "expect_cove_o_valid": true,
            "expect_association_readback_flags": true,
        })),
    );

    write_fixture(
        root,
        entries,
        fixture(
            "accept/cove_map_composite_row_semantics.covemap",
            "covemap",
            "accept",
            None,
            &["§70.3", "§70.9", "§72.8", "§73.6"],
        ),
        cove_map_composite_row_semantics_file(),
    );

    let tombstone_map_path = "accept/cove_map_tombstone_row_semantics.covemap";
    write_fixture(
        root,
        entries,
        fixture(
            tombstone_map_path,
            "covemap",
            "accept",
            None,
            &["§70.3", "§72.8", "§73.6"],
        ),
        cove_map_tombstone_row_semantics_file(),
    );
    write_fixture(
        root,
        entries,
        fixture(
            "accept/cove_map_tombstone_row_semantics_case.json",
            "cove_map_convert_case",
            "accept",
            None,
            &["§70.3", "§72.8", "§73.6"],
        ),
        suite_contract_fixture_bytes(json!({
            "mapping": tombstone_map_path,
            "sources": [source_path],
            "expected_conversion": {
                "object_count": 2,
                "association_count": 0,
            },
            "expect_cove_o_valid": true,
        })),
    );

    write_fixture(
        root,
        entries,
        fixture(
            "reject/cove_map_invalid_row_semantics.covemap",
            "covemap",
            "reject",
            Some("COVE_E_MAP_INVALID"),
            &["§70.3", "§76"],
        ),
        cove_map_invalid_row_semantics_file(),
    );

    let missing_policy_map_path = "reject/cove_map_projection_missing_policy.covemap";
    write_fixture(
        root,
        entries,
        fixture(
            missing_policy_map_path,
            "covemap",
            "reject",
            Some("COVE_E_MAP_INVALID"),
            &["§70.10", "§76"],
        ),
        cove_map_projection_missing_policy_file(),
    );

    let map = root.join(map_path);
    let sources = vec![root.join(source_path)];
    let summary = cove_map::conversion_summary_from_paths(&map, &sources).unwrap();
    let report = summary.get("report").cloned().unwrap_or(Value::Null);
    let projected = cove_map::projected_rows_from_paths(&map, &sources).unwrap();

    write_fixture(
        root,
        entries,
        fixture(
            "accept/cove_map_convert_case.json",
            "cove_map_convert_case",
            "accept",
            None,
            &[
                "§61", "§70.2", "§70.3", "§70.5", "§70.6", "§70.9", "§70.10", "§70.12", "§70.13",
                "§72.8", "§73.6",
            ],
        ),
        suite_contract_fixture_bytes(json!({
            "mapping": map_path,
            "sources": [source_path],
            "expected_conversion": {
                "mapping_id": report["mapping_id"],
                "mapping_version": report["mapping_version"],
                "source_count": report["source_count"],
                "row_count": report["row_count"],
                "object_count": report["object_count"],
                "association_count": report["association_count"],
                "property_value_count": report["property_value_count"],
            },
            "expected_conversion_summary": {
                "materialized_row_count": summary["materialized_row_count"],
                "evidence_entry_count": summary["evidence_entry_count"],
                "assertion_count": summary["assertion_count"],
            },
            "expect_cove_o_valid": true,
            "expect_semantic_map_optional": true,
            "expect_association_readback_flags": true,
        })),
    );
    write_fixture(
        root,
        entries,
        fixture(
            "reject/cove_map_missing_source.json",
            "cove_map_convert_case",
            "reject",
            Some("COVE_E_MAP_INVALID"),
            &["§70.2", "§73.6", "§76"],
        ),
        suite_contract_fixture_bytes(json!({
            "mapping": map_path,
            "sources": [],
        })),
    );

    write_fixture(
        root,
        entries,
        fixture(
            "accept/cove_map_project_case.json",
            "cove_map_project_case",
            "accept",
            None,
            &["§70.9", "§70.10", "§72.8", "§73.6"],
        ),
        suite_contract_fixture_bytes(json!({
            "mapping": map_path,
            "sources": [source_path],
            "expected_projection": {
                "format": projected["format"],
                "mapping_id": projected["mapping_id"],
                "mapping_version": projected["mapping_version"],
            },
            "expected_projected_rows": projected["rows"],
        })),
    );

    let crm_path = "accept/cove_map_crm.csv";
    let support_path = "accept/cove_map_support.csv";
    write_auxiliary_file(root, crm_path, cove_map_crm_source_bytes());
    write_auxiliary_file(root, support_path, cove_map_support_source_bytes());

    let priority_map_path = "accept/cove_map_source_priority.covemap";
    write_fixture(
        root,
        entries,
        fixture(
            priority_map_path,
            "covemap",
            "accept",
            None,
            &["§70.8", "§70.14", "§72.8", "§73.6"],
        ),
        cove_map_conflict_file("source_priority_wins", "emit_effective_policy"),
    );
    let priority_map = root.join(priority_map_path);
    let priority_sources = vec![root.join(crm_path), root.join(support_path)];
    let priority_summary =
        cove_map::conversion_summary_from_paths(&priority_map, &priority_sources).unwrap();
    let priority_report = priority_summary
        .get("report")
        .cloned()
        .unwrap_or(Value::Null);
    write_fixture(
        root,
        entries,
        fixture(
            "accept/cove_map_source_priority_case.json",
            "cove_map_convert_case",
            "accept",
            None,
            &["§70.8", "§70.14", "§72.8", "§73.6"],
        ),
        suite_contract_fixture_bytes(json!({
            "mapping": priority_map_path,
            "sources": [crm_path, support_path],
            "expected_conversion": {
                "mapping_id": priority_report["mapping_id"],
                "mapping_version": priority_report["mapping_version"],
                "property_value_count": priority_report["property_value_count"],
                "governance": priority_report["governance"],
            },
            "expected_conversion_summary": {
                "materialized_row_count": priority_summary["materialized_row_count"],
                "evidence_entry_count": priority_summary["evidence_entry_count"],
            },
            "expect_cove_o_valid": true,
        })),
    );

    let conflict_map_path = "accept/cove_map_property_conflict.covemap";
    write_auxiliary_file(
        root,
        conflict_map_path,
        &cove_map_conflict_file("reject_conflict", "emit_effective_policy"),
    );
    write_fixture(
        root,
        entries,
        fixture(
            "reject/cove_map_property_conflict_case.json",
            "cove_map_convert_case",
            "reject",
            Some("COVE_E_MAP_INVALID"),
            &["§70.8", "§72.8", "§76"],
        ),
        suite_contract_fixture_bytes(json!({
            "mapping": conflict_map_path,
            "sources": [crm_path, support_path],
        })),
    );

    let governance_reject_map_path = "accept/cove_map_mixed_governance_reject.covemap";
    write_auxiliary_file(
        root,
        governance_reject_map_path,
        &cove_map_conflict_file("source_priority_wins", "reject_on_mixed_sensitivity"),
    );
    write_fixture(
        root,
        entries,
        fixture(
            "reject/cove_map_mixed_governance_case.json",
            "cove_map_convert_case",
            "reject",
            Some("COVE_E_MAP_INVALID"),
            &["§70.14", "§72.8", "§76"],
        ),
        suite_contract_fixture_bytes(json!({
            "mapping": governance_reject_map_path,
            "sources": [crm_path, support_path],
        })),
    );
}

fn cove_map_execution_file() -> Vec<u8> {
    cove_map_file_with_sections([0x51; 16], cove_map_execution_sections())
}

fn cove_map_file_with_sections(file_id: [u8; 16], sections: Vec<CovemapSection>) -> Vec<u8> {
    let mut header = CovemapHeaderV1::new(file_id, 1_700_000_000_000_000);
    header.required_features = FEATURE_SEMANTIC_MAP;
    CovemapFile {
        header,
        mapping_version: "2026.05".into(),
        sections,
        postscript: CovemapPostscriptV1 {
            required_features: FEATURE_SEMANTIC_MAP,
            optional_features: 0,
            file_len: 0,
            header_offset: 0,
            header_length: 0,
            checksum: 0,
        },
    }
    .serialize()
    .unwrap()
}

fn cove_map_execution_sections() -> Vec<CovemapSection> {
    vec![
        covemap_section(
            SectionKind::MapSourceCatalog,
            json!({
                "mapping_id": "people-map",
                "mapping_version": "2026.05",
                "sources": [{
                    "source_id": "people",
                    "row_identity_rules": ["person_by_id", "team_by_id"]
                }]
            }),
        ),
        covemap_section(
            SectionKind::MapFunctionRegistry,
            json!({
                "mapping_id": "people-map",
                "mapping_version": "2026.05",
                "functions": [{
                    "function_id": "identity",
                    "version": "1.0.0",
                    "deterministic": true,
                    "dependency": "pure"
                }]
            }),
        ),
        covemap_section(
            SectionKind::MapIdentityRuleCatalog,
            json!({
                "mapping_id": "people-map",
                "mapping_version": "2026.05",
                "identity_rules": [
                    {
                        "rule_id": "person_by_id",
                        "object_type": "Person",
                        "semantic_role": "subject",
                        "confidence_class": "authoritative",
                        "candidate_only": false,
                        "property_conflicts_declared": true,
                        "function_ids": ["identity"],
                        "join_keys": [{
                            "role_id": "person_id",
                            "source_column": "person_id",
                            "logical_type": "utf8",
                            "canonicalization": "identity",
                            "null_policy": "reject",
                            "ordering": "declared"
                        }]
                    },
                    {
                        "rule_id": "team_by_id",
                        "object_type": "Team",
                        "semantic_role": "group",
                        "confidence_class": "authoritative",
                        "candidate_only": false,
                        "property_conflicts_declared": true,
                        "function_ids": ["identity"],
                        "join_keys": [{
                            "role_id": "team_id",
                            "source_column": "team_id",
                            "logical_type": "utf8",
                            "canonicalization": "identity",
                            "null_policy": "reject",
                            "ordering": "declared"
                        }]
                    }
                ],
                "do_not_merge": []
            }),
        ),
        covemap_section(
            SectionKind::MapRowSemanticsCatalog,
            json!({
                "mapping_id": "people-map",
                "mapping_version": "2026.05",
                "rules": [
                    {
                        "rule_id": "upsert_person",
                        "source_id": "people",
                        "identity_rule_id": "person_by_id",
                        "row_semantics_kind": "Object",
                        "assertion_kinds": ["object", "property", "association", "evidence"],
                        "function_ids": ["identity"],
                        "output_assertion_ids": ["person_name_assertion", "member_of_assertion"],
                        "association_endpoints": ["team_by_id"],
                        "property_bindings": [{
                            "assertion_id": "person_name_assertion",
                            "property_id": "person_name",
                            "property_name": "name",
                            "source_column": "person_name",
                            "logical_type": "utf8",
                            "nullable": false,
                            "missing_policy": "reject"
                        }],
                        "association_bindings": [{
                            "assertion_id": "member_of_assertion",
                            "association_type": "member_of",
                            "target_identity_rule_id": "team_by_id",
                            "source_endpoint_expression": "source.goid",
                            "target_endpoint_expression": "identity(team_by_id)",
                            "source_role": "member",
                            "target_role": "team",
                            "valid_from_expression": "source.valid_from",
                            "valid_to_expression": "source.valid_to",
                            "cardinality_policy": "many_to_one",
                            "missing_policy": "reject"
                        }]
                    },
                    {
                        "rule_id": "upsert_team",
                        "source_id": "people",
                        "identity_rule_id": "team_by_id",
                        "row_semantics_kind": "Object",
                        "assertion_kinds": ["object", "property", "evidence"],
                        "function_ids": ["identity"],
                        "output_assertion_ids": ["team_name_assertion"],
                        "association_endpoints": [],
                        "property_bindings": [{
                            "assertion_id": "team_name_assertion",
                            "property_id": "team_name",
                            "property_name": "team_name",
                            "source_column": "team_name",
                            "logical_type": "utf8",
                            "nullable": false,
                            "missing_policy": "reject"
                        }]
                    }
                ]
            }),
        ),
        covemap_section(
            SectionKind::MapProjectionCatalog,
            json!({
                "mapping_id": "people-map",
                "mapping_version": "2026.05",
                "projections": [
                    {
                        "projection_id": "person_projection",
                        "output_table": "people_projection",
                        "row_grain": "one_row_per_object",
                        "anchor": {"object_type": "Person"},
                        "temporal_mode": {"as_of": "latest_committed"},
                        "multi_value_policy": "aggregate",
                        "columns": [
                            {"name": "person_goid", "value": "object.goid", "logical_type": "uuid"},
                            {"name": "name", "value": "name", "logical_type": "utf8"},
                            {"name": "membership_count", "value": "count(association(member_of))", "logical_type": "uint64"}
                        ],
                        "output_modes": ["json", "cove-o"]
                    },
                    {
                        "projection_id": "membership_projection",
                        "output_table": "membership_projection",
                        "row_grain": "one_row_per_association",
                        "anchor": {"association_type": "member_of"},
                        "temporal_mode": {"as_of": "latest_committed"},
                        "multi_value_policy": "explode",
                        "columns": [
                            {"name": "association_goid", "value": "association.goid", "logical_type": "uuid"},
                            {"name": "source_goid", "value": "association.source_goid", "logical_type": "uuid"},
                            {"name": "target_goid", "value": "association.target_goid", "logical_type": "uuid"},
                            {"name": "source_role", "value": "association.source_role", "logical_type": "utf8"},
                            {"name": "target_role", "value": "association.target_role", "logical_type": "utf8"},
                            {"name": "valid_from", "value": "association.valid_from", "logical_type": "json"},
                            {"name": "valid_to", "value": "association.valid_to", "logical_type": "json"},
                            {"name": "cardinality_policy", "value": "association.cardinality_policy", "logical_type": "utf8"}
                        ],
                        "output_modes": ["json", "cove-o"]
                    }
                ]
            }),
        ),
    ]
}

fn cove_map_candidate_identity_file() -> Vec<u8> {
    cove_map_file_with_sections(
        [0x53; 16],
        vec![
            covemap_section(
                SectionKind::MapSourceCatalog,
                json!({
                    "mapping_id": "candidate-map",
                    "mapping_version": "2026.05",
                    "sources": [{
                        "source_id": "people",
                        "row_identity_rules": ["person_name_candidate"]
                    }]
                }),
            ),
            covemap_section(
                SectionKind::MapFunctionRegistry,
                json!({
                    "mapping_id": "candidate-map",
                    "mapping_version": "2026.05",
                    "functions": [{
                        "function_id": "identity",
                        "version": "1.0.0",
                        "deterministic": true,
                        "dependency": "pure"
                    }]
                }),
            ),
            covemap_section(
                SectionKind::MapIdentityRuleCatalog,
                json!({
                    "mapping_id": "candidate-map",
                    "mapping_version": "2026.05",
                    "identity_rules": [{
                        "rule_id": "person_name_candidate",
                        "object_type": "Person",
                        "semantic_role": "subject",
                        "confidence_class": "candidate",
                        "candidate_only": true,
                        "property_conflicts_declared": true,
                        "function_ids": ["identity"],
                        "join_keys": [{
                            "role_id": "person_name",
                            "source_column": "person_name",
                            "logical_type": "utf8",
                            "canonicalization": "identity",
                            "null_policy": "reject",
                            "ordering": "declared"
                        }]
                    }],
                    "do_not_merge": []
                }),
            ),
            covemap_section(
                SectionKind::MapRowSemanticsCatalog,
                json!({
                    "mapping_id": "candidate-map",
                    "mapping_version": "2026.05",
                    "rules": [{
                        "rule_id": "candidate_person",
                        "source_id": "people",
                        "identity_rule_id": "person_name_candidate",
                        "row_semantics_kind": "EvidenceOnly",
                        "assertion_kinds": ["candidate_match", "evidence"],
                        "function_ids": ["identity"],
                        "output_assertion_ids": [],
                        "association_endpoints": []
                    }]
                }),
            ),
        ],
    )
}

fn cove_map_association_only_file() -> Vec<u8> {
    let mut sections = cove_map_execution_sections();
    sections[3] = covemap_section(
        SectionKind::MapRowSemanticsCatalog,
        json!({
            "mapping_id": "people-map",
            "mapping_version": "2026.05",
            "rules": [
                {
                    "rule_id": "person_membership_only",
                    "source_id": "people",
                    "identity_rule_id": "person_by_id",
                    "row_semantics_kind": "AssociationOnly",
                    "assertion_kinds": ["association", "evidence"],
                    "function_ids": ["identity"],
                    "output_assertion_ids": ["member_of_assertion"],
                    "association_endpoints": ["team_by_id"],
                    "association_bindings": [{
                        "assertion_id": "member_of_assertion",
                        "association_type": "member_of",
                        "target_identity_rule_id": "team_by_id",
                        "source_endpoint_expression": "source.goid",
                        "target_endpoint_expression": "identity(team_by_id)",
                        "source_role": "member",
                        "target_role": "team",
                        "valid_from_expression": "source.valid_from",
                        "valid_to_expression": "source.valid_to",
                        "cardinality_policy": "many_to_one",
                        "missing_policy": "reject"
                    }]
                },
                {
                    "rule_id": "upsert_team",
                    "source_id": "people",
                    "identity_rule_id": "team_by_id",
                    "row_semantics_kind": "Object",
                    "assertion_kinds": ["object", "property", "evidence"],
                    "function_ids": ["identity"],
                    "output_assertion_ids": ["team_name_assertion"],
                    "association_endpoints": [],
                    "property_bindings": [{
                        "assertion_id": "team_name_assertion",
                        "property_id": "team_name",
                        "property_name": "team_name",
                        "source_column": "team_name",
                        "logical_type": "utf8",
                        "nullable": false,
                        "missing_policy": "reject"
                    }]
                }
            ]
        }),
    );
    cove_map_file_with_sections([0x54; 16], sections)
}

fn cove_map_composite_row_semantics_file() -> Vec<u8> {
    let mut sections = cove_map_execution_sections();
    sections[3] = covemap_section(
        SectionKind::MapRowSemanticsCatalog,
        json!({
            "mapping_id": "people-map",
            "mapping_version": "2026.05",
            "rules": [
                {
                    "rule_id": "upsert_person",
                    "source_id": "people",
                    "identity_rule_id": "person_by_id",
                    "row_semantics_kind": "Composite",
                    "assertion_kinds": ["object", "property", "association", "evidence"],
                    "function_ids": ["identity"],
                    "output_assertion_ids": ["person_name_assertion", "member_of_assertion"],
                    "association_endpoints": ["team_by_id"],
                    "property_bindings": [{
                        "assertion_id": "person_name_assertion",
                        "property_id": "person_name",
                        "property_name": "name",
                        "source_column": "person_name",
                        "logical_type": "utf8",
                        "nullable": false,
                        "missing_policy": "reject"
                    }],
                    "association_bindings": [{
                        "assertion_id": "member_of_assertion",
                        "association_type": "member_of",
                        "target_identity_rule_id": "team_by_id",
                        "source_endpoint_expression": "source.goid",
                        "target_endpoint_expression": "identity(team_by_id)",
                        "source_role": "member",
                        "target_role": "team",
                        "valid_from_expression": "source.valid_from",
                        "valid_to_expression": "source.valid_to",
                        "cardinality_policy": "many_to_one",
                        "missing_policy": "reject"
                    }]
                },
                {
                    "rule_id": "upsert_team",
                    "source_id": "people",
                    "identity_rule_id": "team_by_id",
                    "row_semantics_kind": "Object",
                    "assertion_kinds": ["object", "property", "evidence"],
                    "function_ids": ["identity"],
                    "output_assertion_ids": ["team_name_assertion"],
                    "association_endpoints": [],
                    "property_bindings": [{
                        "assertion_id": "team_name_assertion",
                        "property_id": "team_name",
                        "property_name": "team_name",
                        "source_column": "team_name",
                        "logical_type": "utf8",
                        "nullable": false,
                        "missing_policy": "reject"
                    }]
                }
            ]
        }),
    );
    cove_map_file_with_sections([0x55; 16], sections)
}

fn cove_map_tombstone_row_semantics_file() -> Vec<u8> {
    let mut sections = cove_map_execution_sections();
    sections.truncate(4);
    sections[3] = covemap_section(
        SectionKind::MapRowSemanticsCatalog,
        json!({
            "mapping_id": "people-map",
            "mapping_version": "2026.05",
            "rules": [{
                "rule_id": "delete_person",
                "source_id": "people",
                "identity_rule_id": "person_by_id",
                "row_semantics_kind": "Tombstone",
                "assertion_kinds": ["object", "tombstone", "evidence"],
                "tombstone_target": "object",
                "function_ids": ["identity"],
                "output_assertion_ids": [],
                "association_endpoints": []
            }]
        }),
    );
    cove_map_file_with_sections([0x56; 16], sections)
}

fn cove_map_invalid_row_semantics_file() -> Vec<u8> {
    let mut sections = cove_map_execution_sections();
    sections[3] = covemap_section(
        SectionKind::MapRowSemanticsCatalog,
        json!({
            "mapping_id": "people-map",
            "mapping_version": "2026.05",
            "rules": [{
                "rule_id": "bad_person",
                "source_id": "people",
                "identity_rule_id": "person_by_id",
                "row_semantics_kind": "ProjectionOnly",
                "assertion_kinds": ["object"]
            }]
        }),
    );
    cove_map_file_with_sections([0x57; 16], sections)
}

fn cove_map_projection_missing_policy_file() -> Vec<u8> {
    let mut sections = cove_map_execution_sections();
    sections[4] = covemap_section(
        SectionKind::MapProjectionCatalog,
        json!({
            "mapping_id": "people-map",
            "mapping_version": "2026.05",
            "projections": [{
                "projection_id": "person_projection",
                "output_table": "people_projection",
                "row_grain": "one_row_per_object",
                "anchor": {"object_type": "Person"},
                "temporal_mode": {"as_of": "latest_committed"},
                "columns": [
                    {"name": "person_goid", "value": "object.goid", "logical_type": "uuid"},
                    {"name": "name", "value": "name", "logical_type": "utf8"}
                ],
                "output_modes": ["json"]
            }]
        }),
    );
    cove_map_file_with_sections([0x58; 16], sections)
}

fn covemap_section(section_kind: SectionKind, value: Value) -> CovemapSection {
    let payload = map_payload_bytes(value);
    CovemapSection {
        entry: CovemapSectionEntryV1 {
            section_id: section_kind as u32,
            offset: 0,
            length: payload.len() as u64,
            uncompressed_length: payload.len() as u64,
            compression: 0,
            required: true,
            reserved: 0,
            checksum: 0,
        },
        payload,
    }
}

fn cove_map_execution_source_bytes() -> &'static [u8] {
    b"person_id,person_name,team_id,team_name,valid_from,valid_to\np1,Ada,t1,Core,2026-01-01,2026-12-31\np2,Linus,t2,Systems,2026-02-01,2026-12-31\n"
}

fn cove_map_crm_source_bytes() -> &'static [u8] {
    b"id,name\np1,CRM Name\n"
}

fn cove_map_support_source_bytes() -> &'static [u8] {
    b"id,name\np1,Support Name\n"
}

fn cove_map_conflict_file(conflict_policy: &str, governance_policy: &str) -> Vec<u8> {
    let mut header = CovemapHeaderV1::new([0x52; 16], 1_700_000_000_000_001);
    header.required_features = FEATURE_SEMANTIC_MAP;
    CovemapFile {
        header,
        mapping_version: "2026.05".into(),
        sections: cove_map_conflict_sections(conflict_policy, governance_policy),
        postscript: CovemapPostscriptV1 {
            required_features: FEATURE_SEMANTIC_MAP,
            optional_features: 0,
            file_len: 0,
            header_offset: 0,
            header_length: 0,
            checksum: 0,
        },
    }
    .serialize()
    .unwrap()
}

fn cove_map_conflict_sections(
    conflict_policy: &str,
    governance_policy: &str,
) -> Vec<CovemapSection> {
    vec![
        covemap_section(
            SectionKind::MapSourceCatalog,
            json!({
                "mapping_id": "people-priority-map",
                "mapping_version": "2026.05",
                "governance_reconciliation_policy": governance_policy,
                "sources": [
                    {
                        "source_id": "cove_map_crm",
                        "row_identity_rules": ["person_by_id"],
                        "source_priority": 10,
                        "sensitivity_label": "public",
                        "sensitivity_rank": 1,
                        "access_policy_ids": ["internal"]
                    },
                    {
                        "source_id": "cove_map_support",
                        "row_identity_rules": ["person_by_id"],
                        "source_priority": 1,
                        "sensitivity_label": "restricted",
                        "sensitivity_rank": 5,
                        "access_policy_ids": ["hipaa"]
                    }
                ]
            }),
        ),
        covemap_section(
            SectionKind::MapFunctionRegistry,
            json!({
                "mapping_id": "people-priority-map",
                "mapping_version": "2026.05",
                "functions": [{
                    "function_id": "identity",
                    "version": "1.0.0",
                    "deterministic": true,
                    "dependency": "pure"
                }]
            }),
        ),
        covemap_section(
            SectionKind::MapIdentityRuleCatalog,
            json!({
                "mapping_id": "people-priority-map",
                "mapping_version": "2026.05",
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
        ),
        covemap_section(
            SectionKind::MapRowSemanticsCatalog,
            json!({
                "mapping_id": "people-priority-map",
                "mapping_version": "2026.05",
                "rules": [
                    {
                        "rule_id": "crm_person",
                        "source_id": "cove_map_crm",
                        "identity_rule_id": "person_by_id",
                        "row_semantics_kind": "Object",
                        "assertion_kinds": ["object", "property", "evidence", "conflict"],
                        "function_ids": ["identity"],
                        "output_assertion_ids": ["crm_name_assertion"],
                        "association_endpoints": [],
                        "property_bindings": [{
                            "assertion_id": "crm_name_assertion",
                            "property_id": "name",
                            "property_name": "name",
                            "source_column": "name",
                            "logical_type": "utf8",
                            "nullable": true,
                            "conflict_policy": conflict_policy,
                            "missing_policy": "null"
                        }]
                    },
                    {
                        "rule_id": "support_person",
                        "source_id": "cove_map_support",
                        "identity_rule_id": "person_by_id",
                        "row_semantics_kind": "Object",
                        "assertion_kinds": ["object", "property", "evidence", "conflict"],
                        "function_ids": ["identity"],
                        "output_assertion_ids": ["support_name_assertion"],
                        "association_endpoints": [],
                        "property_bindings": [{
                            "assertion_id": "support_name_assertion",
                            "property_id": "name",
                            "property_name": "name",
                            "source_column": "name",
                            "logical_type": "utf8",
                            "nullable": true,
                            "conflict_policy": conflict_policy,
                            "missing_policy": "null"
                        }]
                    }
                ]
            }),
        ),
    ]
}

fn cove_map_valid_file() -> Vec<u8> {
    semantic_profile_cove_file(
        PrimaryProfile::SemanticMapping,
        FEATURE_SEMANTIC_MAP,
        0,
        valid_map_sections(),
    )
}

fn cove_map_invalid_file() -> Vec<u8> {
    semantic_profile_cove_file(
        PrimaryProfile::SemanticMapping,
        FEATURE_SEMANTIC_MAP,
        0,
        vec![map_section(
            SectionKind::MapSourceCatalog,
            1,
            json!({
                "mapping_version": "2026.05",
                "sources": [{
                    "source_id": "crm.customers",
                    "schema_fingerprint": "schema-v1",
                    "snapshot_digest": "digest-v1",
                    "row_identity_rules": ["customer_id"],
                    "replay_claimed": true
                }]
            }),
        )],
    )
}

fn cove_map_function_undeclared_file() -> Vec<u8> {
    let mut sections = valid_map_sections();
    sections[1] = map_section(
        SectionKind::MapFunctionRegistry,
        0,
        json!({
            "mapping_id": "customer-map",
            "mapping_version": "2026.05",
            "functions": []
        }),
    );
    semantic_profile_cove_file(
        PrimaryProfile::SemanticMapping,
        FEATURE_SEMANTIC_MAP,
        0,
        sections,
    )
}

fn cove_map_identity_conflict_file() -> Vec<u8> {
    let mut sections = valid_map_sections();
    sections[2] = map_section(
        SectionKind::MapIdentityRuleCatalog,
        1,
        json!({
            "mapping_id": "customer-map",
            "mapping_version": "2026.05",
            "identity_rules": [{
                "rule_id": "customer_identity",
                "object_type": "Customer",
                "semantic_role": "subject",
                "confidence_class": "authoritative",
                "candidate_only": false,
                "property_conflicts_declared": true,
                "function_ids": ["trim_lower"],
                "join_keys": [{
                    "role_id": "customer_id",
                    "source_column": "customer_id",
                    "logical_type": "utf8",
                    "canonicalization": "trim_lower",
                    "null_policy": "reject",
                    "ordering": "asc"
                }]
            }],
            "do_not_merge": [{
                "left_identity": "customer:1",
                "right_identity": "customer:2"
            }]
        }),
    );
    sections.push(map_section(
        SectionKind::MapIdentityEquivalenceIndex,
        1,
        json!({
            "mapping_id": "customer-map",
            "mapping_version": "2026.05",
            "equivalences": [{
                "left_identity": "customer:1",
                "right_identity": "customer:2"
            }]
        }),
    ));
    semantic_profile_cove_file(
        PrimaryProfile::SemanticMapping,
        FEATURE_SEMANTIC_MAP,
        0,
        sections,
    )
}

fn cove_map_source_stale_file() -> Vec<u8> {
    let mut sections = valid_map_sections();
    sections[6] = map_section(
        SectionKind::MapConversionReport,
        1,
        json!({
            "mapping_id": "customer-map",
            "mapping_version": "2026.05",
            "sources": [{
                "source_id": "crm.customers",
                "schema_fingerprint": "schema-v2",
                "snapshot_digest": "digest-v1"
            }]
        }),
    );
    semantic_profile_cove_file(
        PrimaryProfile::SemanticMapping,
        FEATURE_SEMANTIC_MAP,
        0,
        sections,
    )
}

fn cove_map_evidence_invalid_file() -> Vec<u8> {
    let mut sections = valid_map_sections();
    sections[5] = map_section(
        SectionKind::MapEvidenceIndex,
        1,
        json!({
            "mapping_id": "customer-map",
            "mapping_version": "2026.05",
            "entries": [{
                "source_id": "crm.customers",
                "source_row_identity": "customer_id=1",
                "rule_id": "upsert_customer",
                "assertion_id": "assert_missing",
                "output_object_id": "goid:customer:1",
                "observed_schema_fingerprint": "schema-v1",
                "observed_snapshot_digest": "digest-v1"
            }]
        }),
    );
    semantic_profile_cove_file(
        PrimaryProfile::SemanticMapping,
        FEATURE_SEMANTIC_MAP,
        0,
        sections,
    )
}

fn valid_map_sections() -> Vec<SectionPayload> {
    vec![
        map_section(
            SectionKind::MapSourceCatalog,
            1,
            json!({
                "mapping_id": "customer-map",
                "mapping_version": "2026.05",
                "sources": [{
                    "source_id": "crm.customers",
                    "schema_fingerprint": "schema-v1",
                    "snapshot_digest": "digest-v1",
                    "row_identity_rules": ["customer_id"],
                    "replay_claimed": true
                }]
            }),
        ),
        map_section(
            SectionKind::MapFunctionRegistry,
            1,
            json!({
                "mapping_id": "customer-map",
                "mapping_version": "2026.05",
                "functions": [{
                    "function_id": "trim_lower",
                    "version": "1.0.0",
                    "deterministic": true,
                    "dependency": "pure"
                }]
            }),
        ),
        map_section(
            SectionKind::MapIdentityRuleCatalog,
            1,
            json!({
                "mapping_id": "customer-map",
                "mapping_version": "2026.05",
                "identity_rules": [{
                    "rule_id": "customer_identity",
                    "object_type": "Customer",
                    "semantic_role": "subject",
                    "confidence_class": "authoritative",
                    "candidate_only": false,
                    "property_conflicts_declared": true,
                    "function_ids": ["trim_lower"],
                    "join_keys": [{
                        "role_id": "customer_id",
                        "source_column": "customer_id",
                        "logical_type": "utf8",
                        "canonicalization": "trim_lower",
                        "null_policy": "reject",
                        "ordering": "asc"
                    }]
                }],
                "do_not_merge": []
            }),
        ),
        map_section(
            SectionKind::MapRowSemanticsCatalog,
            1,
            json!({
                "mapping_id": "customer-map",
                "mapping_version": "2026.05",
                "rules": [{
                    "rule_id": "upsert_customer",
                    "source_id": "crm.customers",
                    "identity_rule_id": "customer_identity",
                    "row_semantics_kind": "Object",
                    "assertion_kinds": ["object", "property", "evidence"],
                    "function_ids": ["trim_lower"],
                    "output_assertion_ids": ["assert_customer_name"],
                    "association_endpoints": []
                }]
            }),
        ),
        map_section(
            SectionKind::MapAssertionLog,
            1,
            json!({
                "mapping_id": "customer-map",
                "mapping_version": "2026.05",
                "assertions": [{
                    "assertion_id": "assert_customer_name",
                    "output_object_id": "goid:customer:1"
                }]
            }),
        ),
        map_section(
            SectionKind::MapEvidenceIndex,
            1,
            json!({
                "mapping_id": "customer-map",
                "mapping_version": "2026.05",
                "entries": [{
                    "source_id": "crm.customers",
                    "source_row_identity": "customer_id=1",
                    "rule_id": "upsert_customer",
                    "assertion_id": "assert_customer_name",
                    "output_object_id": "goid:customer:1",
                    "observed_schema_fingerprint": "schema-v1",
                    "observed_snapshot_digest": "digest-v1"
                }]
            }),
        ),
        map_section(
            SectionKind::MapConversionReport,
            1,
            json!({
                "mapping_id": "customer-map",
                "mapping_version": "2026.05",
                "sources": [{
                    "source_id": "crm.customers",
                    "schema_fingerprint": "schema-v1",
                    "snapshot_digest": "digest-v1"
                }]
            }),
        ),
        map_section(
            SectionKind::MapProjectionCatalog,
            1,
            json!({
                "mapping_id": "customer-map",
                "mapping_version": "2026.05",
                "projections": [{
                    "projection_id": "customer_projection",
                    "assertion_ids": ["assert_customer_name"]
                }]
            }),
        ),
    ]
}

fn map_section(section_kind: SectionKind, item_count: u64, value: Value) -> SectionPayload {
    SectionPayload {
        section_kind: section_kind as u16,
        profile: PrimaryProfile::SemanticMapping as u8,
        flags: 0,
        item_count,
        row_count: 0,
        compression: 0,
        alignment_log2: 0,
        required_features: FEATURE_SEMANTIC_MAP,
        optional_features: 0,
        data: map_payload_bytes(value),
    }
}

fn clear_required_feature(mut bytes: Vec<u8>, feature: u64) -> Vec<u8> {
    let mut header = CoveHeaderV1::parse(&bytes).unwrap();
    header.required_features &= !feature;
    bytes[..HEADER_SIZE].copy_from_slice(&header.serialize());

    let mut postscript = CovePostscriptV1::parse_from_tail(&bytes).unwrap();
    postscript.required_features &= !feature;
    let tail_start = bytes.len() - POSTSCRIPT_TOTAL_SIZE;
    bytes[tail_start..].copy_from_slice(&postscript.serialize_tail());
    bytes
}

fn rewrite_first_segment_page(
    mut bytes: Vec<u8>,
    mutate: impl FnOnce(&mut ColumnPageIndexEntryV1),
) -> Vec<u8> {
    let mut postscript = CovePostscriptV1::parse_from_tail(&bytes).unwrap();
    let footer_start = postscript.footer.offset as usize;
    let footer_header = CoveFooterHeaderV1::parse(&bytes[footer_start..]).unwrap();
    let entries_start = footer_start + FOOTER_HEADER_SIZE;
    for index in 0..footer_header.section_count as usize {
        let entry_start = entries_start + index * SECTION_ENTRY_SIZE;
        let mut section_entry =
            CoveSectionEntryV1::parse(&bytes[entry_start..entry_start + SECTION_ENTRY_SIZE])
                .unwrap();
        if section_entry.section_kind != SectionKind::TableSegmentData as u16 {
            continue;
        }
        let segment_start = section_entry.offset as usize;
        let segment_end = segment_start + section_entry.length as usize;
        let segment = TableSegmentPayloadV1::parse(&bytes[segment_start..segment_end]).unwrap();
        let column = segment.columns.first().unwrap();
        let page_start = segment_start + column.page_index_offset as usize;
        let mut page = ColumnPageIndexEntryV1::parse(&bytes[page_start..page_start + 60]).unwrap();
        mutate(&mut page);
        bytes[page_start..page_start + 60].copy_from_slice(&page.serialize());
        section_entry.crc32c = checksum::crc32c(&bytes[segment_start..segment_end]);
        bytes[entry_start..entry_start + SECTION_ENTRY_SIZE]
            .copy_from_slice(&section_entry.serialize());

        let footer_end = footer_start + postscript.footer.length as usize;
        postscript.footer.crc32c = checksum::crc32c(&bytes[footer_start..footer_end]);
        let tail_start = bytes.len() - POSTSCRIPT_TOTAL_SIZE;
        bytes[tail_start..].copy_from_slice(&postscript.serialize_tail());
        return bytes;
    }
    panic!("generated COVE-T file did not contain TABLE_SEGMENT_DATA");
}

fn cove_t_local_codebook_lz4_file() -> Vec<u8> {
    let catalog = TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 1,
            namespace: "public".into(),
            name: "events".into(),
            row_count: 6,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: vec![ColumnEntry {
                column_id: 1,
                name: "status_code".into(),
                logical: CoveLogicalType::UInt32,
                physical: CovePhysicalKind::NumCode,
                nullable: false,
                sort_order: 0,
                collation_id: 0,
                precision: 0,
                scale: 0,
                flags: 0,
            }],
        }],
    };
    let payload = LocalCodebookPayload {
        values: LocalCodebookValues::NumCode(vec![100, 200, 300]),
        indexes: LocalIndexPayload::BitPacked(
            BitPackedPayload::pack(&[0, 1, 2, 1, 0, 2], 2).unwrap(),
        ),
    };
    let mut segment = ScanSegment::new(1, 0, 0, 6, 1);
    segment.set_column_pages(
        1,
        vec![ScanPageSpec::new(6, payload.encode())
            .with_compression(CompressionCodec::Lz4)
            .with_encoding_root(CoveEncodingKind::LocalCodebook as u32)],
    );
    let mut writer = ScanProfileCoveWriter::new(catalog);
    writer.push_segment(segment);
    writer.write().unwrap()
}

fn nested_column_catalog(
    column_name: &str,
    logical: CoveLogicalType,
    physical: CovePhysicalKind,
    row_count: u32,
) -> TableCatalog {
    TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 1,
            namespace: "public".into(),
            name: "events".into(),
            row_count: row_count as u64,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: vec![ColumnEntry {
                column_id: 1,
                name: column_name.into(),
                logical,
                physical,
                nullable: false,
                sort_order: 0,
                collation_id: 0,
                precision: 0,
                scale: 0,
                flags: 0,
            }],
        }],
    }
}

fn nested_column_cove_file(
    column_name: &str,
    logical: CoveLogicalType,
    physical: CovePhysicalKind,
    row_count: u32,
    payload: Vec<u8>,
) -> Vec<u8> {
    let mut segment = ScanSegment::new(1, 0, 0, row_count, 1);
    segment.set_column_pages(1, vec![ScanPageSpec::new(row_count, payload)]);
    let mut writer = ScanProfileCoveWriter::new(nested_column_catalog(
        column_name,
        logical,
        physical,
        row_count,
    ));
    writer.push_segment(segment);
    writer.write().unwrap()
}

fn cove_t_nested_list_valid_file() -> Vec<u8> {
    nested_column_cove_file(
        "tags",
        CoveLogicalType::List,
        CovePhysicalKind::List,
        3,
        ListLayoutPayload {
            layout: ListLayout {
                offsets: vec![0, 2, 2, 5],
            },
            child_row_count: 5,
        }
        .encode(),
    )
}

fn cove_t_nested_struct_valid_file() -> Vec<u8> {
    nested_column_cove_file(
        "address",
        CoveLogicalType::Struct,
        CovePhysicalKind::Struct,
        3,
        StructLayoutPayload {
            layout: StructLayout {
                field_row_counts: vec![3, 3],
            },
            parent_null_handling_declared: true,
        }
        .encode(),
    )
}

fn cove_t_nested_map_valid_file() -> Vec<u8> {
    nested_column_cove_file(
        "labels",
        CoveLogicalType::Map,
        CovePhysicalKind::Map,
        2,
        MapLayoutPayload {
            layout: MapLayout {
                offsets: vec![0, 2, 3],
                key_row_count: 3,
                value_row_count: 3,
                keys_are_scalar: true,
                allow_duplicate_keys: false,
                canonical_keys: vec![b"env".to_vec(), b"tier".to_vec(), b"env".to_vec()],
            },
        }
        .encode(),
    )
}

fn cove_t_nested_list_bad_child_count_file() -> Vec<u8> {
    nested_column_cove_file(
        "tags",
        CoveLogicalType::List,
        CovePhysicalKind::List,
        3,
        ListLayoutPayload {
            layout: ListLayout {
                offsets: vec![0, 2, 2, 5],
            },
            child_row_count: 4,
        }
        .encode(),
    )
}

fn cove_t_nested_struct_missing_null_handling_file() -> Vec<u8> {
    nested_column_cove_file(
        "address",
        CoveLogicalType::Struct,
        CovePhysicalKind::Struct,
        3,
        StructLayoutPayload {
            layout: StructLayout {
                field_row_counts: vec![3, 3],
            },
            parent_null_handling_declared: false,
        }
        .encode(),
    )
}

fn cove_t_nested_map_duplicate_keys_file() -> Vec<u8> {
    nested_column_cove_file(
        "labels",
        CoveLogicalType::Map,
        CovePhysicalKind::Map,
        2,
        MapLayoutPayload {
            layout: MapLayout {
                offsets: vec![0, 2, 3],
                key_row_count: 3,
                value_row_count: 3,
                keys_are_scalar: true,
                allow_duplicate_keys: false,
                canonical_keys: vec![b"env".to_vec(), b"env".to_vec(), b"tier".to_vec()],
            },
        }
        .encode(),
    )
}

fn valid_table_catalog() -> TableCatalog {
    TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 1,
            namespace: "public".into(),
            name: "events".into(),
            row_count: 10,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: vec![ColumnEntry {
                column_id: 1,
                name: "active".into(),
                logical: CoveLogicalType::Bool,
                physical: CovePhysicalKind::Boolean,
                nullable: false,
                sort_order: 0,
                collation_id: 0,
                precision: 0,
                scale: 0,
                flags: 0,
            }],
        }],
    }
}

fn duplicate_table_catalog() -> TableCatalog {
    let mut catalog = valid_table_catalog();
    let table = catalog.tables.remove(0);
    TableCatalog {
        flags: 0,
        tables: vec![table.clone(), table],
    }
}

fn bad_pair_table_catalog() -> TableCatalog {
    let mut catalog = valid_table_catalog();
    catalog.tables[0].columns[0].physical = CovePhysicalKind::VarBytes;
    catalog
}

fn valid_column_domain_payload() -> Vec<u8> {
    ColumnDomain::from_sorted_present_codes(&[1, 3], 4, 1, 1, CoveLogicalType::Utf8 as u16, 0, 0)
        .unwrap()
        .serialize()
        .unwrap()
}

fn invalid_column_domain_payload() -> Vec<u8> {
    ColumnDomain {
        header: ColumnDomainHeaderV1 {
            table_or_object_id: 1,
            column_or_property_id: 1,
            logical_type: CoveLogicalType::Utf8 as u16,
            collation_id: 0,
            domain_count: 2,
            sorted_file_codes_offset: COLUMN_DOMAIN_HEADER_LEN as u64,
            file_code_to_rank_offset: (COLUMN_DOMAIN_HEADER_LEN + 8) as u64,
            flags: 0,
            checksum: 0,
        },
        sorted_file_codes: vec![5, 5],
        file_code_to_rank: Vec::new(),
    }
    .serialize()
    .unwrap()
}

fn valid_table_segment_index() -> TableSegmentIndex {
    TableSegmentIndex {
        flags: 0,
        entries: vec![TableSegmentIndexEntryV1 {
            table_id: 1,
            segment_id: 0,
            row_start: 0,
            row_count: 10,
            morsel_count: 1,
            morsel_row_count: 4096,
            column_count: 1,
            offset: 512,
            length: 128,
            stats_ref: 0,
            flags: 0,
            checksum: 0,
        }],
    }
}

fn gap_table_segment_index() -> TableSegmentIndex {
    let mut index = valid_table_segment_index();
    index.entries[0].row_start = 5;
    index
}

fn valid_table_segment_header() -> TableSegmentHeaderV1 {
    TableSegmentHeaderV1 {
        table_id: 1,
        segment_id: 0,
        row_start: 0,
        row_count: 10,
        morsel_count: 1,
        morsel_row_count: 4096,
        column_count: 1,
        morsel_directory_offset: TABLE_SEGMENT_HEADER_LEN as u64,
        column_directory_offset: (TABLE_SEGMENT_HEADER_LEN + 24) as u64,
        page_index_offset: (TABLE_SEGMENT_HEADER_LEN + 24) as u64,
        data_offset: (TABLE_SEGMENT_HEADER_LEN + 24) as u64,
        flags: 0,
        checksum: 0,
    }
}

fn valid_row_morsel_directory() -> RowMorselDirectory {
    RowMorselDirectory {
        entries: vec![row_morsel(0, 0, 4096), row_morsel(1, 4096, 4)],
    }
}

fn gap_row_morsel_directory() -> RowMorselDirectory {
    RowMorselDirectory {
        entries: vec![row_morsel(0, 0, 10), row_morsel(1, 20, 5)],
    }
}

fn row_morsel(morsel_id: u32, first_row_in_segment: u32, row_count: u32) -> RowMorselEntryV1 {
    RowMorselEntryV1 {
        morsel_id,
        first_row_in_segment,
        row_count,
        flags: 0,
        stats_ref: 0,
        checksum: 0,
    }
}

fn valid_sort_key() -> SortKeyEntryV1 {
    SortKeyEntryV1 {
        column_id: 1,
        direction: SortDirection::Ascending,
        null_order: NullOrder::NullsLast,
        collation_id: 0,
    }
}

fn valid_clustering_key() -> ClusteringKeyEntryV1 {
    ClusteringKeyEntryV1 {
        column_id: 1,
        clustering_strength: ClusteringStrength::PERFECT,
        reserved: [0; 3],
    }
}

fn valid_covx_file() -> Vec<u8> {
    CovxFile {
        header: CovxHeaderV1::new([0x11; 16], 0, 1_700_000_000_000_000),
        referenced_files: vec![CovxReferencedFileV1 {
            file_id: [0x22; 16],
            file_len: 244,
            footer_crc32c: checksum::crc32c(b"footer"),
            digest_algorithm: 1,
            digest: vec![0x33; 32],
        }],
        postscript: CovxPostscriptV1 {
            header_offset: 0,
            header_len: 0,
            entries_offset: 0,
            entries_len: 0,
            file_len: 0,
            flags: 0,
            checksum: 0,
        },
    }
    .serialize()
    .unwrap()
}

fn valid_covm_file() -> Vec<u8> {
    CovmFile {
        header: CovmHeaderV1::new([0x55; 16], 1, 0, 1_700_000_000_000_000),
        files: vec![CovmFileEntryV1 {
            file_id: [0x66; 16],
            uri: "file:///dataset/part-0.cove".into(),
            file_len: 244,
            footer_crc32c: checksum::crc32c(b"footer"),
            digest_algorithm: 1,
            digest: vec![0x77; 32],
            row_count: 10,
            segment_count: 1,
            file_stats_ref: 0,
            file_exact_set_ref: 0,
            flags: 0,
        }],
        postscript: CovmPostscriptV1 {
            header_offset: 0,
            header_len: 0,
            entries_offset: 0,
            entries_len: 0,
            file_len: 0,
            flags: 0,
            checksum: 0,
        },
    }
    .serialize()
    .unwrap()
}

#[derive(Debug, Clone, Copy)]
enum SidecarFreshnessCase {
    Valid,
    FileId,
    FileLen,
    FooterCrc,
    Digest,
    Corrupt,
}

fn sidecar_freshness_payload(case: SidecarFreshnessCase) -> Vec<u8> {
    let cove = MinimalCoveWriter::write_empty_file().unwrap();
    let (mut file_id, mut file_len, mut footer_crc32c, mut digest) = cove_identity(&cove);
    if matches!(case, SidecarFreshnessCase::FileId) {
        file_id[0] ^= 0xFF;
    }
    if matches!(case, SidecarFreshnessCase::FileLen) {
        file_len += 1;
    }
    if matches!(case, SidecarFreshnessCase::FooterCrc) {
        footer_crc32c ^= 0xFFFF;
    }
    if matches!(case, SidecarFreshnessCase::Digest) {
        digest[0] ^= 0xFF;
    }

    let (covx, covm, expect) = if matches!(case, SidecarFreshnessCase::Corrupt) {
        (
            b"not a covx".to_vec(),
            b"not a covm".to_vec(),
            "StaleIgnored",
        )
    } else {
        (
            covx_for_reference(file_id, file_len, footer_crc32c, &digest),
            covm_for_reference(file_id, file_len, footer_crc32c, &digest),
            if matches!(case, SidecarFreshnessCase::Valid) {
                "Valid"
            } else {
                "StaleIgnored"
            },
        )
    };

    serde_json::to_vec_pretty(&json!({
        "cove": cove,
        "covx": covx,
        "covm": covm,
        "expect_covx": expect,
        "expect_covm": expect
    }))
    .unwrap()
}

fn cove_identity(cove: &[u8]) -> ([u8; 16], u64, u32, Vec<u8>) {
    let validated = reader::validate_bytes(cove).unwrap();
    let digest = compute_digest(DigestAlgorithm::Sha256, cove).unwrap();
    (
        validated.header.file_id,
        validated.postscript.file_len,
        validated.postscript.footer.crc32c,
        digest,
    )
}

fn covx_for_reference(
    file_id: [u8; 16],
    file_len: u64,
    footer_crc32c: u32,
    digest: &[u8],
) -> Vec<u8> {
    CovxFile {
        header: CovxHeaderV1::new([0x91; 16], 1, 1_700_000_000_000_000),
        referenced_files: vec![CovxReferencedFileV1 {
            file_id,
            file_len,
            footer_crc32c,
            digest_algorithm: DigestAlgorithm::Sha256 as u16,
            digest: digest.to_vec(),
        }],
        postscript: CovxPostscriptV1 {
            header_offset: 0,
            header_len: 0,
            entries_offset: 0,
            entries_len: 0,
            file_len: 0,
            flags: 0,
            checksum: 0,
        },
    }
    .serialize()
    .unwrap()
}

fn covm_for_reference(
    file_id: [u8; 16],
    file_len: u64,
    footer_crc32c: u32,
    digest: &[u8],
) -> Vec<u8> {
    CovmFile {
        header: CovmHeaderV1::new([0x92; 16], 1, 1, 1_700_000_000_000_000),
        files: vec![CovmFileEntryV1 {
            file_id,
            uri: "file:///dataset/part-0.cove".into(),
            file_len,
            footer_crc32c,
            digest_algorithm: DigestAlgorithm::Sha256 as u16,
            digest: digest.to_vec(),
            row_count: 0,
            segment_count: 0,
            file_stats_ref: 0,
            file_exact_set_ref: 0,
            flags: 0,
        }],
        postscript: CovmPostscriptV1 {
            header_offset: 0,
            header_len: 0,
            entries_offset: 0,
            entries_len: 0,
            file_len: 0,
            flags: 0,
            checksum: 0,
        },
    }
    .serialize()
    .unwrap()
}

fn valid_covemap_file() -> Vec<u8> {
    CovemapFile {
        header: CovemapHeaderV1::new([0x33; 16], 1_700_000_000_000_000),
        mapping_version: "example/v1".into(),
        sections: vec![
            CovemapSection {
                entry: CovemapSectionEntryV1 {
                    section_id: SectionKind::MapSourceCatalog as u32,
                    offset: 0,
                    length: 0,
                    uncompressed_length: 0,
                    compression: CompressionCodec::None as u8,
                    required: true,
                    reserved: 0,
                    checksum: 0,
                },
                payload: map_payload_bytes(json!({
                    "mapping_id": "m1",
                    "mapping_version": "example/v1",
                    "sources": [{
                        "source_id": "crm",
                        "schema_fingerprint": "schema:v1",
                        "snapshot_digest": "digest:v1",
                        "row_identity_rules": ["crm.pk"],
                        "replay_claimed": true
                    }]
                })),
            },
            CovemapSection {
                entry: CovemapSectionEntryV1 {
                    section_id: SectionKind::MapFunctionRegistry as u32,
                    offset: 0,
                    length: 0,
                    uncompressed_length: 0,
                    compression: CompressionCodec::None as u8,
                    required: false,
                    reserved: 0,
                    checksum: 0,
                },
                payload: map_payload_bytes(json!({
                    "mapping_id": "m1",
                    "mapping_version": "example/v1",
                    "functions": [{
                        "function_id": "trim_lower",
                        "version": "v1",
                        "deterministic": true,
                        "dependency": "pure"
                    }]
                })),
            },
        ],
        postscript: CovemapPostscriptV1 {
            required_features: FEATURE_SEMANTIC_MAP,
            optional_features: 0,
            file_len: 0,
            header_offset: 0,
            header_length: 0,
            checksum: 0,
        },
    }
    .serialize()
    .unwrap()
}

fn covemap_lz4_missing_feature_file() -> Vec<u8> {
    let mut file = CovemapFile::parse(&valid_covemap_file()).unwrap();
    file.sections[0].entry.compression = CompressionCodec::Lz4 as u8;
    let mut bytes = file.serialize().unwrap();
    rewrite_covemap_feature_bits(&mut bytes, FEATURE_SEMANTIC_MAP, 0);
    bytes
}

fn rewrite_covemap_feature_bits(bytes: &mut [u8], required_features: u64, optional_features: u64) {
    let mut header = CovemapHeaderV1::parse(bytes).unwrap();
    header.required_features = required_features;
    header.optional_features = optional_features;
    bytes[..COVEMAP_HEADER_LEN as usize].copy_from_slice(&header.serialize());

    let mut postscript = CovemapPostscriptV1::parse_from_tail(bytes).unwrap();
    postscript.required_features = required_features;
    postscript.optional_features = optional_features;
    let tail_start = bytes.len() - (COVEMAP_POSTSCRIPT_LEN as usize + COVEMAP_POSTSCRIPT_TAIL_SIZE);
    bytes[tail_start..].copy_from_slice(&postscript.serialize_tail());
}
