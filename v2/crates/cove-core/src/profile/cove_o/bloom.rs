use crate::{checksum, CoveError};

use super::TEMPORAL_BLOOM_ENTRY_LEN;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemporalBloomEntryV1 {
    pub segment_id: u32,
    pub time_bucket_start_us: i64,
    pub time_bucket_end_us: i64,
    pub filter_offset: u64,
    pub filter_length: u64,
    pub checksum: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TemporalBloomIndex {
    pub flags: u32,
    pub entries: Vec<TemporalBloomEntryV1>,
}

impl TemporalBloomIndex {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < 8 {
            return Err(CoveError::BufferTooShort);
        }
        let entry_count = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let flags = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let entries_len = entry_count
            .checked_mul(TEMPORAL_BLOOM_ENTRY_LEN)
            .ok_or(CoveError::ArithOverflow)?;
        let entries_end = 8usize
            .checked_add(entries_len)
            .ok_or(CoveError::ArithOverflow)?;
        if entries_end > bytes.len() {
            return Err(CoveError::BufferTooShort);
        }
        let mut entries = Vec::with_capacity(entry_count);
        let mut pos = 8usize;
        for _ in 0..entry_count {
            let segment_id = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
            let time_bucket_start_us =
                i64::from_le_bytes(bytes[pos + 4..pos + 12].try_into().unwrap());
            let time_bucket_end_us =
                i64::from_le_bytes(bytes[pos + 12..pos + 20].try_into().unwrap());
            if time_bucket_start_us > time_bucket_end_us {
                return Err(CoveError::BadIndex);
            }
            let checksum_field = u32::from_le_bytes(bytes[pos + 36..pos + 40].try_into().unwrap());
            let mut for_crc = [0u8; TEMPORAL_BLOOM_ENTRY_LEN];
            for_crc.copy_from_slice(&bytes[pos..pos + TEMPORAL_BLOOM_ENTRY_LEN]);
            for_crc[36..40].fill(0);
            if checksum::crc32c(&for_crc) != checksum_field {
                return Err(CoveError::ChecksumMismatch);
            }
            let filter_offset = u64::from_le_bytes(bytes[pos + 20..pos + 28].try_into().unwrap());
            let filter_length = u64::from_le_bytes(bytes[pos + 28..pos + 36].try_into().unwrap());
            let filter_end = filter_offset
                .checked_add(filter_length)
                .ok_or(CoveError::ArithOverflow)?;
            if filter_end > bytes.len() as u64 {
                return Err(CoveError::OffsetRange);
            }
            entries.push(TemporalBloomEntryV1 {
                segment_id,
                time_bucket_start_us,
                time_bucket_end_us,
                filter_offset,
                filter_length,
                checksum: checksum_field,
            });
            pos += TEMPORAL_BLOOM_ENTRY_LEN;
        }
        Ok(Self { flags, entries })
    }

    /// Inverse of [`Self::parse`]; computes filter offsets, lengths, and entry
    /// checksums canonically from the provided filter payloads.
    pub fn serialize(&self, filters: &[Vec<u8>]) -> Result<Vec<u8>, CoveError> {
        if self.entries.len() != filters.len() {
            return Err(CoveError::BadIndex);
        }
        let entry_count = u32::try_from(self.entries.len())
            .map_err(|_| CoveError::BadSection("too many temporal bloom entries".into()))?;
        let entries_len = self
            .entries
            .len()
            .checked_mul(TEMPORAL_BLOOM_ENTRY_LEN)
            .ok_or(CoveError::ArithOverflow)?;
        let payload_start = 8usize
            .checked_add(entries_len)
            .ok_or(CoveError::ArithOverflow)?;
        let mut out = Vec::with_capacity(
            payload_start
                .checked_add(filters.iter().map(Vec::len).sum())
                .ok_or(CoveError::ArithOverflow)?,
        );
        out.extend_from_slice(&entry_count.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        let mut next_filter_offset = payload_start as u64;
        for (entry, filter) in self.entries.iter().zip(filters) {
            if entry.time_bucket_start_us > entry.time_bucket_end_us {
                return Err(CoveError::BadIndex);
            }
            let filter_length = u64::try_from(filter.len()).map_err(|_| CoveError::OffsetRange)?;
            let mut entry_bytes = [0u8; TEMPORAL_BLOOM_ENTRY_LEN];
            entry_bytes[0..4].copy_from_slice(&entry.segment_id.to_le_bytes());
            entry_bytes[4..12].copy_from_slice(&entry.time_bucket_start_us.to_le_bytes());
            entry_bytes[12..20].copy_from_slice(&entry.time_bucket_end_us.to_le_bytes());
            entry_bytes[20..28].copy_from_slice(&next_filter_offset.to_le_bytes());
            entry_bytes[28..36].copy_from_slice(&filter_length.to_le_bytes());
            let crc = checksum::crc32c(&entry_bytes);
            entry_bytes[36..40].copy_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&entry_bytes);
            next_filter_offset = next_filter_offset
                .checked_add(filter_length)
                .ok_or(CoveError::ArithOverflow)?;
        }
        for filter in filters {
            out.extend_from_slice(filter);
        }
        Ok(out)
    }
}
