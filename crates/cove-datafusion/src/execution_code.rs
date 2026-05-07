//! DataFusion-agnostic COVE-E policy and literal-resolution helpers.

use cove_core::{
    mount::MountedCoveFile,
    profile::cove_e::{
        ExecutionCodeCanonicality, ExecutionCodeComparisonScope, ExecutionCodeKind,
        FileCodeMappingKind, NullCodePolicy, ReverseLookupPolicy,
    },
    CoveError,
};

use crate::{
    dataset_state::DatasetState,
    options::ExecutionCodePolicy,
    planner::{CovePredicate, FilterPlan},
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExecutionCodePlanStats {
    pub supported_files: usize,
    pub fallback_files: usize,
    pub literal_resolutions: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionCodeSupport {
    Disabled,
    Supported { descriptor_id: u32 },
    Fallback { reason: &'static str },
}

/// Validate execution-code policy for FileCode predicate planning.
///
/// INVARIANT: this function only authorizes execution-code optimization. A
/// fallback result must still execute through ordinary COVE FileCode remapping
/// and residual DataFusion filtering when the pushdown contract is inexact.
pub fn validate_policy_for_filters(
    state: &DatasetState,
    filters: &[FilterPlan],
) -> Result<ExecutionCodePlanStats, CoveError> {
    if !filters_need_execution_codes(filters) {
        return Ok(ExecutionCodePlanStats::default());
    }
    match state.execution_code_policy() {
        ExecutionCodePolicy::Disabled => Ok(ExecutionCodePlanStats {
            fallback_files: state.file_count(),
            ..ExecutionCodePlanStats::default()
        }),
        ExecutionCodePolicy::Opportunistic | ExecutionCodePolicy::RequireSupported => {
            let mut stats = ExecutionCodePlanStats::default();
            for file in state.files() {
                match support_for_mounted_file(file.mounted()) {
                    ExecutionCodeSupport::Supported { .. } => stats.supported_files += 1,
                    ExecutionCodeSupport::Disabled => stats.fallback_files += 1,
                    ExecutionCodeSupport::Fallback { .. } => {
                        stats.fallback_files += 1;
                        if state.execution_code_policy() == ExecutionCodePolicy::RequireSupported {
                            return Err(CoveError::BadEngineProfile);
                        }
                    }
                }
            }
            Ok(stats)
        }
    }
}

pub fn resolve_file_code_predicate_for_file(
    state: &DatasetState,
    file_ordinal: usize,
    canonical_values: &[Vec<u8>],
) -> Result<(Vec<u32>, ExecutionCodePlanStats), CoveError> {
    let file = state.file(file_ordinal)?;
    let support = match state.execution_code_policy() {
        ExecutionCodePolicy::Disabled => ExecutionCodeSupport::Disabled,
        ExecutionCodePolicy::Opportunistic | ExecutionCodePolicy::RequireSupported => {
            support_for_mounted_file(file.mounted())
        }
    };
    if matches!(support, ExecutionCodeSupport::Fallback { .. })
        && state.execution_code_policy() == ExecutionCodePolicy::RequireSupported
    {
        return Err(CoveError::BadEngineProfile);
    }

    let mut resolved = Vec::with_capacity(canonical_values.len());
    for canonical in canonical_values {
        if let Some(file_code) = state.file_code_for_canonical(file_ordinal, canonical)? {
            resolved.push(file_code);
        }
    }
    resolved.sort_unstable();
    resolved.dedup();
    Ok((
        resolved,
        ExecutionCodePlanStats {
            supported_files: usize::from(matches!(support, ExecutionCodeSupport::Supported { .. })),
            fallback_files: usize::from(!matches!(support, ExecutionCodeSupport::Supported { .. })),
            literal_resolutions: canonical_values.len(),
        },
    ))
}

pub fn support_for_mounted_file(mounted: &MountedCoveFile) -> ExecutionCodeSupport {
    let Some(descriptor) = mounted.engine_metadata.execution_descriptors.first() else {
        return ExecutionCodeSupport::Fallback {
            reason: "no COVE-E execution descriptor",
        };
    };
    if descriptor.code_kind != ExecutionCodeKind::DictionaryKey
        || descriptor.code_width_bits != 32
        || descriptor.byte_order != 0
        || !matches!(
            descriptor.null_code_policy,
            NullCodePolicy::NoNullCode | NullCodePolicy::NullBitmapOnly
        )
        || matches!(
            descriptor.canonicality,
            ExecutionCodeCanonicality::EnginePrivate
        )
    {
        return ExecutionCodeSupport::Fallback {
            reason: "unsupported execution-code descriptor",
        };
    }
    if matches!(
        descriptor.comparison_scope,
        ExecutionCodeComparisonScope::NotComparable
    ) {
        return ExecutionCodeSupport::Fallback {
            reason: "execution codes are not comparable",
        };
    }

    for policy in &mounted.engine_metadata.engine_mount_policies {
        match policy.filecode_mapping_kind {
            FileCodeMappingKind::DecodeToValue | FileCodeMappingKind::MapToArrowDictionary => {}
            FileCodeMappingKind::MapToExecutionCode
                if policy.reverse_lookup_policy == ReverseLookupPolicy::BuildFromDictionary => {}
            _ => {
                return ExecutionCodeSupport::Fallback {
                    reason: "unsupported engine mount policy",
                };
            }
        }
    }

    ExecutionCodeSupport::Supported {
        descriptor_id: descriptor.descriptor_id,
    }
}

fn filters_need_execution_codes(filters: &[FilterPlan]) -> bool {
    filters.iter().any(|filter| {
        matches!(
            filter.predicate,
            Some(CovePredicate::FileCodeIn {
                ref canonical_values,
                ..
            }) if !canonical_values.is_empty()
        )
    })
}
