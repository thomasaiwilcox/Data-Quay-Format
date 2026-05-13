use std::{env, fs, path::PathBuf, process::ExitCode};

use cove_core::{
    constants::SectionKind,
    profile::cove_e::{
        CodeSpaceDescriptorV1, EngineMountPolicyV1, ExecutionCodeDescriptorV1,
        ExecutionScopeDescriptorV1,
    },
    reader::{validate_bytes_with_options, ValidationOptions},
    utility::hex_encode,
};
use serde_json::json;

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(success) if success => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(message) => {
            eprintln!("cove-profile: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<bool, String> {
    let Some((command, rest)) = args.split_first() else {
        print_usage();
        return Ok(true);
    };
    match command.as_str() {
        "inspect" => inspect(rest),
        "validate-section" => validate_section(rest),
        "-h" | "--help" => {
            print_usage();
            Ok(true)
        }
        _ => Err(format!("unknown command {command}")),
    }
}

fn inspect(args: &[String]) -> Result<bool, String> {
    if args.len() != 1 {
        eprintln!("usage: cove-profile inspect <file.cove>");
        return Ok(true);
    }
    let path = PathBuf::from(&args[0]);
    let bytes = fs::read(&path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    let report = validate_bytes_with_options(
        &bytes,
        ValidationOptions {
            semantic: true,
            verify_digests: false,
            ..ValidationOptions::default()
        },
    )
    .map_err(|err| format!("cannot validate {}: {err}", path.display()))?;
    let header = &report.validated.header;
    let postscript = &report.validated.postscript;
    let footer = &report.validated.footer;
    let sections = footer
        .sections
        .iter()
        .map(|entry| {
            json!({
                "section_id": entry.section_id,
                "section_kind": entry.section_kind,
                "section_kind_name": SectionKind::from_u16(entry.section_kind).map(|kind| format!("{kind:?}")),
                "profile": entry.profile,
                "flags": entry.flags,
                "offset": entry.offset,
                "length": entry.length,
                "uncompressed_length": entry.uncompressed_length,
                "item_count": entry.item_count,
                "row_count": entry.row_count,
                "compression": entry.compression,
                "required_features": entry.required_features,
                "optional_features": entry.optional_features,
                "crc32c": entry.crc32c,
            })
        })
        .collect::<Vec<_>>();
    print_json(json!({
        "version": 1,
        "file": path.display().to_string(),
        "header": {
            "file_id": hex_encode(&header.file_id),
            "version_major": header.version_major,
            "version_minor": header.version_minor,
            "required_features": header.required_features,
            "optional_features": header.optional_features,
            "primary_profile": header.primary_profile,
            "feature_set_section_id": header.feature_set_section_id,
            "profile_capability_section_id": header.profile_capability_section_id,
            "fast_metadata_section_id": header.fast_metadata_section_id,
        },
        "postscript": {
            "file_len": postscript.file_len,
            "footer_offset": postscript.footer.offset,
            "footer_length": postscript.footer.length,
            "footer_crc32c": postscript.footer.crc32c,
        },
        "footer": {
            "section_count": footer.sections.len(),
            "metadata_json_len": footer.metadata_json.len(),
        },
        "sections": sections,
    }))?;
    Ok(true)
}

fn validate_section(args: &[String]) -> Result<bool, String> {
    let mut kind = None;
    let mut path = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--kind" => {
                kind = Some(
                    iter.next()
                        .ok_or_else(|| "--kind requires a value".to_string())?
                        .to_string(),
                )
            }
            _ if arg.starts_with('-') => {
                return Err(format!("unknown validate-section option {arg}"))
            }
            _ => {
                if path.replace(PathBuf::from(arg)).is_some() {
                    return Err("expected one <section.bin>".into());
                }
            }
        }
    }
    let kind = kind.ok_or_else(|| "--kind is required".to_string())?;
    let path = path.ok_or_else(|| "expected <section.bin>".to_string())?;
    let bytes = fs::read(&path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    let result =
        match kind.as_str() {
            "execution-code" => ExecutionCodeDescriptorV1::parse(&bytes).map(|_| ()),
            "execution-scope" => ExecutionScopeDescriptorV1::parse(&bytes).map(|_| ()),
            "code-space" => CodeSpaceDescriptorV1::parse(&bytes).map(|_| ()),
            "mount-policy" => EngineMountPolicyV1::parse(&bytes).map(|_| ()),
            _ => return Err(
                "--kind must be one of: execution-code, execution-scope, code-space, mount-policy"
                    .into(),
            ),
        };
    let success = result.is_ok();
    print_json(json!({
        "version": 1,
        "command": "validate-section",
        "file": path.display().to_string(),
        "kind": kind,
        "valid": success,
        "error": result.err().map(|err| err.to_string()),
    }))?;
    Ok(success)
}

fn print_json(value: serde_json::Value) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(&value)
            .map_err(|err| format!("cannot serialize report: {err}"))?
    );
    Ok(())
}

fn print_usage() {
    eprintln!(
        "usage: cove-profile inspect <file.cove> | validate-section <section.bin> --kind <execution-code|execution-scope|code-space|mount-policy>"
    );
}
