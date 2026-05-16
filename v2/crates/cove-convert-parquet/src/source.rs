use std::{
    fs,
    io::{Cursor, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Arc,
};

use arrow_csv::reader::{Format as CsvFormat, ReaderBuilder as CsvReaderBuilder};
use arrow_ipc::reader::{FileReader, StreamReader};
use cove_arrow::convert::{
    convert_arrow_record_batches, convert_parquet_bytes, ParquetConversionOptions,
    ParquetConversionResult,
};
use cove_core::{
    checksum, constants::DigestAlgorithm, digest::compute_digest, utility::hex_encode,
};
use orc_rust::ArrowReaderBuilder;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceFormat {
    Parquet,
    ArrowIpc,
    Csv,
    Orc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsvReadOptions {
    pub has_header: bool,
    pub delimiter: u8,
    pub infer_rows: Option<usize>,
    pub batch_size: usize,
    pub allow_truncated_rows: bool,
}

impl Default for CsvReadOptions {
    fn default() -> Self {
        Self {
            has_header: true,
            delimiter: b',',
            infer_rows: Some(1024),
            batch_size: 4096,
            allow_truncated_rows: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConversionOptions {
    pub source_format: Option<SourceFormat>,
    pub cove: ParquetConversionOptions,
    pub csv: CsvReadOptions,
}

pub fn convert_file_to_cove(
    path: impl AsRef<Path>,
    options: ConversionOptions,
) -> Result<ParquetConversionResult, String> {
    let path = path.as_ref();
    let bytes = fs::read(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    let format = match options.source_format {
        Some(format) => format,
        None => detect_source_format(path)?,
    };
    convert_bytes_to_cove(path.display().to_string(), &bytes, format, options)
}

pub fn convert_bytes_to_cove(
    source_id: impl Into<String>,
    bytes: &[u8],
    format: SourceFormat,
    mut options: ConversionOptions,
) -> Result<ParquetConversionResult, String> {
    let source_id = source_id.into();
    options.source_format = Some(format);
    let cove_options = conversion_options_for_source(&source_id, bytes, format, options.cove)?;
    match format {
        SourceFormat::Parquet => {
            convert_parquet_bytes(bytes, &cove_options).map_err(|err| err.to_string())
        }
        SourceFormat::ArrowIpc => {
            let (schema, batches) = read_arrow_batches_from_bytes(bytes)?;
            convert_arrow_record_batches(
                "arrow-ipc",
                schema_fingerprint(&schema),
                schema,
                batches,
                &cove_options,
            )
            .map_err(|err| err.to_string())
        }
        SourceFormat::Csv => {
            let (schema, batches) = read_csv_batches_from_bytes(bytes, &options.csv)?;
            convert_arrow_record_batches(
                "csv",
                schema_fingerprint(&schema),
                schema,
                batches,
                &cove_options,
            )
            .map_err(|err| err.to_string())
        }
        SourceFormat::Orc => {
            let (schema, batches) = read_orc_batches_from_bytes(bytes)?;
            convert_arrow_record_batches(
                "orc",
                schema_fingerprint(&schema),
                schema,
                batches,
                &cove_options,
            )
            .map_err(|err| err.to_string())
        }
    }
}

pub fn detect_source_format(path: impl AsRef<Path>) -> Result<SourceFormat, String> {
    match path
        .as_ref()
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("parquet") => Ok(SourceFormat::Parquet),
        Some("arrow" | "ipc" | "feather") => Ok(SourceFormat::ArrowIpc),
        Some("csv") => Ok(SourceFormat::Csv),
        Some("orc") => Ok(SourceFormat::Orc),
        _ => Err("cannot detect source format; pass source_format explicitly".into()),
    }
}

pub fn read_arrow_batches(
    path: impl AsRef<Path>,
) -> Result<(arrow_schema::SchemaRef, Vec<arrow_array::RecordBatch>), String> {
    let bytes = fs::read(path.as_ref())
        .map_err(|err| format!("cannot read {}: {err}", path.as_ref().display()))?;
    read_arrow_batches_from_bytes(&bytes)
}

pub fn read_csv_batches(
    path: impl AsRef<Path>,
    options: &CsvReadOptions,
) -> Result<(arrow_schema::SchemaRef, Vec<arrow_array::RecordBatch>), String> {
    let bytes = fs::read(path.as_ref())
        .map_err(|err| format!("cannot read {}: {err}", path.as_ref().display()))?;
    read_csv_batches_from_bytes(&bytes, options)
}

pub fn read_orc_batches(
    path: impl AsRef<Path>,
) -> Result<(arrow_schema::SchemaRef, Vec<arrow_array::RecordBatch>), String> {
    let bytes = fs::read(path.as_ref())
        .map_err(|err| format!("cannot read {}: {err}", path.as_ref().display()))?;
    read_orc_batches_from_bytes(&bytes)
}

pub fn source_digest(bytes: &[u8]) -> Result<String, String> {
    compute_digest(DigestAlgorithm::Sha256, bytes)
        .map(|digest| format!("sha256:{}", hex_encode(&digest)))
        .map_err(|err| err.to_string())
}

pub fn schema_fingerprint(schema: &arrow_schema::SchemaRef) -> String {
    format!(
        "crc32c:{:08x}",
        checksum::crc32c(format!("{schema:?}").as_bytes())
    )
}

fn conversion_options_for_source(
    source_id: &str,
    bytes: &[u8],
    format: SourceFormat,
    mut cove: ParquetConversionOptions,
) -> Result<ParquetConversionOptions, String> {
    let fallback = default_table_name(format);
    if cove.table_name == "parquet_import" || cove.table_name == fallback {
        cove.table_name = table_name_from_source(source_id).unwrap_or_else(|| fallback.to_string());
    }
    cove.source_identifier = Some(source_id.to_string());
    cove.source_digest = Some(source_digest(bytes)?);
    Ok(cove)
}

fn default_table_name(format: SourceFormat) -> &'static str {
    match format {
        SourceFormat::Parquet => "parquet_import",
        SourceFormat::ArrowIpc => "arrow_import",
        SourceFormat::Csv => "csv_import",
        SourceFormat::Orc => "orc_import",
    }
}

fn table_name_from_source(source_id: &str) -> Option<String> {
    let stem = PathBuf::from(source_id)
        .file_stem()
        .and_then(|name| name.to_str())
        .map(str::to_string)?;
    if stem.is_empty() {
        None
    } else {
        Some(stem)
    }
}

fn read_arrow_batches_from_bytes(
    bytes: &[u8],
) -> Result<(arrow_schema::SchemaRef, Vec<arrow_array::RecordBatch>), String> {
    if let Ok(reader) = FileReader::try_new(Cursor::new(bytes.to_vec()), None) {
        let schema = reader.schema();
        let batches = reader
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| format!("cannot read Arrow IPC file: {err}"))?;
        return Ok((schema, batches));
    }

    let reader = StreamReader::try_new(Cursor::new(bytes.to_vec()), None)
        .map_err(|err| format!("cannot read Arrow IPC file or stream: {err}"))?;
    let schema = reader.schema();
    let batches = reader
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("cannot read Arrow IPC stream: {err}"))?;
    Ok((schema, batches))
}

fn read_csv_batches_from_bytes(
    bytes: &[u8],
    options: &CsvReadOptions,
) -> Result<(arrow_schema::SchemaRef, Vec<arrow_array::RecordBatch>), String> {
    let mut cursor = Cursor::new(bytes.to_vec());
    let format = CsvFormat::default()
        .with_header(options.has_header)
        .with_delimiter(options.delimiter)
        .with_truncated_rows(options.allow_truncated_rows);
    let (schema, _) = format
        .infer_schema(&mut cursor, options.infer_rows)
        .map_err(|err| format!("cannot infer CSV schema: {err}"))?;
    let schema = Arc::new(schema);
    cursor
        .seek(SeekFrom::Start(0))
        .map_err(|err| format!("cannot rewind CSV bytes: {err}"))?;
    let reader = CsvReaderBuilder::new(schema.clone())
        .with_format(format)
        .with_batch_size(options.batch_size)
        .build(cursor)
        .map_err(|err| format!("cannot read CSV: {err}"))?;
    let batches = reader
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("cannot decode CSV: {err}"))?;
    Ok((schema, batches))
}

fn read_orc_batches_from_bytes(
    bytes: &[u8],
) -> Result<(arrow_schema::SchemaRef, Vec<arrow_array::RecordBatch>), String> {
    let builder = ArrowReaderBuilder::try_new(bytes::Bytes::copy_from_slice(bytes))
        .map_err(|err| format!("cannot open ORC source: {err}"))?;
    let schema = builder.schema().clone();
    let reader = builder.with_batch_size(4096).build();
    let batches = reader
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("cannot read ORC batches: {err}"))?;
    Ok((schema, batches))
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{Int64Array, RecordBatch, StringArray};
    use arrow_ipc::writer::FileWriter as IpcFileWriter;
    use parquet::arrow::ArrowWriter;

    #[test]
    fn converts_supported_source_formats_through_facade() {
        let batch = test_batch();
        let cases = [
            (
                SourceFormat::Parquet,
                "people.parquet",
                write_parquet(&batch),
            ),
            (SourceFormat::ArrowIpc, "people.arrow", write_arrow(&batch)),
            (
                SourceFormat::Csv,
                "people.csv",
                b"id,name\n1,Ada\n2,Linus\n".to_vec(),
            ),
            (SourceFormat::Orc, "people.orc", write_orc(&batch)),
        ];
        for (format, source_id, bytes) in cases {
            let result =
                convert_bytes_to_cove(source_id, &bytes, format, ConversionOptions::default())
                    .unwrap();
            assert!(!result.cove_bytes.is_empty());
            assert!(result.report.validation_result);
            assert_eq!(result.report.source_identifier, source_id);
            assert!(result.report.source_digest.starts_with("sha256:"));
            assert_eq!(result.report.row_count, 2);
        }
    }

    fn test_batch() -> RecordBatch {
        RecordBatch::try_from_iter(vec![
            (
                "id",
                Arc::new(Int64Array::from(vec![1, 2])) as arrow_array::ArrayRef,
            ),
            (
                "name",
                Arc::new(StringArray::from(vec!["Ada", "Linus"])) as arrow_array::ArrayRef,
            ),
        ])
        .unwrap()
    }

    fn write_arrow(batch: &RecordBatch) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut writer = IpcFileWriter::try_new(&mut bytes, &batch.schema()).unwrap();
            writer.write(batch).unwrap();
            writer.finish().unwrap();
        }
        bytes
    }

    fn write_parquet(batch: &RecordBatch) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut writer = ArrowWriter::try_new(&mut bytes, batch.schema(), None).unwrap();
            writer.write(batch).unwrap();
            writer.close().unwrap();
        }
        bytes
    }

    fn write_orc(batch: &RecordBatch) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut writer = orc_rust::ArrowWriterBuilder::new(&mut bytes, batch.schema())
                .try_build()
                .unwrap();
            writer.write(batch).unwrap();
            writer.close().unwrap();
        }
        bytes
    }
}
