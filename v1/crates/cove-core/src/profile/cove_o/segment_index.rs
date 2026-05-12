use crate::{checksum, CoveError};

use super::TEMPORAL_SEGMENT_INDEX_ENTRY_LEN;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TemporalSegmentIndex {
    pub flags: u32,
    pub entries: Vec<TemporalSegmentIndexEntryV1>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemporalSegmentIndexEntryV1 {
    pub segment_id: u32,
    pub object_type_id: u32,
    pub time_range_start_us: i64,
    pub time_range_end_us: i64,
    pub csn_min: u64,
    pub csn_max: u64,
    pub row_count: u32,
    pub delta_count: u32,
    pub snapshot_count: u32,
    pub baseline_count: u32,
    pub tombstone_count: u32,
    pub min_goid: [u8; 16],
    pub max_goid: [u8; 16],
    pub offset: u64,
    pub length: u64,
    pub checksum: u32,
}

impl TemporalSegmentIndex {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < 8 {
            return Err(CoveError::BufferTooShort);
        }
        let entry_count = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let flags = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let needed = 8usize
            .checked_add(
                entry_count
                    .checked_mul(TEMPORAL_SEGMENT_INDEX_ENTRY_LEN)
                    .ok_or(CoveError::ArithOverflow)?,
            )
            .ok_or(CoveError::ArithOverflow)?;
        if needed > bytes.len() {
            return Err(CoveError::BufferTooShort);
        }
        let mut entries = Vec::with_capacity(entry_count);
        let mut pos = 8usize;
        for _ in 0..entry_count {
            entries.push(TemporalSegmentIndexEntryV1::parse(
                &bytes[pos..pos + TEMPORAL_SEGMENT_INDEX_ENTRY_LEN],
            )?);
            pos += TEMPORAL_SEGMENT_INDEX_ENTRY_LEN;
        }
        let index = Self { flags, entries };
        index.validate()?;
        Ok(index)
    }

    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        let count = u32::try_from(self.entries.len())
            .map_err(|_| CoveError::BadSchema("too many temporal segments".into()))?;
        let mut out = Vec::with_capacity(8 + self.entries.len() * TEMPORAL_SEGMENT_INDEX_ENTRY_LEN);
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        for entry in &self.entries {
            out.extend_from_slice(&entry.serialize());
        }
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        let mut seen = std::collections::HashSet::new();
        for entry in &self.entries {
            if !seen.insert((entry.object_type_id, entry.segment_id)) {
                return Err(CoveError::RefInvalid);
            }
            entry.validate()?;
        }
        Ok(())
    }
}

impl TemporalSegmentIndexEntryV1 {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < TEMPORAL_SEGMENT_INDEX_ENTRY_LEN {
            return Err(CoveError::BufferTooShort);
        }
        let segment_id = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let object_type_id = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let time_range_start_us = i64::from_le_bytes(bytes[8..16].try_into().unwrap());
        let time_range_end_us = i64::from_le_bytes(bytes[16..24].try_into().unwrap());
        let csn_min = u64::from_le_bytes(bytes[24..32].try_into().unwrap());
        let csn_max = u64::from_le_bytes(bytes[32..40].try_into().unwrap());
        let row_count = u32::from_le_bytes(bytes[40..44].try_into().unwrap());
        let delta_count = u32::from_le_bytes(bytes[44..48].try_into().unwrap());
        let snapshot_count = u32::from_le_bytes(bytes[48..52].try_into().unwrap());
        let baseline_count = u32::from_le_bytes(bytes[52..56].try_into().unwrap());
        let tombstone_count = u32::from_le_bytes(bytes[56..60].try_into().unwrap());
        let mut min_goid = [0u8; 16];
        min_goid.copy_from_slice(&bytes[60..76]);
        let mut max_goid = [0u8; 16];
        max_goid.copy_from_slice(&bytes[76..92]);
        let offset = u64::from_le_bytes(bytes[92..100].try_into().unwrap());
        let length = u64::from_le_bytes(bytes[100..108].try_into().unwrap());
        let checksum_field = u32::from_le_bytes(bytes[108..112].try_into().unwrap());
        let mut for_crc = [0u8; TEMPORAL_SEGMENT_INDEX_ENTRY_LEN];
        for_crc.copy_from_slice(&bytes[..TEMPORAL_SEGMENT_INDEX_ENTRY_LEN]);
        for_crc[108..112].fill(0);
        if checksum::crc32c(&for_crc) != checksum_field {
            return Err(CoveError::ChecksumMismatch);
        }
        let entry = Self {
            segment_id,
            object_type_id,
            time_range_start_us,
            time_range_end_us,
            csn_min,
            csn_max,
            row_count,
            delta_count,
            snapshot_count,
            baseline_count,
            tombstone_count,
            min_goid,
            max_goid,
            offset,
            length,
            checksum: checksum_field,
        };
        entry.validate()?;
        Ok(entry)
    }

    pub fn serialize(&self) -> [u8; TEMPORAL_SEGMENT_INDEX_ENTRY_LEN] {
        let mut buf = [0u8; TEMPORAL_SEGMENT_INDEX_ENTRY_LEN];
        buf[0..4].copy_from_slice(&self.segment_id.to_le_bytes());
        buf[4..8].copy_from_slice(&self.object_type_id.to_le_bytes());
        buf[8..16].copy_from_slice(&self.time_range_start_us.to_le_bytes());
        buf[16..24].copy_from_slice(&self.time_range_end_us.to_le_bytes());
        buf[24..32].copy_from_slice(&self.csn_min.to_le_bytes());
        buf[32..40].copy_from_slice(&self.csn_max.to_le_bytes());
        buf[40..44].copy_from_slice(&self.row_count.to_le_bytes());
        buf[44..48].copy_from_slice(&self.delta_count.to_le_bytes());
        buf[48..52].copy_from_slice(&self.snapshot_count.to_le_bytes());
        buf[52..56].copy_from_slice(&self.baseline_count.to_le_bytes());
        buf[56..60].copy_from_slice(&self.tombstone_count.to_le_bytes());
        buf[60..76].copy_from_slice(&self.min_goid);
        buf[76..92].copy_from_slice(&self.max_goid);
        buf[92..100].copy_from_slice(&self.offset.to_le_bytes());
        buf[100..108].copy_from_slice(&self.length.to_le_bytes());
        let crc = checksum::crc32c(&buf);
        buf[108..112].copy_from_slice(&crc.to_le_bytes());
        buf
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        if self.time_range_start_us > self.time_range_end_us || self.csn_min > self.csn_max {
            return Err(CoveError::BadSchema(
                "temporal segment range is inverted (Spec §57)".into(),
            ));
        }
        if self.min_goid > self.max_goid {
            return Err(CoveError::BadSchema(
                "temporal segment GOID range is inverted (Spec §57)".into(),
            ));
        }
        let counted = self
            .delta_count
            .checked_add(self.snapshot_count)
            .and_then(|v| v.checked_add(self.baseline_count))
            .and_then(|v| v.checked_add(self.tombstone_count))
            .ok_or(CoveError::ArithOverflow)?;
        if counted != self.row_count {
            return Err(CoveError::BadSchema(
                "temporal segment record-kind counts do not sum to row_count (Spec §57)".into(),
            ));
        }
        Ok(())
    }
}
