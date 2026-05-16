use std::sync::Arc;

use cove_core::{
    checksum,
    codec::CodecExtensionDescriptorV2,
    compression::section_payload_from_raw,
    constants::{SectionKind, FEATURE_EXTENDED_FEATURE_SET},
    dictionary::FileDictionary,
    domain::ColumnDomain,
    feature_binding::SectionFeatureBindingSectionV2,
    feature_scope::{ExtendedFeatureSetV2, FeatureScopeTable, ProfileCapabilityMatrixV2},
    footer::{CoveFooter, CoveSectionEntryV1},
    header::{CoveHeaderV1, HEADER_SIZE},
    index::{
        aggregate::AggregateSynopsis, bloom::BloomFilterIndex, composite::CompositeIndex,
        exact_set::ExactSetIndex, inverted::InvertedMorselIndex, lookup::LookupIndex,
        topn::TopNSummary,
    },
    mount::EngineMetadata,
    nested_schema::NestedSchemaSectionV1,
    postscript::{CovePostscriptV1, POSTSCRIPT_TOTAL_SIZE},
    profile::cove_e::{
        CodeSpaceDescriptorV1, EngineMountPolicyV1, EngineProfileRegistry,
        ExecutionCodeDescriptorV1, ExecutionScopeDescriptorV1,
    },
    segment::TableSegmentIndex,
    table::TableCatalog,
    zone_stats::ZoneStatsSection,
    CoveError,
};
use cove_coverage::{
    CoveragePlanCandidateV2, CoverageProofRecordV2, CoverageProviderDescriptorV2, CoverageSetV2,
    PredicateNormalFormV2, PredicateNormalFormWithPayloadV2,
};
use cove_layout::{
    validate_fast_metadata_authority, validate_page_cluster_authority, FastMetadataIndexV2,
    LayoutPlanV2, PageClusterDirectoryV2, ScanSplitIndexV2, ValidatedLayoutPlanV2,
    ValidatedScanSplitIndexV2, ValidatedZeroCopyBufferMapV2, ZeroCopyBufferMapV2,
};

use crate::{
    dataset_state::{LayoutPlanningMetadataV2, PruningMetadata},
    range_reader::{CoveRangeReader, RangeReadKind},
};

pub(super) async fn bootstrap_header_footer<R: CoveRangeReader + ?Sized>(
    file_len: u64,
    reader: &R,
) -> Result<(CoveHeaderV1, CovePostscriptV1, CoveFooter), CoveError> {
    if file_len < (HEADER_SIZE + POSTSCRIPT_TOTAL_SIZE) as u64 {
        return Err(CoveError::BufferTooShort);
    }
    let tail_start = file_len
        .checked_sub(POSTSCRIPT_TOTAL_SIZE as u64)
        .ok_or(CoveError::BufferTooShort)?;
    let ranges = reader
        .read_ranges(
            &[0..HEADER_SIZE as u64, tail_start..file_len],
            RangeReadKind::Metadata,
        )
        .await?;
    if ranges.len() != 2 {
        return Err(CoveError::BufferTooShort);
    }
    let header = CoveHeaderV1::parse(&ranges[0])?;
    let postscript = CovePostscriptV1::parse_from_tail(&ranges[1])?;
    if postscript.file_len != file_len {
        return Err(CoveError::OffsetRange);
    }
    if header.required_features != postscript.required_features
        || header.optional_features != postscript.optional_features
    {
        return Err(CoveError::BadSection(
            "header and postscript feature bits differ".into(),
        ));
    }
    let footer_end = postscript.footer.end_offset()?;
    if postscript.footer.offset < HEADER_SIZE as u64 || footer_end > tail_start {
        return Err(CoveError::OffsetRange);
    }
    let footer_raw = reader
        .read_range(
            postscript.footer.offset..footer_end,
            RangeReadKind::Metadata,
        )
        .await?;
    let footer_payload = section_payload_from_raw(
        &footer_raw,
        postscript.footer.length,
        postscript.footer.uncompressed_length,
        postscript.footer.compression,
        postscript.footer.crc32c,
    )?;
    let footer = CoveFooter::parse(&footer_payload)?;
    if footer.header.total_len()? != postscript.footer.uncompressed_length {
        return Err(CoveError::BadSection(
            "footer header length does not match postscript footer uncompressed_length".into(),
        ));
    }
    validate_section_ranges(&footer, postscript.footer.offset)?;
    Ok((header, postscript, footer))
}

pub(super) async fn parse_table_catalog<R: CoveRangeReader + ?Sized>(
    reader: &R,
    footer: &CoveFooter,
) -> Result<TableCatalog, CoveError> {
    let entries = find_sections(footer, SectionKind::TableCatalog);
    if entries.len() != 1 {
        return Err(CoveError::BadSchema(format!(
            "COVE DataFusion v2 requires exactly one table catalog section, found {}",
            entries.len()
        )));
    }
    let payload = read_section_payload(reader, entries[0]).await?;
    TableCatalog::parse(&payload)
}

pub(super) async fn parse_dictionary<R: CoveRangeReader + ?Sized>(
    reader: &R,
    footer: &CoveFooter,
) -> Result<Option<FileDictionary>, CoveError> {
    let Some(index_entry) = find_sections(footer, SectionKind::FileDictionaryIndex)
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    let index_payload = read_section_payload(reader, index_entry).await?;
    let payload = match find_sections(footer, SectionKind::FileDictionaryPayload)
        .into_iter()
        .next()
    {
        Some(entry) => read_section_payload(reader, entry).await?,
        None => Vec::new(),
    };
    FileDictionary::parse(&index_payload, &payload).map(Some)
}

pub(super) async fn parse_engine_metadata<R: CoveRangeReader + ?Sized>(
    reader: &R,
    footer: &CoveFooter,
) -> Result<EngineMetadata, CoveError> {
    Ok(EngineMetadata {
        engine_profile_registries: parse_sections(
            reader,
            footer,
            SectionKind::EngineProfileRegistry,
            EngineProfileRegistry::parse,
        )
        .await?,
        execution_descriptors: parse_sections(
            reader,
            footer,
            SectionKind::ExecutionCodeDescriptor,
            ExecutionCodeDescriptorV1::parse,
        )
        .await?,
        execution_scopes: parse_sections(
            reader,
            footer,
            SectionKind::ExecutionScopeDescriptor,
            ExecutionScopeDescriptorV1::parse,
        )
        .await?,
        code_spaces: parse_sections(
            reader,
            footer,
            SectionKind::CodeSpaceDescriptor,
            CodeSpaceDescriptorV1::parse,
        )
        .await?,
        engine_mount_policies: parse_sections(
            reader,
            footer,
            SectionKind::EngineMountPolicy,
            EngineMountPolicyV1::parse,
        )
        .await?,
    })
}

pub(super) async fn parse_segment_index<R: CoveRangeReader + ?Sized>(
    reader: &R,
    footer: &CoveFooter,
) -> Result<TableSegmentIndex, CoveError> {
    let entries = find_sections(footer, SectionKind::TableSegmentIndex);
    if entries.is_empty() {
        return Ok(TableSegmentIndex::default());
    }
    if entries.len() != 1 {
        return Err(CoveError::SegmentCorrupt);
    }
    let payload = read_section_payload(reader, entries[0]).await?;
    TableSegmentIndex::parse(&payload)
}

pub(super) async fn parse_pruning_metadata<R: CoveRangeReader + ?Sized>(
    reader: &R,
    footer: &CoveFooter,
) -> Result<PruningMetadata, CoveError> {
    Ok(PruningMetadata {
        nested_schemas: Arc::new(
            parse_optional_sections(
                reader,
                footer,
                SectionKind::NestedSchema,
                NestedSchemaSectionV1::parse,
            )
            .await,
        ),
        codec_descriptors: Arc::new(
            parse_optional_flat_sections(
                reader,
                footer,
                SectionKind::CodecExtensionRegistry,
                CodecExtensionDescriptorV2::parse_many,
            )
            .await,
        ),
        column_domains: Arc::new(
            parse_optional_sections(
                reader,
                footer,
                SectionKind::ColumnDomain,
                ColumnDomain::parse,
            )
            .await,
        ),
        zone_stats: Arc::new(
            parse_optional_sections(
                reader,
                footer,
                SectionKind::ZoneStats,
                ZoneStatsSection::parse,
            )
            .await,
        ),
        exact_sets: Arc::new(
            parse_optional_sections(
                reader,
                footer,
                SectionKind::ExactSetIndex,
                ExactSetIndex::parse,
            )
            .await,
        ),
        blooms: Arc::new(
            parse_optional_sections(
                reader,
                footer,
                SectionKind::BloomIndex,
                BloomFilterIndex::parse,
            )
            .await,
        ),
        lookups: Arc::new(
            parse_optional_sections(reader, footer, SectionKind::LookupIndex, LookupIndex::parse)
                .await,
        ),
        inverted: Arc::new(
            parse_optional_sections(
                reader,
                footer,
                SectionKind::InvertedMorselIndex,
                InvertedMorselIndex::parse,
            )
            .await,
        ),
        aggregates: Arc::new(
            parse_optional_sections(
                reader,
                footer,
                SectionKind::AggregateSynopsis,
                AggregateSynopsis::parse,
            )
            .await,
        ),
        composites: Arc::new(
            parse_optional_sections(
                reader,
                footer,
                SectionKind::CompositeZoneIndex,
                CompositeIndex::parse,
            )
            .await,
        ),
        topn: Arc::new(
            parse_optional_sections(
                reader,
                footer,
                SectionKind::TopNZoneSummary,
                TopNSummary::parse,
            )
            .await,
        ),
        coverage_providers: Arc::new(
            parse_optional_flat_sections(
                reader,
                footer,
                SectionKind::CoverageProviderRegistry,
                CoverageProviderDescriptorV2::parse_many,
            )
            .await,
        ),
        coverage_sets: Arc::new(
            parse_optional_sections(
                reader,
                footer,
                SectionKind::CoverageSet,
                CoverageSetV2::parse,
            )
            .await,
        ),
        coverage_proofs: Arc::new(
            parse_optional_flat_sections(
                reader,
                footer,
                SectionKind::CoverageProofRecord,
                CoverageProofRecordV2::parse_many,
            )
            .await,
        ),
        coverage_plan_candidates: Arc::new(
            parse_optional_flat_sections(
                reader,
                footer,
                SectionKind::CoveragePlanCandidate,
                CoveragePlanCandidateV2::parse_many,
            )
            .await,
        ),
        predicate_forms: Arc::new(
            parse_optional_flat_sections(
                reader,
                footer,
                SectionKind::PredicateNormalForm,
                PredicateNormalFormV2::parse_many,
            )
            .await,
        ),
        predicate_forms_with_payloads: Arc::new(
            parse_optional_flat_sections(
                reader,
                footer,
                SectionKind::PredicateNormalForm,
                PredicateNormalFormWithPayloadV2::parse_many,
            )
            .await,
        ),
    })
}

pub(super) async fn parse_layout_metadata<R: CoveRangeReader + ?Sized>(
    reader: &R,
    header: &CoveHeaderV1,
    footer: &CoveFooter,
    table: &cove_core::table::TableEntry,
    segments: &[cove_core::segment::TableSegmentIndexEntryV1],
) -> LayoutPlanningMetadataV2 {
    let mut layout = LayoutPlanningMetadataV2::default();

    layout.fast_metadata = match resolve_header_section_id(
        footer,
        header.fast_metadata_section_id,
        SectionKind::FastMetadataIndex,
        "fast_metadata_section_id",
    ) {
        Ok(Some(entry)) => match read_section_payload(reader, entry).await {
            Ok(payload) => match FastMetadataIndexV2::parse(&payload)
                .and_then(|index| validate_fast_metadata_authority(&index, footer).map(|_| index))
            {
                Ok(index) => {
                    layout.record_loaded();
                    Some(Arc::new(index))
                }
                Err(_) => {
                    layout.record_ignored();
                    None
                }
            },
            Err(_) => {
                layout.record_ignored();
                None
            }
        },
        Ok(None) => None,
        Err(_) => {
            layout.record_ignored();
            None
        }
    };

    for entry in find_sections(footer, SectionKind::PageClusterDirectory) {
        let Ok(payload) = read_section_payload(reader, entry).await else {
            layout.record_ignored();
            continue;
        };
        let Ok(directory) = PageClusterDirectoryV2::parse(&payload) else {
            layout.record_ignored();
            continue;
        };
        if validate_page_cluster_authority(&directory, footer, table, segments).is_ok() {
            if layout.page_clusters.is_none() {
                layout.page_clusters = Some(Arc::new(directory));
                layout.record_loaded();
            } else {
                layout.record_ignored();
            }
        } else {
            layout.record_ignored();
        }
    }

    for entry in find_sections(footer, SectionKind::ScanSplitIndex) {
        let Ok(payload) = read_section_payload(reader, entry).await else {
            layout.record_ignored();
            continue;
        };
        let Ok(index) = ScanSplitIndexV2::parse(&payload) else {
            layout.record_ignored();
            continue;
        };
        match ValidatedScanSplitIndexV2::validate(
            index,
            table,
            segments,
            layout.page_clusters.as_deref(),
        ) {
            Ok(validated) if layout.scan_splits.is_none() => {
                layout.scan_splits = Some(Arc::new(validated.index));
                layout.record_loaded();
            }
            _ => layout.record_ignored(),
        }
    }

    let mut layout_plans = Vec::new();
    for entry in find_sections(footer, SectionKind::LayoutPlan) {
        let Ok(payload) = read_section_payload(reader, entry).await else {
            layout.record_ignored();
            continue;
        };
        let Ok(plan) = LayoutPlanV2::parse(&payload) else {
            layout.record_ignored();
            continue;
        };
        match ValidatedLayoutPlanV2::validate(
            plan,
            footer,
            table,
            segments,
            layout.page_clusters.as_deref(),
            layout.scan_splits.as_deref(),
        ) {
            Ok(validated) => {
                layout_plans.push(validated.plan);
                layout.record_loaded();
            }
            Err(_) => layout.record_ignored(),
        }
    }
    layout.layout_plans = Arc::new(layout_plans);

    let mut zero_copy_maps = Vec::new();
    for entry in find_sections(footer, SectionKind::ZeroCopyBufferMap) {
        let Ok(payload) = read_section_payload(reader, entry).await else {
            layout.record_ignored();
            continue;
        };
        let Ok(map) = ZeroCopyBufferMapV2::parse(&payload) else {
            layout.record_ignored();
            continue;
        };
        match ValidatedZeroCopyBufferMapV2::validate(map, table, segments) {
            Ok(validated) => {
                zero_copy_maps.push(validated.map);
                layout.record_loaded();
            }
            Err(_) => layout.record_ignored(),
        }
    }
    layout.zero_copy_maps = Arc::new(zero_copy_maps);

    layout
}

pub(super) async fn parse_feature_scope_table<R: CoveRangeReader + ?Sized>(
    reader: &R,
    header: &CoveHeaderV1,
    footer: &CoveFooter,
) -> Result<FeatureScopeTable, CoveError> {
    let extended_entry = resolve_header_section_id(
        footer,
        header.feature_set_section_id,
        SectionKind::ExtendedFeatureSet,
        "feature_set_section_id",
    )?;
    let profile_matrix_entry = resolve_header_section_id(
        footer,
        header.profile_capability_section_id,
        SectionKind::ProfileCapabilityMatrix,
        "profile_capability_section_id",
    )?;
    resolve_header_section_id(
        footer,
        header.fast_metadata_section_id,
        SectionKind::FastMetadataIndex,
        "fast_metadata_section_id",
    )?;
    if header.required_features & FEATURE_EXTENDED_FEATURE_SET != 0 && extended_entry.is_none() {
        return Err(CoveError::BadSection(
            "FEATURE_EXTENDED_FEATURE_SET is required but feature_set_section_id is absent".into(),
        ));
    }

    let extended = match extended_entry {
        Some(entry) => {
            let payload = read_section_payload(reader, entry).await?;
            let set = ExtendedFeatureSetV2::parse(&payload)?;
            set.validate_against_low_words(header.required_features, header.optional_features)?;
            Some(set)
        }
        None => None,
    };
    let profile_matrix = match profile_matrix_entry {
        Some(entry) => {
            let payload = read_section_payload(reader, entry).await?;
            Some(ProfileCapabilityMatrixV2::parse(&payload)?)
        }
        None => None,
    };
    let mut section_bindings = Vec::<SectionFeatureBindingSectionV2>::new();
    for entry in find_sections(footer, SectionKind::SectionFeatureBinding) {
        let Some(extended) = extended.as_ref() else {
            return Err(CoveError::BadSection(
                "SECTION_FEATURE_BINDING requires EXTENDED_FEATURE_SET".into(),
            ));
        };
        let payload = read_section_payload(reader, entry).await?;
        let parsed = SectionFeatureBindingSectionV2::parse(&payload)?;
        extended.validate_binding_horizon(&parsed)?;
        section_bindings.push(parsed);
    }
    if let Some(matrix) = profile_matrix.as_ref() {
        for entry in &matrix.entries {
            if entry.global_feature_word_index == 0 {
                continue;
            }
            let Some(extended) = extended.as_ref() else {
                return Err(CoveError::BadSection(
                    "PROFILE_CAPABILITY_MATRIX references extended feature word without EXTENDED_FEATURE_SET"
                        .into(),
                ));
            };
            if entry.global_feature_word_index >= extended.header.word_count {
                return Err(CoveError::BadSection(
                    "PROFILE_CAPABILITY_MATRIX references a feature word beyond EXTENDED_FEATURE_SET horizon"
                        .into(),
                ));
            }
        }
    }
    let table = FeatureScopeTable::build_many(
        header,
        footer,
        extended.as_ref(),
        profile_matrix.as_ref(),
        &section_bindings,
    )?;
    table.reject_file_required_unknowns()?;
    Ok(table)
}

fn validate_section_ranges(footer: &CoveFooter, footer_start: u64) -> Result<(), CoveError> {
    let mut ranges: Vec<(u64, u64, u32)> = Vec::new();
    let mut last_section_id = None;
    for entry in &footer.sections {
        if let Some(last) = last_section_id {
            if entry.section_id <= last {
                return Err(CoveError::BadSection(format!(
                    "section_id {} is not greater than previous id {}",
                    entry.section_id, last
                )));
            }
        }
        last_section_id = Some(entry.section_id);
        let section_end = entry.end_offset()?;
        if entry.offset < HEADER_SIZE as u64 || section_end > footer_start {
            return Err(CoveError::OffsetRange);
        }
        for (start, end, id) in &ranges {
            if entry.length != 0 && entry.offset < *end && section_end > *start {
                return Err(CoveError::BadSection(format!(
                    "section {} overlaps section {id}",
                    entry.section_id
                )));
            }
        }
        ranges.push((entry.offset, section_end, entry.section_id));
    }
    Ok(())
}

async fn parse_sections<R, T, F>(
    reader: &R,
    footer: &CoveFooter,
    kind: SectionKind,
    mut parse: F,
) -> Result<Vec<T>, CoveError>
where
    R: CoveRangeReader + ?Sized,
    F: FnMut(&[u8]) -> Result<T, CoveError>,
{
    let mut out = Vec::new();
    for entry in find_sections(footer, kind) {
        let payload = read_section_payload(reader, entry).await?;
        out.push(parse(&payload)?);
    }
    Ok(out)
}

async fn parse_optional_sections<R, T, F>(
    reader: &R,
    footer: &CoveFooter,
    kind: SectionKind,
    mut parse: F,
) -> Vec<T>
where
    R: CoveRangeReader + ?Sized,
    F: FnMut(&[u8]) -> Result<T, CoveError>,
{
    let mut out = Vec::new();
    for entry in find_sections(footer, kind) {
        if let Ok(payload) = read_section_payload(reader, entry).await {
            if let Ok(value) = parse(&payload) {
                out.push(value);
            }
        }
    }
    out
}

async fn parse_optional_flat_sections<R, T, F>(
    reader: &R,
    footer: &CoveFooter,
    kind: SectionKind,
    mut parse: F,
) -> Vec<T>
where
    R: CoveRangeReader + ?Sized,
    F: FnMut(&[u8]) -> Result<Vec<T>, CoveError>,
{
    let mut out = Vec::new();
    for entry in find_sections(footer, kind) {
        if let Ok(payload) = read_section_payload(reader, entry).await {
            if let Ok(mut values) = parse(&payload) {
                out.append(&mut values);
            }
        }
    }
    out
}

async fn read_section_payload<R: CoveRangeReader + ?Sized>(
    reader: &R,
    entry: &CoveSectionEntryV1,
) -> Result<Vec<u8>, CoveError> {
    let end = entry.end_offset()?;
    let raw = reader
        .read_range(entry.offset..end, RangeReadKind::Metadata)
        .await?;
    if checksum::crc32c(&raw) != entry.crc32c {
        return Err(CoveError::ChecksumMismatch);
    }
    section_payload_from_raw(
        &raw,
        entry.length,
        entry.uncompressed_length,
        entry.compression,
        entry.crc32c,
    )
    .map(|payload| payload.into_owned())
}

fn find_sections(footer: &CoveFooter, kind: SectionKind) -> Vec<&CoveSectionEntryV1> {
    footer
        .sections
        .iter()
        .filter(|entry| entry.section_kind == kind as u16)
        .collect()
}

fn resolve_header_section_id<'a>(
    footer: &'a CoveFooter,
    section_id: u32,
    expected_kind: SectionKind,
    field_name: &str,
) -> Result<Option<&'a CoveSectionEntryV1>, CoveError> {
    if section_id == 0 {
        return Ok(None);
    }
    let Some(entry) = footer
        .sections
        .iter()
        .find(|entry| entry.section_id == section_id)
    else {
        return Err(CoveError::BadSection(format!(
            "header {field_name} references missing section id {section_id}"
        )));
    };
    if entry.section_kind != expected_kind as u16 {
        return Err(CoveError::BadSection(format!(
            "header {field_name} references section id {section_id} with wrong kind"
        )));
    }
    Ok(Some(entry))
}
