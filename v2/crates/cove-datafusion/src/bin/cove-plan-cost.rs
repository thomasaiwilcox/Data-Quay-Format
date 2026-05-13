use std::{env, path::PathBuf, process::ExitCode};

use cove_datafusion::explain::{
    parse_filter_dsl, parse_projection_dsl, parse_topn_dsl, plan_cost, ExplainOptions,
};

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("cove-plan-cost: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let Some((input, options, execute)) = parse_args(args)? else {
        print_usage();
        return Ok(());
    };
    let report = plan_cost(&input, options, execute).map_err(|err| err.to_string())?;
    let json = serde_json::to_string_pretty(&report.to_json_value())
        .map_err(|err| format!("cannot serialize report: {err}"))?;
    println!("{json}");
    Ok(())
}

fn parse_args(args: Vec<String>) -> Result<Option<(PathBuf, ExplainOptions, bool)>, String> {
    let mut options = ExplainOptions::default();
    let mut execute = false;
    let mut input = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--execute" => execute = true,
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
    Ok(Some((input, options, execute)))
}

fn next_value(iter: &mut impl Iterator<Item = String>, option: &str) -> Result<String, String> {
    iter.next()
        .ok_or_else(|| format!("{option} requires a value"))
}

fn print_usage() {
    eprintln!(
        "usage: cove-plan-cost [--execute] [--columns a,b] [--filter column=<name|index>,op=<eq|lt|lte|gt|gte|is-null|is-not-null>,value=<literal>] [--top-n column=<name|index>,fetch=<n>,desc=<bool>] <input.cove>"
    );
}
