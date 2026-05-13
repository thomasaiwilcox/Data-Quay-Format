use std::{env, fs, fs::File, path::PathBuf, process::ExitCode};

use arrow_ipc::reader::{FileReader, StreamReader};
use cove_arrow::convert::{
    convert_arrow_record_batches, convert_parquet_bytes, ParquetConversionOptions,
    ParquetConversionResult,
};
use cove_core::checksum;
use orc_rust::ArrowReaderBuilder;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceFormat {
    Parquet,
    Arrow,
    Orc,
}

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("cove-conversion-report: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let Some((format, input)) = parse_args(args)? else {
        print_usage();
        return Ok(());
    };
    let result = match format {
        SourceFormat::Parquet => {
            let bytes = fs::read(&input)
                .map_err(|err| format!("cannot read {}: {err}", input.display()))?;
            convert_parquet_bytes(&bytes, &conversion_options(&input, "source"))
                .map_err(|err| err.to_string())?
        }
        SourceFormat::Arrow => {
            let (schema, batches) = read_arrow_batches(&input)?;
            convert_arrow_record_batches(
                "arrow-ipc",
                schema_fingerprint(&schema),
                schema,
                batches,
                &conversion_options(&input, "arrow_import"),
            )
            .map_err(|err| err.to_string())?
        }
        SourceFormat::Orc => convert_orc(&input)?,
    };
    validate_report_result(&result)?;
    let json = serde_json::to_string_pretty(&result.report.to_json_value())
        .map_err(|err| format!("cannot serialize conversion report: {err}"))?;
    println!("{json}");
    Ok(())
}

fn parse_args(args: Vec<String>) -> Result<Option<(SourceFormat, PathBuf)>, String> {
    let mut source_format = None;
    let mut input = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--source-format" => {
                let raw = iter
                    .next()
                    .ok_or_else(|| "--source-format requires a value".to_string())?;
                source_format = Some(parse_source_format(&raw)?);
            }
            _ if arg.starts_with('-') => return Err(format!("unknown option {arg}")),
            _ => {
                if input.replace(PathBuf::from(arg)).is_some() {
                    return Err("expected a single input file".into());
                }
            }
        }
    }
    let input = input.ok_or_else(|| "expected an input file".to_string())?;
    let format = match source_format {
        Some(format) => format,
        None => detect_source_format(&input)?,
    };
    Ok(Some((format, input)))
}

fn parse_source_format(raw: &str) -> Result<SourceFormat, String> {
    match raw {
        "parquet" => Ok(SourceFormat::Parquet),
        "arrow" | "ipc" | "feather" => Ok(SourceFormat::Arrow),
        "orc" => Ok(SourceFormat::Orc),
        _ => Err("--source-format must be one of: parquet, arrow, orc".into()),
    }
}

fn detect_source_format(input: &PathBuf) -> Result<SourceFormat, String> {
    match input
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("parquet") => Ok(SourceFormat::Parquet),
        Some("arrow" | "ipc" | "feather") => Ok(SourceFormat::Arrow),
        Some("orc") => Ok(SourceFormat::Orc),
        _ => Err("cannot detect source format; pass --source-format".into()),
    }
}

fn conversion_options(input: &PathBuf, fallback: &str) -> ParquetConversionOptions {
    ParquetConversionOptions {
        table_name: input
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or(fallback)
            .to_string(),
        namespace: "interop".into(),
        ..ParquetConversionOptions::default()
    }
}

fn read_arrow_batches(
    path: &PathBuf,
) -> Result<(arrow_schema::SchemaRef, Vec<arrow_array::RecordBatch>), String> {
    let file = File::open(path).map_err(|err| format!("cannot open {}: {err}", path.display()))?;
    if let Ok(reader) = FileReader::try_new(file, None) {
        let schema = reader.schema();
        let batches = reader
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| format!("cannot read Arrow IPC file: {err}"))?;
        return Ok((schema, batches));
    }

    let file = File::open(path).map_err(|err| format!("cannot open {}: {err}", path.display()))?;
    let reader = StreamReader::try_new(file, None)
        .map_err(|err| format!("cannot read Arrow IPC file or stream: {err}"))?;
    let schema = reader.schema();
    let batches = reader
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("cannot read Arrow IPC stream: {err}"))?;
    Ok((schema, batches))
}

fn convert_orc(path: &PathBuf) -> Result<ParquetConversionResult, String> {
    let file = File::open(path).map_err(|err| format!("cannot open {}: {err}", path.display()))?;
    let builder = ArrowReaderBuilder::try_new(file)
        .map_err(|err| format!("cannot open ORC source {}: {err}", path.display()))?;
    let schema = builder.schema().clone();
    let fingerprint = schema_fingerprint(&schema);
    let reader = builder.with_batch_size(4096).build();
    let batches = reader
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("cannot read ORC batches: {err}"))?;
    convert_arrow_record_batches(
        "orc",
        fingerprint,
        schema,
        batches,
        &conversion_options(path, "orc_import"),
    )
    .map_err(|err| err.to_string())
}

fn schema_fingerprint(schema: &arrow_schema::SchemaRef) -> String {
    format!(
        "crc32c:{:08x}",
        checksum::crc32c(format!("{schema:?}").as_bytes())
    )
}

fn validate_report_result(result: &ParquetConversionResult) -> Result<(), String> {
    if !result.report.validation_result {
        return Err("conversion did not produce a validated COVE file".into());
    }
    if result.cove_bytes.is_empty() {
        return Err("conversion produced empty COVE bytes".into());
    }
    Ok(())
}

fn print_usage() {
    eprintln!(
        "usage: cove-conversion-report [--source-format parquet|arrow|orc] <input.parquet|input.arrow|input.orc>"
    );
}
