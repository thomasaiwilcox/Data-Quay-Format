//! Cove Format (COVE) v1.0 — Minimal reference writer.
//!
//! Produces a valid, structurally complete COVE file in memory.
//! The produced file satisfies the COVE-Core Minimal Profile (Section 71.1).
//!
//! # Example
//!
//! ```rust
//! use cove_core::writer::MinimalCoveWriter;
//!
//! let bytes = MinimalCoveWriter::write_empty_file();
//! assert!(bytes.len() > 128);
//! ```

use std::path::{Path, PathBuf};

use crate::{
    checksum, compression,
    constants::{
        CompressionCodec, CovePhysicalKind, PrimaryProfile, ProducerScopeKind, SectionKind,
        ENDIANNESS_LITTLE, FEATURE_CODEC_LZ4, FEATURE_CODEC_ZSTD, FEATURE_NESTED_COLUMNS,
        FEATURE_TABLE_PROFILE, FOOTER_VERSION_V1, HEADER_LEN_V1, KNOWN_FEATURE_BITS_MASK,
        MAGIC_COVE, MAGIC_COVE_FOOTER, METADATA_LEN_MAX, SECTION_ENTRY_LEN, VERSION_MAJOR_V1,
    },
    durable,
    footer::{CoveFooterHeaderV1, CoveSectionEntryV1, FOOTER_HEADER_SIZE},
    header::{CoveHeaderV1, HEADER_SIZE},
    metadata,
    page::ColumnPageIndexEntryV1,
    postscript::{CovePostscriptV1, CoveSectionSpecV1, POSTSCRIPT_SIZE},
    segment::{
        RowMorselDirectory, RowMorselEntryV1, TableColumnDirectoryEntryV1, TableSegmentHeaderV1,
        TableSegmentIndex, TableSegmentIndexEntryV1, ROW_MORSEL_ENTRY_LEN,
        TABLE_COLUMN_DIRECTORY_ENTRY_LEN, TABLE_SEGMENT_HEADER_LEN, TABLE_SEGMENT_INDEX_ENTRY_LEN,
    },
    table::{ColumnEntry, TableCatalog},
    CoveError,
};

/// A simple builder for minimal valid COVE files.
///
/// Produces files that conform to the COVE-Core Minimal Profile (Section 71.1):
/// - valid header,
/// - valid postscript,
/// - valid footer,
/// - binary section directory (possibly empty),
/// - valid checksums.
pub struct MinimalCoveWriter {
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

impl MinimalCoveWriter {
    /// Serialize and durably publish the file to `path` using Spec §74.
    pub fn publish_durable(&self, path: &Path) -> Result<PathBuf, CoveError> {
        durable::durable_replace(path, &self.write())
    }

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
    /// [Magic: "COV1"]
    /// ```
    pub fn write(&self) -> Vec<u8> {
        self.validate_inputs();

        let mut buf: Vec<u8> = Vec::new();

        // ── 1. Reserve space for header (filled in at the end) ─────────────
        buf.extend_from_slice(&[0u8; HEADER_SIZE]);

        // ── 2. Write section payloads and track their offsets ───────────────
        let mut section_entries: Vec<CoveSectionEntryV1> = Vec::new();
        for (idx, section) in self.sections.iter().enumerate() {
            let section_offset = buf.len() as u64;
            let section_data =
                compression::encode_payload_for_codec(&section.data, section.compression)
                    .unwrap_or_else(|err| panic!("section {} compression failed: {err}", idx + 1));
            let section_len = section_data.len() as u64;
            let section_uncompressed_len = section.data.len() as u64;
            let section_crc = checksum::crc32c(&section_data);

            buf.extend_from_slice(&section_data);

            section_entries.push(CoveSectionEntryV1 {
                section_id: (idx + 1) as u32,
                section_kind: section.section_kind,
                profile: section.profile,
                flags: section.flags,
                offset: section_offset,
                length: section_len,
                uncompressed_length: section_uncompressed_len,
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

        let footer_header = CoveFooterHeaderV1 {
            footer_magic: MAGIC_COVE_FOOTER,
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

        let postscript = CovePostscriptV1 {
            required_features: self.required_features,
            optional_features: self.optional_features,
            file_len: total_file_len,
            footer: CoveSectionSpecV1 {
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
        let header = CoveHeaderV1 {
            magic: MAGIC_COVE,
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

    /// Convenience wrapper: write an empty COVE-T file with no sections.
    pub fn write_empty_file() -> Vec<u8> {
        Self::new().write()
    }
}

impl Default for MinimalCoveWriter {
    fn default() -> Self {
        Self::new()
    }
}

/// COVE-T scan-profile writer surface (Spec §71.2/§71.3).
///
/// This builder emits a structurally valid table scan file with:
/// table catalog, table segment index, and table segment data sections.
/// It computes segment payload offsets before delegating to
/// [`MinimalCoveWriter`] so the segment index points at the actual bytes in
/// the produced file.
pub struct ScanProfileCoveWriter {
    pub created_at_us: i64,
    pub file_id: [u8; 16],
    pub producer_scope_id: [u8; 16],
    pub producer_scope_kind: u16,
    pub metadata_json: Vec<u8>,
    pub table_catalog: TableCatalog,
    pub segments: Vec<ScanSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanPageSpec {
    pub row_count: u32,
    pub non_null_count: u32,
    pub null_count: u32,
    pub encoding_root: u32,
    pub compression: CompressionCodec,
    pub stats_ref: u32,
    /// Uncompressed payload bytes; the writer applies `compression` on write.
    pub payload: Vec<u8>,
}

impl ScanPageSpec {
    pub fn new(row_count: u32, payload: Vec<u8>) -> Self {
        Self {
            row_count,
            non_null_count: row_count,
            null_count: 0,
            encoding_root: 0,
            compression: CompressionCodec::None,
            stats_ref: 0,
            payload,
        }
    }

    pub fn with_compression(mut self, compression: CompressionCodec) -> Self {
        self.compression = compression;
        self
    }

    pub fn with_counts(mut self, non_null_count: u32, null_count: u32) -> Self {
        self.non_null_count = non_null_count;
        self.null_count = null_count;
        self
    }

    pub fn with_encoding_root(mut self, encoding_root: u32) -> Self {
        self.encoding_root = encoding_root;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScanColumnPageSpec {
    pub column_id: u32,
    pub pages: Vec<ScanPageSpec>,
}

/// Segment declaration accepted by [`ScanProfileCoveWriter`].
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
    pub column_page_specs: Vec<ScanColumnPageSpec>,
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
            column_page_specs: Vec::new(),
        }
    }

    pub fn set_column_pages(&mut self, column_id: u32, pages: Vec<ScanPageSpec>) {
        if let Some(existing) = self
            .column_page_specs
            .iter_mut()
            .find(|spec| spec.column_id == column_id)
        {
            existing.pages = pages;
        } else {
            self.column_page_specs
                .push(ScanColumnPageSpec { column_id, pages });
        }
    }

    fn morsel_count(&self) -> Result<u32, CoveError> {
        if self.row_count == 0 {
            return Ok(0);
        }
        if self.morsel_row_count == 0 {
            return Err(CoveError::SegmentCorrupt);
        }
        let count = self
            .row_count
            .checked_add(self.morsel_row_count - 1)
            .ok_or(CoveError::ArithOverflow)?
            / self.morsel_row_count;
        Ok(count)
    }

    fn payload(&self, columns: &[ColumnEntry]) -> Result<Vec<u8>, CoveError> {
        let morsel_count = self.morsel_count()?;
        let morsel_dir_len = (morsel_count as usize)
            .checked_mul(ROW_MORSEL_ENTRY_LEN)
            .ok_or(CoveError::ArithOverflow)?;
        let column_dir_len = columns
            .len()
            .checked_mul(TABLE_COLUMN_DIRECTORY_ENTRY_LEN)
            .ok_or(CoveError::ArithOverflow)?;
        let per_column_page_index_len = (morsel_count as usize)
            .checked_mul(crate::page::COLUMN_PAGE_INDEX_ENTRY_LEN)
            .ok_or(CoveError::ArithOverflow)?;
        let total_page_index_len = columns
            .len()
            .checked_mul(per_column_page_index_len)
            .ok_or(CoveError::ArithOverflow)?;
        let column_directory_offset = TABLE_SEGMENT_HEADER_LEN
            .checked_add(morsel_dir_len)
            .ok_or(CoveError::ArithOverflow)? as u64;
        let page_index_offset = column_directory_offset
            .checked_add(column_dir_len as u64)
            .ok_or(CoveError::ArithOverflow)?;
        let data_offset = page_index_offset
            .checked_add(total_page_index_len as u64)
            .ok_or(CoveError::ArithOverflow)?;
        let header = TableSegmentHeaderV1 {
            table_id: self.table_id,
            segment_id: self.segment_id,
            row_start: self.row_start,
            row_count: self.row_count,
            morsel_count,
            morsel_row_count: self.morsel_row_count,
            column_count: columns.len() as u32,
            morsel_directory_offset: TABLE_SEGMENT_HEADER_LEN as u64,
            column_directory_offset,
            page_index_offset,
            data_offset,
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
                .ok_or(CoveError::ArithOverflow)?;
        }
        let morsel_dir = RowMorselDirectory { entries: morsels };
        let known_columns = columns
            .iter()
            .map(|column| column.column_id)
            .collect::<std::collections::BTreeSet<_>>();
        for spec in &self.column_page_specs {
            if !known_columns.contains(&spec.column_id) {
                return Err(CoveError::BadSchema(format!(
                    "segment {} page spec references unknown column_id {}",
                    self.segment_id, spec.column_id
                )));
            }
        }
        let mut column_directory = Vec::with_capacity(columns.len());
        let mut page_index_bytes = Vec::with_capacity(total_page_index_len);
        let mut page_payload_bytes = Vec::new();
        let mut next_page_index_offset = page_index_offset;
        let mut next_data_offset = data_offset;
        for column in columns {
            let column_page_index_offset = next_page_index_offset;
            let column_data_offset = next_data_offset;
            let mut column_page_count = 0usize;
            let custom_pages = self.page_specs_for_column(column.column_id)?;
            if let Some(custom_pages) = custom_pages {
                if custom_pages.len() != morsel_dir.entries.len() {
                    return Err(CoveError::BadSection(format!(
                        "segment {} column {} has {} page specs, expected {}",
                        self.segment_id,
                        column.column_id,
                        custom_pages.len(),
                        morsel_dir.entries.len()
                    )));
                }
                for (morsel, spec) in morsel_dir.entries.iter().zip(custom_pages.iter()) {
                    if spec.row_count != morsel.row_count {
                        return Err(CoveError::PageCorrupt);
                    }
                    if spec
                        .non_null_count
                        .checked_add(spec.null_count)
                        .ok_or(CoveError::ArithOverflow)?
                        != spec.row_count
                    {
                        return Err(CoveError::PageCorrupt);
                    }
                    if spec.payload.is_empty() && spec.compression != CompressionCodec::None {
                        return Err(CoveError::BadSection(
                            "compressed page payload must be non-empty".into(),
                        ));
                    }
                    let wire_payload =
                        compression::encode_page_payload(&spec.payload, spec.compression)?;
                    let page_length = wire_payload.len() as u64;
                    let page_offset = next_data_offset;
                    let page_checksum = checksum::crc32c(&wire_payload);
                    let page = ColumnPageIndexEntryV1 {
                        column_id: column.column_id,
                        morsel_id: morsel.morsel_id,
                        row_count: spec.row_count,
                        non_null_count: spec.non_null_count,
                        null_count: spec.null_count,
                        encoding_root: spec.encoding_root,
                        page_offset,
                        page_length,
                        uncompressed_length: spec.payload.len() as u64,
                        stats_ref: spec.stats_ref,
                        flags: spec.compression as u32,
                        checksum: page_checksum,
                    };
                    page_index_bytes.extend_from_slice(&page.serialize());
                    page_payload_bytes.extend_from_slice(&wire_payload);
                    next_data_offset = next_data_offset
                        .checked_add(page_length)
                        .ok_or(CoveError::ArithOverflow)?;
                    column_page_count += 1;
                }
            } else {
                if column_uses_nested_feature(column) {
                    return Err(CoveError::BadSection(format!(
                        "segment {} nested column {} requires explicit page specs",
                        self.segment_id, column.column_id
                    )));
                }
                for morsel in &morsel_dir.entries {
                    let page_length = u64::from(morsel.row_count != 0);
                    let page_offset = next_data_offset;
                    let dummy_payload = [(column.column_id & 0xFF) as u8];
                    let page_checksum = if page_length == 0 {
                        checksum::crc32c(&[])
                    } else {
                        checksum::crc32c(&dummy_payload)
                    };
                    let page = ColumnPageIndexEntryV1 {
                        column_id: column.column_id,
                        morsel_id: morsel.morsel_id,
                        row_count: morsel.row_count,
                        non_null_count: morsel.row_count,
                        null_count: 0,
                        encoding_root: 0,
                        page_offset,
                        page_length,
                        uncompressed_length: page_length,
                        stats_ref: 0,
                        flags: 0,
                        checksum: page_checksum,
                    };
                    page_index_bytes.extend_from_slice(&page.serialize());
                    if page_length != 0 {
                        page_payload_bytes.extend_from_slice(&dummy_payload);
                        next_data_offset = next_data_offset
                            .checked_add(page_length)
                            .ok_or(CoveError::ArithOverflow)?;
                    }
                    column_page_count += 1;
                }
            }
            let page_index_length = (column_page_count
                .checked_mul(crate::page::COLUMN_PAGE_INDEX_ENTRY_LEN)
                .ok_or(CoveError::ArithOverflow)?) as u64;
            next_page_index_offset = next_page_index_offset
                .checked_add(page_index_length)
                .ok_or(CoveError::ArithOverflow)?;
            column_directory.push(TableColumnDirectoryEntryV1 {
                column_id: column.column_id,
                logical_type: column.logical,
                physical_kind: column.physical,
                flags: 0,
                page_index_offset: column_page_index_offset,
                page_index_length,
                data_offset: column_data_offset,
                data_length: next_data_offset - column_data_offset,
                stats_ref: 0,
                domain_ref: 0,
                checksum: 0,
            });
        }
        let mut out = Vec::with_capacity(
            TABLE_SEGMENT_HEADER_LEN + morsel_dir_len + column_dir_len + total_page_index_len,
        );
        out.extend_from_slice(&header.serialize());
        out.extend_from_slice(&morsel_dir.serialize());
        for entry in &column_directory {
            out.extend_from_slice(&entry.serialize());
        }
        out.extend_from_slice(&page_index_bytes);
        out.extend_from_slice(&page_payload_bytes);
        Ok(out)
    }

    fn index_entry(&self, offset: u64, length: u64) -> Result<TableSegmentIndexEntryV1, CoveError> {
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

    fn page_specs_for_column(&self, column_id: u32) -> Result<Option<&[ScanPageSpec]>, CoveError> {
        let mut matches = self
            .column_page_specs
            .iter()
            .filter(|spec| spec.column_id == column_id);
        let first = matches.next();
        if matches.next().is_some() {
            return Err(CoveError::BadSection(format!(
                "segment {} defines duplicate page specs for column {}",
                self.segment_id, column_id
            )));
        }
        Ok(first.map(|spec| spec.pages.as_slice()))
    }

    fn page_codec_features(&self) -> u64 {
        self.column_page_specs
            .iter()
            .flat_map(|spec| spec.pages.iter())
            .fold(0u64, |bits, page| {
                bits | codec_feature_bit(page.compression)
            })
    }
}

fn codec_feature_bit(codec: CompressionCodec) -> u64 {
    match codec {
        CompressionCodec::None => 0,
        CompressionCodec::Lz4 => FEATURE_CODEC_LZ4,
        CompressionCodec::Zstd => FEATURE_CODEC_ZSTD,
    }
}

fn column_uses_nested_feature(column: &ColumnEntry) -> bool {
    matches!(
        column.physical,
        CovePhysicalKind::List | CovePhysicalKind::Struct | CovePhysicalKind::Map
    )
}

fn columns_feature_bits(columns: &[ColumnEntry]) -> u64 {
    columns.iter().fold(0u64, |bits, column| {
        bits | if column_uses_nested_feature(column) {
            FEATURE_NESTED_COLUMNS
        } else {
            0
        }
    })
}

impl ScanProfileCoveWriter {
    /// Serialize and durably publish the file to `path` using Spec §74.
    pub fn publish_durable(&self, path: &Path) -> Result<PathBuf, CoveError> {
        let bytes = self.write()?;
        durable::durable_replace(path, &bytes)
    }

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

    pub fn write(&self) -> Result<Vec<u8>, CoveError> {
        self.table_catalog.validate()?;
        self.validate_segments_against_catalog()?;

        let tables_by_id = self
            .table_catalog
            .tables
            .iter()
            .map(|table| (table.table_id, table))
            .collect::<std::collections::BTreeMap<_, _>>();

        let table_catalog_payload = self.table_catalog.serialize()?;
        let segment_index_len = 8usize
            .checked_add(
                self.segments
                    .len()
                    .checked_mul(TABLE_SEGMENT_INDEX_ENTRY_LEN)
                    .ok_or(CoveError::ArithOverflow)?,
            )
            .ok_or(CoveError::ArithOverflow)?;
        let segment_payloads = self
            .segments
            .iter()
            .map(|segment| {
                let table = tables_by_id.get(&segment.table_id).ok_or_else(|| {
                    CoveError::BadSchema(format!(
                        "segment references unknown table_id {}",
                        segment.table_id
                    ))
                })?;
                segment.payload(&table.columns)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut offset = (HEADER_SIZE + table_catalog_payload.len() + segment_index_len) as u64;
        let mut index_entries = Vec::with_capacity(self.segments.len());
        for (segment, payload) in self.segments.iter().zip(segment_payloads.iter()) {
            let length = payload.len() as u64;
            index_entries.push(segment.index_entry(offset, length)?);
            offset = offset.checked_add(length).ok_or(CoveError::ArithOverflow)?;
        }
        let segment_index = TableSegmentIndex {
            flags: 0,
            entries: index_entries,
        };
        segment_index.validate()?;
        let segment_index_payload = segment_index.serialize()?;
        let page_codec_features = self
            .segments
            .iter()
            .fold(0u64, |bits, segment| bits | segment.page_codec_features());
        let nested_column_features = self.table_catalog.tables.iter().fold(0u64, |bits, table| {
            bits | columns_feature_bits(&table.columns)
        });
        let table_nested_features = self
            .table_catalog
            .tables
            .iter()
            .map(|table| (table.table_id, columns_feature_bits(&table.columns)))
            .collect::<std::collections::BTreeMap<_, _>>();

        let mut inner = MinimalCoveWriter::new();
        inner.created_at_us = self.created_at_us;
        inner.file_id = self.file_id;
        inner.producer_scope_id = self.producer_scope_id;
        inner.producer_scope_kind = self.producer_scope_kind;
        inner.metadata_json = self.metadata_json.clone();
        inner.required_features = FEATURE_TABLE_PROFILE | nested_column_features;
        inner.optional_features = page_codec_features;
        inner.sections.push(SectionPayload {
            section_kind: SectionKind::TableCatalog as u16,
            profile: PrimaryProfile::TableScan as u8,
            flags: 0,
            item_count: self.table_catalog.tables.len() as u64,
            row_count: self.table_catalog.tables.iter().map(|t| t.row_count).sum(),
            compression: 0,
            alignment_log2: 0,
            required_features: nested_column_features,
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
                required_features: table_nested_features
                    .get(&segment.table_id)
                    .copied()
                    .unwrap_or(0),
                optional_features: segment.page_codec_features(),
                data: payload,
            });
        }
        Ok(inner.write())
    }

    fn validate_segments_against_catalog(&self) -> Result<(), CoveError> {
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
                return Err(CoveError::BadSchema(format!(
                    "segment references unknown table_id {}",
                    segment.table_id
                )));
            };
            if segment.column_count != *column_count {
                return Err(CoveError::BadSchema(format!(
                    "segment {} column_count {} does not match table {} column count {}",
                    segment.segment_id, segment.column_count, segment.table_id, column_count
                )));
            }
            *rows_by_table.entry(segment.table_id).or_default() += segment.row_count as u64;
        }
        for (table_id, (declared_rows, _column_count)) in tables {
            let segment_rows = rows_by_table.get(&table_id).copied().unwrap_or(0);
            if segment_rows != declared_rows {
                return Err(CoveError::BadSchema(format!(
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
        compression::column_page_payload,
        constants::{
            CoveLogicalType, CovePhysicalKind, FEATURE_CODEC_LZ4, FEATURE_CODEC_ZSTD,
            FEATURE_NESTED_COLUMNS,
        },
        encoding::local_codebook::{LocalCodebookPayload, LocalCodebookValues, LocalIndexPayload},
        encoding::nested::{ListLayout, ListLayoutPayload},
        footer::CoveFooter,
        header::CoveHeaderV1,
        page::{ColumnPageIndex, PAGE_FLAG_CODEC_MASK},
        postscript::CovePostscriptV1,
        reader::{validate_bytes_with_options, ValidationOptions},
        segment::TableSegmentPayloadV1,
        table::{ColumnEntry, TableEntry},
    };

    #[test]
    #[should_panic(expected = "metadata_json exceeds v1 limit")]
    fn write_rejects_oversized_metadata() {
        let mut w = MinimalCoveWriter::new();
        w.metadata_json = vec![0u8; (METADATA_LEN_MAX as usize) + 1];
        let _ = w.write();
    }

    #[test]
    #[should_panic(expected = "metadata_json must be valid UTF-8")]
    fn write_rejects_invalid_metadata_utf8() {
        let mut w = MinimalCoveWriter::new();
        w.metadata_json = vec![0xff];
        let _ = w.write();
    }

    #[test]
    #[should_panic(expected = "metadata_json must be syntactically valid JSON")]
    fn write_rejects_invalid_metadata_json() {
        let mut w = MinimalCoveWriter::new();
        w.metadata_json = b"{not-json".to_vec();
        let _ = w.write();
    }

    #[test]
    fn empty_file_is_valid() {
        let bytes = MinimalCoveWriter::write_empty_file();

        // Parse and validate header.
        let header = CoveHeaderV1::parse(&bytes).expect("header parse should succeed");
        assert_eq!(header.magic, MAGIC_COVE);
        assert_eq!(header.version_major, 1);
        assert_eq!(header.required_features, FEATURE_TABLE_PROFILE);

        // Parse and validate postscript.
        let ps =
            CovePostscriptV1::parse_from_tail(&bytes).expect("postscript parse should succeed");
        assert_eq!(ps.file_len, bytes.len() as u64);

        // Verify footer CRC.
        let footer_start = ps.footer.offset as usize;
        let footer_end = (ps.footer.offset + ps.footer.length) as usize;
        assert!(footer_end <= bytes.len(), "footer must be within file");
        let footer_bytes = &bytes[footer_start..footer_end];
        let computed_crc = checksum::crc32c(footer_bytes);
        assert_eq!(computed_crc, ps.footer.crc32c, "footer CRC must match");

        // Parse footer.
        let footer = CoveFooter::parse(footer_bytes).expect("footer parse should succeed");
        assert_eq!(footer.sections.len(), 0);
    }

    #[test]
    fn file_with_section_is_valid() {
        let mut writer = MinimalCoveWriter::new();
        let payload_data = b"hello, cove format".to_vec();
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

        let ps = CovePostscriptV1::parse_from_tail(&bytes).unwrap();
        assert_eq!(ps.file_len, bytes.len() as u64);

        let footer_bytes =
            &bytes[ps.footer.offset as usize..(ps.footer.offset + ps.footer.length) as usize];
        let footer = CoveFooter::parse(footer_bytes).unwrap();
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
    fn minimal_writer_can_publish_durably() {
        let dir = std::env::temp_dir().join(format!(
            "cove-writer-publish-{}-{}",
            std::process::id(),
            checksum::crc32c(b"minimal-writer-publish")
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("published.cove");
        let writer = MinimalCoveWriter::new();
        writer.publish_durable(&path).unwrap();
        assert!(crate::reader::validate_bytes(&std::fs::read(&path).unwrap()).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(feature = "compression-lz4")]
    #[test]
    fn file_with_lz4_section_round_trips_payload() {
        let mut writer = MinimalCoveWriter::new();
        let payload_data = b"hello, compressed cove format".to_vec();
        writer.optional_features = FEATURE_CODEC_LZ4 | crate::constants::FEATURE_FILE_DICTIONARY;
        writer.sections.push(SectionPayload {
            section_kind: crate::constants::SectionKind::FileDictionaryIndex as u16,
            profile: 0,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: CompressionCodec::Lz4 as u8,
            alignment_log2: 0,
            required_features: crate::constants::FEATURE_FILE_DICTIONARY,
            optional_features: FEATURE_CODEC_LZ4,
            data: payload_data.clone(),
        });
        writer.required_features =
            FEATURE_TABLE_PROFILE | crate::constants::FEATURE_FILE_DICTIONARY;

        let bytes = writer.write();
        let ps = CovePostscriptV1::parse_from_tail(&bytes).unwrap();
        let footer_bytes =
            &bytes[ps.footer.offset as usize..(ps.footer.offset + ps.footer.length) as usize];
        let footer = CoveFooter::parse(footer_bytes).unwrap();
        let entry = &footer.sections[0];
        let stored_bytes = &bytes[entry.offset as usize..entry.end_offset().unwrap() as usize];
        assert_ne!(stored_bytes, payload_data.as_slice());
        assert_eq!(entry.uncompressed_length, payload_data.len() as u64);
        let inflated = compression::section_payload(&bytes, entry).unwrap();
        assert_eq!(&*inflated, payload_data.as_slice());
        assert!(crate::reader::validate_bytes(&bytes).is_ok());
    }

    #[cfg(feature = "compression-zstd")]
    #[test]
    fn file_with_zstd_section_round_trips_payload() {
        let mut writer = MinimalCoveWriter::new();
        let payload_data = b"hello, zstd-compressed cove format".to_vec();
        writer.optional_features = FEATURE_CODEC_ZSTD | crate::constants::FEATURE_FILE_DICTIONARY;
        writer.sections.push(SectionPayload {
            section_kind: crate::constants::SectionKind::FileDictionaryIndex as u16,
            profile: 0,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: CompressionCodec::Zstd as u8,
            alignment_log2: 0,
            required_features: crate::constants::FEATURE_FILE_DICTIONARY,
            optional_features: FEATURE_CODEC_ZSTD,
            data: payload_data.clone(),
        });
        writer.required_features =
            FEATURE_TABLE_PROFILE | crate::constants::FEATURE_FILE_DICTIONARY;

        let bytes = writer.write();
        let ps = CovePostscriptV1::parse_from_tail(&bytes).unwrap();
        let footer_bytes =
            &bytes[ps.footer.offset as usize..(ps.footer.offset + ps.footer.length) as usize];
        let footer = CoveFooter::parse(footer_bytes).unwrap();
        let entry = &footer.sections[0];
        let stored_bytes = &bytes[entry.offset as usize..entry.end_offset().unwrap() as usize];
        assert_ne!(stored_bytes, payload_data.as_slice());
        assert_eq!(entry.uncompressed_length, payload_data.len() as u64);
        let inflated = compression::section_payload(&bytes, entry).unwrap();
        assert_eq!(&*inflated, payload_data.as_slice());
        assert!(crate::reader::validate_bytes(&bytes).is_ok());
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
        };
        let mut writer = ScanProfileCoveWriter::new(catalog);
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

    fn local_codebook_page_catalog() -> TableCatalog {
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
        }
    }

    fn assert_scan_writer_emits_compressed_local_codebook_page(
        codec: CompressionCodec,
        feature_bit: u64,
    ) {
        let local_codebook = LocalCodebookPayload {
            values: LocalCodebookValues::NumCode(vec![100, 200, 300]),
            indexes: LocalIndexPayload::BitPacked(
                crate::encoding::bit_packed::BitPackedPayload::pack(&[0, 1, 2, 1, 0, 2], 2)
                    .unwrap(),
            ),
        };
        let mut segment = ScanSegment::new(1, 0, 0, 6, 1);
        segment.set_column_pages(
            1,
            vec![ScanPageSpec::new(6, local_codebook.encode())
                .with_compression(codec)
                .with_encoding_root(17)],
        );

        let mut writer = ScanProfileCoveWriter::new(local_codebook_page_catalog());
        writer.push_segment(segment);
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
        assert_ne!(report.validated.header.optional_features & feature_bit, 0);

        let segment_data_entry = report
            .validated
            .footer
            .sections
            .iter()
            .find(|s| s.section_kind == SectionKind::TableSegmentData as u16)
            .unwrap();
        assert_ne!(segment_data_entry.optional_features & feature_bit, 0);

        let segment_bytes = &bytes
            [segment_data_entry.offset as usize..segment_data_entry.end_offset().unwrap() as usize];
        let payload = TableSegmentPayloadV1::parse(segment_bytes).unwrap();
        let column = &payload.columns[0];
        let page_index_bytes = &segment_bytes[column.page_index_offset as usize
            ..(column.page_index_offset + column.page_index_length) as usize];
        let page_index = ColumnPageIndex::parse(page_index_bytes).unwrap();
        let page = &page_index.entries[0];
        assert_eq!(page.flags & PAGE_FLAG_CODEC_MASK, codec as u32);
        assert_eq!(
            page.uncompressed_length as usize,
            local_codebook.encode().len()
        );

        let page_wire = &segment_bytes
            [page.page_offset as usize..(page.page_offset + page.page_length) as usize];
        let decoded = column_page_payload(page_wire, page).unwrap();
        assert_eq!(
            LocalCodebookPayload::parse(&decoded).unwrap(),
            local_codebook
        );
    }

    #[cfg(feature = "compression-lz4")]
    #[test]
    fn scan_profile_writer_emits_lz4_local_codebook_page() {
        assert_scan_writer_emits_compressed_local_codebook_page(
            CompressionCodec::Lz4,
            FEATURE_CODEC_LZ4,
        );
    }

    #[cfg(feature = "compression-zstd")]
    #[test]
    fn scan_profile_writer_emits_zstd_local_codebook_page() {
        assert_scan_writer_emits_compressed_local_codebook_page(
            CompressionCodec::Zstd,
            FEATURE_CODEC_ZSTD,
        );
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
        let mut writer = ScanProfileCoveWriter::new(catalog);
        writer.push_segment(ScanSegment::new(1, 0, 0, 10, 0));
        assert!(matches!(writer.write(), Err(CoveError::BadSchema(_))));
    }

    #[test]
    fn scan_profile_writer_emits_nested_list_page_and_feature_bit() {
        let catalog = TableCatalog {
            flags: 0,
            tables: vec![TableEntry {
                table_id: 1,
                namespace: "public".into(),
                name: "events".into(),
                row_count: 3,
                primary_sort_key_count: 0,
                clustering_key_count: 0,
                flags: 0,
                columns: vec![ColumnEntry {
                    column_id: 1,
                    name: "tags".into(),
                    logical: CoveLogicalType::List,
                    physical: CovePhysicalKind::List,
                    nullable: false,
                    sort_order: 0,
                    collation_id: 0,
                    precision: 0,
                    scale: 0,
                    flags: 0,
                }],
            }],
        };
        let payload = ListLayoutPayload {
            layout: ListLayout {
                offsets: vec![0, 2, 2, 5],
            },
            child_row_count: 5,
        };
        let mut segment = ScanSegment::new(1, 0, 0, 3, 1);
        segment.set_column_pages(1, vec![ScanPageSpec::new(3, payload.encode())]);
        let mut writer = ScanProfileCoveWriter::new(catalog);
        writer.push_segment(segment);

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
        assert_ne!(
            report.validated.header.required_features & FEATURE_NESTED_COLUMNS,
            0
        );

        let segment_data_entry = report
            .validated
            .footer
            .sections
            .iter()
            .find(|s| s.section_kind == SectionKind::TableSegmentData as u16)
            .unwrap();
        assert_ne!(
            segment_data_entry.required_features & FEATURE_NESTED_COLUMNS,
            0
        );

        let segment_bytes = &bytes
            [segment_data_entry.offset as usize..segment_data_entry.end_offset().unwrap() as usize];
        let parsed = TableSegmentPayloadV1::parse(segment_bytes).unwrap();
        let column = &parsed.columns[0];
        let page_index_bytes = &segment_bytes[column.page_index_offset as usize
            ..(column.page_index_offset + column.page_index_length) as usize];
        let page_index = ColumnPageIndex::parse(page_index_bytes).unwrap();
        let page = &page_index.entries[0];
        let page_wire = &segment_bytes
            [page.page_offset as usize..(page.page_offset + page.page_length) as usize];
        let decoded = column_page_payload(page_wire, page).unwrap();
        assert_eq!(ListLayoutPayload::parse(&decoded).unwrap(), payload);
    }

    #[test]
    fn scan_profile_writer_rejects_nested_column_without_page_specs() {
        let catalog = TableCatalog {
            flags: 0,
            tables: vec![TableEntry {
                table_id: 1,
                namespace: "public".into(),
                name: "events".into(),
                row_count: 3,
                primary_sort_key_count: 0,
                clustering_key_count: 0,
                flags: 0,
                columns: vec![ColumnEntry {
                    column_id: 1,
                    name: "tags".into(),
                    logical: CoveLogicalType::List,
                    physical: CovePhysicalKind::List,
                    nullable: false,
                    sort_order: 0,
                    collation_id: 0,
                    precision: 0,
                    scale: 0,
                    flags: 0,
                }],
            }],
        };
        let mut writer = ScanProfileCoveWriter::new(catalog);
        writer.push_segment(ScanSegment::new(1, 0, 0, 3, 1));
        assert!(matches!(writer.write(), Err(CoveError::BadSection(_))));
    }
}
