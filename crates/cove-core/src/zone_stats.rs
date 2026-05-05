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

pub const STAT_SCALAR_ENCODED_LEN: usize = 20;
pub const ZONE_STATS_ENTRY_LEN: usize = 96;
const STAT_SCALAR_FLAG_TRUNCATED: u8 = 1 << 0;
const STAT_SCALAR_KNOWN_FLAGS: u8 = STAT_SCALAR_FLAG_TRUNCATED;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum StatKind {
    None = 0,
    Int64 = 1,
    UInt64 = 2,
    Float64Bits = 3,
    Decimal128 = 4,
    TimestampMicros = 5,
    TimestampNanos = 6,
    DateDays = 7,
    FixedBytes = 8,
}

impl StatKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::None),
            1 => Some(Self::Int64),
            2 => Some(Self::UInt64),
            3 => Some(Self::Float64Bits),
            4 => Some(Self::Decimal128),
            5 => Some(Self::TimestampMicros),
            6 => Some(Self::TimestampNanos),
            7 => Some(Self::DateDays),
            8 => Some(Self::FixedBytes),
            _ => None,
        }
    }

    fn fixed_len(self) -> Option<usize> {
        match self {
            Self::None => Some(0),
            Self::Int64
            | Self::UInt64
            | Self::Float64Bits
            | Self::TimestampMicros
            | Self::TimestampNanos => Some(8),
            Self::Decimal128 => Some(16),
            Self::DateDays => Some(4),
            Self::FixedBytes => None,
        }
    }
}

/// Bit flags for [`ZoneStats::flags`] (Spec §28.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ZoneStatFlags(pub u32);

impl ZoneStatFlags {
    pub const HAS_MIN_MAX: Self = Self(1 << 0);
    pub const HAS_DOMAIN_RANGE: Self = Self(1 << 1);
    pub const DISTINCT_EXACT: Self = Self(1 << 2);
    pub const CONSTANT: Self = Self(1 << 3);
    pub const SORTED_ASC: Self = Self(1 << 4);
    pub const SORTED_DESC: Self = Self(1 << 5);
    pub const HAS_NAN: Self = Self(1 << 6);
    pub const HAS_REDACTED: Self = Self(1 << 7);
    pub const MINMAX_TRUNCATED: Self = Self(1 << 8);
    pub const HAS_TOP_N_SUMMARY: Self = Self(1 << 9);
    pub const HAS_BOTTOM_N_SUMMARY: Self = Self(1 << 10);

    pub const TRUNCATED_MIN: Self = Self::MINMAX_TRUNCATED;
    pub const TRUNCATED_MAX: Self = Self::MINMAX_TRUNCATED;
    pub const ALL_DISTINCT: Self = Self::DISTINCT_EXACT;

    pub const fn empty() -> Self {
        Self(0)
    }
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
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
    pub kind: StatKind,
    pub bytes: Vec<u8>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NumericStatValue {
    Int64(i64),
    UInt64(u64),
    Float64(f64),
    Decimal128(i128),
    TimestampMicros(i64),
    TimestampNanos(i64),
    DateDays(i32),
}

impl StatScalar {
    pub fn numeric_value(&self) -> Option<NumericStatValue> {
        match self.kind {
            StatKind::Int64 => decode_i64_exact(&self.bytes).map(NumericStatValue::Int64),
            StatKind::UInt64 => decode_u64_exact(&self.bytes).map(NumericStatValue::UInt64),
            StatKind::Float64Bits => decode_u64_exact(&self.bytes)
                .map(f64::from_bits)
                .filter(|value| !value.is_nan())
                .map(NumericStatValue::Float64),
            StatKind::Decimal128 => {
                decode_i128_exact(&self.bytes).map(NumericStatValue::Decimal128)
            }
            StatKind::TimestampMicros => {
                decode_i64_exact(&self.bytes).map(NumericStatValue::TimestampMicros)
            }
            StatKind::TimestampNanos => {
                decode_i64_exact(&self.bytes).map(NumericStatValue::TimestampNanos)
            }
            StatKind::DateDays => decode_i32_exact(&self.bytes).map(NumericStatValue::DateDays),
            StatKind::None | StatKind::FixedBytes => None,
        }
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoneStatsEntry {
    pub table_id: u32,
    pub segment_id: u32,
    pub morsel_id: u32,
    pub column_id: u32,
    pub non_null_count: u32,
    pub distinct_count: u32,
    pub run_count: u32,
    pub stats: ZoneStats,
    pub min_domain_rank: u32,
    pub max_domain_rank: u32,
    pub exact_set_ref: u32,
    pub bloom_ref: u32,
}

impl ZoneStatsEntry {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < ZONE_STATS_ENTRY_LEN {
            return Err(CoveError::BufferTooShort);
        }
        let bytes = &bytes[..ZONE_STATS_ENTRY_LEN];
        let table_id = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let segment_id = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let morsel_id = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        let column_id = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
        let row_count = u32::from_le_bytes(bytes[16..20].try_into().unwrap());
        let null_count = u32::from_le_bytes(bytes[20..24].try_into().unwrap());
        let non_null_count = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
        let distinct_count = u32::from_le_bytes(bytes[28..32].try_into().unwrap());
        let run_count = u32::from_le_bytes(bytes[32..36].try_into().unwrap());
        let flags = ZoneStatFlags::from_bits(u32::from_le_bytes(bytes[36..40].try_into().unwrap()));
        let min = parse_stat_scalar(&bytes[40..40 + STAT_SCALAR_ENCODED_LEN])?;
        let max = parse_stat_scalar(&bytes[60..60 + STAT_SCALAR_ENCODED_LEN])?;
        let min_domain_rank = u32::from_le_bytes(bytes[80..84].try_into().unwrap());
        let max_domain_rank = u32::from_le_bytes(bytes[84..88].try_into().unwrap());
        let exact_set_ref = u32::from_le_bytes(bytes[88..92].try_into().unwrap());
        let bloom_ref = u32::from_le_bytes(bytes[92..96].try_into().unwrap());

        let entry = Self {
            table_id,
            segment_id,
            morsel_id,
            column_id,
            non_null_count,
            distinct_count,
            run_count,
            stats: ZoneStats {
                scope: if morsel_id == u32::MAX {
                    ZoneScope::Segment
                } else {
                    ZoneScope::Morsel
                },
                row_count: row_count as u64,
                null_count: null_count as u64,
                min,
                max,
                flags,
            },
            min_domain_rank,
            max_domain_rank,
            exact_set_ref,
            bloom_ref,
        };
        entry.validate()?;
        Ok(entry)
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        let row_count = u32::try_from(self.stats.row_count).map_err(|_| CoveError::BadStats)?;
        let null_count = u32::try_from(self.stats.null_count).map_err(|_| CoveError::BadStats)?;
        if null_count
            .checked_add(self.non_null_count)
            .ok_or(CoveError::ArithOverflow)?
            != row_count
        {
            return Err(CoveError::BadStats);
        }

        let has_min_max = self.stats.flags.contains(ZoneStatFlags::HAS_MIN_MAX);
        if self.stats.min.is_some() != self.stats.max.is_some() {
            return Err(CoveError::BadStats);
        }
        if has_min_max != self.stats.min.is_some() {
            return Err(CoveError::BadStats);
        }

        self.stats.validate()
    }

    /// Spec §28 — emit the canonical 96-byte wire form consumed by
    /// [`ZoneStatsEntry::parse`].
    pub fn serialize(&self) -> Result<[u8; ZONE_STATS_ENTRY_LEN], CoveError> {
        self.validate()?;
        let mut out = [0u8; ZONE_STATS_ENTRY_LEN];
        out[0..4].copy_from_slice(&self.table_id.to_le_bytes());
        out[4..8].copy_from_slice(&self.segment_id.to_le_bytes());
        out[8..12].copy_from_slice(&self.morsel_id.to_le_bytes());
        out[12..16].copy_from_slice(&self.column_id.to_le_bytes());
        let row_count = self.stats.row_count as u32;
        let null_count = self.stats.null_count as u32;
        out[16..20].copy_from_slice(&row_count.to_le_bytes());
        out[20..24].copy_from_slice(&null_count.to_le_bytes());
        out[24..28].copy_from_slice(&self.non_null_count.to_le_bytes());
        out[28..32].copy_from_slice(&self.distinct_count.to_le_bytes());
        out[32..36].copy_from_slice(&self.run_count.to_le_bytes());
        out[36..40].copy_from_slice(&self.stats.flags.bits().to_le_bytes());
        encode_stat_scalar(&self.stats.min, &mut out[40..40 + STAT_SCALAR_ENCODED_LEN])?;
        encode_stat_scalar(&self.stats.max, &mut out[60..60 + STAT_SCALAR_ENCODED_LEN])?;
        out[80..84].copy_from_slice(&self.min_domain_rank.to_le_bytes());
        out[84..88].copy_from_slice(&self.max_domain_rank.to_le_bytes());
        out[88..92].copy_from_slice(&self.exact_set_ref.to_le_bytes());
        out[92..96].copy_from_slice(&self.bloom_ref.to_le_bytes());
        Ok(out)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ZoneStatsSection {
    pub entries: Vec<ZoneStatsEntry>,
}

impl ZoneStatsSection {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.is_empty() {
            return Ok(Self {
                entries: Vec::new(),
            });
        }
        if !bytes.len().is_multiple_of(ZONE_STATS_ENTRY_LEN) {
            return Err(CoveError::BadStats);
        }

        let mut entries = Vec::with_capacity(bytes.len() / ZONE_STATS_ENTRY_LEN);
        for chunk in bytes.chunks_exact(ZONE_STATS_ENTRY_LEN) {
            entries.push(ZoneStatsEntry::parse(chunk)?);
        }

        Ok(Self { entries })
    }

    /// Spec §28 — emit the canonical concatenated wire form consumed by
    /// [`ZoneStatsSection::parse`].
    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        let mut out = Vec::with_capacity(self.entries.len() * ZONE_STATS_ENTRY_LEN);
        for entry in &self.entries {
            out.extend_from_slice(&entry.serialize()?);
        }
        Ok(out)
    }
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
        if let (Some(min), Some(max)) = (&self.min, &self.max) {
            if min.kind != max.kind {
                return Err(CoveError::BadStats);
            }
        }
        let scalar_truncated = [&self.min, &self.max]
            .into_iter()
            .flatten()
            .any(|scalar| scalar.truncated);
        if scalar_truncated != self.flags.contains(ZoneStatFlags::MINMAX_TRUNCATED) {
            return Err(CoveError::BadStats);
        }
        for scalar in [&self.min, &self.max].into_iter().flatten() {
            validate_stat_scalar_shape(scalar)?;
            if scalar.kind == StatKind::Float64Bits {
                let Some(bits) = decode_u64_exact(&scalar.bytes) else {
                    return Err(CoveError::BadStats);
                };
                if f64::from_bits(bits).is_nan() {
                    return Err(CoveError::BadStats);
                }
            }
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

fn parse_stat_scalar(bytes: &[u8]) -> Result<Option<StatScalar>, CoveError> {
    if bytes.len() < STAT_SCALAR_ENCODED_LEN {
        return Err(CoveError::BufferTooShort);
    }
    let kind = StatKind::from_u8(bytes[0]).ok_or(CoveError::BadStats)?;
    let flags = bytes[1];
    if flags & !STAT_SCALAR_KNOWN_FLAGS != 0 {
        return Err(CoveError::BadStats);
    }
    let length = u16::from_le_bytes(bytes[2..4].try_into().unwrap()) as usize;
    if length > 16 {
        return Err(CoveError::BadStats);
    }
    if kind == StatKind::None {
        if length != 0 || flags != 0 {
            return Err(CoveError::BadStats);
        }
        return Ok(None);
    }
    let scalar = StatScalar {
        kind,
        bytes: bytes[4..4 + length].to_vec(),
        truncated: flags & STAT_SCALAR_FLAG_TRUNCATED != 0,
    };
    validate_stat_scalar_shape(&scalar)?;
    Ok(Some(scalar))
}

fn validate_stat_scalar_shape(scalar: &StatScalar) -> Result<(), CoveError> {
    if scalar.bytes.len() > 16 {
        return Err(CoveError::BadStats);
    }
    if let Some(fixed_len) = scalar.kind.fixed_len() {
        if scalar.bytes.len() != fixed_len {
            return Err(CoveError::BadStats);
        }
    }
    Ok(())
}

/// Spec §28 — inverse of [`parse_stat_scalar`]. The destination slice must be
/// exactly [`STAT_SCALAR_ENCODED_LEN`] bytes.
fn encode_stat_scalar(value: &Option<StatScalar>, dst: &mut [u8]) -> Result<(), CoveError> {
    debug_assert_eq!(dst.len(), STAT_SCALAR_ENCODED_LEN);
    for byte in dst.iter_mut() {
        *byte = 0;
    }
    let Some(scalar) = value else {
        // None: kind=0, flags=0, length=0, payload zero-filled.
        return Ok(());
    };
    validate_stat_scalar_shape(scalar)?;
    let len = scalar.bytes.len();
    dst[0] = scalar.kind as u8;
    dst[1] = if scalar.truncated {
        STAT_SCALAR_FLAG_TRUNCATED
    } else {
        0
    };
    dst[2..4].copy_from_slice(&(len as u16).to_le_bytes());
    dst[4..4 + len].copy_from_slice(&scalar.bytes[..len]);
    Ok(())
}

fn decode_i32_exact(bytes: &[u8]) -> Option<i32> {
    if bytes.len() != 4 {
        return None;
    }
    let array: [u8; 4] = bytes.get(..4)?.try_into().ok()?;
    Some(i32::from_le_bytes(array))
}

fn decode_i64_exact(bytes: &[u8]) -> Option<i64> {
    if bytes.len() != 8 {
        return None;
    }
    let array: [u8; 8] = bytes.get(..8)?.try_into().ok()?;
    Some(i64::from_le_bytes(array))
}

fn decode_u64_exact(bytes: &[u8]) -> Option<u64> {
    if bytes.len() != 8 {
        return None;
    }
    let array: [u8; 8] = bytes.get(..8)?.try_into().ok()?;
    Some(u64::from_le_bytes(array))
}

fn decode_i128_exact(bytes: &[u8]) -> Option<i128> {
    if bytes.len() != 16 {
        return None;
    }
    let array: [u8; 16] = bytes.get(..16)?.try_into().ok()?;
    Some(i128::from_le_bytes(array))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stat_scalar(kind: StatKind, value: &[u8]) -> [u8; STAT_SCALAR_ENCODED_LEN] {
        stat_scalar_with_flags(kind, 0, value)
    }

    fn stat_scalar_with_flags(
        kind: StatKind,
        flags: u8,
        value: &[u8],
    ) -> [u8; STAT_SCALAR_ENCODED_LEN] {
        let mut out = [0u8; STAT_SCALAR_ENCODED_LEN];
        out[0] = kind as u8;
        out[1] = flags;
        out[2..4].copy_from_slice(&(value.len() as u16).to_le_bytes());
        out[4..4 + value.len()].copy_from_slice(value);
        out
    }

    fn zone_stats_entry_bytes(
        row_count: u32,
        null_count: u32,
        non_null_count: u32,
        flags: ZoneStatFlags,
        min: Option<[u8; STAT_SCALAR_ENCODED_LEN]>,
        max: Option<[u8; STAT_SCALAR_ENCODED_LEN]>,
    ) -> [u8; ZONE_STATS_ENTRY_LEN] {
        let mut out = [0u8; ZONE_STATS_ENTRY_LEN];
        out[0..4].copy_from_slice(&1u32.to_le_bytes());
        out[4..8].copy_from_slice(&2u32.to_le_bytes());
        out[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        out[12..16].copy_from_slice(&3u32.to_le_bytes());
        out[16..20].copy_from_slice(&row_count.to_le_bytes());
        out[20..24].copy_from_slice(&null_count.to_le_bytes());
        out[24..28].copy_from_slice(&non_null_count.to_le_bytes());
        out[28..32].copy_from_slice(&5u32.to_le_bytes());
        out[32..36].copy_from_slice(&2u32.to_le_bytes());
        out[36..40].copy_from_slice(&flags.bits().to_le_bytes());
        if let Some(min) = min {
            out[40..60].copy_from_slice(&min);
        }
        if let Some(max) = max {
            out[60..80].copy_from_slice(&max);
        }
        out[80..84].copy_from_slice(&1u32.to_le_bytes());
        out[84..88].copy_from_slice(&2u32.to_le_bytes());
        out[88..92].copy_from_slice(&7u32.to_le_bytes());
        out[92..96].copy_from_slice(&8u32.to_le_bytes());
        out
    }

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
                kind: StatKind::Int64,
                bytes: b.to_vec(),
                truncated: false,
            }),
            max: max.map(|b| StatScalar {
                kind: StatKind::Int64,
                bytes: b.to_vec(),
                truncated: false,
            }),
            flags,
        }
    }

    #[test]
    fn spec_28_null_only_zone_must_omit_min_max() {
        let bytes = 1i64.to_le_bytes();
        let s = stats(Some(&bytes), Some(&bytes), 10, 10, ZoneStatFlags::empty());
        assert_eq!(s.validate(), Err(CoveError::BadStats));
    }

    #[test]
    fn spec_28_null_count_above_row_count_rejected() {
        let s = stats(None, None, 5, 10, ZoneStatFlags::empty());
        assert_eq!(s.validate(), Err(CoveError::BadStats));
    }

    #[test]
    fn spec_28_constant_flag_requires_min_eq_max() {
        let min = 1i64.to_le_bytes();
        let max = 2i64.to_le_bytes();
        let s = stats(Some(&min), Some(&max), 10, 0, ZoneStatFlags::CONSTANT);
        assert_eq!(s.validate(), Err(CoveError::BadStats));
    }

    #[test]
    fn spec_28_sorted_asc_and_desc_conflict() {
        let min = 1i64.to_le_bytes();
        let max = 2i64.to_le_bytes();
        let s = stats(
            Some(&min),
            Some(&max),
            10,
            0,
            ZoneStatFlags::SORTED_ASC | ZoneStatFlags::SORTED_DESC,
        );
        assert_eq!(s.validate(), Err(CoveError::BadStats));
    }

    #[test]
    fn parses_valid_zone_stats_section() {
        let min = stat_scalar(StatKind::Int64, &1i64.to_le_bytes());
        let max = stat_scalar(StatKind::Int64, &9i64.to_le_bytes());
        let bytes =
            zone_stats_entry_bytes(10, 2, 8, ZoneStatFlags::HAS_MIN_MAX, Some(min), Some(max));
        let section = ZoneStatsSection::parse(&bytes).unwrap();
        assert_eq!(section.entries.len(), 1);
        assert_eq!(section.entries[0].stats.row_count, 10);
        assert_eq!(section.entries[0].stats.null_count, 2);
        assert_eq!(section.entries[0].stats.scope, ZoneScope::Segment);
        assert_eq!(
            section.entries[0].stats.min.as_ref().unwrap().kind,
            StatKind::Int64
        );
    }

    #[test]
    fn rejects_zone_stats_when_counts_do_not_balance() {
        let bytes = zone_stats_entry_bytes(10, 3, 8, ZoneStatFlags::empty(), None, None);
        assert_eq!(ZoneStatsSection::parse(&bytes), Err(CoveError::BadStats));
    }

    #[test]
    fn rejects_zone_stats_when_scalar_length_is_not_exact() {
        let min = stat_scalar(StatKind::Int64, &1i64.to_le_bytes()[..7]);
        let max = stat_scalar(StatKind::Int64, &9i64.to_le_bytes());
        let bytes =
            zone_stats_entry_bytes(10, 0, 10, ZoneStatFlags::HAS_MIN_MAX, Some(min), Some(max));
        assert_eq!(ZoneStatsSection::parse(&bytes), Err(CoveError::BadStats));
    }

    #[test]
    fn rejects_zone_stats_when_scalar_flag_is_unknown() {
        let min = stat_scalar_with_flags(StatKind::Int64, 0b10, &1i64.to_le_bytes());
        let max = stat_scalar(StatKind::Int64, &9i64.to_le_bytes());
        let bytes =
            zone_stats_entry_bytes(10, 0, 10, ZoneStatFlags::HAS_MIN_MAX, Some(min), Some(max));
        assert_eq!(ZoneStatsSection::parse(&bytes), Err(CoveError::BadStats));
    }

    #[test]
    fn rejects_zone_stats_when_truncated_scalar_lacks_zone_flag() {
        let min = stat_scalar_with_flags(
            StatKind::Int64,
            STAT_SCALAR_FLAG_TRUNCATED,
            &1i64.to_le_bytes(),
        );
        let max = stat_scalar(StatKind::Int64, &9i64.to_le_bytes());
        let bytes =
            zone_stats_entry_bytes(10, 0, 10, ZoneStatFlags::HAS_MIN_MAX, Some(min), Some(max));
        assert_eq!(ZoneStatsSection::parse(&bytes), Err(CoveError::BadStats));
    }

    #[test]
    fn rejects_zone_stats_when_zone_truncation_flag_lacks_scalar_flag() {
        let min = stat_scalar(StatKind::Int64, &1i64.to_le_bytes());
        let max = stat_scalar(StatKind::Int64, &9i64.to_le_bytes());
        let bytes = zone_stats_entry_bytes(
            10,
            0,
            10,
            ZoneStatFlags::HAS_MIN_MAX | ZoneStatFlags::MINMAX_TRUNCATED,
            Some(min),
            Some(max),
        );
        assert_eq!(ZoneStatsSection::parse(&bytes), Err(CoveError::BadStats));
    }

    #[test]
    fn accepts_zone_stats_when_truncation_flags_match() {
        let min = stat_scalar_with_flags(
            StatKind::Int64,
            STAT_SCALAR_FLAG_TRUNCATED,
            &1i64.to_le_bytes(),
        );
        let max = stat_scalar(StatKind::Int64, &9i64.to_le_bytes());
        let bytes = zone_stats_entry_bytes(
            10,
            0,
            10,
            ZoneStatFlags::HAS_MIN_MAX | ZoneStatFlags::MINMAX_TRUNCATED,
            Some(min),
            Some(max),
        );
        let parsed = ZoneStatsSection::parse(&bytes).unwrap();
        assert!(parsed.entries[0].stats.min.as_ref().unwrap().truncated);
    }

    #[test]
    fn rejects_zone_stats_when_minmax_flag_and_scalars_disagree() {
        let max = stat_scalar(StatKind::Int64, &9i64.to_le_bytes());
        let bytes = zone_stats_entry_bytes(10, 0, 10, ZoneStatFlags::HAS_MIN_MAX, None, Some(max));
        assert_eq!(ZoneStatsSection::parse(&bytes), Err(CoveError::BadStats));
    }

    #[test]
    fn rejects_zone_stats_when_minmax_kinds_disagree() {
        let stats = ZoneStats {
            scope: ZoneScope::Segment,
            row_count: 4,
            null_count: 0,
            min: Some(StatScalar {
                kind: StatKind::Int64,
                bytes: 1i64.to_le_bytes().to_vec(),
                truncated: false,
            }),
            max: Some(StatScalar {
                kind: StatKind::UInt64,
                bytes: 2u64.to_le_bytes().to_vec(),
                truncated: false,
            }),
            flags: ZoneStatFlags::HAS_MIN_MAX,
        };

        assert_eq!(stats.validate(), Err(CoveError::BadStats));
    }

    #[test]
    fn rejects_zone_stats_when_float_minmax_contains_nan() {
        let stats = ZoneStats {
            scope: ZoneScope::Segment,
            row_count: 4,
            null_count: 0,
            min: Some(StatScalar {
                kind: StatKind::Float64Bits,
                bytes: f64::NAN.to_bits().to_le_bytes().to_vec(),
                truncated: false,
            }),
            max: Some(StatScalar {
                kind: StatKind::Float64Bits,
                bytes: 2.0f64.to_bits().to_le_bytes().to_vec(),
                truncated: false,
            }),
            flags: ZoneStatFlags::HAS_MIN_MAX | ZoneStatFlags::HAS_NAN,
        };

        assert_eq!(stats.validate(), Err(CoveError::BadStats));
    }

    #[test]
    fn stat_scalars_decode_numeric_values() {
        let int64 = StatScalar {
            kind: StatKind::Int64,
            bytes: (-7i64).to_le_bytes().to_vec(),
            truncated: false,
        };
        let decimal = StatScalar {
            kind: StatKind::Decimal128,
            bytes: 42i128.to_le_bytes().to_vec(),
            truncated: false,
        };
        let date_days = StatScalar {
            kind: StatKind::DateDays,
            bytes: 12i32.to_le_bytes().to_vec(),
            truncated: false,
        };

        assert_eq!(int64.numeric_value(), Some(NumericStatValue::Int64(-7)));
        assert_eq!(
            decimal.numeric_value(),
            Some(NumericStatValue::Decimal128(42))
        );
        assert_eq!(
            date_days.numeric_value(),
            Some(NumericStatValue::DateDays(12))
        );
        let short = StatScalar {
            kind: StatKind::Int64,
            bytes: vec![1, 2, 3, 4, 5, 6, 7],
            truncated: false,
        };
        assert_eq!(short.numeric_value(), None);
    }

    #[test]
    fn zone_stats_entry_serialize_round_trip_with_min_max() {
        let entry = ZoneStatsEntry {
            table_id: 1,
            segment_id: 2,
            morsel_id: 3,
            column_id: 4,
            non_null_count: 9,
            distinct_count: 5,
            run_count: 2,
            stats: ZoneStats {
                scope: ZoneScope::Morsel,
                row_count: 10,
                null_count: 1,
                min: Some(StatScalar {
                    kind: StatKind::Int64,
                    bytes: 1i64.to_le_bytes().to_vec(),
                    truncated: false,
                }),
                max: Some(StatScalar {
                    kind: StatKind::Int64,
                    bytes: 100i64.to_le_bytes().to_vec(),
                    truncated: false,
                }),
                flags: ZoneStatFlags::HAS_MIN_MAX,
            },
            min_domain_rank: 0,
            max_domain_rank: 7,
            exact_set_ref: u32::MAX,
            bloom_ref: u32::MAX,
        };
        entry.validate().unwrap();
        let bytes = entry.serialize().unwrap();
        let parsed = ZoneStatsEntry::parse(&bytes).unwrap();
        assert_eq!(parsed, entry);
    }

    #[test]
    fn zone_stats_entry_serialize_round_trip_all_null_segment() {
        let entry = ZoneStatsEntry {
            table_id: 11,
            segment_id: 12,
            morsel_id: u32::MAX,
            column_id: 0,
            non_null_count: 0,
            distinct_count: 0,
            run_count: 0,
            stats: ZoneStats {
                scope: ZoneScope::Segment,
                row_count: 4,
                null_count: 4,
                min: None,
                max: None,
                flags: ZoneStatFlags::empty(),
            },
            min_domain_rank: 0,
            max_domain_rank: 0,
            exact_set_ref: 0,
            bloom_ref: 0,
        };
        entry.validate().unwrap();
        let bytes = entry.serialize().unwrap();
        let parsed = ZoneStatsEntry::parse(&bytes).unwrap();
        assert_eq!(parsed, entry);
    }

    #[test]
    fn zone_stats_section_round_trip_empty_and_multi() {
        let empty = ZoneStatsSection::default();
        assert_eq!(empty.serialize().unwrap(), Vec::<u8>::new());
        assert_eq!(
            ZoneStatsSection::parse(&empty.serialize().unwrap()).unwrap(),
            empty
        );

        let entry_a = ZoneStatsEntry {
            table_id: 1,
            segment_id: 0,
            morsel_id: 0,
            column_id: 0,
            non_null_count: 1,
            distinct_count: 1,
            run_count: 1,
            stats: ZoneStats {
                scope: ZoneScope::Morsel,
                row_count: 1,
                null_count: 0,
                min: Some(StatScalar {
                    kind: StatKind::UInt64,
                    bytes: 5u64.to_le_bytes().to_vec(),
                    truncated: false,
                }),
                max: Some(StatScalar {
                    kind: StatKind::UInt64,
                    bytes: 5u64.to_le_bytes().to_vec(),
                    truncated: false,
                }),
                flags: ZoneStatFlags::HAS_MIN_MAX | ZoneStatFlags::CONSTANT,
            },
            min_domain_rank: 0,
            max_domain_rank: 0,
            exact_set_ref: u32::MAX,
            bloom_ref: u32::MAX,
        };
        let mut entry_b = entry_a.clone();
        entry_b.morsel_id = 1;
        entry_b.stats.flags = ZoneStatFlags::HAS_MIN_MAX;
        entry_b.stats.max = Some(StatScalar {
            kind: StatKind::UInt64,
            bytes: 9u64.to_le_bytes().to_vec(),
            truncated: false,
        });

        let section = ZoneStatsSection {
            entries: vec![entry_a, entry_b],
        };
        let bytes = section.serialize().unwrap();
        assert_eq!(bytes.len(), 2 * ZONE_STATS_ENTRY_LEN);
        assert_eq!(ZoneStatsSection::parse(&bytes).unwrap(), section);
    }
}
