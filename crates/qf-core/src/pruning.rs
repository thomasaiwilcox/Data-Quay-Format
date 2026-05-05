//! Quay Format (QF) v1.0 — Pruning explanation (Spec §37).
//!
//! `explain_pruning` returns a structured description of which validated
//! metadata proved each scan decision, so users can audit *why* a zone was
//! kept or skipped. This is the teaching surface that the spec highlights
//! as the bridge between "we have indexes" and "we trust them".

use crate::predicate::PredicateZoneOutcome;

/// Source of a pruning decision (Spec §37.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
        self.steps.push(PruningStep {
            evidence,
            outcome,
            spec_section,
            note: note.into(),
        });
        self.final_outcome = self.final_outcome.and(outcome);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use PredicateZoneOutcome::*;

    #[test]
    fn evidence_tracks_decisions() {
        let mut e = PruningExplanation::new();
        e.final_outcome = AllMatch;
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
}
