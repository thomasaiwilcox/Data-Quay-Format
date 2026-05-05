//! Spec §20.3 — Plain encodings.
//!
//! * [`PlainFixed`] — `row_count` fixed-width little-endian `i64` values.
//! * [`PlainVarint`] — `row_count` zigzag-varint values.

use crate::wire;
use crate::QfError;

use super::Encoding;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlainFixedPayload {
    pub values: Vec<i64>,
}

impl PlainFixedPayload {
    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < 4 {
            return Err(QfError::BufferTooShort);
        }
        let n = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let need = 4 + n * 8;
        if bytes.len() < need {
            return Err(QfError::BufferTooShort);
        }
        let mut values = Vec::with_capacity(n);
        for i in 0..n {
            let off = 4 + i * 8;
            values.push(i64::from_le_bytes(bytes[off..off + 8].try_into().unwrap()));
        }
        Ok(Self { values })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + self.values.len() * 8);
        out.extend_from_slice(&(self.values.len() as u32).to_le_bytes());
        for v in &self.values {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }
}

pub struct PlainFixed;

impl Encoding for PlainFixed {
    type Payload = PlainFixedPayload;

    fn canonical_decode(payload: &Self::Payload) -> Result<Vec<i64>, QfError> {
        Ok(payload.values.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlainVarintPayload {
    pub values: Vec<i64>,
}

impl PlainVarintPayload {
    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < 4 {
            return Err(QfError::BufferTooShort);
        }
        let n = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let mut pos = 4usize;
        let mut values = Vec::with_capacity(n);
        for _ in 0..n {
            let (z, used) = wire::decode_u64_leb128(&bytes[pos..])?;
            pos += used;
            values.push(wire::zigzag_decode_i64(z));
        }
        Ok(Self { values })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(self.values.len() as u32).to_le_bytes());
        for v in &self.values {
            out.extend_from_slice(&wire::encode_u64_leb128(wire::zigzag_encode_i64(*v)));
        }
        out
    }
}

pub struct PlainVarint;

impl Encoding for PlainVarint {
    type Payload = PlainVarintPayload;

    fn canonical_decode(payload: &Self::Payload) -> Result<Vec<i64>, QfError> {
        Ok(payload.values.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::assert_parity;

    #[test]
    fn plain_fixed_round_trip() {
        let p = PlainFixedPayload {
            values: vec![1, -2, 3, -4],
        };
        let bytes = p.encode();
        assert_eq!(PlainFixedPayload::parse(&bytes).unwrap(), p);
        assert!(assert_parity::<PlainFixed>(&p).is_ok());
    }

    #[test]
    fn plain_varint_round_trip() {
        let p = PlainVarintPayload {
            values: vec![0, -1, 1, -2, 2, i64::MAX, i64::MIN],
        };
        let bytes = p.encode();
        assert_eq!(PlainVarintPayload::parse(&bytes).unwrap(), p);
        assert!(assert_parity::<PlainVarint>(&p).is_ok());
    }
}
