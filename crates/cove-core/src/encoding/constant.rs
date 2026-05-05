//! Spec §20.3 — Constant encoding.
//!
//! Every logical row decodes to the same scalar. Wire payload is exactly
//! the value followed by the row count (LE u64). Pages that claim more
//! rows than `u32::MAX` are rejected up-front to bound allocation.

use crate::CoveError;

use super::Encoding;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstantPayload {
    pub value: i64,
    pub row_count: u64,
}

impl ConstantPayload {
    pub const ENCODED_LEN: usize = 16;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::ENCODED_LEN {
            return Err(CoveError::BufferTooShort);
        }
        let value = i64::from_le_bytes(bytes[0..8].try_into().unwrap());
        let row_count = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
        if row_count > u32::MAX as u64 {
            return Err(CoveError::PageCorrupt);
        }
        Ok(Self { value, row_count })
    }

    pub fn encode(&self) -> [u8; Self::ENCODED_LEN] {
        let mut out = [0u8; Self::ENCODED_LEN];
        out[0..8].copy_from_slice(&self.value.to_le_bytes());
        out[8..16].copy_from_slice(&self.row_count.to_le_bytes());
        out
    }
}

pub struct Constant;

impl Encoding for Constant {
    type Payload = ConstantPayload;

    fn canonical_decode(payload: &Self::Payload) -> Result<Vec<i64>, CoveError> {
        Ok(vec![payload.value; payload.row_count as usize])
    }

    fn fast_decode(payload: &Self::Payload) -> Result<Vec<i64>, CoveError> {
        // Same as canonical — there is no faster way.
        Ok(vec![payload.value; payload.row_count as usize])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::assert_parity;

    #[test]
    fn round_trip_and_decode() {
        let p = ConstantPayload {
            value: -42,
            row_count: 5,
        };
        let bytes = p.encode();
        let parsed = ConstantPayload::parse(&bytes).unwrap();
        assert_eq!(parsed, p);
        assert_eq!(Constant::canonical_decode(&p).unwrap(), vec![-42; 5]);
    }

    #[test]
    fn rejects_oversized_row_count() {
        let mut bytes = [0u8; 16];
        bytes[8..16].copy_from_slice(&(u64::MAX).to_le_bytes());
        assert_eq!(ConstantPayload::parse(&bytes), Err(CoveError::PageCorrupt));
    }

    #[test]
    fn parity_holds() {
        let p = ConstantPayload {
            value: 7,
            row_count: 100,
        };
        assert!(assert_parity::<Constant>(&p).is_ok());
    }
}
