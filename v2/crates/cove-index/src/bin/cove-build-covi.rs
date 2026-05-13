use std::{env, fs, process};

use cove_index::{
    build::{build_covi_from_cove_bytes, CoviBuildOptions},
    CoviArtifactV2,
};

fn main() {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let result = if args.len() == 1 {
        build_empty(&args[0])
    } else {
        parse_build_args(args).and_then(|command| build_from_cove(&command))
    };
    if let Err(error) = result {
        eprintln!("{error}");
        process::exit(1);
    }
}

#[derive(Debug, Clone)]
struct BuildCommand {
    input_path: String,
    output_path: String,
    options: CoviBuildOptions,
}

fn parse_build_args(args: Vec<String>) -> Result<BuildCommand, String> {
    let mut positionals = Vec::new();
    let mut options = CoviBuildOptions::default();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--table-id" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--table-id requires a value".to_string())?;
                options.table_id = Some(
                    value
                        .parse::<u32>()
                        .map_err(|_| format!("invalid --table-id value: {value}"))?,
                );
            }
            "--column-id" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--column-id requires a value".to_string())?;
                options.column_ids.push(
                    value
                        .parse::<u32>()
                        .map_err(|_| format!("invalid --column-id value: {value}"))?,
                );
            }
            "--all-columns" => options.all_columns = true,
            "--index-only-counts" => options.include_index_only_counts = true,
            _ if arg.starts_with("--") => return Err(format!("unknown option: {arg}")),
            _ => positionals.push(arg),
        }
    }
    if positionals.len() != 2 {
        return Err(
            "usage: cove-build-covi <input.cove> <output.covi> [--table-id <id>] [--column-id <id> ... | --all-columns] [--index-only-counts]\n       cove-build-covi <output.covi>  # empty artifact compatibility mode"
                .to_string(),
        );
    }
    if options.all_columns && !options.column_ids.is_empty() {
        return Err("--all-columns cannot be combined with --column-id".to_string());
    }
    Ok(BuildCommand {
        input_path: positionals.remove(0),
        output_path: positionals.remove(0),
        options,
    })
}

fn build_empty(output: &str) -> Result<(), String> {
    let artifact = CoviArtifactV2::new_empty([0u8; 16], [0u8; 16]);
    let bytes = artifact
        .serialize_empty()
        .map_err(|error| format!("failed to build empty COVE-I artifact: {error}"))?;
    fs::write(output, bytes).map_err(|error| format!("{output}: {error}"))?;
    println!("wrote empty COVE-I artifact to {output}");
    Ok(())
}

fn build_from_cove(command: &BuildCommand) -> Result<(), String> {
    let input = fs::read(&command.input_path)
        .map_err(|error| format!("{}: {error}", command.input_path))?;
    let bytes = build_covi_from_cove_bytes(&input, &command.options)
        .map_err(|error| format!("{}: {error}", command.input_path))?;
    fs::write(&command.output_path, bytes)
        .map_err(|error| format!("{}: {error}", command.output_path))?;
    println!("wrote COVE-I artifact to {}", command.output_path);
    Ok(())
}
