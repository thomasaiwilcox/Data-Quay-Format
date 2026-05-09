use std::collections::{BTreeMap, BTreeSet};

use cove_core::{artifact::covemap::CovemapFile, profile::cove_map::MapIdentityRule};

use crate::{
    hex_encode, join_key_tuple_from_rule, mapped_goid, mapping_context, object_types_from_mapping,
    row_digest, schema_fingerprint, SourceRow,
};

#[derive(Debug, Clone)]
pub(crate) struct PlannedIdentity {
    pub(crate) source_id: String,
    pub(crate) row_index: usize,
    pub(crate) row_digest: String,
    pub(crate) schema_fingerprint: String,
    pub(crate) source_row_identity: String,
    pub(crate) row_rule_id: String,
    pub(crate) identity_rule_id: String,
    pub(crate) object_type: String,
    pub(crate) join_key_sha256: String,
    pub(crate) identity_alias: String,
    pub(crate) equivalence_id: String,
    pub(crate) canonical_anchor: String,
    pub(crate) goid: [u8; 16],
}

#[derive(Debug, Clone)]
pub(crate) struct CandidateMatch {
    pub(crate) source_id: String,
    pub(crate) row_index: usize,
    pub(crate) row_digest: String,
    pub(crate) schema_fingerprint: String,
    pub(crate) source_row_identity: String,
    pub(crate) row_rule_id: String,
    pub(crate) identity_rule_id: String,
    pub(crate) object_type: String,
    pub(crate) join_key_sha256: String,
    pub(crate) identity_alias: String,
}

#[derive(Debug, Clone)]
pub(crate) struct IdentityPlan {
    pub(crate) canonical: Vec<PlannedIdentity>,
    pub(crate) candidates: Vec<CandidateMatch>,
}

pub(crate) fn plan_identities(
    file: &CovemapFile,
    rows: &[SourceRow],
) -> Result<IdentityPlan, String> {
    let context = mapping_context(file)?;
    let object_types = object_types_from_mapping(&context)?;
    let type_ids = object_types
        .iter()
        .map(|ty| (ty.type_name.clone(), ty.object_type_id))
        .collect::<BTreeMap<_, _>>();
    let mut keys = Vec::<IdentityKey>::new();
    let mut candidates = Vec::<CandidateMatch>::new();
    for row in rows {
        let matching_rules = context
            .row_rules
            .iter()
            .filter(|rule| rule.source_id == row.source_id)
            .collect::<Vec<_>>();
        if matching_rules.is_empty() {
            return Err(format!(
                "source '{}' has no declared row semantic rule",
                row.source_id
            ));
        }
        for row_rule in matching_rules {
            let identity_rule = context
                .identity_rules
                .get(&row_rule.identity_rule_id)
                .ok_or_else(|| {
                    format!(
                        "row rule '{}' references missing identity rule '{}'",
                        row_rule.rule_id, row_rule.identity_rule_id
                    )
                })?;
            let object_type_id = *type_ids
                .get(&identity_rule.object_type)
                .ok_or_else(|| format!("unknown object type '{}'", identity_rule.object_type))?;
            let tuple = join_key_tuple_from_rule(identity_rule, row, object_type_id)?;
            let source_row_identity = format!("{}:{}", row.source_id, row.row_index);
            let row_digest = row_digest(row);
            let schema_fingerprint = schema_fingerprint(row);
            let join_key_sha256 = crate::sha256_hex(&tuple);
            if is_candidate_identity_rule(identity_rule) {
                candidates.push(CandidateMatch {
                    source_id: row.source_id.clone(),
                    row_index: row.row_index,
                    row_digest,
                    schema_fingerprint,
                    source_row_identity,
                    row_rule_id: row_rule.rule_id.clone(),
                    identity_rule_id: identity_rule.rule_id.clone(),
                    object_type: identity_rule.object_type.clone(),
                    join_key_sha256: join_key_sha256.clone(),
                    identity_alias: format!("{}:{join_key_sha256}", identity_rule.rule_id),
                });
                continue;
            }
            let merge_class = merge_class(identity_rule);
            let source_order = context
                .source_order
                .get(&row.source_id)
                .copied()
                .unwrap_or(usize::MAX);
            let rule_order = context
                .identity_rule_order
                .get(&identity_rule.rule_id)
                .copied()
                .unwrap_or(usize::MAX);
            let join_key_sha256 = crate::sha256_hex(&tuple);
            keys.push(IdentityKey {
                source_id: row.source_id.clone(),
                row_index: row.row_index,
                row_digest,
                schema_fingerprint,
                source_row_identity,
                row_rule_id: row_rule.rule_id.clone(),
                identity_rule_id: identity_rule.rule_id.clone(),
                object_type: identity_rule.object_type.clone(),
                object_type_id,
                class_rank: identity_class_rank(&identity_rule.confidence_class),
                rule_order,
                source_order,
                join_key_tuple: tuple,
                join_key_sha256,
                merge_class,
            });
        }
    }

    let mut uf = UnionFind::new(keys.len());
    let mut merge_groups = BTreeMap::<Vec<u8>, Vec<usize>>::new();
    for (index, key) in keys.iter().enumerate() {
        if let Some(group_key) = key.merge_group_key() {
            merge_groups.entry(group_key).or_default().push(index);
        }
    }
    for indexes in merge_groups.values() {
        if let Some((first, rest)) = indexes.split_first() {
            for index in rest {
                uf.union(*first, *index);
            }
        }
    }

    let mut components = BTreeMap::<usize, Vec<usize>>::new();
    for index in 0..keys.len() {
        components.entry(uf.find(index)).or_default().push(index);
    }
    validate_do_not_merge(&context.do_not_merge, &components, &keys)?;

    let mut planned = Vec::with_capacity(keys.len());
    for indexes in components.values() {
        let anchor_index = indexes
            .iter()
            .copied()
            .min_by_key(|index| keys[*index].anchor_sort_key())
            .ok_or_else(|| "empty identity component".to_string())?;
        let anchor = &keys[anchor_index];
        let source_scope = anchor.goid_source_scope();
        let goid = mapped_goid(
            &file.header.mapping_id,
            file.mapping_version.as_bytes(),
            anchor.object_type_id,
            anchor.identity_rule_id.as_bytes(),
            &anchor.join_key_tuple,
            source_scope.as_deref(),
        );
        let equivalence_id = format!("{}:{}", anchor.object_type, hex_encode(&goid));
        let canonical_anchor = anchor.anchor_alias();
        for index in indexes {
            let key = &keys[*index];
            planned.push(PlannedIdentity {
                source_id: key.source_id.clone(),
                row_index: key.row_index,
                row_digest: key.row_digest.clone(),
                schema_fingerprint: key.schema_fingerprint.clone(),
                source_row_identity: key.source_row_identity.clone(),
                row_rule_id: key.row_rule_id.clone(),
                identity_rule_id: key.identity_rule_id.clone(),
                object_type: key.object_type.clone(),
                join_key_sha256: key.join_key_sha256.clone(),
                identity_alias: key.anchor_alias(),
                equivalence_id: equivalence_id.clone(),
                canonical_anchor: canonical_anchor.clone(),
                goid,
            });
        }
    }
    planned.sort_by_key(|identity| {
        (
            identity.source_id.clone(),
            identity.row_index,
            identity.identity_rule_id.clone(),
            identity.goid,
        )
    });
    candidates.sort_by_key(|candidate| {
        (
            candidate.source_id.clone(),
            candidate.row_index,
            candidate.identity_rule_id.clone(),
            candidate.join_key_sha256.clone(),
        )
    });
    Ok(IdentityPlan {
        canonical: planned,
        candidates,
    })
}

fn is_candidate_identity_rule(rule: &MapIdentityRule) -> bool {
    rule.candidate_only || rule.confidence_class == "candidate"
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdentityMergeClass {
    MergeGlobal,
    MergeWithinSource,
    Singleton,
}

#[derive(Debug, Clone)]
struct IdentityKey {
    source_id: String,
    row_index: usize,
    row_digest: String,
    schema_fingerprint: String,
    source_row_identity: String,
    row_rule_id: String,
    identity_rule_id: String,
    object_type: String,
    object_type_id: u32,
    class_rank: u8,
    rule_order: usize,
    source_order: usize,
    join_key_tuple: Vec<u8>,
    join_key_sha256: String,
    merge_class: IdentityMergeClass,
}

impl IdentityKey {
    fn merge_group_key(&self) -> Option<Vec<u8>> {
        if self.merge_class == IdentityMergeClass::Singleton {
            return None;
        }
        let mut out = Vec::new();
        crate::append_len_bytes(&mut out, self.object_type.as_bytes());
        crate::append_len_bytes(&mut out, self.identity_rule_id.as_bytes());
        if self.merge_class == IdentityMergeClass::MergeWithinSource {
            crate::append_len_bytes(&mut out, self.source_id.as_bytes());
        }
        crate::append_len_bytes(&mut out, &self.join_key_tuple);
        Some(out)
    }

    fn anchor_sort_key(&self) -> (u8, usize, usize, Vec<u8>, String) {
        (
            self.class_rank,
            self.rule_order,
            self.source_order,
            self.join_key_tuple.clone(),
            self.source_row_identity.clone(),
        )
    }

    fn goid_source_scope(&self) -> Option<String> {
        match self.merge_class {
            IdentityMergeClass::MergeGlobal => None,
            IdentityMergeClass::MergeWithinSource => Some(self.source_id.clone()),
            IdentityMergeClass::Singleton => Some(self.source_row_identity.clone()),
        }
    }

    fn anchor_alias(&self) -> String {
        format!("{}:{}", self.identity_rule_id, self.join_key_sha256)
    }

    fn aliases(&self) -> BTreeSet<String> {
        BTreeSet::from([
            self.source_row_identity.clone(),
            self.row_digest.clone(),
            self.anchor_alias(),
            format!("{}:{}", self.object_type, self.join_key_sha256),
        ])
    }
}

struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(len: usize) -> Self {
        Self {
            parent: (0..len).collect(),
        }
    }

    fn find(&mut self, index: usize) -> usize {
        let parent = self.parent[index];
        if parent == index {
            index
        } else {
            let root = self.find(parent);
            self.parent[index] = root;
            root
        }
    }

    fn union(&mut self, left: usize, right: usize) {
        let left_root = self.find(left);
        let right_root = self.find(right);
        if left_root != right_root {
            let (keep, replace) = if left_root <= right_root {
                (left_root, right_root)
            } else {
                (right_root, left_root)
            };
            self.parent[replace] = keep;
        }
    }
}

fn merge_class(rule: &MapIdentityRule) -> IdentityMergeClass {
    match rule.confidence_class.as_str() {
        "authoritative" => {
            if rule.auto_merge.unwrap_or(true) {
                IdentityMergeClass::MergeGlobal
            } else {
                IdentityMergeClass::Singleton
            }
        }
        "strong_deterministic" => {
            if rule.auto_merge.unwrap_or(false) {
                IdentityMergeClass::MergeGlobal
            } else {
                IdentityMergeClass::Singleton
            }
        }
        "source_scoped" => IdentityMergeClass::MergeWithinSource,
        _ => IdentityMergeClass::Singleton,
    }
}

fn identity_class_rank(class: &str) -> u8 {
    match class {
        "authoritative" => 0,
        "strong_deterministic" => 1,
        "source_scoped" => 2,
        "weak_deterministic" => 3,
        _ => 4,
    }
}

fn validate_do_not_merge(
    constraints: &[(String, String)],
    components: &BTreeMap<usize, Vec<usize>>,
    keys: &[IdentityKey],
) -> Result<(), String> {
    for indexes in components.values() {
        let aliases = indexes
            .iter()
            .flat_map(|index| keys[*index].aliases())
            .collect::<BTreeSet<_>>();
        for (left, right) in constraints {
            if aliases.contains(left) && aliases.contains(right) {
                return Err(format!(
                    "identity resolution violates do-not-merge constraint '{left}' <-> '{right}'"
                ));
            }
        }
    }
    Ok(())
}
