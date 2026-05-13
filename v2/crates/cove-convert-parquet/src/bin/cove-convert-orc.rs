use std::{env, fs::File, path::PathBuf, process::ExitCode};

use cove_arrow::convert::{convert_arrow_record_batches, ParquetConversionOptions};
use cove_core::{checksum, durable};
use orc_rust::ArrowReaderBuilder;

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("cove-convert-orc: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    if args.len() != 2 || args.iter().any(|arg| arg == "-h" || arg == "--help") {
        eprintln!("usage: cove-convert-orc <input.orc> <output.cove>");
        return Ok(());
    }
    let input = PathBuf::from(&args[0]);
    let output = PathBuf::from(&args[1]);
    let file =
        File::open(&input).map_err(|err| format!("cannot open {}: {err}", input.display()))?;
    let builder = ArrowReaderBuilder::try_new(file)
        .map_err(|err| format!("cannot open ORC source {}: {err}", input.display()))?;
    let schema = builder.schema().clone();
    let fingerprint = format!(
        "crc32c:{:08x}",
        checksum::crc32c(format!("{schema:?}").as_bytes())
    );
    let reader = builder.with_batch_size(4096).build();
    let batches = reader
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("cannot read ORC batches: {err}"))?;
    let result = convert_arrow_record_batches(
        "orc",
        fingerprint,
        schema,
        batches,
        &ParquetConversionOptions {
            table_name: input
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("orc_import")
                .to_string(),
            namespace: "interop".into(),
            ..ParquetConversionOptions::default()
        },
    )
    .map_err(|err| err.to_string())?;
    durable::durable_replace(&output, &result.cove_bytes)
        .map_err(|err| format!("cannot durably publish {}: {err}", output.display()))?;
    eprintln!(
        "converted {} rows and {} columns to {}",
        result.report.row_count,
        result.report.column_count,
        output.display()
    );
    Ok(())
}
