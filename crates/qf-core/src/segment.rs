//! Spec §25 — QF-T table segments and §26 — row morsels (spec-exact).
//!
//! A *segment* is a contiguous block of rows shared by every column in a
//! table; a *morsel* is a fixed-row chunk inside a segment that all columns
//! must align with.
//!
//! This module owns the two parser surfaces that read these structures from
//! their respective section payloads:
//!
//! * [`TableSegmentIndex`] — the segment index (Spec §25.1) listing every
//!   segment in a table with row range, morsel layout, and segment payload
//!   location.
//! * [`TableSegmentHeader`] — the in-segment header (Spec §25.2) with
//!   bootstrap offsets to the morsel directory, column directory, page
//!   index region, and column data region.
//! * [`RowMorselDirectory`] — the per-segment morsel directory (Spec §26)
//!   listing every morsel's `first_row_in_segment` and `row_count`.
//!
//! Spec rules enforced here:
//! * Segment `segment_id` MUST be unique within a `table_id` (Spec §25).
//! * `row_count` MUST equal the sum of row counts in the segment's morsels
//!   (cross-checked by [`RowMorselDirectory::sum_rows`] / caller).
//! * Morsels MUST be ordered by `first_row_in_segment`, contiguous, and
//!   non-overlapping (Spec §26).
//! * The segment header checksum MUST validate before its internal offsets
//!   are trusted (Spec §25.2).
//! * Per-entry CRC32C checksums on segment-index entries and morsel
//!   entries are recomputed and verified.

use crate::checksum;
use crate::QfError;

// ── TableSegmentIndexEntryV1 (Spec §25.1) ────────────────────────────────────

/// Encoded length of [`TableSegmentIndexEntryV1`].
///
/// Layout: table_id(4) + segment_id(4) + row_start(8) + row_count(4)
///       + morsel_count(4) + morsel_row_count(4) + column_count(4)
///       + offset(8) + length(8) + stats_ref(4) + flags(4) + checksum(4) = 60.
pub const TABLE_SEGMENT_INDEX_ENTRY_LEN: usize = 60;

/// Spec §25.1 `TableSegmentIndexEntryV1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableSegmentIndexEntryV1 {
    pub table_id: u32,
    pub segment_id: u32,
    pub row_start: u64,
    pub row_count: u32,
    pub morsel_count: u32,
    pub morsel_row_count: u32,
    pub column_count: u32,
    /// Byte offset of the segment's payload region within the file.
    pub offset: u64,
    /// Byte length of the segment's payload region.
    pub length: u64,
    /// Optional reference into the file-level stats catalog (`0` = none).
    pub stats_ref: u32,
    pub flags: u32,
    /// CRC32C of the 60-byte entry with `checksum` zeroed.
    pub checksum: u32,
}

impl TableSegmentIndexEntryV1 {
    pub fn serialize(&self) -> [u8; TABLE_SEGMENT_INDEX_ENTRY_LEN] {
        let mut buf = [0u8; TABLE_SEGMENT_INDEX_ENTRY_LEN];
        buf[0..4].copy_from_slice(&self.table_id.to_le_bytes());
        buf[4..8].copy_from_slice(&self.segment_id.to_le_bytes());
        buf[8..16].copy_from_slice(&self.row_start.to_le_bytes());
        buf[16..20].copy_from_slice(&self.row_count.to_le_bytes());
        buf[20..24].copy_from_slice(&self.morsel_count.to_le_bytes());
        buf[24..28].copy_from_slice(&self.morsel_row_count.to_le_bytes());
        buf[28..32].copy_from_slice(&self.column_count.to_le_bytes());
        buf[32..40].copy_from_slice(&self.offset.to_le_bytes());
        buf[40..48].copy_from_slice(&self.length.to_le_bytes());
        buf[48..52].copy_from_slice(&self.stats_ref.to_le_bytes());
        buf[52..56].copy_from_slice(&self.flags.to_le_bytes());
        // [56..60] = checksum, zero during CRC.
        let crc = checksum::crc32c(&buf);
        buf[56..60].copy_from_slice(&crc.to_le_bytes());
        buf
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < TABLE_SEGMENT_INDEX_ENTRY_LEN {
            return Err(QfError::BufferTooShort);
        }
        let bytes = &bytes[..TABLE_SEGMENT_INDEX_ENTRY_LEN];
        let table_id = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let segment_id = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let row_start = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
        let row_count = u32::from_le_bytes(bytes[16..20].try_into().unwrap());
        let morsel_count = u32::from_le_bytes(bytes[20..24].try_into().unwrap());
        let morsel_row_count = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
        let column_count = u32::from_le_bytes(bytes[28..32].try_into().unwrap());
        let offset = u64::from_le_bytes(bytes[32..40].try_into().unwrap());
        let length = u64::from_le_bytes(bytes[40..48].try_into().unwrap());
        let stats_ref = u32::from_le_bytes(bytes[48..52].try_into().unwrap());
        let flags = u32::from_le_bytes(bytes[52..56].try_into().unwrap());
        let checksum_field = u32::from_le_bytes(bytes[56..60].try_into().unwrap());

        let mut for_crc = [0u8; TABLE_SEGMENT_INDEX_ENTRY_LEN];
        for_crc.copy_from_slice(bytes);
        for_crc[56..60].fill(0);
        if checksum::crc32c(&for_crc) != checksum_field {
            return Err(QfError::ChecksumMismatch);
        }

        Ok(Self {
            table_id,
            segment_id,
            row_start,
            row_count,
            morsel_count,
            morsel_row_count,
            column_count,
            offset,
            length,
            stats_ref,
            flags,
            checksum: checksum_field,
        })
    }
}

// ── TableSegmentIndex section ────────────────────────────────────────────────

/// Section-payload wrapper for the `TableSegmentIndex` section.
///
/// Layout: `u32 entry_count, u32 flags, TableSegmentIndexEntryV1[entry_count]`.
/// Spec §25 itself defines only the entry struct; the leading
/// `entry_count` + `flags` framing mirrors the `TableCatalogV1` style and
/// is implementation-defined for self-described section payloads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TableSegmentIndex {
    pub flags: u32,
    pub entries: Vec<TableSegmentIndexEntryV1>,
}

impl TableSegmentIndex {
    pub fn serialize(&self) -> Result<Vec<u8>, QfError> {
        let count = u32::try_from(self.entries.len()).map_err(|_| QfError::SegmentCorrupt)?;
        let mut out = Vec::with_capacity(8 + self.entries.len() * TABLE_SEGMENT_INDEX_ENTRY_LEN);
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        for e in &self.entries {
            out.extend_from_slice(&e.serialize());
        }
        Ok(out)
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < 8 {
            return Err(QfError::BufferTooShort);
        }
        let count = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let flags = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let needed = 8usize
            .checked_add(
                count
                    .checked_mul(TABLE_SEGMENT_INDEX_ENTRY_LEN)
                    .ok_or(QfError::ArithOverflow)?,
            )
            .ok_or(QfError::ArithOverflow)?;
        if needed > bytes.len() {
            return Err(QfError::BufferTooShort);
        }
        let mut entries = Vec::with_capacity(count);
        let mut pos = 8usize;
        for _ in 0..count {
            entries.push(TableSegmentIndexEntryV1::parse(
                &bytes[pos..pos + TABLE_SEGMENT_INDEX_ENTRY_LEN],
            )?);
            pos += TABLE_SEGMENT_INDEX_ENTRY_LEN;
        }
        let idx = Self { flags, entries };
        idx.validate()?;
        Ok(idx)
    }

    /// Spec §25 invariants applied to the index as a whole:
    /// * `(table_id, segment_id)` MUST be unique.
    /// * For a given `table_id`, segments MUST be ordered by `row_start`,
    ///   non-overlapping, and contiguous starting from row 0.
    pub fn validate(&self) -> Result<(), QfError> {
        let mut seen = std::collections::HashSet::new();
        for e in &self.entries {
            if !seen.insert((e.table_id, e.segment_id)) {
                return Err(QfError::SegmentCorrupt);
            }
        }
        // Group by table_id and check contiguous row ranges per table.
        use std::collections::BTreeMap;
        let mut by_table: BTreeMap<u32, Vec<&TableSegmentIndexEntryV1>> = BTreeMap::new();
        for e in &self.entries {
            by_table.entry(e.table_id).or_default().push(e);
        }
        for (_tid, mut entries) in by_table {
            entries.sort_by_key(|e| e.row_start);
            let mut next_row = 0u64;
            for e in entries {
                if e.row_start != next_row {
                    return Err(QfError::SegmentCorrupt);
                }
                let end = e
                    .row_start
                    .checked_add(e.row_count as u64)
                    .ok_or(QfError::ArithOverflow)?;
                next_row = end;
            }
        }
        Ok(())
    }
}

// ── TableSegmentHeaderV1 (Spec §25.2) ────────────────────────────────────────

/// Encoded length of [`TableSegmentHeaderV1`].
///
/// Layout: table_id(4) + segment_id(4) + row_start(8) + row_count(4)
///       + morsel_count(4) + morsel_row_count(4) + column_count(4)
///       + morsel_directory_offset(8) + column_directory_offset(8)
///       + page_index_offset(8) + data_offset(8)
///       + flags(4) + checksum(4) = 72.
pub const TABLE_SEGMENT_HEADER_LEN: usize = 72;

/// Spec §25.2 `TableSegmentHeaderV1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableSegmentHeaderV1 {
    pub table_id: u32,
    pub segment_id: u32,
    pub row_start: u64,
    pub row_count: u32,
    pub morsel_count: u32,
    pub morsel_row_count: u32,
    pub column_count: u32,
    /// Offset (within the segment payload) of the morsel directory.
    pub morsel_directory_offset: u64,
    /// Offset (within the segment payload) of the column directory.
    pub column_directory_offset: u64,
    /// Offset (within the segment payload) of the page-index region.
    pub page_index_offset: u64,
    /// Offset (within the segment payload) of the column-data region.
    pub data_offset: u64,
    pub flags: u32,
    /// CRC32C of the 72-byte header with `checksum` zeroed.
    pub checksum: u32,
}

impl TableSegmentHeaderV1 {
    pub fn serialize(&self) -> [u8; TABLE_SEGMENT_HEADER_LEN] {
        let mut buf = [0u8; TABLE_SEGMENT_HEADER_LEN];
        buf[0..4].copy_from_slice(&self.table_id.to_le_bytes());
        buf[4..8].copy_from_slice(&self.segment_id.to_le_bytes());
        buf[8..16].copy_from_slice(&self.row_start.to_le_bytes());
        buf[16..20].copy_from_slice(&self.row_count.to_le_bytes());
        buf[20..24].copy_from_slice(&self.morsel_count.to_le_bytes());
        buf[24..28].copy_from_slice(&self.morsel_row_count.to_le_bytes());
        buf[28..32].copy_from_slice(&self.column_count.to_le_bytes());
        buf[32..40].copy_from_slice(&self.morsel_directory_offset.to_le_bytes());
        buf[40..48].copy_from_slice(&self.column_directory_offset.to_le_bytes());
        buf[48..56].copy_from_slice(&self.page_index_offset.to_le_bytes());
        buf[56..64].copy_from_slice(&self.data_offset.to_le_bytes());
        buf[64..68].copy_from_slice(&self.flags.to_le_bytes());
        // [68..72] = checksum, zero during CRC.
        let crc = checksum::crc32c(&buf);
        buf[68..72].copy_from_slice(&crc.to_le_bytes());
        buf
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < TABLE_SEGMENT_HEADER_LEN {
            return Err(QfError::BufferTooShort);
        }
        let bytes = &bytes[..TABLE_SEGMENT_HEADER_LEN];
        let table_id = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let segment_id = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let row_start = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
        let row_count = u32::from_le_bytes(bytes[16..20].try_into().unwrap());
        let morsel_count = u32::from_le_bytes(bytes[20..24].try_into().unwrap());
        let morsel_row_count = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
        let column_count = u32::from_le_bytes(bytes[28..32].try_into().unwrap());
        let morsel_directory_offset = u64::from_le_bytes(bytes[32..40].try_into().unwrap());
        let column_directory_offset = u64::from_le_bytes(bytes[40..48].try_into().unwrap());
        let page_index_offset = u64::from_le_bytes(bytes[48..56].try_into().unwrap());
        let data_offset = u64::from_le_bytes(bytes[56..64].try_into().unwrap());
        let flags = u32::from_le_bytes(bytes[64..68].try_into().unwrap());
        let checksum_field = u32::from_le_bytes(bytes[68..72].try_into().unwrap());

        let mut for_crc = [0u8; TABLE_SEGMENT_HEADER_LEN];
        for_crc.copy_from_slice(bytes);
        for_crc[68..72].fill(0);
        if checksum::crc32c(&for_crc) != checksum_field {
            return Err(QfError::ChecksumMismatch);
        }

        Ok(Self {
            table_id,
            segment_id,
            row_start,
            row_count,
            morsel_count,
            morsel_row_count,
            column_count,
            morsel_directory_offset,
            column_directory_offset,
            page_index_offset,
            data_offset,
            flags,
            checksum: checksum_field,
        })
    }
}

// ── RowMorselEntryV1 (Spec §26) ──────────────────────────────────────────────

/// Encoded length of [`RowMorselEntryV1`].
///
/// Layout: morsel_id(4) + first_row_in_segment(4) + row_count(4)
///       + flags(4) + stats_ref(4) + checksum(4) = 24.
pub const ROW_MORSEL_ENTRY_LEN: usize = 24;

/// Spec §26 `RowMorselEntryV1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowMorselEntryV1 {
    pub morsel_id: u32,
    pub first_row_in_segment: u32,
    pub row_count: u32,
    pub flags: u32,
    pub stats_ref: u32,
    /// CRC32C of the 24-byte entry with `checksum` zeroed.
    pub checksum: u32,
}

impl RowMorselEntryV1 {
    pub fn serialize(&self) -> [u8; ROW_MORSEL_ENTRY_LEN] {
        let mut buf = [0u8; ROW_MORSEL_ENTRY_LEN];
        buf[0..4].copy_from_slice(&self.morsel_id.to_le_bytes());
        buf[4..8].copy_from_slice(&self.first_row_in_segment.to_le_bytes());
        buf[8..12].copy_from_slice(&self.row_count.to_le_bytes());
        buf[12..16].copy_from_slice(&self.flags.to_le_bytes());
        buf[16..20].copy_from_slice(&self.stats_ref.to_le_bytes());
        // [20..24] = checksum, zero during CRC.
        let crc = checksum::crc32c(&buf);
        buf[20..24].copy_from_slice(&crc.to_le_bytes());
        buf
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < ROW_MORSEL_ENTRY_LEN {
            return Err(QfError::BufferTooShort);
        }
        let bytes = &bytes[..ROW_MORSEL_ENTRY_LEN];
        let morsel_id = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let first_row_in_segment = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let row_count = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        let flags = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
        let stats_ref = u32::from_le_bytes(bytes[16..20].try_into().unwrap());
        let checksum_field = u32::from_le_bytes(bytes[20..24].try_into().unwrap());

        let mut for_crc = [0u8; ROW_MORSEL_ENTRY_LEN];
        for_crc.copy_from_slice(bytes);
        for_crc[20..24].fill(0);
        if checksum::crc32c(&for_crc) != checksum_field {
            return Err(QfError::ChecksumMismatch);
        }
        Ok(Self {
            morsel_id,
            first_row_in_segment,
            row_count,
            flags,
            stats_ref,
            checksum: checksum_field,
        })
    }
}

// ── RowMorselDirectory ───────────────────────────────────────────────────────

/// In-segment morsel directory: the array of `RowMorselEntryV1` listed
/// inside a segment payload (Spec §26).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RowMorselDirectory {
    pub entries: Vec<RowMorselEntryV1>,
}

impl RowMorselDirectory {
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.entries.len() * ROW_MORSEL_ENTRY_LEN);
        for e in &self.entries {
            out.extend_from_slice(&e.serialize());
        }
        out
    }

    /// Parse exactly `morsel_count` entries from the start of `bytes`.
    pub fn parse(bytes: &[u8], morsel_count: u32) -> Result<Self, QfError> {
        let needed = (morsel_count as usize)
            .checked_mul(ROW_MORSEL_ENTRY_LEN)
            .ok_or(QfError::ArithOverflow)?;
        if bytes.len() < needed {
            return Err(QfError::BufferTooShort);
        }
        let mut entries = Vec::with_capacity(morsel_count as usize);
        let mut pos = 0usize;
        for _ in 0..morsel_count {
            entries.push(RowMorselEntryV1::parse(
                &bytes[pos..pos + ROW_MORSEL_ENTRY_LEN],
            )?);
            pos += ROW_MORSEL_ENTRY_LEN;
        }
        let dir = Self { entries };
        dir.validate()?;
        Ok(dir)
    }

    /// Spec §26 invariants:
    /// * Morsels MUST be ordered by `first_row_in_segment`.
    /// * Row ranges MUST be contiguous and non-overlapping.
    /// * Every morsel except possibly the last has `row_count > 0`; an
    ///   empty morsel anywhere is a corruption.
    pub fn validate(&self) -> Result<(), QfError> {
        let mut next_row = self
            .entries
            .first()
            .map(|e| e.first_row_in_segment)
            .unwrap_or(0);
        for e in &self.entries {
            if e.row_count == 0 {
                return Err(QfError::SegmentCorrupt);
            }
            if e.first_row_in_segment != next_row {
                return Err(QfError::SegmentCorrupt);
            }
            next_row = next_row
                .checked_add(e.row_count)
                .ok_or(QfError::ArithOverflow)?;
        }
        Ok(())
    }

    /// Sum of `row_count` over all entries.
    pub fn sum_rows(&self) -> u64 {
        self.entries.iter().map(|e| e.row_count as u64).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(
        table: u32,
        segment: u32,
        row_start: u64,
        row_count: u32,
        morsels: u32,
    ) -> TableSegmentIndexEntryV1 {
        TableSegmentIndexEntryV1 {
            table_id: table,
            segment_id: segment,
            row_start,
            row_count,
            morsel_count: morsels,
            morsel_row_count: 4096,
            column_count: 3,
            offset: 0,
            length: 0,
            stats_ref: 0,
            flags: 0,
            checksum: 0,
        }
    }

    #[test]
    fn segment_index_entry_roundtrip_and_checksum() {
        let e = entry(1, 0, 0, 100, 1);
        let bytes = e.serialize();
        let e2 = TableSegmentIndexEntryV1::parse(&bytes).unwrap();
        assert_eq!(e2.table_id, 1);
        assert_eq!(e2.row_count, 100);
    }

    #[test]
    fn segment_index_entry_rejects_flipped_checksum() {
        let mut bytes = entry(1, 0, 0, 100, 1).serialize();
        bytes[56] ^= 0xFF;
        assert_eq!(
            TableSegmentIndexEntryV1::parse(&bytes),
            Err(QfError::ChecksumMismatch)
        );
    }

    #[test]
    fn segment_index_roundtrip_and_contiguous_validation() {
        let idx = TableSegmentIndex {
            flags: 0,
            entries: vec![entry(1, 0, 0, 100, 1), entry(1, 1, 100, 50, 1)],
        };
        let bytes = idx.serialize().unwrap();
        let parsed = TableSegmentIndex::parse(&bytes).unwrap();
        assert_eq!(parsed.entries.len(), 2);
    }

    #[test]
    fn segment_index_rejects_duplicate_segment_id_in_table() {
        let idx = TableSegmentIndex {
            flags: 0,
            entries: vec![entry(1, 0, 0, 100, 1), entry(1, 0, 100, 50, 1)],
        };
        let bytes = idx.serialize().unwrap();
        assert_eq!(
            TableSegmentIndex::parse(&bytes),
            Err(QfError::SegmentCorrupt)
        );
    }

    #[test]
    fn segment_index_rejects_overlap_within_table() {
        let idx = TableSegmentIndex {
            flags: 0,
            entries: vec![entry(1, 0, 0, 100, 1), entry(1, 1, 50, 100, 1)],
        };
        let bytes = idx.serialize().unwrap();
        assert_eq!(
            TableSegmentIndex::parse(&bytes),
            Err(QfError::SegmentCorrupt)
        );
    }

    #[test]
    fn segment_index_allows_distinct_tables_independently() {
        let idx = TableSegmentIndex {
            flags: 0,
            entries: vec![entry(1, 0, 0, 100, 1), entry(2, 0, 0, 50, 1)],
        };
        let bytes = idx.serialize().unwrap();
        assert!(TableSegmentIndex::parse(&bytes).is_ok());
    }

    #[test]
    fn segment_header_roundtrip_and_checksum() {
        let h = TableSegmentHeaderV1 {
            table_id: 1,
            segment_id: 2,
            row_start: 1000,
            row_count: 4096,
            morsel_count: 1,
            morsel_row_count: 4096,
            column_count: 5,
            morsel_directory_offset: 72,
            column_directory_offset: 1024,
            page_index_offset: 2048,
            data_offset: 4096,
            flags: 0,
            checksum: 0,
        };
        let bytes = h.serialize();
        let h2 = TableSegmentHeaderV1::parse(&bytes).unwrap();
        assert_eq!(h2.morsel_directory_offset, 72);
        assert_eq!(h2.row_count, 4096);
    }

    #[test]
    fn segment_header_rejects_flipped_checksum() {
        let mut bytes = TableSegmentHeaderV1 {
            table_id: 0,
            segment_id: 0,
            row_start: 0,
            row_count: 0,
            morsel_count: 0,
            morsel_row_count: 0,
            column_count: 0,
            morsel_directory_offset: 0,
            column_directory_offset: 0,
            page_index_offset: 0,
            data_offset: 0,
            flags: 0,
            checksum: 0,
        }
        .serialize();
        bytes[68] ^= 0xFF;
        assert_eq!(
            TableSegmentHeaderV1::parse(&bytes),
            Err(QfError::ChecksumMismatch)
        );
    }

    fn morsel(id: u32, first: u32, count: u32) -> RowMorselEntryV1 {
        RowMorselEntryV1 {
            morsel_id: id,
            first_row_in_segment: first,
            row_count: count,
            flags: 0,
            stats_ref: 0,
            checksum: 0,
        }
    }

    #[test]
    fn morsel_directory_roundtrip_and_validation() {
        let dir = RowMorselDirectory {
            entries: vec![morsel(0, 0, 4096), morsel(1, 4096, 512)],
        };
        let bytes = dir.serialize();
        let parsed = RowMorselDirectory::parse(&bytes, 2).unwrap();
        assert_eq!(parsed.entries.len(), 2);
        assert_eq!(parsed.sum_rows(), 4608);
    }

    #[test]
    fn morsel_directory_rejects_zero_row_count() {
        let dir = RowMorselDirectory {
            entries: vec![morsel(0, 0, 0)],
        };
        let bytes = dir.serialize();
        assert_eq!(
            RowMorselDirectory::parse(&bytes, 1),
            Err(QfError::SegmentCorrupt)
        );
    }

    #[test]
    fn morsel_directory_rejects_non_contiguous() {
        let dir = RowMorselDirectory {
            entries: vec![morsel(0, 0, 100), morsel(1, 200, 100)],
        };
        let bytes = dir.serialize();
        assert_eq!(
            RowMorselDirectory::parse(&bytes, 2),
            Err(QfError::SegmentCorrupt)
        );
    }

    #[test]
    fn morsel_directory_rejects_per_entry_checksum_corruption() {
        let dir = RowMorselDirectory {
            entries: vec![morsel(0, 0, 100)],
        };
        let mut bytes = dir.serialize();
        bytes[20] ^= 0xFF;
        assert_eq!(
            RowMorselDirectory::parse(&bytes, 1),
            Err(QfError::ChecksumMismatch)
        );
    }
}
