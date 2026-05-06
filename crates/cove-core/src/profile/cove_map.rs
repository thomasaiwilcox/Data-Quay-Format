//! Cove Format (COVE) v1.0 — COVE-MAP embedded-section reference schema.
//!
//! Spec §70 fixes the COVE-MAP validation boundary but leaves exact reusable
//! mapping-definition payload bodies to a companion schema specification or a
//! required extension. The reference implementation therefore validates
//! embedded `MAP_*` sections using a small JSON-backed schema that captures the
//! normative cross-reference rules from Spec §73.6.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

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

impl EmbeddedMapSection {
    fn mapping_id(&self) -> &str {
        match self {
            Self::SourceCatalog(section) => &section.mapping_id,
            Self::FunctionRegistry(section) => &section.mapping_id,
            Self::IdentityRuleCatalog(section) => &section.mapping_id,
            Self::RowSemanticsCatalog(section) => &section.mapping_id,
            Self::AssertionLog(section) => &section.mapping_id,
            Self::IdentityEquivalenceIndex(section) => &section.mapping_id,
            Self::EvidenceIndex(section) => &section.mapping_id,
            Self::ConversionReport(section) => &section.mapping_id,
            Self::ProjectionCatalog(section) => &section.mapping_id,
        }
    }

    fn mapping_version(&self) -> &str {
        match self {
            Self::SourceCatalog(section) => &section.mapping_version,
            Self::FunctionRegistry(section) => &section.mapping_version,
            Self::IdentityRuleCatalog(section) => &section.mapping_version,
            Self::RowSemanticsCatalog(section) => &section.mapping_version,
            Self::AssertionLog(section) => &section.mapping_version,
            Self::IdentityEquivalenceIndex(section) => &section.mapping_version,
            Self::EvidenceIndex(section) => &section.mapping_version,
            Self::ConversionReport(section) => &section.mapping_version,
            Self::ProjectionCatalog(section) => &section.mapping_version,
        }
    }
}

pub fn parse_embedded_section(
    kind: SectionKind,
    bytes: &[u8],
) -> Result<EmbeddedMapSection, CoveError> {
    match kind {
        SectionKind::MapSourceCatalog => {
            MapSourceCatalog::parse(bytes).map(EmbeddedMapSection::SourceCatalog)
        }
        SectionKind::MapFunctionRegistry => {
            MapFunctionRegistry::parse(bytes).map(EmbeddedMapSection::FunctionRegistry)
        }
        SectionKind::MapIdentityRuleCatalog => {
            MapIdentityRuleCatalog::parse(bytes).map(EmbeddedMapSection::IdentityRuleCatalog)
        }
        SectionKind::MapRowSemanticsCatalog => {
            MapRowSemanticsCatalog::parse(bytes).map(EmbeddedMapSection::RowSemanticsCatalog)
        }
        SectionKind::MapAssertionLog => {
            MapAssertionLog::parse(bytes).map(EmbeddedMapSection::AssertionLog)
        }
        SectionKind::MapIdentityEquivalenceIndex => MapIdentityEquivalenceIndex::parse(bytes)
            .map(EmbeddedMapSection::IdentityEquivalenceIndex),
        SectionKind::MapEvidenceIndex => {
            MapEvidenceIndex::parse(bytes).map(EmbeddedMapSection::EvidenceIndex)
        }
        SectionKind::MapConversionReport => {
            MapConversionReport::parse(bytes).map(EmbeddedMapSection::ConversionReport)
        }
        SectionKind::MapProjectionCatalog => {
            MapProjectionCatalog::parse(bytes).map(EmbeddedMapSection::ProjectionCatalog)
        }
        _ => Err(CoveError::MapInvalid),
    }
}

pub fn validate_embedded_sections(sections: &[EmbeddedMapSection]) -> Result<(), CoveError> {
    if sections.is_empty() {
        return Ok(());
    }

    let mapping_id = sections[0].mapping_id();
    let mapping_version = sections[0].mapping_version();
    for section in sections.iter().skip(1) {
        if section.mapping_id() != mapping_id || section.mapping_version() != mapping_version {
            return Err(CoveError::MapInvalid);
        }
    }

    let mut sources = BTreeMap::<String, MapSourceEntry>::new();
    let mut function_ids = BTreeSet::<String>::new();
    let mut referenced_function_ids = BTreeSet::<String>::new();
    let mut identity_rule_ids = BTreeSet::<String>::new();
    let mut do_not_merge = BTreeSet::<(String, String)>::new();
    let mut row_rules = BTreeMap::<String, MapRowSemanticRule>::new();
    let mut assertion_ids = BTreeSet::<String>::new();
    let mut output_object_ids = BTreeSet::<String>::new();
    let mut equivalence_pairs = Vec::<(String, String)>::new();
    let mut evidence_entries = Vec::<MapEvidenceEntry>::new();
    let mut observed_sources = Vec::<MapObservedSourceState>::new();
    let mut projections = Vec::<MapProjectionEntry>::new();

    for section in sections {
        match section {
            EmbeddedMapSection::SourceCatalog(catalog) => {
                for source in &catalog.sources {
                    if sources
                        .insert(source.source_id.clone(), source.clone())
                        .is_some()
                    {
                        return Err(CoveError::MapInvalid);
                    }
                }
            }
            EmbeddedMapSection::FunctionRegistry(registry) => {
                for function in &registry.functions {
                    if !function_ids.insert(function.function_id.clone()) {
                        return Err(CoveError::MapInvalid);
                    }
                    if !function.deterministic
                        || matches!(
                            function.dependency.as_str(),
                            "random"
                                | "wall_clock"
                                | "locale_default"
                                | "network"
                                | "mutable_external"
                        )
                    {
                        return Err(CoveError::MapInvalid);
                    }
                }
            }
            EmbeddedMapSection::IdentityRuleCatalog(catalog) => {
                for rule in &catalog.identity_rules {
                    if !identity_rule_ids.insert(rule.rule_id.clone()) {
                        return Err(CoveError::MapInvalid);
                    }
                    referenced_function_ids.extend(rule.function_ids.iter().cloned());
                }
                for constraint in &catalog.do_not_merge {
                    let pair =
                        normalize_pair(&constraint.left_identity, &constraint.right_identity)?;
                    do_not_merge.insert(pair);
                }
            }
            EmbeddedMapSection::RowSemanticsCatalog(catalog) => {
                for rule in &catalog.rules {
                    if row_rules
                        .insert(rule.rule_id.clone(), rule.clone())
                        .is_some()
                    {
                        return Err(CoveError::MapInvalid);
                    }
                    referenced_function_ids.extend(rule.function_ids.iter().cloned());
                }
            }
            EmbeddedMapSection::AssertionLog(log) => {
                for assertion in &log.assertions {
                    if !assertion_ids.insert(assertion.assertion_id.clone()) {
                        return Err(CoveError::MapInvalid);
                    }
                    if !output_object_ids.insert(assertion.output_object_id.clone()) {
                        return Err(CoveError::MapInvalid);
                    }
                }
            }
            EmbeddedMapSection::IdentityEquivalenceIndex(index) => {
                for pair in &index.equivalences {
                    equivalence_pairs
                        .push(normalize_pair(&pair.left_identity, &pair.right_identity)?);
                }
            }
            EmbeddedMapSection::EvidenceIndex(index) => {
                evidence_entries.extend(index.entries.iter().cloned());
            }
            EmbeddedMapSection::ConversionReport(report) => {
                observed_sources.extend(report.sources.iter().cloned());
            }
            EmbeddedMapSection::ProjectionCatalog(catalog) => {
                projections.extend(catalog.projections.iter().cloned());
            }
        }
    }

    for function_id in referenced_function_ids {
        if !function_ids.contains(&function_id) {
            return Err(CoveError::MapFunctionUndeclared);
        }
    }

    for rule in row_rules.values() {
        if !sources.contains_key(&rule.source_id)
            || !identity_rule_ids.contains(&rule.identity_rule_id)
        {
            return Err(CoveError::MapInvalid);
        }
        validate_row_semantic_rule_shape(rule)?;
        if !assertion_ids.is_empty()
            && rule
                .output_assertion_ids
                .iter()
                .any(|assertion_id| !assertion_ids.contains(assertion_id))
        {
            return Err(CoveError::MapInvalid);
        }
        if rule
            .association_endpoints
            .iter()
            .any(|identity_id| !identity_rule_ids.contains(identity_id))
        {
            return Err(CoveError::MapInvalid);
        }
        if !assertion_ids.is_empty()
            && rule
                .property_bindings
                .iter()
                .any(|binding| !assertion_ids.contains(&binding.assertion_id))
        {
            return Err(CoveError::MapInvalid);
        }
        if !assertion_ids.is_empty()
            && rule
                .association_bindings
                .iter()
                .any(|binding| !assertion_ids.contains(&binding.assertion_id))
        {
            return Err(CoveError::MapInvalid);
        }
        if rule.association_bindings.iter().any(|binding| {
            !identity_rule_ids.contains(&binding.target_identity_rule_id)
                || (!binding.source_identity_rule_id.is_empty()
                    && !identity_rule_ids.contains(&binding.source_identity_rule_id))
        }) {
            return Err(CoveError::MapInvalid);
        }
    }

    for pair in equivalence_pairs {
        if do_not_merge.contains(&pair) {
            return Err(CoveError::MapIdentityConflict);
        }
    }

    for source_state in observed_sources {
        let Some(source) = sources.get(&source_state.source_id) else {
            return Err(CoveError::MapSourceStale);
        };
        if source_state
            .schema_fingerprint
            .as_ref()
            .zip(source.schema_fingerprint.as_ref())
            .is_some_and(|(observed, expected)| observed != expected)
            || source_state
                .snapshot_digest
                .as_ref()
                .zip(source.snapshot_digest.as_ref())
                .is_some_and(|(observed, expected)| observed != expected)
        {
            return Err(CoveError::MapSourceStale);
        }
    }

    for evidence in evidence_entries {
        let Some(source) = sources.get(&evidence.source_id) else {
            return Err(CoveError::MapEvidenceInvalid);
        };
        if !row_rules.contains_key(&evidence.rule_id)
            || !assertion_ids.contains(&evidence.assertion_id)
            || !output_object_ids.contains(&evidence.output_object_id)
        {
            return Err(CoveError::MapEvidenceInvalid);
        }
        if evidence
            .observed_schema_fingerprint
            .as_ref()
            .zip(source.schema_fingerprint.as_ref())
            .is_some_and(|(observed, expected)| observed != expected)
            || evidence
                .observed_snapshot_digest
                .as_ref()
                .zip(source.snapshot_digest.as_ref())
                .is_some_and(|(observed, expected)| observed != expected)
        {
            return Err(CoveError::MapSourceStale);
        }
    }

    for projection in projections {
        if !assertion_ids.is_empty()
            && projection
                .assertion_ids
                .iter()
                .any(|assertion_id| !assertion_ids.contains(assertion_id))
        {
            return Err(CoveError::MapEvidenceInvalid);
        }
        let expanded = projection.output_table.is_some()
            || projection.row_grain.is_some()
            || projection.anchor.is_some()
            || !projection.columns.is_empty()
            || !projection.output_modes.is_empty();
        if expanded {
            if projection.output_table.is_none()
                || projection.row_grain.is_none()
                || projection.anchor.is_none()
                || projection.temporal_mode.is_none()
                || projection.multi_value_policy.is_none()
                || projection.columns.is_empty()
                || projection.output_modes.is_empty()
            {
                return Err(CoveError::MapInvalid);
            }
            let row_grain = projection
                .row_grain
                .as_deref()
                .ok_or(CoveError::MapInvalid)?;
            if !is_valid_projection_row_grain(row_grain) {
                return Err(CoveError::MapInvalid);
            }
            if !projection
                .temporal_mode
                .as_deref()
                .is_some_and(is_valid_temporal_mode)
            {
                return Err(CoveError::MapInvalid);
            }
            if !projection
                .multi_value_policy
                .as_deref()
                .is_some_and(is_valid_multi_value_policy)
            {
                return Err(CoveError::MapInvalid);
            }
            if projection
                .output_modes
                .iter()
                .any(|mode| !is_valid_projection_output_mode(mode))
            {
                return Err(CoveError::MapInvalid);
            }
            let anchor = projection.anchor.as_ref().ok_or(CoveError::MapInvalid)?;
            if anchor.object_type.is_some() == anchor.association_type.is_some() {
                return Err(CoveError::MapInvalid);
            }
            match row_grain {
                "one_row_per_object"
                | "one_row_per_property_version"
                | "one_row_per_event_object"
                | "one_row_per_object_as_of_time"
                    if anchor.object_type.is_none() =>
                {
                    return Err(CoveError::MapInvalid);
                }
                "one_row_per_association" | "one_row_per_link_object"
                    if anchor.association_type.is_none() =>
                {
                    return Err(CoveError::MapInvalid);
                }
                _ => {}
            }
        }
    }

    Ok(())
}

fn validate_row_semantic_rule_shape(rule: &MapRowSemanticRule) -> Result<(), CoveError> {
    let has = |kind: &str| rule.assertion_kinds.iter().any(|value| value == kind);
    match rule.row_semantics_kind.as_str() {
        "Object" | "EventObject" | "LinkObject" => {
            if !has("object") {
                return Err(CoveError::MapInvalid);
            }
        }
        "AssociationOnly" => {
            if !has("association") || has("object") {
                return Err(CoveError::MapInvalid);
            }
        }
        "Composite" | "Dispatched" => {
            if rule.assertion_kinds.len() < 2 {
                return Err(CoveError::MapInvalid);
            }
        }
        "ProjectionOnly" => {
            if rule
                .assertion_kinds
                .iter()
                .any(|kind| !matches!(kind.as_str(), "projection" | "evidence" | "candidate_match"))
            {
                return Err(CoveError::MapInvalid);
            }
        }
        "EvidenceOnly" => {
            if rule
                .assertion_kinds
                .iter()
                .any(|kind| !matches!(kind.as_str(), "evidence" | "candidate_match" | "conflict"))
            {
                return Err(CoveError::MapInvalid);
            }
        }
        "Tombstone" => {
            if !has("tombstone") || rule.tombstone_target.is_none() {
                return Err(CoveError::MapInvalid);
            }
        }
        "KeyValueFragment" => {
            if !has("property") {
                return Err(CoveError::MapInvalid);
            }
        }
        _ => return Err(CoveError::MapInvalid),
    }
    Ok(())
}

impl MapSourceCatalog {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let root = parse_root(bytes)?;
        let object = as_object(&root)?;
        let (mapping_id, mapping_version) = parse_mapping_identity(object)?;
        let governance_reconciliation_policy =
            optional_non_empty_str(object, "governance_reconciliation_policy")?
                .unwrap_or_else(|| "emit_effective_policy".to_string());
        if !matches!(
            governance_reconciliation_policy.as_str(),
            "emit_effective_policy" | "reject_on_mixed_sensitivity"
        ) {
            return Err(CoveError::MapInvalid);
        }
        let mut sources = Vec::new();
        if let Some(values) = optional_array(object, "sources")? {
            for value in values {
                let entry = as_object(value)?;
                let row_identity_rules = string_list(entry, "row_identity_rules")?;
                if row_identity_rules.is_empty() {
                    return Err(CoveError::MapInvalid);
                }
                let schema_fingerprint = optional_non_empty_str(entry, "schema_fingerprint")?;
                let snapshot_digest = optional_non_empty_str(entry, "snapshot_digest")?;
                let replay_claimed = optional_bool(entry, "replay_claimed", false)?;
                if replay_claimed && (schema_fingerprint.is_none() || snapshot_digest.is_none()) {
                    return Err(CoveError::MapInvalid);
                }
                sources.push(MapSourceEntry {
                    source_id: required_non_empty_str(entry, "source_id")?,
                    schema_fingerprint,
                    snapshot_digest,
                    row_identity_rules,
                    replay_claimed,
                    source_priority: optional_i64(entry, "source_priority")?,
                    sensitivity_label: optional_non_empty_str(entry, "sensitivity_label")?,
                    sensitivity_rank: optional_i64(entry, "sensitivity_rank")?,
                    access_policy_ids: optional_string_list(entry, "access_policy_ids")?,
                });
            }
        }
        Ok(Self {
            mapping_id,
            mapping_version,
            governance_reconciliation_policy,
            sources,
        })
    }
}

impl MapFunctionRegistry {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let root = parse_root(bytes)?;
        let object = as_object(&root)?;
        let (mapping_id, mapping_version) = parse_mapping_identity(object)?;
        let mut functions = Vec::new();
        if let Some(values) = optional_array(object, "functions")? {
            for value in values {
                let entry = as_object(value)?;
                functions.push(MapFunctionEntry {
                    function_id: required_non_empty_str(entry, "function_id")?,
                    version: required_non_empty_str(entry, "version")?,
                    deterministic: required_bool(entry, "deterministic")?,
                    dependency: optional_non_empty_str(entry, "dependency")?
                        .unwrap_or_else(|| "pure".to_string()),
                });
            }
        }
        Ok(Self {
            mapping_id,
            mapping_version,
            functions,
        })
    }
}

impl MapIdentityRuleCatalog {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let root = parse_root(bytes)?;
        let object = as_object(&root)?;
        let (mapping_id, mapping_version) = parse_mapping_identity(object)?;
        let mut identity_rules = Vec::new();
        if let Some(values) = optional_array(object, "identity_rules")? {
            for value in values {
                let entry = as_object(value)?;
                let mut join_keys = Vec::new();
                for join_key in required_array(entry, "join_keys")? {
                    let join_key = as_object(join_key)?;
                    join_keys.push(MapJoinKeyComponent {
                        role_id: required_non_empty_str(join_key, "role_id")?,
                        source_column: required_non_empty_str(join_key, "source_column")?,
                        logical_type: required_non_empty_str(join_key, "logical_type")?,
                        canonicalization: required_non_empty_str(join_key, "canonicalization")?,
                        null_policy: required_non_empty_str(join_key, "null_policy")?,
                        ordering: required_non_empty_str(join_key, "ordering")?,
                    });
                }
                if join_keys.is_empty() {
                    return Err(CoveError::MapInvalid);
                }
                identity_rules.push(MapIdentityRule {
                    rule_id: required_non_empty_str(entry, "rule_id")?,
                    object_type: required_non_empty_str(entry, "object_type")?,
                    semantic_role: required_non_empty_str(entry, "semantic_role")?,
                    confidence_class: required_non_empty_str(entry, "confidence_class")?,
                    auto_merge: optional_bool_value(entry, "auto_merge")?,
                    candidate_only: optional_bool(entry, "candidate_only", false)?,
                    property_conflicts_declared: required_bool(
                        entry,
                        "property_conflicts_declared",
                    )?,
                    function_ids: optional_string_list(entry, "function_ids")?,
                    join_keys,
                });
            }
        }
        let mut do_not_merge = Vec::new();
        if let Some(values) = optional_array(object, "do_not_merge")? {
            for value in values {
                let entry = as_object(value)?;
                do_not_merge.push(MapDoNotMergeConstraint {
                    left_identity: required_non_empty_str(entry, "left_identity")?,
                    right_identity: required_non_empty_str(entry, "right_identity")?,
                });
            }
        }
        Ok(Self {
            mapping_id,
            mapping_version,
            identity_rules,
            do_not_merge,
        })
    }
}

impl MapRowSemanticsCatalog {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let root = parse_root(bytes)?;
        let object = as_object(&root)?;
        let (mapping_id, mapping_version) = parse_mapping_identity(object)?;
        let mut rules = Vec::new();
        if let Some(values) = optional_array(object, "rules")? {
            for value in values {
                let entry = as_object(value)?;
                let property_bindings =
                    parse_property_bindings(optional_array(entry, "property_bindings")?)?;
                let association_bindings =
                    parse_association_bindings(optional_array(entry, "association_bindings")?)?;
                let row_semantics_kind = optional_non_empty_str(entry, "row_semantics_kind")?
                    .or_else(|| optional_non_empty_str(entry, "kind").ok().flatten())
                    .unwrap_or_else(|| "Object".to_string());
                validate_row_semantics_kind(&row_semantics_kind)?;
                let assertion_kinds = string_list(entry, "assertion_kinds")?;
                if assertion_kinds.is_empty() {
                    return Err(CoveError::MapInvalid);
                }
                for kind in &assertion_kinds {
                    validate_assertion_kind(kind)?;
                }
                let tombstone_target = optional_non_empty_str(entry, "tombstone_target")?;
                match row_semantics_kind.as_str() {
                    "Tombstone" => match tombstone_target.as_deref() {
                        Some(target) if is_valid_tombstone_target(target) => {}
                        _ => return Err(CoveError::MapInvalid),
                    },
                    _ if tombstone_target.is_some() => return Err(CoveError::MapInvalid),
                    _ => {}
                }
                rules.push(MapRowSemanticRule {
                    rule_id: required_non_empty_str(entry, "rule_id")?,
                    source_id: required_non_empty_str(entry, "source_id")?,
                    identity_rule_id: required_non_empty_str(entry, "identity_rule_id")?,
                    row_semantics_kind,
                    assertion_kinds,
                    tombstone_target,
                    record_kind: optional_non_empty_str(entry, "record_kind")?
                        .unwrap_or_else(|| "Baseline".to_string()),
                    temporal_policy: optional_non_empty_str(entry, "temporal_policy")?
                        .unwrap_or_else(|| "latest_committed".to_string()),
                    conflict_policy: optional_non_empty_str(entry, "conflict_policy")?
                        .unwrap_or_else(|| "reject_conflict".to_string()),
                    function_ids: optional_string_list(entry, "function_ids")?,
                    output_assertion_ids: optional_string_list(entry, "output_assertion_ids")?,
                    association_endpoints: optional_string_list(entry, "association_endpoints")?,
                    property_bindings,
                    association_bindings,
                });
            }
        }
        Ok(Self {
            mapping_id,
            mapping_version,
            rules,
        })
    }
}

fn parse_property_bindings(
    values: Option<&Vec<Value>>,
) -> Result<Vec<MapPropertyBinding>, CoveError> {
    let Some(values) = values else {
        return Ok(Vec::new());
    };
    values
        .iter()
        .map(|value| {
            let entry = as_object(value)?;
            Ok(MapPropertyBinding {
                assertion_id: required_non_empty_str(entry, "assertion_id")?,
                property_id: required_non_empty_str(entry, "property_id")?,
                property_name: required_non_empty_str(entry, "property_name")?,
                source_column: required_non_empty_str(entry, "source_column")?,
                logical_type: required_non_empty_str(entry, "logical_type")?,
                physical_kind: optional_non_empty_str(entry, "physical_kind")?
                    .unwrap_or_else(|| "auto".to_string()),
                value_expression: optional_non_empty_str(entry, "value_expression")?
                    .unwrap_or_else(|| {
                        entry
                            .get("source_column")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string()
                    }),
                nullable: optional_bool(entry, "nullable", true)?,
                missing_policy: optional_non_empty_str(entry, "missing_policy")?
                    .unwrap_or_else(|| "null".to_string()),
                conflict_policy: optional_non_empty_str(entry, "conflict_policy")?
                    .unwrap_or_else(|| "reject_conflict".to_string()),
                source_priority: optional_i64(entry, "source_priority")?,
            })
        })
        .collect()
}

fn parse_association_bindings(
    values: Option<&Vec<Value>>,
) -> Result<Vec<MapAssociationBinding>, CoveError> {
    let Some(values) = values else {
        return Ok(Vec::new());
    };
    values
        .iter()
        .map(|value| {
            let entry = as_object(value)?;
            Ok(MapAssociationBinding {
                assertion_id: required_non_empty_str(entry, "assertion_id")?,
                association_type: required_non_empty_str(entry, "association_type")?,
                target_identity_rule_id: required_non_empty_str(entry, "target_identity_rule_id")?,
                source_identity_rule_id: optional_non_empty_str(entry, "source_identity_rule_id")?
                    .unwrap_or_default(),
                source_role: optional_non_empty_str(entry, "source_role")?
                    .unwrap_or_else(|| "source".to_string()),
                target_role: optional_non_empty_str(entry, "target_role")?
                    .unwrap_or_else(|| "target".to_string()),
                source_endpoint_expression: optional_non_empty_str(
                    entry,
                    "source_endpoint_expression",
                )?
                .unwrap_or_else(|| "source.goid".to_string()),
                target_endpoint_expression: optional_non_empty_str(
                    entry,
                    "target_endpoint_expression",
                )?
                .unwrap_or_else(|| "target.goid".to_string()),
                valid_from_expression: optional_non_empty_str(entry, "valid_from_expression")?,
                valid_to_expression: optional_non_empty_str(entry, "valid_to_expression")?,
                cardinality_policy: optional_non_empty_str(entry, "cardinality_policy")?
                    .unwrap_or_else(|| "one".to_string()),
                missing_policy: optional_non_empty_str(entry, "missing_policy")?
                    .unwrap_or_else(|| "reject".to_string()),
                link_object_materialization: optional_non_empty_str(
                    entry,
                    "link_object_materialization",
                )?
                .unwrap_or_else(|| "required".to_string()),
            })
        })
        .collect()
}

impl MapAssertionLog {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let root = parse_root(bytes)?;
        let object = as_object(&root)?;
        let (mapping_id, mapping_version) = parse_mapping_identity(object)?;
        let mut assertions = Vec::new();
        if let Some(values) = optional_array(object, "assertions")? {
            for value in values {
                let entry = as_object(value)?;
                assertions.push(MapAssertionEntry {
                    assertion_id: required_non_empty_str(entry, "assertion_id")?,
                    output_object_id: required_non_empty_str(entry, "output_object_id")?,
                });
            }
        }
        Ok(Self {
            mapping_id,
            mapping_version,
            assertions,
        })
    }
}

impl MapIdentityEquivalenceIndex {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let root = parse_root(bytes)?;
        let object = as_object(&root)?;
        let (mapping_id, mapping_version) = parse_mapping_identity(object)?;
        let mut equivalences = Vec::new();
        if let Some(values) = optional_array(object, "equivalences")? {
            for value in values {
                let entry = as_object(value)?;
                equivalences.push(MapEquivalencePair {
                    left_identity: required_non_empty_str(entry, "left_identity")?,
                    right_identity: required_non_empty_str(entry, "right_identity")?,
                });
            }
        }
        Ok(Self {
            mapping_id,
            mapping_version,
            equivalences,
        })
    }
}

impl MapEvidenceIndex {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let root = parse_root(bytes)?;
        let object = as_object(&root)?;
        let (mapping_id, mapping_version) = parse_mapping_identity(object)?;
        let mut entries = Vec::new();
        if let Some(values) = optional_array(object, "entries")? {
            for value in values {
                let entry = as_object(value)?;
                entries.push(MapEvidenceEntry {
                    source_id: required_non_empty_str(entry, "source_id")?,
                    source_row_identity: required_non_empty_str(entry, "source_row_identity")?,
                    rule_id: required_non_empty_str(entry, "rule_id")?,
                    assertion_id: required_non_empty_str(entry, "assertion_id")?,
                    output_object_id: required_non_empty_str(entry, "output_object_id")?,
                    observed_schema_fingerprint: optional_non_empty_str(
                        entry,
                        "observed_schema_fingerprint",
                    )?,
                    observed_snapshot_digest: optional_non_empty_str(
                        entry,
                        "observed_snapshot_digest",
                    )?,
                });
            }
        }
        Ok(Self {
            mapping_id,
            mapping_version,
            entries,
        })
    }
}

impl MapConversionReport {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let root = parse_root(bytes)?;
        let object = as_object(&root)?;
        let (mapping_id, mapping_version) = parse_mapping_identity(object)?;
        let mut sources = Vec::new();
        if let Some(values) = optional_array(object, "sources")? {
            for value in values {
                let entry = as_object(value)?;
                sources.push(MapObservedSourceState {
                    source_id: required_non_empty_str(entry, "source_id")?,
                    schema_fingerprint: optional_non_empty_str(entry, "schema_fingerprint")?,
                    snapshot_digest: optional_non_empty_str(entry, "snapshot_digest")?,
                });
            }
        }
        Ok(Self {
            mapping_id,
            mapping_version,
            sources,
        })
    }
}

impl MapProjectionCatalog {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let root = parse_root(bytes)?;
        let object = as_object(&root)?;
        let (mapping_id, mapping_version) = parse_mapping_identity(object)?;
        let mut projections = Vec::new();
        if let Some(values) = optional_array(object, "projections")? {
            for value in values {
                let entry = as_object(value)?;
                projections.push(MapProjectionEntry {
                    projection_id: required_non_empty_str(entry, "projection_id")?,
                    assertion_ids: optional_string_list(entry, "assertion_ids")?,
                    output_table: optional_non_empty_str(entry, "output_table")?,
                    row_grain: {
                        let row_grain = optional_non_empty_str(entry, "row_grain")?;
                        if row_grain
                            .as_deref()
                            .is_some_and(|row_grain| !is_valid_projection_row_grain(row_grain))
                        {
                            return Err(CoveError::MapInvalid);
                        }
                        row_grain
                    },
                    anchor: parse_projection_anchor(entry)?,
                    temporal_mode: {
                        let mode = parse_temporal_mode(entry)?;
                        if mode
                            .as_deref()
                            .is_some_and(|mode| !is_valid_temporal_mode(mode))
                        {
                            return Err(CoveError::MapInvalid);
                        }
                        mode
                    },
                    columns: parse_projection_columns(optional_array(entry, "columns")?)?,
                    multi_value_policy: {
                        let policy = optional_non_empty_str(entry, "multi_value_policy")?;
                        if policy
                            .as_deref()
                            .is_some_and(|policy| !is_valid_multi_value_policy(policy))
                        {
                            return Err(CoveError::MapInvalid);
                        }
                        policy
                    },
                    missing_policy: optional_non_empty_str(entry, "missing_policy")?
                        .unwrap_or_else(|| "null".to_string()),
                    ordering: optional_string_list(entry, "ordering")?,
                    evidence_policy: optional_non_empty_str(entry, "evidence_policy")?
                        .unwrap_or_else(|| "omit".to_string()),
                    output_modes: {
                        let modes = optional_string_list(entry, "output_modes")?;
                        if modes
                            .iter()
                            .any(|mode| !is_valid_projection_output_mode(mode))
                        {
                            return Err(CoveError::MapInvalid);
                        }
                        modes
                    },
                });
            }
        }
        Ok(Self {
            mapping_id,
            mapping_version,
            projections,
        })
    }
}

fn parse_projection_anchor(
    entry: &Map<String, Value>,
) -> Result<Option<MapProjectionAnchor>, CoveError> {
    let Some(anchor) = entry.get("anchor") else {
        return Ok(None);
    };
    let anchor = as_object(anchor)?;
    Ok(Some(MapProjectionAnchor {
        object_type: optional_non_empty_str(anchor, "object_type")?,
        association_type: optional_non_empty_str(anchor, "association_type")?,
    }))
}

fn parse_temporal_mode(entry: &Map<String, Value>) -> Result<Option<String>, CoveError> {
    match entry.get("temporal_mode") {
        None => Ok(None),
        Some(value) if value.is_string() => value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| Some(value.to_string()))
            .ok_or(CoveError::MapInvalid),
        Some(value) => {
            let mode = as_object(value)?;
            optional_non_empty_str(mode, "as_of")
        }
    }
}

fn parse_projection_columns(
    values: Option<&Vec<Value>>,
) -> Result<Vec<MapProjectionColumn>, CoveError> {
    let Some(values) = values else {
        return Ok(Vec::new());
    };
    values
        .iter()
        .map(|value| {
            let entry = as_object(value)?;
            Ok(MapProjectionColumn {
                name: required_non_empty_str(entry, "name")?,
                value: required_non_empty_str(entry, "value")?,
                logical_type: optional_non_empty_str(entry, "logical_type")?,
                conflict_policy: optional_non_empty_str(entry, "conflict_policy")?
                    .unwrap_or_else(|| "canonical_value".to_string()),
                missing_policy: optional_non_empty_str(entry, "missing_policy")?
                    .unwrap_or_else(|| "null".to_string()),
            })
        })
        .collect()
}

fn validate_row_semantics_kind(kind: &str) -> Result<(), CoveError> {
    match kind {
        "Object" | "EventObject" | "LinkObject" | "AssociationOnly" | "Composite"
        | "Dispatched" | "KeyValueFragment" | "ProjectionOnly" | "EvidenceOnly" | "Tombstone" => {
            Ok(())
        }
        _ => Err(CoveError::MapInvalid),
    }
}

fn validate_assertion_kind(kind: &str) -> Result<(), CoveError> {
    match kind {
        "object"
        | "property"
        | "association"
        | "temporal"
        | "identity_key"
        | "identity_equivalence"
        | "candidate_match"
        | "tombstone"
        | "evidence"
        | "conflict"
        | "projection" => Ok(()),
        _ => Err(CoveError::MapInvalid),
    }
}

fn is_valid_tombstone_target(target: &str) -> bool {
    matches!(
        target,
        "object" | "property" | "association" | "source_record" | "evidence"
    )
}

fn is_valid_temporal_mode(mode: &str) -> bool {
    matches!(
        mode,
        "latest_committed" | "full_history" | "valid_time" | "observed_time" | "commit_order"
    )
}

fn is_valid_multi_value_policy(policy: &str) -> bool {
    matches!(
        policy,
        "reject" | "explode" | "aggregate" | "first" | "last" | "list"
    )
}

fn is_valid_projection_row_grain(row_grain: &str) -> bool {
    matches!(
        row_grain,
        "one_row_per_object"
            | "one_row_per_association"
            | "one_row_per_link_object"
            | "one_row_per_property_version"
            | "one_row_per_event_object"
            | "one_row_per_object_as_of_time"
            | "one_row_per_evidence_assertion"
    )
}

fn is_valid_projection_output_mode(mode: &str) -> bool {
    matches!(mode, "json" | "cove-o" | "cove-t" | "arrow" | "sql")
}

fn parse_root(bytes: &[u8]) -> Result<Value, CoveError> {
    serde_json::from_slice(bytes).map_err(|_| CoveError::MapInvalid)
}

fn parse_mapping_identity(object: &Map<String, Value>) -> Result<(String, String), CoveError> {
    Ok((
        required_non_empty_str(object, "mapping_id")?,
        required_non_empty_str(object, "mapping_version")?,
    ))
}

fn as_object(value: &Value) -> Result<&Map<String, Value>, CoveError> {
    value.as_object().ok_or(CoveError::MapInvalid)
}

fn required_array<'a>(
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a Vec<Value>, CoveError> {
    object
        .get(key)
        .and_then(Value::as_array)
        .ok_or(CoveError::MapInvalid)
}

fn optional_array<'a>(
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<Option<&'a Vec<Value>>, CoveError> {
    match object.get(key) {
        None => Ok(None),
        Some(value) => value.as_array().map(Some).ok_or(CoveError::MapInvalid),
    }
}

fn required_non_empty_str(object: &Map<String, Value>, key: &str) -> Result<String, CoveError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or(CoveError::MapInvalid)
}

fn optional_non_empty_str(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<String>, CoveError> {
    match object.get(key) {
        None => Ok(None),
        Some(value) => value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| Some(value.to_string()))
            .ok_or(CoveError::MapInvalid),
    }
}

fn optional_i64(object: &Map<String, Value>, key: &str) -> Result<Option<i64>, CoveError> {
    match object.get(key) {
        None => Ok(None),
        Some(value) => value.as_i64().map(Some).ok_or(CoveError::MapInvalid),
    }
}

fn required_bool(object: &Map<String, Value>, key: &str) -> Result<bool, CoveError> {
    object
        .get(key)
        .and_then(Value::as_bool)
        .ok_or(CoveError::MapInvalid)
}

fn optional_bool(object: &Map<String, Value>, key: &str, default: bool) -> Result<bool, CoveError> {
    match object.get(key) {
        None => Ok(default),
        Some(value) => value.as_bool().ok_or(CoveError::MapInvalid),
    }
}

fn optional_bool_value(object: &Map<String, Value>, key: &str) -> Result<Option<bool>, CoveError> {
    match object.get(key) {
        None => Ok(None),
        Some(value) => value.as_bool().map(Some).ok_or(CoveError::MapInvalid),
    }
}

fn string_list(object: &Map<String, Value>, key: &str) -> Result<Vec<String>, CoveError> {
    required_array(object, key).and_then(|values| parse_string_values(values))
}

fn optional_string_list(object: &Map<String, Value>, key: &str) -> Result<Vec<String>, CoveError> {
    match optional_array(object, key)? {
        Some(values) => parse_string_values(values),
        None => Ok(Vec::new()),
    }
}

fn parse_string_values(values: &[Value]) -> Result<Vec<String>, CoveError> {
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .ok_or(CoveError::MapInvalid)
        })
        .collect()
}

fn normalize_pair(left: &str, right: &str) -> Result<(String, String), CoveError> {
    if left.is_empty() || right.is_empty() || left == right {
        return Err(CoveError::MapInvalid);
    }
    if left <= right {
        Ok((left.to_string(), right.to_string()))
    } else {
        Ok((right.to_string(), left.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
