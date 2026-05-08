use std::path::PathBuf;

use serde_json::json;

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Command {
    Validate {
        map: PathBuf,
    },
    Preview {
        map: PathBuf,
    },
    PlanKeys {
        map: PathBuf,
        sources: Vec<PathBuf>,
    },
    Convert {
        map: PathBuf,
        sources: Vec<PathBuf>,
        output: Option<PathBuf>,
        format: OutputFormat,
    },
    Explain {
        map: PathBuf,
        id: String,
    },
    Diff {
        left: PathBuf,
        right: PathBuf,
    },
    Project {
        map: PathBuf,
        sources: Vec<PathBuf>,
        output: Option<PathBuf>,
    },
    Test {
        fixture: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputFormat {
    Json,
    CoveO,
}

pub fn run_cli(args: impl IntoIterator<Item = String>) -> Result<(), String> {
    let Some(command) = parse_args(args)? else {
        print_usage();
        return Ok(());
    };
    match command {
        Command::Validate { map } => {
            parse_map(&map)?;
            println!("{}", json!({"ok": true, "path": map.display().to_string()}));
        }
        Command::Preview { map } => {
            let file = parse_map(&map)?;
            print_json(&preview(&file));
        }
        Command::PlanKeys { map, sources } => {
            let file = parse_map(&map)?;
            let inputs = read_source_inputs(&sources)?;
            validate_source_inputs(&file, &inputs.states)?;
            print_json(&plan_keys(&file, &inputs.rows));
        }
        Command::Convert {
            map,
            sources,
            output,
            format,
        } => {
            let file = parse_map(&map)?;
            let inputs = read_source_inputs(&sources)?;
            validate_source_inputs(&file, &inputs.states)?;
            match format {
                OutputFormat::Json => {
                    let materialized =
                        materialize_with_source_states(&file, &inputs.rows, &inputs.states)?;
                    write_or_print(output, &materialized.conversion_report)?;
                }
                OutputFormat::CoveO => {
                    let output = output.ok_or_else(|| {
                        "convert --format cove-o requires --output <path>".to_string()
                    })?;
                    let bytes =
                        build_cove_o_with_source_states(&file, &inputs.rows, &inputs.states)?;
                    durable::durable_replace(&output, &bytes).map_err(|err| {
                        format!("cannot durably publish {}: {err}", output.display())
                    })?;
                }
            }
        }
        Command::Explain { map, id } => {
            let file = parse_map(&map)?;
            print_json(&explain(&file, &id)?);
        }
        Command::Diff { left, right } => {
            let left = parse_map(&left)?;
            let right = parse_map(&right)?;
            print_json(&diff_maps(&left, &right));
        }
        Command::Project {
            map,
            sources,
            output,
        } => {
            let file = parse_map(&map)?;
            let inputs = read_source_inputs(&sources)?;
            validate_source_inputs(&file, &inputs.states)?;
            let projected = project_rows_with_source_states(&file, &inputs.rows, &inputs.states)?;
            write_or_print(output, &projected)?;
        }
        Command::Test { fixture } => run_fixture_path(&fixture)?,
    }
    Ok(())
}

pub(crate) fn parse_args(
    args: impl IntoIterator<Item = String>,
) -> Result<Option<Command>, String> {
    let mut args = args.into_iter();
    let Some(subcommand) = args.next() else {
        return Ok(None);
    };
    if subcommand == "-h" || subcommand == "--help" {
        return Ok(None);
    }
    let command = match subcommand.as_str() {
        "validate" => Command::Validate {
            map: one_path(&mut args, "validate <mapping.covemap>")?,
        },
        "preview" => Command::Preview {
            map: one_path(&mut args, "preview <mapping.covemap>")?,
        },
        "plan-keys" => {
            let map = one_path(&mut args, "plan-keys <mapping.covemap> <source...>")?;
            Command::PlanKeys {
                map,
                sources: args.map(PathBuf::from).collect(),
            }
        }
        "convert" => {
            let (output, format, positional) = parse_output_format_and_positionals(args)?;
            let mut positional = positional.into_iter();
            let map = positional
                .next()
                .ok_or_else(|| "convert requires <mapping.covemap>".to_string())?;
            Command::Convert {
                map,
                sources: positional.collect(),
                output,
                format,
            }
        }
        "explain" => {
            let map = one_path(&mut args, "explain <mapping.covemap> <goid|assertion-id>")?;
            let id = args
                .next()
                .ok_or_else(|| "explain requires an id".to_string())?;
            Command::Explain { map, id }
        }
        "diff" => Command::Diff {
            left: one_path(&mut args, "diff <left.covemap> <right.covemap>")?,
            right: one_path(&mut args, "diff <left.covemap> <right.covemap>")?,
        },
        "project" => {
            let (output, format, positional) = parse_output_format_and_positionals(args)?;
            if format != OutputFormat::Json {
                return Err("project currently supports --format json only".into());
            }
            let mut positional = positional.into_iter();
            let map = positional
                .next()
                .ok_or_else(|| "project requires <mapping.covemap>".to_string())?;
            Command::Project {
                map,
                sources: positional.collect(),
                output,
            }
        }
        "test" => Command::Test {
            fixture: one_path(&mut args, "test <fixture.json>")?,
        },
        _ => return Err(format!("unknown subcommand {subcommand}")),
    };
    Ok(Some(command))
}

fn one_path(args: &mut impl Iterator<Item = String>, usage: &str) -> Result<PathBuf, String> {
    args.next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("usage: cove-map {usage}"))
}

fn parse_output_format_and_positionals(
    args: impl Iterator<Item = String>,
) -> Result<(Option<PathBuf>, OutputFormat, Vec<PathBuf>), String> {
    let mut output = None;
    let mut format = OutputFormat::Json;
    let mut positional = Vec::new();
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        if arg == "--output" || arg == "-o" {
            output = Some(
                args.next()
                    .map(PathBuf::from)
                    .ok_or_else(|| format!("{arg} requires a path"))?,
            );
        } else if arg == "--format" {
            let raw = args
                .next()
                .ok_or_else(|| "--format requires json or cove-o".to_string())?;
            format = match raw.as_str() {
                "json" => OutputFormat::Json,
                "cove-o" => OutputFormat::CoveO,
                _ => return Err("--format must be one of: json, cove-o".into()),
            };
        } else if arg.starts_with('-') {
            return Err(format!("unknown option {arg}"));
        } else {
            positional.push(PathBuf::from(arg));
        }
    }
    Ok((output, format, positional))
}
