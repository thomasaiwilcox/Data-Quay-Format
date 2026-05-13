//! `cove-validate` — Cove Format (COVE) v2.0 file validator.
//!
//! Validates the structural integrity of a COVE file by checking:
//!
//! 1. Trailing magic bytes.
//! 2. Postscript (checksum, file_len, footer bounds).
//! 3. Footer CRC.
//! 4. Footer header (magic, version, section entry length).
//! 5. Every section directory entry (bounds, CRC, reserved fields).
//! 6. Header (checksum, magic, version, endianness, reserved fields).
//!
//! Usage:
//! ```text
//! cove-validate [--semantic] [--verify-digests] [--fail-open-optional-pushdown] [--json] [--explain]
//!             <file.cove|file.covemap> [<file2> ...]
//! ```
//!
//! Exit codes:
//! - 0 — all files are valid.
//! - 1 — one or more validation errors were found.
//! - 2 — usage error (no files specified).

mod args;
mod format;
mod validate;

use std::process;

fn main() {
    let args = match args::parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            process::exit(2);
        }
    };

    let all_ok = validate::validate_paths(&args);
    process::exit(if all_ok { 0 } else { 1 });
}
