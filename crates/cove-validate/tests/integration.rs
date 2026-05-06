use cove_core::{
    checksum,
    constants::{
        PrimaryProfile, SectionKind, FEATURE_COLUMN_DOMAINS, FEATURE_ENGINE_PROFILE,
        FEATURE_FILE_DICTIONARY, FEATURE_HARBOR_PROFILE, FEATURE_OBJECT_PROFILE,
        FEATURE_REDACTIONS, FEATURE_TABLE_PROFILE, FEATURE_TRUST_CHAIN,
    },
    writer::{MinimalCoveWriter, ScanProfileCoveWriter, ScanSegment, SectionPayload},
};
use std::io::Write;

fn write_temp_file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "cove_validate_{name}_{}_{}.cove",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(bytes).unwrap();
    path
}

fn accept_fixture(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/accept")
        .join(name)
}

fn reject_fixture(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/reject")
        .join(name)
}

fn run_validate(path: &std::path::Path, semantic: bool) -> std::process::Output {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_cove-validate"));
    if semantic {
        cmd.arg("--semantic");
    }
    cmd.arg("--json").arg(path).output().unwrap()
}

fn run_validate_json_explain(path: &std::path::Path, semantic: bool) -> std::process::Output {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_cove-validate"));
    if semantic {
        cmd.arg("--semantic");
    }
    cmd.arg("--json")
        .arg("--explain")
        .arg(path)
        .output()
        .unwrap()
}

fn dictionary_index_bytes(redacted: bool) -> Vec<u8> {
    let header = cove_core::dictionary::FileDictionaryHeaderV1 {
        entry_count: 1,
        flags: 0,
        index_entry_len: cove_core::dictionary::FileDictionaryHeaderV1::INDEX_ENTRY_LEN,
        value_hash_algorithm: 0,
        payload_length: 0,
        reserved: [0; 24],
    };
    let mut inline_data = [0u8; 16];
    // Canonical UTF-8 encoding: varint length prefix + bytes ("a" => [0x01, 'a']).
    inline_data[0] = 0x01;
    inline_data[1] = b'a';
    let entry = cove_core::dictionary::FileDictionaryIndexEntryV1 {
        value_tag: cove_core::constants::ValueTag::Utf8 as u16,
        storage_class: if redacted {
            cove_core::constants::StorageClass::Redacted as u8
        } else {
            cove_core::constants::StorageClass::Inline as u8
        },
        flags: 0,
        inline_len: if redacted { 0 } else { 2 },
        reserved0: [0; 3],
        inline_data,
        payload_offset: 0,
        payload_length: 0,
        canonical_hash64: 0,
        reserved1: 0,
    };

    let mut bytes = header.serialize().to_vec();
    bytes.extend_from_slice(&entry.serialize());
    bytes
}

fn redaction_manifest_bytes(section_id: u32, file_codes: &[u64]) -> Vec<u8> {
    let mut bytes = (file_codes.len() as u32).to_le_bytes().to_vec();
    for (idx, file_code) in file_codes.iter().enumerate() {
        bytes.extend_from_slice(&(idx as u64 + 1).to_le_bytes());
        bytes.extend_from_slice(&section_id.to_le_bytes());
        bytes.extend_from_slice(&file_code.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&0i64.to_le_bytes());
    }
    bytes
}

fn zone_stats_entry_bytes(row_count: u32, null_count: u32, non_null_count: u32) -> Vec<u8> {
    let mut bytes = vec![0u8; cove_core::zone_stats::ZONE_STATS_ENTRY_LEN];
    bytes[0..4].copy_from_slice(&1u32.to_le_bytes());
    bytes[4..8].copy_from_slice(&2u32.to_le_bytes());
    bytes[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    bytes[12..16].copy_from_slice(&3u32.to_le_bytes());
    bytes[16..20].copy_from_slice(&row_count.to_le_bytes());
    bytes[20..24].copy_from_slice(&null_count.to_le_bytes());
    bytes[24..28].copy_from_slice(&non_null_count.to_le_bytes());
    bytes
}

fn segment_payload_bytes(row_count: u32, morsel_row_count: u32) -> Vec<u8> {
    let header = cove_core::segment::TableSegmentHeaderV1 {
        table_id: 1,
        segment_id: 0,
        row_start: 0,
        row_count,
        morsel_count: 1,
        morsel_row_count,
        column_count: 0,
        morsel_directory_offset: cove_core::segment::TABLE_SEGMENT_HEADER_LEN as u64,
        column_directory_offset: (cove_core::segment::TABLE_SEGMENT_HEADER_LEN
            + cove_core::segment::ROW_MORSEL_ENTRY_LEN) as u64,
        page_index_offset: (cove_core::segment::TABLE_SEGMENT_HEADER_LEN
            + cove_core::segment::ROW_MORSEL_ENTRY_LEN) as u64,
        data_offset: (cove_core::segment::TABLE_SEGMENT_HEADER_LEN
            + cove_core::segment::ROW_MORSEL_ENTRY_LEN) as u64,
        flags: 0,
        checksum: 0,
    };
    let morsel = cove_core::segment::RowMorselEntryV1 {
        morsel_id: 0,
        first_row_in_segment: 0,
        row_count: morsel_row_count,
        flags: 0,
        stats_ref: 0,
        checksum: 0,
    };

    let mut bytes = header.serialize().to_vec();
    bytes.extend_from_slice(&morsel.serialize());
    bytes
}

fn temporal_row(
    timestamp_us: i64,
    csn: u64,
    prev_ref: Option<cove_core::profile::cove_o::CoveRecordRefV1>,
) -> cove_core::profile::cove_o::TemporalRowEntryV1 {
    cove_core::profile::cove_o::TemporalRowEntryV1 {
        timestamp_us,
        csn,
        branch_key: 0,
        goid: [0; 16],
        record_id: [0; 16],
        record_kind: cove_core::profile::cove_o::RecordKind::Delta,
        prev_ref,
    }
}

fn temporal_segment_data_bytes(
    segment_id: u32,
    rows: &[cove_core::profile::cove_o::TemporalRowEntryV1],
) -> Vec<u8> {
    let row_directory_offset = cove_core::profile::cove_o::TEMPORAL_SEGMENT_HEADER_LEN as u64;
    let row_bytes = (rows.len() * cove_core::profile::cove_o::TEMPORAL_ROW_ENTRY_LEN) as u64;
    let row_end = row_directory_offset + row_bytes;
    let header = cove_core::profile::cove_o::TemporalSegmentHeaderV1 {
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
    };
    let mut bytes = header.serialize().to_vec();
    for row in rows {
        bytes.extend_from_slice(&row.serialize());
    }
    bytes
}

fn temporal_segment_section(
    segment_id: u32,
    rows: &[cove_core::profile::cove_o::TemporalRowEntryV1],
) -> SectionPayload {
    SectionPayload {
        section_kind: SectionKind::TemporalSegmentData as u16,
        profile: 1,
        flags: 0,
        item_count: 1,
        row_count: rows.len() as u64,
        compression: 0,
        alignment_log2: 0,
        required_features: FEATURE_OBJECT_PROFILE,
        optional_features: 0,
        data: temporal_segment_data_bytes(segment_id, rows),
    }
}

fn object_catalog_section() -> SectionPayload {
    let catalog = cove_core::profile::cove_o::ObjectTypeCatalog {
        flags: 0,
        types: vec![cove_core::profile::cove_o::ObjectTypeEntryV1 {
            object_type_id: 1,
            type_name: "TestObject".into(),
            flags: cove_core::profile::cove_o::OBJECT_TYPE_FLAG_ENTITY_OBJECT,
            properties: Vec::new(),
        }],
    };
    SectionPayload {
        section_kind: SectionKind::ObjectTypeCatalog as u16,
        profile: 1,
        flags: 0,
        item_count: 1,
        row_count: 0,
        compression: 0,
        alignment_log2: 0,
        required_features: FEATURE_OBJECT_PROFILE,
        optional_features: 0,
        data: catalog.serialize().unwrap(),
    }
}

fn temporal_segment_index_section(
    segments: &[(u32, &[cove_core::profile::cove_o::TemporalRowEntryV1])],
) -> SectionPayload {
    let entries = segments
        .iter()
        .map(|(segment_id, rows)| {
            let row_count = rows.len() as u32;
            cove_core::profile::cove_o::TemporalSegmentIndexEntryV1 {
                segment_id: *segment_id,
                object_type_id: 1,
                time_range_start_us: rows.first().map(|row| row.timestamp_us).unwrap_or(0),
                time_range_end_us: rows.last().map(|row| row.timestamp_us).unwrap_or(0),
                csn_min: rows.first().map(|row| row.csn).unwrap_or(0),
                csn_max: rows.last().map(|row| row.csn).unwrap_or(0),
                row_count,
                delta_count: row_count,
                snapshot_count: 0,
                baseline_count: 0,
                tombstone_count: 0,
                min_goid: rows.iter().map(|row| row.goid).min().unwrap_or([0; 16]),
                max_goid: rows.iter().map(|row| row.goid).max().unwrap_or([0; 16]),
                offset: 0,
                length: temporal_segment_data_bytes(*segment_id, rows).len() as u64,
                checksum: 0,
            }
        })
        .collect::<Vec<_>>();
    let index = cove_core::profile::cove_o::TemporalSegmentIndex { flags: 0, entries };
    SectionPayload {
        section_kind: SectionKind::TemporalSegmentIndex as u16,
        profile: 1,
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

fn trust_manifest_bytes(
    segment_id: u32,
    rows: &[cove_core::profile::cove_o::TemporalRowEntryV1],
) -> Vec<u8> {
    let mut bytes = (rows.len() as u32).to_le_bytes().to_vec();
    let mut prev = [0u8; 32];
    for (row_index, row) in rows.iter().enumerate() {
        bytes.extend_from_slice(&segment_id.to_le_bytes());
        bytes.extend_from_slice(&(row_index as u32).to_le_bytes());
        prev = cove_core::trust_chain::chain(&prev, &row.trust_payload()).unwrap();
        bytes.extend_from_slice(&prev);
    }
    bytes
}

#[test]
fn validate_empty_file() {
    let bytes = MinimalCoveWriter::write_empty_file();
    let path = write_temp_file("empty", &bytes);

    // Run cove-validate on the file.
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_cove-validate"))
        .arg(&path)
        .status()
        .expect("cove-validate binary should be runnable");

    assert!(
        status.success(),
        "cove-validate should return exit code 0 for a valid file"
    );
    // Cleanup is best-effort; if removal fails the test OS will clean up temp files.
    let _ = std::fs::remove_file(&path);
}

#[test]
fn validate_corrupted_file() {
    let mut bytes = MinimalCoveWriter::write_empty_file();
    // Corrupt the trailing magic.
    let len = bytes.len();
    bytes[len - 1] = 0xFF;

    let path = write_temp_file("corrupt", &bytes);

    let status = std::process::Command::new(env!("CARGO_BIN_EXE_cove-validate"))
        .arg(&path)
        .status()
        .expect("cove-validate binary should be runnable");

    assert!(
        !status.success(),
        "cove-validate should return non-zero for a corrupt file"
    );
    // Cleanup is best-effort; if removal fails the test OS will clean up temp files.
    let _ = std::fs::remove_file(&path);
}

#[test]
fn validate_accepts_standalone_covemap_json() {
    let path = accept_fixture("covemap_valid.covemap");
    let output = run_validate_json_explain(&path, true);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains("\"artifact\":\"covemap\""), "{stdout}");
    assert!(
        stdout.contains("\"mapping_version\":\"example/v1\""),
        "{stdout}"
    );
    assert!(stdout.contains("\"section_count\":2"), "{stdout}");
}

#[test]
fn validate_rejects_corrupt_standalone_covemap_json() {
    let path = reject_fixture("covemap_header_crc_flipped.covemap");
    let output = run_validate(&path, false);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(stdout.contains("COVE_E_CHECKSUM_MISMATCH"), "{stdout}");
}

#[test]
fn json_cli_surfaces_stable_error_code() {
    let mut bytes = MinimalCoveWriter::write_empty_file();
    let len = bytes.len();
    bytes.truncate(len - 4);

    let path = write_temp_file("truncated_magic", &bytes);
    let output = run_validate(&path, false);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(!output.status.success());
    assert!(stdout.contains("\"error_code\":\"COVE_E_BAD_MAGIC\""));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn semantic_cli_rejects_cove_t_bad_column_domain() {
    let mut writer = MinimalCoveWriter::new();
    writer.required_features = FEATURE_TABLE_PROFILE | FEATURE_COLUMN_DOMAINS;
    let header = cove_core::domain::ColumnDomainHeaderV1 {
        table_or_object_id: 1,
        column_or_property_id: 2,
        logical_type: 0,
        collation_id: 0,
        domain_count: 2,
        sorted_file_codes_offset: cove_core::domain::COLUMN_DOMAIN_HEADER_LEN as u64,
        file_code_to_rank_offset: (cove_core::domain::COLUMN_DOMAIN_HEADER_LEN + 8) as u64,
        flags: 0,
        checksum: 0,
    };
    let mut data = header.serialize().to_vec();
    data.extend_from_slice(&5u32.to_le_bytes());
    data.extend_from_slice(&5u32.to_le_bytes());
    writer.sections.push(SectionPayload {
        section_kind: SectionKind::ColumnDomain as u16,
        profile: 2,
        flags: 0,
        item_count: 2,
        row_count: 0,
        compression: 0,
        alignment_log2: 0,
        required_features: FEATURE_COLUMN_DOMAINS,
        optional_features: 0,
        data,
    });
    let path = write_temp_file("cove_t_bad_domain", &writer.write());
    let output = run_validate(&path, true);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success());
    assert!(stdout.contains("COVE_E_BAD_DOMAIN"));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn semantic_cli_accepts_scan_profile_writer_file() {
    let catalog = cove_core::table::TableCatalog {
        flags: 0,
        tables: vec![cove_core::table::TableEntry {
            table_id: 1,
            namespace: "public".into(),
            name: "events".into(),
            row_count: 10,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns: vec![],
        }],
    };
    let mut writer = ScanProfileCoveWriter::new(catalog);
    writer.push_segment(ScanSegment::new(1, 0, 0, 10, 0));
    let path = write_temp_file("scan_profile_writer", &writer.write().unwrap());
    let output = run_validate(&path, true);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains("\"ok\":true"));
    let _ = std::fs::remove_file(&path);
}

fn invalid_execution_descriptor_payload() -> Vec<u8> {
    let mut bytes = cove_core::profile::cove_e::ExecutionCodeDescriptorV1 {
        descriptor_id: 1,
        code_kind: cove_core::profile::cove_e::ExecutionCodeKind::DictionaryKey,
        code_width_bits: 32,
        byte_order: 0,
        lifetime: cove_core::profile::cove_e::ExecutionCodeLifetime::Scan,
        comparison_scope: cove_core::profile::cove_e::ExecutionCodeComparisonScope::File,
        canonicality: cove_core::profile::cove_e::ExecutionCodeCanonicality::Transient,
        null_code_policy: cove_core::profile::cove_e::NullCodePolicy::NullBitmapOnly,
        flags: 0,
        scope_ref: 0,
        code_space_ref: 0,
        checksum: 0,
    }
    .serialize()
    .to_vec();
    bytes[4] = 42;
    bytes[24..28].fill(0);
    let crc = checksum::crc32c(&bytes);
    bytes[24..28].copy_from_slice(&crc.to_le_bytes());
    bytes
}

#[test]
fn semantic_cli_rejects_required_cove_e_profile_error() {
    let mut writer = MinimalCoveWriter::new();
    writer.primary_profile = PrimaryProfile::Mixed as u8;
    writer.required_features = FEATURE_ENGINE_PROFILE;
    writer.sections.push(SectionPayload {
        section_kind: SectionKind::ExecutionCodeDescriptor as u16,
        profile: 4,
        flags: 0,
        item_count: 1,
        row_count: 0,
        compression: 0,
        alignment_log2: 0,
        required_features: FEATURE_ENGINE_PROFILE,
        optional_features: 0,
        data: invalid_execution_descriptor_payload(),
    });
    let path = write_temp_file("cove_e_required_bad", &writer.write());
    let output = run_validate(&path, true);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success());
    assert!(stdout.contains("COVE_E_BAD_ENGINE_PROFILE"));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn semantic_cli_ignores_optional_cove_e_profile_error() {
    let mut writer = MinimalCoveWriter::new();
    writer.primary_profile = PrimaryProfile::Mixed as u8;
    writer.required_features = 0;
    writer.optional_features = FEATURE_ENGINE_PROFILE;
    writer.sections.push(SectionPayload {
        section_kind: SectionKind::ExecutionCodeDescriptor as u16,
        profile: 4,
        flags: 0,
        item_count: 1,
        row_count: 0,
        compression: 0,
        alignment_log2: 0,
        required_features: 0,
        optional_features: FEATURE_ENGINE_PROFILE,
        data: invalid_execution_descriptor_payload(),
    });
    let path = write_temp_file("cove_e_optional_bad", &writer.write());
    let output = run_validate(&path, true);
    assert!(output.status.success());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn semantic_cli_rejects_required_cove_o_schema_error() {
    let mut writer = MinimalCoveWriter::new();
    writer.primary_profile = PrimaryProfile::ObjectTemporal as u8;
    writer.required_features = FEATURE_OBJECT_PROFILE;
    let catalog = cove_core::profile::cove_o::ObjectTypeCatalog {
        flags: 0,
        types: vec![cove_core::profile::cove_o::ObjectTypeEntryV1 {
            object_type_id: 1,
            type_name: "Thing".into(),
            flags: cove_core::profile::cove_o::OBJECT_TYPE_FLAG_ENTITY_OBJECT,
            properties: vec![cove_core::profile::cove_o::PropertyEntryV1 {
                property_id: 1,
                property_name: "bad".into(),
                logical_type: cove_core::constants::CoveLogicalType::Null,
                physical_kind: cove_core::constants::CovePhysicalKind::FileCode,
                nullable: false,
                collation_id: 0,
                flags: 0,
            }],
        }],
    };
    writer.sections.push(SectionPayload {
        section_kind: SectionKind::ObjectTypeCatalog as u16,
        profile: 1,
        flags: 0,
        item_count: 1,
        row_count: 0,
        compression: 0,
        alignment_log2: 0,
        required_features: FEATURE_OBJECT_PROFILE,
        optional_features: 0,
        data: catalog.serialize().unwrap(),
    });
    let path = write_temp_file("cove_o_required_bad", &writer.write());
    let output = run_validate(&path, true);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success());
    assert!(stdout.contains("COVE_E_BAD_SCHEMA"));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn semantic_cli_rejects_required_cove_o_temporal_order_error() {
    let mut writer = MinimalCoveWriter::new();
    writer.primary_profile = PrimaryProfile::ObjectTemporal as u8;
    writer.required_features = FEATURE_OBJECT_PROFILE;
    writer.sections.push(SectionPayload {
        section_kind: SectionKind::TemporalSegmentData as u16,
        profile: 1,
        flags: 0,
        item_count: 1,
        row_count: 2,
        compression: 0,
        alignment_log2: 0,
        required_features: FEATURE_OBJECT_PROFILE,
        optional_features: 0,
        data: temporal_segment_data_bytes(
            3,
            &[temporal_row(20, 2, None), temporal_row(10, 1, None)],
        ),
    });
    let path = write_temp_file("cove_o_bad_temporal_order", &writer.write());
    let output = run_validate(&path, true);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success());
    assert!(stdout.contains("COVE_E_BAD_SCHEMA"), "{stdout}");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn semantic_cli_rejects_required_cove_o_bad_trust_manifest() {
    let mut writer = MinimalCoveWriter::new();
    writer.primary_profile = PrimaryProfile::ObjectTemporal as u8;
    writer.required_features = FEATURE_OBJECT_PROFILE | FEATURE_TRUST_CHAIN;
    let rows = vec![temporal_row(10, 1, None), temporal_row(20, 2, None)];
    let mut manifest = trust_manifest_bytes(5, &rows);
    *manifest.last_mut().unwrap() ^= 0xFF;
    writer.sections.push(object_catalog_section());
    writer
        .sections
        .push(temporal_segment_index_section(&[(5, &rows)]));
    writer.sections.push(SectionPayload {
        section_kind: SectionKind::TemporalSegmentData as u16,
        profile: 1,
        flags: 0,
        item_count: 1,
        row_count: rows.len() as u64,
        compression: 0,
        alignment_log2: 0,
        required_features: FEATURE_OBJECT_PROFILE,
        optional_features: 0,
        data: temporal_segment_data_bytes(5, &rows),
    });
    writer.sections.push(SectionPayload {
        section_kind: SectionKind::TrustManifest as u16,
        profile: 1,
        flags: 0,
        item_count: rows.len() as u64,
        row_count: rows.len() as u64,
        compression: 0,
        alignment_log2: 0,
        required_features: FEATURE_TRUST_CHAIN,
        optional_features: 0,
        data: manifest,
    });
    let path = write_temp_file("cove_o_bad_trust_manifest", &writer.write());
    let output = run_validate(&path, true);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success());
    assert!(stdout.contains("COVE_E_DIGEST_MISMATCH"), "{stdout}");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn semantic_cli_accepts_cross_segment_temporal_prev_ref() {
    let mut writer = MinimalCoveWriter::new();
    writer.primary_profile = PrimaryProfile::ObjectTemporal as u8;
    writer.required_features = FEATURE_OBJECT_PROFILE;
    let earlier_rows = vec![temporal_row(10, 1, None)];
    let later_rows = vec![temporal_row(
        20,
        2,
        Some(cove_core::profile::cove_o::CoveRecordRefV1 {
            segment_id: 4,
            row_index: 0,
            target_kind: 0,
        }),
    )];
    writer.sections.push(object_catalog_section());
    writer.sections.push(temporal_segment_index_section(&[
        (4, &earlier_rows),
        (5, &later_rows),
    ]));
    writer
        .sections
        .push(temporal_segment_section(4, &earlier_rows));
    writer
        .sections
        .push(temporal_segment_section(5, &later_rows));

    let path = write_temp_file("cove_o_cross_segment_prev_ref", &writer.write());
    let output = run_validate(&path, true);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn semantic_cli_ignores_optional_harbor_hint_error() {
    let mut writer = MinimalCoveWriter::new();
    writer.primary_profile = PrimaryProfile::Mixed as u8;
    writer.required_features = 0;
    writer.optional_features = FEATURE_HARBOR_PROFILE;
    let mut data = cove_core::profile::cove_h::HarborMountHintsV1 {
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
    .serialize()
    .to_vec();
    data[29] = 1;
    writer.sections.push(SectionPayload {
        section_kind: SectionKind::HarborMountHints as u16,
        profile: 5,
        flags: 0,
        item_count: 1,
        row_count: 0,
        compression: 0,
        alignment_log2: 0,
        required_features: 0,
        optional_features: FEATURE_HARBOR_PROFILE,
        data,
    });
    let path = write_temp_file("cove_h_optional_bad", &writer.write());
    let output = run_validate(&path, true);
    assert!(output.status.success());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn semantic_cli_rejects_redacted_dictionary_without_manifest() {
    let mut writer = MinimalCoveWriter::new();
    writer.primary_profile = PrimaryProfile::Mixed as u8;
    writer.required_features = FEATURE_FILE_DICTIONARY | FEATURE_REDACTIONS;
    writer.sections.push(SectionPayload {
        section_kind: SectionKind::FileDictionaryIndex as u16,
        profile: PrimaryProfile::Mixed as u8,
        flags: 0,
        item_count: 1,
        row_count: 0,
        compression: 0,
        alignment_log2: 0,
        required_features: FEATURE_FILE_DICTIONARY,
        optional_features: 0,
        data: dictionary_index_bytes(true),
    });

    let path = write_temp_file("redacted_dict_missing_manifest", &writer.write());
    let output = run_validate(&path, true);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success());
    assert!(stdout.contains("COVE_E_BAD_SCHEMA"));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn semantic_cli_rejects_redaction_manifest_for_non_redacted_filecode() {
    let mut writer = MinimalCoveWriter::new();
    writer.primary_profile = PrimaryProfile::Mixed as u8;
    writer.required_features = FEATURE_FILE_DICTIONARY | FEATURE_REDACTIONS;
    writer.sections.push(SectionPayload {
        section_kind: SectionKind::FileDictionaryIndex as u16,
        profile: PrimaryProfile::Mixed as u8,
        flags: 0,
        item_count: 1,
        row_count: 0,
        compression: 0,
        alignment_log2: 0,
        required_features: FEATURE_FILE_DICTIONARY,
        optional_features: 0,
        data: dictionary_index_bytes(false),
    });
    writer.sections.push(SectionPayload {
        section_kind: SectionKind::RedactionManifest as u16,
        profile: PrimaryProfile::Mixed as u8,
        flags: 0,
        item_count: 1,
        row_count: 0,
        compression: 0,
        alignment_log2: 0,
        required_features: FEATURE_REDACTIONS,
        optional_features: 0,
        data: redaction_manifest_bytes(1, &[0]),
    });

    let path = write_temp_file("manifest_non_redacted_filecode", &writer.write());
    let output = run_validate(&path, true);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success());
    assert!(stdout.contains("COVE_E_BAD_SCHEMA"));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn semantic_cli_accepts_redacted_dictionary_with_manifest() {
    let mut writer = MinimalCoveWriter::new();
    writer.primary_profile = PrimaryProfile::Mixed as u8;
    writer.required_features = FEATURE_FILE_DICTIONARY | FEATURE_REDACTIONS;
    writer.sections.push(SectionPayload {
        section_kind: SectionKind::FileDictionaryIndex as u16,
        profile: PrimaryProfile::Mixed as u8,
        flags: 0,
        item_count: 1,
        row_count: 0,
        compression: 0,
        alignment_log2: 0,
        required_features: FEATURE_FILE_DICTIONARY,
        optional_features: 0,
        data: dictionary_index_bytes(true),
    });
    writer.sections.push(SectionPayload {
        section_kind: SectionKind::RedactionManifest as u16,
        profile: PrimaryProfile::Mixed as u8,
        flags: 0,
        item_count: 1,
        row_count: 0,
        compression: 0,
        alignment_log2: 0,
        required_features: FEATURE_REDACTIONS,
        optional_features: 0,
        data: redaction_manifest_bytes(1, &[0]),
    });

    let path = write_temp_file("redacted_dict_with_manifest", &writer.write());
    let output = run_validate(&path, true);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains("\"ok\":true"));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn semantic_cli_rejects_bad_zone_stats_section() {
    let mut writer = MinimalCoveWriter::new();
    writer.primary_profile = PrimaryProfile::TableScan as u8;
    writer.required_features = FEATURE_TABLE_PROFILE;
    writer.sections.push(SectionPayload {
        section_kind: SectionKind::ZoneStats as u16,
        profile: PrimaryProfile::TableScan as u8,
        flags: 0,
        item_count: 1,
        row_count: 1,
        compression: 0,
        alignment_log2: 0,
        required_features: 0,
        optional_features: 0,
        data: zone_stats_entry_bytes(1, 2, 0),
    });

    let path = write_temp_file("bad_zone_stats", &writer.write());
    let output = run_validate(&path, true);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success());
    assert!(stdout.contains("COVE_E_BAD_STATS"));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn semantic_cli_rejects_bad_table_segment_payload() {
    let mut writer = MinimalCoveWriter::new();
    writer.primary_profile = PrimaryProfile::TableScan as u8;
    writer.required_features = FEATURE_TABLE_PROFILE;
    writer.sections.push(SectionPayload {
        section_kind: SectionKind::TableSegmentData as u16,
        profile: PrimaryProfile::TableScan as u8,
        flags: 0,
        item_count: 1,
        row_count: 10,
        compression: 0,
        alignment_log2: 0,
        required_features: 0,
        optional_features: 0,
        data: segment_payload_bytes(10, 9),
    });

    let path = write_temp_file("bad_table_segment_payload", &writer.write());
    let output = run_validate(&path, true);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success());
    assert!(stdout.contains("COVE_E_SEGMENT_CORRUPT"));
    let _ = std::fs::remove_file(&path);
}
