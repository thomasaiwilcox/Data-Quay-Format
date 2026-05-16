//! Public utility helpers for artifact/report CLIs.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{json, Value};

use crate::{
    artifact::{
        covm::{CovmFile, CovmFileEntryV1, CovmHeaderV1, CovmPostscriptV1},
        covx::{CovxFile, CovxHeaderV1, CovxPostscriptV1, CovxReferencedFileV1},
    },
    compression,
    constants::{DigestAlgorithm, SectionKind},
    digest::compute_digest,
    reader::{validate_bytes_with_options, ValidationOptions},
    segment::TableSegmentIndex,
    table::TableCatalog,
    CoveError,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UtilityArtifactReport {
    pub tool: String,
    pub inputs: Vec<Value>,
    pub output: String,
    pub file_id: [u8; 16],
    pub file_len: u64,
    pub footer_crc32c: Option<u32>,
    pub digest: Vec<u8>,
    pub validation_result: bool,
}

impl UtilityArtifactReport {
    pub fn to_json_value(&self) -> Value {
        json!({
            "tool": self.tool,
            "inputs": self.inputs,
            "output": self.output,
            "file_id": hex_encode(&self.file_id),
            "file_len": self.file_len,
            "footer_crc32c": self.footer_crc32c,
            "digest": {
                "algorithm": "sha256",
                "hex": hex_encode(&self.digest),
            },
            "validation_result": self.validation_result,
        })
    }
}

#[derive(Debug, Clone)]
struct InputIdentity {
    path: PathBuf,
    file_id: [u8; 16],
    file_len: u64,
    footer_crc32c: u32,
    digest: Vec<u8>,
    row_count: u64,
    segment_count: u32,
}

pub fn build_covm_artifact(
    output: impl AsRef<Path>,
    inputs: &[PathBuf],
) -> Result<(Vec<u8>, UtilityArtifactReport), CoveError> {
    let identities = input_identities(inputs)?;
    let dataset_id = artifact_id(&identities)?;
    let files = identities
        .iter()
        .map(|identity| CovmFileEntryV1 {
            file_id: identity.file_id,
            uri: identity.path.display().to_string(),
            file_len: identity.file_len,
            footer_crc32c: identity.footer_crc32c,
            digest_algorithm: DigestAlgorithm::Sha256 as u16,
            digest: identity.digest.clone(),
            row_count: identity.row_count,
            segment_count: identity.segment_count,
            file_stats_ref: 0,
            file_exact_set_ref: 0,
            flags: 0,
        })
        .collect::<Vec<_>>();
    let file = CovmFile {
        header: CovmHeaderV1::new(
            dataset_id,
            1,
            u32::try_from(files.len())
                .map_err(|_| CoveError::BadSection("too many COVM inputs".into()))?,
            created_at_us(),
        ),
        files,
        postscript: empty_covm_postscript(),
    };
    let bytes = file.serialize()?;
    let validation_result = CovmFile::parse(&bytes).is_ok();
    let digest = compute_digest(DigestAlgorithm::Sha256, &bytes)?;
    let report = UtilityArtifactReport {
        tool: "cove-build-covm".into(),
        inputs: input_json(&identities),
        output: output.as_ref().display().to_string(),
        file_id: dataset_id,
        file_len: bytes.len() as u64,
        footer_crc32c: None,
        digest,
        validation_result,
    };
    Ok((bytes, report))
}

pub fn build_covx_artifact(
    output: impl AsRef<Path>,
    inputs: &[PathBuf],
) -> Result<(Vec<u8>, UtilityArtifactReport), CoveError> {
    let identities = input_identities(inputs)?;
    let accelerator_id = artifact_id(&identities)?;
    let referenced_files = identities
        .iter()
        .map(|identity| CovxReferencedFileV1 {
            file_id: identity.file_id,
            file_len: identity.file_len,
            footer_crc32c: identity.footer_crc32c,
            digest_algorithm: DigestAlgorithm::Sha256 as u16,
            digest: identity.digest.clone(),
        })
        .collect::<Vec<_>>();
    let file = CovxFile {
        header: CovxHeaderV1::new(
            accelerator_id,
            u32::try_from(referenced_files.len())
                .map_err(|_| CoveError::BadSection("too many COVX inputs".into()))?,
            created_at_us(),
        ),
        referenced_files,
        postscript: empty_covx_postscript(),
    };
    let bytes = file.serialize()?;
    let validation_result = CovxFile::parse(&bytes).is_ok();
    let digest = compute_digest(DigestAlgorithm::Sha256, &bytes)?;
    let report = UtilityArtifactReport {
        tool: "cove-build-covx".into(),
        inputs: input_json(&identities),
        output: output.as_ref().display().to_string(),
        file_id: accelerator_id,
        file_len: bytes.len() as u64,
        footer_crc32c: None,
        digest,
        validation_result,
    };
    Ok((bytes, report))
}

pub fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn input_identities(inputs: &[PathBuf]) -> Result<Vec<InputIdentity>, CoveError> {
    if inputs.is_empty() {
        return Err(CoveError::BadSection(
            "expected at least one COVE input".into(),
        ));
    }
    inputs
        .iter()
        .map(|path| input_identity(path.as_path()))
        .collect()
}

fn input_identity(path: &Path) -> Result<InputIdentity, CoveError> {
    let bytes = fs::read(path)?;
    let report = validate_bytes_with_options(
        &bytes,
        ValidationOptions {
            semantic: true,
            verify_digests: false,
            ..ValidationOptions::default()
        },
    )?;
    let digest = compute_digest(DigestAlgorithm::Sha256, &bytes)?;
    let (row_count, segment_count) = table_shape(&bytes, &report.validated.footer)?;
    Ok(InputIdentity {
        path: path.to_path_buf(),
        file_id: report.validated.header.file_id,
        file_len: bytes.len() as u64,
        footer_crc32c: report.validated.postscript.footer.crc32c,
        digest,
        row_count,
        segment_count,
    })
}

fn table_shape(bytes: &[u8], footer: &crate::footer::CoveFooter) -> Result<(u64, u32), CoveError> {
    let mut row_count = 0u64;
    for entry in footer
        .sections
        .iter()
        .filter(|entry| entry.section_kind == SectionKind::TableCatalog as u16)
    {
        let payload = compression::section_payload(bytes, entry)?;
        let catalog = TableCatalog::parse(&payload)?;
        row_count = row_count
            .checked_add(
                catalog
                    .tables
                    .iter()
                    .map(|table| table.row_count)
                    .sum::<u64>(),
            )
            .ok_or(CoveError::ArithOverflow)?;
    }

    let mut segment_count = 0u32;
    for entry in footer
        .sections
        .iter()
        .filter(|entry| entry.section_kind == SectionKind::TableSegmentIndex as u16)
    {
        let payload = compression::section_payload(bytes, entry)?;
        let index = TableSegmentIndex::parse(&payload)?;
        segment_count = segment_count
            .checked_add(
                u32::try_from(index.entries.len())
                    .map_err(|_| CoveError::BadSection("too many segment entries".into()))?,
            )
            .ok_or(CoveError::ArithOverflow)?;
    }
    Ok((row_count, segment_count))
}

fn artifact_id(identities: &[InputIdentity]) -> Result<[u8; 16], CoveError> {
    let mut seed = Vec::new();
    for identity in identities {
        seed.extend_from_slice(&identity.file_id);
        seed.extend_from_slice(&identity.file_len.to_le_bytes());
        seed.extend_from_slice(&identity.footer_crc32c.to_le_bytes());
        seed.extend_from_slice(&identity.digest);
    }
    let digest = compute_digest(DigestAlgorithm::Sha256, &seed)?;
    let mut id = [0u8; 16];
    id.copy_from_slice(&digest[..16]);
    Ok(id)
}

fn input_json(identities: &[InputIdentity]) -> Vec<Value> {
    identities
        .iter()
        .map(|identity| {
            json!({
                "path": identity.path.display().to_string(),
                "file_id": hex_encode(&identity.file_id),
                "file_len": identity.file_len,
                "footer_crc32c": identity.footer_crc32c,
                "digest": {
                    "algorithm": "sha256",
                    "hex": hex_encode(&identity.digest),
                },
                "row_count": identity.row_count,
                "segment_count": identity.segment_count,
            })
        })
        .collect()
}

fn created_at_us() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_micros().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn empty_covm_postscript() -> CovmPostscriptV1 {
    CovmPostscriptV1 {
        header_offset: 0,
        header_len: 0,
        entries_offset: 0,
        entries_len: 0,
        file_len: 0,
        flags: 0,
        checksum: 0,
    }
}

fn empty_covx_postscript() -> CovxPostscriptV1 {
    CovxPostscriptV1 {
        header_offset: 0,
        header_len: 0,
        entries_offset: 0,
        entries_len: 0,
        file_len: 0,
        flags: 0,
        checksum: 0,
    }
}
