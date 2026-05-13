//! Cove Format (COVE) v2.0 — COVE-MAP embedded-section reference schema.
//!
//! Spec §70 fixes the COVE-MAP validation boundary but leaves exact reusable
//! mapping-definition payload bodies to a companion schema specification or a
//! required extension. The reference implementation therefore validates
//! embedded `MAP_*` sections using a small JSON-backed schema that captures the
//! normative cross-reference rules from Spec §73.6.

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapRowSemanticRule {
    pub rule_id: String,
    pub source_id: String,
    pub identity_rule_id: String,
    pub row_semantics_kind: String,
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
        parse_embedded_section(kind, &serde_json::to_vec_pretty(&value).unwrap()).unwrap()
    }

    fn parse_json_result(kind: SectionKind, value: Value) -> Result<EmbeddedMapSection, CoveError> {
        parse_embedded_section(kind, &serde_json::to_vec_pretty(&value).unwrap())
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
                &serde_json::to_vec_pretty(&json!({
                    "mapping_version": "2026.05",
                    "sources": []
                }))
                .unwrap()
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
