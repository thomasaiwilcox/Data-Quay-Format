use qf_core::{
    checksum,
    constants::{
        PrimaryProfile, SectionKind, FEATURE_COLUMN_DOMAINS, FEATURE_ENGINE_PROFILE,
        FEATURE_HARBOR_PROFILE, FEATURE_OBJECT_PROFILE, FEATURE_TABLE_PROFILE,
    },
    writer::{MinimalQfWriter, ScanProfileQfWriter, ScanSegment, SectionPayload},
};
use std::io::Write;

fn write_temp_file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "qf_validate_{name}_{}_{}.quay",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(bytes).unwrap();
    path
}

fn run_validate(path: &std::path::Path, semantic: bool) -> std::process::Output {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_qf-validate"));
    if semantic {
        cmd.arg("--semantic");
    }
    cmd.arg("--json").arg(path).output().unwrap()
}

#[test]
fn validate_empty_file() {
    let bytes = MinimalQfWriter::write_empty_file();
    let path = write_temp_file("empty", &bytes);

    // Run qf-validate on the file.
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_qf-validate"))
        .arg(&path)
        .status()
        .expect("qf-validate binary should be runnable");

    assert!(
        status.success(),
        "qf-validate should return exit code 0 for a valid file"
    );
    // Cleanup is best-effort; if removal fails the test OS will clean up temp files.
    let _ = std::fs::remove_file(&path);
}

#[test]
fn validate_corrupted_file() {
    let mut bytes = MinimalQfWriter::write_empty_file();
    // Corrupt the trailing magic.
    let len = bytes.len();
    bytes[len - 1] = 0xFF;

    let path = write_temp_file("corrupt", &bytes);

    let status = std::process::Command::new(env!("CARGO_BIN_EXE_qf-validate"))
        .arg(&path)
        .status()
        .expect("qf-validate binary should be runnable");

    assert!(
        !status.success(),
        "qf-validate should return non-zero for a corrupt file"
    );
    // Cleanup is best-effort; if removal fails the test OS will clean up temp files.
    let _ = std::fs::remove_file(&path);
}

#[test]
fn semantic_cli_rejects_qft_bad_column_domain() {
    let mut writer = MinimalQfWriter::new();
    writer.required_features = FEATURE_TABLE_PROFILE | FEATURE_COLUMN_DOMAINS;
    let header = qf_core::domain::ColumnDomainHeaderV1 {
        table_or_object_id: 1,
        column_or_property_id: 2,
        logical_type: 0,
        collation_id: 0,
        domain_count: 2,
        sorted_file_codes_offset: qf_core::domain::COLUMN_DOMAIN_HEADER_LEN as u64,
        file_code_to_rank_offset: (qf_core::domain::COLUMN_DOMAIN_HEADER_LEN + 8) as u64,
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
    let path = write_temp_file("qft_bad_domain", &writer.write());
    let output = run_validate(&path, true);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success());
    assert!(stdout.contains("QF_E_BAD_DOMAIN"));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn semantic_cli_accepts_scan_profile_writer_file() {
    let catalog = qf_core::table::TableCatalog {
        flags: 0,
        tables: vec![qf_core::table::TableEntry {
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
    let mut writer = ScanProfileQfWriter::new(catalog);
    writer.push_segment(ScanSegment::new(1, 0, 0, 10, 0));
    let path = write_temp_file("scan_profile_writer", &writer.write().unwrap());
    let output = run_validate(&path, true);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains("\"ok\":true"));
    let _ = std::fs::remove_file(&path);
}

fn invalid_execution_descriptor_payload() -> Vec<u8> {
    let mut bytes = qf_core::profile::qfe::ExecutionCodeDescriptorV1 {
        descriptor_id: 1,
        code_kind: qf_core::profile::qfe::ExecutionCodeKind::DictionaryKey,
        code_width_bits: 32,
        byte_order: 0,
        lifetime: qf_core::profile::qfe::ExecutionCodeLifetime::Scan,
        comparison_scope: qf_core::profile::qfe::ExecutionCodeComparisonScope::File,
        canonicality: qf_core::profile::qfe::ExecutionCodeCanonicality::Transient,
        null_code_policy: qf_core::profile::qfe::NullCodePolicy::NullBitmapOnly,
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
fn semantic_cli_rejects_required_qfe_profile_error() {
    let mut writer = MinimalQfWriter::new();
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
    let path = write_temp_file("qfe_required_bad", &writer.write());
    let output = run_validate(&path, true);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success());
    assert!(stdout.contains("QF_E_BAD_ENGINE_PROFILE"));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn semantic_cli_ignores_optional_qfe_profile_error() {
    let mut writer = MinimalQfWriter::new();
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
    let path = write_temp_file("qfe_optional_bad", &writer.write());
    let output = run_validate(&path, true);
    assert!(output.status.success());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn semantic_cli_rejects_required_qfo_schema_error() {
    let mut writer = MinimalQfWriter::new();
    writer.primary_profile = PrimaryProfile::ObjectTemporal as u8;
    writer.required_features = FEATURE_OBJECT_PROFILE;
    let catalog = qf_core::profile::qfo::ObjectTypeCatalog {
        flags: 0,
        types: vec![qf_core::profile::qfo::ObjectTypeEntryV1 {
            object_type_id: 1,
            type_name: "Thing".into(),
            properties: vec![qf_core::profile::qfo::PropertyEntryV1 {
                property_id: 1,
                property_name: "bad".into(),
                logical_type: qf_core::constants::QfLogicalType::Null,
                physical_kind: qf_core::constants::QfPhysicalKind::FileCode,
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
    let path = write_temp_file("qfo_required_bad", &writer.write());
    let output = run_validate(&path, true);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success());
    assert!(stdout.contains("QF_E_BAD_SCHEMA"));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn semantic_cli_ignores_optional_harbor_hint_error() {
    let mut writer = MinimalQfWriter::new();
    writer.primary_profile = PrimaryProfile::Mixed as u8;
    writer.required_features = 0;
    writer.optional_features = FEATURE_HARBOR_PROFILE;
    let mut data = qf_core::profile::qfh::HarborMountHintsV1 {
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
    let path = write_temp_file("qfh_optional_bad", &writer.write());
    let output = run_validate(&path, true);
    assert!(output.status.success());
    let _ = std::fs::remove_file(&path);
}
