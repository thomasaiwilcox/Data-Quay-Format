//! Spec §48 — engine-neutral mount/read protocol helpers.
//!
//! Mounting is a metadata and representation-selection step. It validates the
//! host COVE file, reads catalogs and optional acceleration metadata, and
//! prepares dictionary reverse lookups or engine execution-code maps. It does
//! not make COVE-Core/COVE-T decoding depend on COVE-E or Harbor metadata.

use std::collections::BTreeMap;

use crate::{
    artifact::{covm::CovmFile, covx::CovxFile},
    checksum, compression,
    constants::{DigestAlgorithm, SectionKind, ValueTag},
    dictionary::{DictionaryValue, FileDictionary},
    digest::verify_digest,
    domain::ColumnDomain,
    footer::{CoveFooter, CoveSectionEntryV1},
    header::CoveHeaderV1,
    index::{
        aggregate::AggregateSynopsis, bloom::BloomFilterIndex, composite::CompositeIndex,
        exact_set::ExactSetIndex, inverted::InvertedMorselIndex, lookup::LookupIndex,
        topn::TopNSummary,
    },
    profile::cove_e::{EngineMountPolicyV1, ExecutionCodeDescriptorV1},
    reader::{self, IgnoredOptionalSection, OptionalPushdownPolicy, ValidationOptions},
    table::{ColumnEntry, TableCatalog},
    zone_stats::ZoneStatsSection,
    CoveError,
};

/// Requested output representation for dictionary-backed columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputRepresentation {
    DecodeToValue,
    MapToArrowDictionary,
    MapToExecutionCode,
}

/// Options controlling mount-time validation and representation selection.
#[derive(Debug, Clone)]
pub struct MountOptions<'a> {
    pub representation: OutputRepresentation,
    pub verify_digests: bool,
    pub allow_unknown_optional_extensions: bool,
    pub covx: Option<&'a [u8]>,
    pub covm: Option<&'a [u8]>,
}

impl Default for MountOptions<'_> {
    fn default() -> Self {
        Self {
            representation: OutputRepresentation::DecodeToValue,
            verify_digests: false,
            allow_unknown_optional_extensions: true,
            covx: None,
            covm: None,
        }
    }
}

/// Parsed mount result for a COVE file.
#[derive(Debug, Clone)]
pub struct MountedCoveFile {
    pub header: CoveHeaderV1,
    pub footer: CoveFooter,
    pub table_catalog: Option<TableCatalog>,
    pub tables: Vec<MountedTable>,
    pub dictionary: Option<FileDictionary>,
    pub representation: OutputRepresentation,
    pub reverse_lookup: Option<ReverseLookup>,
    pub execution_code_map: Option<ExecutionCodeMap>,
    pub execution_descriptors: Vec<ExecutionCodeDescriptorV1>,
    pub engine_mount_policies: Vec<EngineMountPolicyV1>,
    pub column_domains: Vec<ColumnDomain>,
    pub zone_stats: Vec<ZoneStatsSection>,
    pub scan_indexes: Vec<MountedScanIndex>,
    pub ignored_optional_sections: Vec<IgnoredOptionalSection>,
    pub covx_status: SidecarValidationStatus,
    pub covm_status: SidecarValidationStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountedTable {
    pub table_id: u32,
    pub namespace: String,
    pub name: String,
    pub row_count: u64,
    pub columns: Vec<MountedColumn>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountedColumn {
    pub column_id: u32,
    pub name: String,
    pub logical: crate::constants::CoveLogicalType,
    pub physical: crate::constants::CovePhysicalKind,
    pub nullable: bool,
    pub representation: OutputRepresentation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountedScanIndex {
    pub section_id: u32,
    pub kind: SectionKind,
    pub row_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidecarValidationStatus {
    NotProvided,
    Valid,
    StaleIgnored,
}

/// Canonical-value to FileCode reverse lookup built from the file dictionary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReverseLookup {
    pub by_canonical_value: BTreeMap<Vec<u8>, u32>,
    pub redacted_filecodes: Vec<u32>,
}

/// Engine-local execution code materialized for a FileCode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionCodeValue {
    Unsigned(u64),
    Signed(i64),
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionCodeMap {
    pub filecode_to_execution: Vec<ExecutionCodeValue>,
}

pub struct ExecutionCodeRequest<'a> {
    pub file_code: u32,
    pub value_tag: ValueTag,
    pub canonical_value: &'a [u8],
    pub descriptor: Option<&'a ExecutionCodeDescriptorV1>,
}

/// Resolves canonical COVE values to engine-local execution codes.
pub trait ExecutionCodeResolver {
    fn resolve(&self, request: ExecutionCodeRequest<'_>) -> Result<ExecutionCodeValue, CoveError>;
}

/// Spec §44.3 external Harbor FileCode-to-EngineCode metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarborMountCodeMap {
    pub file_id: [u8; 16],
    pub table_id: u32,
    pub dictionary_crc32c: u32,
    pub lease_epoch: u64,
    pub filecode_to_enginecode: Vec<u64>,
}

/// Mount a COVE file and prepare the requested output representation.
pub fn mount_cove_file(
    data: &[u8],
    options: MountOptions<'_>,
    resolver: Option<&dyn ExecutionCodeResolver>,
) -> Result<MountedCoveFile, CoveError> {
    let validation = reader::validate_bytes_with_options(
        data,
        ValidationOptions {
            semantic: true,
            verify_digests: options.verify_digests,
            allow_unknown_optional_extensions: options.allow_unknown_optional_extensions,
            optional_pushdown_policy: OptionalPushdownPolicy::FailOpen,
        },
    )?;
    let header = validation.validated.header;
    let footer = validation.validated.footer;

    let table_catalog = parse_table_catalog(data, &footer)?;
    let dictionary = parse_dictionary(data, &footer)?;
    let reverse_lookup = dictionary.as_ref().map(build_reverse_lookup).transpose()?;
    let execution_descriptors = parse_execution_descriptors(data, &footer)?;
    let engine_mount_policies = parse_engine_mount_policies(data, &footer)?;
    let execution_code_map = if options.representation == OutputRepresentation::MapToExecutionCode {
        let dictionary = dictionary.as_ref().ok_or(CoveError::ExecutionCodeMap)?;
        Some(build_execution_code_map(
            dictionary,
            execution_descriptors.first(),
            resolver.ok_or(CoveError::ExecutionCodeMap)?,
        )?)
    } else {
        None
    };
    let column_domains = parse_column_domains(data, &footer)?;
    let zone_stats = parse_zone_stats(data, &footer)?;
    let scan_indexes = parse_scan_indexes(data, &footer)?;
    let covx_status = validate_covx_sidecar(
        options.covx,
        data,
        &header,
        &validation.validated.postscript,
    );
    let covm_status = validate_covm_sidecar(
        options.covm,
        data,
        &header,
        &validation.validated.postscript,
    );
    let tables = table_catalog
        .as_ref()
        .map(|catalog| mounted_tables(catalog, options.representation))
        .unwrap_or_default();

    Ok(MountedCoveFile {
        header,
        footer,
        table_catalog,
        tables,
        dictionary,
        representation: options.representation,
        reverse_lookup,
        execution_code_map,
        execution_descriptors,
        engine_mount_policies,
        column_domains,
        zone_stats,
        scan_indexes,
        ignored_optional_sections: validation.ignored_optional_sections,
        covx_status,
        covm_status,
    })
}

pub fn build_reverse_lookup(dictionary: &FileDictionary) -> Result<ReverseLookup, CoveError> {
    let mut lookup = ReverseLookup::default();
    for file_code in 0..dictionary.len() {
        match dictionary.decode_value(file_code)? {
            DictionaryValue::RawBytes(bytes) => {
                if lookup.by_canonical_value.insert(bytes, file_code).is_some() {
                    return Err(CoveError::BadSection(
                        "dictionary contains duplicate canonical values".into(),
                    ));
                }
            }
            DictionaryValue::RedactedPresent => lookup.redacted_filecodes.push(file_code),
        }
    }
    Ok(lookup)
}

pub fn build_execution_code_map(
    dictionary: &FileDictionary,
    descriptor: Option<&ExecutionCodeDescriptorV1>,
    resolver: &dyn ExecutionCodeResolver,
) -> Result<ExecutionCodeMap, CoveError> {
    let mut filecode_to_execution = Vec::with_capacity(dictionary.len() as usize);
    for file_code in 0..dictionary.len() {
        let entry = dictionary.get_entry(file_code)?;
        let value_tag = ValueTag::from_u16(entry.value_tag).ok_or(CoveError::BadFileCode)?;
        let canonical_value = match dictionary.decode_value(file_code)? {
            DictionaryValue::RawBytes(bytes) => bytes,
            DictionaryValue::RedactedPresent => return Err(CoveError::RedactionPolicy),
        };
        filecode_to_execution.push(resolver.resolve(ExecutionCodeRequest {
            file_code,
            value_tag,
            canonical_value: &canonical_value,
            descriptor,
        })?);
    }
    Ok(ExecutionCodeMap {
        filecode_to_execution,
    })
}

pub fn dictionary_crc32c(dictionary: &FileDictionary) -> u32 {
    let mut bytes = Vec::with_capacity(
        crate::dictionary::DICT_HEADER_SIZE
            + dictionary.entries.len() * crate::dictionary::DICT_INDEX_ENTRY_SIZE
            + dictionary.payload.len(),
    );
    bytes.extend_from_slice(&dictionary.header.serialize());
    for entry in &dictionary.entries {
        bytes.extend_from_slice(&entry.serialize());
    }
    bytes.extend_from_slice(&dictionary.payload);
    checksum::crc32c(&bytes)
}

impl HarborMountCodeMap {
    pub fn validate_for(
        &self,
        file_id: [u8; 16],
        table_id: u32,
        dictionary: &FileDictionary,
        lease_epoch: u64,
    ) -> Result<(), CoveError> {
        if self.file_id != file_id
            || self.table_id != table_id
            || self.dictionary_crc32c != dictionary_crc32c(dictionary)
            || self.lease_epoch != lease_epoch
            || self.filecode_to_enginecode.len() != dictionary.len() as usize
        {
            return Err(CoveError::SidecarStale);
        }
        Ok(())
    }

    pub fn is_stale_for(
        &self,
        file_id: [u8; 16],
        table_id: u32,
        dictionary: &FileDictionary,
        lease_epoch: u64,
    ) -> bool {
        self.validate_for(file_id, table_id, dictionary, lease_epoch)
            .is_err()
    }

    pub fn rebuild(
        file_id: [u8; 16],
        table_id: u32,
        dictionary: &FileDictionary,
        lease_epoch: u64,
        descriptor: Option<&ExecutionCodeDescriptorV1>,
        resolver: &dyn ExecutionCodeResolver,
    ) -> Result<Self, CoveError> {
        let execution = build_execution_code_map(dictionary, descriptor, resolver)?;
        let mut filecode_to_enginecode = Vec::with_capacity(execution.filecode_to_execution.len());
        for value in execution.filecode_to_execution {
            match value {
                ExecutionCodeValue::Unsigned(code) => filecode_to_enginecode.push(code),
                _ => return Err(CoveError::ExecutionCodeMap),
            }
        }
        Ok(Self {
            file_id,
            table_id,
            dictionary_crc32c: dictionary_crc32c(dictionary),
            lease_epoch,
            filecode_to_enginecode,
        })
    }
}

pub fn validate_or_rebuild_harbor_map(
    existing: Option<&HarborMountCodeMap>,
    file_id: [u8; 16],
    table_id: u32,
    dictionary: &FileDictionary,
    lease_epoch: u64,
    descriptor: Option<&ExecutionCodeDescriptorV1>,
    resolver: Option<&dyn ExecutionCodeResolver>,
) -> Result<HarborMountCodeMap, CoveError> {
    if let Some(existing) = existing {
        if existing
            .validate_for(file_id, table_id, dictionary, lease_epoch)
            .is_ok()
        {
            return Ok(existing.clone());
        }
    }
    HarborMountCodeMap::rebuild(
        file_id,
        table_id,
        dictionary,
        lease_epoch,
        descriptor,
        resolver.ok_or(CoveError::HarborMountLease)?,
    )
}

fn mounted_tables(
    catalog: &TableCatalog,
    representation: OutputRepresentation,
) -> Vec<MountedTable> {
    catalog
        .tables
        .iter()
        .map(|table| MountedTable {
            table_id: table.table_id,
            namespace: table.namespace.clone(),
            name: table.name.clone(),
            row_count: table.row_count,
            columns: table
                .columns
                .iter()
                .map(|column| mounted_column(column, representation))
                .collect(),
        })
        .collect()
}

fn mounted_column(column: &ColumnEntry, representation: OutputRepresentation) -> MountedColumn {
    MountedColumn {
        column_id: column.column_id,
        name: column.name.clone(),
        logical: column.logical,
        physical: column.physical,
        nullable: column.nullable,
        representation,
    }
}

fn parse_table_catalog(
    data: &[u8],
    footer: &CoveFooter,
) -> Result<Option<TableCatalog>, CoveError> {
    find_sections(footer, SectionKind::TableCatalog)
        .into_iter()
        .next()
        .map(|entry| {
            let payload = compression::section_payload(data, entry)?;
            TableCatalog::parse(&payload)
        })
        .transpose()
}

fn parse_dictionary(data: &[u8], footer: &CoveFooter) -> Result<Option<FileDictionary>, CoveError> {
    let index_entry = find_sections(footer, SectionKind::FileDictionaryIndex)
        .into_iter()
        .next();
    let Some(index_entry) = index_entry else {
        return Ok(None);
    };
    let payload_entry = find_sections(footer, SectionKind::FileDictionaryPayload)
        .into_iter()
        .next();
    let index_payload = compression::section_payload(data, index_entry)?;
    let payload = match payload_entry {
        Some(entry) => compression::section_payload(data, entry)?,
        None => std::borrow::Cow::Borrowed(&[][..]),
    };
    FileDictionary::parse(&index_payload, &payload).map(Some)
}

fn parse_column_domains(data: &[u8], footer: &CoveFooter) -> Result<Vec<ColumnDomain>, CoveError> {
    let mut out = Vec::new();
    for entry in find_sections(footer, SectionKind::ColumnDomain) {
        let Ok(payload) = compression::section_payload(data, entry) else {
            continue;
        };
        let Ok(domain) = ColumnDomain::parse(&payload) else {
            continue;
        };
        out.push(domain);
    }
    Ok(out)
}

fn parse_zone_stats(data: &[u8], footer: &CoveFooter) -> Result<Vec<ZoneStatsSection>, CoveError> {
    let mut out = Vec::new();
    for entry in find_sections(footer, SectionKind::ZoneStats) {
        let payload = compression::section_payload(data, entry)?;
        out.push(ZoneStatsSection::parse(&payload)?);
    }
    Ok(out)
}

fn parse_execution_descriptors(
    data: &[u8],
    footer: &CoveFooter,
) -> Result<Vec<ExecutionCodeDescriptorV1>, CoveError> {
    let mut out = Vec::new();
    for entry in find_sections(footer, SectionKind::ExecutionCodeDescriptor) {
        let payload = compression::section_payload(data, entry)?;
        out.push(ExecutionCodeDescriptorV1::parse(&payload)?);
    }
    Ok(out)
}

fn parse_engine_mount_policies(
    data: &[u8],
    footer: &CoveFooter,
) -> Result<Vec<EngineMountPolicyV1>, CoveError> {
    let mut out = Vec::new();
    for entry in find_sections(footer, SectionKind::EngineMountPolicy) {
        let payload = compression::section_payload(data, entry)?;
        out.push(EngineMountPolicyV1::parse(&payload)?);
    }
    Ok(out)
}

fn parse_scan_indexes(
    data: &[u8],
    footer: &CoveFooter,
) -> Result<Vec<MountedScanIndex>, CoveError> {
    let mut out = Vec::new();
    for entry in &footer.sections {
        let Some(kind) = SectionKind::from_u16(entry.section_kind) else {
            continue;
        };
        match kind {
            SectionKind::ExactSetIndex => {
                let Ok(payload) = compression::section_payload(data, entry) else {
                    continue;
                };
                if ExactSetIndex::parse(&payload).is_err() {
                    continue;
                }
            }
            SectionKind::BloomIndex => {
                let Ok(payload) = compression::section_payload(data, entry) else {
                    continue;
                };
                if BloomFilterIndex::parse(&payload).is_err() {
                    continue;
                }
            }
            SectionKind::InvertedMorselIndex => {
                let Ok(payload) = compression::section_payload(data, entry) else {
                    continue;
                };
                if InvertedMorselIndex::parse(&payload).is_err() {
                    continue;
                }
            }
            SectionKind::LookupIndex => {
                let Ok(payload) = compression::section_payload(data, entry) else {
                    continue;
                };
                if LookupIndex::parse(&payload).is_err() {
                    continue;
                }
            }
            SectionKind::AggregateSynopsis => {
                let Ok(payload) = compression::section_payload(data, entry) else {
                    continue;
                };
                if AggregateSynopsis::parse(&payload).is_err() {
                    continue;
                }
            }
            SectionKind::CompositeZoneIndex => {
                let Ok(payload) = compression::section_payload(data, entry) else {
                    continue;
                };
                if CompositeIndex::parse(&payload).is_err() {
                    continue;
                }
            }
            SectionKind::TopNZoneSummary => {
                let Ok(payload) = compression::section_payload(data, entry) else {
                    continue;
                };
                if TopNSummary::parse(&payload).is_err() {
                    continue;
                }
            }
            _ => continue,
        }
        out.push(MountedScanIndex {
            section_id: entry.section_id,
            kind,
            row_count: entry.row_count,
        });
    }
    Ok(out)
}

fn validate_covx_sidecar(
    sidecar: Option<&[u8]>,
    cove_bytes: &[u8],
    header: &CoveHeaderV1,
    postscript: &crate::postscript::CovePostscriptV1,
) -> SidecarValidationStatus {
    let Some(sidecar) = sidecar else {
        return SidecarValidationStatus::NotProvided;
    };
    let Ok(covx) = CovxFile::parse(sidecar) else {
        return SidecarValidationStatus::StaleIgnored;
    };
    let valid = covx.referenced_files.iter().any(|entry| {
        referenced_file_matches(
            entry.file_id,
            entry.file_len,
            entry.footer_crc32c,
            entry.digest_algorithm,
            &entry.digest,
            cove_bytes,
            header,
            postscript,
        )
    });
    if valid {
        SidecarValidationStatus::Valid
    } else {
        SidecarValidationStatus::StaleIgnored
    }
}

fn validate_covm_sidecar(
    sidecar: Option<&[u8]>,
    cove_bytes: &[u8],
    header: &CoveHeaderV1,
    postscript: &crate::postscript::CovePostscriptV1,
) -> SidecarValidationStatus {
    let Some(sidecar) = sidecar else {
        return SidecarValidationStatus::NotProvided;
    };
    let Ok(covm) = CovmFile::parse(sidecar) else {
        return SidecarValidationStatus::StaleIgnored;
    };
    let valid = covm.files.iter().any(|entry| {
        referenced_file_matches(
            entry.file_id,
            entry.file_len,
            entry.footer_crc32c,
            entry.digest_algorithm,
            &entry.digest,
            cove_bytes,
            header,
            postscript,
        )
    });
    if valid {
        SidecarValidationStatus::Valid
    } else {
        SidecarValidationStatus::StaleIgnored
    }
}

fn referenced_file_matches(
    file_id: [u8; 16],
    file_len: u64,
    footer_crc32c: u32,
    digest_algorithm: u16,
    digest: &[u8],
    cove_bytes: &[u8],
    header: &CoveHeaderV1,
    postscript: &crate::postscript::CovePostscriptV1,
) -> bool {
    if file_id != header.file_id
        || file_len != postscript.file_len
        || footer_crc32c != postscript.footer.crc32c
    {
        return false;
    }
    if digest.is_empty() && digest_algorithm == DigestAlgorithm::None as u16 {
        return true;
    }
    let Some(algorithm) = DigestAlgorithm::from_u16(digest_algorithm) else {
        return false;
    };
    verify_digest(algorithm, cove_bytes, digest).is_ok()
}

fn find_sections(footer: &CoveFooter, kind: SectionKind) -> Vec<&CoveSectionEntryV1> {
    footer
        .sections
        .iter()
        .filter(|entry| entry.section_kind == kind as u16)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        artifact::{
            covm::{CovmFileEntryV1, CovmHeaderV1, CovmPostscriptV1},
            covx::{CovxHeaderV1, CovxPostscriptV1, CovxReferencedFileV1},
        },
        constants::{
            CoveLogicalType, CovePhysicalKind, DigestAlgorithm, PrimaryProfile, StorageClass,
            FEATURE_FILE_DICTIONARY, FEATURE_TABLE_PROFILE,
        },
        dictionary::{FileDictionaryHeaderV1, FileDictionaryIndexEntryV1},
        digest::compute_digest,
        table::{ColumnEntry, TableEntry},
        wire,
        writer::{MinimalCoveWriter, ScanProfileCoveWriter, SectionPayload},
    };

    struct TestResolver;

    impl ExecutionCodeResolver for TestResolver {
        fn resolve(
            &self,
            request: ExecutionCodeRequest<'_>,
        ) -> Result<ExecutionCodeValue, CoveError> {
            Ok(ExecutionCodeValue::Unsigned(
                10_000 + u64::from(request.file_code),
            ))
        }
    }

    #[test]
    fn decoded_mount_reads_table_catalog() {
        let bytes = ScanProfileCoveWriter::new(sample_catalog())
            .write()
            .unwrap();
        let mounted = mount_cove_file(&bytes, MountOptions::default(), None).unwrap();
        assert_eq!(mounted.tables.len(), 1);
        assert_eq!(mounted.tables[0].columns[0].name, "name");
        assert_eq!(mounted.representation, OutputRepresentation::DecodeToValue);
    }

    #[test]
    fn corrupt_covx_and_covm_sidecars_are_ignored() {
        let bytes = ScanProfileCoveWriter::new(sample_catalog())
            .write()
            .unwrap();
        let mounted = mount_cove_file(
            &bytes,
            MountOptions {
                covx: Some(b"not a covx"),
                covm: Some(b"not a covm"),
                ..MountOptions::default()
            },
            None,
        )
        .unwrap();
        assert_eq!(mounted.covx_status, SidecarValidationStatus::StaleIgnored);
        assert_eq!(mounted.covm_status, SidecarValidationStatus::StaleIgnored);
    }

    #[test]
    fn sidecar_digest_mismatch_is_ignored() {
        let bytes = ScanProfileCoveWriter::new(sample_catalog())
            .write()
            .unwrap();
        let validation = reader::validate_bytes(&bytes).unwrap();
        let covx = CovxFile {
            header: CovxHeaderV1::new([0x11; 16], 0, 0),
            referenced_files: vec![CovxReferencedFileV1 {
                file_id: validation.header.file_id,
                file_len: validation.postscript.file_len,
                footer_crc32c: validation.postscript.footer.crc32c,
                digest_algorithm: DigestAlgorithm::Sha256 as u16,
                digest: vec![0; 32],
            }],
            postscript: CovxPostscriptV1 {
                header_offset: 0,
                header_len: 0,
                entries_offset: 0,
                entries_len: 0,
                file_len: 0,
                flags: 0,
                checksum: 0,
            },
        }
        .serialize()
        .unwrap();
        let mounted = mount_cove_file(
            &bytes,
            MountOptions {
                covx: Some(&covx),
                ..MountOptions::default()
            },
            None,
        )
        .unwrap();
        assert_eq!(mounted.covx_status, SidecarValidationStatus::StaleIgnored);
    }

    #[test]
    fn sidecar_digest_match_is_valid() {
        let bytes = ScanProfileCoveWriter::new(sample_catalog())
            .write()
            .unwrap();
        let validation = reader::validate_bytes(&bytes).unwrap();
        let digest = compute_digest(DigestAlgorithm::Sha256, &bytes).unwrap();
        let covm = CovmFile {
            header: CovmHeaderV1::new([0x22; 16], 1, 0, 0),
            files: vec![CovmFileEntryV1 {
                file_id: validation.header.file_id,
                uri: "file:///test.cove".into(),
                file_len: validation.postscript.file_len,
                footer_crc32c: validation.postscript.footer.crc32c,
                digest_algorithm: DigestAlgorithm::Sha256 as u16,
                digest,
                row_count: 0,
                segment_count: 0,
                file_stats_ref: 0,
                file_exact_set_ref: 0,
                flags: 0,
            }],
            postscript: CovmPostscriptV1 {
                header_offset: 0,
                header_len: 0,
                entries_offset: 0,
                entries_len: 0,
                file_len: 0,
                flags: 0,
                checksum: 0,
            },
        }
        .serialize()
        .unwrap();
        let mounted = mount_cove_file(
            &bytes,
            MountOptions {
                covm: Some(&covm),
                ..MountOptions::default()
            },
            None,
        )
        .unwrap();
        assert_eq!(mounted.covm_status, SidecarValidationStatus::Valid);
    }

    #[test]
    fn arrow_dictionary_mount_builds_reverse_lookup() {
        let bytes = dictionary_file();
        let mounted = mount_cove_file(
            &bytes,
            MountOptions {
                representation: OutputRepresentation::MapToArrowDictionary,
                ..MountOptions::default()
            },
            None,
        )
        .unwrap();
        let lookup = mounted.reverse_lookup.unwrap();
        assert_eq!(lookup.by_canonical_value.len(), 2);
        assert_eq!(
            lookup.by_canonical_value.get(&canonical_utf8("red")),
            Some(&0)
        );
    }

    #[test]
    fn execution_code_mount_requires_resolver() {
        let bytes = dictionary_file();
        let err = mount_cove_file(
            &bytes,
            MountOptions {
                representation: OutputRepresentation::MapToExecutionCode,
                ..MountOptions::default()
            },
            None,
        )
        .unwrap_err();
        assert_eq!(err, CoveError::ExecutionCodeMap);
    }

    #[test]
    fn execution_code_mount_uses_resolver() {
        let bytes = dictionary_file();
        let mounted = mount_cove_file(
            &bytes,
            MountOptions {
                representation: OutputRepresentation::MapToExecutionCode,
                ..MountOptions::default()
            },
            Some(&TestResolver),
        )
        .unwrap();
        assert_eq!(
            mounted.execution_code_map.unwrap().filecode_to_execution,
            vec![
                ExecutionCodeValue::Unsigned(10_000),
                ExecutionCodeValue::Unsigned(10_001)
            ]
        );
    }

    #[test]
    fn stale_harbor_map_rebuilds_through_resolver() {
        let dictionary = sample_dictionary();
        let stale = HarborMountCodeMap {
            file_id: [9; 16],
            table_id: 1,
            dictionary_crc32c: 0,
            lease_epoch: 1,
            filecode_to_enginecode: vec![],
        };
        let rebuilt = validate_or_rebuild_harbor_map(
            Some(&stale),
            [1; 16],
            7,
            &dictionary,
            2,
            None,
            Some(&TestResolver),
        )
        .unwrap();
        assert_eq!(rebuilt.file_id, [1; 16]);
        assert_eq!(rebuilt.table_id, 7);
        assert_eq!(rebuilt.lease_epoch, 2);
        assert_eq!(rebuilt.filecode_to_enginecode, vec![10_000, 10_001]);
    }

    #[test]
    fn valid_harbor_map_is_reused() {
        let dictionary = sample_dictionary();
        let existing = HarborMountCodeMap {
            file_id: [1; 16],
            table_id: 7,
            dictionary_crc32c: dictionary_crc32c(&dictionary),
            lease_epoch: 2,
            filecode_to_enginecode: vec![1, 2],
        };
        let reused =
            validate_or_rebuild_harbor_map(Some(&existing), [1; 16], 7, &dictionary, 2, None, None)
                .unwrap();
        assert_eq!(reused, existing);
    }

    fn sample_catalog() -> TableCatalog {
        TableCatalog {
            flags: 0,
            tables: vec![TableEntry {
                table_id: 7,
                namespace: "public".into(),
                name: "items".into(),
                row_count: 0,
                primary_sort_key_count: 0,
                clustering_key_count: 0,
                flags: 0,
                columns: vec![ColumnEntry {
                    column_id: 1,
                    name: "name".into(),
                    logical: CoveLogicalType::Utf8,
                    physical: CovePhysicalKind::VarBytes,
                    nullable: false,
                    sort_order: 0,
                    collation_id: 0,
                    precision: 0,
                    scale: 0,
                    flags: 0,
                }],
            }],
        }
    }

    fn dictionary_file() -> Vec<u8> {
        let dictionary = sample_dictionary();
        let catalog = TableCatalog {
            flags: 0,
            tables: vec![TableEntry {
                table_id: 7,
                namespace: "public".into(),
                name: "items".into(),
                row_count: 0,
                primary_sort_key_count: 0,
                clustering_key_count: 0,
                flags: 0,
                columns: vec![ColumnEntry {
                    column_id: 1,
                    name: "name".into(),
                    logical: CoveLogicalType::Utf8,
                    physical: CovePhysicalKind::FileCode,
                    nullable: false,
                    sort_order: 0,
                    collation_id: 0,
                    precision: 0,
                    scale: 0,
                    flags: 0,
                }],
            }],
        };
        let mut dictionary_index = Vec::new();
        dictionary_index.extend_from_slice(&dictionary.header.serialize());
        for entry in &dictionary.entries {
            dictionary_index.extend_from_slice(&entry.serialize());
        }
        let table_catalog = catalog.serialize().unwrap();
        let mut writer = MinimalCoveWriter::new();
        writer.primary_profile = PrimaryProfile::TableScan as u8;
        writer.required_features = FEATURE_TABLE_PROFILE | FEATURE_FILE_DICTIONARY;
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::FileDictionaryIndex as u16,
            profile: PrimaryProfile::Mixed as u8,
            flags: 0,
            item_count: dictionary.len() as u64,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: 0,
            optional_features: 0,
            data: dictionary_index,
        });
        writer.sections.push(SectionPayload {
            section_kind: SectionKind::TableCatalog as u16,
            profile: PrimaryProfile::TableScan as u8,
            flags: 0,
            item_count: 1,
            row_count: 0,
            compression: 0,
            alignment_log2: 0,
            required_features: 0,
            optional_features: 0,
            data: table_catalog,
        });
        writer.write()
    }

    fn sample_dictionary() -> FileDictionary {
        FileDictionary {
            header: FileDictionaryHeaderV1 {
                entry_count: 2,
                flags: 0,
                index_entry_len: FileDictionaryHeaderV1::INDEX_ENTRY_LEN,
                value_hash_algorithm: 0,
                payload_length: 0,
                reserved: [0; 24],
            },
            entries: vec![inline_utf8_entry("red"), inline_utf8_entry("blue")],
            payload: Vec::new(),
        }
    }

    fn inline_utf8_entry(value: &str) -> FileDictionaryIndexEntryV1 {
        let canonical = canonical_utf8(value);
        let mut inline_data = [0u8; 16];
        inline_data[..canonical.len()].copy_from_slice(&canonical);
        FileDictionaryIndexEntryV1 {
            value_tag: ValueTag::Utf8 as u16,
            storage_class: StorageClass::Inline as u8,
            flags: 0,
            inline_len: canonical.len() as u8,
            reserved0: [0; 3],
            inline_data,
            payload_offset: 0,
            payload_length: 0,
            canonical_hash64: 0,
            reserved1: 0,
        }
    }

    fn canonical_utf8(value: &str) -> Vec<u8> {
        let mut canonical = wire::encode_u64_leb128(value.len() as u64);
        canonical.extend_from_slice(value.as_bytes());
        canonical
    }
}
