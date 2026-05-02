//! Quay Format (QF) v1.0 — reference reader and structural validator.

use std::{collections::HashSet, fs, path::Path};

use crate::{
    checksum,
    constants::MAGIC_QF,
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

    for entry in &footer.sections {
        if !section_ids_seen.insert(entry.section_id) {
            return Err(QfError::BadSection(format!(
                "duplicate section_id {}",
                entry.section_id
            )));
        }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        constants::{SectionKind, FEATURE_FILE_DICTIONARY, FEATURE_TABLE_PROFILE},
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
}
