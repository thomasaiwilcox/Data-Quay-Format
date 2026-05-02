//! Quay Format (QF) v1.0 — Footer and section directory.
//!
//! Corresponds to Section 13 of the QF v1.0 specification.
//!
//! The footer immediately follows the last section payload and contains:
//!
//! 1. A fixed 44-byte `QfFooterHeaderV1`.
//! 2. An array of `section_count` × 76-byte `QfSectionEntryV1` records.
//! 3. An optional UTF-8 JSON metadata blob of `metadata_len` bytes.
//!
//! The postscript points to the footer by offset and length so that the footer
//! can be validated (via CRC) before any internal offsets are trusted.

use crate::{
    checksum,
    constants::{
        CompressionCodec, PrimaryProfile, SectionKind, FOOTER_HEADER_LEN, FOOTER_VERSION_V1,
        KNOWN_FEATURE_BITS_MASK, MAGIC_FOOTER, METADATA_LEN_MAX, SECTION_ENTRY_LEN,
    },
    error::QfError,
};

// ── QfFooterHeaderV1 ──────────────────────────────────────────────────────────

/// Serialised size of the footer header in bytes.
pub const FOOTER_HEADER_SIZE: usize = FOOTER_HEADER_LEN;

/// Fixed 44-byte header at the start of every QF footer.
///
/// Corresponds to `QfFooterHeaderV1` in Section 13 of the specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QfFooterHeaderV1 {
    /// Footer magic — must be `b"QYFF"`.
    pub footer_magic: [u8; 4],
    /// Footer version — must be 1.
    pub footer_version: u16,
    /// Byte length of this header structure, used for forward compatibility.
    pub header_len: u16,
    /// Number of section directory entries that follow this header.
    pub section_count: u32,
    /// Byte length of each section directory entry (76 for v1).
    pub section_entry_len: u16,
    /// Footer-level flags.
    pub flags: u16,
    /// Byte length of the optional JSON metadata blob that follows the directory.
    pub metadata_len: u32,
    /// Reserved — MUST be zero in v1.
    pub reserved: [u8; 24],
}

impl QfFooterHeaderV1 {
    /// Serialise to a 44-byte wire buffer.
    pub fn serialize(&self) -> [u8; FOOTER_HEADER_SIZE] {
        let mut buf = [0u8; FOOTER_HEADER_SIZE];
        buf[0..4].copy_from_slice(&self.footer_magic);
        buf[4..6].copy_from_slice(&self.footer_version.to_le_bytes());
        buf[6..8].copy_from_slice(&self.header_len.to_le_bytes());
        buf[8..12].copy_from_slice(&self.section_count.to_le_bytes());
        buf[12..14].copy_from_slice(&self.section_entry_len.to_le_bytes());
        buf[14..16].copy_from_slice(&self.flags.to_le_bytes());
        buf[16..20].copy_from_slice(&self.metadata_len.to_le_bytes());
        buf[20..44].copy_from_slice(&self.reserved);
        buf
    }

    /// Parse and validate from a byte slice.
    pub fn parse(buf: &[u8]) -> Result<Self, QfError> {
        if buf.len() < FOOTER_HEADER_SIZE {
            return Err(QfError::BufferTooShort);
        }
        let footer_magic: [u8; 4] = buf[0..4].try_into().unwrap();
        if footer_magic != MAGIC_FOOTER {
            return Err(QfError::BadMagic);
        }

        let footer_version = u16::from_le_bytes(buf[4..6].try_into().unwrap());
        if footer_version != FOOTER_VERSION_V1 {
            return Err(QfError::BadVersion);
        }

        let header_len = u16::from_le_bytes(buf[6..8].try_into().unwrap());
        if header_len != FOOTER_HEADER_LEN as u16 {
            return Err(QfError::BadSection(format!(
                "footer header_len is {header_len}, expected {FOOTER_HEADER_LEN}"
            )));
        }
        let section_count = u32::from_le_bytes(buf[8..12].try_into().unwrap());

        let section_entry_len = u16::from_le_bytes(buf[12..14].try_into().unwrap());
        if section_entry_len != SECTION_ENTRY_LEN {
            return Err(QfError::BadSection(format!(
                "section_entry_len is {section_entry_len}, expected {SECTION_ENTRY_LEN}"
            )));
        }

        let flags = u16::from_le_bytes(buf[14..16].try_into().unwrap());
        let metadata_len = u32::from_le_bytes(buf[16..20].try_into().unwrap());
        if metadata_len > METADATA_LEN_MAX {
            return Err(QfError::BadSection(format!(
                "metadata_len {metadata_len} exceeds 1 MiB limit"
            )));
        }

        let mut reserved = [0u8; 24];
        reserved.copy_from_slice(&buf[20..44]);
        if reserved.iter().any(|&b| b != 0) {
            return Err(QfError::ReservedNotZero);
        }

        Ok(Self {
            footer_magic,
            footer_version,
            header_len,
            section_count,
            section_entry_len,
            flags,
            metadata_len,
            reserved,
        })
    }

    /// Total byte length of the footer (header + section entries + metadata JSON).
    pub fn total_len(&self) -> Result<u64, QfError> {
        let entries_len = (self.section_count as u64)
            .checked_mul(SECTION_ENTRY_LEN as u64)
            .ok_or(QfError::ArithOverflow)?;
        let base = FOOTER_HEADER_SIZE as u64;
        let with_entries = base
            .checked_add(entries_len)
            .ok_or(QfError::ArithOverflow)?;
        with_entries
            .checked_add(self.metadata_len as u64)
            .ok_or(QfError::ArithOverflow)
    }
}

// ── QfSectionEntryV1 ──────────────────────────────────────────────────────────

/// Serialised size of each section directory entry in bytes.
pub const SECTION_ENTRY_SIZE: usize = SECTION_ENTRY_LEN as usize;

/// One entry in the footer's binary section directory.
///
/// Corresponds to `QfSectionEntryV1` in Section 13 of the specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QfSectionEntryV1 {
    /// Monotonically increasing ID assigned by the writer.
    pub section_id: u32,
    /// Logical kind of this section (see [`SectionKind`]).
    pub section_kind: u16,
    /// Profile this section belongs to (0 = shared, 1–5 = specific profiles).
    pub profile: u8,
    /// Section-level flags.
    pub flags: u8,
    /// Byte offset of the section within the file.
    pub offset: u64,
    /// Byte length of the section on disk (after compression).
    pub length: u64,
    /// Uncompressed byte length.
    pub uncompressed_length: u64,
    /// Number of logical items in this section (interpretation is section-dependent).
    pub item_count: u64,
    /// Number of logical rows covered by this section.
    pub row_count: u64,
    /// Compression codec.
    pub compression: u8,
    /// Encryption scheme — MUST be 0 in v1.
    pub encryption: u8,
    /// `log2` of the section's alignment (advisory).
    pub alignment_log2: u8,
    /// Reserved — MUST be zero.
    pub reserved0: u8,
    /// Required feature bits that must be understood to use this section.
    pub required_features: u64,
    /// Optional feature bits associated with this section.
    pub optional_features: u64,
    /// CRC32C of the section's on-disk bytes.
    pub crc32c: u32,
    /// Reserved — MUST be zero.
    pub reserved1: u32,
}

impl QfSectionEntryV1 {
    /// Serialise to a 76-byte wire buffer.
    pub fn serialize(&self) -> [u8; SECTION_ENTRY_SIZE] {
        let mut buf = [0u8; SECTION_ENTRY_SIZE];
        buf[0..4].copy_from_slice(&self.section_id.to_le_bytes());
        buf[4..6].copy_from_slice(&self.section_kind.to_le_bytes());
        buf[6] = self.profile;
        buf[7] = self.flags;
        buf[8..16].copy_from_slice(&self.offset.to_le_bytes());
        buf[16..24].copy_from_slice(&self.length.to_le_bytes());
        buf[24..32].copy_from_slice(&self.uncompressed_length.to_le_bytes());
        buf[32..40].copy_from_slice(&self.item_count.to_le_bytes());
        buf[40..48].copy_from_slice(&self.row_count.to_le_bytes());
        buf[48] = self.compression;
        buf[49] = self.encryption;
        buf[50] = self.alignment_log2;
        buf[51] = self.reserved0;
        buf[52..60].copy_from_slice(&self.required_features.to_le_bytes());
        buf[60..68].copy_from_slice(&self.optional_features.to_le_bytes());
        buf[68..72].copy_from_slice(&self.crc32c.to_le_bytes());
        buf[72..76].copy_from_slice(&self.reserved1.to_le_bytes());
        buf
    }

    /// Parse from a 76-byte wire buffer.
    pub fn parse(buf: &[u8]) -> Result<Self, QfError> {
        if buf.len() < SECTION_ENTRY_SIZE {
            return Err(QfError::BufferTooShort);
        }
        let section_id = u32::from_le_bytes(buf[0..4].try_into().unwrap());
        let section_kind = u16::from_le_bytes(buf[4..6].try_into().unwrap());
        if SectionKind::from_u16(section_kind).is_none() {
            return Err(QfError::BadSection(format!(
                "unknown section_kind {section_kind}"
            )));
        }
        let profile = buf[6];
        if PrimaryProfile::from_u8(profile).is_none() {
            return Err(QfError::BadSection(format!(
                "unknown section profile {profile}"
            )));
        }
        let flags = buf[7];
        let offset = u64::from_le_bytes(buf[8..16].try_into().unwrap());
        let length = u64::from_le_bytes(buf[16..24].try_into().unwrap());
        let uncompressed_length = u64::from_le_bytes(buf[24..32].try_into().unwrap());
        let item_count = u64::from_le_bytes(buf[32..40].try_into().unwrap());
        let row_count = u64::from_le_bytes(buf[40..48].try_into().unwrap());
        let compression = buf[48];
        let encryption = buf[49];
        let alignment_log2 = buf[50];
        let reserved0 = buf[51];
        let required_features = u64::from_le_bytes(buf[52..60].try_into().unwrap());
        let optional_features = u64::from_le_bytes(buf[60..68].try_into().unwrap());
        let crc32c = u32::from_le_bytes(buf[68..72].try_into().unwrap());
        let reserved1 = u32::from_le_bytes(buf[72..76].try_into().unwrap());

        if CompressionCodec::from_u8(compression).is_none() {
            return Err(QfError::BadSection(format!(
                "unknown compression codec {compression}"
            )));
        }
        if encryption != 0 {
            return Err(QfError::BadSection(format!(
                "section {section_id}: encryption must be 0 in v1, got {encryption}"
            )));
        }
        let unknown_required = required_features & !KNOWN_FEATURE_BITS_MASK;
        if unknown_required != 0 {
            return Err(QfError::UnknownRequiredFeature(unknown_required));
        }
        if reserved0 != 0 || reserved1 != 0 {
            return Err(QfError::ReservedNotZero);
        }
        if compression == CompressionCodec::None as u8 && length != uncompressed_length {
            return Err(QfError::BadSection(format!(
                "section {section_id}: uncompressed_length must equal length when uncompressed"
            )));
        }

        Ok(Self {
            section_id,
            section_kind,
            profile,
            flags,
            offset,
            length,
            uncompressed_length,
            item_count,
            row_count,
            compression,
            encryption,
            alignment_log2,
            reserved0,
            required_features,
            optional_features,
            crc32c,
            reserved1,
        })
    }

    /// Return the end offset (exclusive) of this section, checking for overflow.
    pub fn end_offset(&self) -> Result<u64, QfError> {
        self.offset
            .checked_add(self.length)
            .ok_or(QfError::ArithOverflow)
    }
}

// ── QfFooter ──────────────────────────────────────────────────────────────────

/// Parsed QF footer: header + section directory + optional JSON metadata.
#[derive(Debug, Clone)]
pub struct QfFooter {
    /// The fixed footer header.
    pub header: QfFooterHeaderV1,
    /// The section directory.
    pub sections: Vec<QfSectionEntryV1>,
    /// Optional descriptive JSON metadata (raw bytes).
    pub metadata_json: Vec<u8>,
}

impl QfFooter {
    /// Serialise the complete footer to bytes.
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&self.header.serialize());
        for entry in &self.sections {
            out.extend_from_slice(&entry.serialize());
        }
        out.extend_from_slice(&self.metadata_json);
        out
    }

    /// Parse the footer from a validated byte slice.
    ///
    /// The CRC of the footer bytes must have been verified by the postscript's
    /// `footer.crc32c` field before calling this function.
    pub fn parse(buf: &[u8]) -> Result<Self, QfError> {
        let header = QfFooterHeaderV1::parse(buf)?;

        let expected_len = header.total_len()? as usize;
        if buf.len() < expected_len {
            return Err(QfError::BufferTooShort);
        }

        let entry_size = SECTION_ENTRY_SIZE;
        let entries_start = FOOTER_HEADER_SIZE;
        let mut sections = Vec::with_capacity(header.section_count as usize);
        for i in 0..header.section_count as usize {
            let start = entries_start + i * entry_size;
            let entry = QfSectionEntryV1::parse(&buf[start..])?;
            sections.push(entry);
        }

        let meta_start = entries_start + header.section_count as usize * entry_size;
        let metadata_json = buf[meta_start..meta_start + header.metadata_len as usize].to_vec();
        if std::str::from_utf8(&metadata_json).is_err() {
            return Err(QfError::BadSection(
                "metadata_json must be valid UTF-8".to_string(),
            ));
        }

        Ok(Self {
            header,
            sections,
            metadata_json,
        })
    }

    /// Compute the CRC32C of the footer bytes as produced by [`serialize`].
    ///
    /// Use this to fill the `crc32c` field of the postscript's footer spec.
    pub fn compute_crc(&self) -> u32 {
        let bytes = self.serialize();
        checksum::crc32c(&bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_footer() -> QfFooter {
        QfFooter {
            header: QfFooterHeaderV1 {
                footer_magic: MAGIC_FOOTER,
                footer_version: FOOTER_VERSION_V1,
                header_len: FOOTER_HEADER_SIZE as u16,
                section_count: 0,
                section_entry_len: SECTION_ENTRY_LEN,
                flags: 0,
                metadata_len: 0,
                reserved: [0u8; 24],
            },
            sections: vec![],
            metadata_json: vec![],
        }
    }

    #[test]
    fn empty_footer_roundtrip() {
        let footer = empty_footer();
        let bytes = footer.serialize();
        assert_eq!(bytes.len(), FOOTER_HEADER_SIZE);
        let parsed = QfFooter::parse(&bytes).expect("parse should succeed");
        assert_eq!(parsed.sections.len(), 0);
        assert!(parsed.metadata_json.is_empty());
    }

    #[test]
    fn section_entry_roundtrip() {
        let entry = QfSectionEntryV1 {
            section_id: 1,
            section_kind: 1, // FileDictionaryIndex
            profile: 0,
            flags: 0,
            offset: 256,
            length: 512,
            uncompressed_length: 512,
            item_count: 10,
            row_count: 0,
            compression: 0,
            encryption: 0,
            alignment_log2: 3,
            reserved0: 0,
            required_features: 0x20, // FEATURE_FILE_DICTIONARY
            optional_features: 0,
            crc32c: 0xdeadbeef,
            reserved1: 0,
        };
        let bytes = entry.serialize();
        assert_eq!(bytes.len(), SECTION_ENTRY_SIZE);
        let parsed = QfSectionEntryV1::parse(&bytes).expect("parse should succeed");
        assert_eq!(parsed.section_id, 1);
        assert_eq!(parsed.offset, 256);
        assert_eq!(parsed.length, 512);
        assert_eq!(parsed.crc32c, 0xdeadbeef);
    }

    #[test]
    fn section_entry_encryption_nonzero_rejected() {
        let entry = QfSectionEntryV1 {
            section_id: 1,
            section_kind: 1,
            profile: 0,
            flags: 0,
            offset: 256,
            length: 512,
            uncompressed_length: 512,
            item_count: 0,
            row_count: 0,
            compression: 0,
            encryption: 0,
            alignment_log2: 0,
            reserved0: 0,
            required_features: 0,
            optional_features: 0,
            crc32c: 0,
            reserved1: 0,
        };
        let mut bytes = entry.serialize();
        // Overwrite encryption field (byte 49) with a non-zero value.
        bytes[49] = 1;
        assert!(matches!(
            QfSectionEntryV1::parse(&bytes),
            Err(QfError::BadSection(_))
        ));
    }

    #[test]
    fn section_entry_end_offset_overflow_rejected() {
        let entry = QfSectionEntryV1 {
            section_id: 1,
            section_kind: 1,
            profile: 0,
            flags: 0,
            offset: u64::MAX,
            length: 1,
            uncompressed_length: 1,
            item_count: 0,
            row_count: 0,
            compression: 0,
            encryption: 0,
            alignment_log2: 0,
            reserved0: 0,
            required_features: 0,
            optional_features: 0,
            crc32c: 0,
            reserved1: 0,
        };
        assert_eq!(entry.end_offset(), Err(QfError::ArithOverflow));
    }
}
