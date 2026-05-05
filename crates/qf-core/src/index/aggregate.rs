//! Spec §34 — Aggregate synopsis.
//!
//! Pre-computed COUNT / SUM / MIN / MAX values for the indexed scope. The
//! synopsis can answer metadata-only queries without any decode. Spec §34.4
//! requires that aggregates be invalidated on redaction or schema evolution
//! and that consumers verify zone-stat compatibility before reading.

use crate::QfError;

use super::{checked_region, verify_checksum_field};

pub const AGGREGATE_SYNOPSIS_ENTRY_LEN: usize = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SynopsisKind {
    Count,
    MinMax,
    Sum,
    SumAndCount,
    BoolTrueFalseCounts,
    FileCodeHistogram,
    NumCodeHistogram,
    DistinctSketch,
    QuantileSketch,
    TopK,
}

impl SynopsisKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Count),
            1 => Some(Self::MinMax),
            2 => Some(Self::Sum),
            3 => Some(Self::SumAndCount),
            4 => Some(Self::BoolTrueFalseCounts),
            5 => Some(Self::FileCodeHistogram),
            6 => Some(Self::NumCodeHistogram),
            7 => Some(Self::DistinctSketch),
            8 => Some(Self::QuantileSketch),
            9 => Some(Self::TopK),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SynopsisAccuracy {
    Exact = 0,
    Approximate = 1,
}

impl SynopsisAccuracy {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Exact),
            1 => Some(Self::Approximate),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AggregateEntry {
    pub table_id: u32,
    pub segment_id: u32,
    pub morsel_id: u32,
    pub column_id: u32,
    pub synopsis_kind: SynopsisKind,
    pub key_kind: u8,
    pub accuracy: SynopsisAccuracy,
    pub flags: u8,
    pub row_count: u32,
    pub null_count: u32,
    pub payload_offset: u64,
    pub payload_length: u64,
    pub checksum: u32,
}

impl AggregateEntry {
    pub fn serialize(&self) -> [u8; AGGREGATE_SYNOPSIS_ENTRY_LEN] {
        let mut out = [0u8; AGGREGATE_SYNOPSIS_ENTRY_LEN];
        out[0..4].copy_from_slice(&self.table_id.to_le_bytes());
        out[4..8].copy_from_slice(&self.segment_id.to_le_bytes());
        out[8..12].copy_from_slice(&self.morsel_id.to_le_bytes());
        out[12..16].copy_from_slice(&self.column_id.to_le_bytes());
        out[16] = self.synopsis_kind as u8;
        out[17] = self.key_kind;
        out[18] = self.accuracy as u8;
        out[19] = self.flags;
        out[20..24].copy_from_slice(&self.row_count.to_le_bytes());
        out[24..28].copy_from_slice(&self.null_count.to_le_bytes());
        out[28..36].copy_from_slice(&self.payload_offset.to_le_bytes());
        out[36..44].copy_from_slice(&self.payload_length.to_le_bytes());
        let crc = crate::checksum::crc32c(&out);
        out[44..48].copy_from_slice(&crc.to_le_bytes());
        out
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < AGGREGATE_SYNOPSIS_ENTRY_LEN {
            return Err(QfError::BufferTooShort);
        }
        let bytes = &bytes[..AGGREGATE_SYNOPSIS_ENTRY_LEN];
        let checksum = verify_checksum_field(bytes, 44)?;
        let synopsis_kind = SynopsisKind::from_u8(bytes[16]).ok_or(QfError::BadIndex)?;
        let accuracy = SynopsisAccuracy::from_u8(bytes[18]).ok_or(QfError::BadIndex)?;
        let row_count = u32::from_le_bytes(bytes[20..24].try_into().unwrap());
        let null_count = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
        if null_count > row_count {
            return Err(QfError::BadIndex);
        }
        Ok(Self {
            table_id: u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
            segment_id: u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            morsel_id: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
            column_id: u32::from_le_bytes(bytes[12..16].try_into().unwrap()),
            synopsis_kind,
            key_kind: bytes[17],
            accuracy,
            flags: bytes[19],
            row_count,
            null_count,
            payload_offset: u64::from_le_bytes(bytes[28..36].try_into().unwrap()),
            payload_length: u64::from_le_bytes(bytes[36..44].try_into().unwrap()),
            checksum,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AggregateSynopsis {
    pub entries: Vec<AggregateEntry>,
}

impl AggregateSynopsis {
    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < AGGREGATE_SYNOPSIS_ENTRY_LEN {
            return Err(QfError::BufferTooShort);
        }
        let first = AggregateEntry::parse(&bytes[..AGGREGATE_SYNOPSIS_ENTRY_LEN])?;
        if first.payload_length != 0 {
            checked_region(bytes, first.payload_offset, first.payload_length)?;
            return Ok(Self {
                entries: vec![first],
            });
        }
        if bytes.len() % AGGREGATE_SYNOPSIS_ENTRY_LEN != 0 {
            return Err(QfError::BadIndex);
        }
        let mut entries = Vec::with_capacity(bytes.len() / AGGREGATE_SYNOPSIS_ENTRY_LEN);
        for chunk in bytes.chunks_exact(AGGREGATE_SYNOPSIS_ENTRY_LEN) {
            let entry = AggregateEntry::parse(chunk)?;
            if entry.payload_length != 0 {
                checked_region(bytes, entry.payload_offset, entry.payload_length)?;
            }
            entries.push(entry);
        }
        Ok(Self { entries })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_count_synopsis() {
        let bytes = AggregateEntry {
            table_id: 1,
            segment_id: 2,
            morsel_id: u32::MAX,
            column_id: 3,
            synopsis_kind: SynopsisKind::Count,
            key_kind: 0,
            accuracy: SynopsisAccuracy::Exact,
            flags: 0,
            row_count: 12345,
            null_count: 0,
            payload_offset: 0,
            payload_length: 0,
            checksum: 0,
        }
        .serialize()
        .to_vec();
        let s = AggregateSynopsis::parse(&bytes).unwrap();
        assert_eq!(s.entries[0].synopsis_kind, SynopsisKind::Count);
        assert_eq!(s.entries[0].row_count, 12345);
    }

    #[test]
    fn rejects_unknown_kind() {
        let mut bytes = AggregateEntry {
            table_id: 1,
            segment_id: 2,
            morsel_id: u32::MAX,
            column_id: 3,
            synopsis_kind: SynopsisKind::Count,
            key_kind: 0,
            accuracy: SynopsisAccuracy::Exact,
            flags: 0,
            row_count: 1,
            null_count: 0,
            payload_offset: 0,
            payload_length: 0,
            checksum: 0,
        }
        .serialize();
        bytes[16] = 99;
        bytes[44..48].fill(0);
        let crc = crate::checksum::crc32c(&bytes);
        bytes[44..48].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(AggregateSynopsis::parse(&bytes), Err(QfError::BadIndex));
    }

    #[test]
    fn rejects_checksum_mismatch() {
        let mut bytes = AggregateEntry {
            table_id: 1,
            segment_id: 2,
            morsel_id: u32::MAX,
            column_id: 3,
            synopsis_kind: SynopsisKind::Count,
            key_kind: 0,
            accuracy: SynopsisAccuracy::Exact,
            flags: 0,
            row_count: 1,
            null_count: 0,
            payload_offset: 0,
            payload_length: 0,
            checksum: 0,
        }
        .serialize();
        bytes[44] ^= 0xff;
        assert_eq!(
            AggregateSynopsis::parse(&bytes),
            Err(QfError::ChecksumMismatch)
        );
    }

    #[test]
    fn validates_attached_payload_region() {
        let payload = [0xA5u8; 16];
        let mut bytes = AggregateEntry {
            table_id: 1,
            segment_id: 2,
            morsel_id: u32::MAX,
            column_id: 3,
            synopsis_kind: SynopsisKind::MinMax,
            key_kind: 0,
            accuracy: SynopsisAccuracy::Exact,
            flags: 0,
            row_count: 10,
            null_count: 1,
            payload_offset: AGGREGATE_SYNOPSIS_ENTRY_LEN as u64,
            payload_length: payload.len() as u64,
            checksum: 0,
        }
        .serialize()
        .to_vec();
        bytes.extend_from_slice(&payload);
        let synopsis = AggregateSynopsis::parse(&bytes).unwrap();
        assert_eq!(synopsis.entries[0].payload_length, payload.len() as u64);
    }
}
