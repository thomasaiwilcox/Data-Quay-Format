use std::{env, fs, path::PathBuf, process::ExitCode, sync::Arc};

use arrow_csv::WriterBuilder as CsvWriterBuilder;
use arrow_ipc::writer::FileWriter as IpcFileWriter;
use cove_arrow::convert::{ParquetConversionOptions, ParquetConversionResult};
use cove_convert_parquet::cli::source_digest;
use cove_convert_parquet::source::{
    convert_bytes_to_cove, ConversionOptions, SourceFormat as FacadeSourceFormat,
};
use cove_core::{durable, reader};
use cove_datafusion::explain::{execute_planned_scan, plan_local_file, ExplainOptions};
use orc_rust::ArrowWriterBuilder as OrcWriterBuilder;
use parquet::arrow::ArrowWriter;
use serde_json::json;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceFormat {
    Parquet,
    Arrow,
    Csv,
    Orc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    SourceToCove,
    CoveToSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetFormat {
    Arrow,
    Csv,
    Parquet,
    Orc,
    Unspecified,
}

#[derive(Debug, Clone)]
struct ReverseOptions {
    output: Option<PathBuf>,
    csv_header: bool,
    csv_delimiter: u8,
    csv_null: String,
}

impl Default for ReverseOptions {
    fn default() -> Self {
        Self {
            output: None,
            csv_header: true,
            csv_delimiter: b',',
            csv_null: String::new(),
        }
    }
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
    let Some((direction, format, target_format, reverse_options, input)) = parse_args(args)? else {
        print_usage();
        return Ok(());
    };
    if direction == Direction::CoveToSource {
        let json = cove_to_source_report(&input, target_format, &reverse_options)?;
        println!("{json}");
        return Ok(());
    }
    let format =
        format.ok_or_else(|| "--source-format is required for source-to-COVE".to_string())?;
    let bytes =
        fs::read(&input).map_err(|err| format!("cannot read {}: {err}", input.display()))?;
    let result = convert_bytes_to_cove(
        input.display().to_string(),
        &bytes,
        facade_source_format(format),
        ConversionOptions {
            source_format: Some(facade_source_format(format)),
            cove: conversion_options(&input, source_format_default_table(format), &bytes)?,
            ..ConversionOptions::default()
        },
    )?;
    validate_report_result(&result)?;
    let json = serde_json::to_string_pretty(&result.report.to_json_value())
        .map_err(|err| format!("cannot serialize conversion report: {err}"))?;
    println!("{json}");
    Ok(())
}

fn parse_args(
    args: Vec<String>,
) -> Result<
    Option<(
        Direction,
        Option<SourceFormat>,
        TargetFormat,
        ReverseOptions,
        PathBuf,
    )>,
    String,
> {
    let mut direction = Direction::SourceToCove;
    let mut source_format = None;
    let mut target_format = TargetFormat::Unspecified;
    let mut reverse_options = ReverseOptions::default();
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
            "--direction" => {
                let raw = iter
                    .next()
                    .ok_or_else(|| "--direction requires a value".to_string())?;
                direction = match raw.as_str() {
                    "source-to-cove" => Direction::SourceToCove,
                    "cove-to-source" => Direction::CoveToSource,
                    _ => return Err("--direction must be source-to-cove or cove-to-source".into()),
                };
            }
            "--target-format" => {
                let raw = iter
                    .next()
                    .ok_or_else(|| "--target-format requires a value".to_string())?;
                target_format = parse_target_format(&raw)?;
            }
            "--output" | "-o" => {
                reverse_options.output = Some(PathBuf::from(
                    iter.next()
                        .ok_or_else(|| format!("{arg} requires a value"))?,
                ));
            }
            "--csv-header" => reverse_options.csv_header = true,
            "--no-csv-header" => reverse_options.csv_header = false,
            "--csv-delimiter" => {
                reverse_options.csv_delimiter = parse_delimiter(
                    &iter
                        .next()
                        .ok_or_else(|| "--csv-delimiter requires a value".to_string())?,
                )?;
            }
            "--csv-null" => {
                reverse_options.csv_null = iter
                    .next()
                    .ok_or_else(|| "--csv-null requires a value".to_string())?;
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
    let format = match (direction, source_format) {
        (Direction::SourceToCove, Some(format)) => Some(format),
        (Direction::SourceToCove, None) => Some(detect_source_format(&input)?),
        (Direction::CoveToSource, _) => None,
    };
    Ok(Some((
        direction,
        format,
        target_format,
        reverse_options,
        input,
    )))
}

fn parse_source_format(raw: &str) -> Result<SourceFormat, String> {
    match raw {
        "parquet" => Ok(SourceFormat::Parquet),
        "arrow" | "ipc" | "feather" => Ok(SourceFormat::Arrow),
        "csv" => Ok(SourceFormat::Csv),
        "orc" => Ok(SourceFormat::Orc),
        _ => Err("--source-format must be one of: parquet, arrow, csv, orc".into()),
    }
}

fn facade_source_format(format: SourceFormat) -> FacadeSourceFormat {
    match format {
        SourceFormat::Parquet => FacadeSourceFormat::Parquet,
        SourceFormat::Arrow => FacadeSourceFormat::ArrowIpc,
        SourceFormat::Csv => FacadeSourceFormat::Csv,
        SourceFormat::Orc => FacadeSourceFormat::Orc,
    }
}

fn source_format_default_table(format: SourceFormat) -> &'static str {
    match format {
        SourceFormat::Parquet => "source",
        SourceFormat::Arrow => "arrow_import",
        SourceFormat::Csv => "csv_import",
        SourceFormat::Orc => "orc_import",
    }
}

fn parse_target_format(raw: &str) -> Result<TargetFormat, String> {
    match raw {
        "arrow" | "ipc" | "feather" => Ok(TargetFormat::Arrow),
        "csv" => Ok(TargetFormat::Csv),
        "parquet" => Ok(TargetFormat::Parquet),
        "orc" => Ok(TargetFormat::Orc),
        _ => Err("--target-format must be one of: arrow, csv, parquet, orc".into()),
    }
}

fn target_format_name(format: TargetFormat) -> &'static str {
    match format {
        TargetFormat::Arrow => "arrow",
        TargetFormat::Csv => "csv",
        TargetFormat::Parquet => "parquet",
        TargetFormat::Orc => "orc",
        TargetFormat::Unspecified => "unspecified",
    }
}

fn parse_delimiter(raw: &str) -> Result<u8, String> {
    match raw {
        "tab" | "\\t" => Ok(b'\t'),
        value if value.len() == 1 => Ok(value.as_bytes()[0]),
        value => value
            .parse::<u8>()
            .map_err(|_| "--csv-delimiter must be a single byte, byte value, tab, or \\t".into()),
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
        Some("csv") => Ok(SourceFormat::Csv),
        Some("orc") => Ok(SourceFormat::Orc),
        _ => Err("cannot detect source format; pass --source-format".into()),
    }
}

fn conversion_options(
    input: &PathBuf,
    fallback: &str,
    source_bytes: &[u8],
) -> Result<ParquetConversionOptions, String> {
    Ok(ParquetConversionOptions {
        table_name: input
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or(fallback)
            .to_string(),
        namespace: "interop".into(),
        source_identifier: Some(input.display().to_string()),
        source_digest: Some(source_digest(source_bytes)?),
        ..ParquetConversionOptions::default()
    })
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

fn cove_to_source_report(
    input: &PathBuf,
    target_format: TargetFormat,
    options: &ReverseOptions,
) -> Result<String, String> {
    let bytes = fs::read(input).map_err(|err| format!("cannot read {}: {err}", input.display()))?;
    let validated = reader::validate_bytes(&bytes)
        .map_err(|err| format!("cannot validate COVE source {}: {err}", input.display()))?;
    let target_format = if target_format == TargetFormat::Unspecified {
        infer_target_format(options.output.as_ref())
    } else {
        target_format
    };
    let target_name = target_format_name(target_format);
    let mut unsupported_features = Vec::<String>::new();
    let export = match target_format {
        TargetFormat::Arrow | TargetFormat::Csv | TargetFormat::Parquet | TargetFormat::Orc => {
            let output = options.output.as_ref().ok_or_else(|| {
                "--output is required for COVE-to-arrow, COVE-to-csv, COVE-to-parquet, and COVE-to-orc exports"
                    .to_string()
            })?;
            match export_cove_t(input, target_format, options) {
                Ok(export) => Some((output.display().to_string(), export)),
                Err(err)
                    if target_format == TargetFormat::Orc
                        && err.starts_with("unsupported ORC export:") =>
                {
                    unsupported_features.push(err);
                    None
                }
                Err(err) => return Err(err),
            }
        }
        TargetFormat::Unspecified => None,
    };
    let supported = export.is_some();
    let report = json!({
        "direction": "cove-to-source",
        "source_format": "cove",
        "source_identifier": input.display().to_string(),
        "source_digest": source_digest(&bytes)?,
        "target_format": target_name,
        "conversion_policy_version": "cove-reference-v2.0",
        "validation_result": true,
        "timestamp_policy": "preserve-cove-logical-timestamps",
        "timezone_policy": "preserve-cove-timezone-annotations",
        "decimal_policy": "preserve-cove-decimal-precision-scale",
        "collation_policy": "preserve-cove-collation-metadata-where-representable",
        "canonicalization_policy": "preserve-cove-canonical-values",
        "row_reordering_policy": "preserve-source-order",
        "required_features": validated.header.required_features,
        "optional_features": validated.header.optional_features,
        "supported": supported,
        "output": export.as_ref().map(|(output, _)| output),
        "rows": export.as_ref().map(|(_, export)| export.rows),
        "columns": export.as_ref().map(|(_, export)| export.columns),
        "artifact_choices": export.as_ref().map(|(_, export)| export.artifact_choices.clone()).unwrap_or_else(Vec::new),
        "unsupported_features": if supported {
            Vec::<String>::new()
        } else if !unsupported_features.is_empty() {
            unsupported_features
        } else {
            vec![format!("COVE-to-{target_name} exporter is not implemented in this reference build")]
        },
        "notes": export.as_ref().map(|(_, export)| export.notes.clone()).unwrap_or_else(Vec::new),
    });
    serde_json::to_string_pretty(&report)
        .map_err(|err| format!("cannot serialize conversion report: {err}"))
}

#[derive(Debug, Clone)]
struct ReverseExportReport {
    rows: usize,
    columns: usize,
    artifact_choices: Vec<String>,
    notes: Vec<String>,
}

fn infer_target_format(output: Option<&PathBuf>) -> TargetFormat {
    match output
        .and_then(|path| path.extension())
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("arrow" | "ipc" | "feather") => TargetFormat::Arrow,
        Some("csv") => TargetFormat::Csv,
        Some("parquet") => TargetFormat::Parquet,
        Some("orc") => TargetFormat::Orc,
        _ => TargetFormat::Unspecified,
    }
}

fn export_cove_t(
    input: &PathBuf,
    target_format: TargetFormat,
    options: &ReverseOptions,
) -> Result<ReverseExportReport, String> {
    let output = options
        .output
        .as_ref()
        .ok_or_else(|| "reverse export requires --output".to_string())?;
    let planned =
        plan_local_file(input, ExplainOptions::default()).map_err(|err| err.to_string())?;
    let decoded = execute_planned_scan(&planned).map_err(|err| err.to_string())?;
    let schema = Arc::clone(&planned.plan.output_schema);
    let bytes = match target_format {
        TargetFormat::Arrow => write_arrow_ipc(&schema, &decoded.batches)?,
        TargetFormat::Csv => write_csv(&decoded.batches, options)?,
        TargetFormat::Parquet => write_parquet(&schema, &decoded.batches)?,
        TargetFormat::Orc => write_orc(&schema, &decoded.batches)?,
        TargetFormat::Unspecified => unreachable!("checked by caller"),
    };
    durable::durable_replace(output, &bytes)
        .map_err(|err| format!("cannot durably publish {}: {err}", output.display()))?;
    Ok(ReverseExportReport {
        rows: decoded.stats.rows_materialized,
        columns: schema.fields().len(),
        artifact_choices: vec![match target_format {
            TargetFormat::Arrow => "arrow-ipc-file".to_string(),
            TargetFormat::Csv => "csv-rfc4180-arrow-writer".to_string(),
            TargetFormat::Parquet => "parquet-arrow-writer".to_string(),
            TargetFormat::Orc => "orc-rust-arrow-writer".to_string(),
            TargetFormat::Unspecified => unreachable!(),
        }],
        notes: Vec::new(),
    })
}

fn write_arrow_ipc(
    schema: &arrow_schema::SchemaRef,
    batches: &[arrow_array::RecordBatch],
) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    {
        let mut writer = IpcFileWriter::try_new(&mut bytes, schema)
            .map_err(|err| format!("IPC writer: {err}"))?;
        for batch in batches {
            writer
                .write(batch)
                .map_err(|err| format!("cannot write Arrow IPC batch: {err}"))?;
        }
        writer
            .finish()
            .map_err(|err| format!("cannot finish Arrow IPC writer: {err}"))?;
    }
    Ok(bytes)
}

fn write_csv(
    batches: &[arrow_array::RecordBatch],
    options: &ReverseOptions,
) -> Result<Vec<u8>, String> {
    let mut writer = CsvWriterBuilder::new()
        .with_header(options.csv_header)
        .with_delimiter(options.csv_delimiter)
        .with_null(options.csv_null.clone())
        .build(Vec::new());
    for batch in batches {
        writer
            .write(batch)
            .map_err(|err| format!("cannot write CSV batch: {err}"))?;
    }
    Ok(writer.into_inner())
}

fn write_parquet(
    schema: &arrow_schema::SchemaRef,
    batches: &[arrow_array::RecordBatch],
) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    {
        let mut writer = ArrowWriter::try_new(&mut bytes, Arc::clone(schema), None)
            .map_err(|err| format!("cannot open Parquet writer: {err}"))?;
        for batch in batches {
            writer
                .write(batch)
                .map_err(|err| format!("cannot write Parquet batch: {err}"))?;
        }
        writer
            .close()
            .map_err(|err| format!("cannot finish Parquet writer: {err}"))?;
    }
    Ok(bytes)
}

fn write_orc(
    schema: &arrow_schema::SchemaRef,
    batches: &[arrow_array::RecordBatch],
) -> Result<Vec<u8>, String> {
    validate_orc_schema(schema)?;
    let mut bytes = Vec::new();
    {
        let mut writer = OrcWriterBuilder::new(&mut bytes, Arc::clone(schema))
            .try_build()
            .map_err(|err| format!("cannot open ORC writer: {err}"))?;
        for batch in batches {
            writer
                .write(batch)
                .map_err(|err| format!("cannot write ORC batch: {err}"))?;
        }
        writer
            .close()
            .map_err(|err| format!("cannot finish ORC writer: {err}"))?;
    }
    Ok(bytes)
}

fn validate_orc_schema(schema: &arrow_schema::SchemaRef) -> Result<(), String> {
    for field in schema.fields() {
        validate_orc_field(field, field.name())?;
    }
    Ok(())
}

fn validate_orc_field(field: &arrow_schema::FieldRef, path: &str) -> Result<(), String> {
    use arrow_schema::{DataType, TimeUnit};

    match field.data_type() {
        DataType::Boolean
        | DataType::Int8
        | DataType::Int16
        | DataType::Int32
        | DataType::Int64
        | DataType::UInt8
        | DataType::UInt16
        | DataType::UInt32
        | DataType::UInt64
        | DataType::Float32
        | DataType::Float64
        | DataType::Utf8
        | DataType::LargeUtf8
        | DataType::Binary
        | DataType::LargeBinary
        | DataType::FixedSizeBinary(_)
        | DataType::Decimal128(_, _)
        | DataType::Date32 => Ok(()),
        DataType::Timestamp(
            TimeUnit::Second | TimeUnit::Millisecond | TimeUnit::Microsecond | TimeUnit::Nanosecond,
            None,
        ) => Ok(()),
        DataType::Timestamp(
            TimeUnit::Second | TimeUnit::Millisecond | TimeUnit::Microsecond | TimeUnit::Nanosecond,
            Some(tz),
        ) if tz.as_ref() == "UTC" => Ok(()),
        DataType::Struct(fields) => {
            for child in fields {
                validate_orc_field(child, &format!("{path}.{}", child.name()))?;
            }
            Ok(())
        }
        DataType::List(child) => validate_orc_field(child, &format!("{path}[]")),
        DataType::Map(entries, _) => {
            let DataType::Struct(fields) = entries.data_type() else {
                return Err(format!(
                    "unsupported ORC export: map field '{path}' does not use struct entries"
                ));
            };
            if fields.len() != 2 {
                return Err(format!(
                    "unsupported ORC export: map field '{path}' must contain key and value fields"
                ));
            }
            validate_orc_field(&fields[0], &format!("{path}.key"))?;
            validate_orc_field(&fields[1], &format!("{path}.value"))
        }
        other => Err(format!(
            "unsupported ORC export: field '{path}' has unsupported Arrow type {other:?}"
        )),
    }
}

fn print_usage() {
    eprintln!(
        "usage: cove-conversion-report [--direction source-to-cove|cove-to-source] [--source-format parquet|arrow|orc] [--target-format parquet|arrow|csv|orc] [--output path] [--csv-header|--no-csv-header] [--csv-delimiter byte|tab|\\t] [--csv-null text] <input>"
    );
}
