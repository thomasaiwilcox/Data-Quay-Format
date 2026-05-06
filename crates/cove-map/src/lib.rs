use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use cove_core::{
    artifact::covemap::CovemapFile,
    canonical::CanonicalValue,
    checksum,
    constants::{
        CompressionCodec, CoveEncodingKind, CoveLogicalType, CovePhysicalKind, PrimaryProfile,
        SectionKind, FEATURE_OBJECT_PROFILE, FEATURE_SEMANTIC_MAP, FEATURE_TRUST_CHAIN,
    },
    durable,
    page::{ColumnPageIndexEntryV1, COLUMN_PAGE_INDEX_ENTRY_LEN},
    page_payload::ColumnPagePayloadV1,
    profile::{
        cove_map::{
            parse_embedded_section, EmbeddedMapSection, MapIdentityRule, MapIdentityRuleCatalog,
            MapProjectionCatalog, MapProjectionEntry, MapPropertyBinding, MapRowSemanticRule,
            MapRowSemanticsCatalog, MapSourceEntry,
        },
        cove_o::{
            ObjectTypeCatalog, ObjectTypeEntryV1, PropertyEntryV1, RecordKind, TemporalRowEntryV1,
            TemporalSegmentHeaderV1, TemporalSegmentIndex, TemporalSegmentIndexEntryV1,
            TrustManifest, TrustManifestEntryV1, OBJECT_TYPE_FLAG_ASSOCIATION_OBJECT,
            OBJECT_TYPE_FLAG_ENTITY_OBJECT, OBJECT_TYPE_FLAG_LINK_OBJECT,
            PROPERTY_FLAG_ASSOCIATION_FROM_GOID, PROPERTY_FLAG_ASSOCIATION_TO_GOID,
            PROPERTY_FLAG_ASSOCIATION_TYPE, PROPERTY_FLAG_EVIDENCE_REF,
            PROPERTY_FLAG_MAPPING_RULE_REF, TEMPORAL_ROW_ENTRY_LEN, TEMPORAL_SEGMENT_HEADER_LEN,
        },
    },
    reader::{validate_bytes_with_options, ValidationOptions},
    segment::{TableColumnDirectoryEntryV1, TABLE_COLUMN_DIRECTORY_ENTRY_LEN},
    trust_chain,
    writer::{MinimalCoveWriter, SectionPayload},
};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    Validate {
        map: PathBuf,
    },
    Preview {
        map: PathBuf,
    },
    PlanKeys {
        map: PathBuf,
        sources: Vec<PathBuf>,
    },
    Convert {
        map: PathBuf,
        sources: Vec<PathBuf>,
        output: Option<PathBuf>,
        format: OutputFormat,
    },
    Explain {
        map: PathBuf,
        id: String,
    },
    Diff {
        left: PathBuf,
        right: PathBuf,
    },
    Project {
        map: PathBuf,
        sources: Vec<PathBuf>,
        output: Option<PathBuf>,
    },
    Test {
        fixture: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Json,
    CoveO,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceRow {
    source_id: String,
    row_index: usize,
    values: BTreeMap<String, Value>,
}

#[derive(Debug, Clone)]
struct SourceInputs {
    rows: Vec<SourceRow>,
    states: Vec<ObservedSourceState>,
}

#[derive(Debug, Clone)]
struct ObservedSourceState {
    source_id: String,
    source_kind: String,
    schema_fingerprint: String,
    snapshot_digest: String,
}

#[derive(Debug, Clone)]
struct MappingContext {
    identity_rules: BTreeMap<String, MapIdentityRule>,
    identity_rule_order: BTreeMap<String, usize>,
    source_order: BTreeMap<String, usize>,
    sources: BTreeMap<String, MapSourceEntry>,
    governance_reconciliation_policy: String,
    row_rules: Vec<MapRowSemanticRule>,
    do_not_merge: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
struct PlannedIdentity {
    source_id: String,
    row_index: usize,
    row_digest: String,
    schema_fingerprint: String,
    source_row_identity: String,
    row_rule_id: String,
    identity_rule_id: String,
    object_type: String,
    join_key_sha256: String,
    identity_alias: String,
    equivalence_id: String,
    canonical_anchor: String,
    goid: [u8; 16],
}

#[derive(Debug, Clone)]
struct CandidateMatch {
    source_id: String,
    row_index: usize,
    row_digest: String,
    schema_fingerprint: String,
    source_row_identity: String,
    row_rule_id: String,
    identity_rule_id: String,
    object_type: String,
    join_key_sha256: String,
    identity_alias: String,
}

#[derive(Debug, Clone)]
struct IdentityPlan {
    canonical: Vec<PlannedIdentity>,
    candidates: Vec<CandidateMatch>,
}

pub fn run_cli(args: impl IntoIterator<Item = String>) -> Result<(), String> {
    let Some(command) = parse_args(args)? else {
        print_usage();
        return Ok(());
    };
    match command {
        Command::Validate { map } => {
            parse_map(&map)?;
            println!("{}", json!({"ok": true, "path": map.display().to_string()}));
        }
        Command::Preview { map } => {
            let file = parse_map(&map)?;
            print_json(&preview(&file));
        }
        Command::PlanKeys { map, sources } => {
            let file = parse_map(&map)?;
            let inputs = read_source_inputs(&sources)?;
            validate_source_inputs(&file, &inputs.states)?;
            print_json(&plan_keys(&file, &inputs.rows));
        }
        Command::Convert {
            map,
            sources,
            output,
            format,
        } => {
            let file = parse_map(&map)?;
            let inputs = read_source_inputs(&sources)?;
            validate_source_inputs(&file, &inputs.states)?;
            match format {
                OutputFormat::Json => {
                    let materialized =
                        materialize_with_source_states(&file, &inputs.rows, &inputs.states)?;
                    write_or_print(output, &materialized.conversion_report)?;
                }
                OutputFormat::CoveO => {
                    let output = output.ok_or_else(|| {
                        "convert --format cove-o requires --output <path>".to_string()
                    })?;
                    let bytes =
                        build_cove_o_with_source_states(&file, &inputs.rows, &inputs.states)?;
                    durable::durable_replace(&output, &bytes).map_err(|err| {
                        format!("cannot durably publish {}: {err}", output.display())
                    })?;
                }
            }
        }
        Command::Explain { map, id } => {
            let file = parse_map(&map)?;
            print_json(&explain(&file, &id)?);
        }
        Command::Diff { left, right } => {
            let left = parse_map(&left)?;
            let right = parse_map(&right)?;
            print_json(&diff_maps(&left, &right));
        }
        Command::Project {
            map,
            sources,
            output,
        } => {
            let file = parse_map(&map)?;
            let inputs = read_source_inputs(&sources)?;
            validate_source_inputs(&file, &inputs.states)?;
            let projected = project_rows_with_source_states(&file, &inputs.rows, &inputs.states)?;
            write_or_print(output, &projected)?;
        }
        Command::Test { fixture } => run_fixture_path(&fixture)?,
    }
    Ok(())
}

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

fn materialize_from_paths(map: &Path, sources: &[PathBuf]) -> Result<MaterializedModel, String> {
    let file = parse_map(map)?;
    let inputs = read_source_inputs(sources)?;
    validate_source_inputs(&file, &inputs.states)?;
    materialize_with_source_states(&file, &inputs.rows, &inputs.states)
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

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<Command>, String> {
    let mut args = args.into_iter();
    let Some(subcommand) = args.next() else {
        return Ok(None);
    };
    if subcommand == "-h" || subcommand == "--help" {
        return Ok(None);
    }
    let command = match subcommand.as_str() {
        "validate" => Command::Validate {
            map: one_path(&mut args, "validate <mapping.covemap>")?,
        },
        "preview" => Command::Preview {
            map: one_path(&mut args, "preview <mapping.covemap>")?,
        },
        "plan-keys" => {
            let map = one_path(&mut args, "plan-keys <mapping.covemap> <source...>")?;
            Command::PlanKeys {
                map,
                sources: args.map(PathBuf::from).collect(),
            }
        }
        "convert" => {
            let (output, format, positional) = parse_output_format_and_positionals(args)?;
            let mut positional = positional.into_iter();
            let map = positional
                .next()
                .ok_or_else(|| "convert requires <mapping.covemap>".to_string())?;
            Command::Convert {
                map,
                sources: positional.collect(),
                output,
                format,
            }
        }
        "explain" => {
            let map = one_path(&mut args, "explain <mapping.covemap> <goid|assertion-id>")?;
            let id = args
                .next()
                .ok_or_else(|| "explain requires an id".to_string())?;
            Command::Explain { map, id }
        }
        "diff" => Command::Diff {
            left: one_path(&mut args, "diff <left.covemap> <right.covemap>")?,
            right: one_path(&mut args, "diff <left.covemap> <right.covemap>")?,
        },
        "project" => {
            let (output, format, positional) = parse_output_format_and_positionals(args)?;
            if format != OutputFormat::Json {
                return Err("project currently supports --format json only".into());
            }
            let mut positional = positional.into_iter();
            let map = positional
                .next()
                .ok_or_else(|| "project requires <mapping.covemap>".to_string())?;
            Command::Project {
                map,
                sources: positional.collect(),
                output,
            }
        }
        "test" => Command::Test {
            fixture: one_path(&mut args, "test <fixture.json>")?,
        },
        _ => return Err(format!("unknown subcommand {subcommand}")),
    };
    Ok(Some(command))
}

fn one_path(args: &mut impl Iterator<Item = String>, usage: &str) -> Result<PathBuf, String> {
    args.next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("usage: cove-map {usage}"))
}

fn parse_output_format_and_positionals(
    args: impl Iterator<Item = String>,
) -> Result<(Option<PathBuf>, OutputFormat, Vec<PathBuf>), String> {
    let mut output = None;
    let mut format = OutputFormat::Json;
    let mut positional = Vec::new();
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        if arg == "--output" || arg == "-o" {
            output = Some(
                args.next()
                    .map(PathBuf::from)
                    .ok_or_else(|| format!("{arg} requires a path"))?,
            );
        } else if arg == "--format" {
            let raw = args
                .next()
                .ok_or_else(|| "--format requires json or cove-o".to_string())?;
            format = match raw.as_str() {
                "json" => OutputFormat::Json,
                "cove-o" => OutputFormat::CoveO,
                _ => return Err("--format must be one of: json, cove-o".into()),
            };
        } else if arg.starts_with('-') {
            return Err(format!("unknown option {arg}"));
        } else {
            positional.push(PathBuf::from(arg));
        }
    }
    Ok((output, format, positional))
}

fn parse_map(path: &Path) -> Result<CovemapFile, String> {
    let bytes = fs::read(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    CovemapFile::parse_validated(&bytes).map_err(|err| format!("{}: {err}", path.display()))
}

fn preview(file: &CovemapFile) -> Value {
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

fn plan_keys(file: &CovemapFile, rows: &[SourceRow]) -> Value {
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

fn plan_identities(file: &CovemapFile, rows: &[SourceRow]) -> Result<IdentityPlan, String> {
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
            let join_key_sha256 = sha256_hex(&tuple);
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
            let join_key_sha256 = sha256_hex(&tuple);
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
        append_len_bytes(&mut out, self.object_type.as_bytes());
        append_len_bytes(&mut out, self.identity_rule_id.as_bytes());
        if self.merge_class == IdentityMergeClass::MergeWithinSource {
            append_len_bytes(&mut out, self.source_id.as_bytes());
        }
        append_len_bytes(&mut out, &self.join_key_tuple);
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

#[cfg(test)]
fn build_cove_o(file: &CovemapFile, rows: &[SourceRow]) -> Result<Vec<u8>, String> {
    build_cove_o_with_source_states(file, rows, &[])
}

fn build_cove_o_with_source_states(
    file: &CovemapFile,
    rows: &[SourceRow],
    source_states: &[ObservedSourceState],
) -> Result<Vec<u8>, String> {
    let materialized = materialize_with_source_states(file, rows, source_states)?;
    let catalog = ObjectTypeCatalog {
        flags: 0,
        types: materialized.object_types.clone(),
    };
    let segments = build_temporal_segments(&materialized)?;
    let segment_index = temporal_segment_index(&segments)?;
    let trust_manifest = trust_manifest(&segments)?;

    let mut writer = MinimalCoveWriter::new();
    writer.primary_profile = PrimaryProfile::ObjectTemporal as u8;
    writer.required_features = FEATURE_OBJECT_PROFILE | FEATURE_TRUST_CHAIN;
    writer.optional_features = FEATURE_SEMANTIC_MAP;
    for section in map_passthrough_sections(file) {
        writer.sections.push(section);
    }
    writer.sections.push(object_section(
        SectionKind::ObjectTypeCatalog,
        catalog.types.len() as u64,
        0,
        catalog.serialize().map_err(|err| err.to_string())?,
    ));
    writer.sections.push(object_section(
        SectionKind::TemporalSegmentIndex,
        segments.len() as u64,
        materialized.rows.len() as u64,
        segment_index.serialize().map_err(|err| err.to_string())?,
    ));
    for segment in &segments {
        writer.sections.push(object_section(
            SectionKind::TemporalSegmentData,
            1,
            segment.rows.len() as u64,
            segment.payload.clone(),
        ));
    }
    writer.sections.push(object_section(
        SectionKind::TrustManifest,
        trust_manifest.entries.len() as u64,
        0,
        trust_manifest.serialize(),
    ));
    writer.sections.push(map_section(
        SectionKind::MapAssertionLog,
        materialized.assertions.len() as u64,
        serde_json::to_vec_pretty(&materialized.assertion_log).map_err(|err| err.to_string())?,
    ));
    writer.sections.push(map_section(
        SectionKind::MapIdentityEquivalenceIndex,
        materialized
            .identity_equivalence_index
            .get("equivalences")
            .and_then(Value::as_array)
            .map(|values| values.len() as u64)
            .unwrap_or(0),
        serde_json::to_vec_pretty(&materialized.identity_equivalence_index)
            .map_err(|err| err.to_string())?,
    ));
    writer.sections.push(map_section(
        SectionKind::MapEvidenceIndex,
        materialized.evidence_entries.len() as u64,
        serde_json::to_vec_pretty(&materialized.evidence_index).map_err(|err| err.to_string())?,
    ));
    writer.sections.push(map_section(
        SectionKind::MapConversionReport,
        1,
        serde_json::to_vec_pretty(&materialized.conversion_report)
            .map_err(|err| err.to_string())?,
    ));
    let bytes = writer.write();
    validate_bytes_with_options(
        &bytes,
        ValidationOptions {
            semantic: true,
            verify_digests: false,
            allow_unknown_optional_extensions: true,
            ..ValidationOptions::default()
        },
    )
    .map_err(|err| err.to_string())?;
    Ok(bytes)
}

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
        evidence_entries.push(evidence_entry_for_candidate(candidate));
    }

    for identity in planned {
        let row_rule = row_rules.get(&identity.row_rule_id).ok_or_else(|| {
            format!(
                "planned row references missing row rule '{}'",
                identity.row_rule_id
            )
        })?;
        if !row_rule_materializes_object(row_rule)? {
            continue;
        }
        let source_row = source_rows
            .get(&(identity.source_id.clone(), identity.row_index))
            .ok_or_else(|| "planned identity references missing source row".to_string())?;
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
        let assertion_id = identity_assertion_id(identity);
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
        evidence_entries.push(evidence_entry_for_identity(identity));
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
                record_kind: RecordKind::Baseline,
                properties,
            });
            push_unique_assertion(
                &mut *assertions,
                &assertion_id,
                &hex_encode(&association_goid),
            );
            evidence_entries.push(json!({
                "source_id": identity.source_id,
                "source_row_identity": identity.source_row_identity,
                "rule_id": row_rule.rule_id,
                "assertion_id": assertion_id,
                "output_object_id": hex_encode(&association_goid),
                "observed_schema_fingerprint": identity.schema_fingerprint,
            }));
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

fn evidence_entry_for_identity(identity: &PlannedIdentity) -> Value {
    json!({
        "source_id": identity.source_id,
        "source_row_identity": identity.source_row_identity,
        "rule_id": identity.row_rule_id,
        "assertion_id": identity_assertion_id(identity),
        "output_object_id": hex_encode(&identity.goid),
        "observed_schema_fingerprint": identity.schema_fingerprint,
    })
}

fn evidence_entry_for_candidate(candidate: &CandidateMatch) -> Value {
    json!({
        "source_id": candidate.source_id,
        "source_row_identity": candidate.source_row_identity,
        "rule_id": candidate.row_rule_id,
        "assertion_id": candidate_assertion_id(candidate),
        "output_object_id": candidate_match_id(candidate),
        "observed_schema_fingerprint": candidate.schema_fingerprint,
        "candidate": true,
        "identity_rule_id": candidate.identity_rule_id,
        "object_type": candidate.object_type,
        "join_key_sha256": candidate.join_key_sha256,
    })
}

fn identity_assertion_id(identity: &PlannedIdentity) -> String {
    format!(
        "assertion:{}:{}",
        identity.identity_rule_id, identity.row_digest
    )
}

fn candidate_assertion_id(candidate: &CandidateMatch) -> String {
    format!(
        "candidate:{}:{}",
        candidate.identity_rule_id, candidate.row_digest
    )
}

fn candidate_match_id(candidate: &CandidateMatch) -> String {
    format!(
        "candidate-match:{}:{}",
        candidate.identity_rule_id, candidate.join_key_sha256
    )
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
    record_kind_from_name(&row_rule.record_kind)
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
        let payload = temporal_segment_payload(segment_id, object_type, &rows)?;
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
    for (index, row) in rows.iter().enumerate() {
        out.extend_from_slice(
            &TemporalRowEntryV1 {
                timestamp_us: 0,
                csn: index as u64,
                branch_key: 0,
                goid: row.goid,
                record_id: row.record_id,
                record_kind: row.record_kind,
                prev_ref: None,
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
        let page_payload = build_property_page_payload(property, rows)?;
        let page_length = page_payload.len() as u64;
        let page_checksum = checksum::crc32c(&page_payload);
        let null_count = rows
            .iter()
            .filter(|row| {
                row.properties
                    .get(&property.property_id)
                    .map_or(true, |value| value.value.is_null())
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
    property: &PropertyEntryV1,
    rows: &[ObjectRow],
) -> Result<Vec<u8>, String> {
    let row_count = u32::try_from(rows.len()).map_err(|_| "too many rows".to_string())?;
    let mut null_bitmap = vec![0u8; (rows.len() + 7) / 8];
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
        append_property_value_bytes(property, value, &mut values)?;
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

fn append_property_value_bytes(
    property: &PropertyEntryV1,
    value: &Value,
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
            return Err("COVE-MAP writer does not assign file dictionary codes yet".into())
        }
        CovePhysicalKind::List | CovePhysicalKind::Struct | CovePhysicalKind::Map => {
            return Err("COVE-MAP writer does not materialize nested properties yet".into())
        }
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
        }
    }
    (delta, snapshot, baseline, tombstone)
}

fn trust_manifest(segments: &[TemporalSegmentBuild]) -> Result<TrustManifest, String> {
    let mut previous = [0u8; 32];
    let mut entries = Vec::new();
    for segment in segments {
        for (index, row) in segment.rows.iter().enumerate() {
            let temporal_row = TemporalRowEntryV1 {
                timestamp_us: 0,
                csn: index as u64,
                branch_key: 0,
                goid: row.goid,
                record_id: row.record_id,
                record_kind: row.record_kind,
                prev_ref: None,
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
        data,
    }
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

fn explain(file: &CovemapFile, id: &str) -> Result<Value, String> {
    for section in embedded_sections(file)? {
        if let EmbeddedMapSection::EvidenceIndex(index) = section {
            for entry in index.entries {
                if entry.assertion_id == id || entry.output_object_id == id {
                    return Ok(json!({
                        "source_id": entry.source_id,
                        "source_row_identity": entry.source_row_identity,
                        "rule_id": entry.rule_id,
                        "assertion_id": entry.assertion_id,
                        "output_object_id": entry.output_object_id,
                        "observed_schema_fingerprint": entry.observed_schema_fingerprint,
                        "observed_snapshot_digest": entry.observed_snapshot_digest,
                    }));
                }
            }
        }
    }
    Err(format!(
        "id {id} was not found in COVE-MAP evidence sections"
    ))
}

fn diff_maps(left: &CovemapFile, right: &CovemapFile) -> Value {
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

fn project_rows(file: &CovemapFile, rows: &[SourceRow]) -> Result<Value, String> {
    project_rows_with_source_states(file, rows, &[])
}

fn project_rows_with_source_states(
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

fn property_by_name(row: &ObjectRow, property_name: &str) -> Value {
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

fn mapping_context(file: &CovemapFile) -> Result<MappingContext, String> {
    let mut identity_rules = BTreeMap::new();
    let mut identity_rule_order = BTreeMap::new();
    let mut source_order = BTreeMap::new();
    let mut sources = BTreeMap::new();
    let mut governance_reconciliation_policy = "emit_effective_policy".to_string();
    let mut row_rules = Vec::new();
    let mut do_not_merge = Vec::new();
    for section in embedded_sections(file)? {
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
    match canonicalization {
        "identity" | "none" => Ok(value.clone()),
        "trim_lower" => {
            if !declared_functions.iter().any(|function| function == canonicalization) {
                return Err(format!(
                    "canonicalization function '{canonicalization}' was not declared on the identity rule"
                ));
            }
            let text = value
                .as_str()
                .ok_or_else(|| "trim_lower canonicalization requires a string value".to_string())?;
            Ok(Value::String(text.trim().to_ascii_lowercase()))
        }
        other => Err(format!(
            "canonicalization function '{other}' is not implemented by the deterministic reference runner"
        )),
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

fn read_sources(paths: &[PathBuf]) -> Result<Vec<SourceRow>, String> {
    read_source_inputs(paths).map(|inputs| inputs.rows)
}

fn read_source_inputs(paths: &[PathBuf]) -> Result<SourceInputs, String> {
    let mut rows = Vec::new();
    let mut states = Vec::new();
    for path in paths {
        let source_id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("source")
            .to_string();
        let bytes =
            fs::read(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
        let source_kind = match path.extension().and_then(|ext| ext.to_str()) {
            Some("jsonl") => "jsonl",
            Some("csv") => "csv",
            _ => return Err(format!("{} must be .jsonl or .csv", path.display())),
        };
        let before_len = rows.len();
        match source_kind {
            "jsonl" => rows.extend(read_jsonl(path, &source_id)?),
            "csv" => rows.extend(read_csv(path, &source_id)?),
            _ => unreachable!(),
        }
        let source_rows = &rows[before_len..];
        states.push(ObservedSourceState {
            source_id,
            source_kind: source_kind.to_string(),
            schema_fingerprint: observed_schema_fingerprint(source_kind, source_rows),
            snapshot_digest: format!("sha256:{}", sha256_hex(&bytes)),
        });
    }
    Ok(SourceInputs { rows, states })
}

fn read_jsonl(path: &Path, source_id: &str) -> Result<Vec<SourceRow>, String> {
    let text =
        fs::read_to_string(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            let value: Value = serde_json::from_str(line)
                .map_err(|err| format!("{}:{} invalid JSONL: {err}", path.display(), index + 1))?;
            let object = value.as_object().ok_or_else(|| {
                format!(
                    "{}:{} JSONL row must be an object",
                    path.display(),
                    index + 1
                )
            })?;
            Ok(SourceRow {
                source_id: source_id.to_string(),
                row_index: index,
                values: object_to_btree(object),
            })
        })
        .collect()
}

fn read_csv(path: &Path, source_id: &str) -> Result<Vec<SourceRow>, String> {
    let text =
        fs::read_to_string(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let header = lines
        .next()
        .ok_or_else(|| format!("{} is empty", path.display()))?
        .split(',')
        .map(|field| field.trim().to_string())
        .collect::<Vec<_>>();
    lines
        .enumerate()
        .map(|(index, line)| {
            let fields = line.split(',').map(str::trim).collect::<Vec<_>>();
            if fields.len() != header.len() {
                return Err(format!(
                    "{}:{} field count {} did not match header count {}",
                    path.display(),
                    index + 2,
                    fields.len(),
                    header.len()
                ));
            }
            let values = header
                .iter()
                .cloned()
                .zip(
                    fields
                        .into_iter()
                        .map(|field| Value::String(field.to_string())),
                )
                .collect::<BTreeMap<_, _>>();
            Ok(SourceRow {
                source_id: source_id.to_string(),
                row_index: index,
                values,
            })
        })
        .collect()
}

fn validate_source_inputs(
    file: &CovemapFile,
    states: &[ObservedSourceState],
) -> Result<(), String> {
    let context = mapping_context(file)?;
    let mut observed = BTreeMap::<String, &ObservedSourceState>::new();
    for state in states {
        if observed.insert(state.source_id.clone(), state).is_some() {
            return Err(format!(
                "source '{}' was supplied more than once",
                state.source_id
            ));
        }
        let expected = context.sources.get(&state.source_id).ok_or_else(|| {
            format!(
                "source '{}' is not declared by the mapping",
                state.source_id
            )
        })?;
        if expected.replay_claimed {
            let expected_schema = expected.schema_fingerprint.as_deref().ok_or_else(|| {
                format!(
                    "source '{}' claims replayability but has no schema_fingerprint",
                    state.source_id
                )
            })?;
            let expected_digest = expected.snapshot_digest.as_deref().ok_or_else(|| {
                format!(
                    "source '{}' claims replayability but has no snapshot_digest",
                    state.source_id
                )
            })?;
            if !is_reference_schema_fingerprint(expected_schema) {
                return Err(format!(
                    "source '{}' replay schema_fingerprint must use cove-map-schema-v1:<sha256>",
                    state.source_id
                ));
            }
            if !is_sha256_digest(expected_digest) {
                return Err(format!(
                    "source '{}' replay snapshot_digest must use sha256:<64 hex>",
                    state.source_id
                ));
            }
            if expected_schema != state.schema_fingerprint
                || expected_digest != state.snapshot_digest
            {
                return Err(format!(
                    "source '{}' does not match replay fingerprint",
                    state.source_id
                ));
            }
            continue;
        }
        if expected
            .schema_fingerprint
            .as_deref()
            .is_some_and(is_reference_schema_fingerprint)
            && expected.schema_fingerprint.as_deref() != Some(state.schema_fingerprint.as_str())
        {
            return Err(format!(
                "source '{}' schema_fingerprint mismatch",
                state.source_id
            ));
        }
        if expected
            .snapshot_digest
            .as_deref()
            .is_some_and(is_sha256_digest)
            && expected.snapshot_digest.as_deref() != Some(state.snapshot_digest.as_str())
        {
            return Err(format!(
                "source '{}' snapshot_digest mismatch",
                state.source_id
            ));
        }
    }
    let row_sources = context
        .row_rules
        .iter()
        .map(|rule| rule.source_id.as_str())
        .collect::<BTreeSet<_>>();
    for (source_id, source) in &context.sources {
        if (source.replay_claimed || row_sources.contains(source_id.as_str()))
            && !observed.contains_key(source_id)
        {
            return Err(format!(
                "source '{}' is required by the mapping but was not supplied",
                source_id
            ));
        }
    }
    Ok(())
}

fn observed_schema_fingerprint(source_kind: &str, rows: &[SourceRow]) -> String {
    let mut fields = BTreeMap::<String, BTreeSet<String>>::new();
    for row in rows {
        for (key, value) in &row.values {
            fields
                .entry(key.clone())
                .or_default()
                .insert(json_primitive_kind(value).to_string());
        }
    }
    let schema = fields
        .into_iter()
        .map(|(key, kinds)| format!("{key}:{}", kinds.into_iter().collect::<Vec<_>>().join(",")))
        .collect::<Vec<_>>()
        .join("|");
    format!(
        "cove-map-schema-v1:{}",
        sha256_hex(format!("{source_kind}\n{schema}").as_bytes())
    )
}

fn json_primitive_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(number) if number.is_i64() => "int",
        Value::Number(number) if number.is_u64() => "uint",
        Value::Number(_) => "float",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn is_reference_schema_fingerprint(value: &str) -> bool {
    value
        .strip_prefix("cove-map-schema-v1:")
        .is_some_and(is_lower_hex_sha256)
}

fn is_sha256_digest(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(is_lower_hex_sha256)
}

fn is_lower_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn mapping_identity(file: &CovemapFile) -> Result<(String, String), String> {
    for section in embedded_sections(file)? {
        match section {
            EmbeddedMapSection::SourceCatalog(section) => {
                return Ok((section.mapping_id, section.mapping_version));
            }
            EmbeddedMapSection::FunctionRegistry(section) => {
                return Ok((section.mapping_id, section.mapping_version));
            }
            EmbeddedMapSection::IdentityRuleCatalog(section) => {
                return Ok((section.mapping_id, section.mapping_version));
            }
            EmbeddedMapSection::RowSemanticsCatalog(section) => {
                return Ok((section.mapping_id, section.mapping_version));
            }
            EmbeddedMapSection::AssertionLog(section) => {
                return Ok((section.mapping_id, section.mapping_version));
            }
            EmbeddedMapSection::IdentityEquivalenceIndex(section) => {
                return Ok((section.mapping_id, section.mapping_version));
            }
            EmbeddedMapSection::EvidenceIndex(section) => {
                return Ok((section.mapping_id, section.mapping_version));
            }
            EmbeddedMapSection::ConversionReport(section) => {
                return Ok((section.mapping_id, section.mapping_version));
            }
            EmbeddedMapSection::ProjectionCatalog(section) => {
                return Ok((section.mapping_id, section.mapping_version));
            }
        }
    }
    Err("mapping contains no embedded sections".into())
}

fn embedded_sections(file: &CovemapFile) -> Result<Vec<EmbeddedMapSection>, String> {
    let mut out = Vec::new();
    for section in &file.sections {
        let kind = u16::try_from(section.entry.section_id)
            .ok()
            .and_then(SectionKind::from_u16)
            .ok_or_else(|| "invalid COVE-MAP section id".to_string())?;
        out.push(
            parse_embedded_section(kind, &section.payload)
                .map_err(|err| format!("invalid embedded map section: {err}"))?,
        );
    }
    Ok(out)
}

fn logical_type_from_name(name: &str) -> Result<CoveLogicalType, String> {
    match name {
        "bool" | "boolean" => Ok(CoveLogicalType::Bool),
        "int64" | "int" => Ok(CoveLogicalType::Int64),
        "uint64" | "uint" => Ok(CoveLogicalType::UInt64),
        "float64" => Ok(CoveLogicalType::Float64),
        "utf8" | "string" => Ok(CoveLogicalType::Utf8),
        "binary" => Ok(CoveLogicalType::Binary),
        "json" => Ok(CoveLogicalType::Json),
        "uuid" => Ok(CoveLogicalType::Uuid),
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

fn section_kind(section_id: u32) -> String {
    u16::try_from(section_id)
        .ok()
        .and_then(SectionKind::from_u16)
        .map(|kind| format!("{kind:?}"))
        .unwrap_or_else(|| format!("Unknown({section_id})"))
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

fn print_json(value: &Value) {
    println!("{}", serde_json::to_string_pretty(value).unwrap());
}

fn write_or_print(output: Option<PathBuf>, value: &Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|err| format!("cannot serialize JSON output: {err}"))?;
    if let Some(output) = output {
        fs::write(&output, text).map_err(|err| format!("cannot write {}: {err}", output.display()))
    } else {
        println!("{text}");
        Ok(())
    }
}

fn print_usage() {
    println!(
        "Usage: cove-map <subcommand> [options]\n\n\
Subcommands:\n  \
validate <mapping.covemap>\n  \
preview <mapping.covemap>\n  \
plan-keys <mapping.covemap> <source.csv|source.jsonl>...\n  \
convert [--format json|cove-o] [-o output] <mapping.covemap> <source.csv|source.jsonl>...\n  \
explain <mapping.covemap> <goid|assertion-id>\n  \
diff <left.covemap> <right.covemap>\n  \
project [-o output.json] <mapping.covemap> <source.csv|source.jsonl>...\n  \
test <fixture.json>"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use cove_core::{
        artifact::covemap::{
            CovemapHeaderV1, CovemapPostscriptV1, CovemapSection, CovemapSectionEntryV1,
        },
        compression,
        constants::FEATURE_SEMANTIC_MAP,
        profile::cove_o::TemporalSegmentData,
    };

    fn test_section(kind: SectionKind, value: Value) -> CovemapSection {
        let payload = serde_json::to_vec_pretty(&value).unwrap();
        CovemapSection {
            entry: CovemapSectionEntryV1 {
                section_id: kind as u32,
                offset: 0,
                length: payload.len() as u64,
                uncompressed_length: payload.len() as u64,
                compression: 0,
                required: true,
                reserved: 0,
                checksum: 0,
            },
            payload,
        }
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
            let payload = serde_json::to_vec_pretty(&value).unwrap();
            CovemapSection {
                entry: CovemapSectionEntryV1 {
                    section_id: kind as u32,
                    offset: 0,
                    length: payload.len() as u64,
                    uncompressed_length: payload.len() as u64,
                    compression: 0,
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
