use std::{env, fs, path::PathBuf, process::ExitCode};

use cove_core::{
    checksum,
    constants::SectionKind,
    profile::cove_e::{
        CodeSpaceDescriptorV1, EngineMountPolicyV1, EngineProfileEntryV1, EngineProfileRegistry,
        ExecutionCodeCanonicality, ExecutionCodeComparisonScope, ExecutionCodeDescriptorV1,
        ExecutionCodeKind, ExecutionCodeLifetime, ExecutionScopeDescriptorV1, ExecutionScopeKind,
        FileCodeMappingKind, MissingValuePolicy, NullCodePolicy, ReverseLookupPolicy,
        StaleMappingPolicy,
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
        "generate" => generate(rest),
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

fn generate(args: &[String]) -> Result<bool, String> {
    let options = GenerateOptions::parse(args)?;
    let bytes = generated_profile_payload(&options)?;
    validate_profile_payload(&options.kind, &bytes)?;
    if let Some(parent) = options.out.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("cannot create {}: {err}", parent.display()))?;
    }
    fs::write(&options.out, &bytes)
        .map_err(|err| format!("cannot write {}: {err}", options.out.display()))?;
    print_json(json!({
        "version": 1,
        "command": "generate",
        "kind": options.kind.as_str(),
        "file": options.out.display().to_string(),
        "bytes": bytes.len(),
        "crc32c": checksum::crc32c(&bytes),
        "valid": true,
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
            "engine-registry" => EngineProfileRegistry::parse(&bytes).map(|_| ()),
            "execution-code" => ExecutionCodeDescriptorV1::parse(&bytes).map(|_| ()),
            "execution-scope" => ExecutionScopeDescriptorV1::parse(&bytes).map(|_| ()),
            "code-space" => CodeSpaceDescriptorV1::parse(&bytes).map(|_| ()),
            "mount-policy" => EngineMountPolicyV1::parse(&bytes).map(|_| ()),
            _ => return Err(
                "--kind must be one of: engine-registry, execution-code, execution-scope, code-space, mount-policy"
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProfilePayloadKind {
    EngineRegistry,
    ExecutionCode,
    ExecutionScope,
    CodeSpace,
    MountPolicy,
}

impl ProfilePayloadKind {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "engine-registry" => Ok(Self::EngineRegistry),
            "execution-code" => Ok(Self::ExecutionCode),
            "execution-scope" => Ok(Self::ExecutionScope),
            "code-space" => Ok(Self::CodeSpace),
            "mount-policy" => Ok(Self::MountPolicy),
            _ => Err("--kind must be one of: engine-registry, execution-code, execution-scope, code-space, mount-policy".into()),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::EngineRegistry => "engine-registry",
            Self::ExecutionCode => "execution-code",
            Self::ExecutionScope => "execution-scope",
            Self::CodeSpace => "code-space",
            Self::MountPolicy => "mount-policy",
        }
    }
}

#[derive(Debug, Clone)]
struct GenerateOptions {
    kind: ProfilePayloadKind,
    out: PathBuf,
    profile_id: u32,
    descriptor_id: u32,
    scope_id: u32,
    code_space_id: u32,
    policy_id: u32,
    namespace: String,
    name: String,
    stable_id: Vec<u8>,
    display_name: String,
    version_major: u16,
    version_minor: u16,
    required_features: u64,
    optional_features: u64,
    flags: u32,
    execution_descriptor_ref: u32,
    mount_policy_ref: u32,
    scope_ref: u32,
    code_space_ref: u32,
    dictionary_digest_ref: u32,
    cache_key_ref: u32,
    private_payload_ref: u32,
    epoch: u64,
}

impl GenerateOptions {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut options = Self::default();
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--kind" => {
                    options.kind = ProfilePayloadKind::parse(next_value(&mut iter, "--kind")?)?
                }
                "--out" => options.out = PathBuf::from(next_value(&mut iter, "--out")?),
                "--profile-id" => {
                    options.profile_id = parse_u32(next_value(&mut iter, "--profile-id")?)?
                }
                "--descriptor-id" => {
                    options.descriptor_id = parse_u32(next_value(&mut iter, "--descriptor-id")?)?
                }
                "--scope-id" => options.scope_id = parse_u32(next_value(&mut iter, "--scope-id")?)?,
                "--code-space-id" => {
                    options.code_space_id = parse_u32(next_value(&mut iter, "--code-space-id")?)?
                }
                "--policy-id" => {
                    options.policy_id = parse_u32(next_value(&mut iter, "--policy-id")?)?
                }
                "--namespace" => options.namespace = next_value(&mut iter, "--namespace")?.into(),
                "--name" | "--profile-name" => options.name = next_value(&mut iter, arg)?.into(),
                "--stable-id" => {
                    options.stable_id = next_value(&mut iter, "--stable-id")?.as_bytes().to_vec()
                }
                "--display-name" => {
                    options.display_name = next_value(&mut iter, "--display-name")?.into()
                }
                "--version-major" => {
                    options.version_major = parse_u16(next_value(&mut iter, "--version-major")?)?
                }
                "--version-minor" => {
                    options.version_minor = parse_u16(next_value(&mut iter, "--version-minor")?)?
                }
                "--required-features" => {
                    options.required_features =
                        parse_u64(next_value(&mut iter, "--required-features")?)?
                }
                "--optional-features" => {
                    options.optional_features =
                        parse_u64(next_value(&mut iter, "--optional-features")?)?
                }
                "--flags" => options.flags = parse_u32(next_value(&mut iter, "--flags")?)?,
                "--execution-descriptor-ref" => {
                    options.execution_descriptor_ref =
                        parse_u32(next_value(&mut iter, "--execution-descriptor-ref")?)?
                }
                "--mount-policy-ref" => {
                    options.mount_policy_ref =
                        parse_u32(next_value(&mut iter, "--mount-policy-ref")?)?
                }
                "--scope-ref" => {
                    options.scope_ref = parse_u32(next_value(&mut iter, "--scope-ref")?)?
                }
                "--code-space-ref" => {
                    options.code_space_ref = parse_u32(next_value(&mut iter, "--code-space-ref")?)?
                }
                "--dictionary-digest-ref" => {
                    options.dictionary_digest_ref =
                        parse_u32(next_value(&mut iter, "--dictionary-digest-ref")?)?
                }
                "--cache-key-ref" => {
                    options.cache_key_ref = parse_u32(next_value(&mut iter, "--cache-key-ref")?)?
                }
                "--private-payload-ref" => {
                    options.private_payload_ref =
                        parse_u32(next_value(&mut iter, "--private-payload-ref")?)?
                }
                "--epoch" => options.epoch = parse_u64(next_value(&mut iter, "--epoch")?)?,
                "-h" | "--help" => {
                    print_usage();
                    return Err("generate requires --kind and --out".into());
                }
                _ => return Err(format!("unknown generate option {arg}")),
            }
        }
        if options.out.as_os_str().is_empty() {
            return Err("generate requires --out <path>".into());
        }
        Ok(options)
    }
}

impl Default for GenerateOptions {
    fn default() -> Self {
        Self {
            kind: ProfilePayloadKind::ExecutionCode,
            out: PathBuf::new(),
            profile_id: 1,
            descriptor_id: 1,
            scope_id: 2,
            code_space_id: 3,
            policy_id: 1,
            namespace: "org.coveformat.reference".into(),
            name: "engine-dictionary-code".into(),
            stable_id: b"catalog/main".to_vec(),
            display_name: "main catalog".into(),
            version_major: 1,
            version_minor: 0,
            required_features: 0,
            optional_features: 0,
            flags: 0,
            execution_descriptor_ref: 1,
            mount_policy_ref: 1,
            scope_ref: 2,
            code_space_ref: 3,
            dictionary_digest_ref: 0,
            cache_key_ref: 0,
            private_payload_ref: 0,
            epoch: 7,
        }
    }
}

fn generated_profile_payload(options: &GenerateOptions) -> Result<Vec<u8>, String> {
    match options.kind {
        ProfilePayloadKind::EngineRegistry => EngineProfileRegistry {
            flags: options.flags,
            profiles: vec![EngineProfileEntryV1 {
                profile_id: options.profile_id,
                namespace: options.namespace.clone(),
                profile_name: options.name.clone(),
                version_major: options.version_major,
                version_minor: options.version_minor,
                required_features: options.required_features,
                optional_features: options.optional_features,
                execution_descriptor_ref: options.execution_descriptor_ref,
                mount_policy_ref: options.mount_policy_ref,
                private_payload_ref: options.private_payload_ref,
                checksum: 0,
            }],
        }
        .serialize()
        .map_err(|err| err.to_string()),
        ProfilePayloadKind::ExecutionCode => Ok(ExecutionCodeDescriptorV1 {
            descriptor_id: options.descriptor_id,
            code_kind: ExecutionCodeKind::DictionaryKey,
            code_width_bits: 32,
            byte_order: 0,
            lifetime: ExecutionCodeLifetime::Scan,
            comparison_scope: ExecutionCodeComparisonScope::File,
            canonicality: ExecutionCodeCanonicality::Transient,
            null_code_policy: NullCodePolicy::NullBitmapOnly,
            flags: options.flags,
            scope_ref: options.scope_ref,
            code_space_ref: options.code_space_ref,
            checksum: 0,
        }
        .serialize()
        .to_vec()),
        ProfilePayloadKind::ExecutionScope => ExecutionScopeDescriptorV1 {
            scope_id: options.scope_id,
            scope_kind: ExecutionScopeKind::Catalog,
            flags: u16::try_from(options.flags)
                .map_err(|_| "--flags exceeds u16 for execution-scope".to_string())?,
            stable_id: options.stable_id.clone(),
            display_name: options.display_name.clone(),
            private_payload_ref: options.private_payload_ref,
        }
        .serialize()
        .map_err(|err| err.to_string()),
        ProfilePayloadKind::CodeSpace => CodeSpaceDescriptorV1 {
            code_space_id: options.code_space_id,
            namespace: options.namespace.clone(),
            stable_id: options.stable_id.clone(),
            epoch: options.epoch,
            flags: options.flags,
            private_payload_ref: options.private_payload_ref,
        }
        .serialize()
        .map_err(|err| err.to_string()),
        ProfilePayloadKind::MountPolicy => Ok(EngineMountPolicyV1 {
            policy_id: options.policy_id,
            filecode_mapping_kind: FileCodeMappingKind::MapToExecutionCode,
            missing_value_policy: MissingValuePolicy::DecodeValueOnly,
            stale_mapping_policy: StaleMappingPolicy::IgnoreIfOptional,
            reverse_lookup_policy: ReverseLookupPolicy::BuildFromDictionary,
            flags: options.flags,
            dictionary_digest_ref: options.dictionary_digest_ref,
            code_space_ref: options.code_space_ref,
            cache_key_ref: options.cache_key_ref,
            private_payload_ref: options.private_payload_ref,
            checksum: 0,
        }
        .serialize()
        .to_vec()),
    }
}

fn validate_profile_payload(kind: &ProfilePayloadKind, bytes: &[u8]) -> Result<(), String> {
    match kind {
        ProfilePayloadKind::EngineRegistry => EngineProfileRegistry::parse(bytes).map(|_| ()),
        ProfilePayloadKind::ExecutionCode => ExecutionCodeDescriptorV1::parse(bytes).map(|_| ()),
        ProfilePayloadKind::ExecutionScope => ExecutionScopeDescriptorV1::parse(bytes).map(|_| ()),
        ProfilePayloadKind::CodeSpace => CodeSpaceDescriptorV1::parse(bytes).map(|_| ()),
        ProfilePayloadKind::MountPolicy => EngineMountPolicyV1::parse(bytes).map(|_| ()),
    }
    .map_err(|err| err.to_string())
}

fn next_value<'a>(
    iter: &mut std::slice::Iter<'a, String>,
    option: &str,
) -> Result<&'a str, String> {
    iter.next()
        .map(String::as_str)
        .ok_or_else(|| format!("{option} requires a value"))
}

fn parse_u16(value: &str) -> Result<u16, String> {
    parse_u64(value)?
        .try_into()
        .map_err(|_| format!("{value:?} exceeds u16"))
}

fn parse_u32(value: &str) -> Result<u32, String> {
    parse_u64(value)?
        .try_into()
        .map_err(|_| format!("{value:?} exceeds u32"))
}

fn parse_u64(value: &str) -> Result<u64, String> {
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        u64::from_str_radix(hex, 16).map_err(|err| format!("invalid integer {value:?}: {err}"))
    } else {
        value
            .parse::<u64>()
            .map_err(|err| format!("invalid integer {value:?}: {err}"))
    }
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
        "usage: cove-profile inspect <file.cove> | generate --kind <engine-registry|execution-code|execution-scope|code-space|mount-policy> --out <section.bin> | validate-section <section.bin> --kind <engine-registry|execution-code|execution-scope|code-space|mount-policy>"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_round_trips_all_profile_payload_kinds() {
        let dir = env::temp_dir().join(format!("cove-profile-generate-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for kind in [
            "engine-registry",
            "execution-code",
            "execution-scope",
            "code-space",
            "mount-policy",
        ] {
            let path = dir.join(format!("{kind}.bin"));
            assert!(run(vec![
                "generate".into(),
                "--kind".into(),
                kind.into(),
                "--out".into(),
                path.display().to_string(),
                "--namespace".into(),
                "org.cove.test".into(),
                "--name".into(),
                "engine-dictionary-code".into(),
            ])
            .unwrap());
            assert!(path.is_file());
            assert!(run(vec![
                "validate-section".into(),
                path.display().to_string(),
                "--kind".into(),
                kind.into(),
            ])
            .unwrap());
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn generate_rejects_bad_option() {
        let err = run(vec![
            "generate".into(),
            "--kind".into(),
            "execution-code".into(),
            "--out".into(),
            "ignored.bin".into(),
            "--bad".into(),
            "1".into(),
        ])
        .unwrap_err();
        assert!(err.contains("unknown generate option"));
    }
}
