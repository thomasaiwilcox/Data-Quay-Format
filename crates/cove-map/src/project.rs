use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{json, Map, Value};

use super::*;

pub(crate) fn diff_maps(left: &CovemapFile, right: &CovemapFile) -> Value {
    let left_sections = section_set(left);
    let right_sections = section_set(right);
    let added = right_sections
        .difference(&left_sections)
        .cloned()
        .collect::<Vec<_>>();
    let removed = left_sections
        .difference(&right_sections)
        .cloned()
        .collect::<Vec<_>>();
    let changed = left
        .sections
        .iter()
        .filter_map(|left_section| {
            right
                .sections
                .iter()
                .find(|right_section| {
                    right_section.entry.section_id == left_section.entry.section_id
                })
                .and_then(|right_section| {
                    (sha256_hex(&left_section.payload) != sha256_hex(&right_section.payload))
                        .then(|| section_kind(left_section.entry.section_id))
                })
        })
        .collect::<Vec<_>>();
    json!({
        "mapping_version_changed": left.mapping_version != right.mapping_version,
        "added_sections": added,
        "removed_sections": removed,
        "changed_sections": changed,
    })
}

pub(crate) fn project_rows(file: &CovemapFile, rows: &[SourceRow]) -> Result<Value, String> {
    project_rows_with_source_states(file, rows, &[])
}

pub(crate) fn project_rows_with_source_states(
    file: &CovemapFile,
    rows: &[SourceRow],
    source_states: &[ObservedSourceState],
) -> Result<Value, String> {
    let materialized = materialize_with_source_states(file, rows, source_states)?;
    let projection_catalog = projection_catalog(file)?
        .ok_or_else(|| "project requires a MAP_PROJECTION_CATALOG section".to_string())?;
    let mut projected_rows = Vec::new();
    for projection in &projection_catalog.projections {
        validate_executable_projection(projection)?;
        projected_rows.extend(project_one(&materialized, projection)?);
    }
    Ok(json!({
        "format": "json",
        "mapping_id": projection_catalog.mapping_id,
        "mapping_version": projection_catalog.mapping_version,
        "rows": projected_rows,
    }))
}

fn projection_catalog(file: &CovemapFile) -> Result<Option<MapProjectionCatalog>, String> {
    for section in embedded_sections(file)? {
        if let EmbeddedMapSection::ProjectionCatalog(catalog) = section {
            return Ok(Some(catalog));
        }
    }
    Ok(None)
}

fn validate_executable_projection(projection: &MapProjectionEntry) -> Result<(), String> {
    if projection.output_table.is_none()
        || projection.row_grain.is_none()
        || projection.anchor.is_none()
        || projection.temporal_mode.is_none()
        || projection.multi_value_policy.is_none()
        || projection.columns.is_empty()
        || projection.output_modes.is_empty()
    {
        return Err(format!(
            "projection '{}' uses the legacy preview schema; add output_table, row_grain, anchor, temporal_mode, multi_value_policy, columns, and output_modes",
            projection.projection_id
        ));
    }
    let temporal_mode = projection.temporal_mode.as_deref().unwrap_or_default();
    if !matches!(
        temporal_mode,
        "latest_committed" | "full_history" | "valid_time" | "observed_time" | "commit_order"
    ) {
        return Err(format!(
            "projection '{}' uses unsupported temporal_mode '{temporal_mode}'",
            projection.projection_id
        ));
    }
    let policy = projection.multi_value_policy.as_deref().unwrap_or_default();
    let row_grain = projection.row_grain.as_deref().unwrap_or_default();
    let uses_association_aggregate = projection
        .columns
        .iter()
        .any(|column| column.value.starts_with("count(association("));
    match policy {
        "aggregate" if uses_association_aggregate => {}
        "aggregate" => {
            return Err(format!(
                "projection '{}' declares aggregate multi_value_policy without an aggregate expression",
                projection.projection_id
            ));
        }
        "explode"
            if matches!(
                row_grain,
                "one_row_per_association" | "one_row_per_link_object"
            ) => {}
        "reject" if !uses_association_aggregate => {}
        "first" | "last" | "list" => {
            return Err(format!(
                "projection '{}' asks for unsupported multi_value_policy '{policy}'",
                projection.projection_id
            ));
        }
        _ if uses_association_aggregate => {
            return Err(format!(
                "projection '{}' must declare multi_value_policy='aggregate' for association aggregates",
                projection.projection_id
            ));
        }
        _ => {
            return Err(format!(
                "projection '{}' uses unsupported multi_value_policy '{policy}' for row_grain '{row_grain}'",
                projection.projection_id
            ));
        }
    }
    Ok(())
}

fn project_one(
    materialized: &MaterializedModel,
    projection: &MapProjectionEntry,
) -> Result<Vec<Value>, String> {
    let row_grain = projection
        .row_grain
        .as_deref()
        .ok_or_else(|| "projection row_grain is required".to_string())?;
    match row_grain {
        "one_row_per_object" => project_object_rows(materialized, projection, false),
        "one_row_per_association" | "one_row_per_link_object" => {
            project_object_rows(materialized, projection, true)
        }
        "one_row_per_property_version" => project_property_versions(materialized, projection),
        "one_row_per_evidence_assertion" => project_evidence_rows(materialized, projection),
        other => Err(format!("unsupported projection row_grain '{other}'")),
    }
}

fn project_object_rows(
    materialized: &MaterializedModel,
    projection: &MapProjectionEntry,
    associations: bool,
) -> Result<Vec<Value>, String> {
    let anchor = projection
        .anchor
        .as_ref()
        .ok_or_else(|| "projection anchor is required".to_string())?;
    let mut rows = Vec::new();
    for row in &materialized.rows {
        if associations {
            let Some(association_type) = &anchor.association_type else {
                continue;
            };
            if row.object_type != format!("Association:{association_type}") {
                continue;
            }
        } else {
            let Some(object_type) = &anchor.object_type else {
                continue;
            };
            if &row.object_type != object_type {
                continue;
            }
        }
        let mut out = Map::new();
        out.insert("projection_id".into(), json!(projection.projection_id));
        if let Some(output_table) = &projection.output_table {
            out.insert("output_table".into(), json!(output_table));
        }
        for column in &projection.columns {
            let value = projection_value(materialized, row, &column.value)?;
            out.insert(column.name.clone(), value);
        }
        rows.push(Value::Object(out));
    }
    Ok(rows)
}

fn project_property_versions(
    materialized: &MaterializedModel,
    projection: &MapProjectionEntry,
) -> Result<Vec<Value>, String> {
    let mut rows = Vec::new();
    for row in &materialized.rows {
        for property in row.properties.values() {
            let mut out = Map::new();
            out.insert("projection_id".into(), json!(projection.projection_id));
            out.insert("object_goid".into(), json!(hex_encode(&row.goid)));
            out.insert("property_id".into(), json!(property.entry.property_id));
            out.insert("property_name".into(), json!(property.entry.property_name));
            out.insert("value".into(), property.value.clone());
            rows.push(Value::Object(out));
        }
    }
    Ok(rows)
}

fn project_evidence_rows(
    materialized: &MaterializedModel,
    projection: &MapProjectionEntry,
) -> Result<Vec<Value>, String> {
    let mut rows = Vec::new();
    for evidence in &materialized.evidence_entries {
        let mut out = Map::new();
        out.insert("projection_id".into(), json!(projection.projection_id));
        for column in &projection.columns {
            let key = column
                .value
                .strip_prefix("evidence.")
                .ok_or_else(|| format!("unsupported evidence expression '{}'", column.value))?;
            out.insert(
                column.name.clone(),
                evidence.get(key).cloned().unwrap_or(Value::Null),
            );
        }
        rows.push(Value::Object(out));
    }
    Ok(rows)
}

fn projection_value(
    materialized: &MaterializedModel,
    row: &ObjectRow,
    expression: &str,
) -> Result<Value, String> {
    match expression {
        "goid" | "object.goid" | "Object.goid" | "association.goid" => {
            return Ok(json!(hex_encode(&row.goid)));
        }
        "object_type" | "object.type" | "Object.type" => return Ok(json!(row.object_type)),
        "association.source_goid" => return Ok(property_by_name(row, "source_goid")),
        "association.target_goid" => return Ok(property_by_name(row, "target_goid")),
        "association.association_type" => return Ok(property_by_name(row, "association_type")),
        "association.mapping_rule_id" => return Ok(property_by_name(row, "mapping_rule_id")),
        "association.source_evidence_id" => return Ok(property_by_name(row, "source_evidence_id")),
        "association.source_role" => return Ok(property_by_name(row, "source_role")),
        "association.target_role" => return Ok(property_by_name(row, "target_role")),
        "association.valid_from" => return Ok(property_by_name(row, "valid_from")),
        "association.valid_to" => return Ok(property_by_name(row, "valid_to")),
        "association.cardinality_policy" => return Ok(property_by_name(row, "cardinality_policy")),
        _ => {}
    }
    if let Some(inner) = expression
        .strip_prefix("count(association(")
        .and_then(|rest| rest.strip_suffix("))"))
    {
        let count = materialized
            .rows
            .iter()
            .filter(|candidate| candidate.object_type == format!("Association:{inner}"))
            .filter(|candidate| {
                property_by_name(candidate, "source_goid") == json!(hex_encode(&row.goid))
            })
            .count();
        return Ok(json!(count));
    }
    let property_name = expression
        .rsplit('.')
        .next()
        .ok_or_else(|| format!("unsupported projection expression '{expression}'"))?;
    Ok(property_by_name(row, property_name))
}

pub(crate) fn property_by_name(row: &ObjectRow, property_name: &str) -> Value {
    row.properties
        .values()
        .find(|property| property.entry.property_name == property_name)
        .map(|property| property.value.clone())
        .unwrap_or(Value::Null)
}

pub fn run_fixture_path(path: &Path) -> Result<(), String> {
    let bytes = fs::read(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    let fixture: Value = serde_json::from_slice(&bytes)
        .map_err(|err| format!("fixture {} is not valid JSON: {err}", path.display()))?;
    let map = PathBuf::from(required_str(&fixture, "mapping")?);
    let sources = fixture
        .get("sources")
        .and_then(Value::as_array)
        .ok_or_else(|| "fixture.sources must be an array".to_string())?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(PathBuf::from)
                .ok_or_else(|| "fixture.sources entries must be strings".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let file = parse_map(&map)?;
    let rows = read_sources(&sources)?;
    if let Some(expected_rows) = fixture.get("expected_projected_rows") {
        let projected = project_rows(&file, &rows)?;
        if &projected["rows"] != expected_rows {
            return Err("fixture projected rows did not match".into());
        }
    }
    println!("{}", json!({"ok": true, "fixture": path}));
    Ok(())
}
