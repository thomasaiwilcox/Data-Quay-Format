//! `qf-validate` — Quay Format (QF) v1.0 file validator.
//!
//! Validates the structural integrity of a QF file by checking:
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
//! qf-validate [--semantic] [--verify-digests] <file.quay> [<file2.quay> ...]
//! ```
//!
//! Exit codes:
//! - 0 — all files are valid.
//! - 1 — one or more validation errors were found.
//! - 2 — usage error (no files specified).

use std::{path::Path, process};

use qf_core::reader::{self, ValidationOptions};

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let mut semantic = false;
    let mut verify_digests = false;
    let mut file_paths: Vec<&str> = Vec::new();

    // Flags must come before files
    let mut parsing_flags = true;
    for arg in &args[1..] {
        if parsing_flags && arg.starts_with("--") {
            match arg.as_str() {
                "--semantic" => semantic = true,
                "--verify-digests" => verify_digests = true,
                other => {
                    eprintln!("Unknown flag: {other}");
                    process::exit(2);
                }
            }
        } else {
            parsing_flags = false;
            file_paths.push(arg.as_str());
        }
    }

    if file_paths.is_empty() {
        eprintln!(
            "Usage: qf-validate [--semantic] [--verify-digests] <file.quay> [<file2.quay> ...]"
        );
        process::exit(2);
    }

    let opts = ValidationOptions {
        semantic,
        verify_digests,
        allow_unknown_optional_extensions: true,
    };

    let mut all_ok = true;
    for path in &file_paths {
        let ok = validate_file(Path::new(path), opts.clone());
        if !ok {
            all_ok = false;
        }
    }

    process::exit(if all_ok { 0 } else { 1 });
}

/// Validate a single QF file. Returns `true` if valid.
fn validate_file(path: &Path, opts: ValidationOptions) -> bool {
    let display = path.display();
    print!("Validating {display} ... ");

    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            println!("ERROR");
            eprintln!("  [I/O] {e}");
            return false;
        }
    };

    match reader::validate_bytes_with_options(&data, opts) {
        Ok(report) => {
            let mode = if report.semantic_checked {
                "semantic"
            } else {
                "structural"
            };
            println!("OK [{mode}]");
            let info = &report.validated;
            println!(
                "  QF v{}.{}",
                info.header.version_major, info.header.version_minor
            );
            println!("  file_len        : {} bytes", data.len());
            println!(
                "  primary_profile : {}",
                profile_name(info.header.primary_profile)
            );
            println!("  section_count   : {}", info.footer.sections.len());
            if let Some(n) = report.dict_entry_count {
                println!("  dict_entries    : {n}");
            }
            let metadata_json_preview = String::from_utf8_lossy(&info.footer.metadata_json)
                .chars()
                .take(120)
                .collect::<String>()
                .replace('\n', " ");
            if !metadata_json_preview.is_empty() {
                println!("  metadata (first 120 chars): {}", &metadata_json_preview);
            }
            true
        }
        Err(error) => {
            println!("INVALID");
            eprintln!("  [ERR] {error}");
            false
        }
    }
}

fn profile_name(code: u8) -> String {
    match code {
        0 => "Mixed/Unknown".into(),
        1 => "QF-O (Object Temporal)".into(),
        2 => "QF-T (Table Scan)".into(),
        3 => "QF-A (Archive Acceleration)".into(),
        4 => "QF-E (Engine Execution)".into(),
        5 => "QF-H (Harbor Execution)".into(),
        _ => format!("Unknown({code})"),
    }
}
