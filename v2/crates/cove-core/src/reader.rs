//! Cove Format (COVE) v2.0 — reference reader and structural validator.

use std::{collections::BTreeSet, fs, path::Path};

use crate::{
    checksum, compression,
    constants::{
        PrimaryProfile, SectionKind, FEATURE_ARCHIVE_PROFILE, FEATURE_CODEC_EXTENSION_REGISTRY,
        FEATURE_CODEC_LZ4, FEATURE_CODEC_ZSTD, FEATURE_COVERAGE_METADATA, FEATURE_ENGINE_PROFILE,
        FEATURE_EXTENDED_FEATURE_SET, FEATURE_HARBOR_PROFILE, FEATURE_LAYOUT_PLAN,
        FEATURE_OBJECT_PROFILE, FEATURE_RUNTIME_COMPATIBILITY_HINTS,
        FEATURE_SECONDARY_INDEX_ARTIFACT, FEATURE_SEMANTIC_MAP, FEATURE_TABLE_PROFILE,
    },
    feature_binding::SectionFeatureBindingSectionV2,
    feature_scope::{ExtendedFeatureSetV2, FeatureScopeTable, ProfileCapabilityMatrixV2},
    footer::CoveFooter,
    header::{CoveHeaderV1, HEADER_SIZE},
    postscript::CovePostscriptV1,
    registry, CoveError,
};

#[path = "reader/digest_verification.rs"]
mod digest_verification;
#[path = "reader/reports.rs"]
mod reports;

pub use reports::{
    validate_bytes_for_feature_use, validate_bytes_with_options, IgnoredOptionalSection,
    OptionalPushdownPolicy, ValidationOptions, ValidationReport, ValidationStage,
    ValidationStageReport, ValidationStageStatus,
};
#[path = "reader/bootstrap.rs"]
mod bootstrap;
use bootstrap::validate_bytes_with_optional_pushdown_policy;
#[path = "reader/profile_validators.rs"]
mod profile_validators;
#[path = "reader/shared_semantics.rs"]
mod shared_semantics;

/// Parsed and structurally validated COVE file.
#[derive(Debug, Clone)]
pub struct ValidatedCoveFile {
    pub header: CoveHeaderV1,
    pub postscript: CovePostscriptV1,
    pub footer: CoveFooter,
}

#[derive(Debug, Clone)]
struct ScopedFeatureMetadata {
    #[allow(dead_code)]
    extended: Option<ExtendedFeatureSetV2>,
    #[allow(dead_code)]
    profile_matrix: Option<ProfileCapabilityMatrixV2>,
    #[allow(dead_code)]
    section_bindings: Vec<SectionFeatureBindingSectionV2>,
    scope_table: FeatureScopeTable,
}

/// Read a complete COVE file and validate its COVE-Core structure.
pub fn read_file(path: &Path) -> Result<ValidatedCoveFile, CoveError> {
    let data = fs::read(path)?;
    validate_bytes(&data)
}

pub fn validate_bytes(data: &[u8]) -> Result<ValidatedCoveFile, CoveError> {
    let (validated, _) =
        validate_bytes_with_optional_pushdown_policy(data, OptionalPushdownPolicy::Strict)?;
    Ok(validated)
}

fn validate_required_feature_implementation(header: &CoveHeaderV1) -> Result<(), CoveError> {
    let mut unsupported_required = 0u64;
    if header.required_features & FEATURE_CODEC_LZ4 != 0 && !cfg!(feature = "compression-lz4") {
        unsupported_required |= FEATURE_CODEC_LZ4;
    }
    if header.required_features & FEATURE_CODEC_ZSTD != 0 && !cfg!(feature = "compression-zstd") {
        unsupported_required |= FEATURE_CODEC_ZSTD;
    }
    if unsupported_required != 0 {
        return Err(CoveError::UnsupportedEncoding(format!(
            "required codec feature bits are unsupported by this build: 0x{unsupported_required:016x}"
        )));
    }
    Ok(())
}

fn validate_sections(
    data: &[u8],
    footer_start: usize,
    footer: &mut CoveFooter,
    header: &CoveHeaderV1,
    optional_pushdown_policy: OptionalPushdownPolicy,
) -> Result<Vec<IgnoredOptionalSection>, CoveError> {
    let mut ranges: Vec<(u64, u64, u32)> = Vec::new();
    let mut last_section_id: Option<u32> = None;
    let mut ignored_optional_sections = Vec::new();

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

        validate_section_profile(entry.section_kind, entry.profile)?;
        validate_section_profile_feature_bit(
            entry.profile,
            header.required_features | header.optional_features,
        )?;
        validate_section_required_feature_advertisement(header, entry)?;
        validate_codec_feature_advertisement(entry.compression, header, entry)?;

        let section_end = entry.end_offset()?;
        if entry.offset < HEADER_SIZE as u64 || section_end > footer_start as u64 {
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

        let section_bytes = &data[entry.offset as usize..section_end as usize];
        if checksum::crc32c(section_bytes) != entry.crc32c {
            if optional_pushdown_policy == OptionalPushdownPolicy::FailOpen
                && profile_validators::is_optional_pushdown_entry(entry)
            {
                ignored_optional_sections.push(IgnoredOptionalSection {
                    section_id: entry.section_id,
                    section_kind: entry.section_kind,
                    reason: CoveError::ChecksumMismatch.to_string(),
                });
                continue;
            }
            return Err(CoveError::ChecksumMismatch);
        }
    }

    if !ignored_optional_sections.is_empty() {
        let ignored_ids = ignored_optional_sections
            .iter()
            .map(|section| section.section_id)
            .collect::<BTreeSet<_>>();
        footer
            .sections
            .retain(|entry| !ignored_ids.contains(&entry.section_id));
        footer.header.section_count = footer.sections.len() as u32;
    }

    Ok(ignored_optional_sections)
}

pub fn feature_scope_table_for(
    data: &[u8],
    validated: &ValidatedCoveFile,
) -> Result<FeatureScopeTable, CoveError> {
    Ok(parse_scoped_feature_metadata(data, &validated.header, &validated.footer)?.scope_table)
}

fn validate_scoped_feature_metadata(
    data: &[u8],
    header: &CoveHeaderV1,
    footer: &CoveFooter,
) -> Result<(), CoveError> {
    let metadata = parse_scoped_feature_metadata(data, header, footer)?;
    metadata.scope_table.reject_file_required_unknowns()?;
    Ok(())
}

fn parse_scoped_feature_metadata(
    data: &[u8],
    header: &CoveHeaderV1,
    footer: &CoveFooter,
) -> Result<ScopedFeatureMetadata, CoveError> {
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

    let extended = extended_entry
        .map(|entry| {
            let payload = compression::section_payload(data, entry)?;
            let set = ExtendedFeatureSetV2::parse(&payload)?;
            set.validate_against_low_words(header.required_features, header.optional_features)?;
            Ok::<ExtendedFeatureSetV2, CoveError>(set)
        })
        .transpose()?;

    let profile_matrix = profile_matrix_entry
        .map(|entry| {
            let payload = compression::section_payload(data, entry)?;
            ProfileCapabilityMatrixV2::parse(&payload)
        })
        .transpose()?;

    let mut section_bindings = Vec::<SectionFeatureBindingSectionV2>::new();
    for entry in footer
        .sections
        .iter()
        .filter(|entry| entry.section_kind == SectionKind::SectionFeatureBinding as u16)
    {
        let payload = compression::section_payload(data, entry)?;
        let parsed = SectionFeatureBindingSectionV2::parse(&payload)?;
        let Some(extended) = extended.as_ref() else {
            return Err(CoveError::BadSection(
                "SECTION_FEATURE_BINDING requires EXTENDED_FEATURE_SET".into(),
            ));
        };
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

    let scope_table = FeatureScopeTable::build_many(
        header,
        footer,
        extended.as_ref(),
        profile_matrix.as_ref(),
        &section_bindings,
    )?;
    Ok(ScopedFeatureMetadata {
        extended,
        profile_matrix,
        section_bindings,
        scope_table,
    })
}

fn resolve_header_section_id<'a>(
    footer: &'a CoveFooter,
    section_id: u32,
    expected_kind: SectionKind,
    field_name: &str,
) -> Result<Option<&'a crate::footer::CoveSectionEntryV1>, CoveError> {
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

fn validate_footer_codec_feature_advertisement(
    postscript: &CovePostscriptV1,
) -> Result<(), CoveError> {
    let advertised = postscript.required_features | postscript.optional_features;
    match postscript.footer.compression {
        1 if advertised & FEATURE_CODEC_LZ4 == 0 => Err(CoveError::BadSection(
            "footer uses LZ4 compression but codec feature bit is not advertised".into(),
        )),
        2 if advertised & FEATURE_CODEC_ZSTD == 0 => Err(CoveError::BadSection(
            "footer uses ZSTD compression but codec feature bit is not advertised".into(),
        )),
        _ => Ok(()),
    }
}

fn validate_section_required_feature_advertisement(
    header: &CoveHeaderV1,
    entry: &crate::footer::CoveSectionEntryV1,
) -> Result<(), CoveError> {
    let Some(info) =
        registry::section_info(SectionKind::from_u16(entry.section_kind).ok_or_else(|| {
            CoveError::BadSection(format!("unknown section_kind {}", entry.section_kind))
        })?)
    else {
        return Ok(());
    };
    let Some(required_feature) = info.required_feature else {
        return Ok(());
    };
    let file_advertised = header.required_features | header.optional_features;
    if file_advertised & required_feature == 0 {
        return Err(CoveError::BadSection(format!(
            "section {} of kind {} requires missing feature bit 0x{required_feature:016x}",
            entry.section_id, info.wire_name
        )));
    }
    Ok(())
}

fn validate_section_profile_feature_bit(profile: u8, file_features: u64) -> Result<(), CoveError> {
    let required_profile_bit = match profile {
        0 => return Ok(()),
        1 => FEATURE_OBJECT_PROFILE,
        2 => FEATURE_TABLE_PROFILE,
        3 => FEATURE_ARCHIVE_PROFILE,
        4 => FEATURE_ENGINE_PROFILE,
        5 => FEATURE_HARBOR_PROFILE,
        6 => FEATURE_SEMANTIC_MAP,
        7 => FEATURE_CODEC_EXTENSION_REGISTRY,
        8 => FEATURE_LAYOUT_PLAN,
        9 => FEATURE_RUNTIME_COMPATIBILITY_HINTS,
        10 => FEATURE_COVERAGE_METADATA,
        11 => FEATURE_SECONDARY_INDEX_ARTIFACT,
        _ => {
            return Err(CoveError::BadSection(format!(
                "unknown profile {profile} in section directory"
            )));
        }
    };
    if file_features & required_profile_bit == 0 {
        return Err(CoveError::BadSection(format!(
            "section profile {profile} requires missing file feature bit 0x{required_profile_bit:016x}"
        )));
    }
    Ok(())
}

fn validate_codec_feature_advertisement(
    compression: u8,
    header: &CoveHeaderV1,
    entry: &crate::footer::CoveSectionEntryV1,
) -> Result<(), CoveError> {
    let advertised = header.required_features | header.optional_features;
    let section_advertised = entry.required_features | entry.optional_features;
    match compression {
        1 if advertised & FEATURE_CODEC_LZ4 == 0 || section_advertised & FEATURE_CODEC_LZ4 == 0 => {
            return Err(CoveError::BadSection(format!(
                "section {} uses LZ4 compression but codec feature bit is not advertised",
                entry.section_id
            )));
        }
        2 if advertised & FEATURE_CODEC_ZSTD == 0
            || section_advertised & FEATURE_CODEC_ZSTD == 0 =>
        {
            return Err(CoveError::BadSection(format!(
                "section {} uses ZSTD compression but codec feature bit is not advertised",
                entry.section_id
            )));
        }
        _ => {}
    }
    Ok(())
}

fn validate_primary_profile_features(header: &CoveHeaderV1) -> Result<(), CoveError> {
    let profile = PrimaryProfile::from_u8(header.primary_profile)
        .ok_or_else(|| CoveError::BadSection("unknown primary profile".to_string()))?;

    let required_bit = match profile {
        PrimaryProfile::Mixed => return Ok(()),
        PrimaryProfile::ObjectTemporal => FEATURE_OBJECT_PROFILE,
        PrimaryProfile::TableScan => FEATURE_TABLE_PROFILE,
        PrimaryProfile::ArchiveAcceleration => FEATURE_ARCHIVE_PROFILE,
        PrimaryProfile::EngineExecution => FEATURE_ENGINE_PROFILE,
        PrimaryProfile::HarborExecution => FEATURE_HARBOR_PROFILE,
        PrimaryProfile::SemanticMapping => FEATURE_SEMANTIC_MAP,
        PrimaryProfile::CodecExtension => FEATURE_CODEC_EXTENSION_REGISTRY,
        PrimaryProfile::LayoutPlanning => FEATURE_LAYOUT_PLAN,
        PrimaryProfile::RuntimeCompatibility => FEATURE_RUNTIME_COMPATIBILITY_HINTS,
        PrimaryProfile::CoverageMetadata => FEATURE_COVERAGE_METADATA,
        PrimaryProfile::SecondaryIndex => FEATURE_SECONDARY_INDEX_ARTIFACT,
    };

    if header.required_features & required_bit == 0 {
        return Err(CoveError::BadSection(format!(
            "primary_profile {:?} requires feature bit 0x{required_bit:016x}",
            profile
        )));
    }
    Ok(())
}

fn validate_section_profile(section_kind: u16, profile: u8) -> Result<(), CoveError> {
    let section = SectionKind::from_u16(section_kind)
        .ok_or_else(|| CoveError::BadSection(format!("unknown section_kind {section_kind}")));
    let allowed: &[u8] = match section? {
        // shared (profile 0)
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
        | SectionKind::FastMetadataIndex
        | SectionKind::SectionFeatureBinding
        | SectionKind::VendorExtension => &[0],
        // COVE-T only (profile 2)
        SectionKind::TableCatalog
        | SectionKind::NestedSchema
        | SectionKind::TableSegmentIndex
        | SectionKind::TableSegmentData
        | SectionKind::ColumnDomain
        | SectionKind::ZoneStats => &[2],
        // COVE-T/COVE-A (profiles 2 or 3)
        SectionKind::ExactSetIndex
        | SectionKind::BloomIndex
        | SectionKind::InvertedMorselIndex
        | SectionKind::KernelCapabilities => &[2, 3],
        // COVE-A only (profile 3)
        SectionKind::LookupIndex
        | SectionKind::AggregateSynopsis
        | SectionKind::CompositeZoneIndex
        | SectionKind::TopNZoneSummary => &[3],
        // COVE-CX (profile 7)
        SectionKind::CodecExtensionRegistry => &[7],
        // COVE-L (profile 8), with zero-copy also allowed as shared metadata.
        SectionKind::LayoutPlan
        | SectionKind::ScanSplitIndex
        | SectionKind::PageClusterDirectory => &[8],
        SectionKind::ZeroCopyBufferMap => &[0, 8],
        // COVE-COVERAGE (profile 10)
        SectionKind::CoverageProviderRegistry
        | SectionKind::CoverageSet
        | SectionKind::CoveragePlanCandidate
        | SectionKind::PredicateNormalForm
        | SectionKind::CoverageProofRecord => &[10],
        // COVE-I/COVE-A
        SectionKind::IndexOnlyCapability => &[3, 11],
        // COVE-E (profile 4)
        SectionKind::EngineProfileRegistry
        | SectionKind::ExecutionCodeDescriptor
        | SectionKind::ExecutionScopeDescriptor
        | SectionKind::CodeSpaceDescriptor
        | SectionKind::EngineMountPolicy => &[4],
        // COVE-O (profile 1)
        SectionKind::ObjectTypeCatalog
        | SectionKind::TemporalSegmentIndex
        | SectionKind::TemporalSegmentData
        | SectionKind::TemporalBloomIndex
        | SectionKind::TrustManifest => &[1],
        // COVE-H (profile 5)
        SectionKind::HarborMountHints => &[5],
        // COVE-MAP (profile 6)
        SectionKind::MapSourceCatalog
        | SectionKind::MapFunctionRegistry
        | SectionKind::MapIdentityRuleCatalog
        | SectionKind::MapRowSemanticsCatalog
        | SectionKind::MapAssertionLog
        | SectionKind::MapIdentityEquivalenceIndex
        | SectionKind::MapEvidenceIndex
        | SectionKind::MapConversionReport
        | SectionKind::MapProjectionCatalog => &[6],
    };
    if !allowed.contains(&profile) {
        return Err(CoveError::BadSection(format!(
            "section_kind {section_kind} must use one of profiles {allowed:?}, got {profile}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "compression-lz4")]
    use crate::{constants::CompressionCodec, footer::CoveFooter};
    use crate::{
        constants::{
            SectionKind, FEATURE_BLOOM_FILTERS, FEATURE_CODEC_LZ4, FEATURE_ENGINE_PROFILE,
            FEATURE_EXTENDED_FEATURE_SET, FEATURE_EXTENSION_REGISTRY, FEATURE_FILE_DICTIONARY,
            FEATURE_HARBOR_PROFILE, FEATURE_OBJECT_PROFILE, FEATURE_TABLE_PROFILE,
        },
        digest::{DigestEntry, DigestManifest, DigestScope, DigestTargetKind},
        extensions::{ExtensionKind, ExtensionRegistry, ExtensionRegistryEntry},
        feature_binding::{FeatureScopeV2, OperationKindV2},
        feature_scope::{
            ExtendedFeatureSetHeaderV2, ExtendedFeatureSetV2, FeatureTargetRefV2,
            FeatureUseRequestV2, ProfileCapabilityEntryV2, ProfileCapabilityMatrixHeaderV2,
            ProfileCapabilityMatrixV2,
        },
        postscript::POSTSCRIPT_TOTAL_SIZE,
        segment::TableSegmentPayloadV1,
        table::{ColumnEntry, TableCatalog, TableEntry},
        writer::{MinimalCoveWriter, ScanProfileCoveWriter, ScanSegment, SectionPayload},
    };

    fn optional_bloom_fixture(data: Vec<u8>) -> Vec<u8> {
        let mut writer = MinimalCoveWriter::new();
        writer.required_features = FEATURE_TABLE_PROFILE;
        writer.optional_features = FEATURE_BLOOM_FILTERS;
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::BloomIndex as u16,
            profile: 2,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: 0,
            optional_features: FEATURE_BLOOM_FILTERS,
            data,
        });
        writer.write().unwrap()
    }

    fn corrupt_first_section_byte(bytes: &mut [u8]) {
        let validated = validate_bytes(bytes).unwrap();
        let entry = validated.footer.sections.first().unwrap();
        bytes[entry.offset as usize] ^= 0x01;
    }

    fn rewrite_postscript(bytes: &mut [u8], postscript: CovePostscriptV1) {
        let tail_start = bytes.len() - POSTSCRIPT_TOTAL_SIZE;
        bytes[tail_start..].copy_from_slice(&postscript.serialize_tail());
    }

    fn required_unknown_extension_registry_payload() -> Vec<u8> {
        ExtensionRegistry {
            flags: 0,
            entries: vec![ExtensionRegistryEntry {
                extension_id: 1,
                namespace: "org".into(),
                name: "test".into(),
                version_major: 1,
                version_minor: 0,
                extension_kind: ExtensionKind::VendorMetadata,
                required_feature_bit: 0x0020_0000,
                optional_feature_bit: 0,
                fallback_kind: 0,
                fallback_ref: 0,
                payload_ref: 0,
                checksum: 0,
            }],
        }
        .serialize()
        .unwrap()
    }

    const UNKNOWN_EXTENDED_FEATURE: u64 = 0x01;

    fn scoped_feature_entry(
        scope: FeatureScopeV2,
        profile: u8,
        operation_kind: OperationKindV2,
        section_id: u32,
        target_local_ref: u64,
        required_mask: u64,
        optional_mask: u64,
    ) -> ProfileCapabilityEntryV2 {
        ProfileCapabilityEntryV2 {
            profile,
            scope,
            operation_kind,
            global_feature_word_index: 1,
            required_mask,
            optional_mask,
            section_id,
            target_local_ref,
            flags: 0,
            reserved: 0,
            checksum: 0,
        }
    }

    fn scoped_feature_file(
        matrix_entries: Vec<ProfileCapabilityEntryV2>,
        extended_required_word_1: u64,
        extended_optional_word_1: u64,
    ) -> Vec<u8> {
        let required_features = FEATURE_TABLE_PROFILE | FEATURE_EXTENDED_FEATURE_SET;
        let extended = ExtendedFeatureSetV2 {
            header: ExtendedFeatureSetHeaderV2 {
                word_count: 2,
                required_word_count: 2,
                optional_word_count: 2,
                flags: 0,
                checksum: 0,
            },
            required_feature_words: vec![required_features, extended_required_word_1],
            optional_feature_words: vec![0, extended_optional_word_1],
        }
        .serialize()
        .unwrap();
        let matrix = ProfileCapabilityMatrixV2 {
            header: ProfileCapabilityMatrixHeaderV2 {
                magic: *b"PCM2",
                version_major: 2,
                header_len: ProfileCapabilityMatrixHeaderV2::LEN as u16,
                entry_len: ProfileCapabilityEntryV2::LEN as u16,
                reserved: 0,
                entry_count: matrix_entries.len() as u32,
                flags: 0,
                entries_offset: ProfileCapabilityMatrixHeaderV2::LEN as u64,
                entries_length: (matrix_entries.len() * ProfileCapabilityEntryV2::LEN) as u64,
                checksum: 0,
            },
            entries: matrix_entries,
        }
        .serialize()
        .unwrap();
        let mut writer = MinimalCoveWriter::new();
        writer.required_features = required_features;
        writer.sections.extend([
            SectionPayload {
                section_kind: SectionKind::ExtendedFeatureSet as u16,
                profile: 0,
                flags: 0,
                item_count: 0,
                row_count: 0,
                compression: 0,
                alignment_log2: 0,
                required_features: 0,
                optional_features: 0,
                data: extended,
            },
            SectionPayload {
                section_kind: SectionKind::ProfileCapabilityMatrix as u16,
                profile: 0,
                flags: 0,
                item_count: 0,
                row_count: 0,
                compression: 0,
                alignment_log2: 0,
                required_features: 0,
                optional_features: 0,
                data: matrix,
            },
            SectionPayload {
                section_kind: SectionKind::VendorExtension as u16,
                profile: 0,
                flags: 0,
                item_count: 0,
                row_count: 0,
                compression: 0,
                alignment_log2: 0,
                required_features: 0,
                optional_features: 0,
                data: Vec::new(),
            },
        ]);
        let mut bytes = writer.write().unwrap();
        set_header_scoped_feature_sections(&mut bytes, 1, 2);
        bytes
    }

    fn unscoped_extended_feature_file() -> Vec<u8> {
        let required_features = FEATURE_TABLE_PROFILE | FEATURE_EXTENDED_FEATURE_SET;
        let extended = ExtendedFeatureSetV2 {
            header: ExtendedFeatureSetHeaderV2 {
                word_count: 2,
                required_word_count: 2,
                optional_word_count: 1,
                flags: 0,
                checksum: 0,
            },
            required_feature_words: vec![required_features, UNKNOWN_EXTENDED_FEATURE],
            optional_feature_words: vec![0],
        }
        .serialize()
        .unwrap();
        let mut writer = MinimalCoveWriter::new();
        writer.required_features = required_features;
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::ExtendedFeatureSet as u16,
            profile: 0,
            flags: 0,
            item_count: 0,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: 0,
            optional_features: 0,
            data: extended,
        });
        let mut bytes = writer.write().unwrap();
        set_header_scoped_feature_sections(&mut bytes, 1, 0);
        bytes
    }

    fn set_header_scoped_feature_sections(
        bytes: &mut [u8],
        feature_set_section_id: u32,
        profile_capability_section_id: u32,
    ) {
        let mut header = CoveHeaderV1::parse(bytes).unwrap();
        header.feature_set_section_id = feature_set_section_id;
        header.profile_capability_section_id = profile_capability_section_id;
        bytes[..HEADER_SIZE].copy_from_slice(&header.serialize());
    }

    #[test]
    fn validates_empty_file() {
        let bytes = MinimalCoveWriter::write_empty_file().unwrap();
        let file = validate_bytes(&bytes).expect("minimal file should validate");
        assert_eq!(file.header.required_features, FEATURE_TABLE_PROFILE);
        assert_eq!(file.footer.sections.len(), 0);
    }

    #[test]
    fn rejects_section_crc_mismatch() {
        let mut writer = MinimalCoveWriter::new();
        writer.required_features = FEATURE_TABLE_PROFILE | FEATURE_FILE_DICTIONARY;
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::FileDictionaryIndex as u16,
            profile: 0,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: FEATURE_FILE_DICTIONARY,
            optional_features: 0,
            data: b"abcdef".to_vec(),
        });
        let mut bytes = writer.write().unwrap();
        bytes[HEADER_SIZE] ^= 0x01;
        assert!(matches!(
            validate_bytes(&bytes),
            Err(CoveError::ChecksumMismatch)
        ));
    }

    #[test]
    fn fail_open_ignores_optional_pushdown_crc_mismatch() {
        let mut bytes = optional_bloom_fixture(vec![0; 64]);
        corrupt_first_section_byte(&mut bytes);

        assert!(matches!(
            validate_bytes(&bytes),
            Err(CoveError::ChecksumMismatch)
        ));

        let report = validate_bytes_with_options(
            &bytes,
            ValidationOptions {
                semantic: true,
                verify_digests: false,
                allow_unknown_optional_extensions: true,
                optional_pushdown_policy: OptionalPushdownPolicy::FailOpen,
            },
        )
        .unwrap();
        assert_eq!(report.validated.footer.sections.len(), 0);
        assert_eq!(report.ignored_optional_sections.len(), 1);
        assert_eq!(
            report.ignored_optional_sections[0].section_kind,
            SectionKind::BloomIndex as u16
        );
    }

    #[test]
    fn fail_open_ignores_malformed_optional_pushdown_payload() {
        let bytes = optional_bloom_fixture(vec![0; 64]);

        assert!(matches!(
            validate_bytes_with_options(
                &bytes,
                ValidationOptions {
                    semantic: true,
                    verify_digests: false,
                    allow_unknown_optional_extensions: true,
                    optional_pushdown_policy: OptionalPushdownPolicy::Strict,
                },
            ),
            Err(CoveError::BadIndex | CoveError::ChecksumMismatch | CoveError::BufferTooShort)
        ));

        let report = validate_bytes_with_options(
            &bytes,
            ValidationOptions {
                semantic: true,
                verify_digests: false,
                allow_unknown_optional_extensions: true,
                optional_pushdown_policy: OptionalPushdownPolicy::FailOpen,
            },
        )
        .unwrap();
        assert_eq!(report.ignored_optional_sections.len(), 1);
    }

    #[test]
    fn rejects_non_utf8_metadata_written_by_external_source() {
        let mut writer = MinimalCoveWriter::new();
        writer.metadata_json = b"{}".to_vec();
        let mut bytes = writer.write().unwrap();

        let ps = CovePostscriptV1::parse_from_tail(&bytes).unwrap();
        let metadata_offset = ps.footer.offset as usize + 44;
        bytes[metadata_offset] = 0xff;

        let footer_start = ps.footer.offset as usize;
        let footer_len = ps.footer.length as usize;
        let footer_crc = checksum::crc32c(&bytes[footer_start..footer_start + footer_len]);
        let mut fixed_ps = ps;
        fixed_ps.footer.crc32c = footer_crc;
        let tail_start = bytes.len() - POSTSCRIPT_TOTAL_SIZE;
        bytes[tail_start..].copy_from_slice(&fixed_ps.serialize_tail());

        assert!(matches!(
            validate_bytes(&bytes),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn rejects_overlapping_sections() {
        let mut writer = MinimalCoveWriter::new();
        writer.required_features = FEATURE_TABLE_PROFILE | FEATURE_FILE_DICTIONARY;
        for data in [b"first".to_vec(), b"second".to_vec()] {
            writer.sections.push(SectionPayload {
                section_kind: SectionKind::FileDictionaryIndex as u16,
                profile: 0,
                flags: 0,
                item_count: 1,
                row_count: 0,
                compression: 0,
                alignment_log2: 0,
                required_features: FEATURE_FILE_DICTIONARY,
                optional_features: 0,
                data,
            });
        }

        let mut bytes = writer.write().unwrap();
        let ps = CovePostscriptV1::parse_from_tail(&bytes).unwrap();
        let footer_start = ps.footer.offset as usize;
        let entries_start = footer_start + 44;
        let first_offset = u64::from_le_bytes(
            bytes[entries_start + 8..entries_start + 16]
                .try_into()
                .unwrap(),
        );
        let second_offset_pos = entries_start + 76 + 8;
        bytes[second_offset_pos..second_offset_pos + 8]
            .copy_from_slice(&first_offset.to_le_bytes());

        let footer_len = ps.footer.length as usize;
        let footer_crc = checksum::crc32c(&bytes[footer_start..footer_start + footer_len]);
        let mut fixed_ps = ps;
        fixed_ps.footer.crc32c = footer_crc;
        let tail_start = bytes.len() - POSTSCRIPT_TOTAL_SIZE;
        bytes[tail_start..].copy_from_slice(&fixed_ps.serialize_tail());

        assert!(matches!(
            validate_bytes(&bytes),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn rejects_out_of_order_section_ids() {
        let mut writer = MinimalCoveWriter::new();
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::FileDictionaryIndex as u16,
            profile: 0,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: 0,
            optional_features: 0,
            data: b"first".to_vec(),
        });
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::FileDictionaryPayload as u16,
            profile: 0,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: 0,
            optional_features: 0,
            data: b"second".to_vec(),
        });
        let mut bytes = writer.write().unwrap();
        let ps = CovePostscriptV1::parse_from_tail(&bytes).unwrap();
        let footer_start = ps.footer.offset as usize;
        let entries_start = footer_start + 44;
        bytes[entries_start + 76..entries_start + 80].copy_from_slice(&1u32.to_le_bytes());

        let footer_len = ps.footer.length as usize;
        let footer_crc = checksum::crc32c(&bytes[footer_start..footer_start + footer_len]);
        let mut fixed_ps = ps;
        fixed_ps.footer.crc32c = footer_crc;
        let tail_start = bytes.len() - POSTSCRIPT_TOTAL_SIZE;
        bytes[tail_start..].copy_from_slice(&fixed_ps.serialize_tail());

        assert!(matches!(
            validate_bytes(&bytes),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn rejects_section_profile_feature_missing_from_header() {
        let mut writer = MinimalCoveWriter::new();
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::TableCatalog as u16,
            profile: 2,
            flags: 0,
            item_count: 0,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: 0,
            optional_features: 0,
            data: b"x".to_vec(),
        });
        writer.required_features = 0;
        let bytes = writer.write().unwrap();
        assert!(matches!(
            validate_bytes(&bytes),
            Err(CoveError::BadSection(_))
        ));
    }

    #[cfg(feature = "compression-lz4")]
    #[test]
    fn rejects_lz4_section_without_codec_feature_advertised() {
        let mut writer = MinimalCoveWriter::new();
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::FileDictionaryIndex as u16,
            profile: 0,
            flags: 0,
            item_count: 0,
            row_count: 0,
            compression: CompressionCodec::Lz4 as u8,
            alignment_log2: 0,
            required_features: 0,
            optional_features: 0,
            data: b"lz4-ish".to_vec(),
        });
        let bytes = writer.write().unwrap();
        assert!(matches!(
            validate_bytes(&bytes),
            Err(CoveError::BadSection(_))
        ));

        let mut good_writer = MinimalCoveWriter::new();
        good_writer.optional_features = FEATURE_CODEC_LZ4 | FEATURE_FILE_DICTIONARY;
        good_writer.sections.push(SectionPayload {
            section_kind: SectionKind::FileDictionaryIndex as u16,
            profile: 0,
            flags: 0,
            item_count: 0,
            row_count: 0,
            compression: CompressionCodec::Lz4 as u8,
            alignment_log2: 0,
            required_features: 0,
            optional_features: FEATURE_CODEC_LZ4 | FEATURE_FILE_DICTIONARY,
            data: b"lz4-ish".to_vec(),
        });
        let good_bytes = good_writer.write().unwrap();
        assert!(validate_bytes(&good_bytes).is_ok());
    }

    #[cfg(feature = "compression-zstd")]
    #[test]
    fn rejects_zstd_section_without_codec_feature_advertised() {
        let mut writer = MinimalCoveWriter::new();
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::FileDictionaryIndex as u16,
            profile: 0,
            flags: 0,
            item_count: 0,
            row_count: 0,
            compression: CompressionCodec::Zstd as u8,
            alignment_log2: 0,
            required_features: 0,
            optional_features: 0,
            data: b"zstd-ish".to_vec(),
        });
        assert!(matches!(
            validate_bytes(&writer.write().unwrap()),
            Err(CoveError::BadSection(_))
        ));
    }

    #[cfg(not(feature = "compression-lz4"))]
    #[test]
    fn required_lz4_feature_rejects_when_codec_disabled() {
        let mut writer = MinimalCoveWriter::new();
        writer.required_features |= FEATURE_CODEC_LZ4;
        let bytes = writer.write().unwrap();
        assert!(matches!(
            validate_bytes(&bytes),
            Err(CoveError::UnsupportedEncoding(_))
        ));
    }

    #[cfg(not(feature = "compression-zstd"))]
    #[test]
    fn required_zstd_feature_rejects_when_codec_disabled() {
        let mut writer = MinimalCoveWriter::new();
        writer.required_features |= crate::constants::FEATURE_CODEC_ZSTD;
        let bytes = writer.write().unwrap();
        assert!(matches!(
            validate_bytes(&bytes),
            Err(CoveError::UnsupportedEncoding(_))
        ));
    }

    #[test]
    fn rejects_primary_profile_missing_required_feature() {
        let mut writer = MinimalCoveWriter::new();
        writer.primary_profile = PrimaryProfile::HarborExecution as u8;
        writer.required_features = FEATURE_TABLE_PROFILE;
        assert!(matches!(
            validate_bytes(&writer.write().unwrap()),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn accepts_primary_profile_when_required_feature_present() {
        let mut writer = MinimalCoveWriter::new();
        writer.primary_profile = PrimaryProfile::HarborExecution as u8;
        writer.required_features = FEATURE_HARBOR_PROFILE;
        assert!(validate_bytes(&writer.write().unwrap()).is_ok());
    }

    #[test]
    fn rejects_table_catalog_with_wrong_profile() {
        let mut writer = MinimalCoveWriter::new();
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::TableCatalog as u16,
            profile: 0,
            flags: 0,
            item_count: 0,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: 0,
            optional_features: 0,
            data: vec![1],
        });
        assert!(matches!(
            validate_bytes(&writer.write().unwrap()),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn rejects_harbor_mount_hints_with_wrong_profile() {
        let mut writer = MinimalCoveWriter::new();
        writer.required_features = FEATURE_HARBOR_PROFILE;
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::HarborMountHints as u16,
            profile: 4,
            flags: 0,
            item_count: 0,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: 0,
            optional_features: 0,
            data: vec![1],
        });
        assert!(matches!(
            validate_bytes(&writer.write().unwrap()),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn rejects_bad_trailing_magic() {
        let mut bytes = MinimalCoveWriter::write_empty_file().unwrap();
        let len = bytes.len();
        bytes[len - 1] = b'X';
        assert!(matches!(validate_bytes(&bytes), Err(CoveError::BadMagic)));
    }

    #[test]
    fn rejects_postscript_file_length_mismatch() {
        let mut bytes = MinimalCoveWriter::write_empty_file().unwrap();
        let mut ps = CovePostscriptV1::parse_from_tail(&bytes).unwrap();
        ps.file_len += 1;
        rewrite_postscript(&mut bytes, ps);
        assert!(matches!(
            validate_bytes(&bytes),
            Err(CoveError::OffsetRange)
        ));
    }

    #[test]
    fn rejects_footer_crc_mismatch() {
        let mut writer = MinimalCoveWriter::new();
        writer.metadata_json = br#"{"k":"v"}"#.to_vec();
        let mut bytes = writer.write().unwrap();
        let ps = CovePostscriptV1::parse_from_tail(&bytes).unwrap();
        bytes[ps.footer.offset as usize] ^= 1;
        assert!(matches!(
            validate_bytes(&bytes),
            Err(CoveError::ChecksumMismatch)
        ));
    }

    #[cfg(feature = "compression-lz4")]
    #[test]
    fn accepts_lz4_compressed_footer() {
        let mut writer = MinimalCoveWriter::new();
        writer.optional_features = FEATURE_CODEC_LZ4;
        let mut bytes = writer.write().unwrap();
        let header = CoveHeaderV1::parse(&bytes).unwrap();
        let original_ps = CovePostscriptV1::parse_from_tail(&bytes).unwrap();
        let original_footer = CoveFooter::parse(
            &bytes[original_ps.footer.offset as usize
                ..original_ps.footer.end_offset().unwrap() as usize],
        )
        .unwrap();

        let footer_plain = original_footer.serialize();
        let footer_compressed = lz4_flex::block::compress(&footer_plain);
        let compressed_offset = bytes.len() - POSTSCRIPT_TOTAL_SIZE - footer_compressed.len();
        bytes.truncate(compressed_offset);
        bytes.extend_from_slice(&footer_compressed);

        let mut new_ps = original_ps;
        new_ps.file_len = (bytes.len() + POSTSCRIPT_TOTAL_SIZE) as u64;
        new_ps.footer.offset = compressed_offset as u64;
        new_ps.footer.length = footer_compressed.len() as u64;
        new_ps.footer.uncompressed_length = footer_plain.len() as u64;
        new_ps.footer.compression = CompressionCodec::Lz4 as u8;
        new_ps.footer.crc32c = checksum::crc32c(&footer_compressed);
        bytes.extend_from_slice(&new_ps.serialize_tail());

        let validated = validate_bytes(&bytes).unwrap();
        assert_eq!(validated.header.required_features, header.required_features);
        assert_eq!(validated.footer.header, original_footer.header);
    }

    #[test]
    fn accepts_required_digest_manifest_feature() {
        let mut writer = MinimalCoveWriter::new();
        writer.required_features =
            FEATURE_TABLE_PROFILE | crate::constants::FEATURE_DIGEST_MANIFEST;
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::DigestManifest as u16,
            profile: 0,
            flags: 0,
            item_count: 0,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: crate::constants::FEATURE_DIGEST_MANIFEST,
            optional_features: 0,
            data: 0u32.to_le_bytes().to_vec(),
        });
        assert!(validate_bytes(&writer.write().unwrap()).is_ok());
    }

    #[test]
    fn rejects_section_kind_missing_required_feature_bit() {
        let mut writer = MinimalCoveWriter::new();
        writer.required_features = FEATURE_TABLE_PROFILE;
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::ColumnDomain as u16,
            profile: 2,
            flags: 0,
            item_count: 0,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: 0,
            optional_features: 0,
            data: vec![0, 0, 0, 0, 0, 0],
        });
        assert!(matches!(
            validate_bytes(&writer.write().unwrap()),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn semantic_validation_parses_table_catalog_section() {
        let mut writer = MinimalCoveWriter::new();
        writer.required_features = FEATURE_TABLE_PROFILE;
        let cat = crate::table::TableCatalog {
            flags: 0,
            tables: vec![crate::table::TableEntry {
                table_id: 1,
                namespace: String::new(),
                name: "t".into(),
                row_count: 0,
                primary_sort_key_count: 0,
                clustering_key_count: 0,
                flags: 0,
                columns: vec![crate::table::ColumnEntry {
                    column_id: 7,
                    name: "c".into(),
                    logical: crate::constants::CoveLogicalType::Bool,
                    physical: crate::constants::CovePhysicalKind::Boolean,
                    nullable: false,
                    sort_order: 0,
                    collation_id: 0,
                    precision: 0,
                    scale: 0,
                    flags: 0,
                }],
            }],
        };
        let data = cat.serialize().unwrap();
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::TableCatalog as u16,
            profile: 2,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: 0,
            optional_features: 0,
            data,
        });
        assert!(validate_bytes_with_options(
            &writer.write().unwrap(),
            ValidationOptions {
                semantic: true,
                verify_digests: false,
                allow_unknown_optional_extensions: true,
                ..ValidationOptions::default()
            },
        )
        .is_ok());
    }

    #[test]
    fn cove_t_semantic_validation_rejects_page_null_count_without_bitmap() {
        let catalog = TableCatalog {
            flags: 0,
            tables: vec![TableEntry {
                table_id: 1,
                namespace: String::new(),
                name: "t".into(),
                row_count: 4,
                primary_sort_key_count: 0,
                clustering_key_count: 0,
                flags: 0,
                columns: vec![ColumnEntry {
                    column_id: 1,
                    name: "active".into(),
                    logical: crate::constants::CoveLogicalType::Bool,
                    physical: crate::constants::CovePhysicalKind::Boolean,
                    nullable: false,
                    sort_order: 0,
                    collation_id: 0,
                    precision: 0,
                    scale: 0,
                    flags: 0,
                }],
            }],
        };
        let mut writer = ScanProfileCoveWriter::new(catalog.clone());
        writer.push_segment(ScanSegment::new(1, 0, 0, 4, 1));
        let bytes = writer.write().unwrap();
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
        let entry = report
            .validated
            .footer
            .sections
            .iter()
            .find(|entry| entry.section_kind == SectionKind::TableSegmentData as u16)
            .unwrap();
        let mut segment_bytes =
            bytes[entry.offset as usize..entry.end_offset().unwrap() as usize].to_vec();
        let segment = TableSegmentPayloadV1::parse(&segment_bytes).unwrap();
        let column = &segment.columns[0];
        let page_offset = column.page_index_offset as usize;
        segment_bytes[page_offset + 12..page_offset + 16].copy_from_slice(&3u32.to_le_bytes());
        segment_bytes[page_offset + 16..page_offset + 20].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(
            TableSegmentPayloadV1::parse(&segment_bytes),
            Err(CoveError::PageCorrupt)
        );
    }

    #[test]
    fn semantic_validation_rejects_invalid_column_domain() {
        // Build a spec-§23 ColumnDomain payload whose `sorted_file_codes`
        // are not strictly ascending (duplicates), which the parser must
        // reject with COVE_E_BAD_DOMAIN.
        let mut writer = MinimalCoveWriter::new();
        writer.required_features = FEATURE_TABLE_PROFILE | crate::constants::FEATURE_COLUMN_DOMAINS;
        let header = crate::domain::ColumnDomainHeaderV1 {
            table_or_object_id: 1,
            column_or_property_id: 2,
            logical_type: 0,
            collation_id: 0,
            domain_count: 2,
            sorted_file_codes_offset: crate::domain::COLUMN_DOMAIN_HEADER_LEN as u64,
            file_code_to_rank_offset: (crate::domain::COLUMN_DOMAIN_HEADER_LEN + 2 * 4) as u64,
            flags: 0,
            checksum: 0,
        };
        let mut data = header.serialize().to_vec();
        // Two duplicate FileCodes — violates strict-ascending requirement.
        data.extend_from_slice(&5u32.to_le_bytes());
        data.extend_from_slice(&5u32.to_le_bytes());
        // Empty rank map region is permissible (zero dictionary entries).
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::ColumnDomain as u16,
            profile: 2,
            flags: 0,
            item_count: 2,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: crate::constants::FEATURE_COLUMN_DOMAINS,
            optional_features: 0,
            data,
        });
        assert_eq!(
            validate_bytes_with_options(
                &writer.write().unwrap(),
                ValidationOptions {
                    semantic: true,
                    verify_digests: false,
                    allow_unknown_optional_extensions: true,
                    ..ValidationOptions::default()
                },
            )
            .unwrap_err(),
            CoveError::BadDomain
        );
    }

    #[test]
    fn rejects_postscript_footer_range_before_header() {
        let mut bytes = MinimalCoveWriter::write_empty_file().unwrap();
        let mut ps = CovePostscriptV1::parse_from_tail(&bytes).unwrap();
        ps.footer.offset = (HEADER_SIZE - 1) as u64;
        rewrite_postscript(&mut bytes, ps);
        assert!(matches!(
            validate_bytes(&bytes),
            Err(CoveError::OffsetRange)
        ));
    }

    #[test]
    fn rejects_header_and_postscript_feature_mismatch() {
        let mut bytes = MinimalCoveWriter::write_empty_file().unwrap();
        let mut ps = CovePostscriptV1::parse_from_tail(&bytes).unwrap();
        ps.optional_features = FEATURE_CODEC_LZ4;
        rewrite_postscript(&mut bytes, ps);
        assert!(matches!(
            validate_bytes(&bytes),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn rejects_section_outside_data_region() {
        let mut writer = MinimalCoveWriter::new();
        writer.required_features = FEATURE_TABLE_PROFILE | FEATURE_FILE_DICTIONARY;
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::FileDictionaryIndex as u16,
            profile: 0,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: FEATURE_FILE_DICTIONARY,
            optional_features: 0,
            data: b"abcdef".to_vec(),
        });
        let mut bytes = writer.write().unwrap();
        let ps = CovePostscriptV1::parse_from_tail(&bytes).unwrap();
        let footer_start = ps.footer.offset as usize;
        let entries_start = footer_start + 44;
        let bad_offset = (footer_start as u64) + 1;
        bytes[entries_start + 8..entries_start + 16].copy_from_slice(&bad_offset.to_le_bytes());
        let footer_crc =
            checksum::crc32c(&bytes[footer_start..footer_start + ps.footer.length as usize]);
        let mut fixed_ps = ps;
        fixed_ps.footer.crc32c = footer_crc;
        rewrite_postscript(&mut bytes, fixed_ps);
        assert!(matches!(
            validate_bytes(&bytes),
            Err(CoveError::OffsetRange)
        ));
    }

    #[test]
    fn rejects_unknown_section_profile_in_directory() {
        let mut writer = MinimalCoveWriter::new();
        writer.required_features = FEATURE_TABLE_PROFILE | FEATURE_FILE_DICTIONARY;
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::FileDictionaryIndex as u16,
            profile: 0,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: FEATURE_FILE_DICTIONARY,
            optional_features: 0,
            data: b"abcdef".to_vec(),
        });
        let mut bytes = writer.write().unwrap();
        let ps = CovePostscriptV1::parse_from_tail(&bytes).unwrap();
        let footer_start = ps.footer.offset as usize;
        let entries_start = footer_start + 44;
        bytes[entries_start + 6] = 99;
        let footer_crc =
            checksum::crc32c(&bytes[footer_start..footer_start + ps.footer.length as usize]);
        let mut fixed_ps = ps;
        fixed_ps.footer.crc32c = footer_crc;
        rewrite_postscript(&mut bytes, fixed_ps);
        assert!(matches!(
            validate_bytes(&bytes),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn rejects_unknown_section_kind_in_directory() {
        let mut writer = MinimalCoveWriter::new();
        writer.required_features = FEATURE_TABLE_PROFILE | FEATURE_FILE_DICTIONARY;
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::FileDictionaryIndex as u16,
            profile: 0,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: FEATURE_FILE_DICTIONARY,
            optional_features: 0,
            data: b"abcdef".to_vec(),
        });
        let mut bytes = writer.write().unwrap();
        let ps = CovePostscriptV1::parse_from_tail(&bytes).unwrap();
        let footer_start = ps.footer.offset as usize;
        let entries_start = footer_start + 44;
        bytes[entries_start + 4..entries_start + 6].copy_from_slice(&999u16.to_le_bytes());
        let footer_crc =
            checksum::crc32c(&bytes[footer_start..footer_start + ps.footer.length as usize]);
        let mut fixed_ps = ps;
        fixed_ps.footer.crc32c = footer_crc;
        rewrite_postscript(&mut bytes, fixed_ps);
        assert!(matches!(
            validate_bytes(&bytes),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn enforces_profile_feature_bit_for_every_non_mixed_profile() {
        let cases: &[(u8, u16, u64)] = &[
            (
                1,
                SectionKind::ObjectTypeCatalog as u16,
                crate::constants::FEATURE_OBJECT_PROFILE,
            ),
            (2, SectionKind::TableCatalog as u16, FEATURE_TABLE_PROFILE),
            (
                3,
                SectionKind::LookupIndex as u16,
                crate::constants::FEATURE_ARCHIVE_PROFILE
                    | crate::constants::FEATURE_LOOKUP_INDEXES,
            ),
            (
                4,
                SectionKind::EngineProfileRegistry as u16,
                crate::constants::FEATURE_ENGINE_PROFILE,
            ),
            (
                5,
                SectionKind::HarborMountHints as u16,
                FEATURE_HARBOR_PROFILE,
            ),
        ];

        for (profile, kind, required_bit) in cases {
            let mut writer = MinimalCoveWriter::new();
            writer.primary_profile = 0; // mixed; avoid primary-profile gating noise
            writer.required_features = 0;
            writer.sections.push(SectionPayload {
                section_kind: *kind,
                profile: *profile,
                flags: 0,
                item_count: 0,
                row_count: 0,
                compression: 0,
                alignment_log2: 0,
                required_features: 0,
                optional_features: 0,
                data: b"x".to_vec(),
            });
            assert!(matches!(
                validate_bytes(&writer.write().unwrap()),
                Err(CoveError::BadSection(_))
            ));

            writer.required_features = *required_bit;
            assert!(
                validate_bytes(&writer.write().unwrap()).is_ok(),
                "profile {profile} should validate when feature bit is present"
            );
        }
    }

    #[test]
    fn rejects_too_short_file() {
        let bytes = vec![0u8; HEADER_SIZE + POSTSCRIPT_TOTAL_SIZE - 1];
        assert!(matches!(
            validate_bytes(&bytes),
            Err(CoveError::BufferTooShort)
        ));
    }

    #[test]
    fn rejects_invalid_footer_header_shape() {
        let mut bytes = MinimalCoveWriter::write_empty_file().unwrap();
        let ps = CovePostscriptV1::parse_from_tail(&bytes).unwrap();
        let footer_start = ps.footer.offset as usize;
        // footer.header.section_entry_len @ offset 12 in footer header
        bytes[footer_start + 12..footer_start + 14].copy_from_slice(&0u16.to_le_bytes());
        let footer_crc =
            checksum::crc32c(&bytes[footer_start..footer_start + ps.footer.length as usize]);
        let mut fixed_ps = ps;
        fixed_ps.footer.crc32c = footer_crc;
        rewrite_postscript(&mut bytes, fixed_ps);
        assert!(matches!(
            validate_bytes(&bytes),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn rejects_bad_header_after_all_tail_checks_pass() {
        let mut bytes = MinimalCoveWriter::write_empty_file().unwrap();
        // Corrupt header magic, then recompute header checksum so header parse fails on magic,
        // not checksum mismatch.
        bytes[0..4].copy_from_slice(b"BAD!");
        bytes[156..160].copy_from_slice(&[0, 0, 0, 0]);
        let crc = checksum::crc32c(&bytes[..HEADER_SIZE]);
        bytes[156..160].copy_from_slice(&crc.to_le_bytes());
        assert!(matches!(validate_bytes(&bytes), Err(CoveError::BadMagic)));
    }

    #[test]
    fn structural_validation_defaults_work() {
        let bytes = MinimalCoveWriter::write_empty_file().unwrap();
        let opts = ValidationOptions::default();
        let report = validate_bytes_with_options(&bytes, opts).expect("should validate");
        assert!(!report.semantic_checked);
        assert!(report.dict_entry_count.is_none());
    }

    #[test]
    fn unscoped_extended_required_feature_is_file_required() {
        let bytes = unscoped_extended_feature_file();
        assert!(matches!(
            validate_bytes(&bytes),
            Err(CoveError::UnknownRequiredFeature(UNKNOWN_EXTENDED_FEATURE))
        ));
    }

    #[test]
    fn scoped_extended_required_feature_does_not_reject_bootstrap() {
        let bytes = scoped_feature_file(
            vec![scoped_feature_entry(
                FeatureScopeV2::OperationRequired,
                PrimaryProfile::TableScan as u8,
                OperationKindV2::CoveragePlanning,
                0,
                u64::MAX,
                UNKNOWN_EXTENDED_FEATURE,
                0,
            )],
            UNKNOWN_EXTENDED_FEATURE,
            0,
        );
        validate_bytes(&bytes).unwrap();
    }

    #[test]
    fn operation_required_unknown_rejects_only_matching_operation() {
        let bytes = scoped_feature_file(
            vec![scoped_feature_entry(
                FeatureScopeV2::OperationRequired,
                PrimaryProfile::TableScan as u8,
                OperationKindV2::CoveragePlanning,
                0,
                u64::MAX,
                UNKNOWN_EXTENDED_FEATURE,
                0,
            )],
            UNKNOWN_EXTENDED_FEATURE,
            0,
        );
        validate_bytes_for_feature_use(
            &bytes,
            ValidationOptions::default(),
            FeatureUseRequestV2::new()
                .with_profile(PrimaryProfile::TableScan as u8)
                .with_operation(OperationKindV2::OrdinaryTableScan),
        )
        .unwrap();
        assert!(matches!(
            validate_bytes_for_feature_use(
                &bytes,
                ValidationOptions::default(),
                FeatureUseRequestV2::new()
                    .with_profile(PrimaryProfile::TableScan as u8)
                    .with_operation(OperationKindV2::CoveragePlanning),
            ),
            Err(CoveError::UnknownRequiredFeature(UNKNOWN_EXTENDED_FEATURE))
        ));
    }

    #[test]
    fn profile_required_unknown_rejects_only_matching_profile() {
        let bytes = scoped_feature_file(
            vec![scoped_feature_entry(
                FeatureScopeV2::ProfileRequired,
                PrimaryProfile::SemanticMapping as u8,
                OperationKindV2::None,
                0,
                u64::MAX,
                UNKNOWN_EXTENDED_FEATURE,
                0,
            )],
            UNKNOWN_EXTENDED_FEATURE,
            0,
        );
        validate_bytes_for_feature_use(
            &bytes,
            ValidationOptions::default(),
            FeatureUseRequestV2::new().with_profile(PrimaryProfile::TableScan as u8),
        )
        .unwrap();
        assert!(matches!(
            validate_bytes_for_feature_use(
                &bytes,
                ValidationOptions::default(),
                FeatureUseRequestV2::new().with_profile(PrimaryProfile::SemanticMapping as u8),
            ),
            Err(CoveError::UnknownRequiredFeature(UNKNOWN_EXTENDED_FEATURE))
        ));
    }

    #[test]
    fn section_required_unknown_rejects_only_requested_section() {
        let bytes = scoped_feature_file(
            vec![scoped_feature_entry(
                FeatureScopeV2::SectionRequired,
                PrimaryProfile::TableScan as u8,
                OperationKindV2::None,
                3,
                u64::MAX,
                UNKNOWN_EXTENDED_FEATURE,
                0,
            )],
            UNKNOWN_EXTENDED_FEATURE,
            0,
        );
        validate_bytes_for_feature_use(
            &bytes,
            ValidationOptions::default(),
            FeatureUseRequestV2::new().with_section(99),
        )
        .unwrap();
        assert!(matches!(
            validate_bytes_for_feature_use(
                &bytes,
                ValidationOptions::default(),
                FeatureUseRequestV2::new().with_section(3),
            ),
            Err(CoveError::UnknownRequiredFeature(UNKNOWN_EXTENDED_FEATURE))
        ));
    }

    #[test]
    fn page_required_unknown_rejects_only_exact_page_ref() {
        let target = FeatureTargetRefV2::cove_t_column_page(3, 11, 12);
        let bytes = scoped_feature_file(
            vec![scoped_feature_entry(
                FeatureScopeV2::PageRequired,
                PrimaryProfile::TableScan as u8,
                OperationKindV2::None,
                target.section_id,
                target.target_local_ref,
                UNKNOWN_EXTENDED_FEATURE,
                0,
            )],
            UNKNOWN_EXTENDED_FEATURE,
            0,
        );
        validate_bytes_for_feature_use(
            &bytes,
            ValidationOptions::default(),
            FeatureUseRequestV2::new().with_cove_t_column_page(3, 11, 13),
        )
        .unwrap();
        assert!(matches!(
            validate_bytes_for_feature_use(
                &bytes,
                ValidationOptions::default(),
                FeatureUseRequestV2::new().with_cove_t_column_page(3, 11, 12),
            ),
            Err(CoveError::UnknownRequiredFeature(UNKNOWN_EXTENDED_FEATURE))
        ));
    }

    #[test]
    fn advisory_optional_feature_never_rejects() {
        let bytes = scoped_feature_file(
            vec![scoped_feature_entry(
                FeatureScopeV2::AdvisoryOnly,
                PrimaryProfile::TableScan as u8,
                OperationKindV2::None,
                0,
                u64::MAX,
                0,
                UNKNOWN_EXTENDED_FEATURE,
            )],
            0,
            UNKNOWN_EXTENDED_FEATURE,
        );
        validate_bytes_for_feature_use(
            &bytes,
            ValidationOptions::default(),
            FeatureUseRequestV2::new()
                .with_profile(PrimaryProfile::TableScan as u8)
                .with_operation(OperationKindV2::CoveragePlanning)
                .with_section(3)
                .with_cove_t_column_page(3, 11, 12),
        )
        .unwrap();
    }

    #[test]
    fn malformed_page_required_scope_rejects_bootstrap() {
        let bytes = scoped_feature_file(
            vec![scoped_feature_entry(
                FeatureScopeV2::PageRequired,
                PrimaryProfile::TableScan as u8,
                OperationKindV2::None,
                0,
                u64::MAX,
                UNKNOWN_EXTENDED_FEATURE,
                0,
            )],
            UNKNOWN_EXTENDED_FEATURE,
            0,
        );
        assert!(matches!(
            validate_bytes(&bytes),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn semantic_validation_on_empty_file_no_dict() {
        let bytes = MinimalCoveWriter::write_empty_file().unwrap();
        let opts = ValidationOptions {
            semantic: true,
            verify_digests: false,
            allow_unknown_optional_extensions: true,
            ..ValidationOptions::default()
        };
        let report = validate_bytes_with_options(&bytes, opts).expect("should validate");
        assert!(report.semantic_checked);
        assert!(report.dict_entry_count.is_none());
        assert_eq!(
            report
                .stages
                .iter()
                .find(|s| s.stage == ValidationStage::CoveTable)
                .unwrap()
                .status,
            ValidationStageStatus::Checked
        );
    }

    fn invalid_execution_descriptor_payload() -> Vec<u8> {
        let mut bytes = crate::profile::cove_e::ExecutionCodeDescriptorV1 {
            descriptor_id: 1,
            code_kind: crate::profile::cove_e::ExecutionCodeKind::DictionaryKey,
            code_width_bits: 32,
            byte_order: 0,
            lifetime: crate::profile::cove_e::ExecutionCodeLifetime::Scan,
            comparison_scope: crate::profile::cove_e::ExecutionCodeComparisonScope::File,
            canonicality: crate::profile::cove_e::ExecutionCodeCanonicality::Transient,
            null_code_policy: crate::profile::cove_e::NullCodePolicy::NullBitmapOnly,
            flags: 0,
            scope_ref: 0,
            code_space_ref: 0,
            checksum: 0,
        }
        .serialize()
        .to_vec();
        bytes[4] = 42;
        bytes[24..28].fill(0);
        let crc = checksum::crc32c(&bytes);
        bytes[24..28].copy_from_slice(&crc.to_le_bytes());
        bytes
    }

    #[test]
    fn semantic_validation_rejects_required_cove_e_profile_error() {
        let mut writer = MinimalCoveWriter::new();
        writer.primary_profile = PrimaryProfile::Mixed as u8;
        writer.required_features = FEATURE_ENGINE_PROFILE;
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::ExecutionCodeDescriptor as u16,
            profile: 4,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: FEATURE_ENGINE_PROFILE,
            optional_features: 0,
            data: invalid_execution_descriptor_payload(),
        });
        assert_eq!(
            validate_bytes_with_options(
                &writer.write().unwrap(),
                ValidationOptions {
                    semantic: true,
                    verify_digests: false,
                    allow_unknown_optional_extensions: true,
                    ..ValidationOptions::default()
                },
            )
            .unwrap_err(),
            CoveError::BadEngineProfile
        );
    }

    #[test]
    fn semantic_validation_ignores_optional_cove_e_profile_error() {
        let mut writer = MinimalCoveWriter::new();
        writer.primary_profile = PrimaryProfile::Mixed as u8;
        writer.required_features = 0;
        writer.optional_features = FEATURE_ENGINE_PROFILE;
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::ExecutionCodeDescriptor as u16,
            profile: 4,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: 0,
            optional_features: FEATURE_ENGINE_PROFILE,
            data: invalid_execution_descriptor_payload(),
        });
        let report = validate_bytes_with_options(
            &writer.write().unwrap(),
            ValidationOptions {
                semantic: true,
                verify_digests: false,
                allow_unknown_optional_extensions: true,
                ..ValidationOptions::default()
            },
        )
        .unwrap();
        assert_eq!(
            report
                .stages
                .iter()
                .find(|s| s.stage == ValidationStage::CoveEngine)
                .unwrap()
                .sections_checked,
            1
        );
    }

    fn valid_engine_profile_registry_payload(
        execution_descriptor_ref: u32,
        mount_policy_ref: u32,
    ) -> Vec<u8> {
        crate::profile::cove_e::EngineProfileRegistry {
            flags: 0,
            profiles: vec![crate::profile::cove_e::EngineProfileEntryV1 {
                profile_id: 1,
                namespace: "org.example".into(),
                profile_name: "engine-dictionary-code".into(),
                version_major: 1,
                version_minor: 0,
                required_features: 0,
                optional_features: 0,
                execution_descriptor_ref,
                mount_policy_ref,
                private_payload_ref: 0,
                checksum: 0,
            }],
        }
        .serialize()
        .unwrap()
    }

    fn valid_code_space_descriptor_payload(code_space_id: u32) -> Vec<u8> {
        crate::profile::cove_e::CodeSpaceDescriptorV1 {
            code_space_id,
            namespace: "org.example.engine".into(),
            stable_id: b"space-1".to_vec(),
            epoch: 7,
            flags: 0,
            private_payload_ref: 0,
        }
        .serialize()
        .unwrap()
    }

    fn valid_execution_descriptor_payload_with_refs(
        descriptor_id: u32,
        scope_ref: u32,
        code_space_ref: u32,
    ) -> Vec<u8> {
        crate::profile::cove_e::ExecutionCodeDescriptorV1 {
            descriptor_id,
            code_kind: crate::profile::cove_e::ExecutionCodeKind::DictionaryKey,
            code_width_bits: 32,
            byte_order: 0,
            lifetime: crate::profile::cove_e::ExecutionCodeLifetime::Scan,
            comparison_scope: crate::profile::cove_e::ExecutionCodeComparisonScope::File,
            canonicality: crate::profile::cove_e::ExecutionCodeCanonicality::Transient,
            null_code_policy: crate::profile::cove_e::NullCodePolicy::NullBitmapOnly,
            flags: 0,
            scope_ref,
            code_space_ref,
            checksum: 0,
        }
        .serialize()
        .to_vec()
    }

    fn valid_mount_policy_payload_with_refs(policy_id: u32, code_space_ref: u32) -> Vec<u8> {
        crate::profile::cove_e::EngineMountPolicyV1 {
            policy_id,
            filecode_mapping_kind: crate::profile::cove_e::FileCodeMappingKind::MapToExecutionCode,
            missing_value_policy: crate::profile::cove_e::MissingValuePolicy::DecodeValueOnly,
            stale_mapping_policy: crate::profile::cove_e::StaleMappingPolicy::IgnoreIfOptional,
            reverse_lookup_policy: crate::profile::cove_e::ReverseLookupPolicy::BuildFromDictionary,
            flags: 0,
            dictionary_digest_ref: 0,
            code_space_ref,
            cache_key_ref: 0,
            private_payload_ref: 0,
            checksum: 0,
        }
        .serialize()
        .to_vec()
    }

    #[test]
    fn semantic_validation_rejects_required_cove_e_missing_scope_reference() {
        let mut writer = MinimalCoveWriter::new();
        writer.primary_profile = PrimaryProfile::Mixed as u8;
        writer.required_features = FEATURE_ENGINE_PROFILE;
        writer.sections.extend([
            SectionPayload {
                section_kind: SectionKind::EngineProfileRegistry as u16,
                profile: 4,
                flags: 0,
                item_count: 1,
                row_count: 0,
                compression: 0,
                alignment_log2: 0,
                required_features: FEATURE_ENGINE_PROFILE,
                optional_features: 0,
                data: valid_engine_profile_registry_payload(11, 21),
            },
            SectionPayload {
                section_kind: SectionKind::ExecutionCodeDescriptor as u16,
                profile: 4,
                flags: 0,
                item_count: 1,
                row_count: 0,
                compression: 0,
                alignment_log2: 0,
                required_features: FEATURE_ENGINE_PROFILE,
                optional_features: 0,
                data: valid_execution_descriptor_payload_with_refs(11, 31, 41),
            },
            SectionPayload {
                section_kind: SectionKind::CodeSpaceDescriptor as u16,
                profile: 4,
                flags: 0,
                item_count: 1,
                row_count: 0,
                compression: 0,
                alignment_log2: 0,
                required_features: FEATURE_ENGINE_PROFILE,
                optional_features: 0,
                data: valid_code_space_descriptor_payload(41),
            },
            SectionPayload {
                section_kind: SectionKind::EngineMountPolicy as u16,
                profile: 4,
                flags: 0,
                item_count: 1,
                row_count: 0,
                compression: 0,
                alignment_log2: 0,
                required_features: FEATURE_ENGINE_PROFILE,
                optional_features: 0,
                data: valid_mount_policy_payload_with_refs(21, 41),
            },
        ]);
        assert_eq!(
            validate_bytes_with_options(
                &writer.write().unwrap(),
                ValidationOptions {
                    semantic: true,
                    verify_digests: false,
                    allow_unknown_optional_extensions: true,
                    ..ValidationOptions::default()
                },
            )
            .unwrap_err(),
            CoveError::BadEngineProfile
        );
    }

    #[test]
    fn semantic_validation_ignores_optional_cove_e_missing_scope_reference() {
        let mut writer = MinimalCoveWriter::new();
        writer.primary_profile = PrimaryProfile::Mixed as u8;
        writer.required_features = 0;
        writer.optional_features = FEATURE_ENGINE_PROFILE;
        writer.sections.extend([
            SectionPayload {
                section_kind: SectionKind::EngineProfileRegistry as u16,
                profile: 4,
                flags: 0,
                item_count: 1,
                row_count: 0,
                compression: 0,
                alignment_log2: 0,
                required_features: 0,
                optional_features: FEATURE_ENGINE_PROFILE,
                data: valid_engine_profile_registry_payload(11, 21),
            },
            SectionPayload {
                section_kind: SectionKind::ExecutionCodeDescriptor as u16,
                profile: 4,
                flags: 0,
                item_count: 1,
                row_count: 0,
                compression: 0,
                alignment_log2: 0,
                required_features: 0,
                optional_features: FEATURE_ENGINE_PROFILE,
                data: valid_execution_descriptor_payload_with_refs(11, 31, 41),
            },
            SectionPayload {
                section_kind: SectionKind::CodeSpaceDescriptor as u16,
                profile: 4,
                flags: 0,
                item_count: 1,
                row_count: 0,
                compression: 0,
                alignment_log2: 0,
                required_features: 0,
                optional_features: FEATURE_ENGINE_PROFILE,
                data: valid_code_space_descriptor_payload(41),
            },
            SectionPayload {
                section_kind: SectionKind::EngineMountPolicy as u16,
                profile: 4,
                flags: 0,
                item_count: 1,
                row_count: 0,
                compression: 0,
                alignment_log2: 0,
                required_features: 0,
                optional_features: FEATURE_ENGINE_PROFILE,
                data: valid_mount_policy_payload_with_refs(21, 41),
            },
        ]);
        let report = validate_bytes_with_options(
            &writer.write().unwrap(),
            ValidationOptions {
                semantic: true,
                verify_digests: false,
                allow_unknown_optional_extensions: true,
                ..ValidationOptions::default()
            },
        )
        .unwrap();
        assert_eq!(
            report
                .stages
                .iter()
                .find(|s| s.stage == ValidationStage::CoveEngine)
                .unwrap()
                .sections_checked,
            4
        );
    }

    #[test]
    fn semantic_validation_rejects_required_cove_o_object_catalog_error() {
        let mut writer = MinimalCoveWriter::new();
        writer.primary_profile = PrimaryProfile::ObjectTemporal as u8;
        writer.required_features = FEATURE_OBJECT_PROFILE;
        let mut catalog = crate::profile::cove_o::ObjectTypeCatalog {
            flags: 0,
            types: vec![crate::profile::cove_o::ObjectTypeEntryV1 {
                object_type_id: 1,
                type_name: "Thing".into(),
                flags: crate::profile::cove_o::OBJECT_TYPE_FLAG_ENTITY_OBJECT,
                properties: vec![crate::profile::cove_o::PropertyEntryV1 {
                    property_id: 1,
                    property_name: "bad".into(),
                    logical_type: crate::constants::CoveLogicalType::Bool,
                    physical_kind: crate::constants::CovePhysicalKind::Boolean,
                    nullable: false,
                    collation_id: 0,
                    flags: 0,
                }],
            }],
        };
        catalog.types[0].properties[0].logical_type = crate::constants::CoveLogicalType::Null;
        catalog.types[0].properties[0].physical_kind = crate::constants::CovePhysicalKind::FileCode;
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::ObjectTypeCatalog as u16,
            profile: 1,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: FEATURE_OBJECT_PROFILE,
            optional_features: 0,
            data: catalog.serialize().unwrap(),
        });
        assert!(matches!(
            validate_bytes_with_options(
                &writer.write().unwrap(),
                ValidationOptions {
                    semantic: true,
                    verify_digests: false,
                    allow_unknown_optional_extensions: true,
                    ..ValidationOptions::default()
                },
            ),
            Err(CoveError::BadSchema(_))
        ));
    }

    #[test]
    fn semantic_validation_ignores_optional_harbor_hint_error() {
        let mut writer = MinimalCoveWriter::new();
        writer.primary_profile = PrimaryProfile::Mixed as u8;
        writer.required_features = 0;
        writer.optional_features = FEATURE_HARBOR_PROFILE;
        let mut data = crate::profile::cove_h::HarborMountHintsV1 {
            harbor_profile_version_major: 1,
            harbor_profile_version_minor: 0,
            tenant_scope_ref: 1,
            code_space_ref: 2,
            lease_epoch: 3,
            dictionary_digest_ref: 0,
            catalog_digest_ref: 0,
            mount_cache_policy: 0,
            reserved: [0; 7],
            private_payload_ref: 0,
            checksum: 0,
        }
        .serialize()
        .to_vec();
        data[29] = 1;
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::HarborMountHints as u16,
            profile: 5,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: 0,
            optional_features: FEATURE_HARBOR_PROFILE,
            data,
        });
        assert!(validate_bytes_with_options(
            &writer.write().unwrap(),
            ValidationOptions {
                semantic: true,
                verify_digests: false,
                allow_unknown_optional_extensions: true,
                ..ValidationOptions::default()
            },
        )
        .is_ok());
    }

    #[test]
    fn semantic_validation_rejects_required_unknown_extension_registry() {
        let mut writer = MinimalCoveWriter::new();
        writer.required_features |= FEATURE_EXTENSION_REGISTRY;
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::ExtensionRegistry as u16,
            profile: 0,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: FEATURE_EXTENSION_REGISTRY,
            optional_features: 0,
            data: required_unknown_extension_registry_payload(),
        });
        let bytes = writer.write().unwrap();
        let opts = ValidationOptions {
            semantic: true,
            verify_digests: false,
            allow_unknown_optional_extensions: true,
            ..ValidationOptions::default()
        };
        assert_eq!(
            validate_bytes_with_options(&bytes, opts).unwrap_err(),
            CoveError::BadExtension
        );
    }

    #[test]
    fn semantic_validation_rejects_missing_extension_registry_section_when_required() {
        let mut writer = MinimalCoveWriter::new();
        writer.required_features |= FEATURE_EXTENSION_REGISTRY;
        let bytes = writer.write().unwrap();
        let opts = ValidationOptions {
            semantic: true,
            verify_digests: false,
            allow_unknown_optional_extensions: true,
            ..ValidationOptions::default()
        };
        assert!(matches!(
            validate_bytes_with_options(&bytes, opts),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn semantic_validation_validates_extension_registry_when_in_optional_features() {
        let mut writer = MinimalCoveWriter::new();
        // Extension registry advertised as optional, not required.
        writer.optional_features |= FEATURE_EXTENSION_REGISTRY;
        // Build an empty Spec §45 registry payload (no entries).
        let mut ext = Vec::new();
        ext.extend_from_slice(&0u32.to_le_bytes()); // extension_count = 0
        ext.extend_from_slice(&0u32.to_le_bytes()); // flags
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::ExtensionRegistry as u16,
            profile: 0,
            flags: 0,
            item_count: 0,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: 0,
            optional_features: FEATURE_EXTENSION_REGISTRY,
            data: ext,
        });
        let bytes = writer.write().unwrap();
        let opts = ValidationOptions {
            semantic: true,
            verify_digests: false,
            allow_unknown_optional_extensions: true,
            ..ValidationOptions::default()
        };
        // Should succeed: empty registry, no required extensions.
        assert!(validate_bytes_with_options(&bytes, opts).is_ok());
    }

    #[test]
    fn semantic_validation_rejects_required_extension_when_in_optional_features_section() {
        let mut writer = MinimalCoveWriter::new();
        // Feature advertised as optional in the file header.
        writer.optional_features |= FEATURE_EXTENSION_REGISTRY;
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::ExtensionRegistry as u16,
            profile: 0,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: 0,
            optional_features: FEATURE_EXTENSION_REGISTRY,
            data: required_unknown_extension_registry_payload(),
        });
        let bytes = writer.write().unwrap();
        let opts = ValidationOptions {
            semantic: true,
            verify_digests: false,
            allow_unknown_optional_extensions: true,
            ..ValidationOptions::default()
        };
        // The registry section is present and contains a required unknown extension — must reject.
        assert_eq!(
            validate_bytes_with_options(&bytes, opts).unwrap_err(),
            CoveError::BadExtension
        );
    }

    #[test]
    fn verify_digests_without_manifest_is_noop() {
        let bytes = MinimalCoveWriter::write_empty_file().unwrap();
        let opts = ValidationOptions {
            semantic: true,
            verify_digests: true,
            allow_unknown_optional_extensions: true,
            ..ValidationOptions::default()
        };
        assert!(validate_bytes_with_options(&bytes, opts).is_ok());
    }

    #[test]
    fn verify_digests_rejects_missing_referenced_section() {
        let mut writer = MinimalCoveWriter::new();
        let digest = DigestManifest {
            algorithm: crate::constants::DigestAlgorithm::Sha256,
            scope: DigestScope::Section,
            root_digest: [0; 32],
            entries: vec![DigestEntry {
                target_kind: DigestTargetKind::Section,
                section_id: 99,
                local_id: 0,
                offset: 0,
                length: 0,
                digest: vec![0; 32],
            }],
        }
        .serialize()
        .unwrap();
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::DigestManifest as u16,
            profile: 0,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: 0,
            optional_features: 0,
            data: digest,
        });
        let bytes = writer.write().unwrap();
        let opts = ValidationOptions {
            semantic: true,
            verify_digests: true,
            allow_unknown_optional_extensions: true,
            ..ValidationOptions::default()
        };
        assert!(matches!(
            validate_bytes_with_options(&bytes, opts),
            Err(CoveError::BadSection(_))
        ));
    }
}
