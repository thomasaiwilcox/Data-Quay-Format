mod args;
mod dump;

use std::{path::Path, process};

fn main() {
    let args = match args::parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            process::exit(2);
        }
    };

    if let Err(error) = dump::dump_file(Path::new(&args.path), args.mode, args.max_bytes) {
        eprintln!("ERROR: {error}");
        process::exit(1);
    }
}
