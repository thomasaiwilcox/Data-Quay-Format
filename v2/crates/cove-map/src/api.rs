use std::{
    fs,
    path::{Path, PathBuf},
};

use cove_core::artifact::covemap::CovemapFile;
use serde_json::{json, Value};

use crate::{
    candidate_match_id,
    emit::build_cove_o_with_source_states,
    hex_encode,
    input::{read_source_inputs, validate_source_inputs, SourceRow},
    materialize_with_source_states, plan_identities,
    project::{
        project_cove_o_path, project_rows_with_source_states,
        project_rows_with_source_states_output, ProjectionFormat,
    },
    section_kind, MaterializedModel,
};

pub fn conversion_report_from_paths(map: &Path, sources: &[PathBuf]) -> Result<Value, String> {
    Ok(materialize_from_paths(map, sources)?.conversion_report)
}

pub fn conversion_summary_from_paths(map: &Path, sources: &[PathBuf]) -> Result<Value, String> {
    let materialized = materialize_from_paths(map, sources)?;
    Ok(json!({
        "report": materialized.conversion_report,
        "materialized_row_count": materialized.rows.len(),
        "evidence_entry_count": materialized.evidence_entries.len(),
        "assertion_count": materialized.assertions.len(),
    }))
}

pub fn cove_o_from_paths(map: &Path, sources: &[PathBuf]) -> Result<Vec<u8>, String> {
    let file = parse_map(map)?;
    let inputs = read_source_inputs(sources)?;
    validate_source_inputs(&file, &inputs.states)?;
    build_cove_o_with_source_states(&file, &inputs.rows, &inputs.states)
}

pub fn projected_rows_from_paths(map: &Path, sources: &[PathBuf]) -> Result<Value, String> {
    let file = parse_map(map)?;
    let inputs = read_source_inputs(sources)?;
    validate_source_inputs(&file, &inputs.states)?;
    project_rows_with_source_states(&file, &inputs.rows, &inputs.states)
}

pub fn projected_output_from_paths(
    map: &Path,
    sources: &[PathBuf],
    format: ProjectionFormat,
    projection_id: Option<&str>,
) -> Result<Vec<u8>, String> {
    let file = parse_map(map)?;
    let inputs = read_source_inputs(sources)?;
    validate_source_inputs(&file, &inputs.states)?;
    project_rows_with_source_states_output(
        &file,
        &inputs.rows,
        &inputs.states,
        format,
        projection_id,
    )
}

pub fn projected_rows_from_cove_o_path(
    object: &Path,
    mapping: Option<&Path>,
) -> Result<Value, String> {
    project_cove_o_path(object, mapping)
}

pub(crate) fn parse_map(path: &Path) -> Result<CovemapFile, String> {
    let bytes = fs::read(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    CovemapFile::parse_validated(&bytes).map_err(|err| format!("{}: {err}", path.display()))
}

pub(crate) fn preview(file: &CovemapFile) -> Value {
    json!({
        "mapping_version": file.mapping_version,
        "section_count": file.sections.len(),
        "sections": file.sections.iter().map(|section| {
            let kind = section_kind(section.entry.section_id);
            json!({
                "section_id": section.entry.section_id,
                "kind": kind,
                "required": section.entry.required,
                "payload_len": section.payload.len(),
            })
        }).collect::<Vec<_>>(),
    })
}

pub(crate) fn plan_keys(file: &CovemapFile, rows: &[SourceRow]) -> Value {
    let planned = match plan_identities(file, rows) {
        Ok(planned) => planned,
        Err(message) => return json!({"error": message}),
    };
    json!({
        "rows": planned.canonical.iter().map(|identity| {
            json!({
                "source_id": identity.source_id,
                "row_index": identity.row_index,
                "source_row_identity": identity.source_row_identity,
                "row_digest": identity.row_digest,
                "row_rule_id": identity.row_rule_id,
                "identity_rule_id": identity.identity_rule_id,
                "object_type": identity.object_type,
                "join_key_sha256": identity.join_key_sha256,
                "identity_alias": identity.identity_alias,
                "equivalence_id": identity.equivalence_id,
                "canonical_anchor": identity.canonical_anchor,
                "goid": hex_encode(&identity.goid),
            })
        }).collect::<Vec<_>>(),
        "candidate_matches": planned.candidates.iter().map(|candidate| {
            json!({
                "source_id": candidate.source_id,
                "row_index": candidate.row_index,
                "source_row_identity": candidate.source_row_identity,
                "row_digest": candidate.row_digest,
                "row_rule_id": candidate.row_rule_id,
                "identity_rule_id": candidate.identity_rule_id,
                "object_type": candidate.object_type,
                "join_key_sha256": candidate.join_key_sha256,
                "identity_alias": candidate.identity_alias,
                "candidate_match_id": candidate_match_id(candidate),
            })
        }).collect::<Vec<_>>()
    })
}

fn materialize_from_paths(map: &Path, sources: &[PathBuf]) -> Result<MaterializedModel, String> {
    let file = parse_map(map)?;
    let inputs = read_source_inputs(sources)?;
    validate_source_inputs(&file, &inputs.states)?;
    materialize_with_source_states(&file, &inputs.rows, &inputs.states)
}
