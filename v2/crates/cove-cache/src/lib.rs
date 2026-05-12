//! COVE-CACHE local coverage-cache diagnostics for COVE v2.

use std::collections::BTreeSet;

use cove_core::{checksum, CoveError};
use cove_coverage::{CoverageExactnessV2, CoverageGranularityV2, CoverageProofStrengthV2};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoveCoverageCacheHeaderV2 {
    pub cache_format_namespace_ref: u32,
    pub cache_format_version_major: u16,
    pub cache_format_version_minor: u16,
    pub flags: u32,
    pub cache_id: [u8; 16],
    pub dataset_id: [u8; 16],
    pub snapshot_id: [u8; 16],
    pub entry_count: u32,
    pub created_at_us: i64,
    pub producer_engine_ref: u32,
    pub reserved: [u8; 32],
    pub checksum: u32,
}

impl CoveCoverageCacheHeaderV2 {
    pub const LEN: usize = 112;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let header = Self {
            cache_format_namespace_ref: read_u32(bytes, 0)?,
            cache_format_version_major: read_u16(bytes, 4)?,
            cache_format_version_minor: read_u16(bytes, 6)?,
            flags: read_u32(bytes, 8)?,
            cache_id: read_array(bytes, 12)?,
            dataset_id: read_array(bytes, 28)?,
            snapshot_id: read_array(bytes, 44)?,
            entry_count: read_u32(bytes, 60)?,
            created_at_us: read_i64(bytes, 64)?,
            producer_engine_ref: read_u32(bytes, 72)?,
            reserved: read_array(bytes, 76)?,
            checksum: read_u32(bytes, 108)?,
        };
        if header.reserved.iter().any(|byte| *byte != 0) {
            return Err(CoveError::ReservedNotZero);
        }
        verify_crc(&bytes[..Self::LEN], 108, header.checksum)?;
        Ok(header)
    }

    pub fn serialize(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.cache_format_namespace_ref.to_le_bytes());
        out[4..6].copy_from_slice(&self.cache_format_version_major.to_le_bytes());
        out[6..8].copy_from_slice(&self.cache_format_version_minor.to_le_bytes());
        out[8..12].copy_from_slice(&self.flags.to_le_bytes());
        out[12..28].copy_from_slice(&self.cache_id);
        out[28..44].copy_from_slice(&self.dataset_id);
        out[44..60].copy_from_slice(&self.snapshot_id);
        out[60..64].copy_from_slice(&self.entry_count.to_le_bytes());
        out[64..72].copy_from_slice(&self.created_at_us.to_le_bytes());
        out[72..76].copy_from_slice(&self.producer_engine_ref.to_le_bytes());
        out[76..108].copy_from_slice(&self.reserved);
        let crc = checksum::crc32c(&out);
        out[108..112].copy_from_slice(&crc.to_le_bytes());
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageCacheEntryV2 {
    pub entry_id: u64,
    pub dataset_id: [u8; 16],
    pub snapshot_id: [u8; 16],
    pub predicate_normal_form_ref: u32,
    pub interval_normal_form_ref: u32,
    pub coverage_set_ref: u32,
    pub coverage_granularity: CoverageGranularityV2,
    pub proof_strength: CoverageProofStrengthV2,
    pub exactness: CoverageExactnessV2,
    pub flags: u8,
    pub actual_coverage_size_bytes: u64,
    pub actual_read_cost_ns: u64,
    pub created_at_us: i64,
    pub valid_until_snapshot_ref: u32,
    pub producer_engine_ref: u32,
    pub checksum: u32,
}

impl CoverageCacheEntryV2 {
    pub const LEN: usize = 92;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let entry = Self {
            entry_id: read_u64(bytes, 0)?,
            dataset_id: read_array(bytes, 8)?,
            snapshot_id: read_array(bytes, 24)?,
            predicate_normal_form_ref: read_u32(bytes, 40)?,
            interval_normal_form_ref: read_u32(bytes, 44)?,
            coverage_set_ref: read_u32(bytes, 48)?,
            coverage_granularity: CoverageGranularityV2::from_u8(read_u8(bytes, 52)?)
                .ok_or(CoveError::CacheStale)?,
            proof_strength: CoverageProofStrengthV2::from_u8(read_u8(bytes, 53)?)
                .ok_or(CoveError::CacheStale)?,
            exactness: CoverageExactnessV2::from_u8(read_u8(bytes, 54)?)
                .ok_or(CoveError::CacheStale)?,
            flags: read_u8(bytes, 55)?,
            actual_coverage_size_bytes: read_u64(bytes, 56)?,
            actual_read_cost_ns: read_u64(bytes, 64)?,
            created_at_us: read_i64(bytes, 72)?,
            valid_until_snapshot_ref: read_u32(bytes, 80)?,
            producer_engine_ref: read_u32(bytes, 84)?,
            checksum: read_u32(bytes, 88)?,
        };
        verify_crc(&bytes[..Self::LEN], 88, entry.checksum)?;
        entry.validate_for_pruning()?;
        Ok(entry)
    }

    pub fn serialize(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0..8].copy_from_slice(&self.entry_id.to_le_bytes());
        out[8..24].copy_from_slice(&self.dataset_id);
        out[24..40].copy_from_slice(&self.snapshot_id);
        out[40..44].copy_from_slice(&self.predicate_normal_form_ref.to_le_bytes());
        out[44..48].copy_from_slice(&self.interval_normal_form_ref.to_le_bytes());
        out[48..52].copy_from_slice(&self.coverage_set_ref.to_le_bytes());
        out[52] = self.coverage_granularity as u8;
        out[53] = self.proof_strength as u8;
        out[54] = self.exactness as u8;
        out[55] = self.flags;
        out[56..64].copy_from_slice(&self.actual_coverage_size_bytes.to_le_bytes());
        out[64..72].copy_from_slice(&self.actual_read_cost_ns.to_le_bytes());
        out[72..80].copy_from_slice(&self.created_at_us.to_le_bytes());
        out[80..84].copy_from_slice(&self.valid_until_snapshot_ref.to_le_bytes());
        out[84..88].copy_from_slice(&self.producer_engine_ref.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[88..92].copy_from_slice(&crc.to_le_bytes());
        out
    }

    pub fn validate_for_pruning(&self) -> Result<(), CoveError> {
        if self.exactness.may_under_include() || !self.proof_strength.allows_pruning() {
            return Err(CoveError::CacheStale);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageCacheV2 {
    pub header: CoveCoverageCacheHeaderV2,
    pub entries: Vec<CoverageCacheEntryV2>,
}

impl CoverageCacheV2 {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let header = CoveCoverageCacheHeaderV2::parse(bytes)?;
        let expected = CoveCoverageCacheHeaderV2::LEN
            .checked_add(header.entry_count as usize * CoverageCacheEntryV2::LEN)
            .ok_or(CoveError::ArithOverflow)?;
        if bytes.len() != expected {
            return Err(CoveError::CacheStale);
        }
        let mut entries = Vec::with_capacity(header.entry_count as usize);
        let mut ids = BTreeSet::new();
        for chunk in bytes[CoveCoverageCacheHeaderV2::LEN..].chunks_exact(CoverageCacheEntryV2::LEN)
        {
            let entry = CoverageCacheEntryV2::parse(chunk)?;
            if entry.dataset_id != header.dataset_id || entry.snapshot_id != header.snapshot_id {
                return Err(CoveError::CacheStale);
            }
            if !ids.insert(entry.entry_id) {
                return Err(CoveError::CacheStale);
            }
            entries.push(entry);
        }
        Ok(Self { header, entries })
    }

    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        if self.header.entry_count as usize != self.entries.len() {
            return Err(CoveError::CacheStale);
        }
        let mut out = Vec::new();
        out.extend_from_slice(&self.header.serialize());
        for entry in &self.entries {
            if entry.dataset_id != self.header.dataset_id
                || entry.snapshot_id != self.header.snapshot_id
            {
                return Err(CoveError::CacheStale);
            }
            out.extend_from_slice(&entry.serialize());
        }
        Ok(out)
    }
}

fn verify_crc(bytes: &[u8], checksum_offset: usize, expected: u32) -> Result<(), CoveError> {
    let mut check = bytes.to_vec();
    check[checksum_offset..checksum_offset + 4].fill(0);
    if checksum::crc32c(&check) != expected {
        return Err(CoveError::ChecksumMismatch);
    }
    Ok(())
}

fn read_u8(bytes: &[u8], offset: usize) -> Result<u8, CoveError> {
    if offset >= bytes.len() {
        return Err(CoveError::BufferTooShort);
    }
    Ok(bytes[offset])
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, CoveError> {
    Ok(u16::from_le_bytes(read_array(bytes, offset)?))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, CoveError> {
    Ok(u32::from_le_bytes(read_array(bytes, offset)?))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, CoveError> {
    Ok(u64::from_le_bytes(read_array(bytes, offset)?))
}

fn read_i64(bytes: &[u8], offset: usize) -> Result<i64, CoveError> {
    Ok(i64::from_le_bytes(read_array(bytes, offset)?))
}

fn read_array<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N], CoveError> {
    let end = offset.checked_add(N).ok_or(CoveError::ArithOverflow)?;
    if end > bytes.len() {
        return Err(CoveError::BufferTooShort);
    }
    Ok(bytes[offset..end].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(dataset_id: [u8; 16], snapshot_id: [u8; 16]) -> CoverageCacheEntryV2 {
        CoverageCacheEntryV2 {
            entry_id: 1,
            dataset_id,
            snapshot_id,
            predicate_normal_form_ref: 1,
            interval_normal_form_ref: u32::MAX,
            coverage_set_ref: 1,
            coverage_granularity: CoverageGranularityV2::Page,
            proof_strength: CoverageProofStrengthV2::ExactConservative,
            exactness: CoverageExactnessV2::Exact,
            flags: 0,
            actual_coverage_size_bytes: 128,
            actual_read_cost_ns: 100,
            created_at_us: 0,
            valid_until_snapshot_ref: u32::MAX,
            producer_engine_ref: u32::MAX,
            checksum: 0,
        }
    }

    #[test]
    fn cache_round_trips() {
        let dataset_id = [1u8; 16];
        let snapshot_id = [2u8; 16];
        let cache = CoverageCacheV2 {
            header: CoveCoverageCacheHeaderV2 {
                cache_format_namespace_ref: 1,
                cache_format_version_major: 2,
                cache_format_version_minor: 0,
                flags: 0,
                cache_id: [3u8; 16],
                dataset_id,
                snapshot_id,
                entry_count: 1,
                created_at_us: 0,
                producer_engine_ref: u32::MAX,
                reserved: [0u8; 32],
                checksum: 0,
            },
            entries: vec![entry(dataset_id, snapshot_id)],
        };
        let bytes = cache.serialize().unwrap();
        let parsed = CoverageCacheV2::parse(&bytes).unwrap();
        assert_eq!(parsed.entries.len(), 1);
    }

    #[test]
    fn under_inclusive_cache_entry_rejected() {
        let mut entry = entry([1u8; 16], [2u8; 16]);
        entry.exactness = CoverageExactnessV2::ApproximateMayUnderInclude;
        assert!(matches!(
            entry.validate_for_pruning(),
            Err(CoveError::CacheStale)
        ));
    }

    #[test]
    fn stale_snapshot_cache_entry_rejected() {
        let dataset_id = [1u8; 16];
        let snapshot_id = [2u8; 16];
        let mut stale = entry(dataset_id, snapshot_id);
        stale.snapshot_id = [9u8; 16];
        let header = CoveCoverageCacheHeaderV2 {
            cache_format_namespace_ref: 1,
            cache_format_version_major: 2,
            cache_format_version_minor: 0,
            flags: 0,
            cache_id: [3u8; 16],
            dataset_id,
            snapshot_id,
            entry_count: 1,
            created_at_us: 0,
            producer_engine_ref: u32::MAX,
            reserved: [0u8; 32],
            checksum: 0,
        };
        let mut bytes = header.serialize().to_vec();
        bytes.extend_from_slice(&stale.serialize());
        assert!(matches!(
            CoverageCacheV2::parse(&bytes),
            Err(CoveError::CacheStale)
        ));
    }

    #[test]
    fn duplicate_cache_entry_id_rejected() {
        let dataset_id = [1u8; 16];
        let snapshot_id = [2u8; 16];
        let cache = CoverageCacheV2 {
            header: CoveCoverageCacheHeaderV2 {
                cache_format_namespace_ref: 1,
                cache_format_version_major: 2,
                cache_format_version_minor: 0,
                flags: 0,
                cache_id: [3u8; 16],
                dataset_id,
                snapshot_id,
                entry_count: 2,
                created_at_us: 0,
                producer_engine_ref: u32::MAX,
                reserved: [0u8; 32],
                checksum: 0,
            },
            entries: vec![
                entry(dataset_id, snapshot_id),
                entry(dataset_id, snapshot_id),
            ],
        };
        let bytes = cache.serialize().unwrap();
        assert!(matches!(
            CoverageCacheV2::parse(&bytes),
            Err(CoveError::CacheStale)
        ));
    }
}
