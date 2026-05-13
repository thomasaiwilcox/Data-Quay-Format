use std::{env, fs, path::PathBuf, process::ExitCode};

use cove_core::{
    constants::SectionKind,
    reader::{validate_bytes_with_options, ValidationOptions},
};
use serde_json::json;

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(success) if success => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(message) => {
            eprintln!("cove-verify-digest: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<bool, String> {
    let Some((input, require)) = parse_args(args)? else {
        print_usage();
        return Ok(true);
    };
    let bytes =
        fs::read(&input).map_err(|err| format!("cannot read {}: {err}", input.display()))?;
    let structural = validate_bytes_with_options(
        &bytes,
        ValidationOptions {
            semantic: false,
            verify_digests: false,
            ..ValidationOptions::default()
        },
    )
    .map_err(|err| format!("cannot validate {}: {err}", input.display()))?;
    let has_manifest = structural
        .validated
        .footer
        .sections
        .iter()
        .any(|entry| entry.section_kind == SectionKind::DigestManifest as u16);

    let (status, success, error) = if !has_manifest {
        ("missing_manifest", !require, None)
    } else {
        match validate_bytes_with_options(
            &bytes,
            ValidationOptions {
                semantic: true,
                verify_digests: true,
                ..ValidationOptions::default()
            },
        ) {
            Ok(_) => ("verified", true, None),
            Err(err) => ("mismatch", false, Some(err.to_string())),
        }
    };
    let report = json!({
        "version": 1,
        "file": input.display().to_string(),
        "status": status,
        "require": require,
        "digest_manifest_present": has_manifest,
        "error": error,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|err| format!("cannot serialize report: {err}"))?
    );
    Ok(success)
}

fn parse_args(args: Vec<String>) -> Result<Option<(PathBuf, bool)>, String> {
    let mut require = false;
    let mut input = None;
    for arg in args {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--require" => require = true,
            _ if arg.starts_with('-') => return Err(format!("unknown option {arg}")),
            _ => {
                if input.replace(PathBuf::from(arg)).is_some() {
                    return Err("expected one <file.cove>".into());
                }
            }
        }
    }
    let input = input.ok_or_else(|| "expected <file.cove>".to_string())?;
    Ok(Some((input, require)))
}

fn print_usage() {
    eprintln!("usage: cove-verify-digest <file.cove> [--require]");
}
