//! Spec §20.3 — Sparse encoding.
//!
//! A page is mostly a single "fill" value, with a small set of override
//! positions. Wire layout (LE):
//! ```text
//! u32 row_count
//! i64 fill_value
//! u32 override_count
//! repeat override_count: u32 position | i64 value
//! ```
//! Spec §20.3.8 requires override positions to be strictly increasing and
//! distinct from any other override.

use crate::QfError;

use super::Encoding;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SparsePayload {
    pub row_count: u32,
    pub fill: i64,
    pub overrides: Vec<(u32, i64)>,
}

impl SparsePayload {
    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < 16 {
            return Err(QfError::BufferTooShort);
        }
        let row_count = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let fill = i64::from_le_bytes(bytes[4..12].try_into().unwrap());
        let oc = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        let need = 16 + oc * 12;
        if bytes.len() < need {
            return Err(QfError::BufferTooShort);
        }
        let mut overrides = Vec::with_capacity(oc);
        let mut prev: Option<u32> = None;
        for i in 0..oc {
            let off = 16 + i * 12;
            let p = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
            let v = i64::from_le_bytes(bytes[off + 4..off + 12].try_into().unwrap());
            if let Some(prev_pos) = prev {
                if p <= prev_pos {
                    return Err(QfError::PageCorrupt);
                }
            }
            if p >= row_count {
                return Err(QfError::PageCorrupt);
            }
            prev = Some(p);
            overrides.push((p, v));
        }
        Ok(Self {
            row_count,
            fill,
            overrides,
        })
    }
}

pub struct Sparse;

impl Encoding for Sparse {
    type Payload = SparsePayload;

    fn canonical_decode(payload: &Self::Payload) -> Result<Vec<i64>, QfError> {
        let mut out = vec![payload.fill; payload.row_count as usize];
        for (p, v) in &payload.overrides {
            out[*p as usize] = *v;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_with_overrides() {
        let p = SparsePayload {
            row_count: 5,
            fill: 0,
            overrides: vec![(1, 42), (4, -7)],
        };
        assert_eq!(Sparse::canonical_decode(&p).unwrap(), vec![0, 42, 0, 0, -7]);
    }
}
