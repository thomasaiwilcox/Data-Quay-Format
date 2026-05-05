use std::{env, fs, path::PathBuf, process::ExitCode};

use cove_core::{
    constants::CompressionCodec,
    interop::parquet::{convert_parquet_bytes, ParquetConversionOptions},
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Command {
    input: PathBuf,
    output: PathBuf,
    options: ParquetConversionOptions,
    report: Option<ReportTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReportTarget {
    Stdout,
    Path(PathBuf),
}

fn main() -> ExitCode {
    match run(env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("cove-convert-parquet: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: impl IntoIterator<Item = String>) -> Result<(), String> {
    let Some(command) = parse_args(args)? else {
        print_usage();
        return Ok(());
    };

    let input = fs::read(&command.input)
        .map_err(|err| format!("cannot read {}: {err}", command.input.display()))?;
    let result = convert_parquet_bytes(&input, &command.options).map_err(|err| err.to_string())?;
    fs::write(&command.output, &result.cove_bytes)
        .map_err(|err| format!("cannot write {}: {err}", command.output.display()))?;

    if let Some(target) = command.report {
        let report = serde_json::to_string_pretty(&result.report.to_json_value())
            .map_err(|err| format!("cannot serialize conversion report: {err}"))?;
        match target {
            ReportTarget::Stdout => println!("{report}"),
            ReportTarget::Path(path) => fs::write(&path, report)
                .map_err(|err| format!("cannot write {}: {err}", path.display()))?,
        }
    } else {
        eprintln!(
            "converted {} rows and {} columns to {}",
            result.report.row_count,
            result.report.column_count,
            command.output.display()
        );
    }
    Ok(())
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<Command>, String> {
    let mut options = ParquetConversionOptions::default();
    let mut report = None;
    let mut positional = Vec::new();
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--table-name" => options.table_name = next_value(&mut iter, "--table-name")?,
            "--namespace" => options.namespace = next_value(&mut iter, "--namespace")?,
            "--morsel-row-count" => {
                let raw = next_value(&mut iter, "--morsel-row-count")?;
                options.morsel_row_count = raw
                    .parse::<u32>()
                    .map_err(|_| "--morsel-row-count must be a u32".to_string())?;
                if options.morsel_row_count == 0 {
                    return Err("--morsel-row-count must be greater than zero".into());
                }
            }
            "--compression" => {
                options.page_compression =
                    parse_compression(&next_value(&mut iter, "--compression")?)?;
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
        return Err("expected <input.parquet> and <output.cove>".into());
    }
    Ok(Some(Command {
        input: positional.remove(0),
        output: positional.remove(0),
        options,
        report,
    }))
}

fn next_value(iter: &mut impl Iterator<Item = String>, option: &str) -> Result<String, String> {
    iter.next()
        .ok_or_else(|| format!("{option} requires a value"))
}

fn parse_compression(value: &str) -> Result<CompressionCodec, String> {
    match value {
        "none" => Ok(CompressionCodec::None),
        "lz4" => Ok(CompressionCodec::Lz4),
        "zstd" => Ok(CompressionCodec::Zstd),
        _ => Err("--compression must be one of: none, lz4, zstd".into()),
    }
}

fn print_usage() {
    println!(
        "Usage: cove-convert-parquet [options] <input.parquet> <output.cove>\n\n\
Options:\n  \
--table-name <name>         Output COVE table name (default: parquet_import)\n  \
--namespace <name>          Output COVE namespace (default: interop)\n  \
--morsel-row-count <rows>   Rows per COVE morsel/page (default: 4096)\n  \
--compression <codec>       Page compression: none, lz4, zstd (default: none)\n  \
--report <path|->           Write the machine-readable conversion report\n  \
-h, --help                  Show this help"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_required_paths_and_options() {
        let command = parse_args([
            "--table-name".to_string(),
            "orders".to_string(),
            "--namespace".to_string(),
            "sales".to_string(),
            "--morsel-row-count".to_string(),
            "128".to_string(),
            "--compression".to_string(),
            "none".to_string(),
            "--report".to_string(),
            "-".to_string(),
            "in.parquet".to_string(),
            "out.cove".to_string(),
        ])
        .unwrap()
        .unwrap();

        assert_eq!(command.options.table_name, "orders");
        assert_eq!(command.options.namespace, "sales");
        assert_eq!(command.options.morsel_row_count, 128);
        assert_eq!(command.report, Some(ReportTarget::Stdout));
    }

    #[test]
    fn rejects_unknown_compression() {
        assert_eq!(
            parse_compression("snappy"),
            Err("--compression must be one of: none, lz4, zstd".into())
        );
    }
}
