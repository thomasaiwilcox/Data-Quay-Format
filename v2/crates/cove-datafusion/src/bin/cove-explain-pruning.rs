use std::{env, path::PathBuf, process::ExitCode};

use cove_datafusion::explain::{
    explain_pruning, parse_filter_dsl, parse_projection_dsl, parse_topn_dsl, ExplainOptions,
};

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("cove-explain-pruning: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let Some((input, options)) = parse_args(args)? else {
        print_usage();
        return Ok(());
    };
    let report = explain_pruning(&input, options).map_err(|err| err.to_string())?;
    let json = serde_json::to_string_pretty(&report.to_json_value())
        .map_err(|err| format!("cannot serialize report: {err}"))?;
    println!("{json}");
    Ok(())
}

fn parse_args(args: Vec<String>) -> Result<Option<(PathBuf, ExplainOptions)>, String> {
    let mut options = ExplainOptions::default();
    let mut input = None;
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
            "--top-n" => {
                let raw = next_value(&mut iter, "--top-n")?;
                options.top_n = Some(parse_topn_dsl(&raw).map_err(|err| err.to_string())?);
            }
            _ if arg.starts_with('-') => return Err(format!("unknown option {arg}")),
            _ => {
                if input.replace(PathBuf::from(arg)).is_some() {
                    return Err("expected a single <input.cove>".into());
                }
            }
        }
    }
    let input = input.ok_or_else(|| "expected <input.cove>".to_string())?;
    Ok(Some((input, options)))
}

fn next_value(iter: &mut impl Iterator<Item = String>, option: &str) -> Result<String, String> {
    iter.next()
        .ok_or_else(|| format!("{option} requires a value"))
}

fn print_usage() {
    eprintln!(
        "usage: cove-explain-pruning [--columns a,b] [--filter column=<name|index>,op=<eq|lt|lte|gt|gte|is-null|is-not-null>,value=<literal>] [--top-n column=<name|index>,fetch=<n>,desc=<bool>] <input.cove>"
    );
}
