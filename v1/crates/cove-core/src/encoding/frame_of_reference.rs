//! Spec §20.3 — Frame-of-Reference encoding.
//!
//! Each value is stored as the offset from a per-page reference value.
//! Wire format (LE): `i64 reference | u32 count | i64 offsets[count]`.
//! The "offset" channel is itself a candidate for further compression
//! (e.g. BitPacked) in real cascades; this module handles only the FoR
//! transform so cascades can be composed without coupling.

use crate::CoveError;

use super::Encoding;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForPayload {
    pub reference: i64,
    pub offsets: Vec<i64>,
}

impl ForPayload {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < 12 {
            return Err(CoveError::BufferTooShort);
        }
        let reference = i64::from_le_bytes(bytes[0..8].try_into().unwrap());
        let n = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let need = 12 + n * 8;
        if bytes.len() < need {
            return Err(CoveError::BufferTooShort);
        }
        let mut offsets = Vec::with_capacity(n);
        for i in 0..n {
            let off = 12 + i * 8;
            offsets.push(i64::from_le_bytes(bytes[off..off + 8].try_into().unwrap()));
        }
        Ok(Self { reference, offsets })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(12 + self.offsets.len() * 8);
        out.extend_from_slice(&self.reference.to_le_bytes());
        out.extend_from_slice(&(self.offsets.len() as u32).to_le_bytes());
        for v in &self.offsets {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }
}

pub struct FrameOfReference;

impl Encoding for FrameOfReference {
    type Payload = ForPayload;

    fn canonical_decode(payload: &Self::Payload) -> Result<Vec<i64>, CoveError> {
        Ok(payload
            .offsets
            .iter()
            .map(|o| payload.reference.wrapping_add(*o))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::assert_parity;

    #[test]
    fn round_trip() {
        let p = ForPayload {
            reference: 1_000_000,
            offsets: vec![0, 1, -2, 3, 4],
        };
        let bytes = p.encode();
        assert_eq!(ForPayload::parse(&bytes).unwrap(), p);
        assert_eq!(
            FrameOfReference::canonical_decode(&p).unwrap(),
            vec![1_000_000, 1_000_001, 999_998, 1_000_003, 1_000_004]
        );
        assert!(assert_parity::<FrameOfReference>(&p).is_ok());
    }
}
