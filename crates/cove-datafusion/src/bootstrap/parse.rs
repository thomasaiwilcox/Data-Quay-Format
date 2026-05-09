use std::sync::Arc;

use cove_core::{
    checksum,
    compression::section_payload_from_raw,
    constants::SectionKind,
    dictionary::FileDictionary,
    domain::ColumnDomain,
    footer::{CoveFooter, CoveSectionEntryV1},
    header::{CoveHeaderV1, HEADER_SIZE},
    index::{
        aggregate::AggregateSynopsis, bloom::BloomFilterIndex, composite::CompositeIndex,
        exact_set::ExactSetIndex, inverted::InvertedMorselIndex, lookup::LookupIndex,
        topn::TopNSummary,
    },
    mount::EngineMetadata,
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

use crate::{
    dataset_state::PruningMetadata,
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
            "COVE DataFusion M2 requires exactly one table catalog section, found {}",
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
    })
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
