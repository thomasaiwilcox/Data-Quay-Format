//! Spec §51 — Parquet conversion profile (skeleton).
//!
//! This module owns the conversion plan from Parquet to QF. The real work
//! (decoding Parquet, building dictionaries, choosing encodings, writing
//! sections) lives in the future `qf-parquet` crate; here we model the
//! conversion *plan* so the rest of the library can reason about it.

use crate::QfError;

/// One step in the Parquet → QF conversion pipeline (Spec §51.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversionStep {
    DecodeSource,
    PartitionSegments,
    BuildDictionaries,
    ChooseFileOrNumCode,
    RecomputeStats,
    BuildDomainsAndIndexes,
    EncodePages,
    WriteSections,
    EmitOptionalQfmQfx,
}

/// Canonical conversion plan in Spec §51.2 order.
pub fn canonical_plan() -> Vec<ConversionStep> {
    vec![
        ConversionStep::DecodeSource,
        ConversionStep::PartitionSegments,
        ConversionStep::BuildDictionaries,
        ConversionStep::ChooseFileOrNumCode,
        ConversionStep::RecomputeStats,
        ConversionStep::BuildDomainsAndIndexes,
        ConversionStep::EncodePages,
        ConversionStep::WriteSections,
        ConversionStep::EmitOptionalQfmQfx,
    ]
}

/// Spec §51.3: unsupported nested source shapes MUST be downgraded to JSON
/// or Binary and marked pushdown-limited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedNestedFallback {
    Json,
    Binary,
}

/// Validate that a conversion plan starts with `DecodeSource` and ends with
/// either `WriteSections` or `EmitOptionalQfmQfx` (Spec §51.2).
pub fn validate_plan(plan: &[ConversionStep]) -> Result<(), QfError> {
    if plan.first() != Some(&ConversionStep::DecodeSource) {
        return Err(QfError::BadSection(
            "conversion plan must start with DecodeSource (Spec §51.2)".into(),
        ));
    }
    let last = plan.last();
    if !matches!(
        last,
        Some(ConversionStep::WriteSections) | Some(ConversionStep::EmitOptionalQfmQfx)
    ) {
        return Err(QfError::BadSection(
            "conversion plan must end with WriteSections or EmitOptionalQfmQfx".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_plan_is_valid() {
        validate_plan(&canonical_plan()).unwrap();
    }

    #[test]
    fn rejects_plan_missing_decode_step() {
        let bad = vec![ConversionStep::WriteSections];
        assert!(matches!(validate_plan(&bad), Err(QfError::BadSection(_))));
    }
}
