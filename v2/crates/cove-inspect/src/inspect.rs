use std::{collections::BTreeSet, fs, path::Path};

use cove_core::{
    artifact::covemap::CovemapFile,
    compression,
    constants::{SectionKind, StorageClass, ValueTag, MAGIC_COVE, MAGIC_COVEMAP},
    mount::{mount_cove_file, MountOptions},
    reader,
    segment::TableSegmentIndex,
    table::TableCatalog,
};
use serde_json::{json, Value};

use crate::format::{comp_name, feature_names, profile_name, section_kind_name};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InspectSections {
    All,
    Only(BTreeSet<String>),
}

impl InspectSections {
    pub(crate) fn parse(raw: &str) -> Result<Self, String> {
        let mut groups = BTreeSet::new();
        for group in raw
            .split(',')
            .map(str::trim)
            .filter(|group| !group.is_empty())
        {
            match group {
                "all" => return Ok(Self::All),
                "stats" | "dictionary" | "execution" | "indexes" | "optional" => {
                    groups.insert(group.to_string());
                }
                other => {
                    return Err(format!(
                        "unknown --sections group {other}; expected stats, dictionary, execution, indexes, optional"
                    ));
                }
            }
        }
        if groups.is_empty() {
            return Err("--sections cannot be empty".into());
        }
        Ok(Self::Only(groups))
    }

    fn includes(&self, group: &str) -> bool {
        match self {
            Self::All => true,
            Self::Only(groups) => groups.contains(group),
        }
    }
}

pub(crate) fn inspect_file(path: &Path) -> Result<(), String> {
    let data = fs::read(path).map_err(|e| format!("{}: {}", path.display(), e))?;

    if data.len() < 4 {
        return Err(format!("{}: invalid trailing magic", path.display()));
    }

    if data[data.len() - 4..] == MAGIC_COVEMAP {
        return inspect_covemap_file(path, &data);
    }

    if data[data.len() - 4..] != MAGIC_COVE {
        return Err(format!("{}: invalid trailing magic", path.display()));
    }

    inspect_cove_file(path, &data)
}

pub(crate) fn inspect_file_json(path: &Path, groups: &InspectSections) -> Result<Value, String> {
    let data = fs::read(path).map_err(|e| format!("{}: {}", path.display(), e))?;
    if data.len() < 4 {
        return Err(format!("{}: invalid trailing magic", path.display()));
    }
    if data[data.len() - 4..] == MAGIC_COVEMAP {
        return inspect_covemap_json(path, &data);
    }
    if data[data.len() - 4..] != MAGIC_COVE {
        return Err(format!("{}: invalid trailing magic", path.display()));
    }
    inspect_cove_json(path, &data, groups)
}

fn inspect_cove_file(path: &Path, data: &[u8]) -> Result<(), String> {
    let parsed = reader::validate_bytes(data).map_err(|e| format!("validation: {e}"))?;
    let header = &parsed.header;
    let postscript = &parsed.postscript;
    let footer = &parsed.footer;

    println!("File: {}", path.display());
    println!("  Size            : {}", data.len());
    println!(
        "  Version         : {}.{}",
        header.version_major, header.version_minor
    );
    println!(
        "  Primary Profile : {}",
        profile_name(header.primary_profile)
    );

    let req_names = feature_names(header.required_features);
    println!("  Required Feat   : 0x{:016x}", header.required_features);
    if !req_names.is_empty() {
        println!("    flags: {}", req_names.join(", "));
    }

    let opt_names = feature_names(header.optional_features);
    println!("  Optional Feat   : 0x{:016x}", header.optional_features);
    if !opt_names.is_empty() {
        println!("    flags: {}", opt_names.join(", "));
    }

    println!(
        "  Footer          : offset={} len={} sections={}",
        postscript.footer.offset,
        postscript.footer.length,
        footer.sections.len()
    );

    for section in &footer.sections {
        let kind_name = SectionKind::from_u16(section.section_kind)
            .map(|kind| format!("{kind:?}"))
            .unwrap_or_else(|| format!("Unknown({})", section.section_kind));
        println!(
            "    - id={} kind={} offset={} len={} rows={} items={} comp={}",
            section.section_id,
            kind_name,
            section.offset,
            section.length,
            section.row_count,
            section.item_count,
            comp_name(section.compression),
        );
    }

    if !footer.metadata_json.is_empty() {
        let preview = String::from_utf8_lossy(&footer.metadata_json)
            .chars()
            .take(120)
            .collect::<String>()
            .replace('\n', " ");
        println!("  Metadata Preview: {}", preview);
    }

    print_table_summary(data, &parsed)?;
    Ok(())
}

fn inspect_cove_json(path: &Path, data: &[u8], groups: &InspectSections) -> Result<Value, String> {
    let parsed = reader::validate_bytes(data).map_err(|e| format!("validation: {e}"))?;
    let mounted =
        mount_cove_file(data, MountOptions::default(), None).map_err(|e| format!("mount: {e}"))?;
    let header = &parsed.header;
    let footer = &parsed.footer;
    let mut object = serde_json::Map::new();
    object.insert("path".into(), json!(path.display().to_string()));
    object.insert("artifact".into(), json!("COVE"));
    object.insert("size".into(), json!(data.len()));
    object.insert(
        "version".into(),
        json!({"major": header.version_major, "minor": header.version_minor}),
    );
    object.insert(
        "primary_profile".into(),
        json!(profile_name(header.primary_profile)),
    );
    object.insert(
        "features".into(),
        json!({
            "required": header.required_features,
            "optional": header.optional_features,
            "required_names": feature_names(header.required_features),
            "optional_names": feature_names(header.optional_features),
        }),
    );
    object.insert(
        "footer".into(),
        json!({
            "sections": footer.sections.len(),
            "metadata_len": footer.metadata_json.len(),
        }),
    );
    object.insert(
        "tables".into(),
        json!(mounted
            .tables
            .iter()
            .map(|table| {
                json!({
                    "table_id": table.table_id,
                    "namespace": table.namespace,
                    "name": table.name,
                    "rows": table.row_count,
                    "columns": table.columns.len(),
                })
            })
            .collect::<Vec<_>>()),
    );

    if groups.includes("stats") {
        object.insert(
            "stats".into(),
            json!({
                "zone_stats_sections": count_sections(&parsed, SectionKind::ZoneStats),
                "zone_stats_section_ids": section_ids(&parsed, SectionKind::ZoneStats),
                "zone_stats_groups": mounted.zone_stats.len(),
                "zone_stats": zone_stats_summary(&mounted.zone_stats),
                "aggregate_synopsis_sections": count_sections(&parsed, SectionKind::AggregateSynopsis),
                "aggregate_synopsis_section_ids": section_ids(&parsed, SectionKind::AggregateSynopsis),
                "topn_sections": count_sections(&parsed, SectionKind::TopNZoneSummary),
                "topn_section_ids": section_ids(&parsed, SectionKind::TopNZoneSummary),
                "column_domain_sections": count_sections(&parsed, SectionKind::ColumnDomain),
                "column_domain_section_ids": section_ids(&parsed, SectionKind::ColumnDomain),
            }),
        );
    }
    if groups.includes("dictionary") {
        object.insert(
            "dictionary".into(),
            json!({
                "present": mounted.dictionary.is_some(),
                "entries": mounted.dictionary.as_ref().map(|dictionary| dictionary.len()).unwrap_or(0),
                "index_sections": count_sections(&parsed, SectionKind::FileDictionaryIndex),
                "index_section_ids": section_ids(&parsed, SectionKind::FileDictionaryIndex),
                "payload_sections": count_sections(&parsed, SectionKind::FileDictionaryPayload),
                "payload_section_ids": section_ids(&parsed, SectionKind::FileDictionaryPayload),
                "samples": dictionary_samples(mounted.dictionary.as_ref(), 8),
            }),
        );
    }
    if groups.includes("execution") {
        object.insert(
            "execution".into(),
            json!({
                "descriptors": mounted.execution_descriptors.len(),
                "scopes": mounted.execution_scopes.len(),
                "code_spaces": mounted.code_spaces.len(),
                "registries": mounted.engine_profile_registries.len(),
                "mount_policies": mounted.engine_mount_policies.len(),
                "descriptor_summaries": mounted.execution_descriptors.iter().map(|descriptor| json!({
                    "descriptor_id": descriptor.descriptor_id,
                    "code_kind": format!("{:?}", descriptor.code_kind),
                    "code_width_bits": descriptor.code_width_bits,
                    "lifetime": format!("{:?}", descriptor.lifetime),
                    "comparison_scope": format!("{:?}", descriptor.comparison_scope),
                    "scope_ref": descriptor.scope_ref,
                    "code_space_ref": descriptor.code_space_ref,
                })).collect::<Vec<_>>(),
                "scope_summaries": mounted.execution_scopes.iter().map(|scope| json!({
                    "scope_id": scope.scope_id,
                    "scope_kind": format!("{:?}", scope.scope_kind),
                    "display_name": &scope.display_name,
                    "private_payload_ref": scope.private_payload_ref,
                })).collect::<Vec<_>>(),
                "code_space_summaries": mounted.code_spaces.iter().map(|code_space| json!({
                    "code_space_id": code_space.code_space_id,
                    "namespace": &code_space.namespace,
                    "epoch": code_space.epoch,
                    "flags": code_space.flags,
                })).collect::<Vec<_>>(),
                "mount_policy_summaries": mounted.engine_mount_policies.iter().map(|policy| json!({
                    "policy_id": policy.policy_id,
                    "filecode_mapping_kind": format!("{:?}", policy.filecode_mapping_kind),
                    "missing_value_policy": format!("{:?}", policy.missing_value_policy),
                    "stale_mapping_policy": format!("{:?}", policy.stale_mapping_policy),
                    "reverse_lookup_policy": format!("{:?}", policy.reverse_lookup_policy),
                    "code_space_ref": policy.code_space_ref,
                })).collect::<Vec<_>>(),
                "registry_summaries": mounted.engine_profile_registries.iter().map(|registry| json!({
                    "flags": registry.flags,
                    "profiles": registry.profiles.iter().map(|profile| json!({
                        "profile_id": profile.profile_id,
                        "namespace": &profile.namespace,
                        "profile_name": &profile.profile_name,
                        "version_major": profile.version_major,
                        "version_minor": profile.version_minor,
                        "execution_descriptor_ref": profile.execution_descriptor_ref,
                        "mount_policy_ref": profile.mount_policy_ref,
                    })).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
                "coverage_sets": count_sections(&parsed, SectionKind::CoverageSet),
                "coverage_set_section_ids": section_ids(&parsed, SectionKind::CoverageSet),
                "coverage_plans": count_sections(&parsed, SectionKind::CoveragePlanCandidate),
                "coverage_plan_section_ids": section_ids(&parsed, SectionKind::CoveragePlanCandidate),
                "predicate_normal_forms": count_sections(&parsed, SectionKind::PredicateNormalForm),
                "predicate_normal_form_section_ids": section_ids(&parsed, SectionKind::PredicateNormalForm),
            }),
        );
    }
    if groups.includes("indexes") {
        object.insert(
            "indexes".into(),
            json!({
                "mounted_scan_indexes": mounted.scan_indexes.len(),
                "mounted_scan_index_summaries": mounted.scan_indexes.iter().map(|index| json!({
                    "section_id": index.section_id,
                    "kind": format!("{:?}", index.kind),
                    "row_count": index.row_count,
                })).collect::<Vec<_>>(),
                "exact_set_sections": count_sections(&parsed, SectionKind::ExactSetIndex),
                "exact_set_section_ids": section_ids(&parsed, SectionKind::ExactSetIndex),
                "bloom_sections": count_sections(&parsed, SectionKind::BloomIndex),
                "bloom_section_ids": section_ids(&parsed, SectionKind::BloomIndex),
                "inverted_morsel_sections": count_sections(&parsed, SectionKind::InvertedMorselIndex),
                "inverted_morsel_section_ids": section_ids(&parsed, SectionKind::InvertedMorselIndex),
                "lookup_sections": count_sections(&parsed, SectionKind::LookupIndex),
                "lookup_section_ids": section_ids(&parsed, SectionKind::LookupIndex),
                "composite_zone_sections": count_sections(&parsed, SectionKind::CompositeZoneIndex),
                "composite_zone_section_ids": section_ids(&parsed, SectionKind::CompositeZoneIndex),
                "temporal_bloom_sections": count_sections(&parsed, SectionKind::TemporalBloomIndex),
                "temporal_bloom_section_ids": section_ids(&parsed, SectionKind::TemporalBloomIndex),
                "map_identity_equivalence_sections": count_sections(&parsed, SectionKind::MapIdentityEquivalenceIndex),
                "map_identity_equivalence_section_ids": section_ids(&parsed, SectionKind::MapIdentityEquivalenceIndex),
                "map_evidence_sections": count_sections(&parsed, SectionKind::MapEvidenceIndex),
                "map_evidence_section_ids": section_ids(&parsed, SectionKind::MapEvidenceIndex),
            }),
        );
    }
    if groups.includes("optional") {
        object.insert(
            "optional".into(),
            json!({
                "cove_e_sections": count_group(&parsed, 30..=39),
                "cove_h_sections": count_sections(&parsed, SectionKind::HarborMountHints),
                "cove_i_sections": count_group(&parsed, 15..=18) + count_sections(&parsed, SectionKind::CompositeZoneIndex),
                "cove_l_sections": count_sections(&parsed, SectionKind::LakehouseHints),
                "cove_o_sections": count_group(&parsed, 40..=47),
                "cove_map_sections": count_group(&parsed, 60..=68),
                "covx_status": format!("{:?}", mounted.covx_status),
                "covm_status": format!("{:?}", mounted.covm_status),
                "ignored_optional_sections": mounted.ignored_optional_sections.len(),
                "ignored_optional_section_summaries": mounted.ignored_optional_sections.iter().map(|section| json!({
                    "section_id": section.section_id,
                    "section_kind": section.section_kind,
                    "reason": &section.reason,
                })).collect::<Vec<_>>(),
                "section_feature_binding_sections": count_sections(&parsed, SectionKind::SectionFeatureBinding),
                "section_feature_binding_section_ids": section_ids(&parsed, SectionKind::SectionFeatureBinding),
            }),
        );
    }
    Ok(Value::Object(object))
}

fn print_table_summary(
    data: &[u8],
    parsed: &cove_core::reader::ValidatedCoveFile,
) -> Result<(), String> {
    let Some(catalog_entry) = parsed
        .footer
        .sections
        .iter()
        .find(|entry| entry.section_kind == SectionKind::TableCatalog as u16)
    else {
        return Ok(());
    };

    let catalog_payload = compression::section_payload(data, catalog_entry)
        .map_err(|e| format!("table catalog payload: {e}"))?;
    let catalog = TableCatalog::parse(catalog_payload.as_ref())
        .map_err(|e| format!("table catalog parse: {e}"))?;
    println!("  Tables          : {}", catalog.tables.len());
    for table in &catalog.tables {
        println!(
            "    - table={} {}.{} rows={} columns={}",
            table.table_id,
            table.namespace,
            table.name,
            table.row_count,
            table.columns.len()
        );
        for column in &table.columns {
            println!(
                "      column={} name={} logical={:?} physical={:?} nullable={}",
                column.column_id, column.name, column.logical, column.physical, column.nullable
            );
        }
    }

    if let Some(index_entry) = parsed
        .footer
        .sections
        .iter()
        .find(|entry| entry.section_kind == SectionKind::TableSegmentIndex as u16)
    {
        let index_payload = compression::section_payload(data, index_entry)
            .map_err(|e| format!("table segment index payload: {e}"))?;
        let index = TableSegmentIndex::parse(index_payload.as_ref())
            .map_err(|e| format!("table segment index parse: {e}"))?;
        println!("  Segments        : {}", index.entries.len());
        for segment in &index.entries {
            println!(
                "    - table={} segment={} row_start={} rows={} morsels={} columns={}",
                segment.table_id,
                segment.segment_id,
                segment.row_start,
                segment.row_count,
                segment.morsel_count,
                segment.column_count
            );
        }
    }
    Ok(())
}

fn inspect_covemap_file(path: &Path, data: &[u8]) -> Result<(), String> {
    let file = CovemapFile::parse_validated(data).map_err(|e| format!("validation: {e}"))?;

    println!("File: {}", path.display());
    println!("  Artifact        : COVEMAP");
    println!("  Size            : {}", data.len());
    println!(
        "  Version         : {}.{}",
        file.header.version_major, file.header.version_minor
    );
    println!("  Mapping Version : {}", file.mapping_version);
    println!("  Section Count   : {}", file.sections.len());
    for warning in file.compatibility_warnings() {
        println!("  Warning         : {warning}");
    }

    let req_names = feature_names(file.header.required_features);
    println!(
        "  Required Feat   : 0x{:016x}",
        file.header.required_features
    );
    if !req_names.is_empty() {
        println!("    flags: {}", req_names.join(", "));
    }

    let opt_names = feature_names(file.header.optional_features);
    println!(
        "  Optional Feat   : 0x{:016x}",
        file.header.optional_features
    );
    if !opt_names.is_empty() {
        println!("    flags: {}", opt_names.join(", "));
    }

    println!(
        "  Header          : offset={} len={}",
        file.postscript.header_offset, file.postscript.header_length
    );

    for section in &file.sections {
        println!(
            "    - kind={} offset={} len={} raw_len={} comp={} encoding={} required={}",
            section_kind_name(section.entry.section_id),
            section.entry.offset,
            section.entry.uncompressed_length,
            section.entry.length,
            comp_name(section.entry.compression),
            section.entry.payload_encoding,
            section.entry.required,
        );
    }

    Ok(())
}

fn inspect_covemap_json(path: &Path, data: &[u8]) -> Result<Value, String> {
    let file = CovemapFile::parse_validated(data).map_err(|e| format!("validation: {e}"))?;
    Ok(json!({
        "path": path.display().to_string(),
        "artifact": "COVEMAP",
        "size": data.len(),
        "version": {
            "major": file.header.version_major,
            "minor": file.header.version_minor,
        },
        "mapping_version": file.mapping_version,
        "features": {
            "required": file.header.required_features,
            "optional": file.header.optional_features,
            "required_names": feature_names(file.header.required_features),
            "optional_names": feature_names(file.header.optional_features),
        },
        "compatibility_warnings": file.compatibility_warnings(),
        "sections": file.sections.iter().map(|section| {
            json!({
                "section_id": section.entry.section_id,
                "kind": section_kind_name(section.entry.section_id),
                "offset": section.entry.offset,
                "len": section.entry.length,
                "raw_len": section.entry.uncompressed_length,
                "compression": comp_name(section.entry.compression),
                "payload_encoding": section.entry.payload_encoding,
                "required": section.entry.required,
            })
        }).collect::<Vec<_>>(),
    }))
}

fn count_sections(parsed: &cove_core::reader::ValidatedCoveFile, kind: SectionKind) -> usize {
    parsed
        .footer
        .sections
        .iter()
        .filter(|section| section.section_kind == kind as u16)
        .count()
}

fn section_ids(parsed: &cove_core::reader::ValidatedCoveFile, kind: SectionKind) -> Vec<u32> {
    parsed
        .footer
        .sections
        .iter()
        .filter(|section| section.section_kind == kind as u16)
        .map(|section| section.section_id)
        .collect()
}

fn zone_stats_summary(sections: &[cove_core::zone_stats::ZoneStatsSection]) -> Vec<Value> {
    sections
        .iter()
        .enumerate()
        .map(|(section_index, section)| {
            json!({
                "section_index": section_index,
                "entry_count": section.entries.len(),
                "sample_entries": section.entries.iter().take(8).map(|entry| json!({
                    "table_id": entry.table_id,
                    "segment_id": entry.segment_id,
                    "morsel_id": entry.morsel_id,
                    "column_id": entry.column_id,
                    "scope": format!("{:?}", entry.stats.scope),
                    "row_count": entry.stats.row_count,
                    "null_count": entry.stats.null_count,
                    "non_null_count": entry.non_null_count,
                    "distinct_count": entry.distinct_count,
                    "flags": entry.stats.flags.bits(),
                    "min_domain_rank": entry.min_domain_rank,
                    "max_domain_rank": entry.max_domain_rank,
                    "exact_set_ref": entry.exact_set_ref,
                    "bloom_ref": entry.bloom_ref,
                })).collect::<Vec<_>>(),
            })
        })
        .collect()
}

fn dictionary_samples(
    dictionary: Option<&cove_core::dictionary::FileDictionary>,
    limit: u32,
) -> Vec<Value> {
    let Some(dictionary) = dictionary else {
        return Vec::new();
    };
    (0..dictionary.len().min(limit))
        .filter_map(|filecode| {
            let entry = dictionary.get_entry(filecode).ok()?;
            let storage = StorageClass::from_u8(entry.storage_class)
                .map(|storage| format!("{storage:?}"))
                .unwrap_or_else(|| format!("Unknown({})", entry.storage_class));
            let tag = ValueTag::from_u16(entry.value_tag)
                .map(|tag| format!("{tag:?}"))
                .unwrap_or_else(|| format!("Unknown({})", entry.value_tag));
            let value = match dictionary.decode_value(filecode) {
                Ok(cove_core::dictionary::DictionaryValue::RawBytes(bytes)) => json!({
                    "kind": "raw_bytes",
                    "len": bytes.len(),
                    "utf8_preview": String::from_utf8(bytes.clone()).ok().map(|value| value.chars().take(64).collect::<String>()),
                }),
                Ok(cove_core::dictionary::DictionaryValue::RedactedPresent) => json!({
                    "kind": "redacted",
                }),
                Ok(_) => json!({
                    "kind": "future",
                }),
                Err(err) => json!({
                    "kind": "decode_error",
                    "error": err.to_string(),
                }),
            };
            Some(json!({
                "filecode": filecode,
                "value_tag": tag,
                "storage_class": storage,
                "flags": entry.flags,
                "inline_len": entry.inline_len,
                "payload_length": entry.payload_length,
                "canonical_hash64": entry.canonical_hash64,
                "value": value,
            }))
        })
        .collect()
}

fn count_group(
    parsed: &cove_core::reader::ValidatedCoveFile,
    range: std::ops::RangeInclusive<u16>,
) -> usize {
    parsed
        .footer
        .sections
        .iter()
        .filter(|section| range.contains(&section.section_kind))
        .count()
}
