use std::{env, fs::File, path::PathBuf, process::ExitCode};

use arrow_ipc::reader::{FileReader, StreamReader};
use cove_arrow::convert::{convert_arrow_record_batches, ParquetConversionOptions};
use cove_core::{checksum, durable};

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("cove-convert-arrow: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    if args.len() != 2 || args.iter().any(|arg| arg == "-h" || arg == "--help") {
        eprintln!("usage: cove-convert-arrow <input.arrow|input.feather> <output.cove>");
        return Ok(());
    }
    let input = PathBuf::from(&args[0]);
    let output = PathBuf::from(&args[1]);
    let (schema, batches) = read_arrow_batches(&input)?;
    let fingerprint = format!(
        "crc32c:{:08x}",
        checksum::crc32c(format!("{schema:?}").as_bytes())
    );
    let result = convert_arrow_record_batches(
        "arrow-ipc",
        fingerprint,
        schema,
        batches,
        &ParquetConversionOptions {
            table_name: input
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("arrow_import")
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
