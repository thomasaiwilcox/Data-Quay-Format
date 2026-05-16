use std::{env, process::ExitCode};

use cove_convert_parquet::cli::{parse_conversion_args, publish_conversion_result, usage};
use cove_convert_parquet::source::{convert_file_to_cove, ConversionOptions, SourceFormat};

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
    let Some(command) = parse_conversion_args(args, "input.arrow|input.feather", "arrow_import")?
    else {
        println!(
            "{}",
            usage("cove-convert-arrow", "input.arrow|input.feather")
        );
        return Ok(());
    };
    let input = command.input.clone();
    let result = convert_file_to_cove(
        &input,
        ConversionOptions {
            source_format: Some(SourceFormat::ArrowIpc),
            cove: command.options.clone(),
            ..ConversionOptions::default()
        },
    )?;
    publish_conversion_result(command, result)
}
