use cove_core::{
    codec::CodecExtensionDescriptorV2,
    compression,
    constants::SectionKind,
    domain::ColumnDomain,
    footer::CoveFooter,
    index::{
        aggregate::AggregateSynopsis, bloom::BloomFilterIndex, composite::CompositeIndex,
        exact_set::ExactSetIndex, inverted::InvertedMorselIndex, lookup::LookupIndex,
        topn::TopNSummary,
    },
    zone_stats::ZoneStatsSection,
};
use cove_coverage::{
    CoveragePlanCandidateV2, CoverageProofRecordV2, CoverageProviderDescriptorV2, CoverageSetV2,
    PredicateNormalFormV2, PredicateNormalFormWithPayloadV2,
};

pub fn parse_column_domains_from_sections(bytes: &[u8], footer: &CoveFooter) -> Vec<ColumnDomain> {
    footer
        .sections
        .iter()
        .filter(|entry| entry.section_kind == SectionKind::ColumnDomain as u16)
        .filter_map(|entry| compression::section_payload(bytes, entry).ok())
        .filter_map(|payload| ColumnDomain::parse(&payload).ok())
        .collect()
}

pub fn parse_codec_descriptors_from_sections(
    bytes: &[u8],
    footer: &CoveFooter,
) -> Vec<CodecExtensionDescriptorV2> {
    footer
        .sections
        .iter()
        .filter(|entry| entry.section_kind == SectionKind::CodecExtensionRegistry as u16)
        .filter_map(|entry| compression::section_payload(bytes, entry).ok())
        .filter_map(|payload| CodecExtensionDescriptorV2::parse_many(&payload).ok())
        .flatten()
        .collect()
}

pub fn parse_zone_stats_from_sections(bytes: &[u8], footer: &CoveFooter) -> Vec<ZoneStatsSection> {
    footer
        .sections
        .iter()
        .filter(|entry| entry.section_kind == SectionKind::ZoneStats as u16)
        .filter_map(|entry| compression::section_payload(bytes, entry).ok())
        .filter_map(|payload| ZoneStatsSection::parse(&payload).ok())
        .collect()
}

pub fn parse_exact_sets_from_sections(bytes: &[u8], footer: &CoveFooter) -> Vec<ExactSetIndex> {
    parse_exact_sets(bytes, footer)
}

pub fn parse_blooms_from_sections(bytes: &[u8], footer: &CoveFooter) -> Vec<BloomFilterIndex> {
    parse_blooms(bytes, footer)
}

pub fn parse_lookups_from_sections(bytes: &[u8], footer: &CoveFooter) -> Vec<LookupIndex> {
    parse_lookups(bytes, footer)
}

pub fn parse_inverted_from_sections(bytes: &[u8], footer: &CoveFooter) -> Vec<InvertedMorselIndex> {
    parse_inverted(bytes, footer)
}

pub fn parse_aggregates_from_sections(bytes: &[u8], footer: &CoveFooter) -> Vec<AggregateSynopsis> {
    parse_aggregates(bytes, footer)
}

pub fn parse_composites_from_sections(bytes: &[u8], footer: &CoveFooter) -> Vec<CompositeIndex> {
    parse_composites(bytes, footer)
}

pub fn parse_topn_from_sections(bytes: &[u8], footer: &CoveFooter) -> Vec<TopNSummary> {
    parse_topn(bytes, footer)
}

pub fn parse_coverage_providers_from_sections(
    bytes: &[u8],
    footer: &CoveFooter,
) -> Vec<CoverageProviderDescriptorV2> {
    footer
        .sections
        .iter()
        .filter(|entry| entry.section_kind == SectionKind::CoverageProviderRegistry as u16)
        .filter_map(|entry| compression::section_payload(bytes, entry).ok())
        .filter_map(|payload| CoverageProviderDescriptorV2::parse_many(&payload).ok())
        .flatten()
        .collect()
}

pub fn parse_coverage_sets_from_sections(bytes: &[u8], footer: &CoveFooter) -> Vec<CoverageSetV2> {
    footer
        .sections
        .iter()
        .filter(|entry| entry.section_kind == SectionKind::CoverageSet as u16)
        .filter_map(|entry| compression::section_payload(bytes, entry).ok())
        .filter_map(|payload| CoverageSetV2::parse(&payload).ok())
        .collect()
}

pub fn parse_coverage_proofs_from_sections(
    bytes: &[u8],
    footer: &CoveFooter,
) -> Vec<CoverageProofRecordV2> {
    footer
        .sections
        .iter()
        .filter(|entry| entry.section_kind == SectionKind::CoverageProofRecord as u16)
        .filter_map(|entry| compression::section_payload(bytes, entry).ok())
        .filter_map(|payload| CoverageProofRecordV2::parse_many(&payload).ok())
        .flatten()
        .collect()
}

pub fn parse_coverage_plan_candidates_from_sections(
    bytes: &[u8],
    footer: &CoveFooter,
) -> Vec<CoveragePlanCandidateV2> {
    footer
        .sections
        .iter()
        .filter(|entry| entry.section_kind == SectionKind::CoveragePlanCandidate as u16)
        .filter_map(|entry| compression::section_payload(bytes, entry).ok())
        .filter_map(|payload| CoveragePlanCandidateV2::parse_many(&payload).ok())
        .flatten()
        .collect()
}

pub fn parse_predicate_forms_from_sections(
    bytes: &[u8],
    footer: &CoveFooter,
) -> Vec<PredicateNormalFormV2> {
    footer
        .sections
        .iter()
        .filter(|entry| entry.section_kind == SectionKind::PredicateNormalForm as u16)
        .filter_map(|entry| compression::section_payload(bytes, entry).ok())
        .filter_map(|payload| PredicateNormalFormV2::parse_many(&payload).ok())
        .flatten()
        .collect()
}

pub fn parse_predicate_forms_with_payloads_from_sections(
    bytes: &[u8],
    footer: &CoveFooter,
) -> Vec<PredicateNormalFormWithPayloadV2> {
    footer
        .sections
        .iter()
        .filter(|entry| entry.section_kind == SectionKind::PredicateNormalForm as u16)
        .filter_map(|entry| compression::section_payload(bytes, entry).ok())
        .filter_map(|payload| PredicateNormalFormWithPayloadV2::parse_many(&payload).ok())
        .flatten()
        .collect()
}

fn parse_exact_sets(bytes: &[u8], footer: &CoveFooter) -> Vec<ExactSetIndex> {
    footer
        .sections
        .iter()
        .filter(|entry| entry.section_kind == SectionKind::ExactSetIndex as u16)
        .filter_map(|entry| compression::section_payload(bytes, entry).ok())
        .filter_map(|payload| ExactSetIndex::parse(&payload).ok())
        .collect()
}

fn parse_blooms(bytes: &[u8], footer: &CoveFooter) -> Vec<BloomFilterIndex> {
    footer
        .sections
        .iter()
        .filter(|entry| entry.section_kind == SectionKind::BloomIndex as u16)
        .filter_map(|entry| compression::section_payload(bytes, entry).ok())
        .filter_map(|payload| BloomFilterIndex::parse(&payload).ok())
        .collect()
}

fn parse_lookups(bytes: &[u8], footer: &CoveFooter) -> Vec<LookupIndex> {
    footer
        .sections
        .iter()
        .filter(|entry| entry.section_kind == SectionKind::LookupIndex as u16)
        .filter_map(|entry| compression::section_payload(bytes, entry).ok())
        .filter_map(|payload| LookupIndex::parse(&payload).ok())
        .collect()
}

fn parse_inverted(bytes: &[u8], footer: &CoveFooter) -> Vec<InvertedMorselIndex> {
    footer
        .sections
        .iter()
        .filter(|entry| entry.section_kind == SectionKind::InvertedMorselIndex as u16)
        .filter_map(|entry| compression::section_payload(bytes, entry).ok())
        .filter_map(|payload| InvertedMorselIndex::parse(&payload).ok())
        .collect()
}

fn parse_aggregates(bytes: &[u8], footer: &CoveFooter) -> Vec<AggregateSynopsis> {
    footer
        .sections
        .iter()
        .filter(|entry| entry.section_kind == SectionKind::AggregateSynopsis as u16)
        .filter_map(|entry| compression::section_payload(bytes, entry).ok())
        .filter_map(|payload| AggregateSynopsis::parse(&payload).ok())
        .collect()
}

fn parse_composites(bytes: &[u8], footer: &CoveFooter) -> Vec<CompositeIndex> {
    footer
        .sections
        .iter()
        .filter(|entry| entry.section_kind == SectionKind::CompositeZoneIndex as u16)
        .filter_map(|entry| compression::section_payload(bytes, entry).ok())
        .filter_map(|payload| CompositeIndex::parse(&payload).ok())
        .collect()
}

fn parse_topn(bytes: &[u8], footer: &CoveFooter) -> Vec<TopNSummary> {
    footer
        .sections
        .iter()
        .filter(|entry| entry.section_kind == SectionKind::TopNZoneSummary as u16)
        .filter_map(|entry| compression::section_payload(bytes, entry).ok())
        .filter_map(|payload| TopNSummary::parse(&payload).ok())
        .collect()
}
