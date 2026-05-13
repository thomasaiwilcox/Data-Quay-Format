use std::{env, process::ExitCode};

fn main() -> ExitCode {
    run_wrapper("cove-map-diff", "diff")
}

fn run_wrapper(binary: &str, subcommand: &str) -> ExitCode {
    let user_args = env::args().skip(1).collect::<Vec<_>>();
    if user_args.iter().any(|arg| arg == "-h" || arg == "--help") {
        eprintln!("usage: {binary} [cove-map {subcommand} arguments]");
        return ExitCode::SUCCESS;
    }
    let mut args = Vec::new();
    args.push(subcommand.to_string());
    args.extend(user_args);
    match cove_map::run_cli(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{binary}: {message}");
            ExitCode::FAILURE
        }
    }
}
