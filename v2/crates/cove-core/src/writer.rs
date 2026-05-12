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
        FEATURE_ARCHIVE_PROFILE, FEATURE_BLOOM_FILTERS, FEATURE_CODEC_EXTENSION_REGISTRY,
        FEATURE_CODEC_LZ4, FEATURE_CODEC_ZSTD, FEATURE_COLUMN_DOMAINS, FEATURE_COMPOSITE_ZONES,
        FEATURE_COVERAGE_METADATA, FEATURE_ENGINE_PROFILE, FEATURE_EXACT_SETS,
        FEATURE_FILE_DICTIONARY, FEATURE_HARBOR_PROFILE, FEATURE_INVERTED_INDEXES,
        FEATURE_LAYOUT_PLAN, FEATURE_LOOKUP_INDEXES, FEATURE_NESTED_COLUMNS,
        FEATURE_OBJECT_PROFILE, FEATURE_PAGE_PAYLOAD_ELISION, FEATURE_RUNTIME_COMPATIBILITY_HINTS,
        FEATURE_SECONDARY_INDEX_ARTIFACT, FEATURE_SEMANTIC_MAP, FEATURE_TABLE_PROFILE,
        FEATURE_TOPN_SUMMARIES, FOOTER_VERSION_V1, HEADER_LEN_V1, KNOWN_FEATURE_BITS_MASK,
        MAGIC_COVE, MAGIC_COVE_FOOTER, SECTION_ENTRY_LEN, VERSION_MAJOR_V1,
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

mod minimal;
mod scan;

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
        assert_eq!(header.version_major, VERSION_MAJOR_V1);
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
