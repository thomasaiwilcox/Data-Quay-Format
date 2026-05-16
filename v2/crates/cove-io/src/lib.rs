//! Stable Rust facade for COVE v2 I/O.
//!
//! This crate gives downstream Rust users one stable import surface for the
//! reference reader, writer, artifact, validation, and conversion APIs.

use std::{fs, path::Path};

pub use cove_convert as convert;
pub use cove_core::{
    artifact,
    constants::{self, DigestAlgorithm},
    digest::compute_digest,
    footer, header,
    mount::{self, MountOptions, MountedCoveFile},
    postscript, profile,
    reader::{self, ValidatedCoveFile},
    table,
    utility::hex_encode,
    writer::{self, ScanProfileCoveWriter},
    CoveError,
};
pub use cove_reader as read;
pub use cove_writer as write;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInspection {
    pub version_major: u16,
    pub version_minor: u16,
    pub primary_profile: u8,
    pub required_features: u64,
    pub optional_features: u64,
    pub section_count: usize,
    pub table_count: usize,
    pub row_count: u64,
}

pub fn validate_file(path: impl AsRef<Path>) -> Result<ValidatedCoveFile, CoveError> {
    reader::read_file(path.as_ref())
}

pub fn inspect_file(path: impl AsRef<Path>) -> Result<FileInspection, CoveError> {
    let bytes = fs::read(path)?;
    let mounted = mount::mount_cove_file(&bytes, MountOptions::default(), None)?;
    Ok(FileInspection {
        version_major: mounted.header.version_major,
        version_minor: mounted.header.version_minor,
        primary_profile: mounted.header.primary_profile,
        required_features: mounted.header.required_features,
        optional_features: mounted.header.optional_features,
        section_count: mounted.footer.sections.len(),
        table_count: mounted.tables.len(),
        row_count: mounted.tables.iter().map(|table| table.row_count).sum(),
    })
}

pub fn read_table(path: impl AsRef<Path>) -> Result<MountedCoveFile, CoveError> {
    let bytes = fs::read(path)?;
    mount::mount_cove_file(&bytes, MountOptions::default(), None)
}

pub fn write_table(
    path: impl AsRef<Path>,
    writer: &ScanProfileCoveWriter,
) -> Result<(), CoveError> {
    let bytes = writer.write()?;
    fs::write(path, bytes).map_err(CoveError::from)
}

pub fn convert_file(
    input: impl AsRef<Path>,
    options: convert::ConversionOptions,
) -> Result<convert::ConversionResult, CoveError> {
    convert::convert_file_to_cove(input, options).map_err(CoveError::BadSchema)
}

pub fn convert_parquet_file(
    input: impl AsRef<Path>,
    mut options: convert::convert::ParquetConversionOptions,
) -> Result<convert::ConversionResult, CoveError> {
    let input = input.as_ref();
    let bytes = fs::read(input)?;
    options.source_identifier = Some(input.display().to_string());
    options.source_digest = Some(format!(
        "sha256:{}",
        hex_encode(&compute_digest(DigestAlgorithm::Sha256, &bytes)?)
    ));
    convert::convert::convert_parquet_bytes(&bytes, &options)
}

pub fn conversion_report(result: &convert::convert::ParquetConversionResult) -> serde_json::Value {
    result.report.to_json_value()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facade_exposes_core_header_constants() {
        assert_eq!(constants::VERSION_MAJOR_V1, 2);
    }

    #[test]
    fn facade_validates_and_inspects_written_file() {
        let path = std::env::temp_dir().join("cove_io_facade_empty.cove");
        std::fs::write(
            &path,
            writer::MinimalCoveWriter::write_empty_file().unwrap(),
        )
        .unwrap();
        let validated = validate_file(&path).unwrap();
        assert_eq!(validated.header.version_major, 2);
        let inspection = inspect_file(&path).unwrap();
        assert_eq!(inspection.section_count, 0);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn facade_converts_csv_through_generic_convert_file() {
        let path = std::env::temp_dir().join("cove_io_facade_people.csv");
        std::fs::write(&path, "id,name\n1,Ada\n2,Linus\n").unwrap();
        let result = convert_file(
            &path,
            convert::ConversionOptions {
                source_format: Some(convert::SourceFormat::Csv),
                ..convert::ConversionOptions::default()
            },
        )
        .unwrap();
        assert!(result.report.validation_result);
        assert_eq!(result.report.source_identifier, path.display().to_string());
        assert!(result.report.source_digest.starts_with("sha256:"));
        assert_eq!(result.report.row_count, 2);
        let _ = std::fs::remove_file(path);
    }
}
