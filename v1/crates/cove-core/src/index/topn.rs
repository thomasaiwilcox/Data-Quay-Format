//! Spec §36 — Top-N Zone Summary.

use crate::CoveError;

use super::{checked_region, verify_checksum_field};

pub const TOPN_ZONE_SUMMARY_LEN: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
#[non_exhaustive]
pub enum TopNDirection {
    Largest = 0,
    Smallest = 1,
}

impl TopNDirection {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Largest),
            1 => Some(Self::Smallest),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopNSummary {
    pub table_id: u32,
    pub column_id: u32,
    pub segment_id: u32,
    pub morsel_id: u32,
    pub direction: TopNDirection,
    pub value_count: u16,
    pub flags: u8,
    pub payload_offset: u64,
    pub payload_length: u64,
    pub checksum: u32,
    pub payload: Vec<u8>,
}

impl TopNSummary {
    pub fn serialize_header(&self) -> [u8; TOPN_ZONE_SUMMARY_LEN] {
        let mut out = [0u8; TOPN_ZONE_SUMMARY_LEN];
        out[0..4].copy_from_slice(&self.table_id.to_le_bytes());
        out[4..8].copy_from_slice(&self.column_id.to_le_bytes());
        out[8..12].copy_from_slice(&self.segment_id.to_le_bytes());
        out[12..16].copy_from_slice(&self.morsel_id.to_le_bytes());
        out[16] = self.direction as u8;
        out[17..19].copy_from_slice(&self.value_count.to_le_bytes());
        out[19] = self.flags;
        out[20..28].copy_from_slice(&self.payload_offset.to_le_bytes());
        out[28..36].copy_from_slice(&self.payload_length.to_le_bytes());
        let crc = crate::checksum::crc32c(&out);
        out[36..40].copy_from_slice(&crc.to_le_bytes());
        out
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < TOPN_ZONE_SUMMARY_LEN {
            return Err(CoveError::BufferTooShort);
        }
        let header = &bytes[..TOPN_ZONE_SUMMARY_LEN];
        let checksum = verify_checksum_field(header, 36)?;
        let direction = TopNDirection::from_u8(header[16]).ok_or(CoveError::BadIndex)?;
        let payload_offset = u64::from_le_bytes(header[20..28].try_into().unwrap());
        let payload_length = u64::from_le_bytes(header[28..36].try_into().unwrap());
        let payload = checked_region(bytes, payload_offset, payload_length)?;
        Ok(Self {
            table_id: u32::from_le_bytes(header[0..4].try_into().unwrap()),
            column_id: u32::from_le_bytes(header[4..8].try_into().unwrap()),
            segment_id: u32::from_le_bytes(header[8..12].try_into().unwrap()),
            morsel_id: u32::from_le_bytes(header[12..16].try_into().unwrap()),
            direction,
            value_count: u16::from_le_bytes(header[17..19].try_into().unwrap()),
            flags: header[19],
            payload_offset,
            payload_length,
            checksum,
            payload: payload.to_vec(),
        })
    }

    /// Inverse of [`Self::parse`]; produces canonical bytes that round-trip.
    pub fn serialize(&self) -> Vec<u8> {
        let mut hdr = self.clone();
        hdr.payload_offset = TOPN_ZONE_SUMMARY_LEN as u64;
        hdr.payload_length = self.payload.len() as u64;
        let mut out = hdr.serialize_header().to_vec();
        out.extend_from_slice(&self.payload);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_topn_summary() {
        let summary = TopNSummary {
            table_id: 1,
            column_id: 2,
            segment_id: 3,
            morsel_id: 4,
            direction: TopNDirection::Largest,
            value_count: 2,
            flags: 0,
            payload_offset: TOPN_ZONE_SUMMARY_LEN as u64,
            payload_length: 16,
            checksum: 0,
            payload: vec![0xAB; 16],
        };
        let mut bytes = summary.serialize_header().to_vec();
        bytes.extend_from_slice(&summary.payload);
        let parsed = TopNSummary::parse(&bytes).unwrap();
        assert_eq!(parsed.direction, TopNDirection::Largest);
        assert_eq!(parsed.value_count, 2);
        assert_eq!(parsed.payload.len(), 16);
    }

    #[test]
    fn rejects_bad_direction() {
        let summary = TopNSummary {
            table_id: 1,
            column_id: 2,
            segment_id: 3,
            morsel_id: 4,
            direction: TopNDirection::Largest,
            value_count: 0,
            flags: 0,
            payload_offset: TOPN_ZONE_SUMMARY_LEN as u64,
            payload_length: 0,
            checksum: 0,
            payload: Vec::new(),
        };
        let mut bytes = summary.serialize_header();
        bytes[16] = 9;
        bytes[36..40].fill(0);
        let crc = crate::checksum::crc32c(&bytes);
        bytes[36..40].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(TopNSummary::parse(&bytes), Err(CoveError::BadIndex));
    }

    #[test]
    fn rejects_checksum_mismatch() {
        let summary = TopNSummary {
            table_id: 1,
            column_id: 2,
            segment_id: 3,
            morsel_id: 4,
            direction: TopNDirection::Largest,
            value_count: 0,
            flags: 0,
            payload_offset: TOPN_ZONE_SUMMARY_LEN as u64,
            payload_length: 0,
            checksum: 0,
            payload: Vec::new(),
        };
        let mut bytes = summary.serialize_header();
        bytes[36] ^= 0xff;
        assert_eq!(TopNSummary::parse(&bytes), Err(CoveError::ChecksumMismatch));
    }

    #[test]
    fn serialize_round_trip_with_payload() {
        let summary = TopNSummary {
            table_id: 1,
            column_id: 2,
            segment_id: 3,
            morsel_id: 4,
            direction: TopNDirection::Smallest,
            value_count: 4,
            flags: 0,
            payload_offset: 0,
            payload_length: 0,
            checksum: 0,
            payload: vec![1, 2, 3, 4, 5, 6, 7, 8],
        };
        let bytes = summary.serialize();
        let parsed = TopNSummary::parse(&bytes).unwrap();
        assert_eq!(parsed.direction, TopNDirection::Smallest);
        assert_eq!(parsed.value_count, 4);
        assert_eq!(parsed.payload, vec![1, 2, 3, 4, 5, 6, 7, 8]);
    }
}
