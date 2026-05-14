mod format;
mod inspect;

use std::{path::Path, process};

fn main() {
    let raw_args = std::env::args().skip(1).collect::<Vec<_>>();
    let args = match parse_args(&raw_args) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error}");
            eprintln!(
                "Usage: cove-inspect [--json] [--sections <stats,dictionary,execution,indexes,optional>] <file.cove> [<file2.cove> ...]"
            );
            process::exit(2);
        }
    };

    if args.file_paths.is_empty() {
        eprintln!(
            "Usage: cove-inspect [--json] [--sections <stats,dictionary,execution,indexes,optional>] <file.cove> [<file2.cove> ...]"
        );
        process::exit(2);
    }

    if args.json {
        let mut values = Vec::new();
        let mut all_ok = true;
        for path in &args.file_paths {
            match inspect::inspect_file_json(Path::new(path), &args.sections) {
                Ok(value) => values.push(value),
                Err(error) => {
                    all_ok = false;
                    eprintln!("ERROR: {error}");
                }
            }
        }
        if all_ok {
            if values.len() == 1 {
                println!("{}", serde_json::to_string_pretty(&values[0]).unwrap());
            } else {
                println!("{}", serde_json::to_string_pretty(&values).unwrap());
            }
        }
        process::exit(if all_ok { 0 } else { 1 });
    }

    let mut all_ok = true;
    for path in &args.file_paths {
        if let Err(error) = inspect::inspect_file(Path::new(path)) {
            all_ok = false;
            eprintln!("ERROR: {error}");
        }
    }

    process::exit(if all_ok { 0 } else { 1 });
}

struct Args {
    json: bool,
    sections: inspect::InspectSections,
    file_paths: Vec<String>,
}

fn parse_args(raw_args: &[String]) -> Result<Args, String> {
    let mut json = false;
    let mut sections = inspect::InspectSections::All;
    let mut file_paths = Vec::new();
    let mut index = 0usize;
    while index < raw_args.len() {
        match raw_args[index].as_str() {
            "--json" => json = true,
            "--sections" => {
                let Some(raw_sections) = raw_args.get(index + 1) else {
                    return Err("--sections requires a comma-separated group list".into());
                };
                sections = inspect::InspectSections::parse(raw_sections)?;
                index += 1;
            }
            arg if arg.starts_with("--") => return Err(format!("unknown argument: {arg}")),
            path => file_paths.push(path.to_string()),
        }
        index += 1;
    }
    Ok(Args {
        json,
        sections,
        file_paths,
    })
}
