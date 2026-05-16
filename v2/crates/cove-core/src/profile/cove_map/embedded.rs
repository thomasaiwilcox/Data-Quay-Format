//! Internal COVE-MAP embedded-section parsing and validation helpers.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};

use super::*;

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

pub(super) fn parse_embedded_section(
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

pub(super) fn validate_embedded_sections(sections: &[EmbeddedMapSection]) -> Result<(), CoveError> {
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
    match rule.source_operation_kind {
        SourceOperationKind::PatchProperty if rule.property_bindings.is_empty() => {
            return Err(CoveError::MapInvalid);
        }
        SourceOperationKind::CloseAssociation if rule.association_bindings.is_empty() => {
            return Err(CoveError::MapInvalid);
        }
        SourceOperationKind::TombstoneObject
        | SourceOperationKind::TombstoneProperty
        | SourceOperationKind::TombstoneAssociation
            if rule.tombstone_target.is_none() =>
        {
            return Err(CoveError::MapInvalid);
        }
        SourceOperationKind::EvidenceOnly if rule.row_semantics_kind != "EvidenceOnly" => {
            return Err(CoveError::MapInvalid);
        }
        _ => {}
    }
    Ok(())
}

impl MapSourceCatalog {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let root = parse_root_for_section(SectionKind::MapSourceCatalog, bytes)?;
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
                validate_keys(
                    entry,
                    &[
                        "source_id",
                        "source_kind",
                        "schema_fingerprint",
                        "snapshot_digest",
                        "row_identity_rules",
                        "replay_claimed",
                        "source_priority",
                        "sensitivity_label",
                        "sensitivity_rank",
                        "access_policy_ids",
                    ],
                )?;
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
        let root = parse_root_for_section(SectionKind::MapFunctionRegistry, bytes)?;
        let object = as_object(&root)?;
        let (mapping_id, mapping_version) = parse_mapping_identity(object)?;
        let mut functions = Vec::new();
        if let Some(values) = optional_array(object, "functions")? {
            for value in values {
                let entry = as_object(value)?;
                validate_keys(
                    entry,
                    &["function_id", "version", "deterministic", "dependency"],
                )?;
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
        let root = parse_root_for_section(SectionKind::MapIdentityRuleCatalog, bytes)?;
        let object = as_object(&root)?;
        let (mapping_id, mapping_version) = parse_mapping_identity(object)?;
        let mut identity_rules = Vec::new();
        if let Some(values) = optional_array(object, "identity_rules")? {
            for value in values {
                let entry = as_object(value)?;
                validate_keys(
                    entry,
                    &[
                        "rule_id",
                        "object_type",
                        "semantic_role",
                        "confidence_class",
                        "auto_merge",
                        "candidate_only",
                        "property_conflicts_declared",
                        "function_ids",
                        "join_keys",
                    ],
                )?;
                let mut join_keys = Vec::new();
                for join_key in required_array(entry, "join_keys")? {
                    let join_key = as_object(join_key)?;
                    validate_keys(
                        join_key,
                        &[
                            "role_id",
                            "source_column",
                            "logical_type",
                            "canonicalization",
                            "null_policy",
                            "ordering",
                        ],
                    )?;
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
                validate_keys(entry, &["left_identity", "right_identity"])?;
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
        let root = parse_root_for_section(SectionKind::MapRowSemanticsCatalog, bytes)?;
        let object = as_object(&root)?;
        let (mapping_id, mapping_version) = parse_mapping_identity(object)?;
        let mut rules = Vec::new();
        if let Some(values) = optional_array(object, "rules")? {
            for value in values {
                let entry = as_object(value)?;
                validate_keys(
                    entry,
                    &[
                        "rule_id",
                        "source_id",
                        "identity_rule_id",
                        "row_semantics_kind",
                        "kind",
                        "source_operation_kind",
                        "operation_kind",
                        "assertion_kinds",
                        "tombstone_target",
                        "record_kind",
                        "temporal_policy",
                        "conflict_policy",
                        "function_ids",
                        "output_assertion_ids",
                        "association_endpoints",
                        "property_bindings",
                        "association_bindings",
                    ],
                )?;
                let property_bindings =
                    parse_property_bindings(optional_array(entry, "property_bindings")?)?;
                let association_bindings =
                    parse_association_bindings(optional_array(entry, "association_bindings")?)?;
                let row_semantics_kind = optional_non_empty_str(entry, "row_semantics_kind")?
                    .or_else(|| optional_non_empty_str(entry, "kind").ok().flatten())
                    .unwrap_or_else(|| "Object".to_string());
                validate_row_semantics_kind(&row_semantics_kind)?;
                let source_operation_kind =
                    parse_source_operation_kind(entry, &row_semantics_kind)?;
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
                    source_operation_kind,
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
            validate_keys(
                entry,
                &[
                    "assertion_id",
                    "property_id",
                    "property_name",
                    "source_column",
                    "logical_type",
                    "physical_kind",
                    "value_expression",
                    "nullable",
                    "missing_policy",
                    "conflict_policy",
                    "source_priority",
                ],
            )?;
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

fn parse_source_operation_kind(
    entry: &Map<String, Value>,
    row_semantics_kind: &str,
) -> Result<SourceOperationKind, CoveError> {
    let value = optional_non_empty_str(entry, "source_operation_kind")?.or_else(|| {
        optional_non_empty_str(entry, "operation_kind")
            .ok()
            .flatten()
    });
    let kind = match value {
        Some(value) => SourceOperationKind::parse(&value).ok_or(CoveError::MapInvalid)?,
        None => match row_semantics_kind {
            "EvidenceOnly" => SourceOperationKind::EvidenceOnly,
            "Tombstone" => SourceOperationKind::TombstoneObject,
            _ => SourceOperationKind::Fact,
        },
    };
    Ok(kind)
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
            validate_keys(
                entry,
                &[
                    "assertion_id",
                    "association_type",
                    "target_identity_rule_id",
                    "source_identity_rule_id",
                    "source_role",
                    "target_role",
                    "source_endpoint_expression",
                    "target_endpoint_expression",
                    "valid_from_expression",
                    "valid_to_expression",
                    "cardinality_policy",
                    "missing_policy",
                    "link_object_materialization",
                ],
            )?;
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
        let root = parse_root_for_section(SectionKind::MapAssertionLog, bytes)?;
        let object = as_object(&root)?;
        let (mapping_id, mapping_version) = parse_mapping_identity(object)?;
        let mut assertions = Vec::new();
        if let Some(values) = optional_array(object, "assertions")? {
            for value in values {
                let entry = as_object(value)?;
                validate_keys(
                    entry,
                    &[
                        "assertion_id",
                        "output_object_id",
                        "source_operation_kind",
                        "operation_effect",
                        "operation_target",
                        "correction_of",
                        "replacement_of",
                        "redaction_scope",
                    ],
                )?;
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
        let root = parse_root_for_section(SectionKind::MapIdentityEquivalenceIndex, bytes)?;
        let object = as_object(&root)?;
        let (mapping_id, mapping_version) = parse_mapping_identity(object)?;
        let mut equivalences = Vec::new();
        if let Some(values) = optional_array(object, "equivalences")? {
            for value in values {
                let entry = as_object(value)?;
                validate_keys(entry, &["left_identity", "right_identity"])?;
                equivalences.push(MapEquivalencePair {
                    left_identity: required_non_empty_str(entry, "left_identity")?,
                    right_identity: required_non_empty_str(entry, "right_identity")?,
                });
            }
        }
        validate_identity_components(object)?;
        Ok(Self {
            mapping_id,
            mapping_version,
            equivalences,
        })
    }
}

impl MapEvidenceIndex {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let root = parse_root_for_section(SectionKind::MapEvidenceIndex, bytes)?;
        let object = as_object(&root)?;
        let (mapping_id, mapping_version) = parse_mapping_identity(object)?;
        let mut entries = Vec::new();
        if let Some(values) = optional_array(object, "entries")? {
            for value in values {
                let entry = as_object(value)?;
                validate_keys(
                    entry,
                    &[
                        "source_id",
                        "source_row_identity",
                        "rule_id",
                        "assertion_id",
                        "output_object_id",
                        "observed_schema_fingerprint",
                        "observed_snapshot_digest",
                        "source_operation_kind",
                        "operation_effect",
                        "operation_target",
                        "property_id",
                        "property_name",
                        "suppressed",
                        "suppressed_reason",
                        "suppressed_value",
                        "redacted",
                        "redaction_scope",
                        "correction_of",
                        "closes_association",
                        "expires_previous",
                        "replacement_of",
                        "candidate",
                        "identity_rule_id",
                        "object_type",
                        "join_key_sha256",
                    ],
                )?;
                let operation_metadata = entry
                    .iter()
                    .filter(|(key, _)| is_evidence_operation_metadata_key(key))
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect();
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
                    operation_metadata,
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

fn is_evidence_operation_metadata_key(key: &str) -> bool {
    matches!(
        key,
        "source_operation_kind"
            | "operation_effect"
            | "operation_target"
            | "property_id"
            | "property_name"
            | "suppressed"
            | "suppressed_reason"
            | "suppressed_value"
            | "redacted"
            | "redaction_scope"
            | "correction_of"
            | "closes_association"
            | "expires_previous"
            | "replacement_of"
            | "candidate"
            | "identity_rule_id"
            | "object_type"
            | "join_key_sha256"
    )
}

impl MapConversionReport {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let root = parse_root_for_section(SectionKind::MapConversionReport, bytes)?;
        let object = as_object(&root)?;
        let (mapping_id, mapping_version) = parse_mapping_identity(object)?;
        let mut sources = Vec::new();
        if let Some(values) = optional_array(object, "sources")? {
            for value in values {
                let entry = as_object(value)?;
                validate_keys(
                    entry,
                    &[
                        "source_id",
                        "source_kind",
                        "schema_fingerprint",
                        "snapshot_digest",
                    ],
                )?;
                sources.push(MapObservedSourceState {
                    source_id: required_non_empty_str(entry, "source_id")?,
                    schema_fingerprint: optional_non_empty_str(entry, "schema_fingerprint")?,
                    snapshot_digest: optional_non_empty_str(entry, "snapshot_digest")?,
                });
            }
        }
        validate_conversion_report_details(object)?;
        Ok(Self {
            mapping_id,
            mapping_version,
            sources,
        })
    }
}

impl MapProjectionCatalog {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let root = parse_root_for_section(SectionKind::MapProjectionCatalog, bytes)?;
        let object = as_object(&root)?;
        let (mapping_id, mapping_version) = parse_mapping_identity(object)?;
        let mut projections = Vec::new();
        if let Some(values) = optional_array(object, "projections")? {
            for value in values {
                let entry = as_object(value)?;
                validate_keys(
                    entry,
                    &[
                        "projection_id",
                        "assertion_ids",
                        "output_table",
                        "row_grain",
                        "anchor",
                        "temporal_mode",
                        "columns",
                        "multi_value_policy",
                        "missing_policy",
                        "ordering",
                        "evidence_policy",
                        "output_modes",
                    ],
                )?;
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
    validate_keys(anchor, &["object_type", "association_type"])?;
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
            validate_keys(mode, &["as_of"])?;
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
            validate_keys(
                entry,
                &[
                    "name",
                    "value",
                    "logical_type",
                    "nested_shape",
                    "conflict_policy",
                    "missing_policy",
                ],
            )?;
            Ok(MapProjectionColumn {
                name: required_non_empty_str(entry, "name")?,
                value: required_non_empty_str(entry, "value")?,
                logical_type: optional_non_empty_str(entry, "logical_type")?,
                nested_shape: optional_nested_shape(entry, "nested_shape")?,
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
    ) || mode
        .strip_prefix("as_of_timestamp_us:")
        .or_else(|| mode.strip_prefix("as_of_timestamp_us="))
        .or_else(|| mode.strip_prefix("timestamp_us:"))
        .or_else(|| mode.strip_prefix("timestamp_us="))
        .or_else(|| mode.strip_prefix("as_of_time:"))
        .or_else(|| mode.strip_prefix("as_of_time="))
        .is_some_and(|value| value.parse::<i64>().is_ok())
        || mode
            .strip_prefix("as_of_csn:")
            .or_else(|| mode.strip_prefix("as_of_csn="))
            .or_else(|| mode.strip_prefix("csn:"))
            .or_else(|| mode.strip_prefix("csn="))
            .is_some_and(|value| value.parse::<u64>().is_ok())
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

const COVE_MAP_JSON_SCHEMA_ID: &str = "org.coveformat.covemap.v2";

#[derive(Debug)]
struct NoDuplicateValue(Value);

impl<'de> Deserialize<'de> for NoDuplicateValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(NoDuplicateValueVisitor)
    }
}

struct NoDuplicateValueVisitor;

impl<'de> Visitor<'de> for NoDuplicateValueVisitor {
    type Value = NoDuplicateValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value without duplicate object keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(NoDuplicateValue(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(NoDuplicateValue(Value::Number(value.into())))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(NoDuplicateValue(Value::Number(value.into())))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .map(NoDuplicateValue)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(NoDuplicateValue(Value::String(value.to_owned())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(NoDuplicateValue(Value::String(value)))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(NoDuplicateValue(Value::Null))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(NoDuplicateValue(Value::Null))
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        NoDuplicateValue::deserialize(deserializer)
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = seq.next_element::<NoDuplicateValue>()? {
            values.push(value.0);
        }
        Ok(NoDuplicateValue(Value::Array(values)))
    }

    fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut keys = BTreeSet::new();
        let mut object = Map::new();
        while let Some((key, value)) = access.next_entry::<String, NoDuplicateValue>()? {
            if !keys.insert(key.clone()) {
                return Err(de::Error::custom(format!("duplicate JSON key `{key}`")));
            }
            object.insert(key, value.0);
        }
        Ok(NoDuplicateValue(Value::Object(object)))
    }
}

fn parse_root_for_section(kind: SectionKind, bytes: &[u8]) -> Result<Value, CoveError> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let root = NoDuplicateValue::deserialize(&mut deserializer)
        .map_err(|_| CoveError::MapInvalid)?
        .0;
    deserializer.end().map_err(|_| CoveError::MapInvalid)?;
    let object = as_object(&root)?;
    validate_payload_envelope(kind, object)?;
    Ok(root)
}

fn validate_payload_envelope(
    kind: SectionKind,
    object: &Map<String, Value>,
) -> Result<(), CoveError> {
    if object.get("schema_id").and_then(Value::as_str) != Some(COVE_MAP_JSON_SCHEMA_ID) {
        return Err(CoveError::MapInvalid);
    }
    if !section_id_matches(kind, object.get("section_id").ok_or(CoveError::MapInvalid)?) {
        return Err(CoveError::MapInvalid);
    }
    if !object.keys().all(|key| is_allowed_root_key(kind, key)) {
        return Err(CoveError::MapInvalid);
    }
    validate_extension_containers(object)?;
    Ok(())
}

fn section_id_matches(kind: SectionKind, value: &Value) -> bool {
    match value {
        Value::Number(number) => number.as_u64() == Some(kind as u16 as u64),
        Value::String(name) => {
            name == section_kind_schema_name(kind) || name == &format!("{kind:?}")
        }
        _ => false,
    }
}

fn section_kind_schema_name(kind: SectionKind) -> &'static str {
    match kind {
        SectionKind::MapSourceCatalog => "MAP_SOURCE_CATALOG",
        SectionKind::MapFunctionRegistry => "MAP_FUNCTION_REGISTRY",
        SectionKind::MapIdentityRuleCatalog => "MAP_IDENTITY_RULE_CATALOG",
        SectionKind::MapRowSemanticsCatalog => "MAP_ROW_SEMANTICS_CATALOG",
        SectionKind::MapAssertionLog => "MAP_ASSERTION_LOG",
        SectionKind::MapIdentityEquivalenceIndex => "MAP_IDENTITY_EQUIVALENCE_INDEX",
        SectionKind::MapEvidenceIndex => "MAP_EVIDENCE_INDEX",
        SectionKind::MapConversionReport => "MAP_CONVERSION_REPORT",
        SectionKind::MapProjectionCatalog => "MAP_PROJECTION_CATALOG",
        _ => "UNKNOWN",
    }
}

fn is_allowed_root_key(kind: SectionKind, key: &str) -> bool {
    matches!(
        key,
        "schema_id" | "section_id" | "mapping_id" | "mapping_version" | "extension" | "extensions"
    ) || match kind {
        SectionKind::MapSourceCatalog => {
            matches!(key, "governance_reconciliation_policy" | "sources")
        }
        SectionKind::MapFunctionRegistry => key == "functions",
        SectionKind::MapIdentityRuleCatalog => matches!(key, "identity_rules" | "do_not_merge"),
        SectionKind::MapRowSemanticsCatalog => key == "rules",
        SectionKind::MapAssertionLog => key == "assertions",
        SectionKind::MapIdentityEquivalenceIndex => matches!(key, "equivalences" | "components"),
        SectionKind::MapEvidenceIndex => key == "entries",
        SectionKind::MapConversionReport => matches!(
            key,
            "sources"
                | "source_count"
                | "row_count"
                | "object_count"
                | "association_count"
                | "property_value_count"
                | "candidate_match_count"
                | "candidate_matches"
                | "generated_artifacts"
                | "unsupported"
                | "operation_counts"
                | "governance"
        ),
        SectionKind::MapProjectionCatalog => key == "projections",
        _ => false,
    }
}

fn validate_keys(object: &Map<String, Value>, allowed: &[&str]) -> Result<(), CoveError> {
    for key in object.keys() {
        if key == "extension" || key == "extensions" || allowed.iter().any(|allowed| allowed == key)
        {
            continue;
        }
        return Err(CoveError::MapInvalid);
    }
    validate_extension_containers(object)?;
    Ok(())
}

fn validate_extension_containers(object: &Map<String, Value>) -> Result<(), CoveError> {
    if let Some(extension) = object.get("extension") {
        if !extension.is_object() {
            return Err(CoveError::MapInvalid);
        }
    }
    if let Some(extensions) = object.get("extensions") {
        let extensions = extensions.as_object().ok_or(CoveError::MapInvalid)?;
        for (extension_id, payload) in extensions {
            if extension_id.trim().is_empty() || !payload.is_object() {
                return Err(CoveError::MapInvalid);
            }
        }
    }
    Ok(())
}

fn validate_identity_components(object: &Map<String, Value>) -> Result<(), CoveError> {
    let Some(components) = optional_array(object, "components")? else {
        return Ok(());
    };
    for value in components {
        let component = as_object(value)?;
        validate_keys(
            component,
            &["equivalence_id", "goid", "canonical_anchor", "members"],
        )?;
        if let Some(members) = optional_array(component, "members")? {
            for value in members {
                let member = as_object(value)?;
                validate_keys(
                    member,
                    &[
                        "source_id",
                        "row_index",
                        "source_row_identity",
                        "row_rule_id",
                        "identity_rule_id",
                        "identity_alias",
                        "object_type",
                        "join_key_sha256",
                        "row_digest",
                    ],
                )?;
            }
        }
    }
    Ok(())
}

fn validate_conversion_report_details(object: &Map<String, Value>) -> Result<(), CoveError> {
    if let Some(matches) = optional_array(object, "candidate_matches")? {
        for value in matches {
            let candidate = as_object(value)?;
            validate_keys(
                candidate,
                &[
                    "candidate_match_id",
                    "source_id",
                    "source_row_identity",
                    "row_rule_id",
                    "identity_rule_id",
                    "object_type",
                    "join_key_sha256",
                ],
            )?;
        }
    }
    if let Some(artifacts) = optional_array(object, "generated_artifacts")? {
        parse_string_values(artifacts)?;
    }
    if let Some(unsupported) = optional_array(object, "unsupported")? {
        parse_string_values(unsupported)?;
    }
    if let Some(operation_counts) = object.get("operation_counts") {
        let operation_counts = as_object(operation_counts)?;
        for (key, value) in operation_counts {
            if SourceOperationKind::parse(key).is_none() || value.as_u64().is_none() {
                return Err(CoveError::MapInvalid);
            }
        }
    }
    if let Some(governance) = object.get("governance") {
        let governance = as_object(governance)?;
        validate_keys(
            governance,
            &[
                "reconciliation_policy",
                "sources",
                "effective_sensitivity_rank",
                "effective_sensitivity_labels",
                "access_policy_ids",
            ],
        )?;
        if let Some(sources) = optional_array(governance, "sources")? {
            for value in sources {
                let source = as_object(value)?;
                validate_keys(
                    source,
                    &[
                        "source_id",
                        "source_priority",
                        "sensitivity_label",
                        "sensitivity_rank",
                        "access_policy_ids",
                    ],
                )?;
            }
        }
    }
    Ok(())
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

fn optional_nested_shape(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<String>, CoveError> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::String(value)) => {
            let value = value.trim();
            if value.is_empty() {
                Err(CoveError::MapInvalid)
            } else {
                Ok(Some(value.to_string()))
            }
        }
        Some(Value::Object(_)) => object
            .get(key)
            .map(serde_json::to_string)
            .transpose()
            .map_err(|_| CoveError::MapInvalid),
        Some(_) => Err(CoveError::MapInvalid),
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
