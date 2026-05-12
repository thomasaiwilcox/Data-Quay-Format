//! Spec §53 — Sort and clustering hints (spec-exact).
//!
//! These structures are *advisory*. A reader MAY rely on them for ordered
//! scans and partition pruning; a producer that cannot uphold the declared
//! order MUST clear the hint rather than emit incorrect data.
//!
//! This module owns:
//!
//! * [`SortDirection`] — Spec §53.1: 0=Ascending, 1=Descending.
//! * [`NullOrder`] — Spec §53.1: 0=NullsFirst, 1=NullsLast.
//! * [`SortKeyEntryV1`] — Spec §53.1 fixed 8-byte entry.
//! * [`ClusteringStrength`] — Spec §53: 0=unknown, 255=perfect.
//! * [`ClusteringKeyEntryV1`] — Spec §53.3 fixed 8-byte entry.
//!
//! Each entry is encoded as a fixed-size little-endian record so it can
//! be embedded directly in a `TableEntryV1` (Spec §24) array of sort or
//! clustering keys without any per-entry framing.

use crate::CoveError;

// ── SortKeyEntryV1 (Spec §53.1) ──────────────────────────────────────────────

/// Encoded length of a [`SortKeyEntryV1`] = 8 bytes.
///
/// Layout: column_id(4) + direction(1) + null_order(1) + collation_id(2).
pub const SORT_KEY_ENTRY_LEN: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
#[non_exhaustive]
pub enum SortDirection {
    Ascending = 0,
    Descending = 1,
}

impl SortDirection {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Ascending),
            1 => Some(Self::Descending),
            _ => None,
        }
    }
}

/// Where nulls sort relative to non-null values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
#[non_exhaustive]
pub enum NullOrder {
    NullsFirst = 0,
    NullsLast = 1,
}

impl NullOrder {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::NullsFirst),
            1 => Some(Self::NullsLast),
            _ => None,
        }
    }
}

/// Spec §53.1 `SortKeyEntryV1`.
///
/// `collation_id` is a registered collation ID (Spec §22). `0` selects
/// the column's logical-type default collation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortKeyEntryV1 {
    pub column_id: u32,
    pub direction: SortDirection,
    pub null_order: NullOrder,
    pub collation_id: u16,
}

impl SortKeyEntryV1 {
    pub fn serialize(&self) -> [u8; SORT_KEY_ENTRY_LEN] {
        let mut buf = [0u8; SORT_KEY_ENTRY_LEN];
        buf[0..4].copy_from_slice(&self.column_id.to_le_bytes());
        buf[4] = self.direction as u8;
        buf[5] = self.null_order as u8;
        buf[6..8].copy_from_slice(&self.collation_id.to_le_bytes());
        buf
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < SORT_KEY_ENTRY_LEN {
            return Err(CoveError::BufferTooShort);
        }
        let column_id = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let direction = SortDirection::from_u8(bytes[4])
            .ok_or_else(|| CoveError::BadSection(format!("bad sort direction {}", bytes[4])))?;
        let null_order = NullOrder::from_u8(bytes[5])
            .ok_or_else(|| CoveError::BadSection(format!("bad null order {}", bytes[5])))?;
        let collation_id = u16::from_le_bytes(bytes[6..8].try_into().unwrap());
        Ok(Self {
            column_id,
            direction,
            null_order,
            collation_id,
        })
    }
}

// ── ClusteringKeyEntryV1 (Spec §53.3) ────────────────────────────────────────

/// Encoded length of a [`ClusteringKeyEntryV1`] = 8 bytes.
///
/// Layout: column_id(4) + clustering_strength(1) + reserved(3).
pub const CLUSTERING_KEY_ENTRY_LEN: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClusteringStrength(pub u8);

impl ClusteringStrength {
    pub const UNKNOWN: Self = Self(0);
    pub const PERFECT: Self = Self(255);

    pub const fn from_u8(v: u8) -> Self {
        Self(v)
    }

    pub const fn as_u8(self) -> u8 {
        self.0
    }

    pub const fn is_unknown(self) -> bool {
        self.0 == Self::UNKNOWN.0
    }

    pub const fn is_perfect(self) -> bool {
        self.0 == Self::PERFECT.0
    }
}

/// Spec §53.3 `ClusteringKeyEntryV1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClusteringKeyEntryV1 {
    pub column_id: u32,
    pub clustering_strength: ClusteringStrength,
    /// Reserved bytes. Producers MUST write zero; readers MUST tolerate
    /// any value for forward compatibility (Spec §53.3).
    pub reserved: [u8; 3],
}

impl ClusteringKeyEntryV1 {
    pub fn serialize(&self) -> [u8; CLUSTERING_KEY_ENTRY_LEN] {
        let mut buf = [0u8; CLUSTERING_KEY_ENTRY_LEN];
        buf[0..4].copy_from_slice(&self.column_id.to_le_bytes());
        buf[4] = self.clustering_strength.as_u8();
        buf[5..8].copy_from_slice(&self.reserved);
        buf
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < CLUSTERING_KEY_ENTRY_LEN {
            return Err(CoveError::BufferTooShort);
        }
        let column_id = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let clustering_strength = ClusteringStrength::from_u8(bytes[4]);
        let mut reserved = [0u8; 3];
        reserved.copy_from_slice(&bytes[5..8]);
        Ok(Self {
            column_id,
            clustering_strength,
            reserved,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_key_entry_round_trip() {
        let e = SortKeyEntryV1 {
            column_id: 7,
            direction: SortDirection::Descending,
            null_order: NullOrder::NullsLast,
            collation_id: 42,
        };
        let bytes = e.serialize();
        assert_eq!(bytes.len(), SORT_KEY_ENTRY_LEN);
        assert_eq!(SortKeyEntryV1::parse(&bytes).unwrap(), e);
    }

    #[test]
    fn sort_key_entry_rejects_bad_direction() {
        let mut bytes = SortKeyEntryV1 {
            column_id: 1,
            direction: SortDirection::Ascending,
            null_order: NullOrder::NullsFirst,
            collation_id: 0,
        }
        .serialize();
        bytes[4] = 9;
        assert!(matches!(
            SortKeyEntryV1::parse(&bytes),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn sort_key_entry_rejects_bad_null_order() {
        let mut bytes = SortKeyEntryV1 {
            column_id: 1,
            direction: SortDirection::Ascending,
            null_order: NullOrder::NullsFirst,
            collation_id: 0,
        }
        .serialize();
        bytes[5] = 7;
        assert!(matches!(
            SortKeyEntryV1::parse(&bytes),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn sort_key_entry_rejects_short_buffer() {
        let bytes = [0u8; 4];
        assert_eq!(
            SortKeyEntryV1::parse(&bytes),
            Err(CoveError::BufferTooShort)
        );
    }

    #[test]
    fn clustering_key_entry_round_trip() {
        let e = ClusteringKeyEntryV1 {
            column_id: 11,
            clustering_strength: ClusteringStrength::PERFECT,
            reserved: [0, 0, 0],
        };
        let bytes = e.serialize();
        assert_eq!(bytes.len(), CLUSTERING_KEY_ENTRY_LEN);
        assert_eq!(ClusteringKeyEntryV1::parse(&bytes).unwrap(), e);
    }

    #[test]
    fn clustering_key_entry_tolerates_nonzero_reserved() {
        // Spec §53.3: readers MUST tolerate any reserved bytes.
        let mut bytes = ClusteringKeyEntryV1 {
            column_id: 11,
            clustering_strength: ClusteringStrength::UNKNOWN,
            reserved: [0, 0, 0],
        }
        .serialize();
        bytes[5] = 0xAA;
        bytes[6] = 0xBB;
        bytes[7] = 0xCC;
        let parsed = ClusteringKeyEntryV1::parse(&bytes).unwrap();
        assert_eq!(parsed.reserved, [0xAA, 0xBB, 0xCC]);
    }

    #[test]
    fn clustering_key_entry_preserves_intermediate_strength() {
        let mut bytes = ClusteringKeyEntryV1 {
            column_id: 0,
            clustering_strength: ClusteringStrength::UNKNOWN,
            reserved: [0; 3],
        }
        .serialize();
        bytes[4] = 5;
        let parsed = ClusteringKeyEntryV1::parse(&bytes).unwrap();
        assert_eq!(parsed.clustering_strength, ClusteringStrength::from_u8(5));
        assert!(!parsed.clustering_strength.is_unknown());
        assert!(!parsed.clustering_strength.is_perfect());
    }
}
