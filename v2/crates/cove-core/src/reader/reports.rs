use crate::{feature_scope::FeatureUseRequestV2, CoveError};

/// Options controlling the depth of validation.
#[derive(Debug, Clone)]
pub struct ValidationOptions {
    /// When true, validates dictionary semantics (entry bounds, redaction).
    pub semantic: bool,
    /// When true, verifies section digests if a DigestManifest is present.
    pub verify_digests: bool,
    /// When true, unknown optional extension registry entries are allowed.
    pub allow_unknown_optional_extensions: bool,
    /// Controls whether optional pushdown/acceleration metadata may fail open.
    pub optional_pushdown_policy: OptionalPushdownPolicy,
}

/// Policy for optional pushdown/acceleration sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OptionalPushdownPolicy {
    /// Corrupt optional metadata rejects the file, suitable for audit tooling.
    Strict,
    /// Corrupt optional metadata is ignored so readers can scan safely.
    FailOpen,
}

/// Optional pushdown metadata ignored under [`OptionalPushdownPolicy::FailOpen`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoredOptionalSection {
    pub section_id: u32,
    pub section_kind: u16,
    pub reason: String,
}

/// Coarse validation stages surfaced by [`ValidationReport`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ValidationStage {
    Bootstrap,
    Structural,
    SharedSemantic,
    DigestVerification,
    CoveTable,
    CoveObject,
    CoveEngine,
    CoveHarbor,
    CoveMap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ValidationStageStatus {
    Checked,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidationStageReport {
    pub stage: ValidationStage,
    pub status: ValidationStageStatus,
    pub sections_checked: u32,
}

impl Default for ValidationOptions {
    fn default() -> Self {
        Self {
            semantic: false,
            verify_digests: false,
            allow_unknown_optional_extensions: true,
            optional_pushdown_policy: OptionalPushdownPolicy::Strict,
        }
    }
}

/// Result of [`validate_bytes_with_options`].
#[derive(Debug, Clone)]
pub struct ValidationReport {
    /// The structurally validated file.
    pub validated: super::ValidatedCoveFile,
    /// Whether semantic checks were performed.
    pub semantic_checked: bool,
    /// Number of dictionary entries, if the dictionary was parsed.
    pub dict_entry_count: Option<u32>,
    /// Per-stage validation outcomes.
    pub stages: Vec<ValidationStageReport>,
    /// Optional pushdown sections ignored under fail-open validation.
    pub ignored_optional_sections: Vec<IgnoredOptionalSection>,
}

/// Validate a COVE file with configurable options.
///
/// Always performs structural validation (equivalent to [`super::validate_bytes`]).
/// When `opts.semantic` is true, additionally parses any file dictionary.
/// When `opts.verify_digests` is true, verifies any `DIGEST_MANIFEST` section
/// against section bytes.
pub fn validate_bytes_with_options(
    data: &[u8],
    opts: ValidationOptions,
) -> Result<ValidationReport, CoveError> {
    let (validated, mut ignored_optional_sections) =
        super::validate_bytes_with_optional_pushdown_policy(data, opts.optional_pushdown_policy)?;
    let mut stages = vec![
        ValidationStageReport {
            stage: ValidationStage::Bootstrap,
            status: ValidationStageStatus::Checked,
            sections_checked: 0,
        },
        ValidationStageReport {
            stage: ValidationStage::Structural,
            status: ValidationStageStatus::Checked,
            sections_checked: validated.footer.sections.len() as u32,
        },
    ];

    if !opts.semantic {
        push_skipped_semantic_stages(&mut stages, opts.verify_digests);
        return Ok(ValidationReport {
            validated,
            semantic_checked: false,
            dict_entry_count: None,
            stages,
            ignored_optional_sections,
        });
    }

    let mut dict_entry_count: Option<u32> = None;
    super::profile_validators::validate_shared_semantics(
        data,
        &validated,
        &opts,
        &mut dict_entry_count,
        &mut stages,
        &mut ignored_optional_sections,
    )?;
    if opts.verify_digests {
        let checked = super::digest_verification::verify_digest_manifests(
            data,
            &validated.footer,
            &ignored_optional_sections,
        )?;
        push_stage(
            &mut stages,
            ValidationStage::DigestVerification,
            ValidationStageStatus::Checked,
            checked,
        );
    } else {
        push_stage(
            &mut stages,
            ValidationStage::DigestVerification,
            ValidationStageStatus::Skipped,
            0,
        );
    }
    super::profile_validators::validate_cove_t_semantics(
        data,
        &validated,
        &opts,
        &mut stages,
        &mut ignored_optional_sections,
    )?;
    super::profile_validators::validate_cove_o_semantics(data, &validated, &mut stages)?;
    super::profile_validators::validate_cove_e_semantics(data, &validated, &mut stages)?;
    super::profile_validators::validate_cove_h_semantics(data, &validated, &mut stages)?;
    super::profile_validators::validate_cove_map_semantics(data, &validated, &mut stages)?;

    Ok(ValidationReport {
        validated,
        semantic_checked: opts.semantic,
        dict_entry_count,
        stages,
        ignored_optional_sections,
    })
}

pub fn validate_bytes_for_feature_use(
    data: &[u8],
    opts: ValidationOptions,
    request: FeatureUseRequestV2,
) -> Result<ValidationReport, CoveError> {
    let report = validate_bytes_with_options(data, opts)?;
    let scope_table = super::feature_scope_table_for(data, &report.validated)?;
    scope_table.reject_unknowns_for_request(&request)?;
    Ok(report)
}

pub(super) fn push_stage(
    stages: &mut Vec<ValidationStageReport>,
    stage: ValidationStage,
    status: ValidationStageStatus,
    sections_checked: u32,
) {
    stages.push(ValidationStageReport {
        stage,
        status,
        sections_checked,
    });
}

fn push_skipped_semantic_stages(stages: &mut Vec<ValidationStageReport>, verify_digests: bool) {
    push_stage(
        stages,
        ValidationStage::SharedSemantic,
        ValidationStageStatus::Skipped,
        0,
    );
    push_stage(
        stages,
        ValidationStage::DigestVerification,
        if verify_digests {
            ValidationStageStatus::Checked
        } else {
            ValidationStageStatus::Skipped
        },
        0,
    );
    for stage in [
        ValidationStage::CoveTable,
        ValidationStage::CoveObject,
        ValidationStage::CoveEngine,
        ValidationStage::CoveHarbor,
        ValidationStage::CoveMap,
    ] {
        push_stage(stages, stage, ValidationStageStatus::Skipped, 0);
    }
}
