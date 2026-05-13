use std::{env, path::PathBuf, process::ExitCode};

use arrow_ipc::writer::FileWriter;
use arrow_json::writer::{LineDelimited, WriterBuilder};
use cove_core::durable;
use cove_datafusion::explain::{
    execute_planned_scan, parse_filter_dsl, parse_projection_dsl, plan_local_file, ExplainOptions,
};
use serde_json::json;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Ipc,
    Json,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReportTarget {
    Stdout,
    Path(PathBuf),
}

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("cove-arrow-export: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let Some((input, output, options, format, report)) = parse_args(args)? else {
        print_usage();
        return Ok(());
    };
    let planned = plan_local_file(&input, options).map_err(|err| err.to_string())?;
    let decoded = execute_planned_scan(&planned).map_err(|err| err.to_string())?;
    let bytes = match format {
        OutputFormat::Ipc => write_ipc(&planned.plan.output_schema, &decoded.batches)?,
        OutputFormat::Json => write_json(&decoded.batches)?,
    };
    durable::durable_replace(&output, &bytes)
        .map_err(|err| format!("cannot durably publish {}: {err}", output.display()))?;

    let report_json = json!({
        "version": 1,
        "source": planned.state.source(),
        "output": output.display().to_string(),
        "format": match format { OutputFormat::Ipc => "ipc", OutputFormat::Json => "json" },
        "batches": decoded.batches.len(),
        "rows": decoded.stats.rows_materialized,
        "columns": planned.plan.output_schema.fields().len(),
        "decode_stats": {
            "metadata_bytes_read": decoded.stats.metadata_bytes_read,
            "data_bytes_read": decoded.stats.data_bytes_read,
            "range_requests": decoded.stats.range_requests,
            "pages_decoded": decoded.stats.pages_decoded,
            "rows_selected": decoded.stats.rows_selected,
            "rows_materialized": decoded.stats.rows_materialized,
        }
    });
    if let Some(target) = report {
        let text = serde_json::to_string_pretty(&report_json)
            .map_err(|err| format!("cannot serialize export report: {err}"))?;
        match target {
            ReportTarget::Stdout => println!("{text}"),
            ReportTarget::Path(path) => std::fs::write(&path, text)
                .map_err(|err| format!("cannot write {}: {err}", path.display()))?,
        }
    }
    Ok(())
}

fn parse_args(
    args: Vec<String>,
) -> Result<
    Option<(
        PathBuf,
        PathBuf,
        ExplainOptions,
        OutputFormat,
        Option<ReportTarget>,
    )>,
    String,
> {
    let mut options = ExplainOptions::default();
    let mut format = OutputFormat::Ipc;
    let mut report = None;
    let mut positional = Vec::new();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--columns" | "--projection" => {
                let raw = next_value(&mut iter, &arg)?;
                options.projection = Some(parse_projection_dsl(&raw));
            }
            "--filter" => {
                let raw = next_value(&mut iter, "--filter")?;
                options
                    .filters
                    .push(parse_filter_dsl(&raw).map_err(|err| err.to_string())?);
            }
            "--format" => {
                format = parse_format(&next_value(&mut iter, "--format")?)?;
            }
            "--report" => {
                let raw = next_value(&mut iter, "--report")?;
                report = Some(if raw == "-" {
                    ReportTarget::Stdout
                } else {
                    ReportTarget::Path(PathBuf::from(raw))
                });
            }
            _ if arg.starts_with('-') => return Err(format!("unknown option {arg}")),
            _ => positional.push(PathBuf::from(arg)),
        }
    }
    if positional.len() != 2 {
        return Err("expected <input.cove> and <output.arrow|output.json>".into());
    }
    Ok(Some((
        positional.remove(0),
        positional.remove(0),
        options,
        format,
        report,
    )))
}

fn write_ipc(
    schema: &arrow_schema::SchemaRef,
    batches: &[arrow_array::RecordBatch],
) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    {
        let mut writer =
            FileWriter::try_new(&mut bytes, schema).map_err(|err| format!("IPC writer: {err}"))?;
        for batch in batches {
            writer
                .write(batch)
                .map_err(|err| format!("cannot write IPC batch: {err}"))?;
        }
        writer
            .finish()
            .map_err(|err| format!("cannot finish IPC writer: {err}"))?;
    }
    Ok(bytes)
}

fn write_json(batches: &[arrow_array::RecordBatch]) -> Result<Vec<u8>, String> {
    let mut writer = WriterBuilder::new()
        .with_explicit_nulls(true)
        .build::<_, LineDelimited>(Vec::new());
    for batch in batches {
        writer
            .write(batch)
            .map_err(|err| format!("cannot write JSON batch: {err}"))?;
    }
    writer
        .finish()
        .map_err(|err| format!("cannot finish JSON writer: {err}"))?;
    Ok(writer.into_inner())
}

fn parse_format(raw: &str) -> Result<OutputFormat, String> {
    match raw {
        "ipc" => Ok(OutputFormat::Ipc),
        "json" => Ok(OutputFormat::Json),
        _ => Err("--format must be ipc or json".into()),
    }
}

fn next_value(iter: &mut impl Iterator<Item = String>, option: &str) -> Result<String, String> {
    iter.next()
        .ok_or_else(|| format!("{option} requires a value"))
}

fn print_usage() {
    eprintln!(
        "usage: cove-arrow-export [--columns a,b] [--filter column=<name|index>,op=<eq|lt|lte|gt|gte|is-null|is-not-null>,value=<literal>] [--format ipc|json] [--report -|path] <input.cove> <output.arrow|output.json>"
    );
}
