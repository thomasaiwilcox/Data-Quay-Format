use std::{env, process::ExitCode};

fn main() -> ExitCode {
    match cove_fuzz::run_cli(env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("cove-fuzz: {message}");
            ExitCode::FAILURE
        }
    }
}
