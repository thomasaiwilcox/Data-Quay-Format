//! Cove Format (COVE) v1.0 — Zone statistics (Spec §28).
//!
//! Zone stats summarise a contiguous range of rows so a planner can prove or
//! disprove a predicate without decoding. Spec §28.5 safety rules:
//! * `min`/`max` MUST be safe under the column's collation.
//! * If any value is NaN, [`ZoneStatFlags::HAS_NAN`] MUST be set and
//!   `min`/`max` MUST NOT include NaN.
//! * If any value is redacted, [`ZoneStatFlags::HAS_REDACTED`] MUST be set and
//!   stats MAY be absent.
//! * If `null_count == row_count` the zone is null-only and stats MUST be
//!   absent.

use crate::CoveError;

/// Scope at which a [`ZoneStats`] applies (Spec §28.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneScope {
    File,
    Table,
    Segment,
    Morsel,
    Page,
}

impl ZoneScope {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(ZoneScope::File),
            1 => Some(ZoneScope::Table),
            2 => Some(ZoneScope::Segment),
            3 => Some(ZoneScope::Morsel),
            4 => Some(ZoneScope::Page),
            _ => None,
        }
    }
}

/// Bit flags for [`ZoneStats::flags`] (Spec §28.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ZoneStatFlags(pub u32);

impl ZoneStatFlags {
    pub const HAS_NAN: Self = Self(0b0000_0001);
    pub const HAS_REDACTED: Self = Self(0b0000_0010);
    pub const TRUNCATED_MIN: Self = Self(0b0000_0100);
    pub const TRUNCATED_MAX: Self = Self(0b0000_1000);
    pub const SORTED_ASC: Self = Self(0b0001_0000);
    pub const SORTED_DESC: Self = Self(0b0010_0000);
    pub const ALL_DISTINCT: Self = Self(0b0100_0000);
    pub const CONSTANT: Self = Self(0b1000_0000);

    pub const fn empty() -> Self {
        Self(0)
    }
    pub const fn bits(self) -> u32 {
        self.0
    }
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

impl std::ops::BitOr for ZoneStatFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatScalar {
    pub bytes: Vec<u8>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoneStats {
    pub scope: ZoneScope,
    pub row_count: u64,
    pub null_count: u64,
    pub min: Option<StatScalar>,
    pub max: Option<StatScalar>,
    pub flags: ZoneStatFlags,
}

impl ZoneStats {
    /// Spec §28.5 safety invariants.
    pub fn validate(&self) -> Result<(), CoveError> {
        if self.null_count > self.row_count {
            return Err(CoveError::BadStats);
        }
        if self.null_count == self.row_count && (self.min.is_some() || self.max.is_some()) {
            return Err(CoveError::BadStats);
        }
        if self.flags.contains(ZoneStatFlags::CONSTANT) && self.min != self.max {
            return Err(CoveError::BadStats);
        }
        if self.flags.contains(ZoneStatFlags::SORTED_ASC)
            && self.flags.contains(ZoneStatFlags::SORTED_DESC)
            && self.row_count > 1
        {
            return Err(CoveError::BadStats);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(
        min: Option<&[u8]>,
        max: Option<&[u8]>,
        rc: u64,
        nc: u64,
        flags: ZoneStatFlags,
    ) -> ZoneStats {
        ZoneStats {
            scope: ZoneScope::Page,
            row_count: rc,
            null_count: nc,
            min: min.map(|b| StatScalar {
                bytes: b.to_vec(),
                truncated: false,
            }),
            max: max.map(|b| StatScalar {
                bytes: b.to_vec(),
                truncated: false,
            }),
            flags,
        }
    }

    #[test]
    fn spec_28_null_only_zone_must_omit_min_max() {
        let s = stats(Some(b"x"), Some(b"x"), 10, 10, ZoneStatFlags::empty());
        assert_eq!(s.validate(), Err(CoveError::BadStats));
    }

    #[test]
    fn spec_28_null_count_above_row_count_rejected() {
        let s = stats(None, None, 5, 10, ZoneStatFlags::empty());
        assert_eq!(s.validate(), Err(CoveError::BadStats));
    }

    #[test]
    fn spec_28_constant_flag_requires_min_eq_max() {
        let s = stats(Some(b"a"), Some(b"b"), 10, 0, ZoneStatFlags::CONSTANT);
        assert_eq!(s.validate(), Err(CoveError::BadStats));
    }

    #[test]
    fn spec_28_sorted_asc_and_desc_conflict() {
        let s = stats(
            Some(b"a"),
            Some(b"z"),
            10,
            0,
            ZoneStatFlags::SORTED_ASC | ZoneStatFlags::SORTED_DESC,
        );
        assert_eq!(s.validate(), Err(CoveError::BadStats));
    }
}
