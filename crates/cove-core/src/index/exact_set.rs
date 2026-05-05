//! Spec §30 — ExactSet index.
//!
//! Exact membership index over a column's FileCodes. The index is sorted and
//! supports binary search; corruption falls back to scan (Spec §73).

use crate::CoveError;

use super::{checked_region, verify_checksum_field};

pub const EXACT_SET_HEADER_LEN: usize = 36;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ExactSetGranularity {
    Segment = 0,
    Morsel = 1,
}

impl ExactSetGranularity {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Segment),
            1 => Some(Self::Morsel),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ExactSetKeyKind {
    FileCode = 0,
    NumCode = 1,
    CanonicalHash = 2,
}

impl ExactSetKeyKind {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::FileCode),
            1 => Some(Self::NumCode),
            2 => Some(Self::CanonicalHash),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ExactSetRepresentation {
    SortedList = 0,
    Bitset = 1,
    RoaringLike = 2,
}

impl ExactSetRepresentation {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::SortedList),
            1 => Some(Self::Bitset),
            2 => Some(Self::RoaringLike),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactSetIndexHeaderV1 {
    pub table_id: u32,
    pub column_id: u32,
    pub granularity: ExactSetGranularity,
    pub key_kind: ExactSetKeyKind,
    pub representation: ExactSetRepresentation,
    pub flags: u8,
    pub entry_count: u32,
    pub data_offset: u64,
    pub data_length: u64,
    pub checksum: u32,
}

impl ExactSetIndexHeaderV1 {
    pub fn serialize(&self) -> [u8; EXACT_SET_HEADER_LEN] {
        let mut out = [0u8; EXACT_SET_HEADER_LEN];
        out[0..4].copy_from_slice(&self.table_id.to_le_bytes());
        out[4..8].copy_from_slice(&self.column_id.to_le_bytes());
        out[8] = self.granularity as u8;
        out[9] = self.key_kind as u8;
        out[10] = self.representation as u8;
        out[11] = self.flags;
        out[12..16].copy_from_slice(&self.entry_count.to_le_bytes());
        out[16..24].copy_from_slice(&self.data_offset.to_le_bytes());
        out[24..32].copy_from_slice(&self.data_length.to_le_bytes());
        let crc = crate::checksum::crc32c(&out);
        out[32..36].copy_from_slice(&crc.to_le_bytes());
        out
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < EXACT_SET_HEADER_LEN {
            return Err(CoveError::BufferTooShort);
        }
        let bytes = &bytes[..EXACT_SET_HEADER_LEN];
        let checksum = verify_checksum_field(bytes, 32)?;
        let granularity = ExactSetGranularity::from_u8(bytes[8]).ok_or(CoveError::BadIndex)?;
        let key_kind = ExactSetKeyKind::from_u8(bytes[9]).ok_or(CoveError::BadIndex)?;
        let representation =
            ExactSetRepresentation::from_u8(bytes[10]).ok_or(CoveError::BadIndex)?;
        Ok(Self {
            table_id: u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
            column_id: u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            granularity,
            key_kind,
            representation,
            flags: bytes[11],
            entry_count: u32::from_le_bytes(bytes[12..16].try_into().unwrap()),
            data_offset: u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
            data_length: u64::from_le_bytes(bytes[24..32].try_into().unwrap()),
            checksum,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactSetIndex {
    pub header: ExactSetIndexHeaderV1,
    /// Sorted unique keys for representation 0; other representations keep raw data only.
    pub keys: Vec<u64>,
    pub data: Vec<u8>,
}

impl ExactSetIndex {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let header = ExactSetIndexHeaderV1::parse(bytes)?;
        let data = checked_region(bytes, header.data_offset, header.data_length)?;
        let mut keys = Vec::new();
        if header.representation == ExactSetRepresentation::SortedList {
            let expected_len = (header.entry_count as usize)
                .checked_mul(8)
                .ok_or(CoveError::ArithOverflow)?;
            if data.len() != expected_len {
                return Err(CoveError::BadIndex);
            }
            keys.reserve(header.entry_count as usize);
            for chunk in data.chunks_exact(8) {
                keys.push(u64::from_le_bytes(chunk.try_into().unwrap()));
            }
            for pair in keys.windows(2) {
                if pair[0] >= pair[1] {
                    return Err(CoveError::BadIndex);
                }
            }
        }
        Ok(Self {
            header,
            keys,
            data: data.to_vec(),
        })
    }

    /// O(log n) membership test.
    pub fn contains(&self, key: u64) -> bool {
        self.keys.binary_search(&key).is_ok()
    }

    /// Inverse of [`Self::parse`]; produces canonical bytes that round-trip.
    /// Recomputes header `data_offset`/`data_length`/`checksum` so the caller
    /// only needs to set semantic fields.
    pub fn serialize(&self) -> Vec<u8> {
        let mut header = self.header.clone();
        header.data_offset = EXACT_SET_HEADER_LEN as u64;
        header.data_length = self.data.len() as u64;
        let mut out = header.serialize().to_vec();
        out.extend_from_slice(&self.data);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(column_id: u32, keys: &[u64]) -> Vec<u8> {
        let mut data = Vec::new();
        for key in keys {
            data.extend_from_slice(&key.to_le_bytes());
        }
        let header = ExactSetIndexHeaderV1 {
            table_id: 1,
            column_id,
            granularity: ExactSetGranularity::Morsel,
            key_kind: ExactSetKeyKind::FileCode,
            representation: ExactSetRepresentation::SortedList,
            flags: 0,
            entry_count: keys.len() as u32,
            data_offset: EXACT_SET_HEADER_LEN as u64,
            data_length: data.len() as u64,
            checksum: 0,
        };
        let mut out = header.serialize().to_vec();
        out.extend_from_slice(&data);
        out
    }

    #[test]
    fn binary_search_membership() {
        let i = ExactSetIndex::parse(&build(1, &[2, 5, 9])).unwrap();
        assert!(i.contains(5));
        assert!(!i.contains(7));
    }

    #[test]
    fn checksum_mismatch_rejected() {
        let mut bytes = build(1, &[2, 5, 9]);
        bytes[32] ^= 0xff;
        assert_eq!(
            ExactSetIndex::parse(&bytes),
            Err(CoveError::ChecksumMismatch)
        );
    }

    #[test]
    fn unsorted_rejected() {
        assert_eq!(
            ExactSetIndex::parse(&build(0, &[5, 2])),
            Err(CoveError::BadIndex)
        );
    }

    #[test]
    fn duplicate_rejected() {
        assert_eq!(
            ExactSetIndex::parse(&build(0, &[5, 5])),
            Err(CoveError::BadIndex)
        );
    }

    #[test]
    fn serialize_round_trip_full_index() {
        let bytes = build(7, &[1, 4, 9, 16]);
        let parsed = ExactSetIndex::parse(&bytes).unwrap();
        let bytes2 = parsed.serialize();
        let parsed2 = ExactSetIndex::parse(&bytes2).unwrap();
        assert_eq!(parsed, parsed2);
        assert_eq!(parsed2.keys, vec![1, 4, 9, 16]);
    }
}
