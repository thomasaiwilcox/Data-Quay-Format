//! Cove Format (COVE) v1.0 — Column page index and page header (Spec §27).
//!
//! A *page* is the smallest physically encoded unit in a column. Each page
//! header records its row count, null count, encoded byte length, encoding
//! kind, and CRC32C. The reader uses page headers to bounds-check decode and
//! to drive the canonical / fast / kernel decode path triad described in
//! Spec §20.

use crate::{
    checksum::crc32c,
    constants::{CompressionCodec, CoveEncodingKind},
    CoveError,
};

pub const COLUMN_PAGE_INDEX_ENTRY_LEN: usize = 60;

/// Spec §27.2 / §66: the low byte of `ColumnPageIndexEntryV1.flags` carries
/// the page-level [`CompressionCodec`] identifier. Bits `0x0100..0x0800`
/// carry the v1 payload-elision flags; the remaining high bits are reserved
/// and MUST be zero in v1.0.
pub const PAGE_FLAG_CODEC_MASK: u32 = 0x0000_00FF;
pub const PAGE_FLAG_STATS_ONLY_CONSTANT: u32 = 0x0000_0100;
pub const PAGE_FLAG_ALL_NULL: u32 = 0x0000_0200;
pub const PAGE_FLAG_ALL_NON_NULL: u32 = 0x0000_0400;
pub const PAGE_FLAG_VALUE_STREAM_ELIDED: u32 = 0x0000_0800;
pub const PAGE_FLAG_KNOWN_MASK: u32 = PAGE_FLAG_CODEC_MASK
    | PAGE_FLAG_STATS_ONLY_CONSTANT
    | PAGE_FLAG_ALL_NULL
    | PAGE_FLAG_ALL_NON_NULL
    | PAGE_FLAG_VALUE_STREAM_ELIDED;
pub const PAGE_FLAG_RESERVED_MASK: u32 = !PAGE_FLAG_KNOWN_MASK;

fn empty_page_checksum() -> u32 {
    crc32c(&[])
}

/// Returns the [`CompressionCodec`] encoded in `flags`, or an error if the
/// codec value is unknown or any reserved bit is set.
pub fn page_flag_codec(flags: u32) -> Result<CompressionCodec, CoveError> {
    if flags & PAGE_FLAG_RESERVED_MASK != 0 {
        return Err(CoveError::BadSection(format!(
            "page flags reserved bits must be zero (flags=0x{flags:08x})"
        )));
    }
    let raw = (flags & PAGE_FLAG_CODEC_MASK) as u8;
    CompressionCodec::from_u8(raw)
        .ok_or_else(|| CoveError::BadSection(format!("unknown page compression codec {raw}")))
}

pub fn page_uses_payload_elision(flags: u32) -> bool {
    flags
        & (PAGE_FLAG_STATS_ONLY_CONSTANT
            | PAGE_FLAG_ALL_NULL
            | PAGE_FLAG_ALL_NON_NULL
            | PAGE_FLAG_VALUE_STREAM_ELIDED)
        != 0
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnPageIndexEntryV1 {
    pub column_id: u32,
    pub morsel_id: u32,
    pub row_count: u32,
    pub non_null_count: u32,
    pub null_count: u32,
    pub encoding_root: u32,
    pub page_offset: u64,
    pub page_length: u64,
    pub uncompressed_length: u64,
    pub stats_ref: u32,
    pub flags: u32,
    pub checksum: u32,
}

impl ColumnPageIndexEntryV1 {
    pub fn serialize(&self) -> [u8; COLUMN_PAGE_INDEX_ENTRY_LEN] {
        let mut out = [0u8; COLUMN_PAGE_INDEX_ENTRY_LEN];
        out[0..4].copy_from_slice(&self.column_id.to_le_bytes());
        out[4..8].copy_from_slice(&self.morsel_id.to_le_bytes());
        out[8..12].copy_from_slice(&self.row_count.to_le_bytes());
        out[12..16].copy_from_slice(&self.non_null_count.to_le_bytes());
        out[16..20].copy_from_slice(&self.null_count.to_le_bytes());
        out[20..24].copy_from_slice(&self.encoding_root.to_le_bytes());
        out[24..32].copy_from_slice(&self.page_offset.to_le_bytes());
        out[32..40].copy_from_slice(&self.page_length.to_le_bytes());
        out[40..48].copy_from_slice(&self.uncompressed_length.to_le_bytes());
        out[48..52].copy_from_slice(&self.stats_ref.to_le_bytes());
        out[52..56].copy_from_slice(&self.flags.to_le_bytes());
        out[56..60].copy_from_slice(&self.checksum.to_le_bytes());
        out
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < COLUMN_PAGE_INDEX_ENTRY_LEN {
            return Err(CoveError::BufferTooShort);
        }
        let bytes = &bytes[..COLUMN_PAGE_INDEX_ENTRY_LEN];
        let checksum = u32::from_le_bytes(bytes[56..60].try_into().unwrap());
        let row_count = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        let non_null_count = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
        let null_count = u32::from_le_bytes(bytes[16..20].try_into().unwrap());
        if non_null_count
            .checked_add(null_count)
            .ok_or(CoveError::ArithOverflow)?
            != row_count
        {
            return Err(CoveError::PageCorrupt);
        }

        let page_length = u64::from_le_bytes(bytes[32..40].try_into().unwrap());
        let uncompressed_length = u64::from_le_bytes(bytes[40..48].try_into().unwrap());
        let flags = u32::from_le_bytes(bytes[52..56].try_into().unwrap());
        let codec = page_flag_codec(flags)?;
        let stats_only_constant = flags & PAGE_FLAG_STATS_ONLY_CONSTANT != 0;
        let all_null = flags & PAGE_FLAG_ALL_NULL != 0;
        let all_non_null = flags & PAGE_FLAG_ALL_NON_NULL != 0;

        if all_null && all_non_null {
            return Err(CoveError::BadSection(
                "PAGE_FLAG_ALL_NULL and PAGE_FLAG_ALL_NON_NULL are mutually exclusive".into(),
            ));
        }
        if all_null && (null_count != row_count || non_null_count != 0) {
            return Err(CoveError::PageCorrupt);
        }
        if all_non_null && (null_count != 0 || non_null_count != row_count) {
            return Err(CoveError::PageCorrupt);
        }
        // Spec §13.2 / §66: codec=None requires page_length == uncompressed_length.
        // Compressed codecs require uncompressed_length > 0 whenever page_length
        // > 0 (a compressed payload cannot decode to zero bytes), and an empty
        // page (page_length == 0) MUST also have uncompressed_length == 0.
        if stats_only_constant {
            if !all_null && !all_non_null {
                return Err(CoveError::BadSection(
                    "PAGE_FLAG_STATS_ONLY_CONSTANT requires PAGE_FLAG_ALL_NULL or PAGE_FLAG_ALL_NON_NULL"
                        .into(),
                ));
            }
            if codec != CompressionCodec::None {
                return Err(CoveError::BadSection(
                    "stats-only constant pages must use page codec=None".into(),
                ));
            }
            if page_length != 0 || uncompressed_length != 0 {
                return Err(CoveError::BadSection(
                    "stats-only constant pages must have page_length=0 and uncompressed_length=0"
                        .into(),
                ));
            }
            if u64::from_le_bytes(bytes[24..32].try_into().unwrap()) != 0 {
                return Err(CoveError::BadSection(
                    "stats-only constant pages must set page_offset=0".into(),
                ));
            }
            if u32::from_le_bytes(bytes[20..24].try_into().unwrap()) != u32::MAX {
                return Err(CoveError::BadSection(
                    "stats-only constant pages must set encoding_root=u32::MAX".into(),
                ));
            }
            if checksum != empty_page_checksum() {
                return Err(CoveError::BadSection(
                    "stats-only constant pages must use the empty-page checksum".into(),
                ));
            }
        } else {
            if page_length == 0 {
                return Err(CoveError::BadSection(
                    "page_length=0 requires PAGE_FLAG_STATS_ONLY_CONSTANT".into(),
                ));
            }
        }
        if codec == CompressionCodec::None && page_length != uncompressed_length {
            return Err(CoveError::BadSection(
                "uncompressed_length must equal page_length when page codec=None".into(),
            ));
        }
        if page_length == 0 && uncompressed_length != 0 {
            return Err(CoveError::BadSection(
                "page_length=0 requires uncompressed_length=0".into(),
            ));
        }
        if page_length != 0 && codec != CompressionCodec::None && uncompressed_length == 0 {
            return Err(CoveError::BadSection(
                "compressed page must declare non-zero uncompressed_length".into(),
            ));
        }

        Ok(Self {
            column_id: u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
            morsel_id: u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            row_count,
            non_null_count,
            null_count,
            encoding_root: u32::from_le_bytes(bytes[20..24].try_into().unwrap()),
            page_offset: u64::from_le_bytes(bytes[24..32].try_into().unwrap()),
            page_length,
            uncompressed_length,
            stats_ref: u32::from_le_bytes(bytes[48..52].try_into().unwrap()),
            flags,
            checksum,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ColumnPageIndex {
    pub entries: Vec<ColumnPageIndexEntryV1>,
}

impl ColumnPageIndex {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.is_empty() {
            return Ok(Self {
                entries: Vec::new(),
            });
        }
        if !bytes.len().is_multiple_of(COLUMN_PAGE_INDEX_ENTRY_LEN) {
            return Err(CoveError::PageCorrupt);
        }
        let mut entries = Vec::with_capacity(bytes.len() / COLUMN_PAGE_INDEX_ENTRY_LEN);
        for chunk in bytes.chunks_exact(COLUMN_PAGE_INDEX_ENTRY_LEN) {
            entries.push(ColumnPageIndexEntryV1::parse(chunk)?);
        }
        Ok(Self { entries })
    }
}

/// One column page entry in the page index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageEntry {
    pub page_id: u32,
    pub morsel_id: u32,
    /// Number of rows encoded by this page.
    pub row_count: u32,
    /// Number of null rows in this page (Spec §27.3).
    pub null_count: u32,
    /// Byte offset within the section payload where this page starts.
    pub offset: u64,
    /// Encoded byte length of this page.
    pub length: u64,
    /// Encoding kind used for this page (Spec §20.1).
    pub encoding: CoveEncodingKind,
    /// CRC32C of the page bytes (Spec §27.4).
    pub crc32c: u32,
}

impl PageEntry {
    /// Validate this page's CRC against `payload`.
    pub fn verify_crc(&self, payload: &[u8]) -> Result<(), CoveError> {
        let end = self
            .offset
            .checked_add(self.length)
            .ok_or(CoveError::ArithOverflow)?;
        if end as usize > payload.len() {
            return Err(CoveError::OffsetRange);
        }
        let actual = crc32c(&payload[self.offset as usize..end as usize]);
        if actual != self.crc32c {
            Err(CoveError::PageCorrupt)
        } else {
            Ok(())
        }
    }

    /// Number of non-null rows. The §27.3 invariant
    /// `null_count + non_null_count == row_count` is enforced at parse time.
    pub fn non_null_count(&self) -> u32 {
        self.row_count - self.null_count
    }
}

/// A parsed column page index for a single column.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageIndex {
    pub entries: Vec<PageEntry>,
}

impl PageIndex {
    /// Wire format (LE):
    ///   `u32` count
    ///   For each entry: `u32` page_id, `u32` morsel_id, `u32` row_count,
    ///                   `u32` null_count, `u64` offset, `u64` length,
    ///                   `u16` encoding, `u32` crc32c.
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < 4 {
            return Err(CoveError::BufferTooShort);
        }
        let count = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let entry_size = 4usize + 4 + 4 + 4 + 8 + 8 + 2 + 4;
        let entries_bytes = count
            .checked_mul(entry_size)
            .ok_or(CoveError::ArithOverflow)?;
        let required_len = 4usize
            .checked_add(entries_bytes)
            .ok_or(CoveError::ArithOverflow)?;
        if required_len > bytes.len() {
            return Err(CoveError::BufferTooShort);
        }
        let mut entries = Vec::with_capacity(count);
        let mut pos = 4usize;
        for _ in 0..count {
            let page_id = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
            pos += 4;
            let morsel_id = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
            pos += 4;
            let row_count = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
            pos += 4;
            let null_count = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
            pos += 4;
            let offset = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
            pos += 8;
            let length = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
            pos += 8;
            let enc_raw = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap());
            pos += 2;
            let crc = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
            pos += 4;
            if null_count > row_count {
                return Err(CoveError::PageCorrupt);
            }
            let encoding = CoveEncodingKind::from_u16(enc_raw)
                .ok_or_else(|| CoveError::BadSection(format!("unknown encoding {enc_raw}")))?;
            entries.push(PageEntry {
                page_id,
                morsel_id,
                row_count,
                null_count,
                offset,
                length,
                encoding,
                crc32c: crc,
            });
        }
        Ok(Self { entries })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_page_bytes(entries: &[(u32, u32, u32, u32, u64, u64, u16, u32)]) -> Vec<u8> {
        let mut out = (entries.len() as u32).to_le_bytes().to_vec();
        for (pid, mid, rc, nc, off, len, enc, crc) in entries {
            out.extend_from_slice(&pid.to_le_bytes());
            out.extend_from_slice(&mid.to_le_bytes());
            out.extend_from_slice(&rc.to_le_bytes());
            out.extend_from_slice(&nc.to_le_bytes());
            out.extend_from_slice(&off.to_le_bytes());
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&enc.to_le_bytes());
            out.extend_from_slice(&crc.to_le_bytes());
        }
        out
    }

    #[test]
    fn round_trip_index() {
        let payload = b"some bytes for a fake page";
        let crc = crc32c(payload);
        let bytes = make_page_bytes(&[(0, 0, 4, 1, 0, payload.len() as u64, 0, crc)]);
        let idx = PageIndex::parse(&bytes).unwrap();
        assert_eq!(idx.entries[0].non_null_count(), 3);
        assert!(idx.entries[0].verify_crc(payload).is_ok());
    }

    #[test]
    fn rejects_null_count_above_row_count() {
        let bytes = make_page_bytes(&[(0, 0, 4, 5, 0, 0, 0, 0)]);
        assert_eq!(PageIndex::parse(&bytes), Err(CoveError::PageCorrupt));
    }

    #[test]
    fn rejects_unknown_encoding() {
        let bytes = make_page_bytes(&[(0, 0, 1, 0, 0, 0, 0xfffe, 0)]);
        assert!(matches!(
            PageIndex::parse(&bytes),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn detects_page_crc_mismatch() {
        let bytes = make_page_bytes(&[(0, 0, 1, 0, 0, 5, 0, 0)]);
        let idx = PageIndex::parse(&bytes).unwrap();
        assert_eq!(
            idx.entries[0].verify_crc(b"hello"),
            Err(CoveError::PageCorrupt)
        );
    }

    #[test]
    fn rejects_oversized_entry_count_before_allocating() {
        let bytes = u32::MAX.to_le_bytes().to_vec();
        assert_eq!(PageIndex::parse(&bytes), Err(CoveError::BufferTooShort));
    }

    #[test]
    fn column_page_index_entry_round_trip() {
        let bytes = ColumnPageIndexEntryV1 {
            column_id: 1,
            morsel_id: 2,
            row_count: 10,
            non_null_count: 8,
            null_count: 2,
            encoding_root: 3,
            page_offset: 100,
            page_length: 50,
            uncompressed_length: 50,
            stats_ref: 4,
            flags: 0,
            checksum: 0,
        }
        .serialize();
        let entry = ColumnPageIndexEntryV1::parse(&bytes).unwrap();
        assert_eq!(entry.column_id, 1);
        assert_eq!(entry.row_count, 10);
    }

    #[test]
    fn column_page_index_entry_rejects_bad_counts() {
        let mut bytes = ColumnPageIndexEntryV1 {
            column_id: 1,
            morsel_id: 2,
            row_count: 10,
            non_null_count: 8,
            null_count: 2,
            encoding_root: 3,
            page_offset: 100,
            page_length: 50,
            uncompressed_length: 50,
            stats_ref: 4,
            flags: 0,
            checksum: 0,
        }
        .serialize();
        bytes[16..20].copy_from_slice(&3u32.to_le_bytes());
        assert_eq!(
            ColumnPageIndexEntryV1::parse(&bytes),
            Err(CoveError::PageCorrupt)
        );
    }

    fn page_entry(
        page_length: u64,
        uncompressed_length: u64,
        flags: u32,
    ) -> ColumnPageIndexEntryV1 {
        ColumnPageIndexEntryV1 {
            column_id: 1,
            morsel_id: 2,
            row_count: 4,
            non_null_count: 4,
            null_count: 0,
            encoding_root: 0,
            page_offset: 0,
            page_length,
            uncompressed_length,
            stats_ref: 0,
            flags,
            checksum: 0,
        }
    }

    #[test]
    fn page_codec_none_requires_matching_lengths() {
        // codec=None but uncompressed_length != page_length is rejected.
        let bytes = page_entry(10, 12, 0).serialize();
        assert!(matches!(
            ColumnPageIndexEntryV1::parse(&bytes),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn page_compressed_requires_nonzero_uncompressed_length() {
        // codec=Lz4 with page_length>0 but uncompressed_length=0 is rejected.
        let bytes = page_entry(10, 0, CompressionCodec::Lz4 as u32).serialize();
        assert!(matches!(
            ColumnPageIndexEntryV1::parse(&bytes),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn page_empty_must_have_zero_uncompressed_length() {
        let bytes = page_entry(0, 5, 0).serialize();
        assert!(matches!(
            ColumnPageIndexEntryV1::parse(&bytes),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn page_unknown_codec_rejected() {
        // Codec value 0xFF is not a known CompressionCodec.
        let bytes = page_entry(10, 10, 0xFF).serialize();
        assert!(matches!(
            ColumnPageIndexEntryV1::parse(&bytes),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn page_reserved_flag_bits_must_be_zero() {
        // Unknown high bits above the known codec/elision flags are rejected.
        let bytes = page_entry(10, 10, 0x0000_1000).serialize();
        assert!(matches!(
            ColumnPageIndexEntryV1::parse(&bytes),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn page_payload_elision_flag_helper_detects_usage() {
        assert!(!page_uses_payload_elision(CompressionCodec::None as u32));
        assert!(page_uses_payload_elision(PAGE_FLAG_ALL_NULL));
        assert!(page_uses_payload_elision(
            PAGE_FLAG_STATS_ONLY_CONSTANT | PAGE_FLAG_ALL_NON_NULL
        ));
    }

    #[test]
    fn page_flags_all_null_requires_all_rows_null() {
        let bytes = ColumnPageIndexEntryV1 {
            column_id: 1,
            morsel_id: 2,
            row_count: 4,
            non_null_count: 1,
            null_count: 3,
            encoding_root: 0,
            page_offset: 0,
            page_length: 4,
            uncompressed_length: 4,
            stats_ref: 0,
            flags: PAGE_FLAG_ALL_NULL,
            checksum: 0,
        }
        .serialize();
        assert_eq!(
            ColumnPageIndexEntryV1::parse(&bytes),
            Err(CoveError::PageCorrupt)
        );
    }

    #[test]
    fn page_flags_all_non_null_requires_no_nulls() {
        let bytes = ColumnPageIndexEntryV1 {
            column_id: 1,
            morsel_id: 2,
            row_count: 4,
            non_null_count: 3,
            null_count: 1,
            encoding_root: 0,
            page_offset: 0,
            page_length: 4,
            uncompressed_length: 4,
            stats_ref: 0,
            flags: PAGE_FLAG_ALL_NON_NULL,
            checksum: 0,
        }
        .serialize();
        assert_eq!(
            ColumnPageIndexEntryV1::parse(&bytes),
            Err(CoveError::PageCorrupt)
        );
    }

    #[test]
    fn page_flags_all_null_and_all_non_null_conflict() {
        let bytes = ColumnPageIndexEntryV1 {
            column_id: 1,
            morsel_id: 2,
            row_count: 4,
            non_null_count: 0,
            null_count: 4,
            encoding_root: 0,
            page_offset: 0,
            page_length: 4,
            uncompressed_length: 4,
            stats_ref: 0,
            flags: PAGE_FLAG_ALL_NULL | PAGE_FLAG_ALL_NON_NULL,
            checksum: 0,
        }
        .serialize();
        assert!(matches!(
            ColumnPageIndexEntryV1::parse(&bytes),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn stats_only_constant_requires_all_null_or_all_non_null() {
        let bytes = ColumnPageIndexEntryV1 {
            column_id: 1,
            morsel_id: 2,
            row_count: 4,
            non_null_count: 2,
            null_count: 2,
            encoding_root: u32::MAX,
            page_offset: 0,
            page_length: 0,
            uncompressed_length: 0,
            stats_ref: 0,
            flags: PAGE_FLAG_STATS_ONLY_CONSTANT,
            checksum: empty_page_checksum(),
        }
        .serialize();
        assert!(matches!(
            ColumnPageIndexEntryV1::parse(&bytes),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn stats_only_constant_requires_empty_none_payload() {
        let bytes = ColumnPageIndexEntryV1 {
            column_id: 1,
            morsel_id: 2,
            row_count: 4,
            non_null_count: 0,
            null_count: 4,
            encoding_root: u32::MAX,
            page_offset: 0,
            page_length: 1,
            uncompressed_length: 1,
            stats_ref: 0,
            flags: PAGE_FLAG_STATS_ONLY_CONSTANT | PAGE_FLAG_ALL_NULL,
            checksum: empty_page_checksum(),
        }
        .serialize();
        assert!(matches!(
            ColumnPageIndexEntryV1::parse(&bytes),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn stats_only_constant_all_null_page_is_accepted() {
        let bytes = ColumnPageIndexEntryV1 {
            column_id: 1,
            morsel_id: 2,
            row_count: 4,
            non_null_count: 0,
            null_count: 4,
            encoding_root: u32::MAX,
            page_offset: 0,
            page_length: 0,
            uncompressed_length: 0,
            stats_ref: 0,
            flags: PAGE_FLAG_STATS_ONLY_CONSTANT | PAGE_FLAG_ALL_NULL,
            checksum: empty_page_checksum(),
        }
        .serialize();
        let entry = ColumnPageIndexEntryV1::parse(&bytes).unwrap();
        assert_eq!(
            entry.flags,
            PAGE_FLAG_STATS_ONLY_CONSTANT | PAGE_FLAG_ALL_NULL
        );
        assert_eq!(entry.page_length, 0);
    }

    #[test]
    fn page_length_zero_without_stats_only_constant_is_rejected() {
        let bytes = ColumnPageIndexEntryV1 {
            column_id: 1,
            morsel_id: 2,
            row_count: 4,
            non_null_count: 4,
            null_count: 0,
            encoding_root: 0,
            page_offset: 0,
            page_length: 0,
            uncompressed_length: 0,
            stats_ref: 0,
            flags: PAGE_FLAG_ALL_NON_NULL,
            checksum: empty_page_checksum(),
        }
        .serialize();
        assert!(matches!(
            ColumnPageIndexEntryV1::parse(&bytes),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn page_lz4_round_trip_accepted() {
        // codec=Lz4 with consistent lengths parses cleanly.
        let bytes = page_entry(20, 50, CompressionCodec::Lz4 as u32).serialize();
        let entry = ColumnPageIndexEntryV1::parse(&bytes).unwrap();
        assert_eq!(
            entry.flags & PAGE_FLAG_CODEC_MASK,
            CompressionCodec::Lz4 as u32
        );
        assert_eq!(entry.uncompressed_length, 50);
    }
}
