use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use cove_core::{
    artifact::covemap::CovemapFile,
    canonical::CanonicalValue,
    constants::{
        CoveLogicalType, CovePhysicalKind, PrimaryProfile, SectionKind, FEATURE_OBJECT_PROFILE,
        FEATURE_SEMANTIC_MAP, FEATURE_TRUST_CHAIN,
    },
    profile::{
        cove_map::{
            parse_embedded_section, EmbeddedMapSection, MapIdentityRule, MapIdentityRuleCatalog,
            MapRowSemanticRule, MapRowSemanticsCatalog,
        },
        cove_o::{
            ObjectTypeCatalog, ObjectTypeEntryV1, PropertyEntryV1, RecordKind, TemporalRowEntryV1,
            TemporalSegmentHeaderV1, TemporalSegmentIndex, TemporalSegmentIndexEntryV1,
            TrustManifest, TrustManifestEntryV1, TEMPORAL_ROW_ENTRY_LEN,
            TEMPORAL_SEGMENT_HEADER_LEN,
        },
    },
    reader::{validate_bytes_with_options, ValidationOptions},
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
struct MappingContext {
    identity_rules: BTreeMap<String, MapIdentityRule>,
    row_rules: Vec<MapRowSemanticRule>,
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
    join_key_tuple: Vec<u8>,
    goid: [u8; 16],
}

fn main() -> ExitCode {
    match run(env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("cove-map: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: impl IntoIterator<Item = String>) -> Result<(), String> {
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
            let rows = read_sources(&sources)?;
            print_json(&plan_keys(&file, &rows));
        }
        Command::Convert {
            map,
            sources,
            output,
            format,
        } => {
            let file = parse_map(&map)?;
            let rows = read_sources(&sources)?;
            match format {
                OutputFormat::Json => {
                    plan_identities(&file, &rows)?;
                    let report = conversion_report(&file, &rows);
                    write_or_print(output, &report)?;
                }
                OutputFormat::CoveO => {
                    let output = output.ok_or_else(|| {
                        "convert --format cove-o requires --output <path>".to_string()
                    })?;
                    let bytes = build_cove_o(&file, &rows)?;
                    fs::write(&output, bytes)
                        .map_err(|err| format!("cannot write {}: {err}", output.display()))?;
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
            let rows = read_sources(&sources)?;
            write_or_print(output, &project_rows(&file, &rows))?;
        }
        Command::Test { fixture } => run_fixture(&fixture)?,
    }
    Ok(())
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
        "rows": planned.iter().map(|identity| {
            json!({
                "source_id": identity.source_id,
                "row_index": identity.row_index,
                "source_row_identity": identity.source_row_identity,
                "row_digest": identity.row_digest,
                "row_rule_id": identity.row_rule_id,
                "identity_rule_id": identity.identity_rule_id,
                "object_type": identity.object_type,
                "join_key_sha256": sha256_hex(&identity.join_key_tuple),
                "goid": hex_encode(&identity.goid),
            })
        }).collect::<Vec<_>>()
    })
}

fn plan_identities(file: &CovemapFile, rows: &[SourceRow]) -> Result<Vec<PlannedIdentity>, String> {
    let context = mapping_context(file)?;
    let mapping_id = hex_encode(&file.header.mapping_id);
    let mut planned = Vec::new();
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
            if identity_rule.candidate_only || identity_rule.confidence_class == "candidate" {
                return Err(format!(
                    "identity rule '{}' is candidate-only and cannot materialize GOIDs",
                    identity_rule.rule_id
                ));
            }
            let tuple = join_key_tuple_from_rule(identity_rule, row)?;
            let goid = goid16(
                mapping_id.as_bytes(),
                file.mapping_version.as_bytes(),
                identity_rule.object_type.as_bytes(),
                identity_rule.rule_id.as_bytes(),
                &tuple,
            );
            planned.push(PlannedIdentity {
                source_id: row.source_id.clone(),
                row_index: row.row_index,
                row_digest: row_digest(row),
                schema_fingerprint: schema_fingerprint(row),
                source_row_identity: format!("{}:{}", row.source_id, row.row_index),
                row_rule_id: row_rule.rule_id.clone(),
                identity_rule_id: identity_rule.rule_id.clone(),
                object_type: identity_rule.object_type.clone(),
                join_key_tuple: tuple,
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
    Ok(planned)
}

fn conversion_report(file: &CovemapFile, rows: &[SourceRow]) -> Value {
    let (mapping_id, mapping_version) = mapping_identity(file).unwrap_or_else(|_| {
        (
            hex_encode(&file.header.mapping_id),
            file.mapping_version.clone(),
        )
    });
    let sources = rows
        .iter()
        .map(|row| {
            json!({
                "source_id": row.source_id,
                "schema_fingerprint": schema_fingerprint(row),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "mapping_id": mapping_id,
        "mapping_version": mapping_version,
        "sources": sources,
        "source_count": rows.iter().map(|row| row.source_id.clone()).collect::<BTreeSet<_>>().len(),
        "row_count": rows.len(),
        "generated_artifacts": ["json-conversion-report", "json-evidence"],
        "unsupported": [],
        "evidence": rows.iter().map(evidence_entry).collect::<Vec<_>>(),
    })
}

fn build_cove_o(file: &CovemapFile, rows: &[SourceRow]) -> Result<Vec<u8>, String> {
    let context = mapping_context(file)?;
    let planned = plan_identities(file, rows)?;
    let object_rows = object_rows(&planned)?;
    let catalog = ObjectTypeCatalog {
        flags: 0,
        types: object_types_from_mapping(&context)?,
    };
    let segment_payload = temporal_segment_payload(&object_rows)?;
    let segment_index = temporal_segment_index(&object_rows, segment_payload.len())?;
    let trust_manifest = trust_manifest(&object_rows)?;
    let evidence = map_evidence_json(file, rows, &planned)?;
    let conversion = conversion_report(file, rows);

    let mut writer = MinimalCoveWriter::new();
    writer.primary_profile = PrimaryProfile::ObjectTemporal as u8;
    writer.required_features = FEATURE_OBJECT_PROFILE | FEATURE_TRUST_CHAIN | FEATURE_SEMANTIC_MAP;
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
        1,
        object_rows.len() as u64,
        segment_index.serialize().map_err(|err| err.to_string())?,
    ));
    writer.sections.push(object_section(
        SectionKind::TemporalSegmentData,
        1,
        object_rows.len() as u64,
        segment_payload,
    ));
    writer.sections.push(object_section(
        SectionKind::TrustManifest,
        trust_manifest.entries.len() as u64,
        0,
        trust_manifest.serialize(),
    ));
    writer.sections.push(map_section(
        SectionKind::MapAssertionLog,
        planned.len() as u64,
        serde_json::to_vec_pretty(&map_assertion_log_json(file, &planned)?)
            .map_err(|err| err.to_string())?,
    ));
    writer.sections.push(map_section(
        SectionKind::MapEvidenceIndex,
        planned.len() as u64,
        serde_json::to_vec_pretty(&evidence).map_err(|err| err.to_string())?,
    ));
    writer.sections.push(map_section(
        SectionKind::MapConversionReport,
        1,
        serde_json::to_vec_pretty(&conversion).map_err(|err| err.to_string())?,
    ));
    let bytes = writer.write();
    validate_bytes_with_options(
        &bytes,
        ValidationOptions {
            semantic: true,
            verify_digests: false,
            allow_unknown_optional_extensions: true,
        },
    )
    .map_err(|err| err.to_string())?;
    Ok(bytes)
}

#[derive(Debug, Clone)]
struct ObjectRow {
    goid: [u8; 16],
    record_id: [u8; 16],
    source_row_index: usize,
}

fn object_rows(planned: &[PlannedIdentity]) -> Result<Vec<ObjectRow>, String> {
    let mut out = planned
        .iter()
        .map(|identity| {
            let record_material = format!(
                "{}:{}:{}:{}",
                identity.source_id,
                identity.row_index,
                identity.row_rule_id,
                hex_encode(&identity.goid)
            );
            ObjectRow {
                goid: identity.goid,
                record_id: first_16(&sha256_array(record_material.as_bytes())),
                source_row_index: identity.row_index,
            }
        })
        .collect::<Vec<_>>();
    out.sort_by_key(|row| (row.source_row_index, row.goid, row.record_id));
    Ok(out)
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
        for row_rule in &context.row_rules {
            let Some(identity_rule) = context.identity_rules.get(&row_rule.identity_rule_id) else {
                continue;
            };
            if identity_rule.object_type != type_name {
                continue;
            }
            for (property_index, binding) in row_rule.property_bindings.iter().enumerate() {
                let logical = logical_type_from_name(&binding.logical_type)?;
                properties.push(PropertyEntryV1 {
                    property_id: stable_u32(&binding.property_id, property_index as u32 + 1),
                    property_name: binding.property_name.clone(),
                    logical_type: logical,
                    physical_kind: physical_for_logical(logical),
                    nullable: true,
                    collation_id: 0,
                    flags: 0,
                });
            }
        }
        if type_name.starts_with("Association:") {
            properties.push(PropertyEntryV1 {
                property_id: 1,
                property_name: "source_goid".into(),
                logical_type: CoveLogicalType::Uuid,
                physical_kind: CovePhysicalKind::FixedBytes,
                nullable: false,
                collation_id: 0,
                flags: 0,
            });
            properties.push(PropertyEntryV1 {
                property_id: 2,
                property_name: "target_goid".into(),
                logical_type: CoveLogicalType::Uuid,
                physical_kind: CovePhysicalKind::FixedBytes,
                nullable: false,
                collation_id: 0,
                flags: 0,
            });
        }
        out.push(ObjectTypeEntryV1 {
            object_type_id: (index + 1) as u32,
            type_name,
            properties,
        });
    }
    Ok(out)
}

fn temporal_segment_payload(rows: &[ObjectRow]) -> Result<Vec<u8>, String> {
    let row_count = u32::try_from(rows.len()).map_err(|_| "too many COVE-O rows".to_string())?;
    let row_directory_offset = TEMPORAL_SEGMENT_HEADER_LEN as u64;
    let row_bytes_len = rows
        .len()
        .checked_mul(TEMPORAL_ROW_ENTRY_LEN)
        .ok_or_else(|| "temporal row directory length overflow".to_string())?;
    let column_directory_offset = row_directory_offset
        .checked_add(row_bytes_len as u64)
        .ok_or_else(|| "temporal offset overflow".to_string())?;
    let header = TemporalSegmentHeaderV1 {
        segment_id: 0,
        object_type_id: 1,
        time_range_start_us: 0,
        time_range_end_us: 0,
        csn_min: 0,
        csn_max: rows.len().saturating_sub(1) as u64,
        row_count,
        morsel_count: if row_count == 0 { 0 } else { 1 },
        morsel_row_count: if row_count == 0 { 0 } else { row_count },
        column_count: 0,
        row_directory_offset,
        column_directory_offset,
        page_index_offset: column_directory_offset,
        data_offset: column_directory_offset,
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
                record_kind: RecordKind::Baseline,
                prev_ref: None,
            }
            .serialize(),
        );
    }
    Ok(out)
}

fn temporal_segment_index(
    rows: &[ObjectRow],
    segment_payload_len: usize,
) -> Result<TemporalSegmentIndex, String> {
    let min_goid = rows.iter().map(|row| row.goid).min().unwrap_or([0; 16]);
    let max_goid = rows.iter().map(|row| row.goid).max().unwrap_or([0; 16]);
    Ok(TemporalSegmentIndex {
        flags: 0,
        entries: vec![TemporalSegmentIndexEntryV1 {
            segment_id: 0,
            object_type_id: 1,
            time_range_start_us: 0,
            time_range_end_us: 0,
            csn_min: 0,
            csn_max: rows.len().saturating_sub(1) as u64,
            row_count: u32::try_from(rows.len()).map_err(|_| "too many COVE-O rows".to_string())?,
            delta_count: 0,
            snapshot_count: 0,
            baseline_count: u32::try_from(rows.len())
                .map_err(|_| "too many COVE-O rows".to_string())?,
            tombstone_count: 0,
            min_goid,
            max_goid,
            offset: 0,
            length: segment_payload_len as u64,
            checksum: 0,
        }],
    })
}

fn trust_manifest(rows: &[ObjectRow]) -> Result<TrustManifest, String> {
    let mut previous = [0u8; 32];
    let mut entries = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let temporal_row = TemporalRowEntryV1 {
            timestamp_us: 0,
            csn: index as u64,
            branch_key: 0,
            goid: row.goid,
            record_id: row.record_id,
            record_kind: RecordKind::Baseline,
            prev_ref: None,
        };
        let expected_hash = trust_chain::chain(&previous, &temporal_row.trust_payload())
            .map_err(|err| err.to_string())?;
        entries.push(TrustManifestEntryV1 {
            segment_id: 0,
            row_index: index as u32,
            expected_hash,
        });
        previous = expected_hash;
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
        required_features: FEATURE_SEMANTIC_MAP,
        optional_features: 0,
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

fn map_assertion_log_json(
    file: &CovemapFile,
    planned: &[PlannedIdentity],
) -> Result<Value, String> {
    let (mapping_id, mapping_version) = mapping_identity(file)?;
    let context = mapping_context(file)?;
    let mut assertions = planned
        .iter()
        .map(|identity| {
            json!({
                "assertion_id": format!("assertion:{}", identity.row_digest),
                "output_object_id": hex_encode(&identity.goid),
            })
        })
        .collect::<Vec<_>>();
    for row_rule in &context.row_rules {
        for binding in &row_rule.property_bindings {
            assertions.push(json!({
                "assertion_id": binding.assertion_id,
                "output_object_id": format!("property:{}", binding.assertion_id),
            }));
        }
        for binding in &row_rule.association_bindings {
            assertions.push(json!({
                "assertion_id": binding.assertion_id,
                "output_object_id": format!("association:{}", binding.assertion_id),
            }));
        }
    }
    Ok(json!({
        "mapping_id": mapping_id,
        "mapping_version": mapping_version,
        "assertions": assertions
    }))
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

fn project_rows(file: &CovemapFile, rows: &[SourceRow]) -> Value {
    let keys = plan_keys(file, rows);
    json!({
        "format": "json",
        "row_grain": "source-row",
        "rows": keys["rows"].clone(),
    })
}

fn run_fixture(path: &Path) -> Result<(), String> {
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
        let projected = project_rows(&file, &rows);
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
    null_policy: &'a str,
    value: Option<&'a [u8]>,
}

fn join_key_tuple(identity_rule_id: &str, components: &[JoinKeyComponent<'_>]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"COVE-MAP-JOIN-KEY-V1");
    append_len_bytes(&mut out, identity_rule_id.as_bytes());
    for component in components {
        append_len_bytes(&mut out, component.role_id.as_bytes());
        append_len_bytes(&mut out, component.logical_type_id.as_bytes());
        append_len_bytes(&mut out, component.null_policy.as_bytes());
        match component.value {
            None => append_len_bytes(&mut out, b"<NULL>"),
            Some(value) => append_len_bytes(&mut out, value),
        }
    }
    out
}

fn mapping_context(file: &CovemapFile) -> Result<MappingContext, String> {
    let mut identity_rules = BTreeMap::new();
    let mut row_rules = Vec::new();
    for section in embedded_sections(file)? {
        match section {
            EmbeddedMapSection::IdentityRuleCatalog(MapIdentityRuleCatalog {
                identity_rules: rules,
                ..
            }) => {
                for rule in rules {
                    if identity_rules.insert(rule.rule_id.clone(), rule).is_some() {
                        return Err("duplicate identity rule".into());
                    }
                }
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
        row_rules,
    })
}

fn join_key_tuple_from_rule(rule: &MapIdentityRule, row: &SourceRow) -> Result<Vec<u8>, String> {
    let mut encoded_values = Vec::<Vec<u8>>::with_capacity(rule.join_keys.len());
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
            encoded_values.push(Vec::new());
            continue;
        }
        let value = apply_canonicalization(
            raw_value.unwrap(),
            &component.canonicalization,
            &rule.function_ids,
        )?;
        encoded_values.push(canonical_component_bytes(&component.logical_type, &value)?);
    }
    let components = rule
        .join_keys
        .iter()
        .zip(encoded_values.iter())
        .map(|(component, bytes)| JoinKeyComponent {
            role_id: component.role_id.as_str(),
            logical_type_id: component.logical_type.as_str(),
            null_policy: component.null_policy.as_str(),
            value: Some(bytes.as_slice()),
        })
        .collect::<Vec<_>>();
    Ok(join_key_tuple(&rule.rule_id, &components))
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

fn goid16(
    mapping_id: &[u8],
    mapping_version: &[u8],
    object_type: &[u8],
    identity_rule_id: &[u8],
    join_key_tuple: &[u8],
) -> [u8; 16] {
    let mut hasher = Sha256::new();
    for part in [
        mapping_id,
        mapping_version,
        object_type,
        identity_rule_id,
        join_key_tuple,
    ] {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    let digest = hasher.finalize();
    let mut out = [0u8; 16];
    out.copy_from_slice(&digest[..16]);
    out
}

fn read_sources(paths: &[PathBuf]) -> Result<Vec<SourceRow>, String> {
    let mut rows = Vec::new();
    for path in paths {
        let source_id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("source")
            .to_string();
        match path.extension().and_then(|ext| ext.to_str()) {
            Some("jsonl") => rows.extend(read_jsonl(path, &source_id)?),
            Some("csv") => rows.extend(read_csv(path, &source_id)?),
            _ => return Err(format!("{} must be .jsonl or .csv", path.display())),
        }
    }
    Ok(rows)
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

fn evidence_entry(row: &SourceRow) -> Value {
    json!({
        "source_id": row.source_id,
        "schema_fingerprint": schema_fingerprint(row),
        "source_row_identity": format!("{}:{}", row.source_id, row.row_index),
        "row_digest": row_digest(row),
        "mapping_id": "local-json",
        "mapping_version": "local",
        "rule_id": "row",
        "assertion_id": format!("assertion:{}", row_digest(row)),
        "output_goid": sha256_hex(row_digest(row).as_bytes())[..32].to_string(),
    })
}

fn map_evidence_json(
    file: &CovemapFile,
    rows: &[SourceRow],
    planned: &[PlannedIdentity],
) -> Result<Value, String> {
    let (mapping_id, mapping_version) = mapping_identity(file)?;
    let rows_by_key = rows
        .iter()
        .map(|row| ((row.source_id.clone(), row.row_index), row))
        .collect::<BTreeMap<_, _>>();
    let entries = planned
        .iter()
        .map(|identity| {
            let mut entry = Map::new();
            entry.insert("source_id".into(), json!(identity.source_id));
            entry.insert(
                "source_row_identity".into(),
                json!(identity.source_row_identity),
            );
            entry.insert("rule_id".into(), json!(identity.row_rule_id));
            entry.insert(
                "assertion_id".into(),
                json!(format!("assertion:{}", identity.row_digest)),
            );
            entry.insert("output_object_id".into(), json!(hex_encode(&identity.goid)));
            if rows_by_key
                .get(&(identity.source_id.clone(), identity.row_index))
                .is_some()
            {
                entry.insert(
                    "observed_schema_fingerprint".into(),
                    json!(identity.schema_fingerprint),
                );
            }
            Value::Object(entry)
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "mapping_id": mapping_id,
        "mapping_version": mapping_version,
        "entries": entries
    }))
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
        constants::FEATURE_SEMANTIC_MAP,
    };

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
                null_policy: "reject-null",
                value: Some(b"a@example.com"),
            },
            JoinKeyComponent {
                role_id: "tenant",
                logical_type_id: "utf8",
                null_policy: "reject-null",
                value: Some(b"t1"),
            },
        ];
        assert_eq!(
            join_key_tuple("person_by_email", &components),
            join_key_tuple("person_by_email", &components)
        );
    }

    #[test]
    fn goid_is_sha256_truncated_to_16_bytes() {
        let goid = goid16(b"map", b"v1", b"person", b"rule", b"key");
        assert_eq!(goid.len(), 16);
        assert_eq!(goid, goid16(b"map", b"v1", b"person", b"rule", b"key"));
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
            },
        )
        .unwrap();
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
                SectionKind::MapEvidenceIndex,
                SectionKind::MapConversionReport,
            ]
        );
    }
}
