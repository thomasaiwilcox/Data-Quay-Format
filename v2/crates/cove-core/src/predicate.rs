//! Cove Format (COVE) v2.0 — Predicate truth table (Spec §29).
//!
//! Predicate evaluation against a zone (segment, morsel, page) yields a
//! [`PredicateZoneOutcome`]. Three-valued logic propagation is governed by
//! Spec §29.2: WHERE clause UNKNOWN behaves like FALSE for selection but
//! `NOT UNKNOWN` is still UNKNOWN. The combinators `and`, `or`, `not` are
//! exhaustively tested below.

/// Outcome of evaluating a predicate against a zone using only its metadata
/// (ColumnDomain, ZoneStats, exact sets, blooms, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum PredicateZoneOutcome {
    /// Every row in the zone satisfies the predicate (Spec §29.1).
    AllMatch,
    /// No row in the zone can satisfy the predicate (Spec §29.1).
    NoMatch,
    /// At least one row may satisfy; decode required (Spec §29.1).
    SomeMatch,
    /// Metadata cannot prove anything; decode required.
    #[default]
    Unknown,
}

impl PredicateZoneOutcome {
    /// Whether the zone can be skipped without decoding.
    pub fn is_proven_no_match(self) -> bool {
        matches!(self, PredicateZoneOutcome::NoMatch)
    }

    /// Whether the zone is proven to fully match (no per-row check needed).
    pub fn is_proven_all_match(self) -> bool {
        matches!(self, PredicateZoneOutcome::AllMatch)
    }

    /// Conjunction: SQL WHERE three-valued AND truth table.
    pub fn and(self, other: Self) -> Self {
        use PredicateZoneOutcome::*;
        match (self, other) {
            (NoMatch, _) | (_, NoMatch) => NoMatch,
            (AllMatch, x) | (x, AllMatch) => x,
            (Unknown, Unknown) => Unknown,
            (Unknown, SomeMatch) | (SomeMatch, Unknown) => SomeMatch,
            (SomeMatch, SomeMatch) => SomeMatch,
        }
    }

    /// Disjunction: SQL WHERE three-valued OR truth table.
    pub fn or(self, other: Self) -> Self {
        use PredicateZoneOutcome::*;
        match (self, other) {
            (AllMatch, _) | (_, AllMatch) => AllMatch,
            (NoMatch, x) | (x, NoMatch) => x,
            (Unknown, Unknown) => Unknown,
            (Unknown, SomeMatch) | (SomeMatch, Unknown) => SomeMatch,
            (SomeMatch, SomeMatch) => SomeMatch,
        }
    }

    /// Negation: WHERE-context, where UNKNOWN remains UNKNOWN (Spec §29.2).
    pub fn negate(self) -> Self {
        use PredicateZoneOutcome::*;
        match self {
            AllMatch => NoMatch,
            NoMatch => AllMatch,
            SomeMatch => SomeMatch,
            Unknown => Unknown,
        }
    }
}

impl std::ops::Not for PredicateZoneOutcome {
    type Output = Self;

    fn not(self) -> Self::Output {
        self.negate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use PredicateZoneOutcome::*;

    #[test]
    fn spec_29_and_truth_table() {
        assert_eq!(AllMatch.and(AllMatch), AllMatch);
        assert_eq!(AllMatch.and(NoMatch), NoMatch);
        assert_eq!(NoMatch.and(SomeMatch), NoMatch);
        assert_eq!(SomeMatch.and(SomeMatch), SomeMatch);
        assert_eq!(Unknown.and(AllMatch), Unknown);
        assert_eq!(Unknown.and(NoMatch), NoMatch);
        assert_eq!(Unknown.and(Unknown), Unknown);
    }

    #[test]
    fn spec_29_or_truth_table() {
        assert_eq!(AllMatch.or(NoMatch), AllMatch);
        assert_eq!(NoMatch.or(NoMatch), NoMatch);
        assert_eq!(NoMatch.or(SomeMatch), SomeMatch);
        assert_eq!(Unknown.or(NoMatch), Unknown);
        assert_eq!(Unknown.or(AllMatch), AllMatch);
    }

    #[test]
    fn spec_29_not_truth_table() {
        assert_eq!(!AllMatch, NoMatch);
        assert_eq!(!NoMatch, AllMatch);
        assert_eq!(!SomeMatch, SomeMatch);
        assert_eq!(!Unknown, Unknown);
    }

    #[test]
    fn spec_29_skip_helpers() {
        assert!(NoMatch.is_proven_no_match());
        assert!(AllMatch.is_proven_all_match());
        assert!(!Unknown.is_proven_no_match());
        assert!(!SomeMatch.is_proven_all_match());
    }
}
