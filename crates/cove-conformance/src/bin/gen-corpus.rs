//! Generates the conformance corpus referenced by `conformance/manifest.jsonl`.
//! Run with `cargo run -p cove-conformance --bin gen-corpus`.
//!
//! Each fixture maps to one or more Spec §75 error codes; the manifest is
//! written alongside the binaries so the generator stays the source of truth.

use std::{fs, path::PathBuf};

use cove_core::{
    artifact::{
        covm::{CovmFile, CovmFileEntryV1, CovmHeaderV1, CovmPostscriptV1},
        covx::{CovxFile, CovxHeaderV1, CovxPostscriptV1, CovxReferencedFileV1},
    },
    checksum,
    constants::{
        CoveEncodingKind, CoveLogicalType, CovePhysicalKind, DigestAlgorithm, PrimaryProfile,
        SectionKind, FEATURE_COLUMN_DOMAINS, FEATURE_ENGINE_PROFILE, FEATURE_HARBOR_PROFILE,
        FEATURE_OBJECT_PROFILE, FEATURE_TABLE_PROFILE,
    },
    digest::compute_digest,
    domain::{ColumnDomain, ColumnDomainHeaderV1, COLUMN_DOMAIN_HEADER_LEN},
    header::HEADER_SIZE,
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
    io_hints::defaults_object_store,
    postscript::{CovePostscriptV1, POSTSCRIPT_SIZE, POSTSCRIPT_TOTAL_SIZE},
    profile::{
        cove_e::{
            EngineMountPolicyV1, EngineProfileEntryV1, EngineProfileRegistry,
            ExecutionCodeCanonicality, ExecutionCodeComparisonScope, ExecutionCodeDescriptorV1,
            ExecutionCodeKind, ExecutionCodeLifetime, FileCodeMappingKind, MissingValuePolicy,
            NullCodePolicy, ReverseLookupPolicy, StaleMappingPolicy,
        },
        cove_h::HarborMountHintsV1,
        cove_o::{
            ObjectTypeCatalog, ObjectTypeEntryV1, PropertyEntryV1, TemporalSegmentIndex,
            TemporalSegmentIndexEntryV1,
        },
    },
    row_ref::RowRef,
    segment::{
        RowMorselDirectory, RowMorselEntryV1, TableSegmentHeaderV1, TableSegmentIndex,
        TableSegmentIndexEntryV1, TABLE_SEGMENT_HEADER_LEN,
    },
    sort::{ClusteringKeyEntryV1, ClusteringStrength, NullOrder, SortDirection, SortKeyEntryV1},
    table::{ColumnEntry, TableCatalog, TableEntry},
    writer::{MinimalCoveWriter, ScanProfileCoveWriter, ScanSegment, SectionPayload},
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
    let bytes = MinimalCoveWriter::write_empty_file();
    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/min_empty.cove",
            "cove",
            "accept",
            None,
            &["§9", "§10", "§13", "§71.1"],
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
            &["§24", "§25", "§26", "§71.2", "§71.3", "§72"],
        ),
        cove_t_scan_table_file(),
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
            "accept/cove_o_object_catalog_valid.bin",
            "cove_o_object_catalog",
            "accept",
            None,
            &["§56"],
        ),
        valid_object_catalog().serialize().unwrap(),
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

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "accept/cove_unknown_optional_feature.cove",
            "cove",
            "accept",
            None,
            &["§76"],
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
            &["§40", "§76"],
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
            "accept/cove_h_optional_bad_hints.cove",
            "cove",
            "accept",
            None,
            &["§44", "§76"],
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
            "accept/cove_o_optional_bad_catalog.cove",
            "cove",
            "accept",
            None,
            &["§56", "§76"],
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
            &["§12", "§75"],
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
            &["§12", "§75"],
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
            &["§9", "§75"],
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
            &["§13", "§75"],
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
            &["§12", "§75"],
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
            &["§23", "§72", "§75"],
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
            "reject/cove_t_duplicate_table_id.cove",
            "cove",
            "reject",
            Some("COVE_E_BAD_SCHEMA"),
            &["§24", "§72", "§75"],
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
            "reject/cove_t_bad_segment_gap.cove",
            "cove",
            "reject",
            Some("COVE_E_SEGMENT_CORRUPT"),
            &["§25", "§72", "§75"],
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
            "reject/column_domain_duplicate.bin",
            "column_domain",
            "reject",
            Some("COVE_E_BAD_DOMAIN"),
            &["§23", "§75"],
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
            &["§24", "§75"],
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
            &["§25", "§75"],
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
            &["§25", "§75"],
        ),
        bad_segment_header,
    );

    let row_morsel_gap = fixture(
        "reject/row_morsel_directory_gap.bin",
        "row_morsel_directory",
        "reject",
        Some("COVE_E_SEGMENT_CORRUPT"),
        &["§26", "§75"],
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
            &["§53", "§75"],
        ),
        bad_sort_key,
    );

    let mut covx_bad = covx_bytes;
    covx_bad[82] ^= 0xFF;
    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/covx_header_crc_flipped.covx",
            "covx",
            "reject",
            Some("COVE_E_CHECKSUM_MISMATCH"),
            &["§68", "§75"],
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
            &["§69", "§75"],
        ),
        covm_bad,
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/metadata_json_invalid.json",
            "metadata_json",
            "reject",
            Some("COVE_E_BAD_SECTION"),
            &["§15", "§75"],
        ),
        b"{not-json".to_vec(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/collation_registry_bad_utf8.bin",
            "collation_registry",
            "reject",
            Some("COVE_E_BAD_SECTION"),
            &["§22", "§75"],
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
            &["§27", "§75"],
        ),
        page_index_payload(4, 5, CoveEncodingKind::PlainFixed as u16),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/digest_manifest_wrong_len.bin",
            "digest_manifest",
            "reject",
            Some("COVE_E_BAD_SECTION"),
            &["§65", "§75"],
        ),
        digest_manifest_wrong_len_payload(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/redaction_manifest_truncated.bin",
            "redaction_manifest",
            "reject",
            Some("COVE_E_OFFSET_RANGE"),
            &["§64", "§75"],
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
            &["§67", "§75"],
        ),
        vec![0; 8],
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/lakehouse_hints_bad_utf8.bin",
            "lakehouse_hints",
            "reject",
            Some("COVE_E_BAD_SECTION"),
            &["§50", "§75"],
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
            &["§21", "§75"],
        ),
        kernel_capabilities_payload(0xfffe),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/exact_set_index_unsorted.bin",
            "exact_set_index",
            "reject",
            Some("COVE_E_BAD_INDEX"),
            &["§30", "§75"],
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
            &["§31", "§75"],
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
            &["§32", "§75"],
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
            &["§33", "§75"],
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
            &["§34", "§75"],
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
            &["§35", "§75"],
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
            &["§36", "§75"],
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
            &["§39", "§75"],
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
            &["§40", "§75"],
        ),
        invalid_execution_descriptor_payload(),
    );

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_e_mount_policy_bad_mapping.bin",
            "cove_e_mount_policy",
            "reject",
            Some("COVE_E_BAD_ENGINE_PROFILE"),
            &["§43", "§75"],
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
            &["§44", "§75"],
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
            &["§56", "§75"],
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
            &["§57", "§75"],
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
            &["§76", "§75"],
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
            &["§40", "§76", "§75"],
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

    write_fixture(
        &root,
        &mut entries,
        fixture(
            "reject/cove_h_required_bad_hints.cove",
            "cove",
            "reject",
            Some("COVE_E_BAD_SECTION"),
            &["§44", "§76", "§75"],
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
            &["§56", "§76", "§75"],
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

fn profile_cove_file(
    required_features: u64,
    optional_features: u64,
    section_kind: SectionKind,
    profile: PrimaryProfile,
    section_required_features: u64,
    section_optional_features: u64,
    data: Vec<u8>,
) -> Vec<u8> {
    let mut writer = MinimalCoveWriter::new();
    writer.primary_profile = PrimaryProfile::Mixed as u8;
    writer.required_features = required_features;
    writer.optional_features = optional_features;
    writer.sections.push(SectionPayload {
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
    });
    writer.write()
}

fn cove_with_unknown_optional_feature() -> Vec<u8> {
    let mut writer = MinimalCoveWriter::new();
    writer.optional_features = 1u64 << 63;
    writer.write()
}

fn cove_with_unknown_required_feature() -> Vec<u8> {
    let writer = MinimalCoveWriter::new();
    let mut bytes = writer.write();
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

fn digest_manifest_payload(
    section_id: u32,
    algorithm: DigestAlgorithm,
    payload: &[u8],
) -> Result<Vec<u8>, cove_core::CoveError> {
    let digest = compute_digest(algorithm, payload)?;
    let mut out = 1u32.to_le_bytes().to_vec();
    out.extend_from_slice(&section_id.to_le_bytes());
    out.extend_from_slice(&(algorithm as u16).to_le_bytes());
    out.extend_from_slice(&(digest.len() as u16).to_le_bytes());
    out.extend_from_slice(&digest);
    Ok(out)
}

fn digest_manifest_wrong_len_payload() -> Vec<u8> {
    let mut out = 1u32.to_le_bytes().to_vec();
    out.extend_from_slice(&7u32.to_le_bytes());
    out.extend_from_slice(&(DigestAlgorithm::Sha256 as u16).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&[0u8; 4]);
    out
}

fn redaction_manifest_payload() -> Vec<u8> {
    let mut out = 1u32.to_le_bytes().to_vec();
    out.extend_from_slice(&7u64.to_le_bytes());
    out.extend_from_slice(&1_700_000_000_000_000i64.to_le_bytes());
    out.extend_from_slice(&12u16.to_le_bytes());
    out.extend_from_slice(b"GDPR-erasure");
    out.extend_from_slice(&9u16.to_le_bytes());
    out.extend_from_slice(b"ticket-42");
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

fn lakehouse_hints_bad_utf8_payload() -> Vec<u8> {
    let mut out = vec![0u8; 32];
    out.extend_from_slice(&0u32.to_le_bytes());
    out.push(0);
    write_len_prefixed(&mut out, &[0xff]);
    write_len_prefixed(&mut out, b"");
    out.extend_from_slice(&[0u8; 32]);
    out
}

fn write_len_prefixed(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
    out.extend_from_slice(bytes);
}

fn kernel_capabilities_payload(encoding: u16) -> Vec<u8> {
    let mut out = 1u32.to_le_bytes().to_vec();
    out.extend_from_slice(&encoding.to_le_bytes());
    out.extend_from_slice(&3u32.to_le_bytes());
    out
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

fn valid_temporal_segment_index() -> TemporalSegmentIndex {
    TemporalSegmentIndex {
        flags: 0,
        entries: vec![temporal_segment_entry(1, 2, 2, 0, 0, 0)],
    }
}

fn invalid_temporal_segment_index() -> TemporalSegmentIndex {
    TemporalSegmentIndex {
        flags: 0,
        entries: vec![temporal_segment_entry(1, 2, 2, 0, 0, 1)],
    }
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
