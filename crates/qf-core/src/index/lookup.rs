//! Spec §33 — Lookup index (point access by FileCode → row reference).
//!
//! Maps each FileCode in the indexed scope to one or more row references.
//! Used for primary-key style lookups and join build sides.

use crate::{row_ref::RowRef, QfError};

use super::{checked_region, verify_checksum_field};

pub const LOOKUP_INDEX_HEADER_LEN: usize = 56;
pub const LOOKUP_INDEX_ENTRY_LEN: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum LookupKeyKind {
    FileCode = 0,
    NumCode = 1,
    CanonicalHash = 2,
    FixedBytes = 3,
}

impl LookupKeyKind {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::FileCode),
            1 => Some(Self::NumCode),
            2 => Some(Self::CanonicalHash),
            3 => Some(Self::FixedBytes),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum LookupIndexKind {
    Hash = 0,
    SparseSorted = 1,
    MinimalPerfectHash = 2,
}

impl LookupIndexKind {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Hash),
            1 => Some(Self::SparseSorted),
            2 => Some(Self::MinimalPerfectHash),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum LookupUniqueness {
    Unknown = 0,
    Unique = 1,
    NonUnique = 2,
}

impl LookupUniqueness {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Unknown),
            1 => Some(Self::Unique),
            2 => Some(Self::NonUnique),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LookupIndexHeaderV1 {
    pub table_id: u32,
    pub column_id: u32,
    pub key_kind: LookupKeyKind,
    pub index_kind: LookupIndexKind,
    pub uniqueness: LookupUniqueness,
    pub flags: u8,
    pub entry_count: u64,
    pub entries_offset: u64,
    pub entries_length: u64,
    pub rowref_offset: u64,
    pub rowref_length: u64,
    pub checksum: u32,
}

impl LookupIndexHeaderV1 {
    pub fn serialize(&self) -> [u8; LOOKUP_INDEX_HEADER_LEN] {
        let mut out = [0u8; LOOKUP_INDEX_HEADER_LEN];
        out[0..4].copy_from_slice(&self.table_id.to_le_bytes());
        out[4..8].copy_from_slice(&self.column_id.to_le_bytes());
        out[8] = self.key_kind as u8;
        out[9] = self.index_kind as u8;
        out[10] = self.uniqueness as u8;
        out[11] = self.flags;
        out[12..20].copy_from_slice(&self.entry_count.to_le_bytes());
        out[20..28].copy_from_slice(&self.entries_offset.to_le_bytes());
        out[28..36].copy_from_slice(&self.entries_length.to_le_bytes());
        out[36..44].copy_from_slice(&self.rowref_offset.to_le_bytes());
        out[44..52].copy_from_slice(&self.rowref_length.to_le_bytes());
        let crc = crate::checksum::crc32c(&out);
        out[52..56].copy_from_slice(&crc.to_le_bytes());
        out
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < LOOKUP_INDEX_HEADER_LEN {
            return Err(QfError::BufferTooShort);
        }
        let bytes = &bytes[..LOOKUP_INDEX_HEADER_LEN];
        let checksum = verify_checksum_field(bytes, 52)?;
        Ok(Self {
            table_id: u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
            column_id: u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            key_kind: LookupKeyKind::from_u8(bytes[8]).ok_or(QfError::BadIndex)?,
            index_kind: LookupIndexKind::from_u8(bytes[9]).ok_or(QfError::BadIndex)?,
            uniqueness: LookupUniqueness::from_u8(bytes[10]).ok_or(QfError::BadIndex)?,
            flags: bytes[11],
            entry_count: u64::from_le_bytes(bytes[12..20].try_into().unwrap()),
            entries_offset: u64::from_le_bytes(bytes[20..28].try_into().unwrap()),
            entries_length: u64::from_le_bytes(bytes[28..36].try_into().unwrap()),
            rowref_offset: u64::from_le_bytes(bytes[36..44].try_into().unwrap()),
            rowref_length: u64::from_le_bytes(bytes[44..52].try_into().unwrap()),
            checksum,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LookupEntry {
    pub key: u64,
    pub rows: Vec<RowRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LookupIndex {
    pub header: LookupIndexHeaderV1,
    pub entries: Vec<LookupEntry>,
}

impl LookupIndex {
    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        let header = LookupIndexHeaderV1::parse(bytes)?;
        let entries_bytes = checked_region(bytes, header.entries_offset, header.entries_length)?;
        let rowref_bytes = checked_region(bytes, header.rowref_offset, header.rowref_length)?;
        let expected_entries_len = header
            .entry_count
            .checked_mul(LOOKUP_INDEX_ENTRY_LEN as u64)
            .ok_or(QfError::ArithOverflow)?;
        if header.entries_length != expected_entries_len
            || rowref_bytes.len() % RowRef::ENCODED_LEN != 0
        {
            return Err(QfError::BadIndex);
        }

        let mut entries = Vec::with_capacity(header.entry_count as usize);
        let mut previous_key = None;
        for chunk in entries_bytes.chunks_exact(LOOKUP_INDEX_ENTRY_LEN) {
            let key = u64::from_le_bytes(chunk[0..8].try_into().unwrap());
            if let Some(previous) = previous_key {
                if key <= previous {
                    return Err(QfError::BadIndex);
                }
            }
            previous_key = Some(key);
            let rowref_start = u32::from_le_bytes(chunk[8..12].try_into().unwrap()) as usize;
            let rowref_count = u32::from_le_bytes(chunk[12..16].try_into().unwrap()) as usize;
            let start = rowref_start
                .checked_mul(RowRef::ENCODED_LEN)
                .ok_or(QfError::ArithOverflow)?;
            let len = rowref_count
                .checked_mul(RowRef::ENCODED_LEN)
                .ok_or(QfError::ArithOverflow)?;
            let end = start.checked_add(len).ok_or(QfError::ArithOverflow)?;
            if end > rowref_bytes.len() {
                return Err(QfError::BadIndex);
            }
            let mut rows = Vec::with_capacity(rowref_count);
            for row_bytes in rowref_bytes[start..end].chunks_exact(RowRef::ENCODED_LEN) {
                rows.push(RowRef::decode(row_bytes)?);
            }
            entries.push(LookupEntry { key, rows });
        }
        Ok(Self { header, entries })
    }

    pub fn rows_for(&self, key: u64) -> Option<&[RowRef]> {
        self.entries
            .binary_search_by_key(&key, |entry| entry.key)
            .ok()
            .map(|i| self.entries[i].rows.as_slice())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(col: u32, entries: &[(u64, &[RowRef])]) -> Vec<u8> {
        let mut entry_bytes = Vec::new();
        let mut rowref_bytes = Vec::new();
        let mut rowref_start = 0u32;
        for (c, rows) in entries {
            entry_bytes.extend_from_slice(&c.to_le_bytes());
            entry_bytes.extend_from_slice(&rowref_start.to_le_bytes());
            entry_bytes.extend_from_slice(&(rows.len() as u32).to_le_bytes());
            for r in *rows {
                rowref_bytes.extend_from_slice(&r.encode());
            }
            rowref_start += rows.len() as u32;
        }
        let rowref_offset = LOOKUP_INDEX_HEADER_LEN + entry_bytes.len();
        let header = LookupIndexHeaderV1 {
            table_id: 1,
            column_id: col,
            key_kind: LookupKeyKind::FileCode,
            index_kind: LookupIndexKind::SparseSorted,
            uniqueness: LookupUniqueness::NonUnique,
            flags: 0,
            entry_count: entries.len() as u64,
            entries_offset: LOOKUP_INDEX_HEADER_LEN as u64,
            entries_length: entry_bytes.len() as u64,
            rowref_offset: rowref_offset as u64,
            rowref_length: rowref_bytes.len() as u64,
            checksum: 0,
        };
        let mut out = header.serialize().to_vec();
        out.extend_from_slice(&entry_bytes);
        out.extend_from_slice(&rowref_bytes);
        out
    }

    #[test]
    fn binary_search_round_trip() {
        let r = RowRef {
            table_id: 1,
            segment_id: 0,
            morsel_id: 0,
            row_in_morsel: 42,
        };
        let bytes = build(0, &[(2, &[r]), (5, &[r])]);
        let idx = LookupIndex::parse(&bytes).unwrap();
        assert_eq!(idx.rows_for(5).unwrap()[0], r);
        assert!(idx.rows_for(99).is_none());
    }

    #[test]
    fn unsorted_codes_rejected() {
        let r = RowRef {
            table_id: 0,
            segment_id: 0,
            morsel_id: 0,
            row_in_morsel: 0,
        };
        let bytes = build(0, &[(5, &[r]), (2, &[r])]);
        assert_eq!(LookupIndex::parse(&bytes), Err(QfError::BadIndex));
    }

    #[test]
    fn checksum_mismatch_rejected() {
        let r = RowRef {
            table_id: 0,
            segment_id: 0,
            morsel_id: 0,
            row_in_morsel: 0,
        };
        let mut bytes = build(0, &[(5, &[r])]);
        bytes[52] ^= 0xff;
        assert_eq!(LookupIndex::parse(&bytes), Err(QfError::ChecksumMismatch));
    }
}
