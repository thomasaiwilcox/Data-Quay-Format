use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
};

use crate::{
    codec::CodecExtensionDescriptorV2,
    collation::CollationRegistry,
    compression,
    constants::{
        SectionKind, StorageClass, FEATURE_ENGINE_PROFILE, FEATURE_EXTENSION_REGISTRY,
        FEATURE_FILE_DICTIONARY, FEATURE_HARBOR_PROFILE, FEATURE_OBJECT_PROFILE,
        FEATURE_SEMANTIC_MAP,
    },
    dictionary::FileDictionaryView,
    digest::DigestManifest,
    domain::ColumnDomain,
    extensions::{ExtensionRegistry, ExtensionValidationContext},
    footer::CoveSectionEntryV1,
    header::CoveHeaderV1,
    index::{
        aggregate::AggregateSynopsis, bloom::BloomFilterIndex, composite::CompositeIndex,
        exact_set::ExactSetIndex, inverted::InvertedMorselIndex, lookup::LookupIndex,
        topn::TopNSummary,
    },
    interop::lakehouse::LakehouseHints,
    kernel::KernelCapabilities,
    nested_schema::NestedSchemaSectionV1,
    page::ColumnPageIndex,
    page_validation::{
        validate_column_page_payload, validate_column_page_wire, validate_stats_only_constant_page,
        PageValidationContext,
    },
    profile::{
        cove_e::{
            CodeSpaceDescriptorV1, EngineMountPolicyV1, EngineProfileRegistry,
            ExecutionCodeDescriptorV1, ExecutionScopeDescriptorV1,
        },
        cove_h::HarborMountHintsV1,
        cove_map::{parse_embedded_section, validate_embedded_sections, EmbeddedMapSection},
        cove_o::{
            validate_self_contained, validate_temporal_property_page_elision_features,
            validate_temporal_property_stats_only_page, ObjectTypeCatalog, PropertyEntryV1,
            RecordKind, TemporalBloomIndex, TemporalPropertyColumn, TemporalSegmentData,
            TemporalSegmentIndex, TemporalSegmentIndexEntryV1, TrustManifest,
            PROPERTY_FLAG_BOOL_DECLARED_NUMERIC,
        },
    },
    redaction::RedactionManifest,
    segment::{
        TableColumnDirectoryEntryV1, TableSegmentIndex, TableSegmentPayloadV1,
        SEGMENT_COLUMN_FLAG_BOOL_DECLARED_NUMERIC,
    },
    table::{ColumnEntry, TableCatalog, TableEntry, COLUMN_FLAG_BOOL_DECLARED_NUMERIC},
    zone_stats::{ZoneStatsEntry, ZoneStatsSection},
    CoveError,
};

use super::{
    reports::{
        push_stage, IgnoredOptionalSection, OptionalPushdownPolicy, ValidationOptions,
        ValidationStage, ValidationStageReport, ValidationStageStatus,
    },
    shared_semantics::parse_validation_dictionary,
    ValidatedCoveFile,
};

pub(super) fn validate_shared_semantics(
    data: &[u8],
    validated: &ValidatedCoveFile,
    opts: &ValidationOptions,
    dict_entry_count: &mut Option<u32>,
    stages: &mut Vec<ValidationStageReport>,
    ignored_optional_sections: &mut Vec<IgnoredOptionalSection>,
) -> Result<(), CoveError> {
    let footer = &validated.footer;
    let mut checked = 0u32;
    let mut parsed_dict: Option<FileDictionaryView<'_>> = None;
    let mut dict_section_id: Option<u32> = None;
    let mut redaction_manifest_refs = BTreeSet::new();

    let has_dict_feature = validated.header.required_features & FEATURE_FILE_DICTIONARY != 0;
    if has_dict_feature {
        let index_entry = footer
            .sections
            .iter()
            .find(|s| s.section_kind == SectionKind::FileDictionaryIndex as u16);
        let payload_entry = footer
            .sections
            .iter()
            .find(|s| s.section_kind == SectionKind::FileDictionaryPayload as u16);

        match index_entry {
            None => {
                return Err(CoveError::BadSection(
                    "FEATURE_FILE_DICTIONARY set but FILE_DICTIONARY_INDEX section missing".into(),
                ));
            }
            Some(idx_entry) => {
                let index_bytes = compression::section_payload(data, idx_entry)?;
                let payload_bytes = match payload_entry {
                    Some(pay_entry) => compression::section_payload(data, pay_entry)?,
                    None => std::borrow::Cow::Borrowed(&[][..]),
                };
                let dict = FileDictionaryView::parse(index_bytes, payload_bytes)?;
                dict.validate_all()?;
                *dict_entry_count = Some(dict.len());
                dict_section_id = Some(idx_entry.section_id);
                parsed_dict = Some(dict);
                checked += 1 + u32::from(payload_entry.is_some());
            }
        }
    }

    let ext_registry_is_required =
        validated.header.required_features & FEATURE_EXTENSION_REGISTRY != 0;
    let ext_registry_is_optional =
        validated.header.optional_features & FEATURE_EXTENSION_REGISTRY != 0;
    if ext_registry_is_required || ext_registry_is_optional {
        let collation_count = footer
            .sections
            .iter()
            .find(|s| s.section_kind == SectionKind::CollationRegistry as u16)
            .map(|entry| {
                let bytes = compression::section_payload(data, entry)?;
                CollationRegistry::parse(&bytes).map(|registry| registry.entries.len())
            })
            .transpose()?;
        let ext_entry = footer
            .sections
            .iter()
            .find(|s| s.section_kind == SectionKind::ExtensionRegistry as u16);
        match (ext_registry_is_required, ext_entry) {
            (true, None) => {
                return Err(CoveError::BadSection(
                    "FEATURE_EXTENSION_REGISTRY set in required_features but \
                     EXTENSION_REGISTRY section missing"
                        .into(),
                ));
            }
            (_, Some(entry)) => {
                let ext_bytes = compression::section_payload(data, entry)?;
                let registry = ExtensionRegistry::parse(&ext_bytes)?;
                registry.validate_in_file(
                    data,
                    footer,
                    opts.allow_unknown_optional_extensions,
                    ExtensionValidationContext { collation_count },
                )?;
                checked += 1;
            }
            (false, None) => {}
        }
    }

    for entry in &footer.sections {
        let kind = SectionKind::from_u16(entry.section_kind).ok_or_else(|| {
            CoveError::BadSection(format!("unknown section_kind {}", entry.section_kind))
        })?;
        match kind {
            SectionKind::CollationRegistry => {
                let payload = compression::section_payload(data, entry)?;
                CollationRegistry::parse(&payload)?;
                checked += 1;
            }
            SectionKind::DigestManifest => {
                let payload = compression::section_payload(data, entry)?;
                DigestManifest::parse(&payload)?;
                checked += 1;
            }
            SectionKind::RedactionManifest => {
                let payload = compression::section_payload(data, entry)?;
                let manifest = RedactionManifest::parse(&payload)?;
                redaction_manifest_refs.extend(
                    manifest
                        .entries
                        .iter()
                        .map(|entry| (entry.section_id, entry.local_ref)),
                );
                checked += 1;
            }
            SectionKind::LakehouseHints => {
                let payload = compression::section_payload(data, entry)?;
                LakehouseHints::parse(&payload)?;
                checked += 1;
            }
            SectionKind::KernelCapabilities => {
                let payload = compression::section_payload(data, entry)?;
                KernelCapabilities::parse(&payload)?;
                checked += 1;
            }
            SectionKind::FileDictionaryIndex
            | SectionKind::FileDictionaryPayload
            | SectionKind::ArrowInteropHints
            | SectionKind::ExtensionRegistry
            | SectionKind::ProfileCapabilityMatrix
            | SectionKind::ExtendedFeatureSet
            | SectionKind::CodecExtensionRegistry
            | SectionKind::LayoutPlan
            | SectionKind::ScanSplitIndex
            | SectionKind::PageClusterDirectory
            | SectionKind::ZeroCopyBufferMap
            | SectionKind::FastMetadataIndex
            | SectionKind::CoverageProviderRegistry
            | SectionKind::CoverageSet
            | SectionKind::CoveragePlanCandidate
            | SectionKind::PredicateNormalForm
            | SectionKind::IndexOnlyCapability
            | SectionKind::SectionFeatureBinding
            | SectionKind::CoverageProofRecord
            | SectionKind::VendorExtension
            | SectionKind::TableCatalog
            | SectionKind::NestedSchema
            | SectionKind::TableSegmentIndex
            | SectionKind::TableSegmentData
            | SectionKind::ColumnDomain
            | SectionKind::ZoneStats
            | SectionKind::ExactSetIndex
            | SectionKind::BloomIndex
            | SectionKind::InvertedMorselIndex
            | SectionKind::LookupIndex
            | SectionKind::AggregateSynopsis
            | SectionKind::CompositeZoneIndex
            | SectionKind::TopNZoneSummary
            | SectionKind::EngineProfileRegistry
            | SectionKind::ExecutionCodeDescriptor
            | SectionKind::ExecutionScopeDescriptor
            | SectionKind::CodeSpaceDescriptor
            | SectionKind::EngineMountPolicy
            | SectionKind::ObjectTypeCatalog
            | SectionKind::TemporalSegmentIndex
            | SectionKind::TemporalSegmentData
            | SectionKind::TemporalBloomIndex
            | SectionKind::TrustManifest
            | SectionKind::HarborMountHints
            | SectionKind::MapSourceCatalog
            | SectionKind::MapFunctionRegistry
            | SectionKind::MapIdentityRuleCatalog
            | SectionKind::MapRowSemanticsCatalog
            | SectionKind::MapAssertionLog
            | SectionKind::MapIdentityEquivalenceIndex
            | SectionKind::MapEvidenceIndex
            | SectionKind::MapConversionReport
            | SectionKind::MapProjectionCatalog => {
                if is_optional_pushdown_section(kind)
                    && opts.optional_pushdown_policy == OptionalPushdownPolicy::FailOpen
                {
                    let _ = optional_section_payload(
                        data,
                        entry,
                        opts.optional_pushdown_policy,
                        ignored_optional_sections,
                    )?;
                }
            }
        }
    }

    validate_redaction_manifest_links(
        parsed_dict.as_ref(),
        dict_section_id,
        &redaction_manifest_refs,
    )?;

    push_stage(
        stages,
        ValidationStage::SharedSemantic,
        ValidationStageStatus::Checked,
        checked,
    );
    Ok(())
}

fn validate_redaction_manifest_links(
    dict: Option<&FileDictionaryView<'_>>,
    dict_section_id: Option<u32>,
    manifest_refs: &BTreeSet<(u32, u64)>,
) -> Result<(), CoveError> {
    let (Some(dict), Some(dict_section_id)) = (dict, dict_section_id) else {
        return Ok(());
    };

    let mut redacted_codes = BTreeSet::new();
    for file_code in 0..dict.len() {
        let entry = dict.get_entry(file_code)?;
        if matches!(
            StorageClass::from_u8(entry.storage_class),
            Some(StorageClass::Redacted)
        ) {
            redacted_codes.insert(u64::from(file_code));
        }
    }

    for file_code in &redacted_codes {
        if !manifest_refs.contains(&(dict_section_id, *file_code)) {
            return Err(CoveError::BadSchema(format!(
                "redacted FileCode {file_code} is missing a redaction manifest entry"
            )));
        }
    }

    for (_, file_code) in manifest_refs
        .iter()
        .filter(|(section_id, _)| *section_id == dict_section_id)
    {
        let file_code = u32::try_from(*file_code).map_err(|_| CoveError::ArithOverflow)?;
        let entry = dict.get_entry(file_code).map_err(|error| match error {
            CoveError::BadFileCode => CoveError::BadSchema(format!(
                "redaction manifest references out-of-range FileCode {file_code}"
            )),
            other => other,
        })?;
        if !matches!(
            StorageClass::from_u8(entry.storage_class),
            Some(StorageClass::Redacted)
        ) {
            return Err(CoveError::BadSchema(format!(
                "redaction manifest references non-redacted FileCode {file_code}"
            )));
        }
    }

    Ok(())
}

pub(super) fn validate_cove_t_semantics(
    data: &[u8],
    validated: &ValidatedCoveFile,
    opts: &ValidationOptions,
    stages: &mut Vec<ValidationStageReport>,
    ignored_optional_sections: &mut Vec<IgnoredOptionalSection>,
) -> Result<(), CoveError> {
    let mut checked = 0u32;
    let mut catalogs = Vec::new();
    let mut nested_schemas = Vec::new();
    let mut segment_indexes = Vec::new();
    let mut segment_payloads = Vec::new();
    let mut zone_stats_entries = Vec::new();
    let mut codec_descriptors = Vec::new();
    let dictionary = parse_validation_dictionary(data, &validated.footer)?;

    for entry in &validated.footer.sections {
        let kind = SectionKind::from_u16(entry.section_kind).ok_or_else(|| {
            CoveError::BadSection(format!("unknown section_kind {}", entry.section_kind))
        })?;
        match kind {
            SectionKind::TableCatalog => {
                let payload = compression::section_payload(data, entry)?;
                catalogs.push((entry.section_id, TableCatalog::parse(&payload)?));
                checked += 1;
            }
            SectionKind::NestedSchema => {
                let payload = compression::section_payload(data, entry)?;
                nested_schemas.push((entry.section_id, NestedSchemaSectionV1::parse(&payload)?));
                checked += 1;
            }
            SectionKind::TableSegmentIndex => {
                let payload = compression::section_payload(data, entry)?;
                segment_indexes.push((entry.section_id, TableSegmentIndex::parse(&payload)?));
                checked += 1;
            }
            SectionKind::TableSegmentData => {
                let payload = compression::section_payload(data, entry)?;
                segment_payloads.push((
                    entry.section_id,
                    entry.offset,
                    TableSegmentPayloadV1::parse_with_required_features(
                        &payload,
                        validated.header.required_features,
                    )?,
                    payload.into_owned(),
                ));
                checked += 1;
            }
            SectionKind::ColumnDomain => {
                if let Some(payload) = optional_section_payload(
                    data,
                    entry,
                    opts.optional_pushdown_policy,
                    ignored_optional_sections,
                )? {
                    match ColumnDomain::parse(&payload) {
                        Ok(_) => checked += 1,
                        Err(error) => {
                            optional_section_parse_error(
                                entry,
                                opts.optional_pushdown_policy,
                                ignored_optional_sections,
                                error,
                            )?;
                        }
                    }
                }
            }
            SectionKind::ExactSetIndex => {
                if let Some(payload) = optional_section_payload(
                    data,
                    entry,
                    opts.optional_pushdown_policy,
                    ignored_optional_sections,
                )? {
                    match ExactSetIndex::parse(&payload) {
                        Ok(_) => checked += 1,
                        Err(error) => {
                            optional_section_parse_error(
                                entry,
                                opts.optional_pushdown_policy,
                                ignored_optional_sections,
                                error,
                            )?;
                        }
                    }
                }
            }
            SectionKind::BloomIndex => {
                if let Some(payload) = optional_section_payload(
                    data,
                    entry,
                    opts.optional_pushdown_policy,
                    ignored_optional_sections,
                )? {
                    match BloomFilterIndex::parse(&payload) {
                        Ok(_) => checked += 1,
                        Err(error) => {
                            optional_section_parse_error(
                                entry,
                                opts.optional_pushdown_policy,
                                ignored_optional_sections,
                                error,
                            )?;
                        }
                    }
                }
            }
            SectionKind::InvertedMorselIndex => {
                if let Some(payload) = optional_section_payload(
                    data,
                    entry,
                    opts.optional_pushdown_policy,
                    ignored_optional_sections,
                )? {
                    match InvertedMorselIndex::parse(&payload) {
                        Ok(_) => checked += 1,
                        Err(error) => {
                            optional_section_parse_error(
                                entry,
                                opts.optional_pushdown_policy,
                                ignored_optional_sections,
                                error,
                            )?;
                        }
                    }
                }
            }
            SectionKind::LookupIndex => {
                if let Some(payload) = optional_section_payload(
                    data,
                    entry,
                    opts.optional_pushdown_policy,
                    ignored_optional_sections,
                )? {
                    match LookupIndex::parse(&payload) {
                        Ok(_) => checked += 1,
                        Err(error) => {
                            optional_section_parse_error(
                                entry,
                                opts.optional_pushdown_policy,
                                ignored_optional_sections,
                                error,
                            )?;
                        }
                    }
                }
            }
            SectionKind::AggregateSynopsis => {
                if let Some(payload) = optional_section_payload(
                    data,
                    entry,
                    opts.optional_pushdown_policy,
                    ignored_optional_sections,
                )? {
                    match AggregateSynopsis::parse(&payload) {
                        Ok(_) => checked += 1,
                        Err(error) => {
                            optional_section_parse_error(
                                entry,
                                opts.optional_pushdown_policy,
                                ignored_optional_sections,
                                error,
                            )?;
                        }
                    }
                }
            }
            SectionKind::CompositeZoneIndex => {
                if let Some(payload) = optional_section_payload(
                    data,
                    entry,
                    opts.optional_pushdown_policy,
                    ignored_optional_sections,
                )? {
                    match CompositeIndex::parse(&payload) {
                        Ok(_) => checked += 1,
                        Err(error) => {
                            optional_section_parse_error(
                                entry,
                                opts.optional_pushdown_policy,
                                ignored_optional_sections,
                                error,
                            )?;
                        }
                    }
                }
            }
            SectionKind::TopNZoneSummary => {
                if let Some(payload) = optional_section_payload(
                    data,
                    entry,
                    opts.optional_pushdown_policy,
                    ignored_optional_sections,
                )? {
                    match TopNSummary::parse(&payload) {
                        Ok(_) => checked += 1,
                        Err(error) => {
                            optional_section_parse_error(
                                entry,
                                opts.optional_pushdown_policy,
                                ignored_optional_sections,
                                error,
                            )?;
                        }
                    }
                }
            }
            SectionKind::ZoneStats => {
                let payload = compression::section_payload(data, entry)?;
                zone_stats_entries.extend(ZoneStatsSection::parse(&payload)?.entries);
                checked += 1;
            }
            SectionKind::CodecExtensionRegistry => {
                let payload = compression::section_payload(data, entry)?;
                codec_descriptors.extend(CodecExtensionDescriptorV2::parse_many(&payload)?);
                checked += 1;
            }
            SectionKind::FileDictionaryIndex
            | SectionKind::FileDictionaryPayload
            | SectionKind::CollationRegistry
            | SectionKind::DigestManifest
            | SectionKind::RedactionManifest
            | SectionKind::ArrowInteropHints
            | SectionKind::LakehouseHints
            | SectionKind::ExtensionRegistry
            | SectionKind::ProfileCapabilityMatrix
            | SectionKind::ExtendedFeatureSet
            | SectionKind::LayoutPlan
            | SectionKind::ScanSplitIndex
            | SectionKind::PageClusterDirectory
            | SectionKind::ZeroCopyBufferMap
            | SectionKind::FastMetadataIndex
            | SectionKind::CoverageProviderRegistry
            | SectionKind::CoverageSet
            | SectionKind::CoveragePlanCandidate
            | SectionKind::PredicateNormalForm
            | SectionKind::IndexOnlyCapability
            | SectionKind::SectionFeatureBinding
            | SectionKind::CoverageProofRecord
            | SectionKind::KernelCapabilities
            | SectionKind::EngineProfileRegistry
            | SectionKind::ExecutionCodeDescriptor
            | SectionKind::ExecutionScopeDescriptor
            | SectionKind::CodeSpaceDescriptor
            | SectionKind::EngineMountPolicy
            | SectionKind::ObjectTypeCatalog
            | SectionKind::TemporalSegmentIndex
            | SectionKind::TemporalSegmentData
            | SectionKind::TemporalBloomIndex
            | SectionKind::TrustManifest
            | SectionKind::HarborMountHints
            | SectionKind::MapSourceCatalog
            | SectionKind::MapFunctionRegistry
            | SectionKind::MapIdentityRuleCatalog
            | SectionKind::MapRowSemanticsCatalog
            | SectionKind::MapAssertionLog
            | SectionKind::MapIdentityEquivalenceIndex
            | SectionKind::MapEvidenceIndex
            | SectionKind::MapConversionReport
            | SectionKind::MapProjectionCatalog
            | SectionKind::VendorExtension => {}
        }
    }
    validate_cove_t_cross_sections(
        &catalogs,
        &nested_schemas,
        &segment_indexes,
        &segment_payloads,
        dictionary.as_ref(),
        &zone_stats_entries,
        &codec_descriptors,
    )?;
    push_stage(
        stages,
        ValidationStage::CoveTable,
        ValidationStageStatus::Checked,
        checked,
    );
    Ok(())
}

fn optional_section_payload<'a>(
    data: &'a [u8],
    entry: &CoveSectionEntryV1,
    policy: OptionalPushdownPolicy,
    ignored_optional_sections: &mut Vec<IgnoredOptionalSection>,
) -> Result<Option<Cow<'a, [u8]>>, CoveError> {
    match compression::section_payload(data, entry) {
        Ok(payload) => Ok(Some(payload)),
        Err(error) => {
            optional_section_parse_error(entry, policy, ignored_optional_sections, error)?;
            Ok(None)
        }
    }
}

fn optional_section_parse_error(
    entry: &CoveSectionEntryV1,
    policy: OptionalPushdownPolicy,
    ignored_optional_sections: &mut Vec<IgnoredOptionalSection>,
    error: CoveError,
) -> Result<(), CoveError> {
    if policy == OptionalPushdownPolicy::FailOpen && is_optional_pushdown_entry(entry) {
        if ignored_optional_sections
            .iter()
            .any(|ignored| ignored.section_id == entry.section_id)
        {
            return Ok(());
        }
        ignored_optional_sections.push(IgnoredOptionalSection {
            section_id: entry.section_id,
            section_kind: entry.section_kind,
            reason: error.to_string(),
        });
        return Ok(());
    }
    Err(error)
}

pub(super) fn is_optional_pushdown_entry(entry: &CoveSectionEntryV1) -> bool {
    entry.required_features == 0
        && SectionKind::from_u16(entry.section_kind)
            .map(is_optional_pushdown_section)
            .unwrap_or(false)
}

fn is_optional_pushdown_section(kind: SectionKind) -> bool {
    matches!(
        kind,
        SectionKind::ColumnDomain
            | SectionKind::ExactSetIndex
            | SectionKind::BloomIndex
            | SectionKind::InvertedMorselIndex
            | SectionKind::LookupIndex
            | SectionKind::AggregateSynopsis
            | SectionKind::CompositeZoneIndex
            | SectionKind::TopNZoneSummary
    )
}

fn validate_cove_t_cross_sections(
    catalogs: &[(u32, TableCatalog)],
    nested_schemas: &[(u32, NestedSchemaSectionV1)],
    segment_indexes: &[(u32, TableSegmentIndex)],
    segment_payloads: &[(u32, u64, TableSegmentPayloadV1, Vec<u8>)],
    dictionary: Option<&FileDictionaryView<'_>>,
    zone_stats: &[ZoneStatsEntry],
    codec_descriptors: &[CodecExtensionDescriptorV2],
) -> Result<(), CoveError> {
    if catalogs.is_empty() && segment_indexes.is_empty() && segment_payloads.is_empty() {
        return Ok(());
    }
    if catalogs.len() != 1 {
        return Err(CoveError::BadSchema(
            "COVE-T validation requires exactly one TableCatalog section".into(),
        ));
    }
    let catalog = &catalogs[0].1;
    if nested_schemas.len() > 1 {
        return Err(CoveError::BadSchema(
            "COVE-T validation supports at most one NestedSchema section".into(),
        ));
    }
    let nested_schema = nested_schemas
        .first()
        .map(|(_section_id, nested_schema)| nested_schema);
    if let Some(nested_schema) = nested_schema {
        nested_schema.validate_for_catalog(catalog)?;
    } else if catalog.tables.iter().any(|table| {
        table
            .columns
            .iter()
            .any(crate::nested_schema::column_uses_nested_schema)
    }) {
        return Err(CoveError::BadSchema(
            "native nested COVE-T columns require a NestedSchema section".into(),
        ));
    }
    if segment_indexes.is_empty() && segment_payloads.is_empty() {
        if catalogs[0]
            .1
            .tables
            .iter()
            .all(|table| table.row_count == 0)
        {
            return Ok(());
        }
        return Err(CoveError::SegmentCorrupt);
    }
    if segment_indexes.len() != 1 {
        return Err(CoveError::SegmentCorrupt);
    }
    let segment_index = &segment_indexes[0].1;
    let tables = catalog
        .tables
        .iter()
        .map(|table| (table.table_id, table))
        .collect::<BTreeMap<_, _>>();
    let mut payloads_by_key = BTreeMap::new();
    for (_section_id, file_offset, payload, bytes) in segment_payloads {
        if payloads_by_key
            .insert(
                (payload.header.table_id, payload.header.segment_id),
                (*file_offset, payload, bytes),
            )
            .is_some()
        {
            return Err(CoveError::SegmentCorrupt);
        }
    }
    let mut rows_by_table = BTreeMap::<u32, u64>::new();
    for entry in &segment_index.entries {
        let table = tables.get(&entry.table_id).ok_or_else(|| {
            CoveError::BadSchema(format!(
                "segment index references unknown table_id {}",
                entry.table_id
            ))
        })?;
        if entry.column_count != table.columns.len() as u32 {
            return Err(CoveError::SegmentCorrupt);
        }
        let Some((file_offset, payload, bytes)) =
            payloads_by_key.get(&(entry.table_id, entry.segment_id))
        else {
            return Err(CoveError::SegmentCorrupt);
        };
        if *file_offset != entry.offset
            || payload.header.row_start != entry.row_start
            || payload.header.row_count != entry.row_count
            || payload.header.morsel_count != entry.morsel_count
            || payload.header.morsel_row_count != entry.morsel_row_count
            || payload.header.column_count != entry.column_count
        {
            return Err(CoveError::SegmentCorrupt);
        }
        if entry.length != bytes.len() as u64 {
            return Err(CoveError::SegmentCorrupt);
        }
        *rows_by_table.entry(entry.table_id).or_default() += u64::from(entry.row_count);
        validate_segment_against_catalog(
            table,
            payload,
            bytes,
            dictionary,
            zone_stats,
            codec_descriptors,
            nested_schema,
        )?;
    }
    for table in &catalog.tables {
        if rows_by_table.get(&table.table_id).copied().unwrap_or(0) != table.row_count {
            return Err(CoveError::SegmentCorrupt);
        }
    }
    if payloads_by_key.len() != segment_index.entries.len() {
        return Err(CoveError::SegmentCorrupt);
    }
    Ok(())
}

fn validate_segment_against_catalog(
    table: &TableEntry,
    segment: &TableSegmentPayloadV1,
    segment_bytes: &[u8],
    dictionary: Option<&FileDictionaryView<'_>>,
    zone_stats: &[ZoneStatsEntry],
    codec_descriptors: &[CodecExtensionDescriptorV2],
    nested_schema: Option<&NestedSchemaSectionV1>,
) -> Result<(), CoveError> {
    if segment.header.table_id != table.table_id {
        return Err(CoveError::SegmentCorrupt);
    }
    let columns = table
        .columns
        .iter()
        .map(|column| (column.column_id, column))
        .collect::<BTreeMap<_, _>>();
    if segment.columns.len() != table.columns.len() {
        return Err(CoveError::SegmentCorrupt);
    }
    for column_dir in &segment.columns {
        let column = columns.get(&column_dir.column_id).ok_or_else(|| {
            CoveError::BadSchema(format!(
                "segment references unknown column_id {}",
                column_dir.column_id
            ))
        })?;
        if column_dir.logical_type != column.logical || column_dir.physical_kind != column.physical
        {
            return Err(CoveError::PageCorrupt);
        }
        if (column.flags & COLUMN_FLAG_BOOL_DECLARED_NUMERIC != 0)
            != (column_dir.flags & SEGMENT_COLUMN_FLAG_BOOL_DECLARED_NUMERIC != 0)
        {
            return Err(CoveError::PageCorrupt);
        }
        validate_column_pages_against_catalog(
            column,
            column_dir,
            segment,
            segment_bytes,
            dictionary,
            zone_stats,
            codec_descriptors,
            nested_schema,
        )?;
    }
    Ok(())
}

fn validate_column_pages_against_catalog(
    column: &ColumnEntry,
    column_dir: &TableColumnDirectoryEntryV1,
    segment: &TableSegmentPayloadV1,
    segment_bytes: &[u8],
    dictionary: Option<&FileDictionaryView<'_>>,
    zone_stats: &[ZoneStatsEntry],
    codec_descriptors: &[CodecExtensionDescriptorV2],
    nested_schema: Option<&NestedSchemaSectionV1>,
) -> Result<(), CoveError> {
    let page_index_start =
        usize::try_from(column_dir.page_index_offset).map_err(|_| CoveError::OffsetRange)?;
    let page_index_end = usize::try_from(
        column_dir
            .page_index_offset
            .checked_add(column_dir.page_index_length)
            .ok_or(CoveError::ArithOverflow)?,
    )
    .map_err(|_| CoveError::OffsetRange)?;
    let page_index = ColumnPageIndex::parse(&segment_bytes[page_index_start..page_index_end])?;
    if page_index.entries.len() != segment.morsels.entries.len() {
        return Err(CoveError::PageCorrupt);
    }
    for page in &page_index.entries {
        if !column.nullable && page.null_count != 0 {
            return Err(CoveError::BadSchema(format!(
                "non-nullable column {} has page null_count {}",
                column.column_id, page.null_count
            )));
        }
        let context = PageValidationContext {
            table_id: Some(segment.header.table_id),
            segment_id: Some(segment.header.segment_id),
            column_id: column.column_id,
            logical_type: column.logical,
            physical_kind: column.physical,
            dictionary,
            zone_stats: Some(zone_stats),
            codec_descriptors,
            nested_schema: nested_schema
                .and_then(|schema| schema.entry(segment.header.table_id, column.column_id))
                .map(|entry| &entry.root),
        };
        if page.page_length == 0 {
            validate_stats_only_constant_page(&context, page)?;
            continue;
        }
        let start = usize::try_from(page.page_offset).map_err(|_| CoveError::OffsetRange)?;
        let end = usize::try_from(
            page.page_offset
                .checked_add(page.page_length)
                .ok_or(CoveError::ArithOverflow)?,
        )
        .map_err(|_| CoveError::OffsetRange)?;
        validate_column_page_wire(&context, page, &segment_bytes[start..end])?;
    }
    Ok(())
}

pub(super) fn validate_cove_o_semantics(
    data: &[u8],
    validated: &ValidatedCoveFile,
    stages: &mut Vec<ValidationStageReport>,
) -> Result<(), CoveError> {
    let mut checked = 0u32;
    let mut object_catalogs = Vec::new();
    let mut temporal_indexes = Vec::new();
    let mut temporal_segments = Vec::new();
    let mut trust_manifests = Vec::new();
    let dictionary = parse_validation_dictionary(data, &validated.footer)?;
    for entry in &validated.footer.sections {
        let kind = SectionKind::from_u16(entry.section_kind).ok_or_else(|| {
            CoveError::BadSection(format!("unknown section_kind {}", entry.section_kind))
        })?;
        let result = match kind {
            SectionKind::ObjectTypeCatalog => {
                let payload = compression::section_payload(data, entry)?;
                ObjectTypeCatalog::parse(&payload).map(|catalog| {
                    object_catalogs.push(catalog);
                })
            }
            SectionKind::TemporalSegmentIndex => {
                let payload = compression::section_payload(data, entry)?;
                TemporalSegmentIndex::parse(&payload).map(|index| {
                    temporal_indexes.push(index);
                })
            }
            SectionKind::TemporalSegmentData => {
                let payload = compression::section_payload(data, entry)?;
                TemporalSegmentData::parse_with_required_features(
                    &payload,
                    validated.header.required_features,
                )
                .map(|segment| {
                    temporal_segments.push((entry.offset, payload.into_owned(), segment));
                })
            }
            SectionKind::TemporalBloomIndex => {
                let payload = compression::section_payload(data, entry)?;
                TemporalBloomIndex::parse(&payload).map(|_| ())
            }
            SectionKind::TrustManifest => {
                let payload = compression::section_payload(data, entry)?;
                TrustManifest::parse(&payload).map(|manifest| {
                    trust_manifests.push(manifest);
                })
            }
            _ => continue,
        };
        checked += 1;
        if let Err(err) = result {
            if profile_error_is_fatal(&validated.header, entry, FEATURE_OBJECT_PROFILE) {
                return Err(err);
            }
        }
    }
    validate_cove_o_cross_sections(
        &object_catalogs,
        &temporal_indexes,
        &temporal_segments,
        &trust_manifests,
        dictionary.as_ref(),
        validated.header.required_features,
    )?;
    push_stage(
        stages,
        ValidationStage::CoveObject,
        ValidationStageStatus::Checked,
        checked,
    );
    Ok(())
}

fn validate_cove_o_cross_sections(
    catalogs: &[ObjectTypeCatalog],
    indexes: &[TemporalSegmentIndex],
    segments: &[(u64, Vec<u8>, TemporalSegmentData)],
    trust_manifests: &[TrustManifest],
    dictionary: Option<&FileDictionaryView<'_>>,
    required_features: u64,
) -> Result<(), CoveError> {
    if catalogs.is_empty()
        && indexes.is_empty()
        && segments.is_empty()
        && trust_manifests.is_empty()
    {
        return Ok(());
    }
    if catalogs.len() != 1 {
        return Err(CoveError::BadSchema(
            "COVE-O validation requires exactly one ObjectTypeCatalog section".into(),
        ));
    }
    if !segments.is_empty() && indexes.len() != 1 {
        return Err(CoveError::SegmentCorrupt);
    }
    if segments.is_empty() {
        if indexes.iter().all(|index| index.entries.is_empty()) {
            return Ok(());
        }
        return Err(CoveError::SegmentCorrupt);
    }

    let catalog = &catalogs[0];
    let object_types = catalog
        .types
        .iter()
        .map(|ty| (ty.object_type_id, ty))
        .collect::<BTreeMap<_, _>>();
    let index = &indexes[0];
    let index_entries = index
        .entries
        .iter()
        .map(|entry| ((entry.object_type_id, entry.segment_id), entry))
        .collect::<BTreeMap<_, _>>();
    if index_entries.len() != index.entries.len() {
        return Err(CoveError::SegmentCorrupt);
    }

    let segment_refs = segments
        .iter()
        .map(|(_, _, segment)| segment)
        .collect::<Vec<_>>();
    let segment_values = segments
        .iter()
        .map(|(_, _, segment)| segment.clone())
        .collect::<Vec<_>>();
    let file_local_record_ids = segment_refs
        .iter()
        .flat_map(|segment| {
            (0..segment.rows.len())
                .map(move |row_index| ((segment.header.segment_id as u64) << 32) | row_index as u64)
        })
        .collect::<Vec<_>>();
    let file_prev_refs = segment_refs
        .iter()
        .flat_map(|segment| {
            segment.rows.iter().map(|row| {
                row.prev_ref.map(|prev_ref| {
                    ((prev_ref.segment_id as u64) << 32) | prev_ref.row_index as u64
                })
            })
        })
        .collect::<Vec<_>>();
    validate_self_contained(&file_prev_refs, &file_local_record_ids)?;
    validate_temporal_chains(&segment_refs)?;

    let mut payloads_by_key = BTreeMap::new();
    for (section_offset, bytes, segment) in segments {
        let object_type = object_types
            .get(&segment.header.object_type_id)
            .ok_or_else(|| {
                CoveError::BadSchema(format!(
                    "temporal segment references unknown object_type_id {}",
                    segment.header.object_type_id
                ))
            })?;
        let key = (segment.header.object_type_id, segment.header.segment_id);
        if payloads_by_key
            .insert(key, (*section_offset, bytes, segment))
            .is_some()
        {
            return Err(CoveError::SegmentCorrupt);
        }
        let index_entry = index_entries.get(&key).ok_or(CoveError::SegmentCorrupt)?;
        validate_temporal_segment_against_index(index_entry, *section_offset, bytes, segment)?;
        validate_temporal_property_columns(object_type, segment, dictionary, required_features)?;
    }
    if payloads_by_key.len() != index.entries.len() {
        return Err(CoveError::SegmentCorrupt);
    }

    for manifest in trust_manifests {
        validate_trust_manifest_references(manifest, &segment_refs)?;
        manifest.verify_against(&segment_values)?;
    }
    Ok(())
}

fn validate_temporal_segment_against_index(
    index: &TemporalSegmentIndexEntryV1,
    section_offset: u64,
    bytes: &[u8],
    segment: &TemporalSegmentData,
) -> Result<(), CoveError> {
    if index.segment_id != segment.header.segment_id
        || index.object_type_id != segment.header.object_type_id
        || index.time_range_start_us != segment.header.time_range_start_us
        || index.time_range_end_us != segment.header.time_range_end_us
        || index.csn_min != segment.header.csn_min
        || index.csn_max != segment.header.csn_max
        || index.row_count != segment.header.row_count
        || index.length != bytes.len() as u64
    {
        return Err(CoveError::SegmentCorrupt);
    }
    if index.offset != 0 && index.offset != section_offset {
        return Err(CoveError::SegmentCorrupt);
    }
    let counts = temporal_record_kind_counts(segment);
    if index.delta_count != counts.0
        || index.snapshot_count != counts.1
        || index.baseline_count != counts.2
        || index.tombstone_count != counts.3
    {
        return Err(CoveError::SegmentCorrupt);
    }
    if !segment.rows.is_empty() {
        let min_goid = segment.rows.iter().map(|row| row.goid).min().unwrap();
        let max_goid = segment.rows.iter().map(|row| row.goid).max().unwrap();
        if index.min_goid != min_goid || index.max_goid != max_goid {
            return Err(CoveError::SegmentCorrupt);
        }
    }
    Ok(())
}

fn temporal_record_kind_counts(segment: &TemporalSegmentData) -> (u32, u32, u32, u32) {
    let mut delta = 0u32;
    let mut snapshot = 0u32;
    let mut baseline = 0u32;
    let mut tombstone = 0u32;
    for row in &segment.rows {
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

fn validate_temporal_property_columns(
    object_type: &crate::profile::cove_o::ObjectTypeEntryV1,
    segment: &TemporalSegmentData,
    dictionary: Option<&FileDictionaryView<'_>>,
    required_features: u64,
) -> Result<(), CoveError> {
    let properties = object_type
        .properties
        .iter()
        .map(|property| (property.property_id, property))
        .collect::<BTreeMap<_, _>>();
    let mut seen = BTreeSet::new();
    for column in &segment.property_columns {
        if !seen.insert(column.directory.column_id) {
            return Err(CoveError::BadSchema(format!(
                "duplicate temporal property column_id {}",
                column.directory.column_id
            )));
        }
        let property = properties.get(&column.directory.column_id).ok_or_else(|| {
            CoveError::BadSchema(format!(
                "temporal segment references unknown property_id {} for object_type_id {}",
                column.directory.column_id, object_type.object_type_id
            ))
        })?;
        if column.directory.logical_type != property.logical_type
            || column.directory.physical_kind != property.physical_kind
        {
            return Err(CoveError::PageCorrupt);
        }
        if (property.flags & PROPERTY_FLAG_BOOL_DECLARED_NUMERIC != 0)
            != (column.directory.flags & SEGMENT_COLUMN_FLAG_BOOL_DECLARED_NUMERIC != 0)
        {
            return Err(CoveError::PageCorrupt);
        }
        validate_temporal_property_pages(property, segment, column, dictionary, required_features)?;
    }
    if segment.header.column_count as usize != segment.property_columns.len() {
        return Err(CoveError::SegmentCorrupt);
    }
    Ok(())
}

fn validate_temporal_property_pages(
    property: &PropertyEntryV1,
    segment: &TemporalSegmentData,
    column: &TemporalPropertyColumn,
    dictionary: Option<&FileDictionaryView<'_>>,
    required_features: u64,
) -> Result<(), CoveError> {
    if column.page_index.entries.len() != expected_temporal_morsel_count(segment)? {
        return Err(CoveError::PageCorrupt);
    }
    let mut seen_morsels = BTreeSet::new();
    let mut rows_seen = 0u64;
    for page in &column.pages {
        if !seen_morsels.insert(page.index_entry.morsel_id) {
            return Err(CoveError::PageCorrupt);
        }
        let expected_rows = temporal_morsel_row_count(segment, page.index_entry.morsel_id)?;
        if page.index_entry.row_count != expected_rows {
            return Err(CoveError::PageCorrupt);
        }
        if !property.nullable && page.index_entry.null_count != 0 {
            return Err(CoveError::BadSchema(format!(
                "non-nullable property {} has page null_count {}",
                property.property_id, page.index_entry.null_count
            )));
        }
        validate_temporal_property_page_elision_features(
            &page.index_entry,
            Some(required_features),
        )?;
        rows_seen = rows_seen
            .checked_add(u64::from(page.index_entry.row_count))
            .ok_or(CoveError::ArithOverflow)?;
        let context = PageValidationContext {
            table_id: None,
            segment_id: Some(segment.header.segment_id),
            column_id: property.property_id,
            logical_type: property.logical_type,
            physical_kind: property.physical_kind,
            dictionary,
            zone_stats: None,
            codec_descriptors: &[],
            nested_schema: None,
        };
        if let Some(payload) = &page.payload {
            validate_column_page_payload(&context, &page.index_entry, payload)?;
        } else {
            validate_temporal_property_stats_only_page(&context, &page.index_entry)?;
        }
    }
    if rows_seen != u64::from(segment.header.row_count) {
        return Err(CoveError::PageCorrupt);
    }
    Ok(())
}

fn expected_temporal_morsel_count(segment: &TemporalSegmentData) -> Result<usize, CoveError> {
    if segment.header.row_count == 0 {
        return Ok(0);
    }
    if segment.header.morsel_count == 0 || segment.header.morsel_row_count == 0 {
        return Err(CoveError::SegmentCorrupt);
    }
    Ok(segment.header.morsel_count as usize)
}

fn temporal_morsel_row_count(
    segment: &TemporalSegmentData,
    morsel_id: u32,
) -> Result<u32, CoveError> {
    if morsel_id >= segment.header.morsel_count {
        return Err(CoveError::SegmentCorrupt);
    }
    let first_row = morsel_id
        .checked_mul(segment.header.morsel_row_count)
        .ok_or(CoveError::ArithOverflow)?;
    if first_row >= segment.header.row_count {
        return Err(CoveError::SegmentCorrupt);
    }
    let remaining = segment.header.row_count - first_row;
    Ok(remaining.min(segment.header.morsel_row_count))
}

fn validate_temporal_chains(segments: &[&TemporalSegmentData]) -> Result<(), CoveError> {
    let rows = segments
        .iter()
        .flat_map(|segment| {
            segment
                .rows
                .iter()
                .enumerate()
                .map(move |(row_index, row)| ((segment.header.segment_id, row_index as u32), row))
        })
        .collect::<BTreeMap<_, _>>();
    for ((segment_id, row_index), row) in &rows {
        validate_prev_ref_target_kind(row.prev_ref, &rows)?;
        if matches!(row.record_kind, RecordKind::Delta | RecordKind::Tombstone) {
            let mut seen = BTreeSet::new();
            let mut current = Some((*segment_id, *row_index));
            let mut anchored = false;
            while let Some(key) = current {
                if !seen.insert(key) {
                    return Err(CoveError::RefInvalid);
                }
                let current_row = rows.get(&key).ok_or(CoveError::NotSelfContained)?;
                if matches!(
                    current_row.record_kind,
                    RecordKind::Baseline | RecordKind::Snapshot
                ) {
                    anchored = true;
                    break;
                }
                if current_row.prev_ref.is_none() {
                    anchored = true;
                    break;
                }
                current = current_row
                    .prev_ref
                    .map(|prev_ref| (prev_ref.segment_id, prev_ref.row_index));
            }
            if !anchored && row.prev_ref.is_some() {
                return Err(CoveError::NotSelfContained);
            }
        }
    }
    Ok(())
}

fn validate_prev_ref_target_kind(
    prev_ref: Option<crate::profile::cove_o::CoveRecordRefV1>,
    rows: &BTreeMap<(u32, u32), &crate::profile::cove_o::TemporalRowEntryV1>,
) -> Result<(), CoveError> {
    let Some(prev_ref) = prev_ref else {
        return Ok(());
    };
    let target = rows
        .get(&(prev_ref.segment_id, prev_ref.row_index))
        .ok_or(CoveError::NotSelfContained)?;
    if prev_ref.target_kind != target_kind_for_record_kind(target.record_kind) {
        return Err(CoveError::RefInvalid);
    }
    Ok(())
}

fn target_kind_for_record_kind(kind: RecordKind) -> u8 {
    match kind {
        RecordKind::Snapshot | RecordKind::Baseline => 1,
        RecordKind::Delta | RecordKind::Tombstone | RecordKind::ReservedLegacyMaterializedDelta => {
            0
        }
    }
}

fn validate_trust_manifest_references(
    manifest: &TrustManifest,
    segments: &[&TemporalSegmentData],
) -> Result<(), CoveError> {
    let row_counts = segments
        .iter()
        .map(|segment| (segment.header.segment_id, segment.rows.len()))
        .collect::<BTreeMap<_, _>>();
    let mut seen = BTreeSet::new();
    for entry in &manifest.entries {
        if !seen.insert((entry.segment_id, entry.row_index)) {
            return Err(CoveError::RefInvalid);
        }
        let row_count = row_counts
            .get(&entry.segment_id)
            .ok_or(CoveError::RefInvalid)?;
        if entry.row_index as usize >= *row_count {
            return Err(CoveError::RefInvalid);
        }
    }
    Ok(())
}

pub(super) fn validate_cove_e_semantics(
    data: &[u8],
    validated: &ValidatedCoveFile,
    stages: &mut Vec<ValidationStageReport>,
) -> Result<(), CoveError> {
    let mut checked = 0u32;
    let mut registries = Vec::new();
    let mut execution_descriptors = Vec::new();
    let mut scope_descriptors = Vec::new();
    let mut code_space_descriptors = Vec::new();
    let mut mount_policies = Vec::new();
    for entry in &validated.footer.sections {
        let kind = SectionKind::from_u16(entry.section_kind).ok_or_else(|| {
            CoveError::BadSection(format!("unknown section_kind {}", entry.section_kind))
        })?;
        let result = match kind {
            SectionKind::EngineProfileRegistry => {
                let payload = compression::section_payload(data, entry)?;
                EngineProfileRegistry::parse(&payload).map(|registry| {
                    registries.push(registry);
                })
            }
            SectionKind::ExecutionCodeDescriptor => {
                let payload = compression::section_payload(data, entry)?;
                ExecutionCodeDescriptorV1::parse(&payload).map(|descriptor| {
                    execution_descriptors.push(descriptor);
                })
            }
            SectionKind::ExecutionScopeDescriptor => {
                let payload = compression::section_payload(data, entry)?;
                ExecutionScopeDescriptorV1::parse(&payload).map(|descriptor| {
                    scope_descriptors.push(descriptor);
                })
            }
            SectionKind::CodeSpaceDescriptor => {
                let payload = compression::section_payload(data, entry)?;
                CodeSpaceDescriptorV1::parse(&payload).map(|descriptor| {
                    code_space_descriptors.push(descriptor);
                })
            }
            SectionKind::EngineMountPolicy => {
                let payload = compression::section_payload(data, entry)?;
                EngineMountPolicyV1::parse(&payload).map(|policy| {
                    mount_policies.push(policy);
                })
            }
            _ => continue,
        };
        checked += 1;
        if let Err(err) = result {
            if profile_error_is_fatal(&validated.header, entry, FEATURE_ENGINE_PROFILE) {
                return Err(err);
            }
        }
    }
    if let Err(err) = validate_cove_e_cross_references(
        &registries,
        &execution_descriptors,
        &scope_descriptors,
        &code_space_descriptors,
        &mount_policies,
    ) {
        let engine_required = validated.header.required_features & FEATURE_ENGINE_PROFILE != 0
            || validated
                .footer
                .sections
                .iter()
                .any(|entry| entry.required_features & FEATURE_ENGINE_PROFILE != 0);
        if engine_required {
            return Err(err);
        }
    }
    push_stage(
        stages,
        ValidationStage::CoveEngine,
        ValidationStageStatus::Checked,
        checked,
    );
    Ok(())
}

fn validate_cove_e_cross_references(
    registries: &[EngineProfileRegistry],
    execution_descriptors: &[ExecutionCodeDescriptorV1],
    scope_descriptors: &[ExecutionScopeDescriptorV1],
    code_space_descriptors: &[CodeSpaceDescriptorV1],
    mount_policies: &[EngineMountPolicyV1],
) -> Result<(), CoveError> {
    use std::collections::HashSet;

    let mut execution_ids = HashSet::new();
    for descriptor in execution_descriptors {
        if !execution_ids.insert(descriptor.descriptor_id) {
            return Err(CoveError::BadEngineProfile);
        }
    }

    let mut scope_ids = HashSet::new();
    for descriptor in scope_descriptors {
        if !scope_ids.insert(descriptor.scope_id) {
            return Err(CoveError::BadEngineProfile);
        }
    }

    let mut code_space_ids = HashSet::new();
    for descriptor in code_space_descriptors {
        if !code_space_ids.insert(descriptor.code_space_id) {
            return Err(CoveError::BadEngineProfile);
        }
    }

    let mut policy_ids = HashSet::new();
    for policy in mount_policies {
        if !policy_ids.insert(policy.policy_id) {
            return Err(CoveError::BadEngineProfile);
        }
    }

    for registry in registries {
        for profile in &registry.profiles {
            if profile.execution_descriptor_ref != 0
                && !execution_ids.contains(&profile.execution_descriptor_ref)
            {
                return Err(CoveError::BadEngineProfile);
            }
            if profile.mount_policy_ref != 0 && !policy_ids.contains(&profile.mount_policy_ref) {
                return Err(CoveError::BadEngineProfile);
            }
        }
    }

    for descriptor in execution_descriptors {
        if descriptor.scope_ref != 0 && !scope_ids.contains(&descriptor.scope_ref) {
            return Err(CoveError::BadEngineProfile);
        }
        if descriptor.code_space_ref != 0 && !code_space_ids.contains(&descriptor.code_space_ref) {
            return Err(CoveError::BadEngineProfile);
        }
    }

    for policy in mount_policies {
        if policy.code_space_ref != 0 && !code_space_ids.contains(&policy.code_space_ref) {
            return Err(CoveError::BadEngineProfile);
        }
    }

    Ok(())
}

pub(super) fn validate_cove_h_semantics(
    data: &[u8],
    validated: &ValidatedCoveFile,
    stages: &mut Vec<ValidationStageReport>,
) -> Result<(), CoveError> {
    let mut checked = 0u32;
    for entry in &validated.footer.sections {
        let kind = SectionKind::from_u16(entry.section_kind).ok_or_else(|| {
            CoveError::BadSection(format!("unknown section_kind {}", entry.section_kind))
        })?;
        let result = match kind {
            SectionKind::HarborMountHints => {
                let payload = compression::section_payload(data, entry)?;
                HarborMountHintsV1::parse(&payload).map(|_| ())
            }
            _ => continue,
        };
        checked += 1;
        if let Err(err) = result {
            if profile_error_is_fatal(&validated.header, entry, FEATURE_HARBOR_PROFILE) {
                return Err(err);
            }
        }
    }
    push_stage(
        stages,
        ValidationStage::CoveHarbor,
        ValidationStageStatus::Checked,
        checked,
    );
    Ok(())
}

pub(super) fn validate_cove_map_semantics(
    data: &[u8],
    validated: &ValidatedCoveFile,
    stages: &mut Vec<ValidationStageReport>,
) -> Result<(), CoveError> {
    let mut checked = 0u32;
    let mut map_sections = Vec::<EmbeddedMapSection>::new();
    for entry in &validated.footer.sections {
        let kind = SectionKind::from_u16(entry.section_kind).ok_or_else(|| {
            CoveError::BadSection(format!("unknown section_kind {}", entry.section_kind))
        })?;
        let result = match kind {
            SectionKind::MapSourceCatalog
            | SectionKind::MapFunctionRegistry
            | SectionKind::MapIdentityRuleCatalog
            | SectionKind::MapRowSemanticsCatalog
            | SectionKind::MapAssertionLog
            | SectionKind::MapIdentityEquivalenceIndex
            | SectionKind::MapEvidenceIndex
            | SectionKind::MapConversionReport
            | SectionKind::MapProjectionCatalog => {
                let payload = compression::section_payload(data, entry)?;
                parse_embedded_section(kind, &payload).map(|section| {
                    map_sections.push(section);
                })
            }
            _ => continue,
        };
        checked += 1;
        if let Err(err) = result {
            if profile_error_is_fatal(&validated.header, entry, FEATURE_SEMANTIC_MAP) {
                return Err(err);
            }
        }
    }
    if let Err(err) = validate_embedded_sections(&map_sections) {
        let map_required = validated.header.required_features & FEATURE_SEMANTIC_MAP != 0
            || validated
                .footer
                .sections
                .iter()
                .any(|entry| entry.required_features & FEATURE_SEMANTIC_MAP != 0);
        if map_required {
            return Err(err);
        }
    }
    push_stage(
        stages,
        ValidationStage::CoveMap,
        ValidationStageStatus::Checked,
        checked,
    );
    Ok(())
}

fn profile_error_is_fatal(header: &CoveHeaderV1, entry: &CoveSectionEntryV1, feature: u64) -> bool {
    header.required_features & feature != 0 || entry.required_features & feature != 0
}
