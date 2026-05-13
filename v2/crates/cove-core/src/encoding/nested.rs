//! Spec §52 — Nested data layouts (List, Struct, Map).
//!
//! These structures store **shape** only; element values live in child
//! arrays. Validators here enforce the v2 invariants: list offsets must be
//! monotonically non-decreasing, struct field counts agree on row count,
//! and map keys must be scalar with no duplicates *within a single map
//! value* (Spec §17.6, §52.4).
//!
//! Map duplicate-key policy (Spec §52.4): duplicates inside one map value
//! are an error in v2. Across map values, the same key may of course
//! repeat.

use std::collections::HashSet;

use crate::{wire, CoveError};

/// List layout: `offsets` of length `row_count + 1`. Element `i` covers
/// child rows `[offsets[i], offsets[i+1])`. Spec §52.2 requires
/// `offsets[0] == 0` and monotonic non-decreasing offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListLayout {
    pub offsets: Vec<u32>,
}

impl ListLayout {
    pub fn validate(&self) -> Result<(), CoveError> {
        if self.offsets.is_empty() {
            return Err(CoveError::PageCorrupt);
        }
        if self.offsets[0] != 0 {
            return Err(CoveError::PageCorrupt);
        }
        for w in self.offsets.windows(2) {
            if w[0] > w[1] {
                return Err(CoveError::PageCorrupt);
            }
        }
        Ok(())
    }

    pub fn validate_child_count(&self, child_row_count: usize) -> Result<(), CoveError> {
        self.validate()?;
        if self.offsets.last().copied().unwrap() as usize != child_row_count {
            return Err(CoveError::PageCorrupt);
        }
        Ok(())
    }

    pub fn row_count(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }
}

/// Page-local list payload used by the reference scan writer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListLayoutPayload {
    pub layout: ListLayout,
    pub child_row_count: u32,
}

impl ListLayoutPayload {
    /// Wire format (LE): `u32 child_row_count | u32 offset_count | u32[offset_count]`.
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < 8 {
            return Err(CoveError::BufferTooShort);
        }
        let child_row_count = read_u32_le(bytes, 0)?;
        let offset_count = read_u32_le(bytes, 4)? as usize;
        let offsets_bytes_len = offset_count
            .checked_mul(4)
            .ok_or(CoveError::ArithOverflow)?;
        let offsets_start = 8usize;
        let offsets_end = offsets_start
            .checked_add(offsets_bytes_len)
            .ok_or(CoveError::ArithOverflow)?;
        if offsets_end > bytes.len() {
            return Err(CoveError::BufferTooShort);
        }
        if offsets_end < bytes.len() {
            return Err(CoveError::PageCorrupt);
        }
        let mut offsets = Vec::with_capacity(offset_count);
        for index in 0..offset_count {
            offsets.push(read_u32_le(bytes, offsets_start + index * 4)?);
        }
        Ok(Self {
            layout: ListLayout { offsets },
            child_row_count,
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + self.layout.offsets.len() * 4);
        out.extend_from_slice(&self.child_row_count.to_le_bytes());
        out.extend_from_slice(&(self.layout.offsets.len() as u32).to_le_bytes());
        for offset in &self.layout.offsets {
            out.extend_from_slice(&offset.to_le_bytes());
        }
        out
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        self.layout
            .validate_child_count(self.child_row_count as usize)
    }
}

/// Struct layout: every child column reports the same `row_count`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructLayout {
    pub field_row_counts: Vec<u64>,
}

impl StructLayout {
    pub fn validate(&self) -> Result<(), CoveError> {
        if self.field_row_counts.is_empty() {
            return Err(CoveError::PageCorrupt);
        }
        let head = self.field_row_counts[0];
        for r in &self.field_row_counts[1..] {
            if *r != head {
                return Err(CoveError::PageCorrupt);
            }
        }
        Ok(())
    }

    pub fn validate_parent_row_count(
        &self,
        parent_row_count: u64,
        parent_null_handling_declared: bool,
    ) -> Result<(), CoveError> {
        self.validate()?;
        if !parent_null_handling_declared {
            return Err(CoveError::PageCorrupt);
        }
        if self.field_row_counts[0] != parent_row_count {
            return Err(CoveError::PageCorrupt);
        }
        Ok(())
    }

    pub fn row_count(&self) -> Result<u64, CoveError> {
        self.validate()?;
        Ok(self.field_row_counts[0])
    }
}

/// Page-local struct payload used by the reference scan writer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructLayoutPayload {
    pub layout: StructLayout,
    pub parent_null_handling_declared: bool,
}

impl StructLayoutPayload {
    /// Wire format (LE):
    /// `u8 parent_null_handling_declared | u8 reserved[3] | u32 field_count | u64[field_count]`.
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < 8 {
            return Err(CoveError::BufferTooShort);
        }
        if bytes[1..4] != [0, 0, 0] {
            return Err(CoveError::PageCorrupt);
        }
        let parent_null_handling_declared = match bytes[0] {
            0 => false,
            1 => true,
            _ => return Err(CoveError::PageCorrupt),
        };
        let field_count = read_u32_le(bytes, 4)? as usize;
        let counts_bytes_len = field_count.checked_mul(8).ok_or(CoveError::ArithOverflow)?;
        let counts_start = 8usize;
        let counts_end = counts_start
            .checked_add(counts_bytes_len)
            .ok_or(CoveError::ArithOverflow)?;
        if counts_end > bytes.len() {
            return Err(CoveError::BufferTooShort);
        }
        if counts_end < bytes.len() {
            return Err(CoveError::PageCorrupt);
        }
        let mut field_row_counts = Vec::with_capacity(field_count);
        for index in 0..field_count {
            field_row_counts.push(read_u64_le(bytes, counts_start + index * 8)?);
        }
        Ok(Self {
            layout: StructLayout { field_row_counts },
            parent_null_handling_declared,
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + self.layout.field_row_counts.len() * 8);
        out.push(u8::from(self.parent_null_handling_declared));
        out.extend_from_slice(&[0, 0, 0]);
        out.extend_from_slice(&(self.layout.field_row_counts.len() as u32).to_le_bytes());
        for count in &self.layout.field_row_counts {
            out.extend_from_slice(&count.to_le_bytes());
        }
        out
    }

    pub fn validate(&self, parent_row_count: u64) -> Result<(), CoveError> {
        self.layout
            .validate_parent_row_count(parent_row_count, self.parent_null_handling_declared)
    }
}

/// Map layout: offset-based parent shape plus child row counts and
/// canonicalised keys for duplicate-key validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapLayout {
    pub offsets: Vec<u32>,
    pub key_row_count: u32,
    pub value_row_count: u32,
    pub keys_are_scalar: bool,
    pub allow_duplicate_keys: bool,
    pub canonical_keys: Vec<Vec<u8>>,
}

impl MapLayout {
    pub fn validate(&self) -> Result<(), CoveError> {
        if !self.keys_are_scalar {
            return Err(CoveError::PageCorrupt);
        }

        let list_layout = ListLayout {
            offsets: self.offsets.clone(),
        };
        list_layout.validate()?;

        if self.key_row_count != self.value_row_count {
            return Err(CoveError::PageCorrupt);
        }

        list_layout.validate_child_count(self.key_row_count as usize)?;

        if self.canonical_keys.len() != self.key_row_count as usize {
            return Err(CoveError::PageCorrupt);
        }

        if !self.allow_duplicate_keys {
            for pair in self.offsets.windows(2) {
                let start = pair[0] as usize;
                let end = pair[1] as usize;
                validate_no_duplicate_keys(&self.canonical_keys[start..end])?;
            }
        }

        Ok(())
    }

    pub fn row_count(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }
}

/// Page-local map payload used by the reference scan writer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapLayoutPayload {
    pub layout: MapLayout,
}

impl MapLayoutPayload {
    /// Wire format (LE):
    /// `u32 offset_count | u32 key_row_count | u32 value_row_count |
    ///  u8 keys_are_scalar | u8 allow_duplicate_keys | u16 reserved |
    ///  u32[offset_count] | repeated(u32 key_len | [u8; key_len])`.
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < 16 {
            return Err(CoveError::BufferTooShort);
        }
        let offset_count = read_u32_le(bytes, 0)? as usize;
        let key_row_count = read_u32_le(bytes, 4)?;
        let value_row_count = read_u32_le(bytes, 8)?;
        let keys_are_scalar = match bytes[12] {
            0 => false,
            1 => true,
            _ => return Err(CoveError::PageCorrupt),
        };
        let allow_duplicate_keys = match bytes[13] {
            0 => false,
            1 => true,
            _ => return Err(CoveError::PageCorrupt),
        };
        if bytes[14..16] != [0, 0] {
            return Err(CoveError::PageCorrupt);
        }

        let offsets_bytes_len = offset_count
            .checked_mul(4)
            .ok_or(CoveError::ArithOverflow)?;
        let offsets_start = 16usize;
        let offsets_end = offsets_start
            .checked_add(offsets_bytes_len)
            .ok_or(CoveError::ArithOverflow)?;
        if offsets_end > bytes.len() {
            return Err(CoveError::BufferTooShort);
        }
        let mut offsets = Vec::with_capacity(offset_count);
        for index in 0..offset_count {
            offsets.push(read_u32_le(bytes, offsets_start + index * 4)?);
        }

        let mut pos = offsets_end;
        let mut canonical_keys = Vec::with_capacity(key_row_count as usize);
        for _ in 0..key_row_count {
            let key_len = read_u32_le(bytes, pos)? as usize;
            pos = pos.checked_add(4).ok_or(CoveError::ArithOverflow)?;
            let key = wire::read_range_checked(bytes, pos, key_len)?.to_vec();
            pos = pos.checked_add(key_len).ok_or(CoveError::ArithOverflow)?;
            canonical_keys.push(key);
        }
        if pos != bytes.len() {
            return Err(CoveError::PageCorrupt);
        }

        Ok(Self {
            layout: MapLayout {
                offsets,
                key_row_count,
                value_row_count,
                keys_are_scalar,
                allow_duplicate_keys,
                canonical_keys,
            },
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(self.layout.offsets.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.layout.key_row_count.to_le_bytes());
        out.extend_from_slice(&self.layout.value_row_count.to_le_bytes());
        out.push(u8::from(self.layout.keys_are_scalar));
        out.push(u8::from(self.layout.allow_duplicate_keys));
        out.extend_from_slice(&0u16.to_le_bytes());
        for offset in &self.layout.offsets {
            out.extend_from_slice(&offset.to_le_bytes());
        }
        for key in &self.layout.canonical_keys {
            out.extend_from_slice(&(key.len() as u32).to_le_bytes());
            out.extend_from_slice(key);
        }
        out
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        self.layout.validate()
    }
}

/// Validate that a single map value contains no duplicate keys (Spec §52.4).
/// Keys are passed as their canonical byte forms.
pub fn validate_no_duplicate_keys(keys: &[Vec<u8>]) -> Result<(), CoveError> {
    let mut seen: HashSet<&[u8]> = HashSet::with_capacity(keys.len());
    for k in keys {
        if !seen.insert(k.as_slice()) {
            return Err(CoveError::PageCorrupt);
        }
    }
    Ok(())
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Result<u32, CoveError> {
    let slice = wire::read_range_checked(bytes, offset, 4)?;
    Ok(u32::from_le_bytes(slice.try_into().unwrap()))
}

fn read_u64_le(bytes: &[u8], offset: usize) -> Result<u64, CoveError> {
    let slice = wire::read_range_checked(bytes, offset, 8)?;
    Ok(u64::from_le_bytes(slice.try_into().unwrap()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_offsets_must_start_at_zero() {
        let l = ListLayout {
            offsets: vec![1, 2],
        };
        assert_eq!(l.validate(), Err(CoveError::PageCorrupt));
    }

    #[test]
    fn list_offsets_must_be_monotonic() {
        let l = ListLayout {
            offsets: vec![0, 5, 4],
        };
        assert_eq!(l.validate(), Err(CoveError::PageCorrupt));
    }

    #[test]
    fn list_valid() {
        let l = ListLayout {
            offsets: vec![0, 3, 3, 7],
        };
        l.validate().unwrap();
        l.validate_child_count(7).unwrap();
        assert_eq!(l.row_count(), 3);
    }

    #[test]
    fn list_last_offset_must_match_child_count() {
        let l = ListLayout {
            offsets: vec![0, 3, 3, 7],
        };
        assert_eq!(l.validate_child_count(6), Err(CoveError::PageCorrupt));
    }

    #[test]
    fn struct_fields_must_share_row_count() {
        let s = StructLayout {
            field_row_counts: vec![10, 9],
        };
        assert_eq!(s.validate(), Err(CoveError::PageCorrupt));
    }

    #[test]
    fn struct_parent_row_count_and_null_handling_must_be_declared() {
        let s = StructLayout {
            field_row_counts: vec![10, 10],
        };
        assert_eq!(
            s.validate_parent_row_count(10, false),
            Err(CoveError::PageCorrupt)
        );
        assert_eq!(
            s.validate_parent_row_count(9, true),
            Err(CoveError::PageCorrupt)
        );
        s.validate_parent_row_count(10, true).unwrap();
        assert_eq!(s.row_count().unwrap(), 10);
    }

    #[test]
    fn map_duplicate_key_within_value_rejected() {
        let keys = vec![b"a".to_vec(), b"a".to_vec()];
        assert_eq!(
            validate_no_duplicate_keys(&keys),
            Err(CoveError::PageCorrupt)
        );
    }

    #[test]
    fn map_distinct_keys_ok() {
        let keys = vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec()];
        validate_no_duplicate_keys(&keys).unwrap();
    }

    #[test]
    fn map_layout_requires_matching_child_counts_and_scalar_keys() {
        let map = MapLayout {
            offsets: vec![0, 2],
            key_row_count: 2,
            value_row_count: 1,
            keys_are_scalar: true,
            allow_duplicate_keys: false,
            canonical_keys: vec![b"a".to_vec(), b"b".to_vec()],
        };
        assert_eq!(map.validate(), Err(CoveError::PageCorrupt));

        let non_scalar = MapLayout {
            offsets: vec![0, 1],
            key_row_count: 1,
            value_row_count: 1,
            keys_are_scalar: false,
            allow_duplicate_keys: false,
            canonical_keys: vec![b"a".to_vec()],
        };
        assert_eq!(non_scalar.validate(), Err(CoveError::PageCorrupt));
    }

    #[test]
    fn map_layout_rejects_duplicate_keys_per_value() {
        let map = MapLayout {
            offsets: vec![0, 2, 3],
            key_row_count: 3,
            value_row_count: 3,
            keys_are_scalar: true,
            allow_duplicate_keys: false,
            canonical_keys: vec![b"a".to_vec(), b"a".to_vec(), b"a".to_vec()],
        };
        assert_eq!(map.validate(), Err(CoveError::PageCorrupt));
    }

    #[test]
    fn map_layout_allows_cross_value_key_reuse() {
        let map = MapLayout {
            offsets: vec![0, 1, 2],
            key_row_count: 2,
            value_row_count: 2,
            keys_are_scalar: true,
            allow_duplicate_keys: false,
            canonical_keys: vec![b"a".to_vec(), b"a".to_vec()],
        };
        map.validate().unwrap();
        assert_eq!(map.row_count(), 2);
    }

    #[test]
    fn list_payload_round_trips() {
        let payload = ListLayoutPayload {
            layout: ListLayout {
                offsets: vec![0, 2, 2, 5],
            },
            child_row_count: 5,
        };
        let bytes = payload.encode();
        let parsed = ListLayoutPayload::parse(&bytes).unwrap();
        assert_eq!(parsed, payload);
        parsed.validate().unwrap();
    }

    #[test]
    fn list_payload_trailing_bytes_are_page_corrupt() {
        let mut bytes = ListLayoutPayload {
            layout: ListLayout {
                offsets: vec![0, 2, 2, 5],
            },
            child_row_count: 5,
        }
        .encode();
        bytes.push(0);

        assert_eq!(
            ListLayoutPayload::parse(&bytes),
            Err(CoveError::PageCorrupt)
        );
    }

    #[test]
    fn struct_payload_round_trips() {
        let payload = StructLayoutPayload {
            layout: StructLayout {
                field_row_counts: vec![3, 3],
            },
            parent_null_handling_declared: true,
        };
        let bytes = payload.encode();
        let parsed = StructLayoutPayload::parse(&bytes).unwrap();
        assert_eq!(parsed, payload);
        parsed.validate(3).unwrap();
    }

    #[test]
    fn struct_payload_trailing_bytes_are_page_corrupt() {
        let mut bytes = StructLayoutPayload {
            layout: StructLayout {
                field_row_counts: vec![3, 3],
            },
            parent_null_handling_declared: true,
        }
        .encode();
        bytes.push(0);

        assert_eq!(
            StructLayoutPayload::parse(&bytes),
            Err(CoveError::PageCorrupt)
        );
    }

    #[test]
    fn map_payload_round_trips() {
        let payload = MapLayoutPayload {
            layout: MapLayout {
                offsets: vec![0, 2, 3],
                key_row_count: 3,
                value_row_count: 3,
                keys_are_scalar: true,
                allow_duplicate_keys: false,
                canonical_keys: vec![b"a".to_vec(), b"b".to_vec(), b"a".to_vec()],
            },
        };
        let bytes = payload.encode();
        let parsed = MapLayoutPayload::parse(&bytes).unwrap();
        assert_eq!(parsed, payload);
        parsed.validate().unwrap();
    }
}
