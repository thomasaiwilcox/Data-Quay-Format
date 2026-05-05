//! Spec §20.3 — RunEnd encoding.
//!
//! Wire format (LE):
//! ```text
//! u32 run_count
//! repeat run_count times: i64 value
//! repeat run_count times: u32 run_end   // strictly increasing, ends at row_count
//! ```
//! `run_end[i]` is the exclusive row index where the run ending at `i`
//! stops. The first run covers rows `[0, run_end[0])`. Spec §20.3.3
//! requires strictly increasing ends.

use crate::QfError;

use super::Encoding;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunEndPayload {
    pub values: Vec<i64>,
    pub run_ends: Vec<u32>,
}

impl RunEndPayload {
    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < 4 {
            return Err(QfError::BufferTooShort);
        }
        let n = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let need = 4 + n * 8 + n * 4;
        if bytes.len() < need {
            return Err(QfError::BufferTooShort);
        }
        let mut values = Vec::with_capacity(n);
        for i in 0..n {
            let off = 4 + i * 8;
            values.push(i64::from_le_bytes(bytes[off..off + 8].try_into().unwrap()));
        }
        let mut run_ends = Vec::with_capacity(n);
        let mut prev: u64 = 0;
        for i in 0..n {
            let off = 4 + n * 8 + i * 4;
            let e = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
            if (e as u64) <= prev {
                return Err(QfError::PageCorrupt);
            }
            prev = e as u64;
            run_ends.push(e);
        }
        Ok(Self { values, run_ends })
    }
}

pub struct RunEnd;

impl Encoding for RunEnd {
    type Payload = RunEndPayload;

    fn canonical_decode(payload: &Self::Payload) -> Result<Vec<i64>, QfError> {
        if payload.values.len() != payload.run_ends.len() {
            return Err(QfError::PageCorrupt);
        }
        let total = *payload.run_ends.last().unwrap_or(&0) as usize;
        let mut out = Vec::with_capacity(total);
        let mut start: u32 = 0;
        for (v, end) in payload.values.iter().zip(payload.run_ends.iter()) {
            for _ in start..*end {
                out.push(*v);
            }
            start = *end;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::assert_parity;

    #[test]
    fn decode_simple() {
        let p = RunEndPayload {
            values: vec![10, 20, 30],
            run_ends: vec![2, 5, 6],
        };
        assert_eq!(
            RunEnd::canonical_decode(&p).unwrap(),
            vec![10, 10, 20, 20, 20, 30]
        );
        assert!(assert_parity::<RunEnd>(&p).is_ok());
    }

    #[test]
    fn rejects_non_increasing_run_ends() {
        let mut bytes = vec![];
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&1i64.to_le_bytes());
        bytes.extend_from_slice(&2i64.to_le_bytes());
        bytes.extend_from_slice(&5u32.to_le_bytes());
        bytes.extend_from_slice(&5u32.to_le_bytes()); // not strictly increasing
        assert_eq!(RunEndPayload::parse(&bytes), Err(QfError::PageCorrupt));
    }
}
