use std::{env, path::PathBuf, process::ExitCode};

use cove_core::{durable, utility::build_covx_artifact};

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("cove-build-covx: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    if args.iter().any(|arg| arg == "-h" || arg == "--help") || args.len() < 2 {
        eprintln!("usage: cove-build-covx <output.covx> <input.cove>...");
        return Ok(());
    }
    let output = PathBuf::from(&args[0]);
    let inputs = args[1..].iter().map(PathBuf::from).collect::<Vec<_>>();
    let (bytes, report) = build_covx_artifact(&output, &inputs).map_err(|err| err.to_string())?;
    durable::durable_replace(&output, &bytes)
        .map_err(|err| format!("cannot durably publish {}: {err}", output.display()))?;
    let json = serde_json::to_string_pretty(&report.to_json_value())
        .map_err(|err| format!("cannot serialize report: {err}"))?;
    println!("{json}");
    Ok(())
}
