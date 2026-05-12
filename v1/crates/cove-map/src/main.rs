use std::{env, process::ExitCode};

fn main() -> ExitCode {
    match cove_map::run_cli(env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("cove-map: {message}");
            ExitCode::FAILURE
        }
    }
}
