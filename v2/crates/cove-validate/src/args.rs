use cove_core::{
    feature_binding::OperationKindV2,
    feature_scope::{FeatureTargetRefV2, FeatureUseRequestV2},
    reader::{OptionalPushdownPolicy, ValidationOptions},
};

pub(crate) const USAGE: &str = "Usage: cove-validate [--semantic] [--verify-digests] [--fail-open-optional-pushdown] [--json] [--explain] [--requested-profile N] [--requested-operation NAME|N] [--needed-section ID] [--needed-page SECTION_ID:TARGET_REF] [--needed-column-page SECTION_ID:COLUMN_ID:MORSEL_ID] <file.cove|file.covemap> [<file2> ...]";

#[derive(Clone)]
pub(crate) struct CliArgs {
    pub(crate) validation: ValidationOptions,
    pub(crate) feature_use: Option<FeatureUseRequestV2>,
    pub(crate) json_out: bool,
    pub(crate) explain: bool,
    pub(crate) file_paths: Vec<String>,
}

pub(crate) fn parse_args(args: impl IntoIterator<Item = String>) -> Result<CliArgs, String> {
    let mut semantic = false;
    let mut verify_digests = false;
    let mut fail_open_optional_pushdown = false;
    let mut json_out = false;
    let mut explain = false;
    let mut feature_use: Option<FeatureUseRequestV2> = None;
    let mut file_paths = Vec::new();

    let mut parsing_flags = true;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if parsing_flags && arg.starts_with("--") {
            match arg.as_str() {
                "--semantic" => semantic = true,
                "--verify-digests" => verify_digests = true,
                "--fail-open-optional-pushdown" => fail_open_optional_pushdown = true,
                "--json" => json_out = true,
                "--explain" => explain = true,
                "--requested-profile" => {
                    let value = args.next().ok_or_else(|| {
                        "--requested-profile requires a numeric profile id".to_string()
                    })?;
                    let profile = value
                        .parse::<u8>()
                        .map_err(|_| format!("Invalid --requested-profile value: {value}"))?;
                    feature_use
                        .get_or_insert_with(FeatureUseRequestV2::new)
                        .requested_profile = Some(profile);
                }
                "--requested-operation" => {
                    let value = args.next().ok_or_else(|| {
                        "--requested-operation requires an operation name or id".to_string()
                    })?;
                    let operation = parse_operation_kind(&value)?;
                    feature_use
                        .get_or_insert_with(FeatureUseRequestV2::new)
                        .requested_operation = Some(operation);
                }
                "--needed-section" => {
                    let value = args
                        .next()
                        .ok_or_else(|| "--needed-section requires a section id".to_string())?;
                    let section_id = value
                        .parse::<u32>()
                        .map_err(|_| format!("Invalid --needed-section value: {value}"))?;
                    feature_use
                        .get_or_insert_with(FeatureUseRequestV2::new)
                        .needed_section_ids
                        .insert(section_id);
                }
                "--needed-page" => {
                    let value = args.next().ok_or_else(|| {
                        "--needed-page requires SECTION_ID:TARGET_REF".to_string()
                    })?;
                    let target = parse_page_ref(&value)?;
                    feature_use
                        .get_or_insert_with(FeatureUseRequestV2::new)
                        .needed_page_refs
                        .insert(target);
                }
                "--needed-column-page" => {
                    let value = args.next().ok_or_else(|| {
                        "--needed-column-page requires SECTION_ID:COLUMN_ID:MORSEL_ID".to_string()
                    })?;
                    let target = parse_column_page_ref(&value)?;
                    feature_use
                        .get_or_insert_with(FeatureUseRequestV2::new)
                        .needed_page_refs
                        .insert(target);
                }
                other => return Err(format!("Unknown flag: {other}")),
            }
        } else {
            parsing_flags = false;
            file_paths.push(arg);
        }
    }

    if file_paths.is_empty() {
        return Err(USAGE.to_string());
    }

    Ok(CliArgs {
        validation: ValidationOptions {
            semantic,
            verify_digests,
            allow_unknown_optional_extensions: true,
            optional_pushdown_policy: if fail_open_optional_pushdown {
                OptionalPushdownPolicy::FailOpen
            } else {
                OptionalPushdownPolicy::Strict
            },
        },
        feature_use,
        json_out,
        explain,
        file_paths,
    })
}

fn parse_operation_kind(value: &str) -> Result<OperationKindV2, String> {
    if let Ok(raw) = value.parse::<u16>() {
        return OperationKindV2::from_u16(raw)
            .ok_or_else(|| format!("Unknown --requested-operation id: {raw}"));
    }
    let normalized = value.to_ascii_lowercase().replace('-', "_");
    match normalized.as_str() {
        "none" => Ok(OperationKindV2::None),
        "ordinary_table_scan" | "table_scan" => Ok(OperationKindV2::OrdinaryTableScan),
        "object_reconstruction" => Ok(OperationKindV2::ObjectReconstruction),
        "mapping_replay" => Ok(OperationKindV2::MappingReplay),
        "mapping_explanation" => Ok(OperationKindV2::MappingExplanation),
        "projection_readback" => Ok(OperationKindV2::ProjectionReadback),
        "trust_verification" => Ok(OperationKindV2::TrustVerification),
        "redaction_policy_evaluation" => Ok(OperationKindV2::RedactionPolicyEvaluation),
        "harbor_mount" => Ok(OperationKindV2::HarborMount),
        "engine_execution_mapping" => Ok(OperationKindV2::EngineExecutionMapping),
        "index_only_answer" => Ok(OperationKindV2::IndexOnlyAnswer),
        "coverage_planning" => Ok(OperationKindV2::CoveragePlanning),
        "zero_copy_export" => Ok(OperationKindV2::ZeroCopyExport),
        "runtime_adapter_selection" => Ok(OperationKindV2::RuntimeAdapterSelection),
        "vendor_defined" => Ok(OperationKindV2::VendorDefined),
        _ => Err(format!("Unknown --requested-operation value: {value}")),
    }
}

fn parse_page_ref(value: &str) -> Result<FeatureTargetRefV2, String> {
    let mut parts = value.split(':');
    let section_id = parse_next_u32(&mut parts, "section id", value)?;
    let target_local_ref = parse_next_u64(&mut parts, "target ref", value)?;
    if parts.next().is_some() {
        return Err(format!("Invalid page ref: {value}"));
    }
    Ok(FeatureTargetRefV2::new(section_id, target_local_ref))
}

fn parse_column_page_ref(value: &str) -> Result<FeatureTargetRefV2, String> {
    let mut parts = value.split(':');
    let section_id = parse_next_u32(&mut parts, "section id", value)?;
    let column_id = parse_next_u32(&mut parts, "column id", value)?;
    let morsel_id = parse_next_u32(&mut parts, "morsel id", value)?;
    if parts.next().is_some() {
        return Err(format!("Invalid column page ref: {value}"));
    }
    Ok(FeatureTargetRefV2::cove_t_column_page(
        section_id, column_id, morsel_id,
    ))
}

fn parse_next_u32<'a>(
    parts: &mut impl Iterator<Item = &'a str>,
    label: &str,
    original: &str,
) -> Result<u32, String> {
    parts
        .next()
        .ok_or_else(|| format!("Missing {label} in {original}"))?
        .parse::<u32>()
        .map_err(|_| format!("Invalid {label} in {original}"))
}

fn parse_next_u64<'a>(
    parts: &mut impl Iterator<Item = &'a str>,
    label: &str,
    original: &str,
) -> Result<u64, String> {
    parts
        .next()
        .ok_or_else(|| format!("Missing {label} in {original}"))?
        .parse::<u64>()
        .map_err(|_| format!("Invalid {label} in {original}"))
}

#[cfg(test)]
mod tests {
    use super::parse_args;

    #[test]
    fn parses_validation_flags() {
        let args = parse_args([
            "--semantic".to_string(),
            "--verify-digests".to_string(),
            "fixture.cove".to_string(),
        ])
        .unwrap();

        assert!(args.validation.semantic);
        assert!(args.validation.verify_digests);
        assert_eq!(args.file_paths, vec!["fixture.cove"]);
    }

    #[test]
    fn parses_feature_use_flags() {
        let args = parse_args([
            "--requested-profile".to_string(),
            "2".to_string(),
            "--requested-operation".to_string(),
            "coverage-planning".to_string(),
            "--needed-section".to_string(),
            "7".to_string(),
            "--needed-page".to_string(),
            "8:9".to_string(),
            "--needed-column-page".to_string(),
            "10:11:12".to_string(),
            "fixture.cove".to_string(),
        ])
        .unwrap();

        let feature_use = args.feature_use.unwrap();
        assert_eq!(feature_use.requested_profile, Some(2));
        assert_eq!(
            feature_use.requested_operation,
            Some(cove_core::feature_binding::OperationKindV2::CoveragePlanning)
        );
        assert!(feature_use.needed_section_ids.contains(&7));
        assert!(feature_use
            .needed_page_refs
            .contains(&cove_core::feature_scope::FeatureTargetRefV2::new(8, 9)));
        assert!(feature_use.needed_page_refs.contains(
            &cove_core::feature_scope::FeatureTargetRefV2::cove_t_column_page(10, 11, 12)
        ));
    }

    #[test]
    fn rejects_empty_input() {
        assert!(parse_args(Vec::<String>::new()).is_err());
    }
}
