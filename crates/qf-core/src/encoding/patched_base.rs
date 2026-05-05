//! Spec §20.3 — Patched-Base encoding.
//!
//! Most rows are stored as a small base value (typically delivered through
//! BitPacked or Frame-of-Reference); a small set of "patches" overrides
//! specific positions with full-width values. Wire layout (LE):
//! ```text
//! u32 row_count
//! i64 base_values[row_count]
//! u32 patch_count
//! repeat patch_count: u32 position | i64 value
//! ```
//! Spec §20.3.7 requires patch positions to be unique and strictly
//! increasing; the parser enforces both.

use crate::QfError;

use super::Encoding;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchedBasePayload {
    pub base: Vec<i64>,
    pub patches: Vec<(u32, i64)>,
}

impl PatchedBasePayload {
    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < 4 {
            return Err(QfError::BufferTooShort);
        }
        let n = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let mut pos = 4usize;
        if bytes.len() < pos + n * 8 + 4 {
            return Err(QfError::BufferTooShort);
        }
        let mut base = Vec::with_capacity(n);
        for _ in 0..n {
            base.push(i64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap()));
            pos += 8;
        }
        let pc = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;
        if bytes.len() < pos + pc * 12 {
            return Err(QfError::BufferTooShort);
        }
        let mut patches = Vec::with_capacity(pc);
        let mut prev: Option<u32> = None;
        for _ in 0..pc {
            let p = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
            let v = i64::from_le_bytes(bytes[pos + 4..pos + 12].try_into().unwrap());
            pos += 12;
            if let Some(prev_pos) = prev {
                if p <= prev_pos {
                    return Err(QfError::PageCorrupt);
                }
            }
            if (p as usize) >= n {
                return Err(QfError::PageCorrupt);
            }
            prev = Some(p);
            patches.push((p, v));
        }
        Ok(Self { base, patches })
    }
}

pub struct PatchedBase;

impl Encoding for PatchedBase {
    type Payload = PatchedBasePayload;

    fn canonical_decode(payload: &Self::Payload) -> Result<Vec<i64>, QfError> {
        let mut out = payload.base.clone();
        for (p, v) in &payload.patches {
            let idx = *p as usize;
            if idx >= out.len() {
                return Err(QfError::PageCorrupt);
            }
            out[idx] = *v;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_applies_patches_in_order() {
        let p = PatchedBasePayload {
            base: vec![0, 0, 0, 0],
            patches: vec![(1, 10), (3, 20)],
        };
        assert_eq!(
            PatchedBase::canonical_decode(&p).unwrap(),
            vec![0, 10, 0, 20]
        );
    }

    #[test]
    fn rejects_non_increasing_patches() {
        // n=2, base=[0,0], patches=[(1,?), (1,?)]
        let mut bytes = vec![];
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&0i64.to_le_bytes());
        bytes.extend_from_slice(&0i64.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&1i64.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&2i64.to_le_bytes());
        assert_eq!(PatchedBasePayload::parse(&bytes), Err(QfError::PageCorrupt));
    }
}
