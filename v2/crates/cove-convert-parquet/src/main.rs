use std::{env, fs, process::ExitCode};

use cove_arrow::convert::convert_parquet_bytes;
use cove_convert_parquet::cli::{
    parse_conversion_args, publish_conversion_result, set_source_identity, usage,
};

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
    let Some(mut command) = parse_conversion_args(args, "input.parquet", "parquet_import")? else {
        println!("{}", usage("cove-convert-parquet", "input.parquet"));
        return Ok(());
    };
    let input = fs::read(&command.input)
        .map_err(|err| format!("cannot read {}: {err}", command.input.display()))?;
    set_source_identity(&mut command.options, &command.input, &input)?;
    let result = convert_parquet_bytes(&input, &command.options).map_err(|err| err.to_string())?;
    publish_conversion_result(command, result)
}
