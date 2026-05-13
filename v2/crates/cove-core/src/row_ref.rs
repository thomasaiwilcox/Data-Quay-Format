//! Cove Format (COVE) v2.0 — Stable row references (Spec §54).
//!
//! Each row in a COVE-T table can be addressed by the Spec §54.1
//! `CoveTableRowRefV1` tuple: `(table_id, segment_id, morsel_id,
//! row_in_morsel)`.

use crate::CoveError;

/// A stable row reference (Spec §54.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RowRef {
    pub table_id: u32,
    pub segment_id: u32,
    pub morsel_id: u32,
    pub row_in_morsel: u16,
}

impl RowRef {
    /// Encoded byte length of `CoveTableRowRefV1` (Spec §54.1).
    pub const ENCODED_LEN: usize = 14;

    pub fn encode(&self) -> [u8; Self::ENCODED_LEN] {
        let mut out = [0u8; Self::ENCODED_LEN];
        out[0..4].copy_from_slice(&self.table_id.to_le_bytes());
        out[4..8].copy_from_slice(&self.segment_id.to_le_bytes());
        out[8..12].copy_from_slice(&self.morsel_id.to_le_bytes());
        out[12..14].copy_from_slice(&self.row_in_morsel.to_le_bytes());
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::ENCODED_LEN {
            return Err(CoveError::BufferTooShort);
        }
        Ok(Self {
            table_id: u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
            segment_id: u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            morsel_id: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
            row_in_morsel: u16::from_le_bytes(bytes[12..14].try_into().unwrap()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let r = RowRef {
            table_id: 1,
            segment_id: 2,
            morsel_id: 3,
            row_in_morsel: 4,
        };
        assert_eq!(RowRef::decode(&r.encode()).unwrap(), r);
    }

    #[test]
    fn truncated_rejected() {
        assert!(matches!(
            RowRef::decode(&[0u8; 4]),
            Err(CoveError::BufferTooShort)
        ));
    }

    #[test]
    fn encoded_len_is_fourteen() {
        assert_eq!(RowRef::ENCODED_LEN, 14);
    }
}
