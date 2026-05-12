use std::collections::BTreeMap;

use cove_core::{
    artifact::covemap::CovemapFile,
    profile::cove_map::{
        EmbeddedMapSection, MapIdentityRule, MapIdentityRuleCatalog, MapRowSemanticRule,
        MapRowSemanticsCatalog, MapSourceEntry,
    },
};

#[derive(Debug, Clone)]
pub(crate) struct MappingContext {
    pub(crate) identity_rules: BTreeMap<String, MapIdentityRule>,
    pub(crate) identity_rule_order: BTreeMap<String, usize>,
    pub(crate) source_order: BTreeMap<String, usize>,
    pub(crate) sources: BTreeMap<String, MapSourceEntry>,
    pub(crate) governance_reconciliation_policy: String,
    pub(crate) row_rules: Vec<MapRowSemanticRule>,
    pub(crate) do_not_merge: Vec<(String, String)>,
}

pub(crate) fn mapping_context(file: &CovemapFile) -> Result<MappingContext, String> {
    let mut identity_rules = BTreeMap::new();
    let mut identity_rule_order = BTreeMap::new();
    let mut source_order = BTreeMap::new();
    let mut sources = BTreeMap::new();
    let mut governance_reconciliation_policy = "emit_effective_policy".to_string();
    let mut row_rules = Vec::new();
    let mut do_not_merge = Vec::new();
    for section in crate::embedded_sections(file)? {
        match section {
            EmbeddedMapSection::SourceCatalog(catalog) => {
                if governance_reconciliation_policy != "emit_effective_policy"
                    && governance_reconciliation_policy != catalog.governance_reconciliation_policy
                {
                    return Err("conflicting governance reconciliation policies".into());
                }
                governance_reconciliation_policy = catalog.governance_reconciliation_policy;
                for source in catalog.sources {
                    let order = source_order.len();
                    if source_order
                        .insert(source.source_id.clone(), order)
                        .is_some()
                    {
                        return Err("duplicate source entry".into());
                    }
                    sources.insert(source.source_id.clone(), source);
                }
            }
            EmbeddedMapSection::IdentityRuleCatalog(MapIdentityRuleCatalog {
                identity_rules: rules,
                do_not_merge: constraints,
                ..
            }) => {
                for rule in rules {
                    let order = identity_rule_order.len();
                    if identity_rule_order
                        .insert(rule.rule_id.clone(), order)
                        .is_some()
                    {
                        return Err("duplicate identity rule".into());
                    }
                    if identity_rules.insert(rule.rule_id.clone(), rule).is_some() {
                        return Err("duplicate identity rule".into());
                    }
                }
                do_not_merge.extend(constraints.into_iter().map(|constraint| {
                    if constraint.left_identity <= constraint.right_identity {
                        (constraint.left_identity, constraint.right_identity)
                    } else {
                        (constraint.right_identity, constraint.left_identity)
                    }
                }));
            }
            EmbeddedMapSection::RowSemanticsCatalog(MapRowSemanticsCatalog { rules, .. }) => {
                row_rules.extend(rules);
            }
            _ => {}
        }
    }
    if identity_rules.is_empty() || row_rules.is_empty() {
        return Err("mapping must declare identity rules and row semantic rules".into());
    }
    Ok(MappingContext {
        identity_rules,
        identity_rule_order,
        source_order,
        sources,
        governance_reconciliation_policy,
        row_rules,
        do_not_merge,
    })
}
