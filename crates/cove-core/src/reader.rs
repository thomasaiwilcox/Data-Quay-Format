//! Cove Format (COVE) v1.0 — reference reader and structural validator.

use std::{collections::BTreeSet, fs, path::Path};

use crate::{
    checksum,
    collation::CollationRegistry,
    compression,
    constants::{
        PrimaryProfile, SectionKind, StorageClass, FEATURE_ARCHIVE_PROFILE, FEATURE_CODEC_LZ4,
        FEATURE_CODEC_ZSTD, FEATURE_ENGINE_PROFILE, FEATURE_EXTENSION_REGISTRY,
        FEATURE_FILE_DICTIONARY, FEATURE_HARBOR_PROFILE, FEATURE_OBJECT_PROFILE,
        FEATURE_SEMANTIC_MAP, FEATURE_TABLE_PROFILE, MAGIC_COVE,
    },
    dictionary::FileDictionary,
    digest::DigestManifest,
    domain::ColumnDomain,
    extensions::ExtensionRegistry,
    footer::CoveFooter,
    header::{CoveHeaderV1, HEADER_SIZE},
    index::{
        aggregate::AggregateSynopsis, bloom::BloomFilterIndex, composite::CompositeIndex,
        exact_set::ExactSetIndex, inverted::InvertedMorselIndex, lookup::LookupIndex,
        topn::TopNSummary,
    },
    interop::lakehouse::LakehouseHints,
    kernel::KernelCapabilities,
    postscript::{CovePostscriptV1, POSTSCRIPT_TOTAL_SIZE},
    profile::{
        cove_e::{
            CodeSpaceDescriptorV1, EngineMountPolicyV1, EngineProfileRegistry,
            ExecutionCodeDescriptorV1, ExecutionScopeDescriptorV1,
        },
        cove_h::HarborMountHintsV1,
        cove_map::{parse_embedded_section, validate_embedded_sections, EmbeddedMapSection},
        cove_o::{
            validate_self_contained, ObjectTypeCatalog, TemporalBloomIndex, TemporalSegmentData,
            TemporalSegmentIndex, TrustManifest,
        },
    },
    redaction::RedactionManifest,
    registry,
    segment::{TableSegmentIndex, TableSegmentPayloadV1},
    table::TableCatalog,
    zone_stats::ZoneStatsSection,
    CoveError,
};

use crate::footer::CoveSectionEntryV1;

/// Parsed and structurally validated COVE file.
#[derive(Debug, Clone)]
pub struct ValidatedCoveFile {
    pub header: CoveHeaderV1,
    pub postscript: CovePostscriptV1,
    pub footer: CoveFooter,
}

/// Read a complete COVE file and validate its COVE-Core structure.
pub fn read_file(path: &Path) -> Result<ValidatedCoveFile, CoveError> {
    let data = fs::read(path)?;
    validate_bytes(&data)
}

pub fn validate_bytes(data: &[u8]) -> Result<ValidatedCoveFile, CoveError> {
    if data.len() < HEADER_SIZE + POSTSCRIPT_TOTAL_SIZE {
        return Err(CoveError::BufferTooShort);
    }

    let trailing_magic: [u8; 4] = data[data.len() - 4..]
        .try_into()
        .map_err(|_| CoveError::BufferTooShort)?;
    if trailing_magic != MAGIC_COVE {
        return Err(CoveError::BadMagic);
    }

    let postscript = CovePostscriptV1::parse_from_tail(data)?;
    if postscript.file_len != data.len() as u64 {
        return Err(CoveError::OffsetRange);
    }

    let footer_end = postscript.footer.end_offset()?;
    let tail_start = data
        .len()
        .checked_sub(POSTSCRIPT_TOTAL_SIZE)
        .ok_or(CoveError::BufferTooShort)? as u64;
    if postscript.footer.offset < HEADER_SIZE as u64 || footer_end > tail_start {
        return Err(CoveError::OffsetRange);
    }

    let footer_start = postscript.footer.offset as usize;
    let footer_bytes = &data[footer_start..footer_end as usize];
    if checksum::crc32c(footer_bytes) != postscript.footer.crc32c {
        return Err(CoveError::ChecksumMismatch);
    }
    validate_footer_codec_feature_advertisement(&postscript)?;
    let footer_payload = compression::section_spec_payload(data, &postscript.footer)?;
    let footer = CoveFooter::parse(&footer_payload)?;
    if footer.header.total_len()? != postscript.footer.uncompressed_length {
        return Err(CoveError::BadSection(
            "footer header length does not match postscript footer uncompressed_length".to_string(),
        ));
    }

    let header = CoveHeaderV1::parse(data)?;
    validate_sections(data, footer_start, &footer, &header)?;
    validate_required_feature_implementation(&header)?;
    validate_primary_profile_features(&header)?;
    if header.required_features != postscript.required_features
        || header.optional_features != postscript.optional_features
    {
        return Err(CoveError::BadSection(
            "header and postscript feature bits differ".to_string(),
        ));
    }

    Ok(ValidatedCoveFile {
        header,
        postscript,
        footer,
    })
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

/// Options controlling the depth of validation.
#[derive(Debug, Clone)]
pub struct ValidationOptions {
    /// When true, validates dictionary semantics (entry bounds, redaction).
    pub semantic: bool,
    /// When true, verifies section digests if a DigestManifest is present.
    pub verify_digests: bool,
    /// When true, unknown optional extension registry entries are allowed.
    pub allow_unknown_optional_extensions: bool,
}

/// Coarse validation stages surfaced by [`ValidationReport`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationStage {
    Bootstrap,
    Structural,
    SharedSemantic,
    DigestVerification,
    CoveTable,
    CoveObject,
    CoveEngine,
    CoveHarbor,
    CoveMap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationStageStatus {
    Checked,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidationStageReport {
    pub stage: ValidationStage,
    pub status: ValidationStageStatus,
    pub sections_checked: u32,
}

impl Default for ValidationOptions {
    fn default() -> Self {
        Self {
            semantic: false,
            verify_digests: false,
            allow_unknown_optional_extensions: true,
        }
    }
}

/// Result of [`validate_bytes_with_options`].
#[derive(Debug, Clone)]
pub struct ValidationReport {
    /// The structurally validated file.
    pub validated: ValidatedCoveFile,
    /// Whether semantic checks were performed.
    pub semantic_checked: bool,
    /// Number of dictionary entries, if the dictionary was parsed.
    pub dict_entry_count: Option<u32>,
    /// Per-stage validation outcomes.
    pub stages: Vec<ValidationStageReport>,
}

/// Validate a COVE file with configurable options.
///
/// Always performs structural validation (equivalent to [`validate_bytes`]).
/// When `opts.semantic` is true, additionally parses any file dictionary.
/// When `opts.verify_digests` is true, verifies any `DIGEST_MANIFEST` section against section bytes.
pub fn validate_bytes_with_options(
    data: &[u8],
    opts: ValidationOptions,
) -> Result<ValidationReport, CoveError> {
    let validated = validate_bytes(data)?;
    let mut stages = vec![
        ValidationStageReport {
            stage: ValidationStage::Bootstrap,
            status: ValidationStageStatus::Checked,
            sections_checked: 0,
        },
        ValidationStageReport {
            stage: ValidationStage::Structural,
            status: ValidationStageStatus::Checked,
            sections_checked: validated.footer.sections.len() as u32,
        },
    ];

    if !opts.semantic {
        push_skipped_semantic_stages(&mut stages, opts.verify_digests);
        return Ok(ValidationReport {
            validated,
            semantic_checked: false,
            dict_entry_count: None,
            stages,
        });
    }

    let mut dict_entry_count: Option<u32> = None;
    validate_shared_semantics(data, &validated, &opts, &mut dict_entry_count, &mut stages)?;
    if opts.verify_digests {
        let checked = verify_digest_manifests(data, &validated.footer)?;
        push_stage(
            &mut stages,
            ValidationStage::DigestVerification,
            ValidationStageStatus::Checked,
            checked,
        );
    } else {
        push_stage(
            &mut stages,
            ValidationStage::DigestVerification,
            ValidationStageStatus::Skipped,
            0,
        );
    }
    validate_cove_t_semantics(data, &validated, &mut stages)?;
    validate_cove_o_semantics(data, &validated, &mut stages)?;
    validate_cove_e_semantics(data, &validated, &mut stages)?;
    validate_cove_h_semantics(data, &validated, &mut stages)?;
    validate_cove_map_semantics(data, &validated, &mut stages)?;

    Ok(ValidationReport {
        validated,
        semantic_checked: opts.semantic,
        dict_entry_count,
        stages,
    })
}

fn push_stage(
    stages: &mut Vec<ValidationStageReport>,
    stage: ValidationStage,
    status: ValidationStageStatus,
    sections_checked: u32,
) {
    stages.push(ValidationStageReport {
        stage,
        status,
        sections_checked,
    });
}

fn push_skipped_semantic_stages(stages: &mut Vec<ValidationStageReport>, verify_digests: bool) {
    push_stage(
        stages,
        ValidationStage::SharedSemantic,
        ValidationStageStatus::Skipped,
        0,
    );
    push_stage(
        stages,
        ValidationStage::DigestVerification,
        if verify_digests {
            ValidationStageStatus::Checked
        } else {
            ValidationStageStatus::Skipped
        },
        0,
    );
    for stage in [
        ValidationStage::CoveTable,
        ValidationStage::CoveObject,
        ValidationStage::CoveEngine,
        ValidationStage::CoveHarbor,
        ValidationStage::CoveMap,
    ] {
        push_stage(stages, stage, ValidationStageStatus::Skipped, 0);
    }
}

fn verify_digest_manifests(data: &[u8], footer: &CoveFooter) -> Result<u32, CoveError> {
    let mut checked = 0u32;
    for digest_section in footer
        .sections
        .iter()
        .filter(|s| s.section_kind == SectionKind::DigestManifest as u16)
    {
        checked += 1;
        let digest_bytes = compression::section_payload(data, digest_section)?;
        let manifest = DigestManifest::parse(&digest_bytes)?;
        for entry in &manifest.entries {
            let target_section = footer
                .sections
                .binary_search_by_key(&entry.section_id, |s| s.section_id)
                .ok()
                .and_then(|idx| footer.sections.get(idx))
                .ok_or_else(|| {
                    CoveError::BadSection(format!(
                        "digest manifest references missing section_id {}",
                        entry.section_id
                    ))
                })?;

            let section_start = target_section.offset as usize;
            let section_end = target_section.end_offset()? as usize;
            let section_bytes = &data[section_start..section_end];
            manifest.verify_section(entry.section_id, section_bytes)?;
        }
    }
    Ok(checked)
}

fn validate_shared_semantics(
    data: &[u8],
    validated: &ValidatedCoveFile,
    opts: &ValidationOptions,
    dict_entry_count: &mut Option<u32>,
    stages: &mut Vec<ValidationStageReport>,
) -> Result<(), CoveError> {
    let footer = &validated.footer;
    let mut checked = 0u32;
    let mut parsed_dict: Option<FileDictionary> = None;
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
                let dict = FileDictionary::parse(&index_bytes, &payload_bytes)?;
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
                registry.validate_known(opts.allow_unknown_optional_extensions)?;
                checked += 1;
            }
            (false, None) => {}
        }
    }

    for entry in &footer.sections {
        let payload = compression::section_payload(data, entry)?;
        match SectionKind::from_u16(entry.section_kind).ok_or_else(|| {
            CoveError::BadSection(format!("unknown section_kind {}", entry.section_kind))
        })? {
            SectionKind::CollationRegistry => {
                CollationRegistry::parse(&payload)?;
                checked += 1;
            }
            SectionKind::DigestManifest => {
                DigestManifest::parse(&payload)?;
                checked += 1;
            }
            SectionKind::RedactionManifest => {
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
                LakehouseHints::parse(&payload)?;
                checked += 1;
            }
            SectionKind::KernelCapabilities => {
                KernelCapabilities::parse(&payload)?;
                checked += 1;
            }
            SectionKind::FileDictionaryIndex
            | SectionKind::FileDictionaryPayload
            | SectionKind::ArrowInteropHints
            | SectionKind::ExtensionRegistry
            | SectionKind::ProfileCapabilityMatrix
            | SectionKind::VendorExtension
            | SectionKind::TableCatalog
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
            | SectionKind::MapProjectionCatalog => {}
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
    dict: Option<&FileDictionary>,
    dict_section_id: Option<u32>,
    manifest_refs: &BTreeSet<(u32, u64)>,
) -> Result<(), CoveError> {
    let (Some(dict), Some(dict_section_id)) = (dict, dict_section_id) else {
        return Ok(());
    };

    let redacted_codes = dict
        .entries
        .iter()
        .enumerate()
        .filter_map(|(file_code, entry)| {
            matches!(
                StorageClass::from_u8(entry.storage_class),
                Some(StorageClass::Redacted)
            )
            .then_some(file_code as u64)
        })
        .collect::<BTreeSet<_>>();

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
        let file_code_index = usize::try_from(*file_code).map_err(|_| CoveError::ArithOverflow)?;
        let Some(entry) = dict.entries.get(file_code_index) else {
            return Err(CoveError::BadSchema(format!(
                "redaction manifest references out-of-range FileCode {file_code}"
            )));
        };
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

fn validate_cove_t_semantics(
    data: &[u8],
    validated: &ValidatedCoveFile,
    stages: &mut Vec<ValidationStageReport>,
) -> Result<(), CoveError> {
    let mut checked = 0u32;
    for entry in &validated.footer.sections {
        let payload = compression::section_payload(data, entry)?;
        match SectionKind::from_u16(entry.section_kind).ok_or_else(|| {
            CoveError::BadSection(format!("unknown section_kind {}", entry.section_kind))
        })? {
            SectionKind::TableCatalog => {
                TableCatalog::parse(&payload)?;
                checked += 1;
            }
            SectionKind::TableSegmentIndex => {
                TableSegmentIndex::parse(&payload)?;
                checked += 1;
            }
            SectionKind::TableSegmentData => {
                TableSegmentPayloadV1::parse_with_required_features(
                    &payload,
                    validated.header.required_features,
                )?;
                checked += 1;
            }
            SectionKind::ColumnDomain => {
                ColumnDomain::parse(&payload)?;
                checked += 1;
            }
            SectionKind::ExactSetIndex => {
                ExactSetIndex::parse(&payload)?;
                checked += 1;
            }
            SectionKind::BloomIndex => {
                BloomFilterIndex::parse(&payload)?;
                checked += 1;
            }
            SectionKind::InvertedMorselIndex => {
                InvertedMorselIndex::parse(&payload)?;
                checked += 1;
            }
            SectionKind::LookupIndex => {
                LookupIndex::parse(&payload)?;
                checked += 1;
            }
            SectionKind::AggregateSynopsis => {
                AggregateSynopsis::parse(&payload)?;
                checked += 1;
            }
            SectionKind::CompositeZoneIndex => {
                CompositeIndex::parse(&payload)?;
                checked += 1;
            }
            SectionKind::TopNZoneSummary => {
                TopNSummary::parse(&payload)?;
                checked += 1;
            }
            SectionKind::ZoneStats => {
                ZoneStatsSection::parse(&payload)?;
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
    push_stage(
        stages,
        ValidationStage::CoveTable,
        ValidationStageStatus::Checked,
        checked,
    );
    Ok(())
}

fn validate_cove_o_semantics(
    data: &[u8],
    validated: &ValidatedCoveFile,
    stages: &mut Vec<ValidationStageReport>,
) -> Result<(), CoveError> {
    let mut checked = 0u32;
    let mut temporal_segments = Vec::new();
    let mut trust_manifests = Vec::new();
    for entry in &validated.footer.sections {
        let kind = SectionKind::from_u16(entry.section_kind).ok_or_else(|| {
            CoveError::BadSection(format!("unknown section_kind {}", entry.section_kind))
        })?;
        let result = match kind {
            SectionKind::ObjectTypeCatalog => {
                let payload = compression::section_payload(data, entry)?;
                ObjectTypeCatalog::parse(&payload).map(|_| ())
            }
            SectionKind::TemporalSegmentIndex => {
                let payload = compression::section_payload(data, entry)?;
                TemporalSegmentIndex::parse(&payload).map(|_| ())
            }
            SectionKind::TemporalSegmentData => {
                let payload = compression::section_payload(data, entry)?;
                TemporalSegmentData::parse(&payload).map(|segment| {
                    temporal_segments.push(segment);
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
    let file_local_record_ids = temporal_segments
        .iter()
        .flat_map(|segment| {
            (0..segment.rows.len())
                .map(move |row_index| ((segment.header.segment_id as u64) << 32) | row_index as u64)
        })
        .collect::<Vec<_>>();
    let file_prev_refs = temporal_segments
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
    for manifest in trust_manifests {
        manifest.verify_against(&temporal_segments)?;
    }
    push_stage(
        stages,
        ValidationStage::CoveObject,
        ValidationStageStatus::Checked,
        checked,
    );
    Ok(())
}

fn validate_cove_e_semantics(
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

fn validate_cove_h_semantics(
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

fn validate_cove_map_semantics(
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

fn validate_sections(
    data: &[u8],
    footer_start: usize,
    footer: &CoveFooter,
    header: &CoveHeaderV1,
) -> Result<(), CoveError> {
    let mut ranges: Vec<(u64, u64, u32)> = Vec::new();
    let mut last_section_id: Option<u32> = None;

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

        let section_bytes = &data[entry.offset as usize..section_end as usize];
        if checksum::crc32c(section_bytes) != entry.crc32c {
            return Err(CoveError::ChecksumMismatch);
        }
        ranges.push((entry.offset, section_end, entry.section_id));
    }

    Ok(())
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
        _ => {
            return Err(CoveError::BadSection(format!(
                "unknown profile {profile} in section directory"
            )))
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
        1 => {
            if advertised & FEATURE_CODEC_LZ4 == 0 || section_advertised & FEATURE_CODEC_LZ4 == 0 {
                return Err(CoveError::BadSection(format!(
                    "section {} uses LZ4 compression but codec feature bit is not advertised",
                    entry.section_id
                )));
            }
        }
        2 => {
            if advertised & FEATURE_CODEC_ZSTD == 0 || section_advertised & FEATURE_CODEC_ZSTD == 0
            {
                return Err(CoveError::BadSection(format!(
                    "section {} uses ZSTD compression but codec feature bit is not advertised",
                    entry.section_id
                )));
            }
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
        | SectionKind::VendorExtension => &[0],
        // COVE-T only (profile 2)
        SectionKind::TableCatalog
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
    use crate::{
        constants::{
            CompressionCodec, SectionKind, FEATURE_CODEC_LZ4, FEATURE_ENGINE_PROFILE,
            FEATURE_EXTENSION_REGISTRY, FEATURE_FILE_DICTIONARY, FEATURE_HARBOR_PROFILE,
            FEATURE_OBJECT_PROFILE, FEATURE_TABLE_PROFILE,
        },
        footer::CoveFooter,
        postscript::POSTSCRIPT_TOTAL_SIZE,
        writer::{MinimalCoveWriter, SectionPayload},
    };

    fn rewrite_postscript(bytes: &mut [u8], postscript: CovePostscriptV1) {
        let tail_start = bytes.len() - POSTSCRIPT_TOTAL_SIZE;
        bytes[tail_start..].copy_from_slice(&postscript.serialize_tail());
    }

    #[test]
    fn validates_empty_file() {
        let bytes = MinimalCoveWriter::write_empty_file();
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
        let mut bytes = writer.write();
        bytes[HEADER_SIZE] ^= 0x01;
        assert!(matches!(
            validate_bytes(&bytes),
            Err(CoveError::ChecksumMismatch)
        ));
    }

    #[test]
    fn rejects_non_utf8_metadata_written_by_external_source() {
        let mut writer = MinimalCoveWriter::new();
        writer.metadata_json = b"{}".to_vec();
        let mut bytes = writer.write();

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

        let mut bytes = writer.write();
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
        let mut bytes = writer.write();
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
        let bytes = writer.write();
        assert!(matches!(
            validate_bytes(&bytes),
            Err(CoveError::BadSection(_))
        ));
    }

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
        let bytes = writer.write();
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
        let good_bytes = good_writer.write();
        assert!(validate_bytes(&good_bytes).is_ok());
    }

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
            validate_bytes(&writer.write()),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn rejects_primary_profile_missing_required_feature() {
        let mut writer = MinimalCoveWriter::new();
        writer.primary_profile = PrimaryProfile::HarborExecution as u8;
        writer.required_features = FEATURE_TABLE_PROFILE;
        assert!(matches!(
            validate_bytes(&writer.write()),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn accepts_primary_profile_when_required_feature_present() {
        let mut writer = MinimalCoveWriter::new();
        writer.primary_profile = PrimaryProfile::HarborExecution as u8;
        writer.required_features = FEATURE_HARBOR_PROFILE;
        assert!(validate_bytes(&writer.write()).is_ok());
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
            validate_bytes(&writer.write()),
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
            validate_bytes(&writer.write()),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn rejects_bad_trailing_magic() {
        let mut bytes = MinimalCoveWriter::write_empty_file();
        let len = bytes.len();
        bytes[len - 1] = b'X';
        assert!(matches!(validate_bytes(&bytes), Err(CoveError::BadMagic)));
    }

    #[test]
    fn rejects_postscript_file_length_mismatch() {
        let mut bytes = MinimalCoveWriter::write_empty_file();
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
        let mut bytes = writer.write();
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
        let mut bytes = writer.write();
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
        assert!(validate_bytes(&writer.write()).is_ok());
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
            validate_bytes(&writer.write()),
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
            &writer.write(),
            ValidationOptions {
                semantic: true,
                verify_digests: false,
                allow_unknown_optional_extensions: true,
            },
        )
        .is_ok());
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
                &writer.write(),
                ValidationOptions {
                    semantic: true,
                    verify_digests: false,
                    allow_unknown_optional_extensions: true,
                },
            )
            .unwrap_err(),
            CoveError::BadDomain
        );
    }

    #[test]
    fn rejects_postscript_footer_range_before_header() {
        let mut bytes = MinimalCoveWriter::write_empty_file();
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
        let mut bytes = MinimalCoveWriter::write_empty_file();
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
        let mut bytes = writer.write();
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
        let mut bytes = writer.write();
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
        let mut bytes = writer.write();
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
                validate_bytes(&writer.write()),
                Err(CoveError::BadSection(_))
            ));

            writer.required_features = *required_bit;
            assert!(
                validate_bytes(&writer.write()).is_ok(),
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
        let mut bytes = MinimalCoveWriter::write_empty_file();
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
        let mut bytes = MinimalCoveWriter::write_empty_file();
        // Corrupt header magic, then recompute header checksum so header parse fails on magic,
        // not checksum mismatch.
        bytes[0..4].copy_from_slice(b"BAD!");
        bytes[124..128].copy_from_slice(&[0, 0, 0, 0]);
        let crc = checksum::crc32c(&bytes[..HEADER_SIZE]);
        bytes[124..128].copy_from_slice(&crc.to_le_bytes());
        assert!(matches!(validate_bytes(&bytes), Err(CoveError::BadMagic)));
    }

    #[test]
    fn structural_validation_defaults_work() {
        let bytes = MinimalCoveWriter::write_empty_file();
        let opts = ValidationOptions::default();
        let report = validate_bytes_with_options(&bytes, opts).expect("should validate");
        assert!(!report.semantic_checked);
        assert!(report.dict_entry_count.is_none());
    }

    #[test]
    fn semantic_validation_on_empty_file_no_dict() {
        let bytes = MinimalCoveWriter::write_empty_file();
        let opts = ValidationOptions {
            semantic: true,
            verify_digests: false,
            allow_unknown_optional_extensions: true,
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
                &writer.write(),
                ValidationOptions {
                    semantic: true,
                    verify_digests: false,
                    allow_unknown_optional_extensions: true,
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
            &writer.write(),
            ValidationOptions {
                semantic: true,
                verify_digests: false,
                allow_unknown_optional_extensions: true,
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
                &writer.write(),
                ValidationOptions {
                    semantic: true,
                    verify_digests: false,
                    allow_unknown_optional_extensions: true,
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
            &writer.write(),
            ValidationOptions {
                semantic: true,
                verify_digests: false,
                allow_unknown_optional_extensions: true,
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
                &writer.write(),
                ValidationOptions {
                    semantic: true,
                    verify_digests: false,
                    allow_unknown_optional_extensions: true,
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
            &writer.write(),
            ValidationOptions {
                semantic: true,
                verify_digests: false,
                allow_unknown_optional_extensions: true,
            },
        )
        .is_ok());
    }

    #[test]
    fn semantic_validation_rejects_required_unknown_extension_registry() {
        let mut writer = MinimalCoveWriter::new();
        writer.required_features |= FEATURE_EXTENSION_REGISTRY;
        // Build a Spec §45 registry payload with one required unknown extension.
        let mut ext = Vec::new();
        ext.extend_from_slice(&1u32.to_le_bytes()); // extension_count
        ext.extend_from_slice(&0u32.to_le_bytes()); // flags
                                                    // ExtensionEntryV1
        ext.extend_from_slice(&1u32.to_le_bytes()); // extension_id
        ext.extend_from_slice(&3u16.to_le_bytes()); // namespace_len
        ext.extend_from_slice(b"org"); // namespace
        ext.extend_from_slice(&4u16.to_le_bytes()); // name_len
        ext.extend_from_slice(b"test"); // name
        ext.extend_from_slice(&1u16.to_le_bytes()); // version_major
        ext.extend_from_slice(&0u16.to_le_bytes()); // version_minor
        ext.extend_from_slice(&0u16.to_le_bytes()); // extension_kind
        ext.extend_from_slice(&0x0020_0000u64.to_le_bytes()); // required_feature_bit (non-zero → required)
        ext.extend_from_slice(&0u64.to_le_bytes()); // optional_feature_bit
        ext.extend_from_slice(&0u16.to_le_bytes()); // fallback_kind
        ext.extend_from_slice(&0u32.to_le_bytes()); // fallback_ref
        ext.extend_from_slice(&0u32.to_le_bytes()); // payload_ref
        ext.extend_from_slice(&0u32.to_le_bytes()); // checksum
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
            data: ext,
        });
        let bytes = writer.write();
        let opts = ValidationOptions {
            semantic: true,
            verify_digests: false,
            allow_unknown_optional_extensions: true,
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
        let bytes = writer.write();
        let opts = ValidationOptions {
            semantic: true,
            verify_digests: false,
            allow_unknown_optional_extensions: true,
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
        let bytes = writer.write();
        let opts = ValidationOptions {
            semantic: true,
            verify_digests: false,
            allow_unknown_optional_extensions: true,
        };
        // Should succeed: empty registry, no required extensions.
        assert!(validate_bytes_with_options(&bytes, opts).is_ok());
    }

    #[test]
    fn semantic_validation_rejects_required_extension_when_in_optional_features_section() {
        let mut writer = MinimalCoveWriter::new();
        // Feature advertised as optional in the file header.
        writer.optional_features |= FEATURE_EXTENSION_REGISTRY;
        // Build a Spec §45 registry payload with one required unknown extension.
        let mut ext = Vec::new();
        ext.extend_from_slice(&1u32.to_le_bytes()); // extension_count
        ext.extend_from_slice(&0u32.to_le_bytes()); // flags
        ext.extend_from_slice(&1u32.to_le_bytes()); // extension_id
        ext.extend_from_slice(&3u16.to_le_bytes()); // namespace_len
        ext.extend_from_slice(b"org"); // namespace
        ext.extend_from_slice(&4u16.to_le_bytes()); // name_len
        ext.extend_from_slice(b"test"); // name
        ext.extend_from_slice(&1u16.to_le_bytes()); // version_major
        ext.extend_from_slice(&0u16.to_le_bytes()); // version_minor
        ext.extend_from_slice(&0u16.to_le_bytes()); // extension_kind
        ext.extend_from_slice(&0x0020_0000u64.to_le_bytes()); // required_feature_bit (non-zero)
        ext.extend_from_slice(&0u64.to_le_bytes()); // optional_feature_bit
        ext.extend_from_slice(&0u16.to_le_bytes()); // fallback_kind
        ext.extend_from_slice(&0u32.to_le_bytes()); // fallback_ref
        ext.extend_from_slice(&0u32.to_le_bytes()); // payload_ref
        ext.extend_from_slice(&0u32.to_le_bytes()); // checksum
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
            data: ext,
        });
        let bytes = writer.write();
        let opts = ValidationOptions {
            semantic: true,
            verify_digests: false,
            allow_unknown_optional_extensions: true,
        };
        // The registry section is present and contains a required unknown extension — must reject.
        assert_eq!(
            validate_bytes_with_options(&bytes, opts).unwrap_err(),
            CoveError::BadExtension
        );
    }

    #[test]
    fn verify_digests_without_manifest_is_noop() {
        let bytes = MinimalCoveWriter::write_empty_file();
        let opts = ValidationOptions {
            semantic: true,
            verify_digests: true,
            allow_unknown_optional_extensions: true,
        };
        assert!(validate_bytes_with_options(&bytes, opts).is_ok());
    }

    #[test]
    fn verify_digests_rejects_missing_referenced_section() {
        let mut writer = MinimalCoveWriter::new();
        let mut digest = Vec::new();
        digest.extend_from_slice(&1u32.to_le_bytes()); // entry count
        digest.extend_from_slice(&99u32.to_le_bytes()); // section_id
        digest.extend_from_slice(&0u16.to_le_bytes()); // algorithm: None
        digest.extend_from_slice(&0u16.to_le_bytes()); // digest length
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
        let bytes = writer.write();
        let opts = ValidationOptions {
            semantic: true,
            verify_digests: true,
            allow_unknown_optional_extensions: true,
        };
        assert!(matches!(
            validate_bytes_with_options(&bytes, opts),
            Err(CoveError::BadSection(_))
        ));
    }
}
