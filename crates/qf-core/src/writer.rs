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
        CompressionCodec, PrimaryProfile, ProducerScopeKind, SectionKind, ENDIANNESS_LITTLE,
        FEATURE_TABLE_PROFILE, FOOTER_VERSION_V1, HEADER_LEN_V1, KNOWN_FEATURE_BITS_MASK,
        MAGIC_FOOTER, MAGIC_QF, METADATA_LEN_MAX, SECTION_ENTRY_LEN, VERSION_MAJOR_V1,
    },
    footer::{QfFooterHeaderV1, QfSectionEntryV1, FOOTER_HEADER_SIZE},
    header::{QfHeaderV1, HEADER_SIZE},
    metadata,
    postscript::{QfPostscriptV1, QfSectionSpecV1, POSTSCRIPT_SIZE},
    segment::{
        RowMorselDirectory, RowMorselEntryV1, TableSegmentHeaderV1, TableSegmentIndex,
        TableSegmentIndexEntryV1, ROW_MORSEL_ENTRY_LEN, TABLE_SEGMENT_HEADER_LEN,
        TABLE_SEGMENT_INDEX_ENTRY_LEN,
    },
    table::TableCatalog,
    QfError,
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
            std::str::from_utf8(&self.metadata_json).is_ok(),
            "metadata_json must be valid UTF-8"
        );
        assert!(
            metadata::validate(&self.metadata_json).is_ok(),
            "metadata_json must be syntactically valid JSON"
        );
        assert!(
            self.sections.len() <= u32::MAX as usize,
            "section count exceeds u32::MAX"
        );
        assert!(
            PrimaryProfile::from_u8(self.primary_profile).is_some(),
            "unknown primary_profile {}",
            self.primary_profile
        );
        assert!(
            ProducerScopeKind::from_u16(self.producer_scope_kind).is_some(),
            "unknown producer_scope_kind {}",
            self.producer_scope_kind
        );
        assert!(
            self.required_features & !KNOWN_FEATURE_BITS_MASK == 0,
            "unknown required feature bits 0x{:016x}",
            self.required_features & !KNOWN_FEATURE_BITS_MASK
        );
        for section in &self.sections {
            assert!(
                SectionKind::from_u16(section.section_kind).is_some(),
                "unknown section_kind {}",
                section.section_kind
            );
            assert!(
                PrimaryProfile::from_u8(section.profile).is_some(),
                "unknown section profile {}",
                section.profile
            );
            assert!(
                CompressionCodec::from_u8(section.compression).is_some(),
                "unknown compression codec {}",
                section.compression
            );
            assert!(
                section.required_features & !KNOWN_FEATURE_BITS_MASK == 0,
                "unknown section required feature bits 0x{:016x}",
                section.required_features & !KNOWN_FEATURE_BITS_MASK
            );
        }
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

/// QF-T scan-profile writer surface (Spec §71.2/§71.3).
///
/// This builder emits a structurally valid table scan file with:
/// table catalog, table segment index, and table segment data sections.
/// It computes segment payload offsets before delegating to
/// [`MinimalQfWriter`] so the segment index points at the actual bytes in
/// the produced file.
pub struct ScanProfileQfWriter {
    pub created_at_us: i64,
    pub file_id: [u8; 16],
    pub producer_scope_id: [u8; 16],
    pub producer_scope_kind: u16,
    pub metadata_json: Vec<u8>,
    pub table_catalog: TableCatalog,
    pub segments: Vec<ScanSegment>,
}

/// Segment declaration accepted by [`ScanProfileQfWriter`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanSegment {
    pub table_id: u32,
    pub segment_id: u32,
    pub row_start: u64,
    pub row_count: u32,
    pub morsel_row_count: u32,
    pub column_count: u32,
    pub stats_ref: u32,
    pub flags: u32,
}

impl ScanSegment {
    pub fn new(
        table_id: u32,
        segment_id: u32,
        row_start: u64,
        row_count: u32,
        column_count: u32,
    ) -> Self {
        Self {
            table_id,
            segment_id,
            row_start,
            row_count,
            morsel_row_count: 4096,
            column_count,
            stats_ref: 0,
            flags: 0,
        }
    }

    fn morsel_count(&self) -> Result<u32, QfError> {
        if self.row_count == 0 {
            return Ok(0);
        }
        if self.morsel_row_count == 0 {
            return Err(QfError::SegmentCorrupt);
        }
        let count = self
            .row_count
            .checked_add(self.morsel_row_count - 1)
            .ok_or(QfError::ArithOverflow)?
            / self.morsel_row_count;
        Ok(count)
    }

    fn payload(&self) -> Result<Vec<u8>, QfError> {
        let morsel_count = self.morsel_count()?;
        let morsel_dir_len = (morsel_count as usize)
            .checked_mul(ROW_MORSEL_ENTRY_LEN)
            .ok_or(QfError::ArithOverflow)?;
        let column_directory_offset = TABLE_SEGMENT_HEADER_LEN
            .checked_add(morsel_dir_len)
            .ok_or(QfError::ArithOverflow)? as u64;
        let header = TableSegmentHeaderV1 {
            table_id: self.table_id,
            segment_id: self.segment_id,
            row_start: self.row_start,
            row_count: self.row_count,
            morsel_count,
            morsel_row_count: self.morsel_row_count,
            column_count: self.column_count,
            morsel_directory_offset: TABLE_SEGMENT_HEADER_LEN as u64,
            column_directory_offset,
            page_index_offset: column_directory_offset,
            data_offset: column_directory_offset,
            flags: self.flags,
            checksum: 0,
        };
        let mut morsels = Vec::with_capacity(morsel_count as usize);
        let mut first_row = 0u32;
        for morsel_id in 0..morsel_count {
            let remaining = self.row_count - first_row;
            let row_count = remaining.min(self.morsel_row_count);
            morsels.push(RowMorselEntryV1 {
                morsel_id,
                first_row_in_segment: first_row,
                row_count,
                flags: 0,
                stats_ref: 0,
                checksum: 0,
            });
            first_row = first_row
                .checked_add(row_count)
                .ok_or(QfError::ArithOverflow)?;
        }
        let morsel_dir = RowMorselDirectory { entries: morsels };
        let mut out = Vec::with_capacity(TABLE_SEGMENT_HEADER_LEN + morsel_dir_len);
        out.extend_from_slice(&header.serialize());
        out.extend_from_slice(&morsel_dir.serialize());
        Ok(out)
    }

    fn index_entry(&self, offset: u64, length: u64) -> Result<TableSegmentIndexEntryV1, QfError> {
        Ok(TableSegmentIndexEntryV1 {
            table_id: self.table_id,
            segment_id: self.segment_id,
            row_start: self.row_start,
            row_count: self.row_count,
            morsel_count: self.morsel_count()?,
            morsel_row_count: self.morsel_row_count,
            column_count: self.column_count,
            offset,
            length,
            stats_ref: self.stats_ref,
            flags: self.flags,
            checksum: 0,
        })
    }
}

impl ScanProfileQfWriter {
    pub fn new(table_catalog: TableCatalog) -> Self {
        Self {
            created_at_us: 0,
            file_id: [0; 16],
            producer_scope_id: [0; 16],
            producer_scope_kind: 0,
            metadata_json: Vec::new(),
            table_catalog,
            segments: Vec::new(),
        }
    }

    pub fn push_segment(&mut self, segment: ScanSegment) {
        self.segments.push(segment);
    }

    pub fn write(&self) -> Result<Vec<u8>, QfError> {
        self.table_catalog.validate()?;
        self.validate_segments_against_catalog()?;

        let table_catalog_payload = self.table_catalog.serialize()?;
        let segment_index_len = 8usize
            .checked_add(
                self.segments
                    .len()
                    .checked_mul(TABLE_SEGMENT_INDEX_ENTRY_LEN)
                    .ok_or(QfError::ArithOverflow)?,
            )
            .ok_or(QfError::ArithOverflow)?;
        let segment_payloads = self
            .segments
            .iter()
            .map(ScanSegment::payload)
            .collect::<Result<Vec<_>, _>>()?;
        let mut offset = (HEADER_SIZE + table_catalog_payload.len() + segment_index_len) as u64;
        let mut index_entries = Vec::with_capacity(self.segments.len());
        for (segment, payload) in self.segments.iter().zip(segment_payloads.iter()) {
            let length = payload.len() as u64;
            index_entries.push(segment.index_entry(offset, length)?);
            offset = offset.checked_add(length).ok_or(QfError::ArithOverflow)?;
        }
        let segment_index = TableSegmentIndex {
            flags: 0,
            entries: index_entries,
        };
        segment_index.validate()?;
        let segment_index_payload = segment_index.serialize()?;

        let mut inner = MinimalQfWriter::new();
        inner.created_at_us = self.created_at_us;
        inner.file_id = self.file_id;
        inner.producer_scope_id = self.producer_scope_id;
        inner.producer_scope_kind = self.producer_scope_kind;
        inner.metadata_json = self.metadata_json.clone();
        inner.required_features = FEATURE_TABLE_PROFILE;
        inner.sections.push(SectionPayload {
            section_kind: SectionKind::TableCatalog as u16,
            profile: PrimaryProfile::TableScan as u8,
            flags: 0,
            item_count: self.table_catalog.tables.len() as u64,
            row_count: self.table_catalog.tables.iter().map(|t| t.row_count).sum(),
            compression: 0,
            alignment_log2: 0,
            required_features: 0,
            optional_features: 0,
            data: table_catalog_payload,
        });
        inner.sections.push(SectionPayload {
            section_kind: SectionKind::TableSegmentIndex as u16,
            profile: PrimaryProfile::TableScan as u8,
            flags: 0,
            item_count: self.segments.len() as u64,
            row_count: self.segments.iter().map(|s| s.row_count as u64).sum(),
            compression: 0,
            alignment_log2: 0,
            required_features: 0,
            optional_features: 0,
            data: segment_index_payload,
        });
        for (segment, payload) in self.segments.iter().zip(segment_payloads) {
            inner.sections.push(SectionPayload {
                section_kind: SectionKind::TableSegmentData as u16,
                profile: PrimaryProfile::TableScan as u8,
                flags: 0,
                item_count: 1,
                row_count: segment.row_count as u64,
                compression: 0,
                alignment_log2: 0,
                required_features: 0,
                optional_features: 0,
                data: payload,
            });
        }
        Ok(inner.write())
    }

    fn validate_segments_against_catalog(&self) -> Result<(), QfError> {
        use std::collections::BTreeMap;

        let mut tables = BTreeMap::new();
        for table in &self.table_catalog.tables {
            tables.insert(
                table.table_id,
                (table.row_count, table.columns.len() as u32),
            );
        }
        let mut rows_by_table: BTreeMap<u32, u64> = BTreeMap::new();
        for segment in &self.segments {
            let Some((_declared_rows, column_count)) = tables.get(&segment.table_id) else {
                return Err(QfError::BadSchema(format!(
                    "segment references unknown table_id {}",
                    segment.table_id
                )));
            };
            if segment.column_count != *column_count {
                return Err(QfError::BadSchema(format!(
                    "segment {} column_count {} does not match table {} column count {}",
                    segment.segment_id, segment.column_count, segment.table_id, column_count
                )));
            }
            *rows_by_table.entry(segment.table_id).or_default() += segment.row_count as u64;
        }
        for (table_id, (declared_rows, _column_count)) in tables {
            let segment_rows = rows_by_table.get(&table_id).copied().unwrap_or(0);
            if segment_rows != declared_rows {
                return Err(QfError::BadSchema(format!(
                    "table {} declares row_count {}, but segments cover {} rows",
                    table_id, declared_rows, segment_rows
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        constants::{QfLogicalType, QfPhysicalKind},
        footer::QfFooter,
        header::QfHeaderV1,
        postscript::QfPostscriptV1,
        reader::{validate_bytes_with_options, ValidationOptions},
        table::{ColumnEntry, TableEntry},
    };

    #[test]
    #[should_panic(expected = "metadata_json exceeds v1 limit")]
    fn write_rejects_oversized_metadata() {
        let mut w = MinimalQfWriter::new();
        w.metadata_json = vec![0u8; (METADATA_LEN_MAX as usize) + 1];
        let _ = w.write();
    }

    #[test]
    #[should_panic(expected = "metadata_json must be valid UTF-8")]
    fn write_rejects_invalid_metadata_utf8() {
        let mut w = MinimalQfWriter::new();
        w.metadata_json = vec![0xff];
        let _ = w.write();
    }

    #[test]
    #[should_panic(expected = "metadata_json must be syntactically valid JSON")]
    fn write_rejects_invalid_metadata_json() {
        let mut w = MinimalQfWriter::new();
        w.metadata_json = b"{not-json".to_vec();
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

    #[test]
    fn scan_profile_writer_emits_semantically_valid_table_file() {
        let catalog = TableCatalog {
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
                    logical: QfLogicalType::Bool,
                    physical: QfPhysicalKind::Boolean,
                    nullable: false,
                    sort_order: 0,
                    collation_id: 0,
                    precision: 0,
                    scale: 0,
                    flags: 0,
                }],
            }],
        };
        let mut writer = ScanProfileQfWriter::new(catalog);
        writer.push_segment(ScanSegment::new(1, 0, 0, 10, 1));
        let bytes = writer.write().unwrap();
        let report = validate_bytes_with_options(
            &bytes,
            ValidationOptions {
                semantic: true,
                verify_digests: false,
                allow_unknown_optional_extensions: true,
            },
        )
        .unwrap();
        assert_eq!(report.validated.footer.sections.len(), 3);

        let segment_index_entry = report
            .validated
            .footer
            .sections
            .iter()
            .find(|s| s.section_kind == SectionKind::TableSegmentIndex as u16)
            .unwrap();
        let segment_index_payload = &bytes[segment_index_entry.offset as usize
            ..segment_index_entry.end_offset().unwrap() as usize];
        let segment_index = TableSegmentIndex::parse(segment_index_payload).unwrap();
        let segment_data_entry = report
            .validated
            .footer
            .sections
            .iter()
            .find(|s| s.section_kind == SectionKind::TableSegmentData as u16)
            .unwrap();
        assert_eq!(segment_index.entries[0].offset, segment_data_entry.offset);
        assert_eq!(segment_index.entries[0].row_count, 10);
    }

    #[test]
    fn scan_profile_writer_rejects_row_count_mismatch() {
        let catalog = TableCatalog {
            flags: 0,
            tables: vec![TableEntry {
                table_id: 1,
                namespace: String::new(),
                name: "events".into(),
                row_count: 11,
                primary_sort_key_count: 0,
                clustering_key_count: 0,
                flags: 0,
                columns: vec![],
            }],
        };
        let mut writer = ScanProfileQfWriter::new(catalog);
        writer.push_segment(ScanSegment::new(1, 0, 0, 10, 0));
        assert!(matches!(writer.write(), Err(QfError::BadSchema(_))));
    }
}
