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
        format: ProjectionFormat,
        projection_id: Option<String>,
    },
    ProjectCoveO {
        object: PathBuf,
        mapping: Option<PathBuf>,
        output: Option<PathBuf>,
        format: ProjectionFormat,
        projection_id: Option<String>,
    },
    Test {
        fixture: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputFormat {
    Json,
    CoveO,
    Arrow,
    CoveT,
    Sql,
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
                OutputFormat::Arrow | OutputFormat::CoveT | OutputFormat::Sql => {
                    return Err("convert supports --format json or cove-o only".into())
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
            format,
            projection_id,
        } => {
            let file = parse_map(&map)?;
            let inputs = read_source_inputs(&sources)?;
            validate_source_inputs(&file, &inputs.states)?;
            let projected = project_rows_with_source_states_output(
                &file,
                &inputs.rows,
                &inputs.states,
                format,
                projection_id.as_deref(),
            )?;
            write_projection_output(output, format, &projected)?;
        }
        Command::ProjectCoveO {
            object,
            mapping,
            output,
            format,
            projection_id,
        } => {
            let projected = project_cove_o_path_output(
                &object,
                mapping.as_deref(),
                format,
                projection_id.as_deref(),
            )?;
            write_projection_output(output, format, &projected)?;
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
            let (output, format, projection_id, positional) =
                parse_output_format_projection_and_positionals(args)?;
            let mut positional = positional.into_iter();
            let map = positional
                .next()
                .ok_or_else(|| "project requires <mapping.covemap>".to_string())?;
            Command::Project {
                map,
                sources: positional.collect(),
                output,
                format: project_format(format)?,
                projection_id,
            }
        }
        "project-cove-o" => {
            let (object, mapping, output, format, projection_id) = parse_project_cove_o_args(args)?;
            Command::ProjectCoveO {
                object,
                mapping,
                output,
                format,
                projection_id,
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
                "arrow" => OutputFormat::Arrow,
                "cove-t" => OutputFormat::CoveT,
                "sql" => OutputFormat::Sql,
                _ => return Err("--format must be one of: json, cove-o, arrow, cove-t, sql".into()),
            };
        } else if arg.starts_with('-') {
            return Err(format!("unknown option {arg}"));
        } else {
            positional.push(PathBuf::from(arg));
        }
    }
    Ok((output, format, positional))
}

fn parse_project_cove_o_args(
    args: impl Iterator<Item = String>,
) -> Result<
    (
        PathBuf,
        Option<PathBuf>,
        Option<PathBuf>,
        ProjectionFormat,
        Option<String>,
    ),
    String,
> {
    let mut output = None;
    let mut mapping = None;
    let mut format = ProjectionFormat::Json;
    let mut projection_id = None;
    let mut positional = Vec::new();
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        if arg == "--output" || arg == "-o" {
            output = Some(
                args.next()
                    .map(PathBuf::from)
                    .ok_or_else(|| format!("{arg} requires a path"))?,
            );
        } else if arg == "--mapping" {
            mapping = Some(
                args.next()
                    .map(PathBuf::from)
                    .ok_or_else(|| "--mapping requires a path".to_string())?,
            );
        } else if arg == "--format" {
            let raw = args.next().ok_or_else(|| {
                "--format requires json, cove-o, arrow, cove-t, or sql".to_string()
            })?;
            format = match raw.as_str() {
                "json" => ProjectionFormat::Json,
                "arrow" => ProjectionFormat::Arrow,
                "cove-t" => ProjectionFormat::CoveT,
                "sql" => ProjectionFormat::Sql,
                "cove-o" => ProjectionFormat::CoveO,
                _ => return Err("--format must be one of: json, cove-o, arrow, cove-t, sql".into()),
            };
        } else if arg == "--projection-id" {
            projection_id = Some(
                args.next()
                    .ok_or_else(|| "--projection-id requires an id".to_string())?,
            );
        } else if arg.starts_with('-') {
            return Err(format!("unknown option {arg}"));
        } else {
            positional.push(PathBuf::from(arg));
        }
    }
    if positional.len() != 1 {
        return Err("project-cove-o requires exactly one <object.cove>".into());
    }
    Ok((positional.remove(0), mapping, output, format, projection_id))
}

fn parse_output_format_projection_and_positionals(
    args: impl Iterator<Item = String>,
) -> Result<(Option<PathBuf>, OutputFormat, Option<String>, Vec<PathBuf>), String> {
    let mut output = None;
    let mut format = OutputFormat::Json;
    let mut projection_id = None;
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
            let raw = args.next().ok_or_else(|| {
                "--format requires json, cove-o, arrow, cove-t, or sql".to_string()
            })?;
            format = match raw.as_str() {
                "json" => OutputFormat::Json,
                "arrow" => OutputFormat::Arrow,
                "cove-t" => OutputFormat::CoveT,
                "sql" => OutputFormat::Sql,
                "cove-o" => OutputFormat::CoveO,
                _ => return Err("--format must be one of: json, cove-o, arrow, cove-t, sql".into()),
            };
        } else if arg == "--projection-id" {
            projection_id = Some(
                args.next()
                    .ok_or_else(|| "--projection-id requires an id".to_string())?,
            );
        } else if arg.starts_with('-') {
            return Err(format!("unknown option {arg}"));
        } else {
            positional.push(PathBuf::from(arg));
        }
    }
    Ok((output, format, projection_id, positional))
}

fn project_format(format: OutputFormat) -> Result<ProjectionFormat, String> {
    match format {
        OutputFormat::Json => Ok(ProjectionFormat::Json),
        OutputFormat::CoveO => Ok(ProjectionFormat::CoveO),
        OutputFormat::Arrow => Ok(ProjectionFormat::Arrow),
        OutputFormat::CoveT => Ok(ProjectionFormat::CoveT),
        OutputFormat::Sql => Ok(ProjectionFormat::Sql),
    }
}

fn write_projection_output(
    output: Option<PathBuf>,
    format: ProjectionFormat,
    bytes: &[u8],
) -> Result<(), String> {
    match output {
        Some(path) => durable::durable_replace(&path, bytes)
            .map(|_| ())
            .map_err(|err| format!("cannot durably publish {}: {err}", path.display())),
        None if matches!(format, ProjectionFormat::Json | ProjectionFormat::Sql) => {
            println!(
                "{}",
                std::str::from_utf8(bytes)
                    .map_err(|err| format!("projection JSON is not UTF-8: {err}"))?
            );
            Ok(())
        }
        None => Err(format!(
            "project --format {} requires --output <path>",
            format.as_str()
        )),
    }
}
