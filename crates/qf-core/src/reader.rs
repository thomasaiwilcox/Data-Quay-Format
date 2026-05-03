//! Quay Format (QF) v1.0 — reference reader and structural validator.

use std::{fs, path::Path};

use crate::{
    checksum,
    constants::{
        PrimaryProfile, SectionKind, FEATURE_ARCHIVE_PROFILE, FEATURE_CODEC_LZ4,
        FEATURE_CODEC_ZSTD, FEATURE_ENGINE_PROFILE, FEATURE_HARBOR_PROFILE, FEATURE_OBJECT_PROFILE,
        FEATURE_TABLE_PROFILE, MAGIC_QF,
    },
    footer::QfFooter,
    header::{QfHeaderV1, HEADER_SIZE},
    postscript::{QfPostscriptV1, POSTSCRIPT_TOTAL_SIZE},
    QfError,
};

/// Parsed and structurally validated QF file.
#[derive(Debug, Clone)]
pub struct ValidatedQfFile {
    pub header: QfHeaderV1,
    pub postscript: QfPostscriptV1,
    pub footer: QfFooter,
}

/// Read a complete QF file and validate its QF-Core structure.
pub fn read_file(path: &Path) -> Result<ValidatedQfFile, QfError> {
    let data = fs::read(path)?;
    validate_bytes(&data)
}

/// Validate a complete in-memory QF file.
pub fn validate_bytes(data: &[u8]) -> Result<ValidatedQfFile, QfError> {
    if data.len() < HEADER_SIZE + POSTSCRIPT_TOTAL_SIZE {
        return Err(QfError::BufferTooShort);
    }

    let trailing_magic: [u8; 4] = data[data.len() - 4..]
        .try_into()
        .map_err(|_| QfError::BufferTooShort)?;
    if trailing_magic != MAGIC_QF {
        return Err(QfError::BadMagic);
    }

    let postscript = QfPostscriptV1::parse_from_tail(data)?;
    if postscript.file_len != data.len() as u64 {
        return Err(QfError::OffsetRange);
    }

    let footer_end = postscript.footer.end_offset()?;
    let tail_start = data
        .len()
        .checked_sub(POSTSCRIPT_TOTAL_SIZE)
        .ok_or(QfError::BufferTooShort)? as u64;
    if postscript.footer.offset < HEADER_SIZE as u64 || footer_end > tail_start {
        return Err(QfError::OffsetRange);
    }

    let footer_start = postscript.footer.offset as usize;
    let footer_bytes = &data[footer_start..footer_end as usize];
    if checksum::crc32c(footer_bytes) != postscript.footer.crc32c {
        return Err(QfError::ChecksumMismatch);
    }

    let footer = QfFooter::parse(footer_bytes)?;
    if footer.header.total_len()? != postscript.footer.length {
        return Err(QfError::BadSection(
            "footer header length does not match postscript footer length".to_string(),
        ));
    }

    let header = QfHeaderV1::parse(data, false)?;
    validate_sections(data, footer_start, &footer, &header)?;
    validate_primary_profile_features(&header)?;
    if header.required_features != postscript.required_features
        || header.optional_features != postscript.optional_features
    {
        return Err(QfError::BadSection(
            "header and postscript feature bits differ".to_string(),
        ));
    }

    Ok(ValidatedQfFile {
        header,
        postscript,
        footer,
    })
}

fn validate_sections(
    data: &[u8],
    footer_start: usize,
    footer: &QfFooter,
    header: &QfHeaderV1,
) -> Result<(), QfError> {
    let mut ranges: Vec<(u64, u64, u32)> = Vec::new();
    let mut last_section_id: Option<u32> = None;

    for entry in &footer.sections {
        if let Some(last) = last_section_id {
            if entry.section_id <= last {
                return Err(QfError::BadSection(format!(
                    "section_id {} is not greater than previous id {}",
                    entry.section_id, last
                )));
            }
        }
        last_section_id = Some(entry.section_id);

        validate_section_profile(entry.section_kind, entry.profile)?;
        validate_section_profile_feature_bit(entry.profile, header.required_features)?;
        validate_codec_feature_advertisement(entry.compression, header, entry)?;

        let section_end = entry.end_offset()?;
        if entry.offset < HEADER_SIZE as u64 || section_end > footer_start as u64 {
            return Err(QfError::OffsetRange);
        }

        for (start, end, id) in &ranges {
            if entry.length != 0 && entry.offset < *end && section_end > *start {
                return Err(QfError::BadSection(format!(
                    "section {} overlaps section {id}",
                    entry.section_id
                )));
            }
        }

        let section_bytes = &data[entry.offset as usize..section_end as usize];
        if checksum::crc32c(section_bytes) != entry.crc32c {
            return Err(QfError::ChecksumMismatch);
        }
        ranges.push((entry.offset, section_end, entry.section_id));
    }

    Ok(())
}

fn validate_section_profile_feature_bit(
    profile: u8,
    file_required_features: u64,
) -> Result<(), QfError> {
    let required_profile_bit = match profile {
        0 => return Ok(()),
        1 => FEATURE_OBJECT_PROFILE,
        2 => FEATURE_TABLE_PROFILE,
        3 => FEATURE_ARCHIVE_PROFILE,
        4 => FEATURE_ENGINE_PROFILE,
        5 => FEATURE_HARBOR_PROFILE,
        _ => {
            return Err(QfError::BadSection(format!(
                "unknown profile {profile} in section directory"
            )))
        }
    };
    if file_required_features & required_profile_bit == 0 {
        return Err(QfError::BadSection(format!(
            "section profile {profile} requires missing file feature bit 0x{required_profile_bit:016x}"
        )));
    }
    Ok(())
}

fn validate_codec_feature_advertisement(
    compression: u8,
    header: &QfHeaderV1,
    entry: &crate::footer::QfSectionEntryV1,
) -> Result<(), QfError> {
    let advertised = header.required_features | header.optional_features;
    let section_advertised = entry.required_features | entry.optional_features;
    match compression {
        1 => {
            if advertised & FEATURE_CODEC_LZ4 == 0 || section_advertised & FEATURE_CODEC_LZ4 == 0 {
                return Err(QfError::BadSection(format!(
                    "section {} uses LZ4 compression but codec feature bit is not advertised",
                    entry.section_id
                )));
            }
        }
        2 => {
            if advertised & FEATURE_CODEC_ZSTD == 0 || section_advertised & FEATURE_CODEC_ZSTD == 0
            {
                return Err(QfError::BadSection(format!(
                    "section {} uses ZSTD compression but codec feature bit is not advertised",
                    entry.section_id
                )));
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_primary_profile_features(header: &QfHeaderV1) -> Result<(), QfError> {
    let profile = PrimaryProfile::from_u8(header.primary_profile)
        .ok_or_else(|| QfError::BadSection("unknown primary profile".to_string()))?;

    let required_bit = match profile {
        PrimaryProfile::Mixed => return Ok(()),
        PrimaryProfile::ObjectTemporal => FEATURE_OBJECT_PROFILE,
        PrimaryProfile::TableScan => FEATURE_TABLE_PROFILE,
        PrimaryProfile::ArchiveAcceleration => FEATURE_ARCHIVE_PROFILE,
        PrimaryProfile::EngineExecution => FEATURE_ENGINE_PROFILE,
        PrimaryProfile::HarborExecution => FEATURE_HARBOR_PROFILE,
    };

    if header.required_features & required_bit == 0 {
        return Err(QfError::BadSection(format!(
            "primary_profile {:?} requires feature bit 0x{required_bit:016x}",
            profile
        )));
    }
    Ok(())
}

fn validate_section_profile(section_kind: u16, profile: u8) -> Result<(), QfError> {
    let section = SectionKind::from_u16(section_kind)
        .ok_or_else(|| QfError::BadSection(format!("unknown section_kind {section_kind}")));
    let allowed: &[u8] = match section? {
        // shared (profile 0)
        SectionKind::FileDictionaryIndex
        | SectionKind::FileDictionaryPayload
        | SectionKind::CollationRegistry
        | SectionKind::DigestManifest
        | SectionKind::RedactionManifest
        | SectionKind::ArrowInteropHints
        | SectionKind::LakehouseHints
        | SectionKind::ExtensionRegistry
        | SectionKind::ProfileCapabilityMatrix
        | SectionKind::VendorExtension => &[0],
        // QF-T only (profile 2)
        SectionKind::TableCatalog
        | SectionKind::TableSegmentIndex
        | SectionKind::TableSegmentData
        | SectionKind::ColumnDomain
        | SectionKind::ZoneStats => &[2],
        // QF-T/QF-A (profiles 2 or 3)
        SectionKind::ExactSetIndex
        | SectionKind::BloomIndex
        | SectionKind::InvertedMorselIndex
        | SectionKind::KernelCapabilities => &[2, 3],
        // QF-A only (profile 3)
        SectionKind::LookupIndex
        | SectionKind::AggregateSynopsis
        | SectionKind::CompositeZoneIndex
        | SectionKind::TopNZoneSummary => &[3],
        // QF-E (profile 4)
        SectionKind::EngineProfileRegistry
        | SectionKind::ExecutionCodeDescriptor
        | SectionKind::ExecutionScopeDescriptor
        | SectionKind::CodeSpaceDescriptor
        | SectionKind::EngineMountPolicy => &[4],
        // QF-O (profile 1)
        SectionKind::ObjectTypeCatalog
        | SectionKind::TemporalSegmentIndex
        | SectionKind::TemporalSegmentData
        | SectionKind::TemporalBloomIndex
        | SectionKind::TrustManifest => &[1],
        // QF-H (profile 5)
        SectionKind::HarborMountHints => &[5],
    };
    if !allowed.contains(&profile) {
        return Err(QfError::BadSection(format!(
            "section_kind {section_kind} must use one of profiles {allowed:?}, got {profile}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        constants::{
            CompressionCodec, SectionKind, FEATURE_CODEC_LZ4, FEATURE_FILE_DICTIONARY,
            FEATURE_HARBOR_PROFILE, FEATURE_TABLE_PROFILE,
        },
        postscript::POSTSCRIPT_TOTAL_SIZE,
        writer::{MinimalQfWriter, SectionPayload},
    };

    fn rewrite_postscript(bytes: &mut [u8], postscript: QfPostscriptV1) {
        let tail_start = bytes.len() - POSTSCRIPT_TOTAL_SIZE;
        bytes[tail_start..].copy_from_slice(&postscript.serialize_tail());
    }

    #[test]
    fn validates_empty_file() {
        let bytes = MinimalQfWriter::write_empty_file();
        let file = validate_bytes(&bytes).expect("minimal file should validate");
        assert_eq!(file.header.required_features, FEATURE_TABLE_PROFILE);
        assert_eq!(file.footer.sections.len(), 0);
    }

    #[test]
    fn rejects_section_crc_mismatch() {
        let mut writer = MinimalQfWriter::new();
        writer.required_features = FEATURE_TABLE_PROFILE | FEATURE_FILE_DICTIONARY;
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::FileDictionaryIndex as u16,
            profile: 0,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: FEATURE_FILE_DICTIONARY,
            optional_features: 0,
            data: b"abcdef".to_vec(),
        });
        let mut bytes = writer.write();
        bytes[HEADER_SIZE] ^= 0x01;
        assert!(matches!(
            validate_bytes(&bytes),
            Err(QfError::ChecksumMismatch)
        ));
    }

    #[test]
    fn rejects_non_utf8_metadata_written_by_external_source() {
        let mut writer = MinimalQfWriter::new();
        writer.metadata_json = b"{}".to_vec();
        let mut bytes = writer.write();

        let ps = QfPostscriptV1::parse_from_tail(&bytes).unwrap();
        let metadata_offset = ps.footer.offset as usize + 44;
        bytes[metadata_offset] = 0xff;

        let footer_start = ps.footer.offset as usize;
        let footer_len = ps.footer.length as usize;
        let footer_crc = checksum::crc32c(&bytes[footer_start..footer_start + footer_len]);
        let mut fixed_ps = ps;
        fixed_ps.footer.crc32c = footer_crc;
        let tail_start = bytes.len() - POSTSCRIPT_TOTAL_SIZE;
        bytes[tail_start..].copy_from_slice(&fixed_ps.serialize_tail());

        assert!(matches!(
            validate_bytes(&bytes),
            Err(QfError::BadSection(_))
        ));
    }

    #[test]
    fn rejects_overlapping_sections() {
        let mut writer = MinimalQfWriter::new();
        writer.required_features = FEATURE_TABLE_PROFILE | FEATURE_FILE_DICTIONARY;
        for data in [b"first".to_vec(), b"second".to_vec()] {
            writer.sections.push(SectionPayload {
                section_kind: SectionKind::FileDictionaryIndex as u16,
                profile: 0,
                flags: 0,
                item_count: 1,
                row_count: 0,
                compression: 0,
                alignment_log2: 0,
                required_features: FEATURE_FILE_DICTIONARY,
                optional_features: 0,
                data,
            });
        }

        let mut bytes = writer.write();
        let ps = QfPostscriptV1::parse_from_tail(&bytes).unwrap();
        let footer_start = ps.footer.offset as usize;
        let entries_start = footer_start + 44;
        let first_offset = u64::from_le_bytes(
            bytes[entries_start + 8..entries_start + 16]
                .try_into()
                .unwrap(),
        );
        let second_offset_pos = entries_start + 76 + 8;
        bytes[second_offset_pos..second_offset_pos + 8]
            .copy_from_slice(&first_offset.to_le_bytes());

        let footer_len = ps.footer.length as usize;
        let footer_crc = checksum::crc32c(&bytes[footer_start..footer_start + footer_len]);
        let mut fixed_ps = ps;
        fixed_ps.footer.crc32c = footer_crc;
        let tail_start = bytes.len() - POSTSCRIPT_TOTAL_SIZE;
        bytes[tail_start..].copy_from_slice(&fixed_ps.serialize_tail());

        assert!(matches!(
            validate_bytes(&bytes),
            Err(QfError::BadSection(_))
        ));
    }

    #[test]
    fn rejects_out_of_order_section_ids() {
        let mut writer = MinimalQfWriter::new();
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::FileDictionaryIndex as u16,
            profile: 0,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: 0,
            optional_features: 0,
            data: b"first".to_vec(),
        });
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::FileDictionaryPayload as u16,
            profile: 0,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: 0,
            optional_features: 0,
            data: b"second".to_vec(),
        });
        let mut bytes = writer.write();
        let ps = QfPostscriptV1::parse_from_tail(&bytes).unwrap();
        let footer_start = ps.footer.offset as usize;
        let entries_start = footer_start + 44;
        bytes[entries_start + 76..entries_start + 80].copy_from_slice(&1u32.to_le_bytes());

        let footer_len = ps.footer.length as usize;
        let footer_crc = checksum::crc32c(&bytes[footer_start..footer_start + footer_len]);
        let mut fixed_ps = ps;
        fixed_ps.footer.crc32c = footer_crc;
        let tail_start = bytes.len() - POSTSCRIPT_TOTAL_SIZE;
        bytes[tail_start..].copy_from_slice(&fixed_ps.serialize_tail());

        assert!(matches!(
            validate_bytes(&bytes),
            Err(QfError::BadSection(_))
        ));
    }

    #[test]
    fn rejects_section_profile_feature_missing_from_header() {
        let mut writer = MinimalQfWriter::new();
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::TableCatalog as u16,
            profile: 2,
            flags: 0,
            item_count: 0,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: 0,
            optional_features: 0,
            data: b"x".to_vec(),
        });
        writer.required_features = 0;
        let bytes = writer.write();
        assert!(matches!(
            validate_bytes(&bytes),
            Err(QfError::BadSection(_))
        ));
    }

    #[test]
    fn rejects_lz4_section_without_codec_feature_advertised() {
        let mut writer = MinimalQfWriter::new();
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::FileDictionaryIndex as u16,
            profile: 0,
            flags: 0,
            item_count: 0,
            row_count: 0,
            compression: CompressionCodec::Lz4 as u8,
            alignment_log2: 0,
            required_features: 0,
            optional_features: 0,
            data: b"lz4-ish".to_vec(),
        });
        let bytes = writer.write();
        assert!(matches!(
            validate_bytes(&bytes),
            Err(QfError::BadSection(_))
        ));

        let mut good_writer = MinimalQfWriter::new();
        good_writer.optional_features = FEATURE_CODEC_LZ4;
        good_writer.sections.push(SectionPayload {
            section_kind: SectionKind::FileDictionaryIndex as u16,
            profile: 0,
            flags: 0,
            item_count: 0,
            row_count: 0,
            compression: CompressionCodec::Lz4 as u8,
            alignment_log2: 0,
            required_features: 0,
            optional_features: FEATURE_CODEC_LZ4,
            data: b"lz4-ish".to_vec(),
        });
        let good_bytes = good_writer.write();
        assert!(validate_bytes(&good_bytes).is_ok());
    }

    #[test]
    fn rejects_zstd_section_without_codec_feature_advertised() {
        let mut writer = MinimalQfWriter::new();
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::FileDictionaryIndex as u16,
            profile: 0,
            flags: 0,
            item_count: 0,
            row_count: 0,
            compression: CompressionCodec::Zstd as u8,
            alignment_log2: 0,
            required_features: 0,
            optional_features: 0,
            data: b"zstd-ish".to_vec(),
        });
        assert!(matches!(validate_bytes(&writer.write()), Err(QfError::BadSection(_))));
    }

    #[test]
    fn rejects_primary_profile_missing_required_feature() {
        let mut writer = MinimalQfWriter::new();
        writer.primary_profile = PrimaryProfile::HarborExecution as u8;
        writer.required_features = FEATURE_TABLE_PROFILE;
        assert!(matches!(validate_bytes(&writer.write()), Err(QfError::BadSection(_))));
    }

    #[test]
    fn accepts_primary_profile_when_required_feature_present() {
        let mut writer = MinimalQfWriter::new();
        writer.primary_profile = PrimaryProfile::HarborExecution as u8;
        writer.required_features = FEATURE_HARBOR_PROFILE;
        assert!(validate_bytes(&writer.write()).is_ok());
    }

    #[test]
    fn rejects_table_catalog_with_wrong_profile() {
        let mut writer = MinimalQfWriter::new();
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::TableCatalog as u16,
            profile: 0,
            flags: 0,
            item_count: 0,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: 0,
            optional_features: 0,
            data: vec![1],
        });
        assert!(matches!(validate_bytes(&writer.write()), Err(QfError::BadSection(_))));
    }

    #[test]
    fn rejects_harbor_mount_hints_with_wrong_profile() {
        let mut writer = MinimalQfWriter::new();
        writer.required_features = FEATURE_HARBOR_PROFILE;
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::HarborMountHints as u16,
            profile: 4,
            flags: 0,
            item_count: 0,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: 0,
            optional_features: 0,
            data: vec![1],
        });
        assert!(matches!(validate_bytes(&writer.write()), Err(QfError::BadSection(_))));
    }

    #[test]
    fn rejects_bad_trailing_magic() {
        let mut bytes = MinimalQfWriter::write_empty_file();
        let len = bytes.len();
        bytes[len - 1] = b'X';
        assert!(matches!(validate_bytes(&bytes), Err(QfError::BadMagic)));
    }

    #[test]
    fn rejects_postscript_file_length_mismatch() {
        let mut bytes = MinimalQfWriter::write_empty_file();
        let mut ps = QfPostscriptV1::parse_from_tail(&bytes).unwrap();
        ps.file_len += 1;
        rewrite_postscript(&mut bytes, ps);
        assert!(matches!(validate_bytes(&bytes), Err(QfError::OffsetRange)));
    }

    #[test]
    fn rejects_footer_crc_mismatch() {
        let mut writer = MinimalQfWriter::new();
        writer.metadata_json = br#"{"k":"v"}"#.to_vec();
        let mut bytes = writer.write();
        let ps = QfPostscriptV1::parse_from_tail(&bytes).unwrap();
        bytes[ps.footer.offset as usize] ^= 1;
        assert!(matches!(
            validate_bytes(&bytes),
            Err(QfError::ChecksumMismatch)
        ));
    }

    #[test]
    fn rejects_postscript_footer_range_before_header() {
        let mut bytes = MinimalQfWriter::write_empty_file();
        let mut ps = QfPostscriptV1::parse_from_tail(&bytes).unwrap();
        ps.footer.offset = (HEADER_SIZE - 1) as u64;
        rewrite_postscript(&mut bytes, ps);
        assert!(matches!(validate_bytes(&bytes), Err(QfError::OffsetRange)));
    }

    #[test]
    fn rejects_header_and_postscript_feature_mismatch() {
        let mut bytes = MinimalQfWriter::write_empty_file();
        let mut ps = QfPostscriptV1::parse_from_tail(&bytes).unwrap();
        ps.optional_features = FEATURE_CODEC_LZ4;
        rewrite_postscript(&mut bytes, ps);
        assert!(matches!(validate_bytes(&bytes), Err(QfError::BadSection(_))));
    }

    #[test]
    fn rejects_section_outside_data_region() {
        let mut writer = MinimalQfWriter::new();
        writer.required_features = FEATURE_TABLE_PROFILE | FEATURE_FILE_DICTIONARY;
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::FileDictionaryIndex as u16,
            profile: 0,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: FEATURE_FILE_DICTIONARY,
            optional_features: 0,
            data: b"abcdef".to_vec(),
        });
        let mut bytes = writer.write();
        let ps = QfPostscriptV1::parse_from_tail(&bytes).unwrap();
        let footer_start = ps.footer.offset as usize;
        let entries_start = footer_start + 44;
        let bad_offset = (footer_start as u64) + 1;
        bytes[entries_start + 8..entries_start + 16].copy_from_slice(&bad_offset.to_le_bytes());
        let footer_crc = checksum::crc32c(&bytes[footer_start..footer_start + ps.footer.length as usize]);
        let mut fixed_ps = ps;
        fixed_ps.footer.crc32c = footer_crc;
        rewrite_postscript(&mut bytes, fixed_ps);
        assert!(matches!(validate_bytes(&bytes), Err(QfError::OffsetRange)));
    }

    #[test]
    fn rejects_unknown_section_profile_in_directory() {
        let mut writer = MinimalQfWriter::new();
        writer.required_features = FEATURE_TABLE_PROFILE | FEATURE_FILE_DICTIONARY;
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::FileDictionaryIndex as u16,
            profile: 0,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: FEATURE_FILE_DICTIONARY,
            optional_features: 0,
            data: b"abcdef".to_vec(),
        });
        let mut bytes = writer.write();
        let ps = QfPostscriptV1::parse_from_tail(&bytes).unwrap();
        let footer_start = ps.footer.offset as usize;
        let entries_start = footer_start + 44;
        bytes[entries_start + 6] = 99;
        let footer_crc = checksum::crc32c(&bytes[footer_start..footer_start + ps.footer.length as usize]);
        let mut fixed_ps = ps;
        fixed_ps.footer.crc32c = footer_crc;
        rewrite_postscript(&mut bytes, fixed_ps);
        assert!(matches!(validate_bytes(&bytes), Err(QfError::BadSection(_))));
    }

    #[test]
    fn rejects_unknown_section_kind_in_directory() {
        let mut writer = MinimalQfWriter::new();
        writer.required_features = FEATURE_TABLE_PROFILE | FEATURE_FILE_DICTIONARY;
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::FileDictionaryIndex as u16,
            profile: 0,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: FEATURE_FILE_DICTIONARY,
            optional_features: 0,
            data: b"abcdef".to_vec(),
        });
        let mut bytes = writer.write();
        let ps = QfPostscriptV1::parse_from_tail(&bytes).unwrap();
        let footer_start = ps.footer.offset as usize;
        let entries_start = footer_start + 44;
        bytes[entries_start + 4..entries_start + 6].copy_from_slice(&999u16.to_le_bytes());
        let footer_crc = checksum::crc32c(&bytes[footer_start..footer_start + ps.footer.length as usize]);
        let mut fixed_ps = ps;
        fixed_ps.footer.crc32c = footer_crc;
        rewrite_postscript(&mut bytes, fixed_ps);
        assert!(matches!(validate_bytes(&bytes), Err(QfError::BadSection(_))));
    }

    #[test]
    fn enforces_profile_feature_bit_for_every_non_mixed_profile() {
        let cases: &[(u8, u16, u64)] = &[
            (1, SectionKind::ObjectTypeCatalog as u16, crate::constants::FEATURE_OBJECT_PROFILE),
            (2, SectionKind::TableCatalog as u16, FEATURE_TABLE_PROFILE),
            (3, SectionKind::LookupIndex as u16, crate::constants::FEATURE_ARCHIVE_PROFILE),
            (4, SectionKind::EngineProfileRegistry as u16, crate::constants::FEATURE_ENGINE_PROFILE),
            (5, SectionKind::HarborMountHints as u16, FEATURE_HARBOR_PROFILE),
        ];

        for (profile, kind, required_bit) in cases {
            let mut writer = MinimalQfWriter::new();
            writer.primary_profile = 0; // mixed; avoid primary-profile gating noise
            writer.required_features = 0;
            writer.sections.push(SectionPayload {
                section_kind: *kind,
                profile: *profile,
                flags: 0,
                item_count: 0,
                row_count: 0,
                compression: 0,
                alignment_log2: 0,
                required_features: 0,
                optional_features: 0,
                data: b"x".to_vec(),
            });
            assert!(matches!(validate_bytes(&writer.write()), Err(QfError::BadSection(_))));

            writer.required_features = *required_bit;
            assert!(
                validate_bytes(&writer.write()).is_ok(),
                "profile {profile} should validate when feature bit is present"
            );
        }
    }

    #[test]
    fn rejects_too_short_file() {
        let bytes = vec![0u8; HEADER_SIZE + POSTSCRIPT_TOTAL_SIZE - 1];
        assert!(matches!(validate_bytes(&bytes), Err(QfError::BufferTooShort)));
    }

    #[test]
    fn rejects_invalid_footer_header_shape() {
        let mut bytes = MinimalQfWriter::write_empty_file();
        let ps = QfPostscriptV1::parse_from_tail(&bytes).unwrap();
        let footer_start = ps.footer.offset as usize;
        // footer.header.section_entry_len @ offset 12 in footer header
        bytes[footer_start + 12..footer_start + 14].copy_from_slice(&0u16.to_le_bytes());
        let footer_crc = checksum::crc32c(&bytes[footer_start..footer_start + ps.footer.length as usize]);
        let mut fixed_ps = ps;
        fixed_ps.footer.crc32c = footer_crc;
        rewrite_postscript(&mut bytes, fixed_ps);
        assert!(matches!(validate_bytes(&bytes), Err(QfError::BadSection(_))));
    }

    #[test]
    fn rejects_bad_header_after_all_tail_checks_pass() {
        let mut bytes = MinimalQfWriter::write_empty_file();
        // Corrupt header magic, then recompute header checksum so header parse fails on magic,
        // not checksum mismatch.
        bytes[0..4].copy_from_slice(b"BAD!");
        bytes[124..128].copy_from_slice(&[0, 0, 0, 0]);
        let crc = checksum::crc32c(&bytes[..HEADER_SIZE]);
        bytes[124..128].copy_from_slice(&crc.to_le_bytes());
        assert!(matches!(validate_bytes(&bytes), Err(QfError::BadMagic)));
    }
}
