use std::collections::{BTreeMap, BTreeSet};

use cove_core::{
    artifact::covemap::CovemapFile,
    canonical::{CanonicalField, CanonicalValue},
    checksum,
    constants::{
        CompressionCodec, CoveEncodingKind, CoveLogicalType, CovePhysicalKind, PrimaryProfile,
        SectionKind, FEATURE_FILE_DICTIONARY, FEATURE_OBJECT_PROFILE, FEATURE_SEMANTIC_MAP,
        FEATURE_TRUST_CHAIN,
    },
    dictionary::{FileDictionary, FileDictionaryEncoding, FileDictionaryKey},
    durable,
    nested_schema::NestedSchemaNodeV1,
    page::{ColumnPageIndexEntryV1, COLUMN_PAGE_INDEX_ENTRY_LEN},
    page_payload::ColumnPagePayloadV1,
    profile::{
        cove_map::{
            MapIdentityRule, MapProjectionCatalog, MapProjectionEntry, MapPropertyBinding,
            MapRowSemanticRule, SourceOperationKind,
        },
        cove_o::{
            CoveRecordRefV1, ObjectTypeCatalog, ObjectTypeEntryV1, PropertyEntryV1, RecordKind,
            TemporalRowEntryV1, TemporalSegmentHeaderV1, TemporalSegmentIndex,
            TemporalSegmentIndexEntryV1, TrustManifest, TrustManifestEntryV1,
            OBJECT_TYPE_FLAG_ASSOCIATION_OBJECT, OBJECT_TYPE_FLAG_ENTITY_OBJECT,
            OBJECT_TYPE_FLAG_LINK_OBJECT, PROPERTY_FLAG_ASSOCIATION_FROM_GOID,
            PROPERTY_FLAG_ASSOCIATION_TO_GOID, PROPERTY_FLAG_ASSOCIATION_TYPE,
            PROPERTY_FLAG_EVIDENCE_REF, PROPERTY_FLAG_MAPPING_RULE_REF, TEMPORAL_ROW_ENTRY_LEN,
            TEMPORAL_SEGMENT_HEADER_LEN,
        },
    },
    reader::{validate_bytes_with_options, ValidationOptions},
    segment::{TableColumnDirectoryEntryV1, TABLE_COLUMN_DIRECTORY_ENTRY_LEN},
    trust_chain,
    writer::{MinimalCoveWriter, SectionPayload},
};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

mod api;
mod cli;
mod context;
mod emit;
mod identity;
mod input;
mod project;
mod sections;
mod ui;

#[cfg(test)]
use crate::cli::{parse_args, Command, OutputFormat};
pub use api::{
    conversion_report_from_paths, conversion_summary_from_paths, cove_o_from_paths,
    projected_output_from_paths, projected_rows_from_cove_o_path, projected_rows_from_paths,
};
pub(crate) use api::{parse_map, plan_keys, preview};
pub(crate) use context::{mapping_context, MappingContext};
#[cfg(test)]
use emit::build_cove_o;
use emit::build_cove_o_with_source_states;
pub(crate) use identity::{plan_identities, CandidateMatch, PlannedIdentity};
#[cfg(test)]
use input::read_csv;
use input::{
    read_source_inputs, read_sources, validate_source_inputs, ObservedSourceState, SourceRow,
};
pub use project::ProjectionFormat;
use project::{
    diff_maps, project_cove_o_path_output, project_rows_with_source_states_output, run_fixture_path,
};
#[cfg(test)]
use project::{project_cove_o_path, project_rows, property_by_name};
pub(crate) use sections::{embedded_sections, mapping_identity, section_kind};
#[cfg(test)]
use std::fs;
#[cfg(test)]
use std::path::PathBuf;
pub(crate) use ui::{
    candidate_assertion_id, candidate_match_id, evidence_entry_for_candidate,
    evidence_entry_for_identity, explain, identity_assertion_id, print_json, print_usage,
    write_or_print,
};

pub use cli::run_cli;

#[derive(Debug, Clone)]
struct ObjectRow {
    goid: [u8; 16],
    record_id: [u8; 16],
    object_type_id: u32,
    object_type: String,
    source_id: String,
    source_row_index: usize,
    record_kind: RecordKind,
    properties: BTreeMap<u32, MaterializedProperty>,
}

#[derive(Debug, Clone)]
struct MaterializedProperty {
    entry: PropertyEntryV1,
    value: Value,
    assertion_id: String,
    source_id: String,
    source_row_index: usize,
    source_priority: i64,
    source_order: usize,
    conflict_policy: String,
}

#[derive(Debug, Clone)]
struct MaterializedModel {
    object_types: Vec<ObjectTypeEntryV1>,
    rows: Vec<ObjectRow>,
    assertions: Vec<Value>,
    assertion_log: Value,
    identity_equivalence_index: Value,
    evidence_entries: Vec<Value>,
    evidence_index: Value,
    conversion_report: Value,
}

#[derive(Debug, Clone)]
struct TemporalSegmentBuild {
    segment_id: u32,
    object_type_id: u32,
    rows: Vec<ObjectRow>,
    payload: Vec<u8>,
}

type NestedShapeByProperty = BTreeMap<(u32, u32), NestedSchemaNodeV1>;

fn materialize_with_source_states(
    file: &CovemapFile,
    rows: &[SourceRow],
    source_states: &[ObservedSourceState],
) -> Result<MaterializedModel, String> {
    let context = mapping_context(file)?;
    let identity_plan = plan_identities(file, rows)?;
    let planned = &identity_plan.canonical;
    let object_types = object_types_from_mapping(&context)?;
    let type_ids = object_types
        .iter()
        .map(|ty| (ty.type_name.clone(), ty.object_type_id))
        .collect::<BTreeMap<_, _>>();
    let properties_by_type = object_types
        .iter()
        .map(|ty| {
            (
                ty.object_type_id,
                ty.properties
                    .iter()
                    .map(|property| (property.property_id, property.clone()))
                    .collect::<BTreeMap<_, _>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let source_rows = rows
        .iter()
        .map(|row| ((row.source_id.clone(), row.row_index), row))
        .collect::<BTreeMap<_, _>>();
    let planned_by_key = planned
        .iter()
        .map(|identity| {
            (
                (
                    identity.source_id.clone(),
                    identity.row_index,
                    identity.identity_rule_id.clone(),
                ),
                identity,
            )
        })
        .collect::<BTreeMap<_, _>>();
    let planned_by_join = planned
        .iter()
        .map(|identity| {
            (
                (
                    identity.identity_rule_id.clone(),
                    identity.join_key_sha256.clone(),
                ),
                identity,
            )
        })
        .collect::<BTreeMap<_, _>>();
    let row_rules = context
        .row_rules
        .iter()
        .map(|rule| (rule.rule_id.clone(), rule))
        .collect::<BTreeMap<_, _>>();
    let (mapping_id, mapping_version) = mapping_identity(file)?;
    let mut object_rows = Vec::new();
    let mut assertions = Vec::new();
    let mut evidence_entries = Vec::new();
    for row_rule in &context.row_rules {
        for binding in &row_rule.property_bindings {
            push_unique_assertion(
                &mut assertions,
                &binding.assertion_id,
                &format!("property:{}", binding.assertion_id),
            );
        }
        for binding in &row_rule.association_bindings {
            push_unique_assertion(
                &mut assertions,
                &binding.assertion_id,
                &format!("association:{}", binding.assertion_id),
            );
        }
    }

    for candidate in &identity_plan.candidates {
        let assertion_id = candidate_assertion_id(candidate);
        let candidate_id = candidate_match_id(candidate);
        push_unique_assertion(&mut assertions, &assertion_id, &candidate_id);
        let mut evidence = evidence_entry_for_candidate(candidate);
        if let Some(row_rule) = row_rules.get(&candidate.row_rule_id) {
            add_operation_metadata(&mut evidence, row_rule, None);
        }
        evidence_entries.push(evidence);
    }

    for identity in planned {
        let row_rule = row_rules.get(&identity.row_rule_id).ok_or_else(|| {
            format!(
                "planned row references missing row rule '{}'",
                identity.row_rule_id
            )
        })?;
        let source_row = source_rows
            .get(&(identity.source_id.clone(), identity.row_index))
            .ok_or_else(|| "planned identity references missing source row".to_string())?;
        let assertion_id = identity_assertion_id(identity);
        if !row_rule_materializes_object(row_rule)? {
            if row_rule_emits_non_object_evidence(row_rule) {
                push_unique_assertion(&mut assertions, &assertion_id, &hex_encode(&identity.goid));
                let mut evidence = evidence_entry_for_identity(identity);
                add_operation_metadata(&mut evidence, row_rule, Some(source_row));
                evidence_entries.push(evidence);
            }
            continue;
        }
        let object_type_id = *type_ids
            .get(&identity.object_type)
            .ok_or_else(|| format!("unknown object type '{}'", identity.object_type))?;
        let properties = materialize_properties(
            &context,
            row_rule,
            source_row,
            object_type_id,
            &properties_by_type,
        )?;
        let record_id = record_id_for(
            &identity.source_id,
            identity.row_index,
            &identity.row_rule_id,
            &identity.goid,
        );
        object_rows.push(ObjectRow {
            goid: identity.goid,
            record_id,
            object_type_id,
            object_type: identity.object_type.clone(),
            source_id: identity.source_id.clone(),
            source_row_index: identity.row_index,
            record_kind: record_kind_for_row_rule(row_rule)?,
            properties,
        });
        push_unique_assertion(&mut assertions, &assertion_id, &hex_encode(&identity.goid));
        let mut evidence = evidence_entry_for_identity(identity);
        add_operation_metadata(&mut evidence, row_rule, Some(source_row));
        evidence_entries.push(evidence);
    }

    materialize_associations(
        file,
        &context,
        planned,
        &planned_by_key,
        &planned_by_join,
        &source_rows,
        &type_ids,
        &properties_by_type,
        &mut object_rows,
        &mut assertions,
        &mut evidence_entries,
    )?;

    resolve_property_conflicts(&mut object_rows, &mut evidence_entries)?;

    object_rows.sort_by_key(|row| {
        (
            row.object_type_id,
            row.source_id.clone(),
            row.source_row_index,
            row.goid,
            row.record_id,
        )
    });
    let conversion_report = json!({
        "mapping_id": mapping_id,
        "mapping_version": mapping_version,
        "sources": conversion_report_sources(rows, source_states),
        "source_count": rows.iter().map(|row| row.source_id.clone()).collect::<BTreeSet<_>>().len(),
        "row_count": rows.len(),
        "object_count": object_rows.iter().filter(|row| !row.object_type.starts_with("Association:")).count(),
        "association_count": object_rows.iter().filter(|row| row.object_type.starts_with("Association:")).count(),
        "property_value_count": object_rows.iter().map(|row| row.properties.len()).sum::<usize>(),
        "candidate_match_count": identity_plan.candidates.len(),
        "candidate_matches": identity_plan.candidates.iter().map(|candidate| {
            json!({
                "candidate_match_id": candidate_match_id(candidate),
                "source_id": candidate.source_id,
                "source_row_identity": candidate.source_row_identity,
                "row_rule_id": candidate.row_rule_id,
                "identity_rule_id": candidate.identity_rule_id,
                "object_type": candidate.object_type,
                "join_key_sha256": candidate.join_key_sha256,
            })
        }).collect::<Vec<_>>(),
        "generated_artifacts": ["cove-o", "map-assertion-log", "map-identity-equivalence-index", "map-evidence-index"],
        "unsupported": [],
        "operation_counts": operation_counts(&evidence_entries),
        "governance": governance_report(&context, rows)?,
    });
    let assertion_log = json!({
        "mapping_id": mapping_id,
        "mapping_version": mapping_version,
        "assertions": assertions,
    });
    let identity_equivalence_index =
        identity_equivalence_index(&mapping_id, &mapping_version, planned);
    let evidence_index = json!({
        "mapping_id": mapping_id,
        "mapping_version": mapping_version,
        "entries": evidence_entries,
    });
    Ok(MaterializedModel {
        object_types,
        rows: object_rows,
        assertions,
        assertion_log,
        identity_equivalence_index,
        evidence_entries,
        evidence_index,
        conversion_report,
    })
}

fn push_unique_assertion(assertions: &mut Vec<Value>, assertion_id: &str, output_object_id: &str) {
    if assertions.iter().any(|entry| {
        entry.get("assertion_id").and_then(Value::as_str) == Some(assertion_id)
            || entry.get("output_object_id").and_then(Value::as_str) == Some(output_object_id)
    }) {
        return;
    }
    assertions.push(json!({
        "assertion_id": assertion_id,
        "output_object_id": output_object_id,
    }));
}

fn conversion_report_sources(rows: &[SourceRow], source_states: &[ObservedSourceState]) -> Value {
    if !source_states.is_empty() {
        return Value::Array(
            source_states
                .iter()
                .map(|state| {
                    json!({
                        "source_id": state.source_id,
                        "source_kind": state.source_kind,
                        "schema_fingerprint": state.schema_fingerprint,
                        "snapshot_digest": state.snapshot_digest,
                    })
                })
                .collect(),
        );
    }
    Value::Array(
        rows.iter()
            .map(|row| {
                json!({
                    "source_id": row.source_id,
                    "schema_fingerprint": schema_fingerprint(row),
                })
            })
            .collect(),
    )
}

fn add_operation_metadata(
    evidence: &mut Value,
    row_rule: &MapRowSemanticRule,
    source_row: Option<&SourceRow>,
) {
    let Some(object) = evidence.as_object_mut() else {
        return;
    };
    object.insert(
        "source_operation_kind".into(),
        json!(row_rule.source_operation_kind.as_str()),
    );
    object.insert(
        "operation_effect".into(),
        json!(operation_effect(row_rule.source_operation_kind)),
    );
    object.insert("operation_target".into(), json!(operation_target(row_rule)));
    if let Some(source_row) = source_row {
        copy_operation_policy_value(object, source_row, "correction_of");
        copy_operation_policy_value(object, source_row, "replacement_of");
        copy_operation_policy_value(object, source_row, "redaction_scope");
        copy_operation_policy_value(object, source_row, "expires_previous");
        copy_operation_policy_value(object, source_row, "closes_association");
    }
}

fn copy_operation_policy_value(object: &mut Map<String, Value>, source_row: &SourceRow, key: &str) {
    if let Some(value) = source_row.values.get(key).filter(|value| !value.is_null()) {
        object.insert(key.to_string(), value.clone());
    }
}

fn operation_counts(evidence_entries: &[Value]) -> Value {
    let mut counts = BTreeMap::<String, u64>::new();
    for entry in evidence_entries {
        if let Some(kind) = entry.get("source_operation_kind").and_then(Value::as_str) {
            *counts.entry(kind.to_string()).or_default() += 1;
        }
    }
    json!(counts)
}

fn operation_effect(kind: SourceOperationKind) -> &'static str {
    match kind {
        SourceOperationKind::Fact => "fact",
        SourceOperationKind::Insert => "insert_object_state",
        SourceOperationKind::Upsert => "upsert_object_state",
        SourceOperationKind::PatchProperty => "patch_property",
        SourceOperationKind::ReplaceObjectState => "replace_object_state",
        SourceOperationKind::CloseAssociation => "close_association",
        SourceOperationKind::ExpireAndCreate => "expire_and_create",
        SourceOperationKind::TombstoneObject => "tombstone_object",
        SourceOperationKind::TombstoneProperty => "tombstone_property",
        SourceOperationKind::TombstoneAssociation => "tombstone_association",
        SourceOperationKind::RedactEvidence => "redact_evidence",
        SourceOperationKind::EvidenceOnly => "evidence_only",
        SourceOperationKind::Correction => "correction",
    }
}

fn operation_target(row_rule: &MapRowSemanticRule) -> &'static str {
    if let Some(target) = row_rule.tombstone_target.as_deref() {
        return match target {
            "property" => "property",
            "association" => "association",
            "source_record" => "source_record",
            "evidence" => "evidence",
            _ => "object",
        };
    }
    match row_rule.source_operation_kind {
        SourceOperationKind::PatchProperty | SourceOperationKind::TombstoneProperty => "property",
        SourceOperationKind::CloseAssociation | SourceOperationKind::TombstoneAssociation => {
            "association"
        }
        SourceOperationKind::RedactEvidence | SourceOperationKind::EvidenceOnly => "evidence",
        _ => "object",
    }
}

fn row_rule_emits_non_object_evidence(row_rule: &MapRowSemanticRule) -> bool {
    row_rule.assertion_kinds.iter().any(|kind| {
        matches!(
            kind.as_str(),
            "evidence" | "candidate_match" | "conflict" | "projection"
        )
    }) || matches!(
        row_rule.source_operation_kind,
        SourceOperationKind::EvidenceOnly | SourceOperationKind::RedactEvidence
    )
}

fn governance_report(context: &MappingContext, rows: &[SourceRow]) -> Result<Value, String> {
    let used_source_ids = rows
        .iter()
        .map(|row| row.source_id.clone())
        .collect::<BTreeSet<_>>();
    let mut sources = Vec::new();
    let mut access_policy_ids = BTreeSet::<String>::new();
    let mut sensitivity_identities = BTreeSet::<(Option<String>, Option<i64>)>::new();
    let mut max_sensitivity_rank = 0i64;
    let mut labels_by_rank = BTreeMap::<i64, BTreeSet<String>>::new();

    for source_id in used_source_ids {
        let Some(source) = context.sources.get(&source_id) else {
            sources.push(json!({ "source_id": source_id }));
            continue;
        };
        for policy_id in &source.access_policy_ids {
            access_policy_ids.insert(policy_id.clone());
        }
        if source.sensitivity_label.is_some() || source.sensitivity_rank.is_some() {
            sensitivity_identities
                .insert((source.sensitivity_label.clone(), source.sensitivity_rank));
        }
        let rank = source.sensitivity_rank.unwrap_or(0);
        max_sensitivity_rank = max_sensitivity_rank.max(rank);
        if let Some(label) = &source.sensitivity_label {
            labels_by_rank
                .entry(rank)
                .or_default()
                .insert(label.clone());
        }
        sources.push(json!({
            "source_id": source.source_id,
            "source_priority": source.source_priority,
            "sensitivity_label": source.sensitivity_label.clone(),
            "sensitivity_rank": source.sensitivity_rank,
            "access_policy_ids": source.access_policy_ids.clone(),
        }));
    }

    if context.governance_reconciliation_policy == "reject_on_mixed_sensitivity"
        && sensitivity_identities.len() > 1
    {
        return Err("mixed source sensitivity labels require governance reconciliation".into());
    }

    Ok(json!({
        "reconciliation_policy": context.governance_reconciliation_policy,
        "sources": sources,
        "effective_sensitivity_rank": max_sensitivity_rank,
        "effective_sensitivity_labels": labels_by_rank
            .remove(&max_sensitivity_rank)
            .unwrap_or_default()
            .into_iter()
            .collect::<Vec<_>>(),
        "access_policy_ids": access_policy_ids.into_iter().collect::<Vec<_>>(),
    }))
}

fn materialize_properties(
    context: &MappingContext,
    row_rule: &MapRowSemanticRule,
    source_row: &SourceRow,
    object_type_id: u32,
    properties_by_type: &BTreeMap<u32, BTreeMap<u32, PropertyEntryV1>>,
) -> Result<BTreeMap<u32, MaterializedProperty>, String> {
    let declared = properties_by_type
        .get(&object_type_id)
        .ok_or_else(|| format!("object_type_id {object_type_id} has no property catalog"))?;
    let mut properties = BTreeMap::new();
    for (index, binding) in row_rule.property_bindings.iter().enumerate() {
        let property_id = property_id_from_binding(binding, index as u32 + 1);
        let entry = declared.get(&property_id).ok_or_else(|| {
            format!(
                "row rule '{}' references undeclared property '{}'",
                row_rule.rule_id, binding.property_id
            )
        })?;
        let value = source_value_for_binding(source_row, binding)?;
        validate_property_conflict_policy(&binding.conflict_policy)?;
        if value.is_null() && !entry.nullable {
            return Err(format!(
                "non-nullable property '{}' was null/missing for {}:{}",
                binding.property_name, source_row.source_id, source_row.row_index
            ));
        }
        let source_order = context
            .source_order
            .get(&source_row.source_id)
            .copied()
            .unwrap_or(usize::MAX);
        let source_priority = binding
            .source_priority
            .or_else(|| {
                context
                    .sources
                    .get(&source_row.source_id)
                    .and_then(|source| source.source_priority)
            })
            .unwrap_or(source_order as i64);
        if properties
            .insert(
                property_id,
                MaterializedProperty {
                    entry: entry.clone(),
                    value,
                    assertion_id: binding.assertion_id.clone(),
                    source_id: source_row.source_id.clone(),
                    source_row_index: source_row.row_index,
                    source_priority,
                    source_order,
                    conflict_policy: binding.conflict_policy.clone(),
                },
            )
            .is_some()
            && binding.conflict_policy == "reject_conflict"
        {
            return Err(format!(
                "duplicate materialized value for property '{}'",
                binding.property_name
            ));
        }
    }
    Ok(properties)
}

fn validate_property_conflict_policy(policy: &str) -> Result<(), String> {
    match policy {
        "reject_conflict" | "source_priority_wins" => Ok(()),
        other => Err(format!("unsupported property conflict_policy '{other}'")),
    }
}

fn resolve_property_conflicts(
    rows: &mut [ObjectRow],
    evidence_entries: &mut Vec<Value>,
) -> Result<(), String> {
    let mut groups = BTreeMap::<([u8; 16], u32), Vec<(usize, MaterializedProperty)>>::new();
    for (row_index, row) in rows.iter().enumerate() {
        for (property_id, property) in &row.properties {
            groups
                .entry((row.goid, *property_id))
                .or_default()
                .push((row_index, property.clone()));
        }
    }

    let mut removals = Vec::<(usize, u32, String)>::new();
    for ((goid, property_id), candidates) in groups {
        if candidates.len() <= 1 {
            continue;
        }
        let policies = candidates
            .iter()
            .map(|(_, property)| property.conflict_policy.as_str())
            .collect::<BTreeSet<_>>();
        if policies.len() != 1 {
            return Err(format!(
                "conflicting policies declared for property_id {property_id} on {}",
                hex_encode(&goid)
            ));
        }
        let policy = policies.iter().next().copied().unwrap_or("reject_conflict");
        validate_property_conflict_policy(policy)?;

        let non_null = candidates
            .iter()
            .filter(|(_, property)| !property.value.is_null())
            .cloned()
            .collect::<Vec<_>>();
        if non_null.is_empty() {
            continue;
        }

        match policy {
            "reject_conflict" => {
                let first = &non_null[0].1.value;
                if non_null
                    .iter()
                    .any(|(_, property)| property.value != *first)
                {
                    return Err(format!(
                        "unresolved property conflict for property_id {property_id} on {}",
                        hex_encode(&goid)
                    ));
                }
                for (row_index, property) in candidates {
                    if property.value.is_null() {
                        removals.push((
                            row_index,
                            property_id,
                            "null_does_not_overwrite_non_null".into(),
                        ));
                    }
                }
            }
            "source_priority_wins" => {
                let (winner_row, winner) = non_null
                    .iter()
                    .min_by_key(|(row_index, property)| {
                        (
                            property.source_priority,
                            property.source_order,
                            property.source_row_index,
                            property.assertion_id.clone(),
                            *row_index,
                        )
                    })
                    .map(|(row_index, property)| (*row_index, property.clone()))
                    .ok_or_else(|| "empty source-priority conflict group".to_string())?;
                for (row_index, property) in candidates {
                    if row_index != winner_row || property.assertion_id != winner.assertion_id {
                        removals.push((row_index, property_id, "source_priority_wins".into()));
                    }
                }
            }
            _ => unreachable!(),
        }
    }

    for (row_index, property_id, reason) in removals {
        if let Some(property) = rows
            .get_mut(row_index)
            .and_then(|row| row.properties.remove(&property_id))
        {
            let source_id = property.source_id.clone();
            evidence_entries.push(json!({
                "source_id": source_id,
                "source_row_identity": format!("{}:{}", property.source_id, property.source_row_index),
                "rule_id": "property_conflict_resolution",
                "assertion_id": property.assertion_id,
                "output_object_id": hex_encode(&rows[row_index].goid),
                "property_id": property_id,
                "property_name": property.entry.property_name,
                "suppressed": true,
                "suppressed_reason": reason,
                "suppressed_value": property.value,
            }));
        }
    }

    Ok(())
}

fn source_value_for_binding(
    source_row: &SourceRow,
    binding: &MapPropertyBinding,
) -> Result<Value, String> {
    source_value_for_expression(
        source_row,
        &binding.value_expression,
        Some(&binding.source_column),
        &binding.missing_policy,
        &binding.property_name,
    )
}

fn source_value_for_expression(
    source_row: &SourceRow,
    expression: &str,
    fallback_column: Option<&str>,
    missing_policy: &str,
    label: &str,
) -> Result<Value, String> {
    let expression = expression.trim();
    let column = expression.strip_prefix("source.").unwrap_or_else(|| {
        if expression.is_empty() {
            fallback_column.unwrap_or("")
        } else {
            expression
        }
    });
    match source_row.values.get(column) {
        Some(value) if !value.is_null() => Ok(value.clone()),
        _ if missing_policy == "reject" => Err(format!(
            "source column '{}' required by '{}' is missing/null",
            column, label
        )),
        _ => Ok(Value::Null),
    }
}

fn association_validity_value(
    source_row: &SourceRow,
    expression: Option<&str>,
    missing_policy: &str,
    label: &str,
) -> Result<Option<Value>, String> {
    let Some(expression) = expression else {
        return Ok(Some(Value::Null));
    };
    let value = source_value_for_expression(source_row, expression, None, "null", label)?;
    if !value.is_null() {
        return Ok(Some(value));
    }
    match missing_policy {
        "reject" => Err(format!(
            "association {label} expression '{expression}' is missing/null"
        )),
        "skip" => Ok(None),
        _ => Ok(Some(Value::Null)),
    }
}

#[allow(clippy::too_many_arguments)]
fn materialize_associations(
    file: &CovemapFile,
    context: &MappingContext,
    planned: &[PlannedIdentity],
    planned_by_key: &BTreeMap<(String, usize, String), &PlannedIdentity>,
    planned_by_join: &BTreeMap<(String, String), &PlannedIdentity>,
    source_rows: &BTreeMap<(String, usize), &SourceRow>,
    type_ids: &BTreeMap<String, u32>,
    properties_by_type: &BTreeMap<u32, BTreeMap<u32, PropertyEntryV1>>,
    object_rows: &mut Vec<ObjectRow>,
    assertions: &mut Vec<Value>,
    evidence_entries: &mut Vec<Value>,
) -> Result<(), String> {
    let (mapping_id, mapping_version) = mapping_identity(file)?;
    let row_rules = context
        .row_rules
        .iter()
        .map(|rule| (rule.rule_id.clone(), rule))
        .collect::<BTreeMap<_, _>>();
    for identity in planned {
        let row_rule = row_rules.get(&identity.row_rule_id).ok_or_else(|| {
            format!(
                "planned identity references missing row rule '{}'",
                identity.row_rule_id
            )
        })?;
        if !row_rule_materializes_associations(row_rule)? {
            continue;
        }
        for binding in &row_rule.association_bindings {
            let source_rule = if binding.source_identity_rule_id.is_empty() {
                &row_rule.identity_rule_id
            } else {
                &binding.source_identity_rule_id
            };
            if &identity.identity_rule_id != source_rule {
                continue;
            }
            let source_row = source_rows
                .get(&(identity.source_id.clone(), identity.row_index))
                .ok_or_else(|| "association references missing source row".to_string())?;
            let Some(source_endpoint) = resolve_association_endpoint(
                &binding.source_endpoint_expression,
                source_rule,
                identity,
                source_row,
                context,
                type_ids,
                planned_by_key,
                planned_by_join,
            )?
            else {
                if binding.missing_policy == "skip" {
                    continue;
                }
                return Err(format!(
                    "association '{}' could not resolve source endpoint '{}'",
                    binding.association_type, binding.source_endpoint_expression
                ));
            };
            let Some(target) = resolve_association_endpoint(
                &binding.target_endpoint_expression,
                &binding.target_identity_rule_id,
                identity,
                source_row,
                context,
                type_ids,
                planned_by_key,
                planned_by_join,
            )?
            else {
                if binding.missing_policy == "skip" {
                    continue;
                }
                return Err(format!(
                    "association '{}' could not resolve target identity rule '{}'",
                    binding.association_type, binding.target_identity_rule_id
                ));
            };
            let object_type = format!("Association:{}", binding.association_type);
            let object_type_id = *type_ids
                .get(&object_type)
                .ok_or_else(|| format!("missing association object type '{object_type}'"))?;
            let declared = properties_by_type
                .get(&object_type_id)
                .ok_or_else(|| format!("association type '{object_type}' has no properties"))?;
            let association_goid = association_goid(
                mapping_id.as_bytes(),
                mapping_version.as_bytes(),
                binding,
                &source_endpoint.goid,
                &target.goid,
            );
            let assertion_id = format!("{}:{}", binding.assertion_id, identity.row_digest);
            let source_evidence_id = format!("{}:{}", identity.source_id, identity.row_index);
            let Some(valid_from) = association_validity_value(
                source_row,
                binding.valid_from_expression.as_deref(),
                &binding.missing_policy,
                "valid_from",
            )?
            else {
                continue;
            };
            let Some(valid_to) = association_validity_value(
                source_row,
                binding.valid_to_expression.as_deref(),
                &binding.missing_policy,
                "valid_to",
            )?
            else {
                continue;
            };
            let property_values = BTreeMap::from([
                (1u32, json!(hex_encode(&source_endpoint.goid))),
                (2u32, json!(hex_encode(&target.goid))),
                (3u32, json!(binding.association_type)),
                (4u32, json!(row_rule.rule_id)),
                (5u32, json!(source_evidence_id)),
                (6u32, json!(binding.source_role)),
                (7u32, json!(binding.target_role)),
                (8u32, valid_from),
                (9u32, valid_to),
                (10u32, json!(binding.cardinality_policy)),
            ]);
            let mut properties = BTreeMap::new();
            for (property_id, value) in property_values {
                let entry = declared.get(&property_id).ok_or_else(|| {
                    format!("association property_id {property_id} is not declared")
                })?;
                properties.insert(
                    property_id,
                    MaterializedProperty {
                        entry: entry.clone(),
                        value,
                        assertion_id: binding.assertion_id.clone(),
                        source_id: identity.source_id.clone(),
                        source_row_index: identity.row_index,
                        source_priority: context
                            .sources
                            .get(&identity.source_id)
                            .and_then(|source| source.source_priority)
                            .unwrap_or_else(|| {
                                context
                                    .source_order
                                    .get(&identity.source_id)
                                    .copied()
                                    .unwrap_or(usize::MAX) as i64
                            }),
                        source_order: context
                            .source_order
                            .get(&identity.source_id)
                            .copied()
                            .unwrap_or(usize::MAX),
                        conflict_policy: "reject_conflict".into(),
                    },
                );
            }
            let record_id = record_id_for(
                &identity.source_id,
                identity.row_index,
                &binding.assertion_id,
                &association_goid,
            );
            object_rows.push(ObjectRow {
                goid: association_goid,
                record_id,
                object_type_id,
                object_type: object_type.clone(),
                source_id: identity.source_id.clone(),
                source_row_index: identity.row_index,
                record_kind: association_record_kind_for_row_rule(row_rule),
                properties,
            });
            push_unique_assertion(
                &mut *assertions,
                &assertion_id,
                &hex_encode(&association_goid),
            );
            let mut evidence = json!({
                "source_id": identity.source_id,
                "source_row_identity": identity.source_row_identity,
                "rule_id": row_rule.rule_id,
                "assertion_id": assertion_id,
                "output_object_id": hex_encode(&association_goid),
                "observed_schema_fingerprint": identity.schema_fingerprint,
            });
            add_operation_metadata(&mut evidence, row_rule, Some(source_row));
            evidence_entries.push(evidence);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn resolve_association_endpoint<'a>(
    expression: &str,
    default_identity_rule_id: &str,
    current_identity: &'a PlannedIdentity,
    source_row: &SourceRow,
    context: &MappingContext,
    type_ids: &BTreeMap<String, u32>,
    planned_by_key: &BTreeMap<(String, usize, String), &'a PlannedIdentity>,
    planned_by_join: &BTreeMap<(String, String), &'a PlannedIdentity>,
) -> Result<Option<&'a PlannedIdentity>, String> {
    let expression = expression.trim();
    if expression == "source.goid" {
        return Ok(Some(current_identity));
    }
    let rule_id = if expression == "target.goid" || expression.is_empty() {
        default_identity_rule_id
    } else if let Some(rule_id) = expression
        .strip_prefix("identity(")
        .and_then(|value| value.strip_suffix(')'))
    {
        rule_id.trim()
    } else {
        return Err(format!(
            "unsupported association endpoint expression '{expression}'"
        ));
    };

    if rule_id == current_identity.identity_rule_id {
        return Ok(Some(current_identity));
    }
    if let Some(identity) = planned_by_key.get(&(
        source_row.source_id.clone(),
        source_row.row_index,
        rule_id.to_string(),
    )) {
        return Ok(Some(*identity));
    }
    let rule = context.identity_rules.get(rule_id).ok_or_else(|| {
        format!("association endpoint references missing identity rule '{rule_id}'")
    })?;
    let object_type_id = *type_ids
        .get(&rule.object_type)
        .ok_or_else(|| format!("unknown object type '{}'", rule.object_type))?;
    let tuple = join_key_tuple_from_rule(rule, source_row, object_type_id)?;
    let digest = sha256_hex(&tuple);
    Ok(planned_by_join.get(&(rule_id.to_string(), digest)).copied())
}

fn row_rule_materializes_object(row_rule: &MapRowSemanticRule) -> Result<bool, String> {
    match row_rule.row_semantics_kind.as_str() {
        "Object" | "EventObject" | "LinkObject" | "Composite" | "Dispatched"
        | "KeyValueFragment" | "Tombstone" => Ok(true),
        "AssociationOnly" | "EvidenceOnly" | "ProjectionOnly" => Ok(false),
        other => Err(format!("unsupported row_semantics_kind '{other}'")),
    }
}

fn row_rule_materializes_associations(row_rule: &MapRowSemanticRule) -> Result<bool, String> {
    match row_rule.row_semantics_kind.as_str() {
        "Object" | "EventObject" | "LinkObject" | "AssociationOnly" | "Composite"
        | "Dispatched" | "KeyValueFragment" => Ok(true),
        "EvidenceOnly" | "ProjectionOnly" | "Tombstone" => Ok(false),
        other => Err(format!("unsupported row_semantics_kind '{other}'")),
    }
}

fn record_kind_for_row_rule(row_rule: &MapRowSemanticRule) -> Result<RecordKind, String> {
    if row_rule.row_semantics_kind == "Tombstone" {
        return Ok(RecordKind::Tombstone);
    }
    match row_rule.source_operation_kind {
        SourceOperationKind::PatchProperty
        | SourceOperationKind::CloseAssociation
        | SourceOperationKind::ExpireAndCreate
        | SourceOperationKind::RedactEvidence
        | SourceOperationKind::Correction => return Ok(RecordKind::Delta),
        SourceOperationKind::ReplaceObjectState => return Ok(RecordKind::Snapshot),
        SourceOperationKind::TombstoneObject
        | SourceOperationKind::TombstoneProperty
        | SourceOperationKind::TombstoneAssociation => return Ok(RecordKind::Tombstone),
        SourceOperationKind::Fact
        | SourceOperationKind::Insert
        | SourceOperationKind::Upsert
        | SourceOperationKind::EvidenceOnly => {}
    }
    record_kind_from_name(&row_rule.record_kind)
}

fn association_record_kind_for_row_rule(row_rule: &MapRowSemanticRule) -> RecordKind {
    match row_rule.source_operation_kind {
        SourceOperationKind::CloseAssociation
        | SourceOperationKind::ExpireAndCreate
        | SourceOperationKind::Correction => RecordKind::Delta,
        SourceOperationKind::TombstoneAssociation => RecordKind::Tombstone,
        SourceOperationKind::ReplaceObjectState => RecordKind::Snapshot,
        _ => RecordKind::Baseline,
    }
}

fn identity_equivalence_index(
    mapping_id: &str,
    mapping_version: &str,
    planned: &[PlannedIdentity],
) -> Value {
    let mut groups = BTreeMap::<String, Vec<&PlannedIdentity>>::new();
    for identity in planned {
        groups
            .entry(identity.equivalence_id.clone())
            .or_default()
            .push(identity);
    }
    let mut equivalences = Vec::new();
    let mut components = Vec::new();
    for (equivalence_id, mut members) in groups {
        members.sort_by_key(|member| {
            (
                member.canonical_anchor.clone(),
                member.identity_rule_id.clone(),
                member.source_id.clone(),
                member.row_index,
            )
        });
        let Some(anchor) = members.first().copied() else {
            continue;
        };
        for member in members.iter().skip(1) {
            equivalences.push(json!({
                "left_identity": anchor.identity_alias,
                "right_identity": member.identity_alias,
            }));
        }
        components.push(json!({
            "equivalence_id": equivalence_id,
            "goid": hex_encode(&anchor.goid),
            "canonical_anchor": anchor.canonical_anchor,
            "members": members.iter().map(|member| json!({
                "source_id": member.source_id,
                "row_index": member.row_index,
                "source_row_identity": member.source_row_identity,
                "row_rule_id": member.row_rule_id,
                "identity_rule_id": member.identity_rule_id,
                "identity_alias": member.identity_alias,
                "object_type": member.object_type,
                "join_key_sha256": member.join_key_sha256,
                "row_digest": member.row_digest,
            })).collect::<Vec<_>>(),
        }));
    }
    json!({
        "mapping_id": mapping_id,
        "mapping_version": mapping_version,
        "equivalences": equivalences,
        "components": components,
    })
}

fn record_id_for(source_id: &str, row_index: usize, rule_id: &str, goid: &[u8; 16]) -> [u8; 16] {
    let record_material = format!("{source_id}:{row_index}:{rule_id}:{}", hex_encode(goid));
    first_16(&sha256_array(record_material.as_bytes()))
}

fn association_goid(
    mapping_id: &[u8],
    mapping_version: &[u8],
    binding: &cove_core::profile::cove_map::MapAssociationBinding,
    source_goid: &[u8; 16],
    target_goid: &[u8; 16],
) -> [u8; 16] {
    let mut tuple = Vec::new();
    tuple.extend_from_slice(source_goid);
    tuple.extend_from_slice(target_goid);
    goid16_parts(&[
        mapping_id,
        mapping_version,
        format!("Association:{}", binding.association_type).as_bytes(),
        binding.assertion_id.as_bytes(),
        &tuple,
    ])
}

fn object_types_from_mapping(context: &MappingContext) -> Result<Vec<ObjectTypeEntryV1>, String> {
    let mut object_type_names = context
        .identity_rules
        .values()
        .map(|rule| rule.object_type.clone())
        .collect::<BTreeSet<_>>();
    for row_rule in &context.row_rules {
        for binding in &row_rule.association_bindings {
            object_type_names.insert(format!("Association:{}", binding.association_type));
        }
    }
    let mut out = Vec::new();
    for (index, type_name) in object_type_names.into_iter().enumerate() {
        let mut properties = Vec::new();
        let mut seen_properties = BTreeSet::new();
        for row_rule in &context.row_rules {
            let Some(identity_rule) = context.identity_rules.get(&row_rule.identity_rule_id) else {
                continue;
            };
            if identity_rule.object_type != type_name {
                continue;
            }
            for (property_index, binding) in row_rule.property_bindings.iter().enumerate() {
                let logical = logical_type_from_name(&binding.logical_type)?;
                let property_id = property_id_from_binding(binding, property_index as u32 + 1);
                if !seen_properties.insert(property_id) {
                    continue;
                }
                properties.push(PropertyEntryV1 {
                    property_id,
                    property_name: binding.property_name.clone(),
                    logical_type: logical,
                    physical_kind: physical_kind_from_binding(binding, logical)?,
                    nullable: binding.nullable,
                    collation_id: 0,
                    flags: 0,
                });
            }
        }
        if type_name.starts_with("Association:") {
            properties.extend(association_properties());
        }
        out.push(ObjectTypeEntryV1 {
            object_type_id: (index + 1) as u32,
            flags: if type_name.starts_with("Association:") {
                OBJECT_TYPE_FLAG_ASSOCIATION_OBJECT | OBJECT_TYPE_FLAG_LINK_OBJECT
            } else {
                OBJECT_TYPE_FLAG_ENTITY_OBJECT
            },
            type_name,
            properties,
        });
    }
    Ok(out)
}

fn property_id_from_binding(binding: &MapPropertyBinding, fallback: u32) -> u32 {
    stable_u32(&binding.property_id, fallback)
}

fn physical_kind_from_binding(
    binding: &MapPropertyBinding,
    logical: CoveLogicalType,
) -> Result<CovePhysicalKind, String> {
    match binding.physical_kind.as_str() {
        "auto" | "" => Ok(physical_for_logical(logical)),
        "boolean" | "bool" => Ok(CovePhysicalKind::Boolean),
        "filecode" | "file_code" => Ok(CovePhysicalKind::FileCode),
        "numcode" | "num_code" => Ok(CovePhysicalKind::NumCode),
        "fixedbytes" | "fixed_bytes" => Ok(CovePhysicalKind::FixedBytes),
        "varbytes" | "var_bytes" => Ok(CovePhysicalKind::VarBytes),
        other => Err(format!("unsupported MAP physical kind '{other}'")),
    }
}

fn association_properties() -> Vec<PropertyEntryV1> {
    vec![
        PropertyEntryV1 {
            property_id: 1,
            property_name: "source_goid".into(),
            logical_type: CoveLogicalType::Uuid,
            physical_kind: CovePhysicalKind::FixedBytes,
            nullable: false,
            collation_id: 0,
            flags: PROPERTY_FLAG_ASSOCIATION_FROM_GOID,
        },
        PropertyEntryV1 {
            property_id: 2,
            property_name: "target_goid".into(),
            logical_type: CoveLogicalType::Uuid,
            physical_kind: CovePhysicalKind::FixedBytes,
            nullable: false,
            collation_id: 0,
            flags: PROPERTY_FLAG_ASSOCIATION_TO_GOID,
        },
        PropertyEntryV1 {
            property_id: 3,
            property_name: "association_type".into(),
            logical_type: CoveLogicalType::Utf8,
            physical_kind: CovePhysicalKind::VarBytes,
            nullable: false,
            collation_id: 0,
            flags: PROPERTY_FLAG_ASSOCIATION_TYPE,
        },
        PropertyEntryV1 {
            property_id: 4,
            property_name: "mapping_rule_id".into(),
            logical_type: CoveLogicalType::Utf8,
            physical_kind: CovePhysicalKind::VarBytes,
            nullable: false,
            collation_id: 0,
            flags: PROPERTY_FLAG_MAPPING_RULE_REF,
        },
        PropertyEntryV1 {
            property_id: 5,
            property_name: "source_evidence_id".into(),
            logical_type: CoveLogicalType::Utf8,
            physical_kind: CovePhysicalKind::VarBytes,
            nullable: false,
            collation_id: 0,
            flags: PROPERTY_FLAG_EVIDENCE_REF,
        },
        PropertyEntryV1 {
            property_id: 6,
            property_name: "source_role".into(),
            logical_type: CoveLogicalType::Utf8,
            physical_kind: CovePhysicalKind::VarBytes,
            nullable: false,
            collation_id: 0,
            flags: 0,
        },
        PropertyEntryV1 {
            property_id: 7,
            property_name: "target_role".into(),
            logical_type: CoveLogicalType::Utf8,
            physical_kind: CovePhysicalKind::VarBytes,
            nullable: false,
            collation_id: 0,
            flags: 0,
        },
        PropertyEntryV1 {
            property_id: 8,
            property_name: "valid_from".into(),
            logical_type: CoveLogicalType::Json,
            physical_kind: CovePhysicalKind::VarBytes,
            nullable: true,
            collation_id: 0,
            flags: 0,
        },
        PropertyEntryV1 {
            property_id: 9,
            property_name: "valid_to".into(),
            logical_type: CoveLogicalType::Json,
            physical_kind: CovePhysicalKind::VarBytes,
            nullable: true,
            collation_id: 0,
            flags: 0,
        },
        PropertyEntryV1 {
            property_id: 10,
            property_name: "cardinality_policy".into(),
            logical_type: CoveLogicalType::Utf8,
            physical_kind: CovePhysicalKind::VarBytes,
            nullable: false,
            collation_id: 0,
            flags: 0,
        },
    ]
}

fn build_temporal_segments(
    materialized: &MaterializedModel,
    nested_shapes: &NestedShapeByProperty,
    dictionary: Option<&FileDictionaryEncoding>,
) -> Result<Vec<TemporalSegmentBuild>, String> {
    let mut grouped = BTreeMap::<u32, Vec<ObjectRow>>::new();
    for row in &materialized.rows {
        grouped
            .entry(row.object_type_id)
            .or_default()
            .push(row.clone());
    }
    let object_types = materialized
        .object_types
        .iter()
        .map(|ty| (ty.object_type_id, ty))
        .collect::<BTreeMap<_, _>>();
    let mut out = Vec::new();
    for (segment_index, (object_type_id, mut rows)) in grouped.into_iter().enumerate() {
        rows.sort_by_key(|row| (row.source_row_index, row.goid, row.record_id));
        let object_type = object_types
            .get(&object_type_id)
            .ok_or_else(|| format!("missing object_type_id {object_type_id}"))?;
        let segment_id = u32::try_from(segment_index)
            .map_err(|_| "too many COVE-O temporal segments".to_string())?;
        let payload =
            temporal_segment_payload(segment_id, object_type, &rows, nested_shapes, dictionary)?;
        out.push(TemporalSegmentBuild {
            segment_id,
            object_type_id,
            rows,
            payload,
        });
    }
    Ok(out)
}

fn temporal_segment_payload(
    segment_id: u32,
    object_type: &ObjectTypeEntryV1,
    rows: &[ObjectRow],
    nested_shapes: &NestedShapeByProperty,
    dictionary: Option<&FileDictionaryEncoding>,
) -> Result<Vec<u8>, String> {
    let row_count = u32::try_from(rows.len()).map_err(|_| "too many COVE-O rows".to_string())?;
    let row_directory_offset = TEMPORAL_SEGMENT_HEADER_LEN as u64;
    let row_bytes_len = rows
        .len()
        .checked_mul(TEMPORAL_ROW_ENTRY_LEN)
        .ok_or_else(|| "temporal row directory length overflow".to_string())?;
    let column_directory_offset = row_directory_offset
        .checked_add(row_bytes_len as u64)
        .ok_or_else(|| "temporal offset overflow".to_string())?;
    let column_count = u32::try_from(object_type.properties.len())
        .map_err(|_| "too many COVE-O property columns".to_string())?;
    let column_dir_len = object_type
        .properties
        .len()
        .checked_mul(TABLE_COLUMN_DIRECTORY_ENTRY_LEN)
        .ok_or_else(|| "temporal column directory length overflow".to_string())?;
    let page_index_offset = column_directory_offset
        .checked_add(column_dir_len as u64)
        .ok_or_else(|| "temporal offset overflow".to_string())?;
    let total_page_index_len = object_type
        .properties
        .len()
        .checked_mul(COLUMN_PAGE_INDEX_ENTRY_LEN)
        .ok_or_else(|| "temporal page index length overflow".to_string())?;
    let data_offset = page_index_offset
        .checked_add(total_page_index_len as u64)
        .ok_or_else(|| "temporal offset overflow".to_string())?;
    let header = TemporalSegmentHeaderV1 {
        segment_id,
        object_type_id: object_type.object_type_id,
        time_range_start_us: 0,
        time_range_end_us: 0,
        csn_min: 0,
        csn_max: rows.len().saturating_sub(1) as u64,
        row_count,
        morsel_count: if row_count == 0 { 0 } else { 1 },
        morsel_row_count: if row_count == 0 { 0 } else { row_count },
        column_count,
        row_directory_offset,
        column_directory_offset,
        page_index_offset,
        data_offset,
        flags: 0,
        checksum: 0,
    };
    let mut out = header.serialize().to_vec();
    let prev_refs = temporal_prev_refs(segment_id, rows);
    for (index, row) in rows.iter().enumerate() {
        out.extend_from_slice(
            &TemporalRowEntryV1 {
                timestamp_us: 0,
                csn: index as u64,
                branch_key: 0,
                goid: row.goid,
                record_id: row.record_id,
                record_kind: row.record_kind,
                prev_ref: prev_refs[index],
            }
            .serialize(),
        );
    }
    let mut column_directory = Vec::new();
    let mut page_index_bytes = Vec::new();
    let mut page_payload_bytes = Vec::new();
    let mut next_page_index_offset = page_index_offset;
    let mut next_data_offset = data_offset;
    for property in &object_type.properties {
        let column_page_index_offset = next_page_index_offset;
        let column_data_offset = next_data_offset;
        let page_payload = build_property_page_payload(
            object_type.object_type_id,
            property,
            rows,
            nested_shapes,
            dictionary,
        )?;
        let page_length = page_payload.len() as u64;
        let page_checksum = checksum::crc32c(&page_payload);
        let null_count = rows
            .iter()
            .filter(|row| {
                row.properties
                    .get(&property.property_id)
                    .is_none_or(|value| value.value.is_null())
            })
            .count() as u32;
        let page = ColumnPageIndexEntryV1 {
            column_id: property.property_id,
            morsel_id: 0,
            row_count,
            non_null_count: row_count.saturating_sub(null_count),
            null_count,
            encoding_root: encoding_for_physical(property.physical_kind) as u32,
            page_offset: next_data_offset,
            page_length,
            uncompressed_length: page_length,
            stats_ref: 0,
            flags: CompressionCodec::None as u32,
            checksum: page_checksum,
        };
        page_index_bytes.extend_from_slice(&page.serialize());
        page_payload_bytes.extend_from_slice(&page_payload);
        next_page_index_offset = next_page_index_offset
            .checked_add(COLUMN_PAGE_INDEX_ENTRY_LEN as u64)
            .ok_or_else(|| "temporal page index offset overflow".to_string())?;
        next_data_offset = next_data_offset
            .checked_add(page_length)
            .ok_or_else(|| "temporal data offset overflow".to_string())?;
        column_directory.push(TableColumnDirectoryEntryV1 {
            column_id: property.property_id,
            logical_type: property.logical_type,
            physical_kind: property.physical_kind,
            flags: 0,
            page_index_offset: column_page_index_offset,
            page_index_length: COLUMN_PAGE_INDEX_ENTRY_LEN as u64,
            data_offset: column_data_offset,
            data_length: next_data_offset - column_data_offset,
            stats_ref: 0,
            domain_ref: 0,
            checksum: 0,
        });
    }
    for entry in &column_directory {
        out.extend_from_slice(&entry.serialize());
    }
    out.extend_from_slice(&page_index_bytes);
    out.extend_from_slice(&page_payload_bytes);
    Ok(out)
}

fn build_property_page_payload(
    object_type_id: u32,
    property: &PropertyEntryV1,
    rows: &[ObjectRow],
    nested_shapes: &NestedShapeByProperty,
    dictionary: Option<&FileDictionaryEncoding>,
) -> Result<Vec<u8>, String> {
    let row_count = u32::try_from(rows.len()).map_err(|_| "too many rows".to_string())?;
    let mut null_bitmap = vec![0u8; rows.len().div_ceil(8)];
    let mut values = Vec::new();
    let mut null_count = 0usize;
    for (row_index, row) in rows.iter().enumerate() {
        let value = row
            .properties
            .get(&property.property_id)
            .map(|property| &property.value)
            .unwrap_or(&Value::Null);
        if value.is_null() {
            null_count += 1;
            null_bitmap[row_index / 8] |= 1u8 << (row_index % 8);
        }
        append_property_value_bytes(
            property,
            value,
            nested_shapes.get(&(object_type_id, property.property_id)),
            dictionary,
            &mut values,
        )?;
    }
    ColumnPagePayloadV1::build_single_node(
        row_count,
        encoding_for_physical(property.physical_kind),
        property.logical_type,
        property.physical_kind,
        (null_count != 0).then_some(null_bitmap),
        values,
    )
    .map_err(|err| err.to_string())
}

fn nested_shapes_for_model(
    file: &CovemapFile,
    materialized: &MaterializedModel,
) -> Result<NestedShapeByProperty, String> {
    let mut out = NestedShapeByProperty::new();
    let object_types_by_name = materialized
        .object_types
        .iter()
        .map(|object_type| (object_type.type_name.as_str(), object_type))
        .collect::<BTreeMap<_, _>>();
    for section in embedded_sections(file)? {
        let cove_core::profile::cove_map::EmbeddedMapSection::ProjectionCatalog(catalog) = section
        else {
            continue;
        };
        for projection in catalog.projections {
            let output_table = projection
                .output_table
                .as_deref()
                .unwrap_or(&projection.projection_id);
            let Some(object_type) = object_types_by_name.get(output_table) else {
                continue;
            };
            let properties_by_name = object_type
                .properties
                .iter()
                .map(|property| (property.property_name.as_str(), property))
                .collect::<BTreeMap<_, _>>();
            for column in projection.columns {
                let Some(shape) = column.nested_shape.as_deref() else {
                    continue;
                };
                let Some(property) = properties_by_name.get(column.name.as_str()) else {
                    continue;
                };
                let shape_value: Value = serde_json::from_str(shape).map_err(|err| {
                    format!(
                        "projection column '{}' has invalid nested_shape JSON: {err}",
                        column.name
                    )
                })?;
                let mut node =
                    project::nested_schema_node_from_shape(&column.name, &shape_value, true)?;
                node.name = column.name.clone();
                node.logical = property.logical_type;
                node.physical = physical_for_logical(property.logical_type);
                out.insert((object_type.object_type_id, property.property_id), node);
            }
        }
    }
    Ok(out)
}

fn file_dictionary_for_model(
    materialized: &MaterializedModel,
    nested_shapes: &NestedShapeByProperty,
) -> Result<Option<FileDictionaryEncoding>, String> {
    let mut keys = BTreeSet::<FileDictionaryKey>::new();
    let properties_by_type = materialized
        .object_types
        .iter()
        .flat_map(|object_type| {
            object_type
                .properties
                .iter()
                .map(move |property| ((object_type.object_type_id, property.property_id), property))
        })
        .collect::<BTreeMap<_, _>>();
    for row in &materialized.rows {
        for (property_id, property_value) in &row.properties {
            let Some(property) = properties_by_type.get(&(row.object_type_id, *property_id)) else {
                continue;
            };
            if property.physical_kind != CovePhysicalKind::FileCode
                || property_value.value.is_null()
            {
                continue;
            }
            keys.insert(file_dictionary_key_for_property(
                property.logical_type,
                &property_value.value,
                nested_shapes.get(&(row.object_type_id, *property_id)),
            )?);
        }
    }
    if keys.is_empty() {
        return Ok(None);
    }
    FileDictionaryEncoding::from_keys(keys)
        .map(Some)
        .map_err(|err| err.to_string())
}

fn file_dictionary_index_bytes(dictionary: &FileDictionary) -> Vec<u8> {
    let mut out = Vec::with_capacity(
        cove_core::dictionary::DICT_HEADER_SIZE
            + dictionary.entries.len() * cove_core::dictionary::DICT_INDEX_ENTRY_SIZE,
    );
    out.extend_from_slice(&dictionary.header.serialize());
    for entry in &dictionary.entries {
        out.extend_from_slice(&entry.serialize());
    }
    out
}

fn file_dictionary_key_for_property(
    logical: CoveLogicalType,
    value: &Value,
    nested_shape: Option<&NestedSchemaNodeV1>,
) -> Result<FileDictionaryKey, String> {
    if logical == CoveLogicalType::Json {
        let text = serde_json::to_string(value).map_err(|err| err.to_string())?;
        let canonical = CanonicalValue::Json(&text);
        return Ok(FileDictionaryKey {
            value_tag: canonical.value_tag() as u16,
            canonical: canonical.encode().map_err(|err| err.to_string())?,
        });
    }
    let canonical = canonical_value_for_logical(logical, value, nested_shape)?;
    let value_tag = canonical.value_tag() as u16;
    let canonical = canonical.encode().map_err(|err| err.to_string())?;
    Ok(FileDictionaryKey {
        value_tag,
        canonical,
    })
}

fn canonical_value_for_logical<'a>(
    logical: CoveLogicalType,
    value: &'a Value,
    nested_shape: Option<&NestedSchemaNodeV1>,
) -> Result<CanonicalValue<'a>, String> {
    if value.is_null() {
        return Ok(CanonicalValue::Null);
    }
    match logical {
        CoveLogicalType::Null => Ok(CanonicalValue::Null),
        CoveLogicalType::Bool => Ok(CanonicalValue::Bool(json_bool(value)?)),
        CoveLogicalType::Int8 => Ok(CanonicalValue::Int {
            width: 1,
            value: i128::from(json_i64(value)?),
        }),
        CoveLogicalType::Int16 => Ok(CanonicalValue::Int {
            width: 2,
            value: i128::from(json_i64(value)?),
        }),
        CoveLogicalType::Int32 => Ok(CanonicalValue::Int {
            width: 4,
            value: i128::from(json_i64(value)?),
        }),
        CoveLogicalType::Int64 => Ok(CanonicalValue::Int {
            width: 8,
            value: i128::from(json_i64(value)?),
        }),
        CoveLogicalType::UInt8 => Ok(CanonicalValue::Uint {
            width: 1,
            value: u128::from(json_u64(value)?),
        }),
        CoveLogicalType::UInt16 => Ok(CanonicalValue::Uint {
            width: 2,
            value: u128::from(json_u64(value)?),
        }),
        CoveLogicalType::UInt32 => Ok(CanonicalValue::Uint {
            width: 4,
            value: u128::from(json_u64(value)?),
        }),
        CoveLogicalType::UInt64 => Ok(CanonicalValue::Uint {
            width: 8,
            value: u128::from(json_u64(value)?),
        }),
        CoveLogicalType::Float32 => Ok(CanonicalValue::Float32(json_f64(value)? as f32)),
        CoveLogicalType::Float64 => Ok(CanonicalValue::Float64(json_f64(value)?)),
        CoveLogicalType::Decimal64 => Ok(CanonicalValue::Decimal64(json_i64(value)?)),
        CoveLogicalType::Decimal128 => Ok(CanonicalValue::Decimal128(json_i128(value)?)),
        CoveLogicalType::DateDays => Ok(CanonicalValue::DateDays(
            json_i64(value)?
                .try_into()
                .map_err(|_| "date_days out of i32 range".to_string())?,
        )),
        CoveLogicalType::TimestampMicros => Ok(CanonicalValue::TimestampMicros(json_i64(value)?)),
        CoveLogicalType::TimestampNanos => Ok(CanonicalValue::TimestampNanos(json_i64(value)?)),
        CoveLogicalType::Utf8 => Ok(CanonicalValue::Utf8(json_string(value)?)),
        CoveLogicalType::Binary => Ok(CanonicalValue::Bytes(json_string(value)?.as_bytes())),
        CoveLogicalType::Uuid => Ok(CanonicalValue::Uuid(json_uuid(value)?)),
        CoveLogicalType::Json => unreachable!("JSON is handled before borrowing conversion"),
        CoveLogicalType::List | CoveLogicalType::Struct | CoveLogicalType::Map => {
            if let Some(shape) = nested_shape {
                canonical_value_for_nested_shape(shape, value)
            } else {
                match logical {
                    CoveLogicalType::List => canonical_list_value(value),
                    CoveLogicalType::Struct => canonical_struct_value(value),
                    CoveLogicalType::Map => canonical_map_value(value),
                    _ => unreachable!(),
                }
            }
        }
        _ => Err("unsupported future logical type for FileCode dictionary".into()),
    }
}

fn canonical_value_for_nested_shape<'a>(
    shape: &NestedSchemaNodeV1,
    value: &'a Value,
) -> Result<CanonicalValue<'a>, String> {
    if value.is_null() {
        return Ok(CanonicalValue::Null);
    }
    match shape.logical {
        CoveLogicalType::List => {
            let item_shape = shape
                .children
                .first()
                .ok_or_else(|| "list nested_shape requires one child".to_string())?;
            let items = value
                .as_array()
                .ok_or_else(|| "list property value must be an array".to_string())?
                .iter()
                .map(|item| canonical_value_for_nested_shape(item_shape, item))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(CanonicalValue::List(items))
        }
        CoveLogicalType::Struct => {
            let object = value
                .as_object()
                .ok_or_else(|| "struct property value must be an object".to_string())?;
            let mut fields = Vec::with_capacity(shape.children.len());
            for (index, child) in shape.children.iter().enumerate() {
                let child_value = object.get(&child.name).unwrap_or(&Value::Null);
                fields.push(CanonicalField {
                    field_id: stable_u32(&child.name, index as u32 + 1) as u64,
                    value: canonical_value_for_nested_shape(child, child_value)?,
                });
            }
            Ok(CanonicalValue::Struct(fields))
        }
        CoveLogicalType::Map => {
            if shape.children.len() != 2 {
                return Err("map nested_shape requires key and value children".into());
            }
            let key_shape = &shape.children[0];
            let value_shape = &shape.children[1];
            let mut entries = Vec::new();
            match value {
                Value::Object(object) => {
                    for (key, value) in object {
                        entries.push((
                            canonical_map_object_key_for_shape(key_shape, key)?,
                            canonical_value_for_nested_shape(value_shape, value)?,
                        ));
                    }
                }
                Value::Array(items) => {
                    for item in items {
                        let pair = item.as_array().ok_or_else(|| {
                            "map array entries must be [key, value] pairs".to_string()
                        })?;
                        if pair.len() != 2 {
                            return Err("map array entries must be [key, value] pairs".into());
                        }
                        entries.push((
                            canonical_value_for_nested_shape(key_shape, &pair[0])?,
                            canonical_value_for_nested_shape(value_shape, &pair[1])?,
                        ));
                    }
                }
                _ => return Err("map property value must be an object or pair array".into()),
            }
            Ok(CanonicalValue::Map(entries))
        }
        _ => canonical_value_for_logical(shape.logical, value, None),
    }
}

fn canonical_map_object_key_for_shape<'a>(
    shape: &NestedSchemaNodeV1,
    key: &'a str,
) -> Result<CanonicalValue<'a>, String> {
    match shape.logical {
        CoveLogicalType::Bool => match key {
            "true" => Ok(CanonicalValue::Bool(true)),
            "false" => Ok(CanonicalValue::Bool(false)),
            _ => Err("map object key is not a boolean".into()),
        },
        CoveLogicalType::Int8 => Ok(CanonicalValue::Int {
            width: 1,
            value: key
                .parse::<i8>()
                .map(i128::from)
                .map_err(|_| "map object key is not an int8".to_string())?,
        }),
        CoveLogicalType::Int16 => Ok(CanonicalValue::Int {
            width: 2,
            value: key
                .parse::<i16>()
                .map(i128::from)
                .map_err(|_| "map object key is not an int16".to_string())?,
        }),
        CoveLogicalType::Int32 => Ok(CanonicalValue::Int {
            width: 4,
            value: key
                .parse::<i32>()
                .map(i128::from)
                .map_err(|_| "map object key is not an int32".to_string())?,
        }),
        CoveLogicalType::Int64 => Ok(CanonicalValue::Int {
            width: 8,
            value: key
                .parse::<i64>()
                .map(i128::from)
                .map_err(|_| "map object key is not an int64".to_string())?,
        }),
        CoveLogicalType::UInt8 => Ok(CanonicalValue::Uint {
            width: 1,
            value: key
                .parse::<u8>()
                .map(u128::from)
                .map_err(|_| "map object key is not a uint8".to_string())?,
        }),
        CoveLogicalType::UInt16 => Ok(CanonicalValue::Uint {
            width: 2,
            value: key
                .parse::<u16>()
                .map(u128::from)
                .map_err(|_| "map object key is not a uint16".to_string())?,
        }),
        CoveLogicalType::UInt32 => Ok(CanonicalValue::Uint {
            width: 4,
            value: key
                .parse::<u32>()
                .map(u128::from)
                .map_err(|_| "map object key is not a uint32".to_string())?,
        }),
        CoveLogicalType::UInt64 => Ok(CanonicalValue::Uint {
            width: 8,
            value: key
                .parse::<u64>()
                .map(u128::from)
                .map_err(|_| "map object key is not a uint64".to_string())?,
        }),
        CoveLogicalType::Float32 => Ok(CanonicalValue::Float32(
            key.parse::<f32>()
                .map_err(|_| "map object key is not a float32".to_string())?,
        )),
        CoveLogicalType::Float64 => Ok(CanonicalValue::Float64(
            key.parse::<f64>()
                .map_err(|_| "map object key is not a float64".to_string())?,
        )),
        CoveLogicalType::Decimal64 => Ok(CanonicalValue::Decimal64(
            key.parse::<i64>()
                .map_err(|_| "map object key is not a decimal64".to_string())?,
        )),
        CoveLogicalType::Decimal128 => Ok(CanonicalValue::Decimal128(
            key.parse::<i128>()
                .map_err(|_| "map object key is not a decimal128".to_string())?,
        )),
        CoveLogicalType::DateDays => Ok(CanonicalValue::DateDays(
            key.parse::<i32>()
                .map_err(|_| "map object key is not a date_days".to_string())?,
        )),
        CoveLogicalType::TimestampMicros => Ok(CanonicalValue::TimestampMicros(
            key.parse::<i64>()
                .map_err(|_| "map object key is not a timestamp_micros".to_string())?,
        )),
        CoveLogicalType::TimestampNanos => Ok(CanonicalValue::TimestampNanos(
            key.parse::<i64>()
                .map_err(|_| "map object key is not a timestamp_nanos".to_string())?,
        )),
        CoveLogicalType::Utf8 => Ok(CanonicalValue::Utf8(key)),
        CoveLogicalType::Binary => Ok(CanonicalValue::Bytes(key.as_bytes())),
        CoveLogicalType::Json => Ok(CanonicalValue::Json(key)),
        CoveLogicalType::Uuid => Ok(CanonicalValue::Uuid(hex_decode_16(key)?)),
        CoveLogicalType::List | CoveLogicalType::Struct | CoveLogicalType::Map => {
            Err("map object keys cannot use nested logical types".into())
        }
        _ => Err("unsupported future map key logical type".into()),
    }
}

fn canonical_value_from_json<'a>(value: &'a Value) -> Result<CanonicalValue<'a>, String> {
    match value {
        Value::Null => Ok(CanonicalValue::Null),
        Value::Bool(value) => Ok(CanonicalValue::Bool(*value)),
        Value::Number(number) => {
            if let Some(value) = number.as_i64() {
                Ok(CanonicalValue::Int {
                    width: 8,
                    value: i128::from(value),
                })
            } else if let Some(value) = number.as_u64() {
                Ok(CanonicalValue::Uint {
                    width: 8,
                    value: u128::from(value),
                })
            } else {
                Ok(CanonicalValue::Float64(
                    number
                        .as_f64()
                        .ok_or_else(|| "non-finite JSON number".to_string())?,
                ))
            }
        }
        Value::String(value) => Ok(CanonicalValue::Utf8(value)),
        Value::Array(_) => canonical_list_value(value),
        Value::Object(_) => canonical_struct_value(value),
    }
}

fn canonical_list_value<'a>(value: &'a Value) -> Result<CanonicalValue<'a>, String> {
    let items = value
        .as_array()
        .ok_or_else(|| "list property value must be an array".to_string())?
        .iter()
        .map(canonical_value_from_json)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(CanonicalValue::List(items))
}

fn canonical_struct_value<'a>(value: &'a Value) -> Result<CanonicalValue<'a>, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "struct property value must be an object".to_string())?;
    let mut fields = Vec::with_capacity(object.len());
    for (index, (name, value)) in object.iter().enumerate() {
        fields.push(CanonicalField {
            field_id: stable_u32(name, index as u32 + 1) as u64,
            value: canonical_value_from_json(value)?,
        });
    }
    fields.sort_by_key(|field| field.field_id);
    Ok(CanonicalValue::Struct(fields))
}

fn canonical_map_value<'a>(value: &'a Value) -> Result<CanonicalValue<'a>, String> {
    let mut entries = Vec::new();
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                entries.push((CanonicalValue::Utf8(key), canonical_value_from_json(value)?));
            }
        }
        Value::Array(items) => {
            for item in items {
                let pair = item
                    .as_array()
                    .ok_or_else(|| "map array entries must be [key, value] pairs".to_string())?;
                if pair.len() != 2 {
                    return Err("map array entries must be [key, value] pairs".into());
                }
                entries.push((
                    canonical_value_from_json(&pair[0])?,
                    canonical_value_from_json(&pair[1])?,
                ));
            }
        }
        _ => return Err("map property value must be an object or pair array".into()),
    }
    Ok(CanonicalValue::Map(entries))
}

fn append_property_value_bytes(
    property: &PropertyEntryV1,
    value: &Value,
    nested_shape: Option<&NestedSchemaNodeV1>,
    dictionary: Option<&FileDictionaryEncoding>,
    out: &mut Vec<u8>,
) -> Result<(), String> {
    if value.is_null() {
        append_null_placeholder(property, out)?;
        return Ok(());
    }
    match property.physical_kind {
        CovePhysicalKind::Boolean => out.push(if json_bool(value)? { 1 } else { 0 }),
        CovePhysicalKind::NumCode => out.extend_from_slice(&json_numcode(value)?.to_le_bytes()),
        CovePhysicalKind::FixedBytes => {
            let bytes = fixed_bytes_for_property(property, value)?;
            out.extend_from_slice(&bytes);
        }
        CovePhysicalKind::VarBytes => {
            let bytes = var_bytes_for_property(property, value)?;
            let len = u32::try_from(bytes.len())
                .map_err(|_| "property value is too large".to_string())?;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&bytes);
        }
        CovePhysicalKind::FileCode => {
            let dictionary = dictionary.ok_or_else(|| {
                "COVE-MAP writer needs a file dictionary for FileCode properties".to_string()
            })?;
            let key = file_dictionary_key_for_property(property.logical_type, value, nested_shape)?;
            let code = dictionary
                .file_code_for_key(&key)
                .map_err(|err| err.to_string())?;
            out.extend_from_slice(&code.to_le_bytes());
        }
        CovePhysicalKind::List | CovePhysicalKind::Struct | CovePhysicalKind::Map => {
            return Err("COVE-MAP writer does not materialize nested properties yet".into())
        }
        _ => return Err("unsupported future COVE physical kind".into()),
    }
    Ok(())
}

fn append_null_placeholder(property: &PropertyEntryV1, out: &mut Vec<u8>) -> Result<(), String> {
    match property.physical_kind {
        CovePhysicalKind::Boolean => out.push(0),
        CovePhysicalKind::NumCode => out.extend_from_slice(&0u64.to_le_bytes()),
        CovePhysicalKind::FixedBytes => {
            let width = match property.logical_type {
                CoveLogicalType::Uuid | CoveLogicalType::Decimal128 => 16,
                CoveLogicalType::Decimal64 => 8,
                _ => return Err("unsupported fixed-width null placeholder".into()),
            };
            out.resize(out.len() + width, 0);
        }
        CovePhysicalKind::VarBytes => out.extend_from_slice(&0u32.to_le_bytes()),
        CovePhysicalKind::FileCode => out.extend_from_slice(&0u32.to_le_bytes()),
        CovePhysicalKind::List | CovePhysicalKind::Struct | CovePhysicalKind::Map => {
            return Err("nested null placeholders are not supported".into())
        }
        _ => return Err("unsupported future COVE physical kind".into()),
    }
    Ok(())
}

fn temporal_segment_index(
    segments: &[TemporalSegmentBuild],
) -> Result<TemporalSegmentIndex, String> {
    let mut entries = Vec::with_capacity(segments.len());
    for segment in segments {
        let min_goid = segment
            .rows
            .iter()
            .map(|row| row.goid)
            .min()
            .unwrap_or([0; 16]);
        let max_goid = segment
            .rows
            .iter()
            .map(|row| row.goid)
            .max()
            .unwrap_or([0; 16]);
        let (delta_count, snapshot_count, baseline_count, tombstone_count) =
            row_kind_counts(&segment.rows);
        entries.push(TemporalSegmentIndexEntryV1 {
            segment_id: segment.segment_id,
            object_type_id: segment.object_type_id,
            time_range_start_us: 0,
            time_range_end_us: 0,
            csn_min: 0,
            csn_max: segment.rows.len().saturating_sub(1) as u64,
            row_count: u32::try_from(segment.rows.len())
                .map_err(|_| "too many COVE-O rows".to_string())?,
            delta_count,
            snapshot_count,
            baseline_count,
            tombstone_count,
            min_goid,
            max_goid,
            offset: 0,
            length: segment.payload.len() as u64,
            checksum: 0,
        });
    }
    Ok(TemporalSegmentIndex { flags: 0, entries })
}

fn row_kind_counts(rows: &[ObjectRow]) -> (u32, u32, u32, u32) {
    let mut delta = 0;
    let mut snapshot = 0;
    let mut baseline = 0;
    let mut tombstone = 0;
    for row in rows {
        match row.record_kind {
            RecordKind::Delta => delta += 1,
            RecordKind::Snapshot => snapshot += 1,
            RecordKind::Baseline => baseline += 1,
            RecordKind::Tombstone => tombstone += 1,
            RecordKind::ReservedLegacyMaterializedDelta => {}
            _ => {}
        }
    }
    (delta, snapshot, baseline, tombstone)
}

fn temporal_prev_refs(segment_id: u32, rows: &[ObjectRow]) -> Vec<Option<CoveRecordRefV1>> {
    let mut latest_by_goid = BTreeMap::<[u8; 16], u32>::new();
    let mut refs = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let prev_ref = if matches!(
            row.record_kind,
            RecordKind::Delta | RecordKind::Snapshot | RecordKind::Tombstone
        ) {
            latest_by_goid
                .get(&row.goid)
                .copied()
                .map(|row_index| CoveRecordRefV1 {
                    segment_id,
                    row_index,
                    target_kind: 0,
                })
        } else {
            None
        };
        refs.push(prev_ref);
        latest_by_goid.insert(row.goid, index as u32);
    }
    refs
}

fn trust_manifest(segments: &[TemporalSegmentBuild]) -> Result<TrustManifest, String> {
    let mut previous = [0u8; 32];
    let mut entries = Vec::new();
    for segment in segments {
        let prev_refs = temporal_prev_refs(segment.segment_id, &segment.rows);
        for (index, row) in segment.rows.iter().enumerate() {
            let temporal_row = TemporalRowEntryV1 {
                timestamp_us: 0,
                csn: index as u64,
                branch_key: 0,
                goid: row.goid,
                record_id: row.record_id,
                record_kind: row.record_kind,
                prev_ref: prev_refs[index],
            };
            let expected_hash = trust_chain::chain(&previous, &temporal_row.trust_payload())
                .map_err(|err| err.to_string())?;
            entries.push(TrustManifestEntryV1 {
                segment_id: segment.segment_id,
                row_index: index as u32,
                expected_hash,
            });
            previous = expected_hash;
        }
    }
    Ok(TrustManifest { entries })
}

fn object_section(
    kind: SectionKind,
    item_count: u64,
    row_count: u64,
    data: Vec<u8>,
) -> SectionPayload {
    SectionPayload {
        section_kind: kind as u16,
        profile: PrimaryProfile::ObjectTemporal as u8,
        flags: 0,
        item_count,
        row_count,
        compression: 0,
        alignment_log2: 0,
        required_features: 0,
        optional_features: 0,
        data,
    }
}

fn dictionary_section(kind: SectionKind, item_count: u64, data: Vec<u8>) -> SectionPayload {
    SectionPayload {
        section_kind: kind as u16,
        profile: PrimaryProfile::Mixed as u8,
        flags: 0,
        item_count,
        row_count: 0,
        compression: 0,
        alignment_log2: 0,
        required_features: FEATURE_FILE_DICTIONARY,
        optional_features: 0,
        data,
    }
}

fn map_section(kind: SectionKind, item_count: u64, data: Vec<u8>) -> SectionPayload {
    SectionPayload {
        section_kind: kind as u16,
        profile: PrimaryProfile::SemanticMapping as u8,
        flags: 0,
        item_count,
        row_count: 0,
        compression: 0,
        alignment_log2: 0,
        required_features: 0,
        optional_features: FEATURE_SEMANTIC_MAP,
        data: ensure_covemap_payload_envelope(kind, data),
    }
}

fn ensure_covemap_payload_envelope(kind: SectionKind, data: Vec<u8>) -> Vec<u8> {
    let Ok(mut value) = serde_json::from_slice::<Value>(&data) else {
        return data;
    };
    let Value::Object(object) = &mut value else {
        return data;
    };
    object.insert(
        "schema_id".to_string(),
        Value::String("org.coveformat.covemap.v2".to_string()),
    );
    object.insert(
        "section_id".to_string(),
        Value::Number(serde_json::Number::from(kind as u16)),
    );
    serde_json::to_vec_pretty(&value).unwrap_or(data)
}

fn map_passthrough_sections(file: &CovemapFile) -> Vec<SectionPayload> {
    file.sections
        .iter()
        .filter_map(|section| {
            let kind = u16::try_from(section.entry.section_id)
                .ok()
                .and_then(SectionKind::from_u16)?;
            matches!(
                kind,
                SectionKind::MapSourceCatalog
                    | SectionKind::MapFunctionRegistry
                    | SectionKind::MapIdentityRuleCatalog
                    | SectionKind::MapRowSemanticsCatalog
                    | SectionKind::MapProjectionCatalog
            )
            .then(|| map_section(kind, 1, section.payload.clone()))
        })
        .collect()
}

#[derive(Debug, Clone, Copy)]
struct JoinKeyComponent<'a> {
    role_id: &'a str,
    logical_type_id: &'a str,
    value: Option<&'a [u8]>,
}

fn join_key_tuple(
    object_type_id: u32,
    identity_rule_id: &str,
    components: &[JoinKeyComponent<'_>],
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"COVE-MAP-JOIN-KEY-V1");
    out.extend_from_slice(&object_type_id.to_le_bytes());
    append_len_bytes(&mut out, identity_rule_id.as_bytes());
    out.extend_from_slice(&(components.len() as u32).to_le_bytes());
    for component in components {
        append_len_bytes(&mut out, component.role_id.as_bytes());
        append_len_bytes(&mut out, component.logical_type_id.as_bytes());
        match component.value {
            None => out.push(0),
            Some(value) => {
                out.push(1);
                append_len_bytes(&mut out, value);
            }
        }
    }
    out
}

fn join_key_tuple_from_rule(
    rule: &MapIdentityRule,
    row: &SourceRow,
    object_type_id: u32,
) -> Result<Vec<u8>, String> {
    let mut encoded_values = Vec::<Option<Vec<u8>>>::with_capacity(rule.join_keys.len());
    for component in &rule.join_keys {
        let raw_value = row.values.get(&component.source_column);
        if raw_value.is_none() || matches!(raw_value, Some(Value::Null)) {
            if matches!(
                component.null_policy.as_str(),
                "reject" | "reject-null" | "all_components_required"
            ) {
                return Err(format!(
                    "identity rule '{}' rejected null/missing source column '{}'",
                    rule.rule_id, component.source_column
                ));
            }
            encoded_values.push(None);
            continue;
        }
        let value = apply_canonicalization(
            raw_value.unwrap(),
            &component.canonicalization,
            &rule.function_ids,
        )?;
        encoded_values.push(Some(canonical_component_bytes(
            &component.logical_type,
            &value,
        )?));
    }
    let components = rule
        .join_keys
        .iter()
        .zip(encoded_values.iter())
        .map(|(component, bytes)| JoinKeyComponent {
            role_id: component.role_id.as_str(),
            logical_type_id: component.logical_type.as_str(),
            value: bytes.as_deref(),
        })
        .collect::<Vec<_>>();
    Ok(join_key_tuple(object_type_id, &rule.rule_id, &components))
}

fn apply_canonicalization(
    value: &Value,
    canonicalization: &str,
    declared_functions: &[String],
) -> Result<Value, String> {
    let function_id = if canonicalization == "none" {
        "identity"
    } else {
        canonicalization
    };
    if !declared_functions
        .iter()
        .any(|function| function == function_id || function == canonicalization)
    {
        return Err(format!(
            "canonicalization function '{canonicalization}' was not declared on the identity rule"
        ));
    }
    if !deterministic_builtin_function_ids().contains(&function_id) {
        return Err(format!(
            "canonicalization function '{canonicalization}' is not implemented by the deterministic reference runner"
        ));
    }
    match function_id {
        "identity" => Ok(value.clone()),
        "trim" => Ok(Value::String(string_arg(value, "trim")?.trim().to_string())),
        "ascii_lower" => Ok(Value::String(
            string_arg(value, "ascii_lower")?.to_ascii_lowercase(),
        )),
        "unicode_nfc" => {
            let text = string_arg(value, function_id)?;
            let normalizer = icu_normalizer::ComposingNormalizerBorrowed::new_nfc();
            Ok(Value::String(normalizer.normalize(text).into_owned()))
        }
        "unicode_nfkc" => {
            let text = string_arg(value, function_id)?;
            let normalizer = icu_normalizer::ComposingNormalizerBorrowed::new_nfkc();
            Ok(Value::String(normalizer.normalize(text).into_owned()))
        }
        "unicode_casefold" => {
            let case_mapper = icu_casemap::CaseMapper::new();
            Ok(Value::String(
                case_mapper
                    .fold_string(string_arg(value, "unicode_casefold")?)
                    .into_owned(),
            ))
        }
        "trim_lower" => Ok(Value::String(
            string_arg(value, "trim_lower")?.trim().to_ascii_lowercase(),
        )),
        "concat_delimited" => {
            let items = value
                .as_array()
                .ok_or_else(|| "concat_delimited requires a JSON array".to_string())?;
            let mut out = Vec::new();
            for item in items {
                out.push(string_arg(item, "concat_delimited")?);
            }
            Ok(Value::String(out.join("|")))
        }
        "parse_int64" => {
            let text = string_arg(value, "parse_int64")?.trim();
            let parsed = text
                .parse::<i64>()
                .map_err(|_| "parse_int64 requires a base-10 int64 string".to_string())?;
            Ok(Value::Number(parsed.into()))
        }
        "parse_decimal" => {
            let text = string_arg(value, "parse_decimal")?.trim();
            validate_decimal_text(text)?;
            Ok(Value::String(text.to_string()))
        }
        "parse_timestamp_utc" => {
            let text = string_arg(value, "parse_timestamp_utc")?.trim();
            validate_utc_timestamp_text(text)?;
            Ok(Value::String(text.to_string()))
        }
        "sha256_hex" => Ok(Value::String(sha256_hex(
            string_arg(value, "sha256_hex")?.as_bytes(),
        ))),
        _ => unreachable!("registry membership checked above"),
    }
}

fn deterministic_builtin_function_ids() -> &'static [&'static str] {
    &[
        "identity",
        "trim",
        "ascii_lower",
        "unicode_nfc",
        "unicode_nfkc",
        "unicode_casefold",
        "trim_lower",
        "concat_delimited",
        "parse_int64",
        "parse_decimal",
        "parse_timestamp_utc",
        "sha256_hex",
    ]
}

fn string_arg<'a>(value: &'a Value, function_id: &str) -> Result<&'a str, String> {
    value
        .as_str()
        .ok_or_else(|| format!("{function_id} requires a string value"))
}

fn validate_decimal_text(text: &str) -> Result<(), String> {
    if text.is_empty() {
        return Err("parse_decimal requires a non-empty decimal string".into());
    }
    let mut chars = text.chars();
    if matches!(chars.clone().next(), Some('+') | Some('-')) {
        chars.next();
    }
    let mut digits = 0usize;
    let mut dots = 0usize;
    for ch in chars {
        match ch {
            '0'..='9' => digits += 1,
            '.' => dots += 1,
            _ => return Err("parse_decimal only accepts base-10 decimal text".into()),
        }
    }
    if digits == 0 || dots > 1 {
        return Err("parse_decimal only accepts base-10 decimal text".into());
    }
    Ok(())
}

fn validate_utc_timestamp_text(text: &str) -> Result<(), String> {
    let has_utc_suffix = text.ends_with('Z') || text.ends_with("+00:00");
    if has_utc_suffix && text.contains('T') {
        Ok(())
    } else {
        Err("parse_timestamp_utc requires an ISO-8601 UTC timestamp".into())
    }
}

fn canonical_component_bytes(logical_type: &str, value: &Value) -> Result<Vec<u8>, String> {
    let canonical = match logical_type {
        "bool" | "boolean" => CanonicalValue::Bool(
            value
                .as_bool()
                .ok_or_else(|| "bool join key value must be JSON bool".to_string())?,
        ),
        "int64" | "int" => CanonicalValue::Int {
            width: 8,
            value: json_i64(value)? as i128,
        },
        "uint64" | "uint" => CanonicalValue::Uint {
            width: 8,
            value: json_u64(value)? as u128,
        },
        "float64" => CanonicalValue::Float64(json_f64(value)?),
        "utf8" | "string" => CanonicalValue::Utf8(
            value
                .as_str()
                .ok_or_else(|| "utf8 join key value must be JSON string".to_string())?,
        ),
        "binary" => CanonicalValue::Bytes(
            value
                .as_str()
                .ok_or_else(|| "binary join key value must be encoded as a string".to_string())?
                .as_bytes(),
        ),
        other => {
            return Err(format!(
                "logical type '{other}' is not supported in COVE-MAP join keys"
            ))
        }
    };
    canonical.encode().map_err(|err| err.to_string())
}

fn mapped_goid(
    mapping_id: &[u8],
    mapping_version: &[u8],
    object_type_id: u32,
    anchor_kind: &[u8],
    anchor_bytes: &[u8],
    source_scope: Option<&str>,
) -> [u8; 16] {
    let object_type_id = object_type_id.to_le_bytes();
    let source_scope = source_scope.unwrap_or("").as_bytes();
    goid16_parts(&[
        mapping_id,
        mapping_version,
        &object_type_id,
        anchor_kind,
        anchor_bytes,
        source_scope,
    ])
}

fn goid16_parts(parts: &[&[u8]]) -> [u8; 16] {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    let digest = hasher.finalize();
    let mut out = [0u8; 16];
    out.copy_from_slice(&digest[..16]);
    out
}

fn logical_type_from_name(name: &str) -> Result<CoveLogicalType, String> {
    match name {
        "null" => Ok(CoveLogicalType::Null),
        "bool" | "boolean" => Ok(CoveLogicalType::Bool),
        "int8" => Ok(CoveLogicalType::Int8),
        "int16" => Ok(CoveLogicalType::Int16),
        "int32" => Ok(CoveLogicalType::Int32),
        "int64" | "int" => Ok(CoveLogicalType::Int64),
        "uint8" => Ok(CoveLogicalType::UInt8),
        "uint16" => Ok(CoveLogicalType::UInt16),
        "uint32" => Ok(CoveLogicalType::UInt32),
        "uint64" | "uint" => Ok(CoveLogicalType::UInt64),
        "float32" => Ok(CoveLogicalType::Float32),
        "float64" | "float" => Ok(CoveLogicalType::Float64),
        "decimal64" => Ok(CoveLogicalType::Decimal64),
        "decimal128" | "decimal" => Ok(CoveLogicalType::Decimal128),
        "date_days" | "date32" | "date" => Ok(CoveLogicalType::DateDays),
        "timestamp_micros" | "timestamp_us" => Ok(CoveLogicalType::TimestampMicros),
        "timestamp_nanos" | "timestamp_ns" => Ok(CoveLogicalType::TimestampNanos),
        "utf8" | "string" => Ok(CoveLogicalType::Utf8),
        "binary" => Ok(CoveLogicalType::Binary),
        "json" => Ok(CoveLogicalType::Json),
        "uuid" => Ok(CoveLogicalType::Uuid),
        "list" => Ok(CoveLogicalType::List),
        "struct" => Ok(CoveLogicalType::Struct),
        "map" => Ok(CoveLogicalType::Map),
        other => Err(format!("unsupported COVE-MAP logical type '{other}'")),
    }
}

fn physical_for_logical(logical: CoveLogicalType) -> CovePhysicalKind {
    match logical {
        CoveLogicalType::Bool => CovePhysicalKind::Boolean,
        CoveLogicalType::Utf8 | CoveLogicalType::Binary | CoveLogicalType::Json => {
            CovePhysicalKind::VarBytes
        }
        CoveLogicalType::Uuid | CoveLogicalType::Decimal128 | CoveLogicalType::Decimal64 => {
            CovePhysicalKind::FixedBytes
        }
        CoveLogicalType::List | CoveLogicalType::Struct | CoveLogicalType::Map => {
            CovePhysicalKind::FileCode
        }
        _ => CovePhysicalKind::NumCode,
    }
}

fn record_kind_from_name(name: &str) -> Result<RecordKind, String> {
    match name {
        "delta" | "Delta" => Ok(RecordKind::Delta),
        "snapshot" | "Snapshot" => Ok(RecordKind::Snapshot),
        "baseline" | "Baseline" | "upsert" | "Upsert" => Ok(RecordKind::Baseline),
        "tombstone" | "Tombstone" => Ok(RecordKind::Tombstone),
        other => Err(format!("unsupported COVE-O record kind '{other}'")),
    }
}

fn encoding_for_physical(physical: CovePhysicalKind) -> CoveEncodingKind {
    match physical {
        CovePhysicalKind::Boolean | CovePhysicalKind::FixedBytes => CoveEncodingKind::PlainFixed,
        CovePhysicalKind::NumCode => CoveEncodingKind::NumCode,
        CovePhysicalKind::FileCode => CoveEncodingKind::FileCode,
        CovePhysicalKind::VarBytes => CoveEncodingKind::VarBytes,
        CovePhysicalKind::List | CovePhysicalKind::Struct | CovePhysicalKind::Map => {
            CoveEncodingKind::Canonical
        }
        _ => CoveEncodingKind::Canonical,
    }
}

fn json_bool(value: &Value) -> Result<bool, String> {
    match value {
        Value::Bool(value) => Ok(*value),
        Value::String(text) if text.eq_ignore_ascii_case("true") => Ok(true),
        Value::String(text) if text.eq_ignore_ascii_case("false") => Ok(false),
        _ => Err("property value is not a bool".into()),
    }
}

fn json_numcode(value: &Value) -> Result<u64, String> {
    match value {
        Value::Bool(value) => Ok(u64::from(*value)),
        Value::Number(number) => number
            .as_u64()
            .or_else(|| number.as_i64().and_then(|value| u64::try_from(value).ok()))
            .ok_or_else(|| "numeric property value is outside supported NumCode range".to_string()),
        Value::String(text) => text
            .parse::<u64>()
            .map_err(|_| format!("'{text}' is not a supported NumCode value")),
        _ => Err("property value is not numeric".into()),
    }
}

fn fixed_bytes_for_property(property: &PropertyEntryV1, value: &Value) -> Result<Vec<u8>, String> {
    match property.logical_type {
        CoveLogicalType::Uuid => {
            let text = value
                .as_str()
                .ok_or_else(|| "uuid property values must be hex strings".to_string())?;
            Ok(hex_decode_16(text)?.to_vec())
        }
        CoveLogicalType::Decimal128 => {
            let int = value
                .as_i64()
                .map(i128::from)
                .or_else(|| value.as_str().and_then(|text| text.parse::<i128>().ok()))
                .ok_or_else(|| "decimal128 property value must be an integer".to_string())?;
            Ok(int.to_le_bytes().to_vec())
        }
        CoveLogicalType::Decimal64 => {
            let int = value
                .as_i64()
                .or_else(|| value.as_str().and_then(|text| text.parse::<i64>().ok()))
                .ok_or_else(|| "decimal64 property value must be an integer".to_string())?;
            Ok(int.to_le_bytes().to_vec())
        }
        other => Err(format!("unsupported fixed-bytes logical type '{other:?}'")),
    }
}

fn var_bytes_for_property(property: &PropertyEntryV1, value: &Value) -> Result<Vec<u8>, String> {
    match property.logical_type {
        CoveLogicalType::Utf8 => value
            .as_str()
            .map(|text| text.as_bytes().to_vec())
            .ok_or_else(|| "utf8 property value must be a string".to_string()),
        CoveLogicalType::Json => serde_json::to_vec(value).map_err(|err| err.to_string()),
        CoveLogicalType::Binary => value
            .as_str()
            .map(|text| text.as_bytes().to_vec())
            .ok_or_else(|| "binary property value must be encoded as a string".to_string()),
        other => Err(format!("unsupported var-bytes logical type '{other:?}'")),
    }
}

fn stable_u32(text: &str, fallback: u32) -> u32 {
    let digest = Sha256::digest(text.as_bytes());
    let value = u32::from_le_bytes(digest[..4].try_into().unwrap());
    if value == 0 {
        fallback
    } else {
        value
    }
}

fn section_set(file: &CovemapFile) -> BTreeSet<String> {
    file.sections
        .iter()
        .map(|section| section_kind(section.entry.section_id))
        .collect()
}

fn object_to_btree(object: &Map<String, Value>) -> BTreeMap<String, Value> {
    object
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn row_digest(row: &SourceRow) -> String {
    sha256_hex(canonical_row_json(&row.values).as_bytes())
}

fn schema_fingerprint(row: &SourceRow) -> String {
    let schema = row
        .values
        .iter()
        .map(|(key, value)| format!("{key}:{}", logical_type_name(value)))
        .collect::<Vec<_>>()
        .join("|");
    sha256_hex(schema.as_bytes())
}

fn canonical_row_json(values: &BTreeMap<String, Value>) -> String {
    serde_json::to_string(values).expect("BTreeMap JSON serialization cannot fail")
}

fn logical_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(number) if number.is_i64() => "int64",
        Value::Number(number) if number.is_u64() => "uint64",
        Value::Number(_) => "float64",
        Value::String(_) => "utf8",
        Value::Array(_) => "list",
        Value::Object(_) => "struct",
    }
}

fn json_i64(value: &Value) -> Result<i64, String> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .ok_or_else(|| "JSON number is not an i64".to_string()),
        Value::String(text) => text
            .parse::<i64>()
            .map_err(|_| format!("'{text}' is not an i64")),
        _ => Err("join key value is not an i64".into()),
    }
}

fn json_u64(value: &Value) -> Result<u64, String> {
    match value {
        Value::Number(number) => number
            .as_u64()
            .ok_or_else(|| "JSON number is not a u64".to_string()),
        Value::String(text) => text
            .parse::<u64>()
            .map_err(|_| format!("'{text}' is not a u64")),
        _ => Err("join key value is not a u64".into()),
    }
}

fn json_f64(value: &Value) -> Result<f64, String> {
    match value {
        Value::Number(number) => number
            .as_f64()
            .ok_or_else(|| "JSON number is not a finite f64".to_string()),
        Value::String(text) => text
            .parse::<f64>()
            .map_err(|_| format!("'{text}' is not an f64")),
        _ => Err("join key value is not an f64".into()),
    }
}

fn json_i128(value: &Value) -> Result<i128, String> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .map(i128::from)
            .or_else(|| number.as_u64().map(|value| value as i128))
            .ok_or_else(|| "JSON number is not an i128-compatible integer".to_string()),
        Value::String(text) => text
            .parse::<i128>()
            .map_err(|_| format!("'{text}' is not an i128")),
        _ => Err("value is not an i128".into()),
    }
}

fn json_string(value: &Value) -> Result<&str, String> {
    value
        .as_str()
        .ok_or_else(|| "value must be a string".to_string())
}

fn json_uuid(value: &Value) -> Result<[u8; 16], String> {
    hex_decode_16(json_string(value)?)
}

fn append_len_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(bytes);
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex_encode(&Sha256::digest(bytes))
}

fn sha256_array(bytes: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

fn first_16(bytes: &[u8; 32]) -> [u8; 16] {
    let mut out = [0u8; 16];
    out.copy_from_slice(&bytes[..16]);
    out
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn hex_decode_16(text: &str) -> Result<[u8; 16], String> {
    let text = text.trim();
    if text.len() != 32 {
        return Err("uuid hex string must contain 32 hex characters".into());
    }
    let mut out = [0u8; 16];
    for (index, chunk) in text.as_bytes().chunks_exact(2).enumerate() {
        out[index] = (hex_nibble(chunk[0])? << 4) | hex_nibble(chunk[1])?;
    }
    Ok(out)
}

fn hex_nibble(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err("invalid hex character".into()),
    }
}

fn required_str<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("fixture.{key} must be a string"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cove_core::{
        artifact::covemap::{
            CovemapHeaderV1, CovemapPayloadEncodingV2, CovemapPostscriptV1, CovemapSection,
            CovemapSectionEntryV1,
        },
        compression,
        constants::FEATURE_SEMANTIC_MAP,
        profile::cove_o::{
            read_object_surface_from_bytes, TemporalSegmentData,
            PROPERTY_FLAG_ASSOCIATION_FROM_GOID, PROPERTY_FLAG_ASSOCIATION_TO_GOID,
            PROPERTY_FLAG_ASSOCIATION_TYPE, PROPERTY_FLAG_EVIDENCE_REF,
        },
    };

    fn test_section(kind: SectionKind, value: Value) -> CovemapSection {
        let payload = serde_json::to_vec_pretty(&covemap_payload_value(kind, value)).unwrap();
        CovemapSection {
            entry: CovemapSectionEntryV1 {
                section_id: kind as u32,
                offset: 0,
                length: payload.len() as u64,
                uncompressed_length: payload.len() as u64,
                compression: 0,
                payload_encoding: CovemapPayloadEncodingV2::CoveMapJsonV2 as u8,
                required: true,
                reserved: 0,
                checksum: 0,
            },
            payload,
        }
    }

    fn mutate_section_payload(file: &mut CovemapFile, index: usize, edit: impl FnOnce(&mut Value)) {
        let mut payload: Value = serde_json::from_slice(&file.sections[index].payload).unwrap();
        edit(&mut payload);
        let bytes = serde_json::to_vec_pretty(&payload).unwrap();
        file.sections[index].entry.length = bytes.len() as u64;
        file.sections[index].entry.uncompressed_length = bytes.len() as u64;
        file.sections[index].payload = bytes;
    }

    fn covemap_payload_value(kind: SectionKind, mut value: Value) -> Value {
        if let Value::Object(object) = &mut value {
            object.insert(
                "schema_id".to_string(),
                Value::String("org.coveformat.covemap.v2".to_string()),
            );
            object.insert(
                "section_id".to_string(),
                Value::Number((kind as u16).into()),
            );
        }
        value
    }

    fn test_covemap(sections: Vec<CovemapSection>) -> CovemapFile {
        CovemapFile {
            header: CovemapHeaderV1::new([0x42; 16], 0),
            mapping_version: "test/v1".into(),
            sections,
            postscript: CovemapPostscriptV1 {
                required_features: FEATURE_SEMANTIC_MAP,
                optional_features: 0,
                file_len: 0,
                header_offset: 0,
                header_length: 0,
                checksum: 0,
            },
        }
    }

    fn two_source_identity_map(do_not_merge: Vec<Value>) -> CovemapFile {
        test_covemap(vec![
            test_section(
                SectionKind::MapSourceCatalog,
                json!({
                    "mapping_id": "people-map",
                    "mapping_version": "test/v1",
                    "sources": [
                        {"source_id": "crm", "row_identity_rules": ["person_by_id"]},
                        {"source_id": "support", "row_identity_rules": ["person_by_id"]}
                    ]
                }),
            ),
            test_section(
                SectionKind::MapFunctionRegistry,
                json!({
                    "mapping_id": "people-map",
                    "mapping_version": "test/v1",
                    "functions": [{
                        "function_id": "identity",
                        "version": "1",
                        "deterministic": true,
                        "dependency": "pure"
                    }]
                }),
            ),
            test_section(
                SectionKind::MapIdentityRuleCatalog,
                json!({
                    "mapping_id": "people-map",
                    "mapping_version": "test/v1",
                    "identity_rules": [{
                        "rule_id": "person_by_id",
                        "object_type": "Person",
                        "semantic_role": "subject",
                        "confidence_class": "authoritative",
                        "candidate_only": false,
                        "property_conflicts_declared": true,
                        "function_ids": ["identity"],
                        "join_keys": [{
                            "role_id": "person_id",
                            "source_column": "id",
                            "logical_type": "utf8",
                            "canonicalization": "identity",
                            "null_policy": "reject",
                            "ordering": "declared"
                        }]
                    }],
                    "do_not_merge": do_not_merge
                }),
            ),
            test_section(
                SectionKind::MapRowSemanticsCatalog,
                json!({
                    "mapping_id": "people-map",
                    "mapping_version": "test/v1",
                    "rules": [
                        {
                            "rule_id": "crm_person",
                            "source_id": "crm",
                            "identity_rule_id": "person_by_id",
                            "row_semantics_kind": "Object",
                            "assertion_kinds": ["object", "evidence"],
                            "function_ids": ["identity"],
                            "output_assertion_ids": [],
                            "association_endpoints": []
                        },
                        {
                            "rule_id": "support_person",
                            "source_id": "support",
                            "identity_rule_id": "person_by_id",
                            "row_semantics_kind": "Object",
                            "assertion_kinds": ["object", "evidence"],
                            "function_ids": ["identity"],
                            "output_assertion_ids": [],
                            "association_endpoints": []
                        }
                    ]
                }),
            ),
        ])
    }

    fn add_optional_i64(object: &mut Value, key: &str, value: Option<i64>) {
        if let Some(value) = value {
            object
                .as_object_mut()
                .unwrap()
                .insert(key.into(), json!(value));
        }
    }

    fn two_source_property_map(
        conflict_policy: &str,
        crm_priority: Option<i64>,
        support_priority: Option<i64>,
    ) -> CovemapFile {
        let mut crm = json!({
            "source_id": "crm",
            "row_identity_rules": ["person_by_id"]
        });
        add_optional_i64(&mut crm, "source_priority", crm_priority);
        let mut support = json!({
            "source_id": "support",
            "row_identity_rules": ["person_by_id"]
        });
        add_optional_i64(&mut support, "source_priority", support_priority);

        test_covemap(vec![
            test_section(
                SectionKind::MapSourceCatalog,
                json!({
                    "mapping_id": "people-map",
                    "mapping_version": "test/v1",
                    "sources": [crm, support]
                }),
            ),
            test_section(
                SectionKind::MapFunctionRegistry,
                json!({
                    "mapping_id": "people-map",
                    "mapping_version": "test/v1",
                    "functions": [{
                        "function_id": "identity",
                        "version": "1",
                        "deterministic": true,
                        "dependency": "pure"
                    }]
                }),
            ),
            test_section(
                SectionKind::MapIdentityRuleCatalog,
                json!({
                    "mapping_id": "people-map",
                    "mapping_version": "test/v1",
                    "identity_rules": [{
                        "rule_id": "person_by_id",
                        "object_type": "Person",
                        "semantic_role": "subject",
                        "confidence_class": "authoritative",
                        "candidate_only": false,
                        "property_conflicts_declared": true,
                        "function_ids": ["identity"],
                        "join_keys": [{
                            "role_id": "person_id",
                            "source_column": "id",
                            "logical_type": "utf8",
                            "canonicalization": "identity",
                            "null_policy": "reject",
                            "ordering": "declared"
                        }]
                    }],
                    "do_not_merge": []
                }),
            ),
            test_section(
                SectionKind::MapRowSemanticsCatalog,
                json!({
                    "mapping_id": "people-map",
                    "mapping_version": "test/v1",
                    "rules": [
                        {
                            "rule_id": "crm_person",
                            "source_id": "crm",
                            "identity_rule_id": "person_by_id",
                            "row_semantics_kind": "Object",
                            "assertion_kinds": ["object", "property", "evidence"],
                            "function_ids": ["identity"],
                            "output_assertion_ids": [],
                            "association_endpoints": [],
                            "property_bindings": [{
                                "assertion_id": "crm_name",
                                "property_id": "name",
                                "property_name": "name",
                                "source_column": "name",
                                "logical_type": "utf8",
                                "nullable": true,
                                "conflict_policy": conflict_policy
                            }]
                        },
                        {
                            "rule_id": "support_person",
                            "source_id": "support",
                            "identity_rule_id": "person_by_id",
                            "row_semantics_kind": "Object",
                            "assertion_kinds": ["object", "property", "evidence"],
                            "function_ids": ["identity"],
                            "output_assertion_ids": [],
                            "association_endpoints": [],
                            "property_bindings": [{
                                "assertion_id": "support_name",
                                "property_id": "name",
                                "property_name": "name",
                                "source_column": "name",
                                "logical_type": "utf8",
                                "nullable": true,
                                "conflict_policy": conflict_policy
                            }]
                        }
                    ]
                }),
            ),
        ])
    }

    fn conflict_rows(crm_name: Value, support_name: Value) -> Vec<SourceRow> {
        vec![
            SourceRow {
                source_id: "crm".into(),
                row_index: 0,
                values: BTreeMap::from([("id".into(), json!("1")), ("name".into(), crm_name)]),
            },
            SourceRow {
                source_id: "support".into(),
                row_index: 0,
                values: BTreeMap::from([("id".into(), json!("1")), ("name".into(), support_name)]),
            },
        ]
    }

    fn association_readback_map() -> CovemapFile {
        test_covemap(vec![
            test_section(
                SectionKind::MapSourceCatalog,
                json!({
                    "mapping_id": "people-map",
                    "mapping_version": "test/v1",
                    "sources": [{
                        "source_id": "people",
                        "row_identity_rules": ["person_by_id", "team_by_id"]
                    }]
                }),
            ),
            test_section(
                SectionKind::MapFunctionRegistry,
                json!({
                    "mapping_id": "people-map",
                    "mapping_version": "test/v1",
                    "functions": [{
                        "function_id": "identity",
                        "version": "1",
                        "deterministic": true,
                        "dependency": "pure"
                    }]
                }),
            ),
            test_section(
                SectionKind::MapIdentityRuleCatalog,
                json!({
                    "mapping_id": "people-map",
                    "mapping_version": "test/v1",
                    "identity_rules": [
                        {
                            "rule_id": "person_by_id",
                            "object_type": "Person",
                            "semantic_role": "subject",
                            "confidence_class": "authoritative",
                            "candidate_only": false,
                            "property_conflicts_declared": true,
                            "function_ids": ["identity"],
                            "join_keys": [{
                                "role_id": "person_id",
                                "source_column": "person_id",
                                "logical_type": "utf8",
                                "canonicalization": "identity",
                                "null_policy": "reject",
                                "ordering": "declared"
                            }]
                        },
                        {
                            "rule_id": "team_by_id",
                            "object_type": "Team",
                            "semantic_role": "organization",
                            "confidence_class": "authoritative",
                            "candidate_only": false,
                            "property_conflicts_declared": true,
                            "function_ids": ["identity"],
                            "join_keys": [{
                                "role_id": "team_id",
                                "source_column": "team_id",
                                "logical_type": "utf8",
                                "canonicalization": "identity",
                                "null_policy": "reject",
                                "ordering": "declared"
                            }]
                        }
                    ],
                    "do_not_merge": []
                }),
            ),
            test_section(
                SectionKind::MapRowSemanticsCatalog,
                json!({
                    "mapping_id": "people-map",
                    "mapping_version": "test/v1",
                    "rules": [
                        {
                            "rule_id": "person_row",
                            "source_id": "people",
                            "identity_rule_id": "person_by_id",
                            "row_semantics_kind": "Object",
                            "assertion_kinds": ["object", "association", "evidence"],
                            "function_ids": ["identity"],
                            "output_assertion_ids": [],
                            "association_endpoints": [],
                            "association_bindings": [{
                                "assertion_id": "member_of_assertion",
                                "association_type": "member_of",
                                "source_identity_rule_id": "person_by_id",
                                "source_endpoint_expression": "source.goid",
                                "target_identity_rule_id": "team_by_id",
                                "target_endpoint_expression": "identity(team_by_id)",
                                "source_role": "member",
                                "target_role": "team",
                                "valid_from_expression": "source.valid_from",
                                "valid_to_expression": "source.valid_to",
                                "cardinality_policy": "many_to_one",
                                "missing_policy": "reject"
                            }]
                        },
                        {
                            "rule_id": "team_row",
                            "source_id": "people",
                            "identity_rule_id": "team_by_id",
                            "row_semantics_kind": "Object",
                            "assertion_kinds": ["object", "evidence"],
                            "function_ids": ["identity"],
                            "output_assertion_ids": [],
                            "association_endpoints": []
                        }
                    ]
                }),
            ),
        ])
    }

    fn governance_map(policy: &str) -> CovemapFile {
        let mut file = two_source_identity_map(Vec::new());
        file.sections[0] = test_section(
            SectionKind::MapSourceCatalog,
            json!({
                "mapping_id": "people-map",
                "mapping_version": "test/v1",
                "governance_reconciliation_policy": policy,
                "sources": [
                    {
                        "source_id": "crm",
                        "row_identity_rules": ["person_by_id"],
                        "sensitivity_label": "public",
                        "sensitivity_rank": 1,
                        "access_policy_ids": ["internal"]
                    },
                    {
                        "source_id": "support",
                        "row_identity_rules": ["person_by_id"],
                        "sensitivity_label": "restricted",
                        "sensitivity_rank": 5,
                        "access_policy_ids": ["hipaa"]
                    }
                ]
            }),
        );
        file
    }

    #[test]
    fn parses_validate_command() {
        assert_eq!(
            parse_args(["validate".to_string(), "mapping.covemap".to_string()])
                .unwrap()
                .unwrap(),
            Command::Validate {
                map: PathBuf::from("mapping.covemap")
            }
        );
    }

    #[test]
    fn parses_convert_cove_o_format() {
        let command = parse_args([
            "convert".to_string(),
            "--format".to_string(),
            "cove-o".to_string(),
            "-o".to_string(),
            "out.cove".to_string(),
            "mapping.covemap".to_string(),
            "source.jsonl".to_string(),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(
            command,
            Command::Convert {
                map: PathBuf::from("mapping.covemap"),
                sources: vec![PathBuf::from("source.jsonl")],
                output: Some(PathBuf::from("out.cove")),
                format: OutputFormat::CoveO,
            }
        );
    }

    #[test]
    fn parses_project_cove_o_command() {
        let command = parse_args([
            "project-cove-o".to_string(),
            "--mapping".to_string(),
            "mapping.covemap".to_string(),
            "-o".to_string(),
            "projection.json".to_string(),
            "object.cove".to_string(),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(
            command,
            Command::ProjectCoveO {
                object: PathBuf::from("object.cove"),
                mapping: Some(PathBuf::from("mapping.covemap")),
                output: Some(PathBuf::from("projection.json")),
                format: ProjectionFormat::Json,
                projection_id: None,
            }
        );
    }

    #[test]
    fn join_key_is_deterministic() {
        let components = [
            JoinKeyComponent {
                role_id: "email",
                logical_type_id: "utf8",
                value: Some(b"a@example.com"),
            },
            JoinKeyComponent {
                role_id: "tenant",
                logical_type_id: "utf8",
                value: Some(b"t1"),
            },
        ];
        assert_eq!(
            join_key_tuple(1, "person_by_email", &components),
            join_key_tuple(1, "person_by_email", &components)
        );
    }

    #[test]
    fn join_key_distinguishes_null_from_empty_value() {
        let null_component = [JoinKeyComponent {
            role_id: "email",
            logical_type_id: "utf8",
            value: None,
        }];
        let empty_component = [JoinKeyComponent {
            role_id: "email",
            logical_type_id: "utf8",
            value: Some(b""),
        }];
        assert_ne!(
            join_key_tuple(1, "person_by_email", &null_component),
            join_key_tuple(1, "person_by_email", &empty_component)
        );
    }

    #[test]
    fn unicode_casefold_uses_full_unicode_mapping() {
        let folded = apply_canonicalization(
            &json!("Straße"),
            "unicode_casefold",
            &["unicode_casefold".to_string()],
        )
        .unwrap();
        assert_eq!(folded, json!("strasse"));
    }

    #[test]
    fn goid_is_sha256_truncated_to_16_bytes() {
        let goid = goid16_parts(&[b"map", b"v1", b"person", b"rule", b"key"]);
        assert_eq!(goid.len(), 16);
        assert_eq!(
            goid,
            goid16_parts(&[b"map", b"v1", b"person", b"rule", b"key"])
        );
    }

    #[test]
    fn csv_reader_is_deterministic_for_simple_rows() {
        let dir = std::env::temp_dir().join(format!("cove-map-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("people.csv");
        fs::write(&path, "id,name\n1,Ada\n2,Linus\n").unwrap();
        let rows = read_csv(&path, "people").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].values["id"], json!("1"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cross_source_authoritative_identity_merges_to_one_goid() {
        let file = two_source_identity_map(Vec::new());
        let rows = vec![
            SourceRow {
                source_id: "crm".into(),
                row_index: 0,
                values: BTreeMap::from([("id".into(), json!("1"))]),
            },
            SourceRow {
                source_id: "support".into(),
                row_index: 0,
                values: BTreeMap::from([("id".into(), json!("1"))]),
            },
        ];
        let planned = plan_identities(&file, &rows).unwrap();
        let goids = planned
            .canonical
            .iter()
            .map(|identity| identity.goid)
            .collect::<BTreeSet<_>>();
        assert_eq!(goids.len(), 1);
        let index = identity_equivalence_index("people-map", "test/v1", &planned.canonical);
        assert_eq!(index["equivalences"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn candidate_identity_rules_emit_evidence_without_goids() {
        let mut file = two_source_identity_map(Vec::new());
        file.sections[2] = test_section(
            SectionKind::MapIdentityRuleCatalog,
            json!({
                "mapping_id": "people-map",
                "mapping_version": "test/v1",
                "identity_rules": [{
                    "rule_id": "person_by_id",
                    "object_type": "Person",
                    "semantic_role": "subject",
                    "confidence_class": "candidate",
                    "candidate_only": true,
                    "property_conflicts_declared": true,
                    "function_ids": ["identity"],
                    "join_keys": [{
                        "role_id": "person_id",
                        "source_column": "id",
                        "logical_type": "utf8",
                        "canonicalization": "identity",
                        "null_policy": "reject",
                        "ordering": "declared"
                    }]
                }],
                "do_not_merge": []
            }),
        );
        file.sections[3] = test_section(
            SectionKind::MapRowSemanticsCatalog,
            json!({
                "mapping_id": "people-map",
                "mapping_version": "test/v1",
                "rules": [
                    {
                        "rule_id": "crm_candidate_person",
                        "source_id": "crm",
                        "identity_rule_id": "person_by_id",
                        "row_semantics_kind": "EvidenceOnly",
                        "assertion_kinds": ["candidate_match", "evidence"],
                        "function_ids": ["identity"],
                        "output_assertion_ids": [],
                        "association_endpoints": []
                    },
                    {
                        "rule_id": "support_candidate_person",
                        "source_id": "support",
                        "identity_rule_id": "person_by_id",
                        "row_semantics_kind": "EvidenceOnly",
                        "assertion_kinds": ["candidate_match", "evidence"],
                        "function_ids": ["identity"],
                        "output_assertion_ids": [],
                        "association_endpoints": []
                    }
                ]
            }),
        );
        let rows = vec![
            SourceRow {
                source_id: "crm".into(),
                row_index: 0,
                values: BTreeMap::from([("id".into(), json!("1"))]),
            },
            SourceRow {
                source_id: "support".into(),
                row_index: 0,
                values: BTreeMap::from([("id".into(), json!("1"))]),
            },
        ];
        let plan = plan_identities(&file, &rows).unwrap();
        assert!(plan.canonical.is_empty());
        assert_eq!(plan.candidates.len(), 2);
        let materialized = materialize_with_source_states(&file, &rows, &[]).unwrap();
        assert!(materialized.rows.is_empty());
        assert_eq!(
            materialized.conversion_report["candidate_match_count"],
            json!(2)
        );
        assert_eq!(
            materialized.identity_equivalence_index["equivalences"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
        assert!(materialized
            .evidence_entries
            .iter()
            .all(|entry| entry["candidate"] == json!(true)));
    }

    #[test]
    fn do_not_merge_conflict_rejects_identity_resolution() {
        let file = two_source_identity_map(vec![json!({
            "left_identity": "crm:0",
            "right_identity": "support:0"
        })]);
        let rows = vec![
            SourceRow {
                source_id: "crm".into(),
                row_index: 0,
                values: BTreeMap::from([("id".into(), json!("1"))]),
            },
            SourceRow {
                source_id: "support".into(),
                row_index: 0,
                values: BTreeMap::from([("id".into(), json!("1"))]),
            },
        ];
        assert!(plan_identities(&file, &rows).is_err());
    }

    #[test]
    fn property_conflict_rejects_unequal_cross_source_values() {
        let file = two_source_property_map("reject_conflict", None, None);
        let rows = conflict_rows(json!("Ada"), json!("Ada Lovelace"));
        let err = materialize_with_source_states(&file, &rows, &[]).unwrap_err();
        assert!(err.contains("unresolved property conflict"));
    }

    #[test]
    fn property_conflict_accepts_equal_duplicate_values() {
        let file = two_source_property_map("reject_conflict", None, None);
        let rows = conflict_rows(json!("Ada"), json!("Ada"));
        let materialized = materialize_with_source_states(&file, &rows, &[]).unwrap();
        let name_values = materialized
            .rows
            .iter()
            .flat_map(|row| row.properties.values())
            .filter(|property| property.entry.property_name == "name")
            .map(|property| property.value.clone())
            .collect::<Vec<_>>();
        assert_eq!(name_values, vec![json!("Ada"), json!("Ada")]);
    }

    #[test]
    fn null_property_candidate_does_not_overwrite_non_null_value() {
        let file = two_source_property_map("reject_conflict", None, None);
        let rows = conflict_rows(Value::Null, json!("Ada"));
        let materialized = materialize_with_source_states(&file, &rows, &[]).unwrap();
        let name_values = materialized
            .rows
            .iter()
            .flat_map(|row| row.properties.values())
            .filter(|property| property.entry.property_name == "name")
            .map(|property| property.value.clone())
            .collect::<Vec<_>>();
        assert_eq!(name_values, vec![json!("Ada")]);
        assert!(materialized.evidence_entries.iter().any(|entry| {
            entry.get("suppressed_reason").and_then(Value::as_str)
                == Some("null_does_not_overwrite_non_null")
        }));
    }

    #[test]
    fn source_priority_wins_suppresses_losing_property_values() {
        let file = two_source_property_map("source_priority_wins", Some(10), Some(1));
        let rows = conflict_rows(json!("CRM"), json!("Support"));
        let materialized = materialize_with_source_states(&file, &rows, &[]).unwrap();
        let name_values = materialized
            .rows
            .iter()
            .flat_map(|row| row.properties.values())
            .filter(|property| property.entry.property_name == "name")
            .map(|property| property.value.clone())
            .collect::<Vec<_>>();
        assert_eq!(name_values, vec![json!("Support")]);
        assert!(materialized.evidence_entries.iter().any(|entry| {
            entry.get("suppressed_reason").and_then(Value::as_str) == Some("source_priority_wins")
                && entry.get("suppressed_value") == Some(&json!("CRM"))
        }));
    }

    #[test]
    fn patch_operation_sets_delta_metadata_and_round_trips_evidence() {
        let mut file = two_source_property_map("reject_conflict", None, None);
        mutate_section_payload(&mut file, 3, |payload| {
            let rule = payload["rules"].as_array_mut().unwrap()[0]
                .as_object_mut()
                .unwrap();
            rule.insert("source_operation_kind".into(), json!("PatchProperty"));
        });
        let rows = vec![SourceRow {
            source_id: "crm".into(),
            row_index: 0,
            values: BTreeMap::from([
                ("id".into(), json!("1")),
                ("name".into(), json!("Ada")),
                ("correction_of".into(), json!("crm:previous")),
                ("replacement_of".into(), json!("goid:previous")),
            ]),
        }];
        let materialized = materialize_with_source_states(&file, &rows, &[]).unwrap();
        assert_eq!(materialized.rows[0].record_kind, RecordKind::Delta);
        let evidence = materialized
            .evidence_entries
            .iter()
            .find(|entry| entry["rule_id"] == json!("crm_person"))
            .unwrap();
        assert_eq!(evidence["source_operation_kind"], json!("PatchProperty"));
        assert_eq!(evidence["operation_effect"], json!("patch_property"));
        assert_eq!(evidence["operation_target"], json!("property"));
        assert_eq!(evidence["correction_of"], json!("crm:previous"));
        assert_eq!(evidence["replacement_of"], json!("goid:previous"));
        assert_eq!(
            materialized.conversion_report["operation_counts"]["PatchProperty"],
            json!(1)
        );

        let bytes = build_cove_o(&file, &rows).unwrap();
        let surface = read_object_surface_from_bytes(&bytes).unwrap();
        let persisted = surface
            .evidence_index
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .find(|entry| entry.rule_id == "crm_person")
            .unwrap();
        assert_eq!(
            persisted.operation_metadata["source_operation_kind"],
            json!("PatchProperty")
        );
        assert_eq!(
            persisted.operation_metadata["correction_of"],
            json!("crm:previous")
        );
    }

    #[test]
    fn close_association_operation_marks_association_delta_and_policy_metadata() {
        let mut file = association_readback_map();
        mutate_section_payload(&mut file, 3, |payload| {
            let rule = payload["rules"].as_array_mut().unwrap()[0]
                .as_object_mut()
                .unwrap();
            rule.insert("source_operation_kind".into(), json!("CloseAssociation"));
        });
        let rows = vec![SourceRow {
            source_id: "people".into(),
            row_index: 0,
            values: BTreeMap::from([
                ("person_id".into(), json!("p1")),
                ("team_id".into(), json!("t1")),
                ("valid_from".into(), json!("2026-01-01")),
                ("valid_to".into(), json!("2026-12-31")),
                ("closes_association".into(), json!("member_of:p1:t1")),
            ]),
        }];
        let materialized = materialize_with_source_states(&file, &rows, &[]).unwrap();
        let association = materialized
            .rows
            .iter()
            .find(|row| row.object_type == "Association:member_of")
            .unwrap();
        assert_eq!(association.record_kind, RecordKind::Delta);
        assert!(materialized.evidence_entries.iter().any(|entry| {
            entry["source_operation_kind"] == json!("CloseAssociation")
                && entry["operation_effect"] == json!("close_association")
                && entry["operation_target"] == json!("association")
                && entry["closes_association"] == json!("member_of:p1:t1")
        }));
    }

    #[test]
    fn evidence_only_operation_emits_evidence_without_object_rows() {
        let mut file = two_source_identity_map(Vec::new());
        mutate_section_payload(&mut file, 3, |payload| {
            let rule = payload["rules"].as_array_mut().unwrap()[0]
                .as_object_mut()
                .unwrap();
            rule.insert("row_semantics_kind".into(), json!("EvidenceOnly"));
            rule.insert("source_operation_kind".into(), json!("RedactEvidence"));
            rule.insert("assertion_kinds".into(), json!(["evidence"]));
        });
        let rows = vec![SourceRow {
            source_id: "crm".into(),
            row_index: 0,
            values: BTreeMap::from([
                ("id".into(), json!("1")),
                ("redaction_scope".into(), json!("source_evidence")),
            ]),
        }];
        let materialized = materialize_with_source_states(&file, &rows, &[]).unwrap();
        assert!(materialized.rows.is_empty());
        assert!(materialized.evidence_entries.iter().any(|entry| {
            entry["source_operation_kind"] == json!("RedactEvidence")
                && entry["operation_effect"] == json!("redact_evidence")
                && entry["operation_target"] == json!("evidence")
                && entry["redaction_scope"] == json!("source_evidence")
        }));
    }

    #[test]
    fn association_readback_preserves_roles_validity_and_cardinality() {
        let file = association_readback_map();
        let rows = vec![SourceRow {
            source_id: "people".into(),
            row_index: 0,
            values: BTreeMap::from([
                ("person_id".into(), json!("p1")),
                ("team_id".into(), json!("t1")),
                ("valid_from".into(), json!("2026-01-01")),
                ("valid_to".into(), json!("2026-12-31")),
            ]),
        }];
        let materialized = materialize_with_source_states(&file, &rows, &[]).unwrap();
        let association = materialized
            .rows
            .iter()
            .find(|row| row.object_type == "Association:member_of")
            .unwrap();
        assert_eq!(
            property_by_name(association, "source_role"),
            json!("member")
        );
        assert_eq!(property_by_name(association, "target_role"), json!("team"));
        assert_eq!(
            property_by_name(association, "valid_from"),
            json!("2026-01-01")
        );
        assert_eq!(
            property_by_name(association, "valid_to"),
            json!("2026-12-31")
        );
        assert_eq!(
            property_by_name(association, "cardinality_policy"),
            json!("many_to_one")
        );
    }

    #[test]
    fn cove_o_readback_decodes_association_surface_from_persisted_bytes() {
        let file = association_readback_map();
        let rows = vec![SourceRow {
            source_id: "people".into(),
            row_index: 0,
            values: BTreeMap::from([
                ("person_id".into(), json!("p1")),
                ("team_id".into(), json!("t1")),
                ("valid_from".into(), json!("2026-01-01")),
                ("valid_to".into(), json!("2026-12-31")),
            ]),
        }];
        let bytes = build_cove_o(&file, &rows).unwrap();
        let surface = read_object_surface_from_bytes(&bytes).unwrap();
        let association_records = surface
            .records
            .iter()
            .filter(|record| record.association.is_some())
            .collect::<Vec<_>>();
        assert_eq!(surface.records.len(), 3);
        assert_eq!(association_records.len(), 1);

        let association = association_records[0];
        let metadata = association.association.as_ref().unwrap();
        assert_eq!(metadata.association_type.as_deref(), Some("member_of"));
        let source = association
            .properties
            .iter()
            .find(|property| property.flags & PROPERTY_FLAG_ASSOCIATION_FROM_GOID != 0)
            .unwrap();
        let target = association
            .properties
            .iter()
            .find(|property| property.flags & PROPERTY_FLAG_ASSOCIATION_TO_GOID != 0)
            .unwrap();
        let association_type = association
            .properties
            .iter()
            .find(|property| property.flags & PROPERTY_FLAG_ASSOCIATION_TYPE != 0)
            .unwrap();
        let evidence = association
            .properties
            .iter()
            .find(|property| property.flags & PROPERTY_FLAG_EVIDENCE_REF != 0)
            .unwrap();
        assert_eq!(source.value.as_str().unwrap().len(), 32);
        assert_eq!(target.value.as_str().unwrap().len(), 32);
        assert_eq!(association_type.value, json!("member_of"));
        assert_eq!(evidence.value, json!("people:0"));
        assert_eq!(
            metadata.source_goid,
            source.value.as_str().map(str::to_string)
        );
        assert_eq!(
            metadata.target_goid,
            target.value.as_str().map(str::to_string)
        );
        assert_eq!(metadata.evidence_ref.as_deref(), Some("people:0"));
    }

    #[test]
    fn project_cove_o_matches_source_projection_for_objects_associations_and_evidence() {
        let mut file = association_readback_map();
        file.sections.push(test_section(
            SectionKind::MapProjectionCatalog,
            json!({
                "mapping_id": "people-map",
                "mapping_version": "test/v1",
                "projections": [
                    {
                        "projection_id": "person_objects.v1",
                        "output_table": "person_objects",
                        "row_grain": "one_row_per_object",
                        "anchor": {"object_type": "Person"},
                        "temporal_mode": {"as_of": "latest_committed"},
                        "multi_value_policy": "reject",
                        "columns": [
                            {"name": "goid", "value": "object.goid"},
                            {"name": "object_type", "value": "object.type"}
                        ],
                        "output_modes": ["json", "cove-o"]
                    },
                    {
                        "projection_id": "member_links.v1",
                        "output_table": "member_links",
                        "row_grain": "one_row_per_association",
                        "anchor": {"association_type": "member_of"},
                        "temporal_mode": {"as_of": "latest_committed"},
                        "multi_value_policy": "explode",
                        "columns": [
                            {"name": "source_goid", "value": "association.source_goid"},
                            {"name": "target_goid", "value": "association.target_goid"},
                            {"name": "association_type", "value": "association.association_type"},
                            {"name": "evidence_id", "value": "association.source_evidence_id"}
                        ],
                        "output_modes": ["json"]
                    },
                    {
                        "projection_id": "evidence_rows.v1",
                        "output_table": "evidence_rows",
                        "row_grain": "one_row_per_evidence_assertion",
                        "anchor": {"object_type": "Person"},
                        "temporal_mode": {"as_of": "latest_committed"},
                        "multi_value_policy": "reject",
                        "columns": [
                            {"name": "source_id", "value": "evidence.source_id"},
                            {"name": "rule_id", "value": "evidence.rule_id"},
                            {"name": "assertion_id", "value": "evidence.assertion_id"},
                            {"name": "output_object_id", "value": "evidence.output_object_id"}
                        ],
                        "output_modes": ["json"]
                    }
                ]
            }),
        ));
        let rows = vec![SourceRow {
            source_id: "people".into(),
            row_index: 0,
            values: BTreeMap::from([
                ("person_id".into(), json!("p1")),
                ("team_id".into(), json!("t1")),
                ("valid_from".into(), json!("2026-01-01")),
                ("valid_to".into(), json!("2026-12-31")),
            ]),
        }];
        let source_projected = project_rows(&file, &rows).unwrap();
        let bytes = build_cove_o(&file, &rows).unwrap();
        let dir = std::env::temp_dir().join(format!(
            "cove-map-project-cove-o-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        let object_path = dir.join("object.cove");
        fs::write(&object_path, bytes).unwrap();
        let persisted_projected = project_cove_o_path(&object_path, None).unwrap();
        assert_eq!(persisted_projected["rows"], source_projected["rows"]);
        assert_eq!(
            persisted_projected["rows"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|row| row["projection_id"] == json!("member_links.v1"))
                .count(),
            1
        );
        assert!(persisted_projected["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["projection_id"] == json!("evidence_rows.v1")));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn projection_cove_o_output_materializes_projected_objects() {
        let mut file = association_readback_map();
        file.sections.push(test_section(
            SectionKind::MapProjectionCatalog,
            json!({
                "mapping_id": "people-map",
                "mapping_version": "test/v1",
                "projections": [{
                    "projection_id": "person_objects.v1",
                    "output_table": "person_objects",
                    "row_grain": "one_row_per_object",
                    "anchor": {"object_type": "Person"},
                    "temporal_mode": {"as_of": "latest_committed"},
                    "multi_value_policy": "list",
                    "columns": [
                        {"name": "goid", "value": "object.goid"},
                        {"name": "object_type", "value": "object.type"}
                    ],
                    "output_modes": ["json", "cove-o"]
                }]
            }),
        ));
        let rows = vec![SourceRow {
            source_id: "people".into(),
            row_index: 0,
            values: BTreeMap::from([
                ("person_id".into(), json!("p1")),
                ("team_id".into(), json!("t1")),
                ("valid_from".into(), json!("2026-01-01")),
                ("valid_to".into(), json!("2026-12-31")),
            ]),
        }];
        let bytes = crate::project::project_rows_with_source_states_output(
            &file,
            &rows,
            &[],
            crate::project::ProjectionFormat::CoveO,
            Some("person_objects.v1"),
        )
        .unwrap();
        let surface = read_object_surface_from_bytes(&bytes).unwrap();
        assert_eq!(
            surface.projection_catalog.as_ref().unwrap().projections[0].projection_id,
            "person_objects.v1"
        );
        let projected = surface
            .records
            .iter()
            .find(|record| record.object_type_name == "person_objects")
            .unwrap();
        assert!(projected
            .properties
            .iter()
            .any(|property| property.property_name == "object_type"
                && property.value == json!("Person")));
    }

    #[test]
    fn projection_cove_o_output_stores_nested_properties_as_filecodes() {
        let mut file = association_readback_map();
        mutate_section_payload(&mut file, 3, |payload| {
            let rule = payload["rules"].as_array_mut().unwrap()[0]
                .as_object_mut()
                .unwrap();
            rule.insert(
                "property_bindings".into(),
                json!([
                    {
                        "assertion_id": "person_tags",
                        "property_id": "tags",
                        "property_name": "tags",
                        "source_column": "tags",
                        "logical_type": "list",
                        "physical_kind": "auto",
                        "nullable": true,
                        "missing_policy": "null",
                        "conflict_policy": "reject_conflict"
                    },
                    {
                        "assertion_id": "person_profile",
                        "property_id": "profile",
                        "property_name": "profile",
                        "source_column": "profile",
                        "logical_type": "struct",
                        "physical_kind": "auto",
                        "nullable": true,
                        "missing_policy": "null",
                        "conflict_policy": "reject_conflict"
                    },
                    {
                        "assertion_id": "person_scores",
                        "property_id": "scores",
                        "property_name": "scores",
                        "source_column": "scores",
                        "logical_type": "map",
                        "physical_kind": "auto",
                        "nullable": true,
                        "missing_policy": "null",
                        "conflict_policy": "reject_conflict"
                    }
                ]),
            );
        });
        file.sections.push(test_section(
            SectionKind::MapProjectionCatalog,
            json!({
                "mapping_id": "people-map",
                "mapping_version": "test/v1",
                "projections": [{
                    "projection_id": "person_nested.v1",
                    "output_table": "person_nested",
                    "row_grain": "one_row_per_object",
                    "anchor": {"object_type": "Person"},
                    "temporal_mode": {"as_of": "latest_committed"},
                    "multi_value_policy": "list",
                    "columns": [
                        {
                            "name": "tags",
                            "value": "tags",
                            "logical_type": "list",
                            "nested_shape": {
                                "type": "list",
                                "item": {"logical_type": "utf8"}
                            }
                        },
                        {
                            "name": "profile",
                            "value": "profile",
                            "logical_type": "struct",
                            "nested_shape": {
                                "type": "struct",
                                "fields": [
                                    {"name": "active", "logical_type": "bool"},
                                    {"name": "level", "logical_type": "int64"}
                                ]
                            }
                        },
                        {
                            "name": "scores",
                            "value": "scores",
                            "logical_type": "map",
                            "nested_shape": {
                                "type": "map",
                                "key": {"logical_type": "utf8"},
                                "value": {"logical_type": "int64"}
                            }
                        }
                    ],
                    "output_modes": ["json", "cove-o"]
                }]
            }),
        ));
        let rows = vec![SourceRow {
            source_id: "people".into(),
            row_index: 0,
            values: BTreeMap::from([
                ("person_id".into(), json!("p1")),
                ("team_id".into(), json!("t1")),
                ("valid_from".into(), json!("2026-01-01")),
                ("valid_to".into(), json!("2026-12-31")),
                ("tags".into(), json!(["alpha", "beta"])),
                ("profile".into(), json!({"active": true, "level": 7})),
                ("scores".into(), json!({"logic": 100, "math": 99})),
            ]),
        }];
        let bytes = crate::project::project_rows_with_source_states_output(
            &file,
            &rows,
            &[],
            crate::project::ProjectionFormat::CoveO,
            Some("person_nested.v1"),
        )
        .unwrap();
        let report = validate_bytes_with_options(&bytes, ValidationOptions::default()).unwrap();
        assert!(report
            .validated
            .footer
            .sections
            .iter()
            .any(|entry| { entry.section_kind == SectionKind::FileDictionaryIndex as u16 }));
        let surface = read_object_surface_from_bytes(&bytes).unwrap();
        let object_type = surface
            .object_types
            .iter()
            .find(|object_type| object_type.type_name == "person_nested")
            .unwrap();
        for property_name in ["tags", "profile", "scores"] {
            let property = object_type
                .properties
                .iter()
                .find(|property| property.property_name == property_name)
                .unwrap();
            assert_eq!(property.physical_kind, CovePhysicalKind::FileCode);
        }
        assert_eq!(
            object_type
                .properties
                .iter()
                .find(|property| property.property_name == "tags")
                .unwrap()
                .logical_type,
            CoveLogicalType::List
        );
        assert_eq!(
            object_type
                .properties
                .iter()
                .find(|property| property.property_name == "profile")
                .unwrap()
                .logical_type,
            CoveLogicalType::Struct
        );
        assert_eq!(
            object_type
                .properties
                .iter()
                .find(|property| property.property_name == "scores")
                .unwrap()
                .logical_type,
            CoveLogicalType::Map
        );
        let projected = surface
            .records
            .iter()
            .find(|record| record.object_type_name == "person_nested")
            .unwrap();
        let projected_property = |name: &str| {
            projected
                .properties
                .iter()
                .find(|property| property.property_name == name)
                .unwrap()
                .value
                .clone()
        };
        assert_eq!(projected_property("tags"), json!(["alpha", "beta"]));
        assert_eq!(
            projected_property("profile"),
            json!({"active": true, "level": 7})
        );
        assert_eq!(
            projected_property("scores"),
            json!({"logic": 100, "math": 99})
        );
    }

    #[test]
    fn governance_metadata_emits_effective_policy_by_default() {
        let file = governance_map("emit_effective_policy");
        let rows = vec![
            SourceRow {
                source_id: "crm".into(),
                row_index: 0,
                values: BTreeMap::from([("id".into(), json!("1"))]),
            },
            SourceRow {
                source_id: "support".into(),
                row_index: 0,
                values: BTreeMap::from([("id".into(), json!("2"))]),
            },
        ];
        let materialized = materialize_with_source_states(&file, &rows, &[]).unwrap();
        let governance = &materialized.conversion_report["governance"];
        assert_eq!(governance["effective_sensitivity_rank"], json!(5));
        assert_eq!(
            governance["effective_sensitivity_labels"],
            json!(["restricted"])
        );
        assert_eq!(
            governance["access_policy_ids"],
            json!(["hipaa", "internal"])
        );
    }

    #[test]
    fn governance_policy_rejects_mixed_sensitivity_when_requested() {
        let file = governance_map("reject_on_mixed_sensitivity");
        let rows = vec![
            SourceRow {
                source_id: "crm".into(),
                row_index: 0,
                values: BTreeMap::from([("id".into(), json!("1"))]),
            },
            SourceRow {
                source_id: "support".into(),
                row_index: 0,
                values: BTreeMap::from([("id".into(), json!("2"))]),
            },
        ];
        let err = materialize_with_source_states(&file, &rows, &[]).unwrap_err();
        assert!(err.contains("mixed source sensitivity"));
    }

    #[test]
    fn replay_claimed_source_validates_fingerprints() {
        let dir = std::env::temp_dir().join(format!("cove-map-replay-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("crm.csv");
        fs::write(&path, "id\n1\n").unwrap();
        let inputs = read_source_inputs(&[path]).unwrap();
        let state = &inputs.states[0];
        let mut file = two_source_identity_map(Vec::new());
        file.sections[0] = test_section(
            SectionKind::MapSourceCatalog,
            json!({
                "mapping_id": "people-map",
                "mapping_version": "test/v1",
                "sources": [{
                    "source_id": "crm",
                    "row_identity_rules": ["person_by_id"],
                    "schema_fingerprint": state.schema_fingerprint,
                    "snapshot_digest": state.snapshot_digest,
                    "replay_claimed": true
                }]
            }),
        );
        validate_source_inputs(&file, &inputs.states).unwrap();
        file.sections[0] = test_section(
            SectionKind::MapSourceCatalog,
            json!({
                "mapping_id": "people-map",
                "mapping_version": "test/v1",
                "sources": [{
                    "source_id": "crm",
                    "row_identity_rules": ["person_by_id"],
                    "schema_fingerprint": state.schema_fingerprint,
                    "snapshot_digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
                    "replay_claimed": true
                }]
            }),
        );
        assert!(validate_source_inputs(&file, &inputs.states).is_err());
        assert!(validate_source_inputs(&file, &[]).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn build_cove_o_emits_valid_object_temporal_file() {
        fn section(kind: SectionKind, value: Value) -> CovemapSection {
            let payload = serde_json::to_vec_pretty(&covemap_payload_value(kind, value)).unwrap();
            CovemapSection {
                entry: CovemapSectionEntryV1 {
                    section_id: kind as u32,
                    offset: 0,
                    length: payload.len() as u64,
                    uncompressed_length: payload.len() as u64,
                    compression: 0,
                    payload_encoding: CovemapPayloadEncodingV2::CoveMapJsonV2 as u8,
                    required: true,
                    reserved: 0,
                    checksum: 0,
                },
                payload,
            }
        }
        let file = CovemapFile {
            header: CovemapHeaderV1::new([0x42; 16], 0),
            mapping_version: "test/v1".into(),
            sections: vec![
                section(
                    SectionKind::MapSourceCatalog,
                    json!({
                        "mapping_id": "people-map",
                        "mapping_version": "test/v1",
                        "sources": [{
                            "source_id": "people",
                            "row_identity_rules": ["person_by_id"]
                        }]
                    }),
                ),
                section(
                    SectionKind::MapFunctionRegistry,
                    json!({
                        "mapping_id": "people-map",
                        "mapping_version": "test/v1",
                        "functions": [{
                            "function_id": "identity",
                            "version": "1",
                            "deterministic": true,
                            "dependency": "pure"
                        }]
                    }),
                ),
                section(
                    SectionKind::MapIdentityRuleCatalog,
                    json!({
                        "mapping_id": "people-map",
                        "mapping_version": "test/v1",
                        "identity_rules": [{
                            "rule_id": "person_by_id",
                            "object_type": "Person",
                            "semantic_role": "subject",
                            "confidence_class": "authoritative",
                            "candidate_only": false,
                            "property_conflicts_declared": true,
                            "function_ids": ["identity"],
                            "join_keys": [{
                                "role_id": "person_id",
                                "source_column": "id",
                                "logical_type": "utf8",
                                "canonicalization": "identity",
                                "null_policy": "reject",
                                "ordering": "declared"
                            }]
                        }],
                        "do_not_merge": []
                    }),
                ),
                section(
                    SectionKind::MapRowSemanticsCatalog,
                    json!({
                        "mapping_id": "people-map",
                        "mapping_version": "test/v1",
                        "rules": [{
                            "rule_id": "upsert_person",
                            "source_id": "people",
                            "identity_rule_id": "person_by_id",
                            "row_semantics_kind": "Object",
                            "assertion_kinds": ["object", "property", "evidence"],
                            "function_ids": ["identity"],
                            "output_assertion_ids": [],
                            "association_endpoints": [],
                            "property_bindings": [{
                                "assertion_id": "name_assertion",
                                "property_id": "name",
                                "property_name": "name",
                                "source_column": "name",
                                "logical_type": "utf8"
                            }]
                        }]
                    }),
                ),
            ],
            postscript: CovemapPostscriptV1 {
                required_features: FEATURE_SEMANTIC_MAP,
                optional_features: 0,
                file_len: 0,
                header_offset: 0,
                header_length: 0,
                checksum: 0,
            },
        };
        let rows = vec![
            SourceRow {
                source_id: "people".into(),
                row_index: 0,
                values: BTreeMap::from([("id".into(), json!("1")), ("name".into(), json!("Ada"))]),
            },
            SourceRow {
                source_id: "people".into(),
                row_index: 1,
                values: BTreeMap::from([
                    ("id".into(), json!("2")),
                    ("name".into(), json!("Linus")),
                ]),
            },
        ];
        let bytes = build_cove_o(&file, &rows).unwrap();
        let report = validate_bytes_with_options(
            &bytes,
            ValidationOptions {
                semantic: true,
                verify_digests: false,
                allow_unknown_optional_extensions: true,
                ..ValidationOptions::default()
            },
        )
        .unwrap();
        assert_eq!(
            report.validated.header.required_features & FEATURE_SEMANTIC_MAP,
            0
        );
        assert_ne!(
            report.validated.header.optional_features & FEATURE_SEMANTIC_MAP,
            0
        );
        assert!(report
            .validated
            .footer
            .sections
            .iter()
            .filter(|entry| {
                matches!(
                    SectionKind::from_u16(entry.section_kind),
                    Some(
                        SectionKind::MapSourceCatalog
                            | SectionKind::MapFunctionRegistry
                            | SectionKind::MapIdentityRuleCatalog
                            | SectionKind::MapRowSemanticsCatalog
                            | SectionKind::MapAssertionLog
                            | SectionKind::MapIdentityEquivalenceIndex
                            | SectionKind::MapEvidenceIndex
                            | SectionKind::MapConversionReport
                    )
                )
            })
            .all(|entry| entry.required_features & FEATURE_SEMANTIC_MAP == 0
                && entry.optional_features & FEATURE_SEMANTIC_MAP != 0));
        let kinds = report
            .validated
            .footer
            .sections
            .iter()
            .map(|entry| SectionKind::from_u16(entry.section_kind).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![
                SectionKind::MapSourceCatalog,
                SectionKind::MapFunctionRegistry,
                SectionKind::MapIdentityRuleCatalog,
                SectionKind::MapRowSemanticsCatalog,
                SectionKind::ObjectTypeCatalog,
                SectionKind::TemporalSegmentIndex,
                SectionKind::TemporalSegmentData,
                SectionKind::TrustManifest,
                SectionKind::MapAssertionLog,
                SectionKind::MapIdentityEquivalenceIndex,
                SectionKind::MapEvidenceIndex,
                SectionKind::MapConversionReport,
            ]
        );
        let segment_entry = report
            .validated
            .footer
            .sections
            .iter()
            .find(|entry| entry.section_kind == SectionKind::TemporalSegmentData as u16)
            .unwrap();
        let segment_bytes = compression::section_payload(&bytes, segment_entry).unwrap();
        let segment = TemporalSegmentData::parse(&segment_bytes).unwrap();
        assert_eq!(segment.header.column_count, 1);
        assert_eq!(segment.property_columns.len(), 1);
        assert_eq!(segment.property_columns[0].page_index.entries.len(), 1);

        let mut projected_file = file.clone();
        projected_file.sections.push(section(
            SectionKind::MapProjectionCatalog,
            json!({
                "mapping_id": "people-map",
                "mapping_version": "test/v1",
                "projections": [{
                    "projection_id": "people_names.v1",
                    "output_table": "people_names",
                    "row_grain": "one_row_per_object",
                    "anchor": {"object_type": "Person"},
                    "temporal_mode": {"as_of": "latest_committed"},
                    "multi_value_policy": "reject",
                    "columns": [
                        {"name": "person_goid", "value": "object.goid"},
                        {"name": "name", "value": "Person.name"}
                    ],
                    "output_modes": ["json"]
                }]
            }),
        ));
        let projected = project_rows(&projected_file, &rows).unwrap();
        assert_eq!(projected["rows"].as_array().unwrap().len(), 2);
        assert_eq!(projected["rows"][0]["name"], json!("Ada"));
    }
}
