//! Spec §20.3 — Run-Length Encoding (RLE).
//!
//! Wire format (LE):
//! ```text
//! u32 run_count
//! repeat run_count times:
//!   i64 value
//!   u32 length   // length > 0
//! ```
//! Spec §20.3.2 forbids zero-length runs and forbids two adjacent runs with
//! the same value (the encoder MUST coalesce them). The parser enforces
//! both invariants so corrupt pages fail closed.

use crate::CoveError;

use super::Encoding;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RlePayload {
    pub runs: Vec<(i64, u32)>,
}

impl RlePayload {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < 4 {
            return Err(CoveError::BufferTooShort);
        }
        let run_count = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let need = 4 + run_count * 12;
        if bytes.len() < need {
            return Err(CoveError::BufferTooShort);
        }
        let mut runs = Vec::with_capacity(run_count);
        let mut total: u64 = 0;
        let mut prev_value: Option<i64> = None;
        for i in 0..run_count {
            let off = 4 + i * 12;
            let v = i64::from_le_bytes(bytes[off..off + 8].try_into().unwrap());
            let len = u32::from_le_bytes(bytes[off + 8..off + 12].try_into().unwrap());
            if len == 0 {
                return Err(CoveError::PageCorrupt);
            }
            if prev_value == Some(v) {
                return Err(CoveError::PageCorrupt);
            }
            total = total
                .checked_add(len as u64)
                .ok_or(CoveError::ArithOverflow)?;
            prev_value = Some(v);
            runs.push((v, len));
        }
        if total > u32::MAX as u64 {
            return Err(CoveError::PageCorrupt);
        }
        Ok(Self { runs })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + self.runs.len() * 12);
        out.extend_from_slice(&(self.runs.len() as u32).to_le_bytes());
        for (v, len) in &self.runs {
            out.extend_from_slice(&v.to_le_bytes());
            out.extend_from_slice(&len.to_le_bytes());
        }
        out
    }
}

pub struct Rle;

impl Encoding for Rle {
    type Payload = RlePayload;

    fn canonical_decode(payload: &Self::Payload) -> Result<Vec<i64>, CoveError> {
        let total: u64 = payload.runs.iter().map(|(_, l)| *l as u64).sum();
        let mut out = Vec::with_capacity(total as usize);
        for (v, len) in &payload.runs {
            out.resize(out.len() + *len as usize, *v);
        }
        Ok(out)
    }

    fn fast_decode(payload: &Self::Payload) -> Result<Vec<i64>, CoveError> {
        // Memset-style: extend_from_slice with prebuilt repeat slices.
        let total: usize = payload.runs.iter().map(|(_, l)| *l as usize).sum();
        let mut out = Vec::with_capacity(total);
        for (v, len) in &payload.runs {
            out.resize(out.len() + *len as usize, *v);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::assert_parity;

    #[test]
    fn round_trip() {
        let p = RlePayload {
            runs: vec![(1, 3), (2, 1), (1, 2)],
        };
        let bytes = p.encode();
        assert_eq!(RlePayload::parse(&bytes).unwrap(), p);
        assert_eq!(Rle::canonical_decode(&p).unwrap(), vec![1, 1, 1, 2, 1, 1]);
    }

    #[test]
    fn rejects_zero_length_run() {
        let mut bytes = vec![];
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&0i64.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(RlePayload::parse(&bytes), Err(CoveError::PageCorrupt));
    }

    #[test]
    fn rejects_adjacent_duplicate_runs() {
        let mut bytes = vec![];
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&5i64.to_le_bytes());
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&5i64.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        assert_eq!(RlePayload::parse(&bytes), Err(CoveError::PageCorrupt));
    }

    #[test]
    fn parity_holds() {
        let p = RlePayload {
            runs: vec![(7, 5), (-3, 2), (0, 8)],
        };
        assert!(assert_parity::<Rle>(&p).is_ok());
    }
}
