//! Spec §31 — Bloom filter index.
//!
//! Probabilistic membership test: returns `true` (may contain) or `false`
//! (definitely does not contain). False positives are allowed; false
//! negatives are forbidden. The index falls back to scan on corruption.
//!
//! The hash function is the canonical 64-bit FNV-1a applied to the *canonical
//! value bytes* (Spec §17), not raw FileCodes — this lets the same filter
//! survive dictionary re-encoding and FileCode reassignment.

use crate::CoveError;

use super::{checked_region, verify_checksum_field};

pub const BLOOM_INDEX_HEADER_LEN: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BloomGranularity {
    Segment = 0,
    Morsel = 1,
}

impl BloomGranularity {
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
pub enum BloomHashDomain {
    FileCode = 0,
    NumCode = 1,
    CanonicalValueHash = 2,
}

impl BloomHashDomain {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::FileCode),
            1 => Some(Self::NumCode),
            2 => Some(Self::CanonicalValueHash),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BloomAlgorithm {
    SplitBlock = 0,
}

impl BloomAlgorithm {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::SplitBlock),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BloomIndexHeaderV1 {
    pub table_id: u32,
    pub column_id: u32,
    pub granularity: BloomGranularity,
    pub hash_domain: BloomHashDomain,
    pub algorithm: BloomAlgorithm,
    pub flags: u8,
    pub target_fpr_ppm: u32,
    pub filter_count: u32,
    pub data_offset: u64,
    pub data_length: u64,
    pub checksum: u32,
}

impl BloomIndexHeaderV1 {
    pub fn serialize(&self) -> [u8; BLOOM_INDEX_HEADER_LEN] {
        let mut out = [0u8; BLOOM_INDEX_HEADER_LEN];
        out[0..4].copy_from_slice(&self.table_id.to_le_bytes());
        out[4..8].copy_from_slice(&self.column_id.to_le_bytes());
        out[8] = self.granularity as u8;
        out[9] = self.hash_domain as u8;
        out[10] = self.algorithm as u8;
        out[11] = self.flags;
        out[12..16].copy_from_slice(&self.target_fpr_ppm.to_le_bytes());
        out[16..20].copy_from_slice(&self.filter_count.to_le_bytes());
        out[20..28].copy_from_slice(&self.data_offset.to_le_bytes());
        out[28..36].copy_from_slice(&self.data_length.to_le_bytes());
        let crc = crate::checksum::crc32c(&out);
        out[36..40].copy_from_slice(&crc.to_le_bytes());
        out
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < BLOOM_INDEX_HEADER_LEN {
            return Err(CoveError::BufferTooShort);
        }
        let bytes = &bytes[..BLOOM_INDEX_HEADER_LEN];
        let checksum = verify_checksum_field(bytes, 36)?;
        let granularity = BloomGranularity::from_u8(bytes[8]).ok_or(CoveError::BadIndex)?;
        let hash_domain = BloomHashDomain::from_u8(bytes[9]).ok_or(CoveError::BadIndex)?;
        let algorithm = BloomAlgorithm::from_u8(bytes[10]).ok_or(CoveError::BadIndex)?;
        Ok(Self {
            table_id: u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
            column_id: u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            granularity,
            hash_domain,
            algorithm,
            flags: bytes[11],
            target_fpr_ppm: u32::from_le_bytes(bytes[12..16].try_into().unwrap()),
            filter_count: u32::from_le_bytes(bytes[16..20].try_into().unwrap()),
            data_offset: u64::from_le_bytes(bytes[20..28].try_into().unwrap()),
            data_length: u64::from_le_bytes(bytes[28..36].try_into().unwrap()),
            checksum,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BloomFilterIndex {
    pub header: BloomIndexHeaderV1,
    pub hash_count: u8,
    pub bits: Vec<u8>,
}

impl BloomFilterIndex {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let header = BloomIndexHeaderV1::parse(bytes)?;
        let bits = checked_region(bytes, header.data_offset, header.data_length)?;
        if header.filter_count == 0 || bits.is_empty() {
            return Err(CoveError::BadIndex);
        }
        Ok(Self {
            header,
            hash_count: 7,
            bits: bits.to_vec(),
        })
    }

    /// Insert a canonical-value byte slice into the filter.
    pub fn insert(&mut self, value: &[u8]) {
        let nbits = (self.bits.len() as u64) * 8;
        if nbits == 0 {
            return;
        }
        for i in 0..self.hash_count {
            let bit = double_hash(value, i) % nbits;
            self.bits[(bit / 8) as usize] |= 1 << (bit % 8);
        }
    }

    /// Test membership. Returns `true` for "possibly contains".
    pub fn might_contain(&self, value: &[u8]) -> bool {
        let nbits = (self.bits.len() as u64) * 8;
        if nbits == 0 {
            return false;
        }
        for i in 0..self.hash_count {
            let bit = double_hash(value, i) % nbits;
            if self.bits[(bit / 8) as usize] & (1 << (bit % 8)) == 0 {
                return false;
            }
        }
        true
    }

    /// Inverse of [`Self::parse`]; produces canonical bytes that round-trip.
    pub fn serialize(&self) -> Vec<u8> {
        let mut header = self.header.clone();
        header.data_offset = BLOOM_INDEX_HEADER_LEN as u64;
        header.data_length = self.bits.len() as u64;
        let mut out = header.serialize().to_vec();
        out.extend_from_slice(&self.bits);
        out
    }
}

fn fnv1a64(bytes: &[u8], seed: u64) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64 ^ seed;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn double_hash(value: &[u8], i: u8) -> u64 {
    let h1 = fnv1a64(value, 0);
    let h2 = fnv1a64(value, 0xdead_beef);
    h1.wrapping_add((i as u64).wrapping_mul(h2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bloom_no_false_negatives() {
        let mut b = BloomFilterIndex {
            header: BloomIndexHeaderV1 {
                table_id: 1,
                column_id: 0,
                granularity: BloomGranularity::Morsel,
                hash_domain: BloomHashDomain::CanonicalValueHash,
                algorithm: BloomAlgorithm::SplitBlock,
                flags: 0,
                target_fpr_ppm: 10_000,
                filter_count: 1,
                data_offset: BLOOM_INDEX_HEADER_LEN as u64,
                data_length: 32,
                checksum: 0,
            },
            hash_count: 4,
            bits: vec![0u8; 32],
        };
        for v in &[b"alice".as_ref(), b"bob".as_ref(), b"carol".as_ref()] {
            b.insert(v);
        }
        assert!(b.might_contain(b"alice"));
        assert!(b.might_contain(b"bob"));
        assert!(b.might_contain(b"carol"));
    }

    #[test]
    fn parses_headered_filter() {
        let bits = [0xA5u8; 8];
        let header = BloomIndexHeaderV1 {
            table_id: 1,
            column_id: 2,
            granularity: BloomGranularity::Segment,
            hash_domain: BloomHashDomain::FileCode,
            algorithm: BloomAlgorithm::SplitBlock,
            flags: 0,
            target_fpr_ppm: 10_000,
            filter_count: 1,
            data_offset: BLOOM_INDEX_HEADER_LEN as u64,
            data_length: bits.len() as u64,
            checksum: 0,
        };
        let mut bytes = header.serialize().to_vec();
        bytes.extend_from_slice(&bits);
        let parsed = BloomFilterIndex::parse(&bytes).unwrap();
        assert_eq!(parsed.header.column_id, 2);
        assert_eq!(parsed.bits, bits);
    }

    #[test]
    fn rejects_checksum_mismatch() {
        let bits = [0u8; 8];
        let header = BloomIndexHeaderV1 {
            table_id: 1,
            column_id: 2,
            granularity: BloomGranularity::Segment,
            hash_domain: BloomHashDomain::FileCode,
            algorithm: BloomAlgorithm::SplitBlock,
            flags: 0,
            target_fpr_ppm: 10_000,
            filter_count: 1,
            data_offset: BLOOM_INDEX_HEADER_LEN as u64,
            data_length: bits.len() as u64,
            checksum: 0,
        };
        let mut bytes = header.serialize().to_vec();
        bytes[36] ^= 0xff;
        bytes.extend_from_slice(&bits);
        assert_eq!(
            BloomFilterIndex::parse(&bytes),
            Err(CoveError::ChecksumMismatch)
        );
    }

    #[test]
    fn rejects_zero_filter_count() {
        let bits = [0u8; 8];
        let header = BloomIndexHeaderV1 {
            table_id: 1,
            column_id: 2,
            granularity: BloomGranularity::Segment,
            hash_domain: BloomHashDomain::FileCode,
            algorithm: BloomAlgorithm::SplitBlock,
            flags: 0,
            target_fpr_ppm: 10_000,
            filter_count: 0,
            data_offset: BLOOM_INDEX_HEADER_LEN as u64,
            data_length: bits.len() as u64,
            checksum: 0,
        };
        let mut bytes = header.serialize().to_vec();
        bytes.extend_from_slice(&bits);
        assert_eq!(BloomFilterIndex::parse(&bytes), Err(CoveError::BadIndex));
    }

    #[test]
    fn serialize_round_trip_full_index() {
        let header = BloomIndexHeaderV1 {
            table_id: 1,
            column_id: 2,
            granularity: BloomGranularity::Morsel,
            hash_domain: BloomHashDomain::CanonicalValueHash,
            algorithm: BloomAlgorithm::SplitBlock,
            flags: 0,
            target_fpr_ppm: 10_000,
            filter_count: 1,
            data_offset: BLOOM_INDEX_HEADER_LEN as u64,
            data_length: 16,
            checksum: 0,
        };
        let mut idx = BloomFilterIndex {
            header,
            hash_count: 7,
            bits: vec![0u8; 16],
        };
        idx.insert(b"alpha");
        idx.insert(b"omega");
        let bytes = idx.serialize();
        let parsed = BloomFilterIndex::parse(&bytes).unwrap();
        assert!(parsed.might_contain(b"alpha"));
        assert!(parsed.might_contain(b"omega"));
        assert_eq!(parsed.bits, idx.bits);
    }
}
