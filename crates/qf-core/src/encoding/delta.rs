//! Spec §20.3 — Delta encoding.
//!
//! Stores a base value followed by `row_count - 1` zigzag varint deltas.
//! `value[i] = value[i-1] + delta[i-1]`. Wrapping arithmetic on `i64` per
//! Spec §20.3.6 — the canonical decode never panics on overflow.

use crate::wire;
use crate::QfError;

use super::Encoding;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeltaPayload {
    pub base: i64,
    pub deltas: Vec<i64>,
}

impl DeltaPayload {
    /// Wire format: `i64 base | u32 delta_count | varint(zigzag) deltas`.
    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < 12 {
            return Err(QfError::BufferTooShort);
        }
        let base = i64::from_le_bytes(bytes[0..8].try_into().unwrap());
        let n = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let mut deltas = Vec::with_capacity(n);
        let mut pos = 12usize;
        for _ in 0..n {
            let (z, used) = wire::decode_u64_leb128(&bytes[pos..])?;
            pos += used;
            deltas.push(wire::zigzag_decode_i64(z));
        }
        Ok(Self { base, deltas })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&self.base.to_le_bytes());
        out.extend_from_slice(&(self.deltas.len() as u32).to_le_bytes());
        for d in &self.deltas {
            out.extend_from_slice(&wire::encode_u64_leb128(wire::zigzag_encode_i64(*d)));
        }
        out
    }
}

pub struct Delta;

impl Encoding for Delta {
    type Payload = DeltaPayload;

    fn canonical_decode(payload: &Self::Payload) -> Result<Vec<i64>, QfError> {
        let mut out = Vec::with_capacity(1 + payload.deltas.len());
        let mut cur = payload.base;
        out.push(cur);
        for d in &payload.deltas {
            cur = cur.wrapping_add(*d);
            out.push(cur);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::assert_parity;

    #[test]
    fn round_trip_and_decode() {
        let p = DeltaPayload {
            base: 100,
            deltas: vec![1, 2, -3, 5],
        };
        let bytes = p.encode();
        assert_eq!(DeltaPayload::parse(&bytes).unwrap(), p);
        assert_eq!(
            Delta::canonical_decode(&p).unwrap(),
            vec![100, 101, 103, 100, 105]
        );
        assert!(assert_parity::<Delta>(&p).is_ok());
    }

    #[test]
    fn overflow_wraps_without_panic() {
        let p = DeltaPayload {
            base: i64::MAX - 1,
            deltas: vec![5],
        };
        let out = Delta::canonical_decode(&p).unwrap();
        assert_eq!(out[0], i64::MAX - 1);
        assert_eq!(out[1], (i64::MAX - 1).wrapping_add(5));
    }
}
