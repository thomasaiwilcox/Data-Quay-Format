use crate::CoveError;

/// Record kinds (Spec §59.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RecordKind {
    Delta,
    Snapshot,
    ReservedLegacyMaterializedDelta,
    Baseline,
    Tombstone,
}

impl RecordKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(RecordKind::Delta),
            1 => Some(RecordKind::Snapshot),
            2 => Some(RecordKind::ReservedLegacyMaterializedDelta),
            3 => Some(RecordKind::Baseline),
            4 => Some(RecordKind::Tombstone),
            _ => None,
        }
    }

    /// Reserved legacy materialized-delta records are not valid published rows.
    pub fn validate_published(self) -> Result<(), CoveError> {
        if matches!(self, RecordKind::ReservedLegacyMaterializedDelta) {
            Err(CoveError::BadSchema(
                "reserved legacy materialized delta is not valid in published files (Spec §59.1)"
                    .into(),
            ))
        } else {
            Ok(())
        }
    }
}

/// One row in the temporal segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TemporalRowKey {
    pub timestamp_us: i64,
    pub csn: u64,
    pub branch_key: u64,
    pub goid: [u8; 16],
    pub record_id: [u8; 16],
}

impl TemporalRowKey {
    /// Lexicographic compare per Spec §58.3.
    pub fn cmp_lex(&self, other: &Self) -> std::cmp::Ordering {
        (
            self.timestamp_us,
            self.csn,
            self.branch_key,
            self.goid,
            self.record_id,
        )
            .cmp(&(
                other.timestamp_us,
                other.csn,
                other.branch_key,
                other.goid,
                other.record_id,
            ))
    }
}

/// Validate that a slice of temporal rows is sorted in the §58.3 order.
pub fn validate_temporal_order(rows: &[TemporalRowKey]) -> Result<(), CoveError> {
    for w in rows.windows(2) {
        if w[0].cmp_lex(&w[1]) == std::cmp::Ordering::Greater {
            return Err(CoveError::BadSchema(
                "temporal rows out of order (Spec §58.3)".into(),
            ));
        }
    }
    Ok(())
}

/// Self-containment check (Spec §60). A COVE-O file is self-contained if every
/// `prev_ref` points to a row inside the same file. v1 forbids cross-file
/// `prev_ref` chains.
pub fn validate_self_contained(
    prev_refs: &[Option<u64>],
    local_record_ids: &[u64],
) -> Result<(), CoveError> {
    let local: std::collections::HashSet<u64> = local_record_ids.iter().copied().collect();
    for p in prev_refs.iter().flatten() {
        if !local.contains(p) {
            return Err(CoveError::NotSelfContained);
        }
    }
    Ok(())
}
