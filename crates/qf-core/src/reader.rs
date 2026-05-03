//! Quay Format (QF) v1.0 — reference reader and structural validator.

use std::{collections::HashSet, fs, path::Path};

use crate::{
    checksum,
    constants::{
        PrimaryProfile, SectionKind, FEATURE_ARCHIVE_PROFILE, FEATURE_ENGINE_PROFILE,
        FEATURE_CODEC_LZ4, FEATURE_CODEC_ZSTD, FEATURE_HARBOR_PROFILE, FEATURE_OBJECT_PROFILE,
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

    validate_sections(data, footer_start, &footer)?;

    let header = QfHeaderV1::parse(data, false)?;
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

fn validate_sections(data: &[u8], footer_start: usize, footer: &QfFooter) -> Result<(), QfError> {
    let mut section_ids_seen = HashSet::new();
    let mut ranges: Vec<(u64, u64, u32)> = Vec::new();
    let mut expected_section_id: u32 = 1;

    for entry in &footer.sections {
        if !section_ids_seen.insert(entry.section_id) {
            return Err(QfError::BadSection(format!(
                "duplicate section_id {}",
                entry.section_id
            )));
        }
        if entry.section_id != expected_section_id {
            return Err(QfError::BadSection(format!(
                "section_id {} out of sequence, expected {}",
                entry.section_id, expected_section_id
            )));
        }
        expected_section_id = expected_section_id.saturating_add(1);

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

fn validate_section_profile_feature_bit(profile: u8, file_required_features: u64) -> Result<(), QfError> {
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
    let expected = match section? {
        SectionKind::FileDictionaryIndex
        | SectionKind::FileDictionaryPayload
        | SectionKind::CollationRegistry
        | SectionKind::DigestManifest
        | SectionKind::RedactionManifest
        | SectionKind::ArrowInteropHints
        | SectionKind::LakehouseHints
        | SectionKind::ExtensionRegistry
        | SectionKind::ProfileCapabilityMatrix
        | SectionKind::VendorExtension => 0,
        SectionKind::TableCatalog
        | SectionKind::TableSegmentIndex
        | SectionKind::TableSegmentData
        | SectionKind::ColumnDomain
        | SectionKind::ZoneStats
        | SectionKind::ExactSetIndex
        | SectionKind::BloomIndex
        | SectionKind::InvertedMorselIndex
        | SectionKind::LookupIndex
        | SectionKind::AggregateSynopsis
        | SectionKind::CompositeZoneIndex
        | SectionKind::TopNZoneSummary
        | SectionKind::KernelCapabilities => 2,
        SectionKind::EngineProfileRegistry
        | SectionKind::ExecutionCodeDescriptor
        | SectionKind::ExecutionScopeDescriptor
        | SectionKind::CodeSpaceDescriptor
        | SectionKind::EngineMountPolicy => 4,
        SectionKind::ObjectTypeCatalog
        | SectionKind::TemporalSegmentIndex
        | SectionKind::TemporalSegmentData
        | SectionKind::TemporalBloomIndex
        | SectionKind::TrustManifest => 1,
        SectionKind::HarborMountHints => 5,
    };
    if profile != expected {
        return Err(QfError::BadSection(format!(
            "section_kind {section_kind} must use profile {expected}, got {profile}"
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
            FEATURE_TABLE_PROFILE,
        },
        postscript::POSTSCRIPT_TOTAL_SIZE,
        writer::{MinimalQfWriter, SectionPayload},
    };

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
        bytes[entries_start + 76..entries_start + 80].copy_from_slice(&3u32.to_le_bytes());

        let footer_len = ps.footer.length as usize;
        let footer_crc = checksum::crc32c(&bytes[footer_start..footer_start + footer_len]);
        let mut fixed_ps = ps;
        fixed_ps.footer.crc32c = footer_crc;
        let tail_start = bytes.len() - POSTSCRIPT_TOTAL_SIZE;
        bytes[tail_start..].copy_from_slice(&fixed_ps.serialize_tail());

        assert!(matches!(validate_bytes(&bytes), Err(QfError::BadSection(_))));
    }
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
        assert!(matches!(validate_bytes(&bytes), Err(QfError::BadSection(_))));
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
        assert!(matches!(validate_bytes(&bytes), Err(QfError::BadSection(_))));

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
