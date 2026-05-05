//! Spec §32 — Inverted morsel index.
//!
//! Maps a FileCode to the bit-set of morsel ids that contain at least one row
//! with that value. Useful for low-cardinality equality / `IN` predicates.

use crate::CoveError;

use super::{checked_region, verify_checksum_field};

pub const INVERTED_MORSEL_INDEX_HEADER_LEN: usize = 36;
pub const INVERTED_MORSEL_ENTRY_LEN: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum InvertedKeyKind {
    FileCode = 0,
    NumCode = 1,
}

impl InvertedKeyKind {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::FileCode),
            1 => Some(Self::NumCode),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvertedMorselIndexHeaderV1 {
    pub table_id: u32,
    pub column_id: u32,
    pub key_kind: InvertedKeyKind,
    pub flags: u8,
    pub representation: u8,
    pub reserved: u8,
    pub entry_count: u32,
    pub entries_offset: u64,
    pub bitmap_data_offset: u64,
    pub checksum: u32,
}

impl InvertedMorselIndexHeaderV1 {
    pub fn serialize(&self) -> [u8; INVERTED_MORSEL_INDEX_HEADER_LEN] {
        let mut out = [0u8; INVERTED_MORSEL_INDEX_HEADER_LEN];
        out[0..4].copy_from_slice(&self.table_id.to_le_bytes());
        out[4..8].copy_from_slice(&self.column_id.to_le_bytes());
        out[8] = self.key_kind as u8;
        out[9] = self.flags;
        out[10] = self.representation;
        out[11] = self.reserved;
        out[12..16].copy_from_slice(&self.entry_count.to_le_bytes());
        out[16..24].copy_from_slice(&self.entries_offset.to_le_bytes());
        out[24..32].copy_from_slice(&self.bitmap_data_offset.to_le_bytes());
        let crc = crate::checksum::crc32c(&out);
        out[32..36].copy_from_slice(&crc.to_le_bytes());
        out
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < INVERTED_MORSEL_INDEX_HEADER_LEN {
            return Err(CoveError::BufferTooShort);
        }
        let bytes = &bytes[..INVERTED_MORSEL_INDEX_HEADER_LEN];
        let checksum = verify_checksum_field(bytes, 32)?;
        let key_kind = InvertedKeyKind::from_u8(bytes[8]).ok_or(CoveError::BadIndex)?;
        if bytes[11] != 0 {
            return Err(CoveError::ReservedNotZero);
        }
        Ok(Self {
            table_id: u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
            column_id: u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            key_kind,
            flags: bytes[9],
            representation: bytes[10],
            reserved: bytes[11],
            entry_count: u32::from_le_bytes(bytes[12..16].try_into().unwrap()),
            entries_offset: u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
            bitmap_data_offset: u64::from_le_bytes(bytes[24..32].try_into().unwrap()),
            checksum,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvertedEntry {
    pub key: u64,
    pub morsel_bitmap_offset: u64,
    pub morsel_bitmap_length: u32,
    pub row_bitmap_offset: u64,
    pub row_bitmap_length: u32,
}

impl InvertedEntry {
    pub fn serialize(&self) -> [u8; INVERTED_MORSEL_ENTRY_LEN] {
        let mut out = [0u8; INVERTED_MORSEL_ENTRY_LEN];
        out[0..8].copy_from_slice(&self.key.to_le_bytes());
        out[8..16].copy_from_slice(&self.morsel_bitmap_offset.to_le_bytes());
        out[16..20].copy_from_slice(&self.morsel_bitmap_length.to_le_bytes());
        out[20..28].copy_from_slice(&self.row_bitmap_offset.to_le_bytes());
        out[28..32].copy_from_slice(&self.row_bitmap_length.to_le_bytes());
        out
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < INVERTED_MORSEL_ENTRY_LEN {
            return Err(CoveError::BufferTooShort);
        }
        Ok(Self {
            key: u64::from_le_bytes(bytes[0..8].try_into().unwrap()),
            morsel_bitmap_offset: u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
            morsel_bitmap_length: u32::from_le_bytes(bytes[16..20].try_into().unwrap()),
            row_bitmap_offset: u64::from_le_bytes(bytes[20..28].try_into().unwrap()),
            row_bitmap_length: u32::from_le_bytes(bytes[28..32].try_into().unwrap()),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvertedMorselIndex {
    pub header: InvertedMorselIndexHeaderV1,
    pub entries: Vec<InvertedEntry>,
    pub bitmap_data: Vec<u8>,
}

impl InvertedMorselIndex {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let header = InvertedMorselIndexHeaderV1::parse(bytes)?;
        let entries_len = (header.entry_count as u64)
            .checked_mul(INVERTED_MORSEL_ENTRY_LEN as u64)
            .ok_or(CoveError::ArithOverflow)?;
        let entry_bytes = checked_region(bytes, header.entries_offset, entries_len)?;
        let mut entries = Vec::with_capacity(header.entry_count as usize);
        let mut previous_key = None;
        for chunk in entry_bytes.chunks_exact(INVERTED_MORSEL_ENTRY_LEN) {
            let entry = InvertedEntry::parse(chunk)?;
            if let Some(previous) = previous_key {
                if entry.key <= previous {
                    return Err(CoveError::BadIndex);
                }
            }
            let morsel_start = header
                .bitmap_data_offset
                .checked_add(entry.morsel_bitmap_offset)
                .ok_or(CoveError::ArithOverflow)?;
            checked_region(bytes, morsel_start, entry.morsel_bitmap_length as u64)?;
            if entry.row_bitmap_length != 0 {
                let row_start = header
                    .bitmap_data_offset
                    .checked_add(entry.row_bitmap_offset)
                    .ok_or(CoveError::ArithOverflow)?;
                checked_region(bytes, row_start, entry.row_bitmap_length as u64)?;
            }
            previous_key = Some(entry.key);
            entries.push(entry);
        }
        let bitmap_data = if header.bitmap_data_offset <= bytes.len() as u64 {
            bytes[header.bitmap_data_offset as usize..].to_vec()
        } else {
            return Err(CoveError::OffsetRange);
        };
        Ok(Self {
            header,
            entries,
            bitmap_data,
        })
    }

    /// Inverse of [`Self::parse`]; produces canonical bytes that round-trip.
    pub fn serialize(&self) -> Vec<u8> {
        let mut header = self.header.clone();
        header.entry_count = self.entries.len() as u32;
        header.entries_offset = INVERTED_MORSEL_INDEX_HEADER_LEN as u64;
        header.bitmap_data_offset = (INVERTED_MORSEL_INDEX_HEADER_LEN
            + self.entries.len() * INVERTED_MORSEL_ENTRY_LEN)
            as u64;
        let mut out = header.serialize().to_vec();
        for entry in &self.entries {
            out.extend_from_slice(&entry.serialize());
        }
        out.extend_from_slice(&self.bitmap_data);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_two_entries() {
        let entries = [
            InvertedEntry {
                key: 5,
                morsel_bitmap_offset: 0,
                morsel_bitmap_length: 1,
                row_bitmap_offset: 0,
                row_bitmap_length: 0,
            },
            InvertedEntry {
                key: 7,
                morsel_bitmap_offset: 1,
                morsel_bitmap_length: 1,
                row_bitmap_offset: 0,
                row_bitmap_length: 0,
            },
        ];
        let header = InvertedMorselIndexHeaderV1 {
            table_id: 1,
            column_id: 1,
            key_kind: InvertedKeyKind::FileCode,
            flags: 0,
            representation: 0,
            reserved: 0,
            entry_count: entries.len() as u32,
            entries_offset: INVERTED_MORSEL_INDEX_HEADER_LEN as u64,
            bitmap_data_offset: (INVERTED_MORSEL_INDEX_HEADER_LEN
                + entries.len() * INVERTED_MORSEL_ENTRY_LEN) as u64,
            checksum: 0,
        };
        let mut bytes = header.serialize().to_vec();
        for entry in entries {
            bytes.extend_from_slice(&entry.serialize());
        }
        bytes.extend_from_slice(&[0b0000_1001, 0b0000_0100]);
        let idx = InvertedMorselIndex::parse(&bytes).unwrap();
        assert_eq!(idx.entries[0].key, 5);
        assert_eq!(idx.entries[1].key, 7);
    }

    #[test]
    fn checksum_mismatch_rejected() {
        let header = InvertedMorselIndexHeaderV1 {
            table_id: 1,
            column_id: 1,
            key_kind: InvertedKeyKind::FileCode,
            flags: 0,
            representation: 0,
            reserved: 0,
            entry_count: 0,
            entries_offset: INVERTED_MORSEL_INDEX_HEADER_LEN as u64,
            bitmap_data_offset: INVERTED_MORSEL_INDEX_HEADER_LEN as u64,
            checksum: 0,
        };
        let mut bytes = header.serialize().to_vec();
        bytes[32] ^= 0xff;
        assert_eq!(
            InvertedMorselIndex::parse(&bytes),
            Err(CoveError::ChecksumMismatch)
        );
    }

    #[test]
    fn unsorted_keys_rejected() {
        let entries = [
            InvertedEntry {
                key: 7,
                morsel_bitmap_offset: 0,
                morsel_bitmap_length: 1,
                row_bitmap_offset: 0,
                row_bitmap_length: 0,
            },
            InvertedEntry {
                key: 5,
                morsel_bitmap_offset: 0,
                morsel_bitmap_length: 1,
                row_bitmap_offset: 0,
                row_bitmap_length: 0,
            },
        ];
        let header = InvertedMorselIndexHeaderV1 {
            table_id: 1,
            column_id: 1,
            key_kind: InvertedKeyKind::FileCode,
            flags: 0,
            representation: 0,
            reserved: 0,
            entry_count: entries.len() as u32,
            entries_offset: INVERTED_MORSEL_INDEX_HEADER_LEN as u64,
            bitmap_data_offset: (INVERTED_MORSEL_INDEX_HEADER_LEN
                + entries.len() * INVERTED_MORSEL_ENTRY_LEN) as u64,
            checksum: 0,
        };
        let mut bytes = header.serialize().to_vec();
        for entry in entries {
            bytes.extend_from_slice(&entry.serialize());
        }
        bytes.push(0);
        assert_eq!(InvertedMorselIndex::parse(&bytes), Err(CoveError::BadIndex));
    }

    #[test]
    fn serialize_round_trip_full_index() {
        let entries = vec![
            InvertedEntry {
                key: 5,
                morsel_bitmap_offset: 0,
                morsel_bitmap_length: 1,
                row_bitmap_offset: 0,
                row_bitmap_length: 0,
            },
            InvertedEntry {
                key: 7,
                morsel_bitmap_offset: 1,
                morsel_bitmap_length: 1,
                row_bitmap_offset: 0,
                row_bitmap_length: 0,
            },
        ];
        let header = InvertedMorselIndexHeaderV1 {
            table_id: 1,
            column_id: 1,
            key_kind: InvertedKeyKind::FileCode,
            flags: 0,
            representation: 0,
            reserved: 0,
            entry_count: entries.len() as u32,
            entries_offset: INVERTED_MORSEL_INDEX_HEADER_LEN as u64,
            bitmap_data_offset: (INVERTED_MORSEL_INDEX_HEADER_LEN
                + entries.len() * INVERTED_MORSEL_ENTRY_LEN) as u64,
            checksum: 0,
        };
        let idx = InvertedMorselIndex {
            header,
            entries,
            bitmap_data: vec![0b0000_1001, 0b0000_0100],
        };
        let bytes = idx.serialize();
        let parsed = InvertedMorselIndex::parse(&bytes).unwrap();
        assert_eq!(parsed.entries.len(), 2);
        assert_eq!(parsed.entries[0].key, 5);
        assert_eq!(parsed.bitmap_data, idx.bitmap_data);
    }
}
