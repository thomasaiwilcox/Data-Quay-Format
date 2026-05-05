//! `cove-validate` — Cove Format (COVE) v1.0 file validator.
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
//! cove-validate [--semantic] [--verify-digests] [--json] [--explain]
//!             <file.cove> [<file2.cove> ...]
//! ```
//!
//! Exit codes:
//! - 0 — all files are valid.
//! - 1 — one or more validation errors were found.
//! - 2 — usage error (no files specified).

use std::{path::Path, process};

use cove_core::reader::{self, ValidationOptions};

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let mut semantic = false;
    let mut verify_digests = false;
    let mut json_out = false;
    let mut explain = false;
    let mut file_paths: Vec<&str> = Vec::new();

    // Flags must come before files
    let mut parsing_flags = true;
    for arg in &args[1..] {
        if parsing_flags && arg.starts_with("--") {
            match arg.as_str() {
                "--semantic" => semantic = true,
                "--verify-digests" => verify_digests = true,
                "--json" => json_out = true,
                "--explain" => explain = true,
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
            "Usage: cove-validate [--semantic] [--verify-digests] [--json] [--explain] <file.cove> [<file2.cove> ...]"
        );
        process::exit(2);
    }

    let opts = ValidationOptions {
        semantic,
        verify_digests,
        allow_unknown_optional_extensions: true,
    };

    let mut all_ok = true;
    if json_out {
        print!("[");
    }
    let mut first = true;
    for path in &file_paths {
        let ok = if json_out {
            if !first {
                print!(",");
            }
            first = false;
            validate_file_json(Path::new(path), opts.clone(), explain)
        } else {
            validate_file(Path::new(path), opts.clone())
        };
        if !ok {
            all_ok = false;
        }
    }
    if json_out {
        println!("]");
    }

    process::exit(if all_ok { 0 } else { 1 });
}

/// Machine-readable validation output (Spec §72, §75): emits one JSON object
/// per file with `path`, `ok`, optional `error_code`, and (when --explain)
/// the validated header/footer summary.
fn validate_file_json(path: &Path, opts: ValidationOptions, explain: bool) -> bool {
    let path_str = path.display().to_string();
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            print!(
                "{{\"path\":{},\"ok\":false,\"error\":{}}}",
                json_str(&path_str),
                json_str(&format!("io: {e}"))
            );
            return false;
        }
    };
    match reader::validate_bytes_with_options(&data, opts) {
        Ok(report) => {
            let info = &report.validated;
            print!(
                "{{\"path\":{},\"ok\":true,\"semantic\":{},\"version_major\":{},\"version_minor\":{},\"primary_profile\":{},\"section_count\":{}",
                json_str(&path_str),
                report.semantic_checked,
                info.header.version_major,
                info.header.version_minor,
                info.header.primary_profile,
                info.footer.sections.len()
            );
            if explain {
                print!(",\"sections\":[");
                for (i, s) in info.footer.sections.iter().enumerate() {
                    if i > 0 {
                        print!(",");
                    }
                    print!(
                        "{{\"kind\":{},\"offset\":{},\"length\":{}}}",
                        s.section_kind, s.offset, s.length
                    );
                }
                print!("]");
            }
            print!("}}");
            true
        }
        Err(error) => {
            print!("{{\"path\":{},\"ok\":false", json_str(&path_str));
            if let Some(code) = error.spec_code() {
                print!(",\"error_code\":{}", json_str(code));
            }
            print!(",\"error\":{}}}", json_str(&error.to_string()));
            false
        }
    }
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Validate a single COVE file. Returns `true` if valid.
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
                "  COVE v{}.{}",
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
        1 => "COVE-O (Object Temporal)".into(),
        2 => "COVE-T (Table Scan)".into(),
        3 => "COVE-A (Archive Acceleration)".into(),
        4 => "COVE-E (Engine Execution)".into(),
        5 => "COVE-H (Harbor Execution)".into(),
        _ => format!("Unknown({code})"),
    }
}
