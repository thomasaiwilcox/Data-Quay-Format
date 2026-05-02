//! Quay Format (QF) v1.0 — Minimal reference writer.
//!
//! Produces a valid, structurally complete QF file in memory.
//! The produced file satisfies the QF-Core Minimal Profile (Section 71.1).
//!
//! # Example
//!
//! ```rust
//! use qf_core::writer::MinimalQfWriter;
//!
//! let bytes = MinimalQfWriter::write_empty_file();
//! assert!(bytes.len() > 128);
//! ```

use crate::{
    checksum,
    constants::{
        PrimaryProfile, ENDIANNESS_LITTLE, FEATURE_TABLE_PROFILE, FOOTER_VERSION_V1, HEADER_LEN_V1,
        MAGIC_FOOTER, MAGIC_QF, METADATA_LEN_MAX, SECTION_ENTRY_LEN, VERSION_MAJOR_V1,
    },
    footer::{QfFooterHeaderV1, QfSectionEntryV1, FOOTER_HEADER_SIZE},
    header::{QfHeaderV1, HEADER_SIZE},
    postscript::{QfPostscriptV1, QfSectionSpecV1, POSTSCRIPT_SIZE},
};

/// A simple builder for minimal valid QF files.
///
/// Produces files that conform to the QF-Core Minimal Profile (Section 71.1):
/// - valid header,
/// - valid postscript,
/// - valid footer,
/// - binary section directory (possibly empty),
/// - valid checksums.
pub struct MinimalQfWriter {
    /// File creation timestamp (microseconds since Unix epoch).
    pub created_at_us: i64,
    /// Globally unique file identifier.
    pub file_id: [u8; 16],
    /// Producer scope identifier.
    pub producer_scope_id: [u8; 16],
    /// Producer scope kind.
    pub producer_scope_kind: u16,
    /// Primary profile indicator.
    pub primary_profile: u8,
    /// Required feature bits.
    pub required_features: u64,
    /// Optional feature bits.
    pub optional_features: u64,
    /// Optional JSON metadata blob (must be valid UTF-8, ≤ 1 MiB).
    pub metadata_json: Vec<u8>,
    /// Sections to include in the directory.
    pub sections: Vec<SectionPayload>,
}

/// A raw section payload to be embedded in the file.
pub struct SectionPayload {
    pub section_kind: u16,
    pub profile: u8,
    pub flags: u8,
    pub item_count: u64,
    pub row_count: u64,
    pub compression: u8,
    pub alignment_log2: u8,
    pub required_features: u64,
    pub optional_features: u64,
    /// Raw bytes of the section payload (already serialised).
    pub data: Vec<u8>,
}

impl MinimalQfWriter {
    /// Validate builder inputs that have strict on-disk bounds in v1.
    fn validate_inputs(&self) {
        assert!(
            self.metadata_json.len() <= METADATA_LEN_MAX as usize,
            "metadata_json exceeds v1 limit of {} bytes",
            METADATA_LEN_MAX
        );
        assert!(
            self.sections.len() <= u32::MAX as usize,
            "section count exceeds u32::MAX"
        );
    }

    /// Create a writer with all-zero defaults (empty table-scan file).
    pub fn new() -> Self {
        Self {
            created_at_us: 0,
            file_id: [0u8; 16],
            producer_scope_id: [0u8; 16],
            producer_scope_kind: 0,
            primary_profile: PrimaryProfile::TableScan as u8,
            required_features: FEATURE_TABLE_PROFILE,
            optional_features: 0,
            metadata_json: vec![],
            sections: vec![],
        }
    }

    /// Serialise the file to a byte vector.
    ///
    /// Layout:
    /// ```text
    /// [Header: 128 bytes]
    /// [Section payloads ...]
    /// [Footer header: 44 bytes]
    /// [Section entries: section_count × 76 bytes]
    /// [Metadata JSON: metadata_len bytes]
    /// [Postscript: 64 bytes]
    /// [postscript_version: u16]
    /// [postscript_len: u16]
    /// [Magic: "QYF1"]
    /// ```
    pub fn write(&self) -> Vec<u8> {
        self.validate_inputs();

        let mut buf: Vec<u8> = Vec::new();

        // ── 1. Reserve space for header (filled in at the end) ─────────────
        buf.extend_from_slice(&[0u8; HEADER_SIZE]);

        // ── 2. Write section payloads and track their offsets ───────────────
        let mut section_entries: Vec<QfSectionEntryV1> = Vec::new();
        for (idx, section) in self.sections.iter().enumerate() {
            let section_offset = buf.len() as u64;
            let section_data = &section.data;
            let section_len = section_data.len() as u64;
            let section_crc = checksum::crc32c(section_data);

            buf.extend_from_slice(section_data);

            section_entries.push(QfSectionEntryV1 {
                section_id: (idx + 1) as u32,
                section_kind: section.section_kind,
                profile: section.profile,
                flags: section.flags,
                offset: section_offset,
                length: section_len,
                uncompressed_length: section_len,
                item_count: section.item_count,
                row_count: section.row_count,
                compression: section.compression,
                encryption: 0,
                alignment_log2: section.alignment_log2,
                reserved0: 0,
                required_features: section.required_features,
                optional_features: section.optional_features,
                crc32c: section_crc,
                reserved1: 0,
            });
        }

        // ── 3. Build and write footer ────────────────────────────────────────
        let footer_offset = buf.len() as u64;
        let section_count = section_entries.len() as u32;
        let metadata_len = self.metadata_json.len() as u32;

        let footer_header = QfFooterHeaderV1 {
            footer_magic: MAGIC_FOOTER,
            footer_version: FOOTER_VERSION_V1,
            header_len: FOOTER_HEADER_SIZE as u16,
            section_count,
            section_entry_len: SECTION_ENTRY_LEN,
            flags: 0,
            metadata_len,
            reserved: [0u8; 24],
        };
        buf.extend_from_slice(&footer_header.serialize());
        for entry in &section_entries {
            buf.extend_from_slice(&entry.serialize());
        }
        buf.extend_from_slice(&self.metadata_json);

        let footer_len = buf.len() as u64 - footer_offset;
        let footer_crc = checksum::crc32c(&buf[footer_offset as usize..]);

        // ── 4. Write postscript ──────────────────────────────────────────────
        // file_len includes the entire postscript tail (payload + version + len + magic)
        let file_len_before_tail = buf.len() as u64;
        let total_file_len = file_len_before_tail + POSTSCRIPT_SIZE as u64 + 2 + 2 + 4;

        let postscript = QfPostscriptV1 {
            required_features: self.required_features,
            optional_features: self.optional_features,
            file_len: total_file_len,
            footer: QfSectionSpecV1 {
                offset: footer_offset,
                length: footer_len,
                uncompressed_length: footer_len,
                compression: 0,
                encryption: 0,
                alignment_log2: 0,
                flags: 0,
                crc32c: footer_crc,
                reserved: 0,
            },
            checksum: 0, // recomputed by serialize_tail
        };
        buf.extend_from_slice(&postscript.serialize_tail());

        // ── 5. Back-fill the header ──────────────────────────────────────────
        let header = QfHeaderV1 {
            magic: MAGIC_QF,
            header_len: HEADER_LEN_V1,
            version_major: VERSION_MAJOR_V1,
            version_minor: 0,
            primary_profile: self.primary_profile,
            endianness: ENDIANNESS_LITTLE,
            flags: 0,
            required_features: self.required_features,
            optional_features: self.optional_features,
            file_id: self.file_id,
            producer_scope_id: self.producer_scope_id,
            producer_scope_kind: self.producer_scope_kind,
            reserved_scope_flags: 0,
            created_at_us: self.created_at_us,
            reserved: [0u8; 48],
            checksum: 0, // recomputed by serialize()
        };
        let header_bytes = header.serialize();
        buf[..HEADER_SIZE].copy_from_slice(&header_bytes);

        buf
    }

    /// Convenience wrapper: write an empty QF-T file with no sections.
    pub fn write_empty_file() -> Vec<u8> {
        Self::new().write()
    }
}

impl Default for MinimalQfWriter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{footer::QfFooter, header::QfHeaderV1, postscript::QfPostscriptV1};

    #[test]
    #[should_panic(expected = "metadata_json exceeds v1 limit")]
    fn write_rejects_oversized_metadata() {
        let mut w = MinimalQfWriter::new();
        w.metadata_json = vec![0u8; (METADATA_LEN_MAX as usize) + 1];
        let _ = w.write();
    }

    #[test]
    fn empty_file_is_valid() {
        let bytes = MinimalQfWriter::write_empty_file();

        // Parse and validate header.
        let header = QfHeaderV1::parse(&bytes, false).expect("header parse should succeed");
        assert_eq!(header.magic, MAGIC_QF);
        assert_eq!(header.version_major, 1);
        assert_eq!(header.required_features, FEATURE_TABLE_PROFILE);

        // Parse and validate postscript.
        let ps = QfPostscriptV1::parse_from_tail(&bytes).expect("postscript parse should succeed");
        assert_eq!(ps.file_len, bytes.len() as u64);

        // Verify footer CRC.
        let footer_start = ps.footer.offset as usize;
        let footer_end = (ps.footer.offset + ps.footer.length) as usize;
        assert!(footer_end <= bytes.len(), "footer must be within file");
        let footer_bytes = &bytes[footer_start..footer_end];
        let computed_crc = checksum::crc32c(footer_bytes);
        assert_eq!(computed_crc, ps.footer.crc32c, "footer CRC must match");

        // Parse footer.
        let footer = QfFooter::parse(footer_bytes).expect("footer parse should succeed");
        assert_eq!(footer.sections.len(), 0);
    }

    #[test]
    fn file_with_section_is_valid() {
        let mut writer = MinimalQfWriter::new();
        let payload_data = b"hello, quay format".to_vec();
        writer.sections.push(SectionPayload {
            section_kind: crate::constants::SectionKind::FileDictionaryIndex as u16,
            profile: 0,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: crate::constants::FEATURE_FILE_DICTIONARY,
            optional_features: 0,
            data: payload_data.clone(),
        });
        writer.required_features =
            FEATURE_TABLE_PROFILE | crate::constants::FEATURE_FILE_DICTIONARY;

        let bytes = writer.write();

        let ps = QfPostscriptV1::parse_from_tail(&bytes).unwrap();
        assert_eq!(ps.file_len, bytes.len() as u64);

        let footer_bytes =
            &bytes[ps.footer.offset as usize..(ps.footer.offset + ps.footer.length) as usize];
        let footer = QfFooter::parse(footer_bytes).unwrap();
        assert_eq!(footer.sections.len(), 1);
        assert_eq!(
            footer.sections[0].section_kind,
            crate::constants::SectionKind::FileDictionaryIndex as u16
        );

        // Validate section CRC.
        let s = &footer.sections[0];
        let section_data = &bytes[s.offset as usize..(s.offset + s.length) as usize];
        assert_eq!(checksum::crc32c(section_data), s.crc32c);
        assert_eq!(section_data, payload_data.as_slice());
    }
}
