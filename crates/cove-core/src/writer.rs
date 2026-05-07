//! Cove Format (COVE) v1.0 — Minimal reference writer.
//!
//! Produces a valid, structurally complete COVE file.
//! The produced file satisfies the COVE-Core Minimal Profile (Section 72.1).
//!
//! `write()` is a convenience wrapper that buffers the complete file in memory.
//! `write_to()` streams the file to a `Write + Seek` target, and durable
//! publication uses that path so it does not require a second full-file buffer;
//! transient allocation is bounded by the largest section or encoded page that
//! must be compressed before its footer entry can be written.
//!
//! # Example
//!
//! ```rust
//! use cove_core::writer::MinimalCoveWriter;
//!
//! let bytes = MinimalCoveWriter::write_empty_file().unwrap();
//! assert!(bytes.len() > 128);
//! ```

use std::{
    io::{Cursor, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use crate::{
    checksum, compression,
    constants::{
        CompressionCodec, CoveEncodingKind, CoveLogicalType, CovePhysicalKind, PrimaryProfile,
        ProducerScopeKind, SectionKind, ENDIANNESS_LITTLE, FEATURE_AGGREGATE_SYNOPSES,
        FEATURE_ARCHIVE_PROFILE, FEATURE_BLOOM_FILTERS, FEATURE_CODEC_LZ4, FEATURE_CODEC_ZSTD,
        FEATURE_COLUMN_DOMAINS, FEATURE_COMPOSITE_ZONES, FEATURE_ENGINE_PROFILE,
        FEATURE_EXACT_SETS, FEATURE_FILE_DICTIONARY, FEATURE_HARBOR_PROFILE,
        FEATURE_INVERTED_INDEXES, FEATURE_LOOKUP_INDEXES, FEATURE_NESTED_COLUMNS,
        FEATURE_OBJECT_PROFILE, FEATURE_PAGE_PAYLOAD_ELISION, FEATURE_SEMANTIC_MAP,
        FEATURE_TABLE_PROFILE, FEATURE_TOPN_SUMMARIES, FOOTER_VERSION_V1, HEADER_LEN_V1,
        KNOWN_FEATURE_BITS_MASK, MAGIC_COVE, MAGIC_COVE_FOOTER, SECTION_ENTRY_LEN,
        VERSION_MAJOR_V1,
    },
    dictionary::FileDictionary,
    domain::ColumnDomain,
    durable,
    footer::{CoveFooterHeaderV1, CoveSectionEntryV1, FOOTER_HEADER_SIZE},
    header::{CoveHeaderV1, HEADER_SIZE},
    index::{
        aggregate::AggregateSynopsis, bloom::BloomFilterIndex, composite::CompositeIndex,
        exact_set::ExactSetIndex, inverted::InvertedMorselIndex, lookup::LookupIndex,
        topn::TopNSummary,
    },
    metadata,
    page::{
        page_uses_payload_elision, ColumnPageIndexEntryV1, PAGE_FLAG_CODEC_MASK,
        PAGE_FLAG_STATS_ONLY_CONSTANT,
    },
    page_payload::ColumnPagePayloadV1,
    postscript::{CovePostscriptV1, CoveSectionSpecV1, POSTSCRIPT_SIZE},
    segment::{
        RowMorselDirectory, RowMorselEntryV1, TableColumnDirectoryEntryV1, TableSegmentHeaderV1,
        TableSegmentIndex, TableSegmentIndexEntryV1, ROW_MORSEL_ENTRY_LEN,
        SEGMENT_COLUMN_FLAG_BOOL_DECLARED_NUMERIC, TABLE_COLUMN_DIRECTORY_ENTRY_LEN,
        TABLE_SEGMENT_HEADER_LEN, TABLE_SEGMENT_INDEX_ENTRY_LEN,
    },
    table::{ColumnEntry, TableCatalog, COLUMN_FLAG_BOOL_DECLARED_NUMERIC},
    zone_stats::ZoneStatsSection,
    CoveError,
};

/// A simple builder for minimal valid COVE files.
///
/// Produces files that conform to the COVE-Core Minimal Profile (Section 72.1):
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Serialize and durably publish the file to `path` using Spec §75.
    pub fn publish_durable(&self, path: &Path) -> Result<PathBuf, CoveError> {
        durable::durable_replace_with_writer(path, |file| self.write_to(file))
    }

    /// Validate builder inputs that have strict on-disk bounds in v1.
    fn validate_inputs(&self) -> Result<(), CoveError> {
        metadata::validate(&self.metadata_json)?;
        if self.sections.len() > u32::MAX as usize {
            return Err(CoveError::ArithOverflow);
        }
        if PrimaryProfile::from_u8(self.primary_profile).is_none() {
            return Err(CoveError::BadSection(format!(
                "unknown primary_profile {}",
                self.primary_profile
            )));
        }
        if ProducerScopeKind::from_u16(self.producer_scope_kind).is_none() {
            return Err(CoveError::BadSection(format!(
                "unknown producer_scope_kind {}",
                self.producer_scope_kind
            )));
        }
        if self.required_features & !KNOWN_FEATURE_BITS_MASK != 0 {
            return Err(CoveError::UnknownRequiredFeature(
                self.required_features & !KNOWN_FEATURE_BITS_MASK,
            ));
        }
        for section in &self.sections {
            if SectionKind::from_u16(section.section_kind).is_none() {
                return Err(CoveError::BadSection(format!(
                    "unknown section_kind {}",
                    section.section_kind
                )));
            }
            if PrimaryProfile::from_u8(section.profile).is_none() {
                return Err(CoveError::BadSection(format!(
                    "unknown section profile {}",
                    section.profile
                )));
            }
            if CompressionCodec::from_u8(section.compression).is_none() {
                return Err(CoveError::BadSection(format!(
                    "unknown compression codec {}",
                    section.compression
                )));
            }
            if section.required_features & !KNOWN_FEATURE_BITS_MASK != 0 {
                return Err(CoveError::UnknownRequiredFeature(
                    section.required_features & !KNOWN_FEATURE_BITS_MASK,
                ));
            }
        }
        Ok(())
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

    /// Stream the file to `writer`.
    ///
    /// The writer must be positioned at byte 0 for a new, truncated output
    /// target. COVE offsets are absolute from the start of the file.
    pub fn write_to<W: Write + Seek>(&self, writer: &mut W) -> Result<(), CoveError> {
        self.validate_inputs()?;
        if writer.stream_position()? != 0 {
            return Err(CoveError::BadSection(
                "MinimalCoveWriter::write_to requires a writer positioned at byte 0".into(),
            ));
        }

        writer.write_all(&[0u8; HEADER_SIZE])?;

        let mut section_entries: Vec<CoveSectionEntryV1> = Vec::new();
        for (idx, section) in self.sections.iter().enumerate() {
            let section_offset = writer.stream_position()?;
            let section_data =
                compression::encode_payload_for_codec(&section.data, section.compression)?;
            let section_len =
                u64::try_from(section_data.len()).map_err(|_| CoveError::ArithOverflow)?;
            let section_uncompressed_len =
                u64::try_from(section.data.len()).map_err(|_| CoveError::ArithOverflow)?;
            let section_crc = checksum::crc32c(&section_data);

            writer.write_all(&section_data)?;

            section_entries.push(CoveSectionEntryV1 {
                section_id: u32::try_from(idx + 1).map_err(|_| CoveError::ArithOverflow)?,
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

        let footer_offset = writer.stream_position()?;
        let section_count =
            u32::try_from(section_entries.len()).map_err(|_| CoveError::ArithOverflow)?;
        let metadata_len =
            u32::try_from(self.metadata_json.len()).map_err(|_| CoveError::ArithOverflow)?;

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
        let mut footer_bytes = Vec::with_capacity(
            FOOTER_HEADER_SIZE
                + section_entries.len() * usize::from(SECTION_ENTRY_LEN)
                + self.metadata_json.len(),
        );
        footer_bytes.extend_from_slice(&footer_header.serialize());
        for entry in &section_entries {
            footer_bytes.extend_from_slice(&entry.serialize());
        }
        footer_bytes.extend_from_slice(&self.metadata_json);
        let footer_len = u64::try_from(footer_bytes.len()).map_err(|_| CoveError::ArithOverflow)?;
        let footer_crc = checksum::crc32c(&footer_bytes);
        writer.write_all(&footer_bytes)?;

        // file_len includes the entire postscript tail (payload + version + len + magic).
        let file_len_before_tail = writer.stream_position()?;
        let total_file_len = file_len_before_tail
            .checked_add(POSTSCRIPT_SIZE as u64)
            .and_then(|len| len.checked_add(2 + 2 + 4))
            .ok_or(CoveError::ArithOverflow)?;

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
            checksum: 0,
        };
        writer.write_all(&postscript.serialize_tail())?;

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
            checksum: 0,
        };
        let header_bytes = header.serialize();
        // INVARIANT: the header checksum covers final feature bits and IDs, and
        // the placeholder may be replaced only after every offset and file_len
        // has been computed from bytes already written to the stream.
        writer.seek(SeekFrom::Start(0))?;
        writer.write_all(&header_bytes)?;
        writer.seek(SeekFrom::Start(total_file_len))?;
        Ok(())
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
    pub fn write(&self) -> Result<Vec<u8>, CoveError> {
        let mut cursor = Cursor::new(Vec::new());
        self.write_to(&mut cursor)?;
        Ok(cursor.into_inner())
    }

    /// Convenience wrapper: write an empty COVE-T file with no sections.
    pub fn write_empty_file() -> Result<Vec<u8>, CoveError> {
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
    /// Optional shared/profile sections inserted after `TABLE_CATALOG` and
    /// before `TABLE_SEGMENT_INDEX`. Segment offsets are computed after these
    /// payloads are accounted for.
    pub extra_sections: Vec<SectionPayload>,
    pub segments: Vec<ScanSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanPageSpec {
    pub row_count: u32,
    pub non_null_count: u32,
    pub null_count: u32,
    pub encoding_root: u32,
    pub compression: CompressionCodec,
    pub flags: u32,
    pub stats_ref: u32,
    /// Logical value payload bytes; the writer wraps them in the §27.3
    /// self-describing page container before applying `compression`.
    ///
    /// When `null_count > 0`, this buffer MUST start with the packed COVE null
    /// bitmap for exactly `row_count` rows (`1` bit means null, LSB-first).
    /// Unused bits in the final bitmap byte MUST be zero and the number of set
    /// bits MUST equal `null_count`; all remaining bytes are the non-null value
    /// stream for `encoding_root`.
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
            flags: 0,
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

    pub fn with_flags(mut self, flags: u32) -> Self {
        self.flags = flags;
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
                    if spec.flags & PAGE_FLAG_CODEC_MASK != 0 {
                        return Err(CoveError::BadSection(
                            "ScanPageSpec flags must not set codec bits directly".into(),
                        ));
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
                    let stats_only_constant = spec.flags & PAGE_FLAG_STATS_ONLY_CONSTANT != 0;
                    if stats_only_constant {
                        if !spec.payload.is_empty() {
                            return Err(CoveError::BadSection(
                                "stats-only constant page specs must use an empty payload".into(),
                            ));
                        }
                        if spec.compression != CompressionCodec::None {
                            return Err(CoveError::BadSection(
                                "stats-only constant page specs must use compression=None".into(),
                            ));
                        }
                        if spec.encoding_root != u32::MAX {
                            return Err(CoveError::BadSection(
                                "stats-only constant page specs must use encoding_root=u32::MAX"
                                    .into(),
                            ));
                        }
                    } else if spec.payload.is_empty() {
                        return Err(CoveError::BadSection(
                            "empty page payload requires PAGE_FLAG_STATS_ONLY_CONSTANT".into(),
                        ));
                    }
                    let encoded_payload = if stats_only_constant {
                        Vec::new()
                    } else {
                        encode_scan_page_payload(column, spec)?
                    };
                    let wire_payload =
                        compression::encode_page_payload(&encoded_payload, spec.compression)?;
                    let page_length = wire_payload.len() as u64;
                    let page_offset = if stats_only_constant {
                        0
                    } else {
                        next_data_offset
                    };
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
                        uncompressed_length: encoded_payload.len() as u64,
                        stats_ref: spec.stats_ref,
                        flags: spec.flags | spec.compression as u32,
                        checksum: page_checksum,
                    };
                    page_index_bytes.extend_from_slice(&page.serialize());
                    if page_length != 0 {
                        page_payload_bytes.extend_from_slice(&wire_payload);
                        next_data_offset = next_data_offset
                            .checked_add(page_length)
                            .ok_or(CoveError::ArithOverflow)?;
                    }
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
                    let payload = default_page_payload(column, morsel.row_count)?;
                    let page_length = payload.len() as u64;
                    let page_offset = next_data_offset;
                    let page_checksum = checksum::crc32c(&payload);
                    let page = ColumnPageIndexEntryV1 {
                        column_id: column.column_id,
                        morsel_id: morsel.morsel_id,
                        row_count: morsel.row_count,
                        non_null_count: morsel.row_count,
                        null_count: 0,
                        encoding_root: default_encoding_kind(column) as u32,
                        page_offset,
                        page_length,
                        uncompressed_length: page_length,
                        stats_ref: 0,
                        flags: 0,
                        checksum: page_checksum,
                    };
                    page_index_bytes.extend_from_slice(&page.serialize());
                    if page_length != 0 {
                        page_payload_bytes.extend_from_slice(&payload);
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
                flags: segment_column_flags(column),
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

    fn page_required_features(&self) -> u64 {
        self.column_page_specs
            .iter()
            .flat_map(|spec| spec.pages.iter())
            .fold(0u64, |bits, page| {
                bits | if page_uses_payload_elision(page.flags) {
                    FEATURE_PAGE_PAYLOAD_ELISION
                } else {
                    0
                }
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

fn encode_scan_page_payload(
    column: &ColumnEntry,
    spec: &ScanPageSpec,
) -> Result<Vec<u8>, CoveError> {
    let encoding_raw = u16::try_from(spec.encoding_root).map_err(|_| {
        CoveError::UnsupportedEncoding(format!(
            "encoding_root {} does not fit a v1 encoding kind",
            spec.encoding_root
        ))
    })?;
    let encoding_kind = CoveEncodingKind::from_u16(encoding_raw).ok_or_else(|| {
        CoveError::UnsupportedEncoding(format!("unknown page encoding kind {encoding_raw}"))
    })?;
    let (null_bitmap, values) = if spec.null_count == 0 {
        (None, spec.payload.clone())
    } else {
        let validity_len = (spec.row_count as usize)
            .checked_add(7)
            .ok_or(CoveError::ArithOverflow)?
            / 8;
        if spec.payload.len() < validity_len {
            return Err(CoveError::PageCorrupt);
        }
        let bitmap = &spec.payload[..validity_len];
        // INVARIANT: a writer-created non-elided mixed/null page must carry an
        // exact §27 null bitmap prefix; counts and tail bits are part of the
        // decode contract, not optional metadata.
        if spec.row_count % 8 != 0 && validity_len != 0 {
            let valid_bits = spec.row_count % 8;
            let mask = (1u8 << valid_bits) - 1;
            if bitmap[validity_len - 1] & !mask != 0 {
                return Err(CoveError::PageCorrupt);
            }
        }
        let counted = bitmap.iter().try_fold(0u32, |acc, byte| {
            acc.checked_add(byte.count_ones())
                .ok_or(CoveError::ArithOverflow)
        })?;
        if counted != spec.null_count {
            return Err(CoveError::PageCorrupt);
        }
        (Some(bitmap.to_vec()), spec.payload[validity_len..].to_vec())
    };
    ColumnPagePayloadV1::build_single_node(
        spec.row_count,
        encoding_kind,
        column.logical,
        column.physical,
        null_bitmap,
        values,
    )
}

fn default_encoding_kind(column: &ColumnEntry) -> CoveEncodingKind {
    match column.physical {
        CovePhysicalKind::FileCode => CoveEncodingKind::FileCode,
        CovePhysicalKind::NumCode => CoveEncodingKind::NumCode,
        CovePhysicalKind::Boolean | CovePhysicalKind::FixedBytes => CoveEncodingKind::PlainFixed,
        CovePhysicalKind::VarBytes => CoveEncodingKind::VarBytes,
        CovePhysicalKind::List | CovePhysicalKind::Struct | CovePhysicalKind::Map => {
            CoveEncodingKind::Canonical
        }
    }
}

fn segment_column_flags(column: &ColumnEntry) -> u8 {
    if column.flags & COLUMN_FLAG_BOOL_DECLARED_NUMERIC != 0 {
        SEGMENT_COLUMN_FLAG_BOOL_DECLARED_NUMERIC
    } else {
        0
    }
}

fn default_page_payload(column: &ColumnEntry, row_count: u32) -> Result<Vec<u8>, CoveError> {
    let values = default_physical_payload(column, row_count)?;
    ColumnPagePayloadV1::build_single_node(
        row_count,
        default_encoding_kind(column),
        column.logical,
        column.physical,
        None,
        values,
    )
}

fn default_physical_payload(column: &ColumnEntry, row_count: u32) -> Result<Vec<u8>, CoveError> {
    let rows = row_count as usize;
    match column.physical {
        CovePhysicalKind::Boolean => Ok(vec![0u8; rows]),
        CovePhysicalKind::FileCode => rows
            .checked_mul(4)
            .map(|len| vec![0u8; len])
            .ok_or(CoveError::ArithOverflow),
        CovePhysicalKind::NumCode => rows
            .checked_mul(8)
            .map(|len| vec![0u8; len])
            .ok_or(CoveError::ArithOverflow),
        CovePhysicalKind::FixedBytes => {
            let width = match column.logical {
                CoveLogicalType::Decimal64 => 8,
                CoveLogicalType::Decimal128 | CoveLogicalType::Uuid => 16,
                _ => 0,
            };
            rows.checked_mul(width)
                .map(|len| vec![0u8; len])
                .ok_or(CoveError::ArithOverflow)
        }
        CovePhysicalKind::VarBytes => {
            let len = rows.checked_mul(4).ok_or(CoveError::ArithOverflow)?;
            Ok(vec![0u8; len])
        }
        CovePhysicalKind::List | CovePhysicalKind::Struct | CovePhysicalKind::Map => Err(
            CoveError::BadSection("nested columns require explicit page payloads".into()),
        ),
    }
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

fn nested_column_features_for_catalog(catalog: &TableCatalog) -> u64 {
    catalog.tables.iter().fold(0u64, |bits, table| {
        bits | columns_feature_bits(&table.columns)
    })
}

fn section_kind_feature_bits(section_kind: u16) -> u64 {
    match SectionKind::from_u16(section_kind) {
        Some(SectionKind::FileDictionaryIndex | SectionKind::FileDictionaryPayload) => {
            FEATURE_FILE_DICTIONARY
        }
        Some(SectionKind::ColumnDomain) => FEATURE_COLUMN_DOMAINS,
        Some(SectionKind::ExactSetIndex) => FEATURE_EXACT_SETS,
        Some(SectionKind::BloomIndex) => FEATURE_BLOOM_FILTERS,
        Some(SectionKind::InvertedMorselIndex) => FEATURE_INVERTED_INDEXES,
        Some(SectionKind::LookupIndex) => FEATURE_LOOKUP_INDEXES,
        Some(SectionKind::AggregateSynopsis) => FEATURE_AGGREGATE_SYNOPSES,
        Some(SectionKind::CompositeZoneIndex) => FEATURE_COMPOSITE_ZONES,
        Some(SectionKind::TopNZoneSummary) => FEATURE_TOPN_SUMMARIES,
        _ => 0,
    }
}

fn profile_feature_bit(profile: u8) -> u64 {
    match PrimaryProfile::from_u8(profile) {
        Some(PrimaryProfile::Mixed) | None => 0,
        Some(PrimaryProfile::ObjectTemporal) => FEATURE_OBJECT_PROFILE,
        Some(PrimaryProfile::TableScan) => FEATURE_TABLE_PROFILE,
        Some(PrimaryProfile::ArchiveAcceleration) => FEATURE_ARCHIVE_PROFILE,
        Some(PrimaryProfile::EngineExecution) => FEATURE_ENGINE_PROFILE,
        Some(PrimaryProfile::HarborExecution) => FEATURE_HARBOR_PROFILE,
        Some(PrimaryProfile::SemanticMapping) => FEATURE_SEMANTIC_MAP,
    }
}

fn section_encoded_len(section: &SectionPayload) -> Result<usize, CoveError> {
    compression::encode_payload_for_codec(&section.data, section.compression)
        .map(|bytes| bytes.len())
}

impl ScanProfileCoveWriter {
    /// Serialize and durably publish the file to `path` using Spec §75.
    pub fn publish_durable(&self, path: &Path) -> Result<PathBuf, CoveError> {
        durable::durable_replace_with_writer(path, |file| self.write_to(file))
    }

    pub fn new(table_catalog: TableCatalog) -> Self {
        Self {
            created_at_us: 0,
            file_id: [0; 16],
            producer_scope_id: [0; 16],
            producer_scope_kind: 0,
            metadata_json: Vec::new(),
            table_catalog,
            extra_sections: Vec::new(),
            segments: Vec::new(),
        }
    }

    pub fn push_segment(&mut self, segment: ScanSegment) {
        self.segments.push(segment);
    }

    pub fn push_extra_section(&mut self, section: SectionPayload) {
        self.extra_sections.push(section);
    }

    pub fn push_file_dictionary(&mut self, dictionary: &FileDictionary) {
        let mut index = Vec::with_capacity(
            crate::dictionary::DICT_HEADER_SIZE
                + dictionary.entries.len() * crate::dictionary::DICT_INDEX_ENTRY_SIZE,
        );
        index.extend_from_slice(&dictionary.header.serialize());
        for entry in &dictionary.entries {
            index.extend_from_slice(&entry.serialize());
        }
        self.extra_sections.push(SectionPayload {
            section_kind: SectionKind::FileDictionaryIndex as u16,
            profile: PrimaryProfile::Mixed as u8,
            flags: 0,
            item_count: dictionary.len() as u64,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: FEATURE_FILE_DICTIONARY,
            optional_features: 0,
            data: index,
        });
        if !dictionary.payload.is_empty() {
            self.extra_sections.push(SectionPayload {
                section_kind: SectionKind::FileDictionaryPayload as u16,
                profile: PrimaryProfile::Mixed as u8,
                flags: 0,
                item_count: 1,
                row_count: 0,
                compression: 0,
                alignment_log2: 0,
                required_features: FEATURE_FILE_DICTIONARY,
                optional_features: 0,
                data: dictionary.payload.clone(),
            });
        }
    }

    pub fn push_column_domain(&mut self, domain: &ColumnDomain) -> Result<(), CoveError> {
        self.extra_sections.push(SectionPayload {
            section_kind: SectionKind::ColumnDomain as u16,
            profile: PrimaryProfile::TableScan as u8,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: 0,
            optional_features: FEATURE_COLUMN_DOMAINS,
            data: domain.serialize()?,
        });
        Ok(())
    }

    pub fn push_zone_stats(&mut self, zone_stats: &ZoneStatsSection) -> Result<(), CoveError> {
        self.extra_sections.push(SectionPayload {
            section_kind: SectionKind::ZoneStats as u16,
            profile: PrimaryProfile::TableScan as u8,
            flags: 0,
            item_count: zone_stats.entries.len() as u64,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: 0,
            optional_features: 0,
            data: zone_stats.serialize()?,
        });
        Ok(())
    }

    pub fn push_exact_set_index(&mut self, index: &ExactSetIndex) {
        self.push_serialized_scan_artifact(
            SectionKind::ExactSetIndex,
            PrimaryProfile::TableScan,
            FEATURE_EXACT_SETS,
            index.serialize(),
        );
    }

    pub fn push_bloom_index(&mut self, index: &BloomFilterIndex) {
        self.push_serialized_scan_artifact(
            SectionKind::BloomIndex,
            PrimaryProfile::TableScan,
            FEATURE_BLOOM_FILTERS,
            index.serialize(),
        );
    }

    pub fn push_inverted_morsel_index(&mut self, index: &InvertedMorselIndex) {
        self.push_serialized_scan_artifact(
            SectionKind::InvertedMorselIndex,
            PrimaryProfile::TableScan,
            FEATURE_INVERTED_INDEXES,
            index.serialize(),
        );
    }

    pub fn push_lookup_index(&mut self, index: &LookupIndex) -> Result<(), CoveError> {
        self.push_serialized_scan_artifact(
            SectionKind::LookupIndex,
            PrimaryProfile::ArchiveAcceleration,
            FEATURE_LOOKUP_INDEXES,
            index.serialize()?,
        );
        Ok(())
    }

    pub fn push_aggregate_synopsis(&mut self, synopsis: &AggregateSynopsis) {
        self.push_serialized_scan_artifact(
            SectionKind::AggregateSynopsis,
            PrimaryProfile::ArchiveAcceleration,
            FEATURE_AGGREGATE_SYNOPSES,
            synopsis.serialize(),
        );
    }

    pub fn push_composite_zone_index(&mut self, index: &CompositeIndex) {
        self.push_serialized_scan_artifact(
            SectionKind::CompositeZoneIndex,
            PrimaryProfile::ArchiveAcceleration,
            FEATURE_COMPOSITE_ZONES,
            index.serialize(),
        );
    }

    pub fn push_topn_summary(&mut self, summary: &TopNSummary) {
        self.push_serialized_scan_artifact(
            SectionKind::TopNZoneSummary,
            PrimaryProfile::ArchiveAcceleration,
            FEATURE_TOPN_SUMMARIES,
            summary.serialize(),
        );
    }

    fn push_serialized_scan_artifact(
        &mut self,
        kind: SectionKind,
        profile: PrimaryProfile,
        feature: u64,
        data: Vec<u8>,
    ) {
        self.extra_sections.push(SectionPayload {
            section_kind: kind as u16,
            profile: profile as u8,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: 0,
            optional_features: feature,
            data,
        });
    }

    pub fn write_to<W: Write + Seek>(&self, writer: &mut W) -> Result<(), CoveError> {
        let inner = self.prepare_inner_writer()?;
        inner.write_to(writer)
    }

    pub fn write(&self) -> Result<Vec<u8>, CoveError> {
        let mut cursor = Cursor::new(Vec::new());
        self.write_to(&mut cursor)?;
        Ok(cursor.into_inner())
    }

    fn prepare_inner_writer(&self) -> Result<MinimalCoveWriter, CoveError> {
        self.table_catalog.validate()?;
        self.validate_segments_against_catalog()?;

        let tables_by_id = self
            .table_catalog
            .tables
            .iter()
            .map(|table| (table.table_id, table))
            .collect::<std::collections::BTreeMap<_, _>>();

        let table_catalog_payload = self.table_catalog.serialize()?;
        let table_catalog_section = SectionPayload {
            section_kind: SectionKind::TableCatalog as u16,
            profile: PrimaryProfile::TableScan as u8,
            flags: 0,
            item_count: self.table_catalog.tables.len() as u64,
            row_count: self.table_catalog.tables.iter().map(|t| t.row_count).sum(),
            compression: 0,
            alignment_log2: 0,
            required_features: nested_column_features_for_catalog(&self.table_catalog),
            optional_features: 0,
            data: table_catalog_payload,
        };
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
        let table_catalog_len = section_encoded_len(&table_catalog_section)?;
        let extra_sections_len = self
            .extra_sections
            .iter()
            .try_fold(0usize, |acc, section| {
                section_encoded_len(section)
                    .and_then(|len| acc.checked_add(len).ok_or(CoveError::ArithOverflow))
            })?;
        let pre_segment_len = HEADER_SIZE
            .checked_add(table_catalog_len)
            .and_then(|len| len.checked_add(extra_sections_len))
            .and_then(|len| len.checked_add(segment_index_len))
            .ok_or(CoveError::ArithOverflow)?;
        let mut offset = pre_segment_len as u64;
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
        let page_required_features = self.segments.iter().fold(0u64, |bits, segment| {
            bits | segment.page_required_features()
        });
        let nested_column_features = nested_column_features_for_catalog(&self.table_catalog);
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
        let extra_required_features = self.extra_sections.iter().fold(0u64, |bits, section| {
            bits | section.required_features
                | profile_feature_bit(section.profile)
                | if matches!(
                    SectionKind::from_u16(section.section_kind),
                    Some(SectionKind::FileDictionaryIndex | SectionKind::FileDictionaryPayload)
                ) {
                    FEATURE_FILE_DICTIONARY
                } else {
                    0
                }
        });
        let extra_optional_features = self.extra_sections.iter().fold(0u64, |bits, section| {
            let kind_bits = if matches!(
                SectionKind::from_u16(section.section_kind),
                Some(SectionKind::FileDictionaryIndex | SectionKind::FileDictionaryPayload)
            ) {
                0
            } else {
                section_kind_feature_bits(section.section_kind)
            };
            bits | section.optional_features | kind_bits
        });
        inner.required_features = FEATURE_TABLE_PROFILE
            | nested_column_features
            | page_required_features
            | extra_required_features;
        inner.optional_features = page_codec_features | extra_optional_features;
        inner.sections.push(table_catalog_section);
        inner.sections.extend(self.extra_sections.iter().cloned());
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
                    .unwrap_or(0)
                    | segment.page_required_features(),
                optional_features: segment.page_codec_features(),
                data: payload,
            });
        }
        Ok(inner)
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
    #[cfg(feature = "compression-lz4")]
    use crate::constants::FEATURE_CODEC_LZ4;
    #[cfg(feature = "compression-zstd")]
    use crate::constants::FEATURE_CODEC_ZSTD;
    use crate::{
        compression::column_page_payload,
        constants::{
            CoveLogicalType, CovePhysicalKind, FEATURE_NESTED_COLUMNS,
            FEATURE_PAGE_PAYLOAD_ELISION, METADATA_LEN_MAX,
        },
        encoding::nested::{ListLayout, ListLayoutPayload},
        footer::CoveFooter,
        header::CoveHeaderV1,
        page::{ColumnPageIndex, PAGE_FLAG_ALL_NULL, PAGE_FLAG_STATS_ONLY_CONSTANT},
        page_payload::{ColumnPagePayloadV1, PageBufferKind},
        postscript::CovePostscriptV1,
        reader::{validate_bytes_with_options, ValidationOptions},
        segment::TableSegmentPayloadV1,
        table::{ColumnEntry, TableEntry},
    };
    #[cfg(any(feature = "compression-lz4", feature = "compression-zstd"))]
    use crate::{
        constants::CoveEncodingKind,
        encoding::local_codebook::{LocalCodebookPayload, LocalCodebookValues, LocalIndexPayload},
        page::PAGE_FLAG_CODEC_MASK,
    };

    #[test]
    fn write_rejects_oversized_metadata() {
        let mut w = MinimalCoveWriter::new();
        w.metadata_json = vec![0u8; (METADATA_LEN_MAX as usize) + 1];
        assert!(matches!(w.write(), Err(CoveError::BadSection(_))));
    }

    #[test]
    fn write_rejects_invalid_metadata_utf8() {
        let mut w = MinimalCoveWriter::new();
        w.metadata_json = vec![0xff];
        assert!(matches!(w.write(), Err(CoveError::BadSection(_))));
    }

    #[test]
    fn write_rejects_invalid_metadata_json() {
        let mut w = MinimalCoveWriter::new();
        w.metadata_json = b"{not-json".to_vec();
        assert!(matches!(w.write(), Err(CoveError::BadSection(_))));
    }

    #[test]
    fn empty_file_is_valid() {
        let bytes = MinimalCoveWriter::write_empty_file().unwrap();

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

        let bytes = writer.write().unwrap();

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
    fn minimal_write_to_matches_vec_writer() {
        let mut writer = MinimalCoveWriter::new();
        writer.metadata_json = br#"{"fixture":"streaming"}"#.to_vec();
        writer.required_features =
            FEATURE_TABLE_PROFILE | crate::constants::FEATURE_FILE_DICTIONARY;
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
            data: Vec::new(),
        });

        let buffered = writer.write().unwrap();
        let mut streamed = std::io::Cursor::new(Vec::new());
        writer.write_to(&mut streamed).unwrap();
        assert_eq!(streamed.into_inner(), buffered);
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

        let bytes = writer.write().unwrap();
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

        let bytes = writer.write().unwrap();
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
                ..ValidationOptions::default()
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
    fn scan_profile_write_to_matches_vec_writer() {
        let catalog = TableCatalog {
            flags: 0,
            tables: vec![TableEntry {
                table_id: 1,
                namespace: "public".into(),
                name: "events".into(),
                row_count: 2,
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
        writer.push_segment(ScanSegment::new(1, 0, 0, 2, 1));

        let buffered = writer.write().unwrap();
        let mut streamed = std::io::Cursor::new(Vec::new());
        writer.write_to(&mut streamed).unwrap();
        assert_eq!(streamed.into_inner(), buffered);
    }

    #[test]
    fn scan_profile_writer_accounts_for_extra_sections_before_segment_data() {
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
        writer
            .push_zone_stats(&ZoneStatsSection::default())
            .unwrap();
        writer.push_segment(ScanSegment::new(1, 0, 0, 10, 1));

        let bytes = writer.write().unwrap();
        let report = validate_bytes_with_options(
            &bytes,
            ValidationOptions {
                semantic: true,
                verify_digests: false,
                allow_unknown_optional_extensions: true,
                ..ValidationOptions::default()
            },
        )
        .unwrap();
        assert_eq!(report.validated.footer.sections.len(), 4);
        assert_eq!(
            report.validated.footer.sections[1].section_kind,
            SectionKind::ZoneStats as u16
        );

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
    }

    #[cfg(any(feature = "compression-lz4", feature = "compression-zstd"))]
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

    #[cfg(any(feature = "compression-lz4", feature = "compression-zstd"))]
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
                .with_encoding_root(CoveEncodingKind::LocalCodebook as u32)],
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
                ..ValidationOptions::default()
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
        assert!(page.uncompressed_length as usize > local_codebook.encode().len());

        let page_wire = &segment_bytes
            [page.page_offset as usize..(page.page_offset + page.page_length) as usize];
        let decoded = column_page_payload(page_wire, page).unwrap();
        let decoded = ColumnPagePayloadV1::parse(&decoded).unwrap();
        let values = decoded
            .buffer_bytes(PageBufferKind::Values)
            .unwrap()
            .unwrap();
        assert_eq!(LocalCodebookPayload::parse(values).unwrap(), local_codebook);
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
    fn scan_profile_writer_propagates_inner_metadata_errors() {
        let catalog = TableCatalog {
            flags: 0,
            tables: vec![TableEntry {
                table_id: 1,
                namespace: "public".into(),
                name: "events".into(),
                row_count: 0,
                primary_sort_key_count: 0,
                clustering_key_count: 0,
                flags: 0,
                columns: vec![],
            }],
        };
        let mut writer = ScanProfileCoveWriter::new(catalog);
        writer.metadata_json = vec![0xff];
        assert!(matches!(writer.write(), Err(CoveError::BadSection(_))));
    }

    fn nullable_bool_writer_for_page(spec: ScanPageSpec) -> ScanProfileCoveWriter {
        let row_count = spec.row_count;
        let catalog = TableCatalog {
            flags: 0,
            tables: vec![TableEntry {
                table_id: 1,
                namespace: "public".into(),
                name: "events".into(),
                row_count: u64::from(row_count),
                primary_sort_key_count: 0,
                clustering_key_count: 0,
                flags: 0,
                columns: vec![ColumnEntry {
                    column_id: 1,
                    name: "active".into(),
                    logical: CoveLogicalType::Bool,
                    physical: CovePhysicalKind::Boolean,
                    nullable: true,
                    sort_order: 0,
                    collation_id: 0,
                    precision: 0,
                    scale: 0,
                    flags: 0,
                }],
            }],
        };
        let mut segment = ScanSegment::new(1, 0, 0, row_count, 1);
        segment.set_column_pages(1, vec![spec]);
        let mut writer = ScanProfileCoveWriter::new(catalog);
        writer.push_segment(segment);
        writer
    }

    #[test]
    fn scan_page_null_bitmap_validation_matches_spec_counts() {
        let too_short = ScanPageSpec::new(9, vec![0x01]).with_counts(8, 1);
        assert!(matches!(
            nullable_bool_writer_for_page(too_short).write(),
            Err(CoveError::PageCorrupt)
        ));

        let tail_bits_set = ScanPageSpec::new(9, vec![0x01, 0x02]).with_counts(8, 1);
        assert!(matches!(
            nullable_bool_writer_for_page(tail_bits_set).write(),
            Err(CoveError::PageCorrupt)
        ));

        let count_mismatch = ScanPageSpec::new(9, vec![0x01, 0x00]).with_counts(7, 2);
        assert!(matches!(
            nullable_bool_writer_for_page(count_mismatch).write(),
            Err(CoveError::PageCorrupt)
        ));
    }

    #[test]
    fn scan_profile_writer_emits_stats_only_all_null_page_and_required_feature() {
        let catalog = TableCatalog {
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
                    nullable: true,
                    sort_order: 0,
                    collation_id: 0,
                    precision: 0,
                    scale: 0,
                    flags: 0,
                }],
            }],
        };
        let mut segment = ScanSegment::new(1, 0, 0, 6, 1);
        segment.set_column_pages(
            1,
            vec![ScanPageSpec::new(6, Vec::new())
                .with_counts(0, 6)
                .with_encoding_root(u32::MAX)
                .with_flags(PAGE_FLAG_STATS_ONLY_CONSTANT | PAGE_FLAG_ALL_NULL)],
        );

        let mut writer = ScanProfileCoveWriter::new(catalog);
        writer.push_segment(segment);

        let bytes = writer.write().unwrap();
        let report = validate_bytes_with_options(
            &bytes,
            ValidationOptions {
                semantic: true,
                verify_digests: false,
                allow_unknown_optional_extensions: true,
                ..ValidationOptions::default()
            },
        )
        .unwrap();
        assert_ne!(
            report.validated.header.required_features & FEATURE_PAGE_PAYLOAD_ELISION,
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
            segment_data_entry.required_features & FEATURE_PAGE_PAYLOAD_ELISION,
            0
        );

        let segment_bytes = &bytes
            [segment_data_entry.offset as usize..segment_data_entry.end_offset().unwrap() as usize];
        let payload = TableSegmentPayloadV1::parse_with_required_features(
            segment_bytes,
            report.validated.header.required_features,
        )
        .unwrap();
        let column = &payload.columns[0];
        let page_index_bytes = &segment_bytes[column.page_index_offset as usize
            ..(column.page_index_offset + column.page_index_length) as usize];
        let page_index = ColumnPageIndex::parse(page_index_bytes).unwrap();
        let page = &page_index.entries[0];
        assert_eq!(
            page.flags & (PAGE_FLAG_STATS_ONLY_CONSTANT | PAGE_FLAG_ALL_NULL),
            PAGE_FLAG_STATS_ONLY_CONSTANT | PAGE_FLAG_ALL_NULL
        );
        assert_eq!(page.page_length, 0);
        assert_eq!(page.page_offset, 0);
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
                ..ValidationOptions::default()
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
        let decoded = ColumnPagePayloadV1::parse(&decoded).unwrap();
        let values = decoded
            .buffer_bytes(PageBufferKind::Values)
            .unwrap()
            .unwrap();
        assert_eq!(ListLayoutPayload::parse(values).unwrap(), payload);
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
