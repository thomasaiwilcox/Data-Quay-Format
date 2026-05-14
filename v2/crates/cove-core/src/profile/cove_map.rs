//! Cove Format (COVE) v2.0 — COVE-MAP embedded-section reference schema.
//!
//! Spec §70 fixes the COVE-MAP validation boundary but leaves exact reusable
//! mapping-definition payload bodies to a companion schema specification or a
//! required extension. The reference implementation therefore validates
//! embedded `MAP_*` sections using a small JSON-backed schema that captures the
//! normative cross-reference rules from Spec §73.6.

use std::collections::BTreeMap;

use crate::{constants::SectionKind, CoveError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapSourceEntry {
    pub source_id: String,
    pub schema_fingerprint: Option<String>,
    pub snapshot_digest: Option<String>,
    pub row_identity_rules: Vec<String>,
    pub replay_claimed: bool,
    pub source_priority: Option<i64>,
    pub sensitivity_label: Option<String>,
    pub sensitivity_rank: Option<i64>,
    pub access_policy_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapSourceCatalog {
    pub mapping_id: String,
    pub mapping_version: String,
    pub governance_reconciliation_policy: String,
    pub sources: Vec<MapSourceEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapFunctionEntry {
    pub function_id: String,
    pub version: String,
    pub deterministic: bool,
    pub dependency: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapFunctionRegistry {
    pub mapping_id: String,
    pub mapping_version: String,
    pub functions: Vec<MapFunctionEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapJoinKeyComponent {
    pub role_id: String,
    pub source_column: String,
    pub logical_type: String,
    pub canonicalization: String,
    pub null_policy: String,
    pub ordering: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapIdentityRule {
    pub rule_id: String,
    pub object_type: String,
    pub semantic_role: String,
    pub confidence_class: String,
    pub auto_merge: Option<bool>,
    pub candidate_only: bool,
    pub property_conflicts_declared: bool,
    pub function_ids: Vec<String>,
    pub join_keys: Vec<MapJoinKeyComponent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapDoNotMergeConstraint {
    pub left_identity: String,
    pub right_identity: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapIdentityRuleCatalog {
    pub mapping_id: String,
    pub mapping_version: String,
    pub identity_rules: Vec<MapIdentityRule>,
    pub do_not_merge: Vec<MapDoNotMergeConstraint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceOperationKind {
    Fact,
    Insert,
    Upsert,
    PatchProperty,
    ReplaceObjectState,
    CloseAssociation,
    ExpireAndCreate,
    TombstoneObject,
    TombstoneProperty,
    TombstoneAssociation,
    RedactEvidence,
    EvidenceOnly,
    Correction,
}

impl SourceOperationKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "Fact" => Some(Self::Fact),
            "Insert" => Some(Self::Insert),
            "Upsert" => Some(Self::Upsert),
            "PatchProperty" => Some(Self::PatchProperty),
            "ReplaceObjectState" => Some(Self::ReplaceObjectState),
            "CloseAssociation" => Some(Self::CloseAssociation),
            "ExpireAndCreate" => Some(Self::ExpireAndCreate),
            "TombstoneObject" => Some(Self::TombstoneObject),
            "TombstoneProperty" => Some(Self::TombstoneProperty),
            "TombstoneAssociation" => Some(Self::TombstoneAssociation),
            "RedactEvidence" => Some(Self::RedactEvidence),
            "EvidenceOnly" => Some(Self::EvidenceOnly),
            "Correction" => Some(Self::Correction),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Fact => "Fact",
            Self::Insert => "Insert",
            Self::Upsert => "Upsert",
            Self::PatchProperty => "PatchProperty",
            Self::ReplaceObjectState => "ReplaceObjectState",
            Self::CloseAssociation => "CloseAssociation",
            Self::ExpireAndCreate => "ExpireAndCreate",
            Self::TombstoneObject => "TombstoneObject",
            Self::TombstoneProperty => "TombstoneProperty",
            Self::TombstoneAssociation => "TombstoneAssociation",
            Self::RedactEvidence => "RedactEvidence",
            Self::EvidenceOnly => "EvidenceOnly",
            Self::Correction => "Correction",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapRowSemanticRule {
    pub rule_id: String,
    pub source_id: String,
    pub identity_rule_id: String,
    pub row_semantics_kind: String,
    pub source_operation_kind: SourceOperationKind,
    pub assertion_kinds: Vec<String>,
    pub tombstone_target: Option<String>,
    pub record_kind: String,
    pub temporal_policy: String,
    pub conflict_policy: String,
    pub function_ids: Vec<String>,
    pub output_assertion_ids: Vec<String>,
    pub association_endpoints: Vec<String>,
    pub property_bindings: Vec<MapPropertyBinding>,
    pub association_bindings: Vec<MapAssociationBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapPropertyBinding {
    pub assertion_id: String,
    pub property_id: String,
    pub property_name: String,
    pub source_column: String,
    pub logical_type: String,
    pub physical_kind: String,
    pub value_expression: String,
    pub nullable: bool,
    pub missing_policy: String,
    pub conflict_policy: String,
    pub source_priority: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapAssociationBinding {
    pub assertion_id: String,
    pub association_type: String,
    pub target_identity_rule_id: String,
    pub source_identity_rule_id: String,
    pub source_role: String,
    pub target_role: String,
    pub source_endpoint_expression: String,
    pub target_endpoint_expression: String,
    pub valid_from_expression: Option<String>,
    pub valid_to_expression: Option<String>,
    pub cardinality_policy: String,
    pub missing_policy: String,
    pub link_object_materialization: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapRowSemanticsCatalog {
    pub mapping_id: String,
    pub mapping_version: String,
    pub rules: Vec<MapRowSemanticRule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapAssertionEntry {
    pub assertion_id: String,
    pub output_object_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapAssertionLog {
    pub mapping_id: String,
    pub mapping_version: String,
    pub assertions: Vec<MapAssertionEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapEquivalencePair {
    pub left_identity: String,
    pub right_identity: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapIdentityEquivalenceIndex {
    pub mapping_id: String,
    pub mapping_version: String,
    pub equivalences: Vec<MapEquivalencePair>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapEvidenceEntry {
    pub source_id: String,
    pub source_row_identity: String,
    pub rule_id: String,
    pub assertion_id: String,
    pub output_object_id: String,
    pub observed_schema_fingerprint: Option<String>,
    pub observed_snapshot_digest: Option<String>,
    pub operation_metadata: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapEvidenceIndex {
    pub mapping_id: String,
    pub mapping_version: String,
    pub entries: Vec<MapEvidenceEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapObservedSourceState {
    pub source_id: String,
    pub schema_fingerprint: Option<String>,
    pub snapshot_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapConversionReport {
    pub mapping_id: String,
    pub mapping_version: String,
    pub sources: Vec<MapObservedSourceState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapProjectionEntry {
    pub projection_id: String,
    pub assertion_ids: Vec<String>,
    pub output_table: Option<String>,
    pub row_grain: Option<String>,
    pub anchor: Option<MapProjectionAnchor>,
    pub temporal_mode: Option<String>,
    pub columns: Vec<MapProjectionColumn>,
    pub multi_value_policy: Option<String>,
    pub missing_policy: String,
    pub ordering: Vec<String>,
    pub evidence_policy: String,
    pub output_modes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapProjectionAnchor {
    pub object_type: Option<String>,
    pub association_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapProjectionColumn {
    pub name: String,
    pub value: String,
    pub logical_type: Option<String>,
    pub nested_shape: Option<String>,
    pub conflict_policy: String,
    pub missing_policy: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapProjectionCatalog {
    pub mapping_id: String,
    pub mapping_version: String,
    pub projections: Vec<MapProjectionEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EmbeddedMapSection {
    SourceCatalog(MapSourceCatalog),
    FunctionRegistry(MapFunctionRegistry),
    IdentityRuleCatalog(MapIdentityRuleCatalog),
    RowSemanticsCatalog(MapRowSemanticsCatalog),
    AssertionLog(MapAssertionLog),
    IdentityEquivalenceIndex(MapIdentityEquivalenceIndex),
    EvidenceIndex(MapEvidenceIndex),
    ConversionReport(MapConversionReport),
    ProjectionCatalog(MapProjectionCatalog),
}

mod embedded;

pub fn parse_embedded_section(
    kind: SectionKind,
    bytes: &[u8],
) -> Result<EmbeddedMapSection, CoveError> {
    embedded::parse_embedded_section(kind, bytes)
}

pub fn validate_embedded_sections(sections: &[EmbeddedMapSection]) -> Result<(), CoveError> {
    embedded::validate_embedded_sections(sections)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn parse_json(kind: SectionKind, value: Value) -> EmbeddedMapSection {
        parse_embedded_section(
            kind,
            &serde_json::to_vec_pretty(&payload(kind, value)).unwrap(),
        )
        .unwrap()
    }

    fn parse_json_result(kind: SectionKind, value: Value) -> Result<EmbeddedMapSection, CoveError> {
        parse_embedded_section(
            kind,
            &serde_json::to_vec_pretty(&payload(kind, value)).unwrap(),
        )
    }

    fn payload(kind: SectionKind, mut value: Value) -> Value {
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

    fn row_rule_with_operation(
        operation: &str,
        row_semantics_kind: &str,
        assertion_kinds: Vec<&str>,
        tombstone_target: Option<&str>,
        property_binding: bool,
        association_binding: bool,
    ) -> Value {
        let mut rule = json!({
            "rule_id": "upsert_customer",
            "source_id": "crm.customers",
            "identity_rule_id": "customer_identity",
            "row_semantics_kind": row_semantics_kind,
            "source_operation_kind": operation,
            "assertion_kinds": assertion_kinds,
            "function_ids": ["trim_lower"],
            "output_assertion_ids": ["assert_customer_name"],
            "association_endpoints": []
        });
        let object = rule.as_object_mut().unwrap();
        if let Some(target) = tombstone_target {
            object.insert("tombstone_target".into(), json!(target));
        }
        if property_binding {
            object.insert(
                "property_bindings".into(),
                json!([{
                    "assertion_id": "assert_customer_name",
                    "property_id": "name",
                    "property_name": "name",
                    "source_column": "name",
                    "logical_type": "utf8"
                }]),
            );
        }
        if association_binding {
            object.insert(
                "association_bindings".into(),
                json!([{
                    "assertion_id": "assert_customer_name",
                    "association_type": "member_of",
                    "target_identity_rule_id": "customer_identity"
                }]),
            );
        }
        rule
    }

    fn replace_row_rule(sections: &mut [EmbeddedMapSection], rule: Value) {
        sections[3] = parse_json(
            SectionKind::MapRowSemanticsCatalog,
            json!({
                "mapping_id": "customer-map",
                "mapping_version": "2026.05",
                "rules": [rule]
            }),
        );
    }

    fn valid_sections() -> Vec<EmbeddedMapSection> {
        vec![
            parse_json(
                SectionKind::MapSourceCatalog,
                json!({
                    "mapping_id": "customer-map",
                    "mapping_version": "2026.05",
                    "sources": [{
                        "source_id": "crm.customers",
                        "schema_fingerprint": "schema-v1",
                        "snapshot_digest": "digest-v1",
                        "row_identity_rules": ["customer_id"],
                        "replay_claimed": true
                    }]
                }),
            ),
            parse_json(
                SectionKind::MapFunctionRegistry,
                json!({
                    "mapping_id": "customer-map",
                    "mapping_version": "2026.05",
                    "functions": [{
                        "function_id": "trim_lower",
                        "version": "1.0.0",
                        "deterministic": true,
                        "dependency": "pure"
                    }]
                }),
            ),
            parse_json(
                SectionKind::MapIdentityRuleCatalog,
                json!({
                    "mapping_id": "customer-map",
                    "mapping_version": "2026.05",
                    "identity_rules": [{
                        "rule_id": "customer_identity",
                        "object_type": "Customer",
                        "semantic_role": "subject",
                        "confidence_class": "authoritative",
                        "candidate_only": false,
                        "property_conflicts_declared": true,
                        "function_ids": ["trim_lower"],
                        "join_keys": [{
                            "role_id": "customer_id",
                            "source_column": "customer_id",
                            "logical_type": "utf8",
                            "canonicalization": "trim_lower",
                            "null_policy": "reject",
                            "ordering": "asc"
                        }]
                    }],
                    "do_not_merge": []
                }),
            ),
            parse_json(
                SectionKind::MapRowSemanticsCatalog,
                json!({
                    "mapping_id": "customer-map",
                    "mapping_version": "2026.05",
                    "rules": [{
                        "rule_id": "upsert_customer",
                        "source_id": "crm.customers",
                        "identity_rule_id": "customer_identity",
                        "row_semantics_kind": "Object",
                        "assertion_kinds": ["object", "property", "evidence"],
                        "function_ids": ["trim_lower"],
                        "output_assertion_ids": ["assert_customer_name"],
                        "association_endpoints": []
                    }]
                }),
            ),
            parse_json(
                SectionKind::MapAssertionLog,
                json!({
                    "mapping_id": "customer-map",
                    "mapping_version": "2026.05",
                    "assertions": [{
                        "assertion_id": "assert_customer_name",
                        "output_object_id": "goid:customer:1"
                    }]
                }),
            ),
            parse_json(
                SectionKind::MapEvidenceIndex,
                json!({
                    "mapping_id": "customer-map",
                    "mapping_version": "2026.05",
                    "entries": [{
                        "source_id": "crm.customers",
                        "source_row_identity": "customer_id=1",
                        "rule_id": "upsert_customer",
                        "assertion_id": "assert_customer_name",
                        "output_object_id": "goid:customer:1",
                        "observed_schema_fingerprint": "schema-v1",
                        "observed_snapshot_digest": "digest-v1"
                    }]
                }),
            ),
        ]
    }

    #[test]
    fn map_source_catalog_parse_rejects_missing_mapping_id() {
        assert_eq!(
            MapSourceCatalog::parse(
                &serde_json::to_vec_pretty(&payload(
                    SectionKind::MapSourceCatalog,
                    json!({
                        "mapping_version": "2026.05",
                        "sources": []
                    })
                ))
                .unwrap()
            ),
            Err(CoveError::MapInvalid)
        );
    }

    #[test]
    fn map_payload_rejects_duplicate_keys_before_value_collapse() {
        let bytes = br#"{"schema_id":"org.coveformat.covemap.v2","schema_id":"org.coveformat.covemap.v2","section_id":60,"mapping_id":"m","mapping_version":"v"}"#;
        assert_eq!(
            parse_embedded_section(SectionKind::MapSourceCatalog, bytes),
            Err(CoveError::MapInvalid)
        );
    }

    #[test]
    fn map_payload_rejects_wrong_section_id() {
        let bytes = br#"{"schema_id":"org.coveformat.covemap.v2","section_id":61,"mapping_id":"customer-map","mapping_version":"2026.05"}"#;
        assert_eq!(
            parse_embedded_section(SectionKind::MapSourceCatalog, bytes),
            Err(CoveError::MapInvalid)
        );
    }

    #[test]
    fn map_payload_rejects_unknown_nested_source_field() {
        assert_eq!(
            parse_json_result(
                SectionKind::MapSourceCatalog,
                json!({
                    "mapping_id": "customer-map",
                    "mapping_version": "2026.05",
                    "sources": [{
                        "source_id": "crm.customers",
                        "row_identity_rules": ["customer_id"],
                        "unexpected_source_field": true
                    }]
                }),
            ),
            Err(CoveError::MapInvalid)
        );
    }

    #[test]
    fn map_payload_rejects_unknown_nested_projection_column_field() {
        assert_eq!(
            parse_json_result(
                SectionKind::MapProjectionCatalog,
                json!({
                    "mapping_id": "customer-map",
                    "mapping_version": "2026.05",
                    "projections": [{
                        "projection_id": "customer_projection",
                        "output_table": "customers",
                        "row_grain": "one_row_per_object",
                        "anchor": {"object_type": "Customer"},
                        "temporal_mode": {"as_of": "latest_committed"},
                        "multi_value_policy": "reject",
                        "columns": [{
                            "name": "goid",
                            "value": "object.goid",
                            "unexpected_column_field": "bad"
                        }],
                        "output_modes": ["json"]
                    }]
                }),
            ),
            Err(CoveError::MapInvalid)
        );
    }

    #[test]
    fn map_payload_accepts_object_extensions() {
        assert!(parse_json_result(
            SectionKind::MapSourceCatalog,
            json!({
                "mapping_id": "customer-map",
                "mapping_version": "2026.05",
                "extension": {"x.example": {"enabled": true}},
                "extensions": {"x.example.audit": {"mode": "strict"}},
                "sources": []
            }),
        )
        .is_ok());
    }

    #[test]
    fn map_payload_rejects_malformed_extension_containers() {
        for payload in [
            json!({
                "mapping_id": "customer-map",
                "mapping_version": "2026.05",
                "extension": "bad",
                "sources": []
            }),
            json!({
                "mapping_id": "customer-map",
                "mapping_version": "2026.05",
                "extensions": [],
                "sources": []
            }),
            json!({
                "mapping_id": "customer-map",
                "mapping_version": "2026.05",
                "extensions": {"x.example": null},
                "sources": []
            }),
        ] {
            assert_eq!(
                parse_json_result(SectionKind::MapSourceCatalog, payload),
                Err(CoveError::MapInvalid)
            );
        }
    }

    #[test]
    fn projection_parse_accepts_nested_shape() {
        let section = parse_json_result(
            SectionKind::MapProjectionCatalog,
            json!({
                "mapping_id": "customer-map",
                "mapping_version": "2026.05",
                "projections": [{
                    "projection_id": "customer_projection",
                    "output_table": "customers",
                    "row_grain": "one_row_per_object",
                    "anchor": {"object_type": "Customer"},
                    "temporal_mode": {"as_of": "latest_committed"},
                    "multi_value_policy": "reject",
                    "columns": [{
                        "name": "tags",
                        "value": "property.tags",
                        "logical_type": "list",
                        "nested_shape": {"type": "list", "item": {"logical_type": "utf8"}}
                    }],
                    "output_modes": ["json", "arrow"]
                }]
            }),
        )
        .unwrap();
        let EmbeddedMapSection::ProjectionCatalog(catalog) = section else {
            panic!("expected projection catalog");
        };
        assert!(catalog.projections[0].columns[0]
            .nested_shape
            .as_deref()
            .unwrap()
            .contains("\"list\""));
    }

    #[test]
    fn projection_parse_rejects_malformed_nested_shape() {
        assert_eq!(
            parse_json_result(
                SectionKind::MapProjectionCatalog,
                json!({
                    "mapping_id": "customer-map",
                    "mapping_version": "2026.05",
                    "projections": [{
                        "projection_id": "customer_projection",
                        "output_table": "customers",
                        "row_grain": "one_row_per_object",
                        "anchor": {"object_type": "Customer"},
                        "temporal_mode": {"as_of": "latest_committed"},
                        "multi_value_policy": "reject",
                        "columns": [{
                            "name": "tags",
                            "value": "property.tags",
                            "logical_type": "list",
                            "nested_shape": []
                        }],
                        "output_modes": ["json", "arrow"]
                    }]
                }),
            ),
            Err(CoveError::MapInvalid)
        );
    }

    #[test]
    fn row_semantics_parse_rejects_missing_assertion_kinds() {
        assert_eq!(
            parse_json_result(
                SectionKind::MapRowSemanticsCatalog,
                json!({
                    "mapping_id": "customer-map",
                    "mapping_version": "2026.05",
                    "rules": [{
                        "rule_id": "upsert_customer",
                        "source_id": "crm.customers",
                        "identity_rule_id": "customer_identity"
                    }]
                }),
            ),
            Err(CoveError::MapInvalid)
        );
    }

    #[test]
    fn row_semantics_parse_rejects_unknown_row_kind() {
        assert_eq!(
            parse_json_result(
                SectionKind::MapRowSemanticsCatalog,
                json!({
                    "mapping_id": "customer-map",
                    "mapping_version": "2026.05",
                    "rules": [{
                        "rule_id": "bad_customer",
                        "source_id": "crm.customers",
                        "identity_rule_id": "customer_identity",
                        "row_semantics_kind": "MaybeObject",
                        "assertion_kinds": ["object"]
                    }]
                }),
            ),
            Err(CoveError::MapInvalid)
        );
    }

    #[test]
    fn row_semantics_parse_rejects_unknown_source_operation_kind() {
        assert_eq!(
            parse_json_result(
                SectionKind::MapRowSemanticsCatalog,
                json!({
                    "mapping_id": "customer-map",
                    "mapping_version": "2026.05",
                    "rules": [{
                        "rule_id": "bad_customer",
                        "source_id": "crm.customers",
                        "identity_rule_id": "customer_identity",
                        "row_semantics_kind": "Object",
                        "source_operation_kind": "MaybePatch",
                        "assertion_kinds": ["object"]
                    }]
                }),
            ),
            Err(CoveError::MapInvalid)
        );
    }

    #[test]
    fn row_semantics_parse_rejects_invalid_tombstone_target() {
        assert_eq!(
            parse_json_result(
                SectionKind::MapRowSemanticsCatalog,
                json!({
                    "mapping_id": "customer-map",
                    "mapping_version": "2026.05",
                    "rules": [{
                        "rule_id": "delete_customer",
                        "source_id": "crm.customers",
                        "identity_rule_id": "customer_identity",
                        "row_semantics_kind": "Tombstone",
                        "assertion_kinds": ["tombstone", "evidence"],
                        "tombstone_target": "foreign_key"
                    }]
                }),
            ),
            Err(CoveError::MapInvalid)
        );
    }

    #[test]
    fn projection_parse_rejects_malformed_policy() {
        assert_eq!(
            parse_json_result(
                SectionKind::MapProjectionCatalog,
                json!({
                    "mapping_id": "customer-map",
                    "mapping_version": "2026.05",
                    "projections": [{
                        "projection_id": "customer_projection",
                        "output_table": "customers",
                        "row_grain": "one_row_per_object",
                        "anchor": {"object_type": "Customer"},
                        "temporal_mode": {"as_of": "latest_committed"},
                        "multi_value_policy": "maybe",
                        "columns": [{"name": "goid", "value": "object.goid"}],
                        "output_modes": ["json"]
                    }]
                }),
            ),
            Err(CoveError::MapInvalid)
        );
    }

    #[test]
    fn embedded_map_validation_rejects_expanded_projection_without_policy() {
        let mut sections = valid_sections();
        sections.push(parse_json(
            SectionKind::MapProjectionCatalog,
            json!({
                "mapping_id": "customer-map",
                "mapping_version": "2026.05",
                "projections": [{
                    "projection_id": "customer_projection",
                    "output_table": "customers",
                    "row_grain": "one_row_per_object",
                    "anchor": {"object_type": "Customer"},
                    "temporal_mode": {"as_of": "latest_committed"},
                    "columns": [{"name": "goid", "value": "object.goid"}],
                    "output_modes": ["json"]
                }]
            }),
        ));
        assert_eq!(
            validate_embedded_sections(&sections),
            Err(CoveError::MapInvalid)
        );
    }

    #[test]
    fn embedded_map_validation_accepts_consistent_sections() {
        assert_eq!(validate_embedded_sections(&valid_sections()), Ok(()));
    }

    #[test]
    fn embedded_map_validation_accepts_all_source_operation_kinds() {
        let cases = [
            (
                "Fact",
                "Object",
                vec!["object", "evidence"],
                None,
                false,
                false,
            ),
            (
                "Insert",
                "Object",
                vec!["object", "evidence"],
                None,
                false,
                false,
            ),
            (
                "Upsert",
                "Object",
                vec!["object", "evidence"],
                None,
                false,
                false,
            ),
            (
                "PatchProperty",
                "Object",
                vec!["object", "property", "evidence"],
                None,
                true,
                false,
            ),
            (
                "ReplaceObjectState",
                "Object",
                vec!["object", "evidence"],
                None,
                false,
                false,
            ),
            (
                "CloseAssociation",
                "Object",
                vec!["object", "association", "evidence"],
                None,
                false,
                true,
            ),
            (
                "ExpireAndCreate",
                "Object",
                vec!["object", "evidence"],
                None,
                false,
                false,
            ),
            (
                "TombstoneObject",
                "Tombstone",
                vec!["tombstone", "evidence"],
                Some("object"),
                false,
                false,
            ),
            (
                "TombstoneProperty",
                "Tombstone",
                vec!["tombstone", "evidence"],
                Some("property"),
                false,
                false,
            ),
            (
                "TombstoneAssociation",
                "Tombstone",
                vec!["tombstone", "evidence"],
                Some("association"),
                false,
                false,
            ),
            (
                "RedactEvidence",
                "EvidenceOnly",
                vec!["evidence"],
                None,
                false,
                false,
            ),
            (
                "EvidenceOnly",
                "EvidenceOnly",
                vec!["evidence"],
                None,
                false,
                false,
            ),
            (
                "Correction",
                "Object",
                vec!["object", "property", "evidence"],
                None,
                true,
                false,
            ),
        ];

        for (operation, row_kind, assertions, target, property, association) in cases {
            let mut sections = valid_sections();
            replace_row_rule(
                &mut sections,
                row_rule_with_operation(
                    operation,
                    row_kind,
                    assertions,
                    target,
                    property,
                    association,
                ),
            );
            assert_eq!(
                validate_embedded_sections(&sections),
                Ok(()),
                "operation {operation} should validate"
            );
        }
    }

    #[test]
    fn embedded_map_validation_rejects_malformed_operation_payloads() {
        let mut sections = valid_sections();
        replace_row_rule(
            &mut sections,
            row_rule_with_operation(
                "PatchProperty",
                "Object",
                vec!["object", "property", "evidence"],
                None,
                false,
                false,
            ),
        );
        assert_eq!(
            validate_embedded_sections(&sections),
            Err(CoveError::MapInvalid)
        );

        let mut sections = valid_sections();
        replace_row_rule(
            &mut sections,
            row_rule_with_operation(
                "CloseAssociation",
                "Object",
                vec!["object", "association", "evidence"],
                None,
                false,
                false,
            ),
        );
        assert_eq!(
            validate_embedded_sections(&sections),
            Err(CoveError::MapInvalid)
        );

        let mut sections = valid_sections();
        replace_row_rule(
            &mut sections,
            row_rule_with_operation(
                "EvidenceOnly",
                "Object",
                vec!["object", "evidence"],
                None,
                false,
                false,
            ),
        );
        assert_eq!(
            validate_embedded_sections(&sections),
            Err(CoveError::MapInvalid)
        );
    }

    #[test]
    fn embedded_map_validation_rejects_undeclared_function_reference() {
        let mut sections = valid_sections();
        sections[1] = parse_json(
            SectionKind::MapFunctionRegistry,
            json!({
                "mapping_id": "customer-map",
                "mapping_version": "2026.05",
                "functions": []
            }),
        );
        assert_eq!(
            validate_embedded_sections(&sections),
            Err(CoveError::MapFunctionUndeclared)
        );
    }

    #[test]
    fn embedded_map_validation_rejects_identity_conflict() {
        let mut sections = valid_sections();
        sections.push(parse_json(
            SectionKind::MapIdentityEquivalenceIndex,
            json!({
                "mapping_id": "customer-map",
                "mapping_version": "2026.05",
                "equivalences": [{
                    "left_identity": "customer:1",
                    "right_identity": "customer:2"
                }]
            }),
        ));
        sections[2] = parse_json(
            SectionKind::MapIdentityRuleCatalog,
            json!({
                "mapping_id": "customer-map",
                "mapping_version": "2026.05",
                "identity_rules": [{
                    "rule_id": "customer_identity",
                    "object_type": "Customer",
                    "semantic_role": "subject",
                    "confidence_class": "authoritative",
                    "candidate_only": false,
                    "property_conflicts_declared": true,
                    "function_ids": ["trim_lower"],
                    "join_keys": [{
                        "role_id": "customer_id",
                        "source_column": "customer_id",
                        "logical_type": "utf8",
                        "canonicalization": "trim_lower",
                        "null_policy": "reject",
                        "ordering": "asc"
                    }]
                }],
                "do_not_merge": [{
                    "left_identity": "customer:1",
                    "right_identity": "customer:2"
                }]
            }),
        );
        assert_eq!(
            validate_embedded_sections(&sections),
            Err(CoveError::MapIdentityConflict)
        );
    }

    #[test]
    fn embedded_map_validation_rejects_stale_source_state() {
        let mut sections = valid_sections();
        sections.push(parse_json(
            SectionKind::MapConversionReport,
            json!({
                "mapping_id": "customer-map",
                "mapping_version": "2026.05",
                "sources": [{
                    "source_id": "crm.customers",
                    "schema_fingerprint": "schema-v2",
                    "snapshot_digest": "digest-v1"
                }]
            }),
        ));
        assert_eq!(
            validate_embedded_sections(&sections),
            Err(CoveError::MapSourceStale)
        );
    }

    #[test]
    fn embedded_map_validation_rejects_invalid_evidence_reference() {
        let mut sections = valid_sections();
        sections[5] = parse_json(
            SectionKind::MapEvidenceIndex,
            json!({
                "mapping_id": "customer-map",
                "mapping_version": "2026.05",
                "entries": [{
                    "source_id": "crm.customers",
                    "source_row_identity": "customer_id=1",
                    "rule_id": "upsert_customer",
                    "assertion_id": "assert_missing",
                    "output_object_id": "goid:customer:1"
                }]
            }),
        );
        assert_eq!(
            validate_embedded_sections(&sections),
            Err(CoveError::MapEvidenceInvalid)
        );
    }
}
