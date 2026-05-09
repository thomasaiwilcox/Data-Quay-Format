mod format;
mod inspect;

use std::{path::Path, process};

fn main() {
    let file_paths = std::env::args().skip(1).collect::<Vec<_>>();
    if file_paths.is_empty() {
        eprintln!("Usage: cove-inspect <file.cove> [<file2.cove> ...]");
        process::exit(2);
    }

    let mut all_ok = true;
    for path in &file_paths {
        if let Err(error) = inspect::inspect_file(Path::new(path)) {
            all_ok = false;
            eprintln!("ERROR: {error}");
        }
    }

    process::exit(if all_ok { 0 } else { 1 });
}
