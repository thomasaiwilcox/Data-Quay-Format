//! Spec §52 — Nested data layouts (List, Struct, Map).
//!
//! These structures store **shape** only; element values live in child
//! arrays. Validators here enforce the v1 invariants: list offsets must be
//! monotonically non-decreasing, struct field counts agree on row count,
//! and map keys must be scalar with no duplicates *within a single map
//! value* (Spec §17.6, §52.4).
//!
//! Map duplicate-key policy (Spec §52.4): duplicates inside one map value
//! are an error in v1. Across map values, the same key may of course
//! repeat.

use std::collections::HashSet;

use crate::CoveError;

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

    pub fn row_count(&self) -> usize {
        self.offsets.len().saturating_sub(1)
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
        assert_eq!(l.row_count(), 3);
    }

    #[test]
    fn struct_fields_must_share_row_count() {
        let s = StructLayout {
            field_row_counts: vec![10, 9],
        };
        assert_eq!(s.validate(), Err(CoveError::PageCorrupt));
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
}
