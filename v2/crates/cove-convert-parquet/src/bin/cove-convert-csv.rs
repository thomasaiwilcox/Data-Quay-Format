use std::{env, process::ExitCode};

use cove_convert_parquet::cli::{parse_conversion_args, publish_conversion_result, usage};
use cove_convert_parquet::source::{
    convert_file_to_cove, ConversionOptions, CsvReadOptions, SourceFormat,
};

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("cove-convert-csv: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let (csv_options, conversion_args) = parse_csv_options(args)?;
    let Some(command) = parse_conversion_args(conversion_args, "input.csv", "csv_import")? else {
        println!("{}", csv_usage());
        return Ok(());
    };
    let input = command.input.clone();
    let result = convert_file_to_cove(
        &input,
        ConversionOptions {
            source_format: Some(SourceFormat::Csv),
            cove: command.options.clone(),
            csv: csv_options,
        },
    )?;
    publish_conversion_result(command, result)
}

fn parse_csv_options(args: Vec<String>) -> Result<(CsvReadOptions, Vec<String>), String> {
    let mut csv = CsvReadOptions::default();
    let mut rest = Vec::new();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--csv-header" => csv.has_header = true,
            "--no-csv-header" => csv.has_header = false,
            "--csv-delimiter" => csv.delimiter = parse_delimiter(&next_value(&mut iter, &arg)?)?,
            "--csv-infer-rows" => {
                let raw = next_value(&mut iter, &arg)?;
                csv.infer_rows = if raw == "all" {
                    None
                } else {
                    Some(
                        raw.parse::<usize>()
                            .map_err(|_| "--csv-infer-rows must be a usize or `all`".to_string())?,
                    )
                };
            }
            "--csv-batch-size" => {
                csv.batch_size = next_value(&mut iter, &arg)?
                    .parse::<usize>()
                    .map_err(|_| "--csv-batch-size must be a usize".to_string())?;
                if csv.batch_size == 0 {
                    return Err("--csv-batch-size must be greater than zero".into());
                }
            }
            "--csv-allow-truncated-rows" => csv.allow_truncated_rows = true,
            _ => rest.push(arg),
        }
    }
    Ok((csv, rest))
}

fn parse_delimiter(value: &str) -> Result<u8, String> {
    match value {
        "tab" | "\\t" => Ok(b'\t'),
        _ if value.len() == 1 => Ok(value.as_bytes()[0]),
        _ => Err("--csv-delimiter must be one byte, `tab`, or `\\t`".into()),
    }
}

fn next_value(iter: &mut impl Iterator<Item = String>, option: &str) -> Result<String, String> {
    iter.next()
        .ok_or_else(|| format!("{option} requires a value"))
}

fn csv_usage() -> String {
    format!(
        "{}\nCSV options: [--csv-header|--no-csv-header] [--csv-delimiter <byte|tab>] [--csv-infer-rows <n|all>] [--csv-batch-size <n>] [--csv-allow-truncated-rows]",
        usage("cove-convert-csv", "input.csv")
    )
}
