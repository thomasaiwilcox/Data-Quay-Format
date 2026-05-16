//! Cove Format (COVE) v1.0 — Pruning explanation (Spec §37).
//!
//! `explain_pruning` returns a structured description of which validated
//! metadata proved each scan decision, so users can audit *why* a zone was
//! kept or skipped. This is the teaching surface that the spec highlights
//! as the bridge between "we have indexes" and "we trust them".

use crate::{
    domain::ColumnDomain,
    index::{
        aggregate::AggregateSynopsis,
        bloom::BloomFilterIndex,
        composite::CompositeIndex,
        exact_set::{ExactSetIndex, ExactSetKeyKind},
        inverted::InvertedMorselIndex,
        lookup::LookupIndex,
    },
    predicate::PredicateZoneOutcome,
    zone_stats::{NumericStatValue, ZoneStatFlags, ZoneStatsEntry},
};

/// Source of a pruning decision (Spec §37.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PruningEvidence {
    /// No metadata available; outcome is `Unknown` and decode is required.
    NoMetadata,
    /// ZoneStats min/max ruled the zone in or out.
    ZoneStats,
    /// ColumnDomain rank map ruled the zone in or out.
    ColumnDomain,
    /// Exact-set membership index.
    ExactSet,
    /// Bloom filter said `false` (definitely no match).
    BloomFilter,
    /// Inverted morsel index narrowed candidate morsels.
    InvertedIndex,
    /// Composite zone index narrowed candidate morsels.
    CompositeIndex,
    /// Aggregate synopsis answered the query without scanning.
    AggregateSynopsis,
    /// Top-N summary answered an ordered-limit query.
    TopNSummary,
    /// Optional index was corrupt or stale; falling back to scan (Spec §73).
    FallbackToScan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PruningStep {
    pub evidence: PruningEvidence,
    pub outcome: PredicateZoneOutcome,
    /// Spec section that justifies the decision.
    pub spec_section: &'static str,
    pub note: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PruningExplanation {
    pub steps: Vec<PruningStep>,
    pub final_outcome: PredicateZoneOutcome,
}

impl PruningExplanation {
    pub fn new() -> Self {
        Self {
            steps: Vec::new(),
            final_outcome: PredicateZoneOutcome::Unknown,
        }
    }

    /// Combine `step` into the running outcome with `op` and record evidence.
    pub fn record(
        &mut self,
        evidence: PruningEvidence,
        outcome: PredicateZoneOutcome,
        spec_section: &'static str,
        note: impl Into<String>,
    ) {
        self.final_outcome = if self.steps.is_empty() {
            outcome
        } else {
            self.final_outcome.and(outcome)
        };
        self.steps.push(PruningStep {
            evidence,
            outcome,
            spec_section,
            note: note.into(),
        });
    }

    pub fn and(mut self, other: Self) -> Self {
        if self.steps.is_empty() {
            return other;
        }
        if other.steps.is_empty() {
            return self;
        }
        self.final_outcome = self.final_outcome.and(other.final_outcome);
        self.steps.extend(other.steps);
        self
    }

    pub fn or(mut self, other: Self) -> Self {
        if self.steps.is_empty() {
            return other;
        }
        if other.steps.is_empty() {
            return self;
        }
        self.final_outcome = self.final_outcome.or(other.final_outcome);
        self.steps.extend(other.steps);
        self
    }

    pub fn negate(mut self) -> Self {
        self.final_outcome = !self.final_outcome;
        self
    }
}

impl std::ops::Not for PruningExplanation {
    type Output = Self;

    fn not(self) -> Self::Output {
        self.negate()
    }
}

pub fn explain_is_null(zone_stats: Option<&ZoneStatsEntry>) -> PruningExplanation {
    match zone_stats {
        Some(zone) if zone.stats.null_count == 0 => single_step(
            PruningEvidence::ZoneStats,
            PredicateZoneOutcome::NoMatch,
            "§37.4",
            "null_count = 0 proves the zone cannot satisfy IS NULL",
        ),
        Some(zone) if zone.stats.null_count == zone.stats.row_count => single_step(
            PruningEvidence::ZoneStats,
            PredicateZoneOutcome::AllMatch,
            "§37.4",
            "null_count = row_count proves the entire zone satisfies IS NULL",
        ),
        Some(zone) => single_step(
            PruningEvidence::ZoneStats,
            if zone.stats.null_count > 0 {
                PredicateZoneOutcome::SomeMatch
            } else {
                PredicateZoneOutcome::Unknown
            },
            "§37.4",
            "zone mixes null and non-null rows, so row-level filtering is still required",
        ),
        None => single_step(
            PruningEvidence::NoMetadata,
            PredicateZoneOutcome::Unknown,
            "§37.4",
            "IS NULL proof requires zone statistics",
        ),
    }
}

pub fn explain_is_not_null(zone_stats: Option<&ZoneStatsEntry>) -> PruningExplanation {
    match zone_stats {
        Some(zone) if zone.stats.null_count == zone.stats.row_count => single_step(
            PruningEvidence::ZoneStats,
            PredicateZoneOutcome::NoMatch,
            "§37.4",
            "null_count = row_count proves the zone cannot satisfy IS NOT NULL",
        ),
        Some(zone) if zone.stats.null_count == 0 => single_step(
            PruningEvidence::ZoneStats,
            PredicateZoneOutcome::AllMatch,
            "§37.4",
            "null_count = 0 proves the entire zone satisfies IS NOT NULL",
        ),
        Some(_) => single_step(
            PruningEvidence::ZoneStats,
            PredicateZoneOutcome::SomeMatch,
            "§37.4",
            "zone mixes null and non-null rows, so row-level filtering is still required",
        ),
        None => single_step(
            PruningEvidence::NoMetadata,
            PredicateZoneOutcome::Unknown,
            "§37.4",
            "IS NOT NULL proof requires zone statistics",
        ),
    }
}

pub fn explain_file_code_equality(
    file_code: u32,
    zone_stats: Option<&ZoneStatsEntry>,
    domain: Option<&ColumnDomain>,
    exact_set: Option<&ExactSetIndex>,
) -> PruningExplanation {
    if let Some(zone) = zone_stats {
        if zone.stats.null_count == zone.stats.row_count {
            return single_step(
                PruningEvidence::ZoneStats,
                PredicateZoneOutcome::NoMatch,
                "§37.1",
                "zone is null-only, so a non-null FileCode equality cannot match",
            );
        }
    }

    let safe_domain = usable_domain(domain);
    if let Some(domain) = safe_domain {
        match domain.rank_of(file_code) {
            None => {
                return single_step(
                    PruningEvidence::ColumnDomain,
                    PredicateZoneOutcome::NoMatch,
                    "§37.1",
                    format!("FileCode {file_code} is absent from the safe ColumnDomain"),
                );
            }
            Some(rank) => {
                if let Some(zone) = zone_stats {
                    if zone.stats.flags.contains(ZoneStatFlags::HAS_DOMAIN_RANGE)
                        && (rank < zone.min_domain_rank || rank > zone.max_domain_rank)
                    {
                        return single_step(
                            PruningEvidence::ColumnDomain,
                            PredicateZoneOutcome::NoMatch,
                            "§37.1",
                            format!(
                                "FileCode {file_code} rank {rank} falls outside the zone domain interval {}..={}",
                                zone.min_domain_rank, zone.max_domain_rank
                            ),
                        );
                    }

                    if zone.stats.flags.contains(ZoneStatFlags::HAS_DOMAIN_RANGE)
                        && zone.stats.flags.contains(ZoneStatFlags::CONSTANT)
                        && zone.min_domain_rank == rank
                        && zone.max_domain_rank == rank
                        && zone.stats.null_count == 0
                    {
                        return single_step(
                            PruningEvidence::ColumnDomain,
                            PredicateZoneOutcome::AllMatch,
                            "§37.1",
                            format!(
                                "safe ColumnDomain plus constant zone stats prove every row equals FileCode {file_code}"
                            ),
                        );
                    }
                }
            }
        }
    }

    if let Some(contains) = exact_set_membership(exact_set, file_code) {
        return single_step(
            PruningEvidence::ExactSet,
            if contains {
                PredicateZoneOutcome::SomeMatch
            } else {
                PredicateZoneOutcome::NoMatch
            },
            "§37.1",
            if contains {
                format!("exact-set contains FileCode {file_code}, so surviving rows may match")
            } else {
                format!("exact-set excludes FileCode {file_code}, so the zone can be skipped")
            },
        );
    }

    if let (Some(zone), Some(domain)) = (zone_stats, safe_domain) {
        if zone.stats.flags.contains(ZoneStatFlags::HAS_DOMAIN_RANGE) {
            if let Some(rank) = domain.rank_of(file_code) {
                return single_step(
                    PruningEvidence::ColumnDomain,
                    PredicateZoneOutcome::SomeMatch,
                    "§37.1",
                    format!(
                        "FileCode {file_code} rank {rank} overlaps the zone domain interval {}..={}, so decode is still required",
                        zone.min_domain_rank, zone.max_domain_rank
                    ),
                );
            }
        }
    }

    if zone_stats.is_some() || domain.is_some() || exact_set.is_some() {
        return single_step(
            PruningEvidence::FallbackToScan,
            PredicateZoneOutcome::Unknown,
            "§37.1",
            "available metadata cannot safely prove the FileCode equality outcome",
        );
    }

    single_step(
        PruningEvidence::NoMetadata,
        PredicateZoneOutcome::Unknown,
        "§37.1",
        "no metadata available for FileCode equality pruning",
    )
}

pub fn explain_resolved_domain_rank_range(
    min_rank: u32,
    max_rank: u32,
    zone_stats: Option<&ZoneStatsEntry>,
    domain: Option<&ColumnDomain>,
) -> PruningExplanation {
    if min_rank > max_rank {
        return single_step(
            PruningEvidence::ColumnDomain,
            PredicateZoneOutcome::NoMatch,
            "§37.2",
            format!("resolved domain-rank interval {min_rank}..={max_rank} is empty"),
        );
    }

    let Some(zone) = zone_stats else {
        return single_step(
            PruningEvidence::NoMetadata,
            PredicateZoneOutcome::Unknown,
            "§37.2",
            "range pruning requires zone statistics for the target zone",
        );
    };

    if zone.stats.null_count == zone.stats.row_count {
        return single_step(
            PruningEvidence::ZoneStats,
            PredicateZoneOutcome::NoMatch,
            "§37.2",
            "zone is null-only, so a non-null FileCode range cannot match",
        );
    }

    let Some(_) = usable_domain(domain) else {
        return single_step(
            PruningEvidence::FallbackToScan,
            PredicateZoneOutcome::Unknown,
            "§37.2",
            "range pruning requires a safe ColumnDomain",
        );
    };

    if !zone.stats.flags.contains(ZoneStatFlags::HAS_DOMAIN_RANGE) {
        return single_step(
            PruningEvidence::FallbackToScan,
            PredicateZoneOutcome::Unknown,
            "§37.2",
            "zone statistics do not carry domain-rank bounds",
        );
    }

    if zone.max_domain_rank < min_rank || zone.min_domain_rank > max_rank {
        return single_step(
            PruningEvidence::ColumnDomain,
            PredicateZoneOutcome::NoMatch,
            "§37.2",
            format!(
                "zone domain interval {}..={} is disjoint from resolved predicate interval {min_rank}..={max_rank}",
                zone.min_domain_rank, zone.max_domain_rank
            ),
        );
    }

    if zone.min_domain_rank >= min_rank
        && zone.max_domain_rank <= max_rank
        && zone.stats.null_count == 0
    {
        return single_step(
            PruningEvidence::ColumnDomain,
            PredicateZoneOutcome::AllMatch,
            "§37.2",
            format!(
                "zone domain interval {}..={} is fully contained in resolved predicate interval {min_rank}..={max_rank}",
                zone.min_domain_rank, zone.max_domain_rank
            ),
        );
    }

    single_step(
        PruningEvidence::ColumnDomain,
        PredicateZoneOutcome::SomeMatch,
        "§37.2",
        format!(
            "zone domain interval {}..={} overlaps resolved predicate interval {min_rank}..={max_rank}",
            zone.min_domain_rank, zone.max_domain_rank
        ),
    )
}

pub fn explain_numcode_range(
    lower_bound: Option<NumericStatValue>,
    lower_inclusive: bool,
    upper_bound: Option<NumericStatValue>,
    upper_inclusive: bool,
    zone_stats: Option<&ZoneStatsEntry>,
) -> PruningExplanation {
    let Some(zone) = zone_stats else {
        return single_step(
            PruningEvidence::NoMetadata,
            PredicateZoneOutcome::Unknown,
            "§37.3",
            "numeric range pruning requires zone statistics",
        );
    };

    if zone.stats.null_count == zone.stats.row_count {
        return single_step(
            PruningEvidence::ZoneStats,
            PredicateZoneOutcome::NoMatch,
            "§37.3",
            "zone is null-only, so a non-null numeric predicate cannot match",
        );
    }

    if !zone.stats.flags.contains(ZoneStatFlags::HAS_MIN_MAX) {
        return single_step(
            PruningEvidence::FallbackToScan,
            PredicateZoneOutcome::Unknown,
            "§37.3",
            "numeric range pruning requires validated min/max zone stats",
        );
    }

    if zone.stats.flags.contains(ZoneStatFlags::HAS_NAN) {
        return single_step(
            PruningEvidence::FallbackToScan,
            PredicateZoneOutcome::Unknown,
            "§37.3",
            "numeric range pruning cannot safely exclude zones with NaN-bearing stats",
        );
    }

    if zone.stats.flags.contains(ZoneStatFlags::MINMAX_TRUNCATED) {
        return single_step(
            PruningEvidence::FallbackToScan,
            PredicateZoneOutcome::Unknown,
            "§37.3",
            "numeric range pruning cannot safely use truncated min/max bounds",
        );
    }

    let Some(min) = zone
        .stats
        .min
        .as_ref()
        .and_then(|value| value.numeric_value())
    else {
        return single_step(
            PruningEvidence::FallbackToScan,
            PredicateZoneOutcome::Unknown,
            "§37.3",
            "numeric range pruning requires decodable typed min stats",
        );
    };
    let Some(max) = zone
        .stats
        .max
        .as_ref()
        .and_then(|value| value.numeric_value())
    else {
        return single_step(
            PruningEvidence::FallbackToScan,
            PredicateZoneOutcome::Unknown,
            "§37.3",
            "numeric range pruning requires decodable typed max stats",
        );
    };

    if !numeric_bounds_share_kind(lower_bound.as_ref(), &min)
        || !numeric_bounds_share_kind(upper_bound.as_ref(), &min)
        || !numeric_kinds_match(&min, &max)
    {
        return single_step(
            PruningEvidence::FallbackToScan,
            PredicateZoneOutcome::Unknown,
            "§37.3",
            "numeric range pruning requires matching NumCode stat and predicate kinds",
        );
    }

    if predicate_disjoint_from_zone(
        lower_bound.as_ref(),
        lower_inclusive,
        upper_bound.as_ref(),
        upper_inclusive,
        &min,
        &max,
    ) {
        return single_step(
            PruningEvidence::ZoneStats,
            PredicateZoneOutcome::NoMatch,
            "§37.3",
            format!(
                "typed NumCode min/max {}..={} is disjoint from predicate {}",
                numeric_value_note(&min),
                numeric_value_note(&max),
                numeric_range_note(
                    lower_bound.as_ref(),
                    lower_inclusive,
                    upper_bound.as_ref(),
                    upper_inclusive
                )
            ),
        );
    }

    if zone_fully_within_predicate(
        lower_bound.as_ref(),
        lower_inclusive,
        upper_bound.as_ref(),
        upper_inclusive,
        &min,
        &max,
    ) && zone.stats.null_count == 0
    {
        return single_step(
            PruningEvidence::ZoneStats,
            PredicateZoneOutcome::AllMatch,
            "§37.3",
            format!(
                "typed NumCode min/max {}..={} is fully contained in predicate {}",
                numeric_value_note(&min),
                numeric_value_note(&max),
                numeric_range_note(
                    lower_bound.as_ref(),
                    lower_inclusive,
                    upper_bound.as_ref(),
                    upper_inclusive
                )
            ),
        );
    }

    single_step(
        PruningEvidence::ZoneStats,
        PredicateZoneOutcome::SomeMatch,
        "§37.3",
        format!(
            "typed NumCode min/max {}..={} overlaps predicate {}",
            numeric_value_note(&min),
            numeric_value_note(&max),
            numeric_range_note(
                lower_bound.as_ref(),
                lower_inclusive,
                upper_bound.as_ref(),
                upper_inclusive
            )
        ),
    )
}

fn usable_domain(domain: Option<&ColumnDomain>) -> Option<&ColumnDomain> {
    domain.filter(|domain| domain.is_safe())
}

fn numeric_kinds_match(left: &NumericStatValue, right: &NumericStatValue) -> bool {
    std::mem::discriminant(left) == std::mem::discriminant(right)
}

fn numeric_bounds_share_kind(bound: Option<&NumericStatValue>, base: &NumericStatValue) -> bool {
    match bound {
        Some(bound) => numeric_kinds_match(bound, base),
        None => true,
    }
}

fn predicate_disjoint_from_zone(
    lower_bound: Option<&NumericStatValue>,
    lower_inclusive: bool,
    upper_bound: Option<&NumericStatValue>,
    upper_inclusive: bool,
    zone_min: &NumericStatValue,
    zone_max: &NumericStatValue,
) -> bool {
    if upper_bound
        .is_some_and(|upper_bound| is_strictly_below(upper_bound, zone_min, upper_inclusive))
    {
        return true;
    }
    match lower_bound {
        Some(lower_bound) => is_strictly_above(lower_bound, zone_max, lower_inclusive),
        None => false,
    }
}

fn zone_fully_within_predicate(
    lower_bound: Option<&NumericStatValue>,
    lower_inclusive: bool,
    upper_bound: Option<&NumericStatValue>,
    upper_inclusive: bool,
    zone_min: &NumericStatValue,
    zone_max: &NumericStatValue,
) -> bool {
    let lower_ok = match lower_bound {
        Some(lower_bound) => range_satisfies_lower(zone_min, lower_bound, lower_inclusive),
        None => true,
    };
    let upper_ok = match upper_bound {
        Some(upper_bound) => range_satisfies_upper(zone_max, upper_bound, upper_inclusive),
        None => true,
    };
    lower_ok && upper_ok
}

fn numeric_range_note(
    lower_bound: Option<&NumericStatValue>,
    lower_inclusive: bool,
    upper_bound: Option<&NumericStatValue>,
    upper_inclusive: bool,
) -> String {
    let lower_bracket = if lower_inclusive { '[' } else { '(' };
    let upper_bracket = if upper_inclusive { ']' } else { ')' };
    let lower = match lower_bound {
        Some(lower_bound) => numeric_value_note(lower_bound),
        None => "-inf".to_string(),
    };
    let upper = match upper_bound {
        Some(upper_bound) => numeric_value_note(upper_bound),
        None => "+inf".to_string(),
    };
    format!("{lower_bracket}{lower}, {upper}{upper_bracket}")
}

fn numeric_value_note(value: &NumericStatValue) -> String {
    match value {
        NumericStatValue::Int64(value) => value.to_string(),
        NumericStatValue::UInt64(value) => value.to_string(),
        NumericStatValue::Float64(value) => value.to_string(),
        NumericStatValue::Decimal128(value) => value.to_string(),
        NumericStatValue::TimestampMicros(value) => value.to_string(),
        NumericStatValue::TimestampNanos(value) => value.to_string(),
        NumericStatValue::DateDays(value) => value.to_string(),
    }
}

fn range_satisfies_lower(
    zone_min: &NumericStatValue,
    predicate_lower: &NumericStatValue,
    inclusive: bool,
) -> bool {
    match compare_numeric(zone_min, predicate_lower) {
        Some(ordering) => ordering.is_gt() || (inclusive && ordering.is_eq()),
        None => false,
    }
}

fn range_satisfies_upper(
    zone_max: &NumericStatValue,
    predicate_upper: &NumericStatValue,
    inclusive: bool,
) -> bool {
    match compare_numeric(zone_max, predicate_upper) {
        Some(ordering) => ordering.is_lt() || (inclusive && ordering.is_eq()),
        None => false,
    }
}

fn is_strictly_below(
    predicate_upper: &NumericStatValue,
    zone_min: &NumericStatValue,
    upper_inclusive: bool,
) -> bool {
    match compare_numeric(predicate_upper, zone_min) {
        Some(ordering) => ordering.is_lt() || (!upper_inclusive && ordering.is_eq()),
        None => false,
    }
}

fn is_strictly_above(
    predicate_lower: &NumericStatValue,
    zone_max: &NumericStatValue,
    lower_inclusive: bool,
) -> bool {
    match compare_numeric(predicate_lower, zone_max) {
        Some(ordering) => ordering.is_gt() || (!lower_inclusive && ordering.is_eq()),
        None => false,
    }
}

fn compare_numeric(
    left: &NumericStatValue,
    right: &NumericStatValue,
) -> Option<std::cmp::Ordering> {
    match (left, right) {
        (NumericStatValue::Int64(left), NumericStatValue::Int64(right)) => Some(left.cmp(right)),
        (NumericStatValue::UInt64(left), NumericStatValue::UInt64(right)) => Some(left.cmp(right)),
        (NumericStatValue::Float64(left), NumericStatValue::Float64(right)) => {
            left.partial_cmp(right)
        }
        (NumericStatValue::Decimal128(left), NumericStatValue::Decimal128(right)) => {
            Some(left.cmp(right))
        }
        (NumericStatValue::TimestampMicros(left), NumericStatValue::TimestampMicros(right)) => {
            Some(left.cmp(right))
        }
        (NumericStatValue::TimestampNanos(left), NumericStatValue::TimestampNanos(right)) => {
            Some(left.cmp(right))
        }
        (NumericStatValue::DateDays(left), NumericStatValue::DateDays(right)) => {
            Some(left.cmp(right))
        }
        _ => None,
    }
}

/// Bloom membership pruning (Spec §31).
///
/// Returns `NoMatch` when the bloom filter proves absence, `SomeMatch` when
/// the filter cannot rule out membership, `Unknown` when no filter is
/// available, and `FallbackToScan`/`Unknown` when the filter exists but is
/// flagged as corrupt or stale (Spec §73 fail-open).
pub fn explain_bloom_membership(
    value: &[u8],
    bloom: Option<&BloomFilterIndex>,
    fail_open: bool,
) -> PruningExplanation {
    match (bloom, fail_open) {
        (None, _) => single_step(
            PruningEvidence::NoMetadata,
            PredicateZoneOutcome::Unknown,
            "§31",
            "no bloom filter is attached to the zone",
        ),
        (Some(_), true) => single_step(
            PruningEvidence::FallbackToScan,
            PredicateZoneOutcome::Unknown,
            "§73",
            "bloom filter is corrupt or stale; fall back to scan",
        ),
        (Some(bloom), false) => {
            if bloom.might_contain(value) {
                single_step(
                    PruningEvidence::BloomFilter,
                    PredicateZoneOutcome::SomeMatch,
                    "§31",
                    "bloom filter cannot rule out membership; decode is still required",
                )
            } else {
                single_step(
                    PruningEvidence::BloomFilter,
                    PredicateZoneOutcome::NoMatch,
                    "§31",
                    "bloom filter proves the value is absent from the zone",
                )
            }
        }
    }
}

/// Inverted morsel index lookup (Spec §32).
pub fn explain_inverted_morsel_lookup(
    key: u64,
    index: Option<&InvertedMorselIndex>,
    fail_open: bool,
) -> PruningExplanation {
    match (index, fail_open) {
        (None, _) => single_step(
            PruningEvidence::NoMetadata,
            PredicateZoneOutcome::Unknown,
            "§32",
            "no inverted morsel index is attached to the zone",
        ),
        (Some(_), true) => single_step(
            PruningEvidence::FallbackToScan,
            PredicateZoneOutcome::Unknown,
            "§73",
            "inverted morsel index is corrupt or stale; fall back to scan",
        ),
        (Some(idx), false) => {
            let present = idx
                .entries
                .binary_search_by_key(&key, |entry| entry.key)
                .is_ok();
            if present {
                single_step(
                    PruningEvidence::InvertedIndex,
                    PredicateZoneOutcome::SomeMatch,
                    "§32",
                    format!("inverted morsel index lists key {key}; surviving morsels may match"),
                )
            } else {
                single_step(
                    PruningEvidence::InvertedIndex,
                    PredicateZoneOutcome::NoMatch,
                    "§32",
                    format!("inverted morsel index has no entry for key {key}"),
                )
            }
        }
    }
}

/// Lookup index point access (Spec §33).
pub fn explain_lookup_index_point(
    key: u64,
    index: Option<&LookupIndex>,
    fail_open: bool,
) -> PruningExplanation {
    match (index, fail_open) {
        (None, _) => single_step(
            PruningEvidence::NoMetadata,
            PredicateZoneOutcome::Unknown,
            "§33",
            "no lookup index is attached to the zone",
        ),
        (Some(_), true) => single_step(
            PruningEvidence::FallbackToScan,
            PredicateZoneOutcome::Unknown,
            "§73",
            "lookup index is corrupt or stale; fall back to scan",
        ),
        (Some(idx), false) => match idx.rows_for(key) {
            Some(rows) if !rows.is_empty() => single_step(
                PruningEvidence::InvertedIndex,
                PredicateZoneOutcome::SomeMatch,
                "§33",
                format!(
                    "lookup index resolves key {key} to {} row reference(s)",
                    rows.len()
                ),
            ),
            _ => single_step(
                PruningEvidence::InvertedIndex,
                PredicateZoneOutcome::NoMatch,
                "§33",
                format!("lookup index has no row reference for key {key}"),
            ),
        },
    }
}

/// Aggregate synopsis pruning (Spec §34).
///
/// Set `proves_no_match = true` when the synopsis (e.g. COUNT, MinMax, or a
/// histogram bucket) lets the planner prove the predicate excludes the zone.
pub fn explain_aggregate_synopsis(
    index: Option<&AggregateSynopsis>,
    fail_open: bool,
    proves_no_match: bool,
) -> PruningExplanation {
    match (index, fail_open) {
        (None, _) => single_step(
            PruningEvidence::NoMetadata,
            PredicateZoneOutcome::Unknown,
            "§34",
            "no aggregate synopsis is attached to the zone",
        ),
        (Some(_), true) => single_step(
            PruningEvidence::FallbackToScan,
            PredicateZoneOutcome::Unknown,
            "§73",
            "aggregate synopsis is corrupt or stale; fall back to scan",
        ),
        (Some(_), false) => {
            if proves_no_match {
                single_step(
                    PruningEvidence::AggregateSynopsis,
                    PredicateZoneOutcome::NoMatch,
                    "§34",
                    "aggregate synopsis proves the predicate excludes the zone",
                )
            } else {
                single_step(
                    PruningEvidence::AggregateSynopsis,
                    PredicateZoneOutcome::SomeMatch,
                    "§34",
                    "aggregate synopsis cannot prove pruning; decode may still be required",
                )
            }
        }
    }
}

/// Composite zone index pruning (Spec §35).
///
/// `matches_bindings` reflects whether the planner found at least one
/// composite entry whose bindings overlap the predicate.
pub fn explain_composite_zone(
    index: Option<&CompositeIndex>,
    fail_open: bool,
    matches_bindings: bool,
) -> PruningExplanation {
    match (index, fail_open) {
        (None, _) => single_step(
            PruningEvidence::NoMetadata,
            PredicateZoneOutcome::Unknown,
            "§35",
            "no composite zone index is attached to the zone",
        ),
        (Some(_), true) => single_step(
            PruningEvidence::FallbackToScan,
            PredicateZoneOutcome::Unknown,
            "§73",
            "composite zone index is corrupt or stale; fall back to scan",
        ),
        (Some(_), false) => {
            if matches_bindings {
                single_step(
                    PruningEvidence::CompositeIndex,
                    PredicateZoneOutcome::SomeMatch,
                    "§35",
                    "composite zone index lists matching bindings; decode is still required",
                )
            } else {
                single_step(
                    PruningEvidence::CompositeIndex,
                    PredicateZoneOutcome::NoMatch,
                    "§35",
                    "composite zone index has no matching bindings for the predicate",
                )
            }
        }
    }
}

fn exact_set_membership(exact_set: Option<&ExactSetIndex>, file_code: u32) -> Option<bool> {
    let exact_set = exact_set?;
    if exact_set.header.key_kind != ExactSetKeyKind::FileCode {
        return None;
    }
    Some(exact_set.contains(file_code as u64))
}

fn single_step(
    evidence: PruningEvidence,
    outcome: PredicateZoneOutcome,
    spec_section: &'static str,
    note: impl Into<String>,
) -> PruningExplanation {
    let mut explanation = PruningExplanation::new();
    explanation.record(evidence, outcome, spec_section, note);
    explanation
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::ColumnDomainHeaderV1,
        index::exact_set::{ExactSetGranularity, ExactSetIndexHeaderV1, ExactSetRepresentation},
        zone_stats::{StatKind, StatScalar, ZoneScope, ZoneStats},
    };
    use PredicateZoneOutcome::*;

    fn zone_entry(
        row_count: u64,
        null_count: u64,
        flags: ZoneStatFlags,
        min_domain_rank: u32,
        max_domain_rank: u32,
    ) -> ZoneStatsEntry {
        ZoneStatsEntry {
            table_id: 1,
            segment_id: 0,
            morsel_id: u32::MAX,
            column_id: 7,
            non_null_count: (row_count - null_count) as u32,
            distinct_count: 0,
            run_count: 0,
            stats: ZoneStats {
                scope: ZoneScope::Segment,
                row_count,
                null_count,
                min: None,
                max: None,
                flags,
            },
            min_domain_rank,
            max_domain_rank,
            exact_set_ref: 0,
            bloom_ref: 0,
        }
    }

    fn safe_domain() -> ColumnDomain {
        ColumnDomain::from_sorted_present_codes(&[1, 3, 4, 7], 8, 1, 7, 0, 0, 0).unwrap()
    }

    fn unsafe_domain() -> ColumnDomain {
        ColumnDomain {
            header: ColumnDomainHeaderV1 {
                table_or_object_id: 1,
                column_or_property_id: 7,
                logical_type: 0,
                collation_id: 0,
                domain_count: 2,
                sorted_file_codes_offset: 40,
                file_code_to_rank_offset: 48,
                flags: 0,
                checksum: 0,
            },
            sorted_file_codes: vec![1, 3],
            file_code_to_rank: vec![u32::MAX, 1, u32::MAX, 0],
        }
    }

    fn exact_set(keys: &[u64]) -> ExactSetIndex {
        ExactSetIndex {
            header: ExactSetIndexHeaderV1 {
                table_id: 1,
                column_id: 7,
                granularity: ExactSetGranularity::Segment,
                key_kind: ExactSetKeyKind::FileCode,
                representation: ExactSetRepresentation::SortedList,
                flags: 0,
                entry_count: keys.len() as u32,
                data_offset: 0,
                data_length: 0,
                checksum: 0,
            },
            keys: keys.to_vec(),
            data: Vec::new(),
        }
    }

    fn numeric_zone_entry(
        row_count: u64,
        null_count: u64,
        flags: ZoneStatFlags,
        min: StatScalar,
        max: StatScalar,
    ) -> ZoneStatsEntry {
        ZoneStatsEntry {
            table_id: 1,
            segment_id: 0,
            morsel_id: u32::MAX,
            column_id: 7,
            non_null_count: (row_count - null_count) as u32,
            distinct_count: 0,
            run_count: 0,
            stats: ZoneStats {
                scope: ZoneScope::Segment,
                row_count,
                null_count,
                min: Some(min),
                max: Some(max),
                flags,
            },
            min_domain_rank: 0,
            max_domain_rank: 0,
            exact_set_ref: 0,
            bloom_ref: 0,
        }
    }

    fn int64_stat(value: i64) -> StatScalar {
        StatScalar {
            kind: StatKind::Int64,
            bytes: value.to_le_bytes().to_vec(),
            truncated: false,
        }
    }

    fn float64_stat(value: f64) -> StatScalar {
        StatScalar {
            kind: StatKind::Float64Bits,
            bytes: value.to_bits().to_le_bytes().to_vec(),
            truncated: false,
        }
    }

    #[test]
    fn evidence_tracks_decisions() {
        let mut e = PruningExplanation::new();
        e.record(
            PruningEvidence::ZoneStats,
            AllMatch,
            "§28",
            "min/max in range",
        );
        e.record(PruningEvidence::ExactSet, NoMatch, "§30", "value absent");
        assert_eq!(e.final_outcome, NoMatch);
        assert_eq!(e.steps.len(), 2);
    }

    #[test]
    fn no_metadata_starts_unknown() {
        let e = PruningExplanation::new();
        assert_eq!(e.final_outcome, Unknown);
    }

    #[test]
    fn null_predicates_use_zone_stats_counts() {
        let mixed_zone = zone_entry(10, 3, ZoneStatFlags::empty(), 0, 0);
        let null_only_zone = zone_entry(10, 10, ZoneStatFlags::empty(), 0, 0);

        assert_eq!(explain_is_null(Some(&mixed_zone)).final_outcome, SomeMatch);
        assert_eq!(
            explain_is_not_null(Some(&mixed_zone)).final_outcome,
            SomeMatch
        );
        assert_eq!(
            explain_is_null(Some(&null_only_zone)).final_outcome,
            AllMatch
        );
        assert_eq!(
            explain_is_not_null(Some(&null_only_zone)).final_outcome,
            NoMatch
        );
    }

    #[test]
    fn file_code_equality_uses_exact_set_and_constant_domain_range() {
        let exact_only = exact_set(&[1, 4, 7]);
        let exact_explanation = explain_file_code_equality(3, None, None, Some(&exact_only));
        assert_eq!(exact_explanation.final_outcome, NoMatch);
        assert_eq!(
            exact_explanation.steps[0].evidence,
            PruningEvidence::ExactSet
        );

        let constant_zone = zone_entry(
            5,
            0,
            ZoneStatFlags::HAS_DOMAIN_RANGE | ZoneStatFlags::CONSTANT,
            1,
            1,
        );
        let domain = safe_domain();
        let constant_explanation =
            explain_file_code_equality(3, Some(&constant_zone), Some(&domain), None);
        assert_eq!(constant_explanation.final_outcome, AllMatch);
        assert_eq!(
            constant_explanation.steps[0].evidence,
            PruningEvidence::ColumnDomain
        );
    }

    #[test]
    fn domain_rank_range_falls_back_without_safe_domain() {
        let zone = zone_entry(8, 0, ZoneStatFlags::HAS_DOMAIN_RANGE, 2, 4);
        let explanation =
            explain_resolved_domain_rank_range(1, 3, Some(&zone), Some(&unsafe_domain()));
        assert_eq!(explanation.final_outcome, Unknown);
        assert_eq!(
            explanation.steps[0].evidence,
            PruningEvidence::FallbackToScan
        );
    }

    #[test]
    fn explanation_combinators_merge_outcomes() {
        let left = single_step(PruningEvidence::ZoneStats, AllMatch, "§37.4", "left");
        let right = single_step(PruningEvidence::ExactSet, NoMatch, "§37.1", "right");

        assert_eq!(left.clone().and(right.clone()).final_outcome, NoMatch);
        assert_eq!(left.clone().or(right.clone()).final_outcome, AllMatch);
        assert_eq!((!right).final_outcome, AllMatch);
    }

    #[test]
    fn numeric_range_uses_typed_numcode_bounds() {
        let zone = numeric_zone_entry(
            8,
            0,
            ZoneStatFlags::HAS_MIN_MAX,
            int64_stat(22),
            int64_stat(51),
        );

        let all_match = explain_numcode_range(
            Some(NumericStatValue::Int64(18)),
            true,
            Some(NumericStatValue::Int64(65)),
            true,
            Some(&zone),
        );
        assert_eq!(all_match.final_outcome, AllMatch);

        let no_match = explain_numcode_range(
            Some(NumericStatValue::Int64(70)),
            true,
            Some(NumericStatValue::Int64(90)),
            true,
            Some(&zone),
        );
        assert_eq!(no_match.final_outcome, NoMatch);

        let overlap = explain_numcode_range(
            Some(NumericStatValue::Int64(40)),
            true,
            Some(NumericStatValue::Int64(90)),
            true,
            Some(&zone),
        );
        assert_eq!(overlap.final_outcome, SomeMatch);
    }

    #[test]
    fn numeric_range_falls_back_for_nan_or_truncated_bounds() {
        let nan_zone = numeric_zone_entry(
            8,
            0,
            ZoneStatFlags::HAS_MIN_MAX | ZoneStatFlags::HAS_NAN,
            float64_stat(1.0),
            float64_stat(2.0),
        );
        let truncated_zone = numeric_zone_entry(
            8,
            0,
            ZoneStatFlags::HAS_MIN_MAX | ZoneStatFlags::MINMAX_TRUNCATED,
            int64_stat(1),
            int64_stat(2),
        );

        let nan_explanation = explain_numcode_range(
            Some(NumericStatValue::Float64(0.0)),
            true,
            Some(NumericStatValue::Float64(3.0)),
            true,
            Some(&nan_zone),
        );
        let truncated_explanation = explain_numcode_range(
            Some(NumericStatValue::Int64(0)),
            true,
            Some(NumericStatValue::Int64(3)),
            true,
            Some(&truncated_zone),
        );

        assert_eq!(nan_explanation.final_outcome, Unknown);
        assert_eq!(
            nan_explanation.steps[0].evidence,
            PruningEvidence::FallbackToScan
        );
        assert_eq!(truncated_explanation.final_outcome, Unknown);
        assert_eq!(
            truncated_explanation.steps[0].evidence,
            PruningEvidence::FallbackToScan
        );
    }

    fn bloom_with(values: &[&[u8]]) -> BloomFilterIndex {
        use crate::index::bloom::{
            BloomAlgorithm, BloomFilterIndex, BloomGranularity, BloomHashDomain,
            BloomIndexHeaderV1, BLOOM_INDEX_HEADER_LEN,
        };
        let mut bloom = BloomFilterIndex {
            header: BloomIndexHeaderV1 {
                table_id: 1,
                column_id: 7,
                granularity: BloomGranularity::Segment,
                hash_domain: BloomHashDomain::CanonicalValueHash,
                algorithm: BloomAlgorithm::SplitBlock,
                flags: 0,
                target_fpr_ppm: 10_000,
                filter_count: 1,
                data_offset: BLOOM_INDEX_HEADER_LEN as u64,
                data_length: 64,
                checksum: 0,
            },
            hash_count: 4,
            bits: vec![0u8; 64],
        };
        for value in values {
            bloom.insert(value);
        }
        bloom
    }

    fn inverted_with(keys: &[u64]) -> crate::index::inverted::InvertedMorselIndex {
        use crate::index::inverted::{
            InvertedEntry, InvertedKeyKind, InvertedMorselIndex, InvertedMorselIndexHeaderV1,
            INVERTED_MORSEL_INDEX_HEADER_LEN,
        };
        InvertedMorselIndex {
            header: InvertedMorselIndexHeaderV1 {
                table_id: 1,
                column_id: 7,
                key_kind: InvertedKeyKind::FileCode,
                flags: 0,
                representation: 0,
                reserved: 0,
                entry_count: keys.len() as u32,
                entries_offset: INVERTED_MORSEL_INDEX_HEADER_LEN as u64,
                bitmap_data_offset: INVERTED_MORSEL_INDEX_HEADER_LEN as u64,
                checksum: 0,
            },
            entries: keys
                .iter()
                .map(|key| InvertedEntry {
                    key: *key,
                    morsel_bitmap_offset: 0,
                    morsel_bitmap_length: 0,
                    row_bitmap_offset: 0,
                    row_bitmap_length: 0,
                })
                .collect(),
            bitmap_data: Vec::new(),
        }
    }

    fn lookup_with(keys: &[u64]) -> crate::index::lookup::LookupIndex {
        use crate::index::lookup::{
            LookupEntry, LookupIndex, LookupIndexHeaderV1, LookupIndexKind, LookupKeyKind,
            LookupUniqueness,
        };
        use crate::row_ref::RowRef;
        LookupIndex {
            header: LookupIndexHeaderV1 {
                table_id: 1,
                column_id: 7,
                key_kind: LookupKeyKind::FileCode,
                index_kind: LookupIndexKind::SparseSorted,
                uniqueness: LookupUniqueness::Unique,
                flags: 0,
                entry_count: keys.len() as u64,
                entries_offset: 0,
                entries_length: 0,
                rowref_offset: 0,
                rowref_length: 0,
                checksum: 0,
            },
            entries: keys
                .iter()
                .map(|key| LookupEntry {
                    key: *key,
                    rows: vec![RowRef {
                        table_id: 1,
                        segment_id: 0,
                        morsel_id: 0,
                        row_in_morsel: 0,
                    }],
                })
                .collect(),
        }
    }

    fn composite_index_stub() -> crate::index::composite::CompositeIndex {
        use crate::index::composite::{
            CompositeIndex, CompositeTransformKind, CompositeZoneIndexHeaderV1,
            COMPOSITE_ZONE_INDEX_HEADER_LEN,
        };
        CompositeIndex {
            header: CompositeZoneIndexHeaderV1 {
                table_id: 1,
                key_column_count: 1,
                transform_kind: CompositeTransformKind::Tuple,
                flags: 0,
                zone_count: 1,
                key_columns_offset: COMPOSITE_ZONE_INDEX_HEADER_LEN as u64,
                entries_offset: (COMPOSITE_ZONE_INDEX_HEADER_LEN + 4) as u64,
                entries_length: 0,
                checksum: 0,
            },
            key_columns: vec![7],
            entries: Vec::new(),
        }
    }

    fn aggregate_stub() -> AggregateSynopsis {
        AggregateSynopsis::default()
    }

    #[test]
    fn bloom_membership_proves_no_match_and_falls_back_when_corrupt() {
        let bloom = bloom_with(&[b"present".as_ref()]);
        let no_match = explain_bloom_membership(b"absent", Some(&bloom), false);
        assert_eq!(no_match.final_outcome, NoMatch);
        assert_eq!(no_match.steps[0].evidence, PruningEvidence::BloomFilter);

        let some_match = explain_bloom_membership(b"present", Some(&bloom), false);
        assert_eq!(some_match.final_outcome, SomeMatch);

        let fallback = explain_bloom_membership(b"absent", Some(&bloom), true);
        assert_eq!(fallback.final_outcome, Unknown);
        assert_eq!(fallback.steps[0].evidence, PruningEvidence::FallbackToScan);

        let none = explain_bloom_membership(b"absent", None, false);
        assert_eq!(none.final_outcome, Unknown);
        assert_eq!(none.steps[0].evidence, PruningEvidence::NoMetadata);
    }

    #[test]
    fn inverted_lookup_proves_no_match_and_falls_back_when_corrupt() {
        let idx = inverted_with(&[3, 5, 7]);
        assert_eq!(
            explain_inverted_morsel_lookup(4, Some(&idx), false).final_outcome,
            NoMatch
        );
        assert_eq!(
            explain_inverted_morsel_lookup(5, Some(&idx), false).final_outcome,
            SomeMatch
        );
        let corrupt = explain_inverted_morsel_lookup(5, Some(&idx), true);
        assert_eq!(corrupt.final_outcome, Unknown);
        assert_eq!(corrupt.steps[0].evidence, PruningEvidence::FallbackToScan);
    }

    #[test]
    fn lookup_point_proves_no_match_and_falls_back_when_corrupt() {
        let idx = lookup_with(&[10, 20, 30]);
        assert_eq!(
            explain_lookup_index_point(15, Some(&idx), false).final_outcome,
            NoMatch
        );
        assert_eq!(
            explain_lookup_index_point(20, Some(&idx), false).final_outcome,
            SomeMatch
        );
        assert_eq!(
            explain_lookup_index_point(20, Some(&idx), true).final_outcome,
            Unknown
        );
    }

    #[test]
    fn aggregate_synopsis_proves_no_match_and_falls_back_when_corrupt() {
        let idx = aggregate_stub();
        assert_eq!(
            explain_aggregate_synopsis(Some(&idx), false, true).final_outcome,
            NoMatch
        );
        assert_eq!(
            explain_aggregate_synopsis(Some(&idx), false, false).final_outcome,
            SomeMatch
        );
        let corrupt = explain_aggregate_synopsis(Some(&idx), true, true);
        assert_eq!(corrupt.final_outcome, Unknown);
        assert_eq!(corrupt.steps[0].evidence, PruningEvidence::FallbackToScan);
    }

    #[test]
    fn composite_zone_proves_no_match_and_falls_back_when_corrupt() {
        let idx = composite_index_stub();
        assert_eq!(
            explain_composite_zone(Some(&idx), false, false).final_outcome,
            NoMatch
        );
        assert_eq!(
            explain_composite_zone(Some(&idx), false, true).final_outcome,
            SomeMatch
        );
        let corrupt = explain_composite_zone(Some(&idx), true, true);
        assert_eq!(corrupt.final_outcome, Unknown);
        assert_eq!(corrupt.steps[0].evidence, PruningEvidence::FallbackToScan);
    }
}
