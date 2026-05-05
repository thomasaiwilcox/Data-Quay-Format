//! Spec §51 — Parquet conversion profile (skeleton).
//!
//! This module owns the conversion plan from Parquet to COVE. The real work
//! (decoding Parquet, building dictionaries, choosing encodings, writing
//! sections) lives in the future `cove-parquet` crate; here we model the
//! conversion *plan* so the rest of the library can reason about it.

use crate::CoveError;

/// One step in the Parquet → COVE conversion pipeline (Spec §51.2).
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
    EmitOptionalCovmCovx,
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
        ConversionStep::EmitOptionalCovmCovx,
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
/// either `WriteSections` or `EmitOptionalCovmCovx` (Spec §51.2).
pub fn validate_plan(plan: &[ConversionStep]) -> Result<(), CoveError> {
    if plan.first() != Some(&ConversionStep::DecodeSource) {
        return Err(CoveError::BadSection(
            "conversion plan must start with DecodeSource (Spec §51.2)".into(),
        ));
    }
    let last = plan.last();
    if !matches!(
        last,
        Some(ConversionStep::WriteSections) | Some(ConversionStep::EmitOptionalCovmCovx)
    ) {
        return Err(CoveError::BadSection(
            "conversion plan must end with WriteSections or EmitOptionalCovmCovx".into(),
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
        assert!(matches!(validate_plan(&bad), Err(CoveError::BadSection(_))));
    }
}
