use std::{env, fs, path::PathBuf, process::ExitCode};

use cove_core::{
    canonical::{validate_canonical_payload, CanonicalValue},
    constants::{CoveLogicalType, SectionKind, ValueTag},
    reader::{validate_bytes_with_options, ValidationOptions},
    utility::hex_encode,
    CoveError,
};
use serde_json::{json, Value};

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(success) if success => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(message) => {
            eprintln!("cove-canonicalise: {message}");
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
        "validate-payload" => validate_payload(rest),
        "encode-json" => encode_json(rest),
        "check-domain" => check_file(rest, SectionKind::ColumnDomain, "domain"),
        "check-trust" => check_file(rest, SectionKind::TrustManifest, "trust"),
        "-h" | "--help" => {
            print_usage();
            Ok(true)
        }
        _ => Err(format!("unknown command {command}")),
    }
}

fn validate_payload(args: &[String]) -> Result<bool, String> {
    let mut tag = None;
    let mut hex = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--tag" => tag = Some(next_value(&mut iter, "--tag")?.to_string()),
            "--hex" => hex = Some(next_value(&mut iter, "--hex")?.to_string()),
            _ => return Err(format!("unknown validate-payload option {arg}")),
        }
    }
    let tag = parse_value_tag(&tag.ok_or_else(|| "--tag is required".to_string())?)?;
    let payload = hex_decode(&hex.ok_or_else(|| "--hex is required".to_string())?)?;
    let result = validate_canonical_payload(tag, &payload);
    let valid = result.is_ok();
    let report = json!({
        "version": 1,
        "command": "validate-payload",
        "tag": format!("{tag:?}"),
        "payload_hex": hex_encode(&payload),
        "valid": valid,
        "error": result.as_ref().err().map(|err| err.to_string()),
    });
    print_json(report)?;
    Ok(valid)
}

fn encode_json(args: &[String]) -> Result<bool, String> {
    let mut logical = None;
    let mut value = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--logical" => logical = Some(next_value(&mut iter, "--logical")?.to_string()),
            "--value" => value = Some(next_value(&mut iter, "--value")?.to_string()),
            _ => return Err(format!("unknown encode-json option {arg}")),
        }
    }
    let logical = parse_logical(&logical.ok_or_else(|| "--logical is required".to_string())?)?;
    let value: Value =
        serde_json::from_str(&value.ok_or_else(|| "--value is required".to_string())?)
            .map_err(|err| format!("--value must be JSON: {err}"))?;
    let (tag, payload) = encode_value(logical, &value).map_err(|err| err.to_string())?;
    print_json(json!({
        "version": 1,
        "command": "encode-json",
        "logical": format!("{logical:?}"),
        "tag": format!("{tag:?}"),
        "payload_hex": hex_encode(&payload),
        "payload_len": payload.len(),
    }))?;
    Ok(true)
}

fn check_file(args: &[String], kind: SectionKind, label: &str) -> Result<bool, String> {
    if args.len() != 1 || args.iter().any(|arg| arg == "-h" || arg == "--help") {
        eprintln!("usage: cove-canonicalise check-{label} <file.cove>");
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
    );
    let (valid, error, section_count) = match report {
        Ok(report) => {
            let count = report
                .validated
                .footer
                .sections
                .iter()
                .filter(|entry| entry.section_kind == kind as u16)
                .count();
            (true, None, count)
        }
        Err(err) => (false, Some(err.to_string()), 0),
    };
    print_json(json!({
        "version": 1,
        "command": format!("check-{label}"),
        "file": path.display().to_string(),
        "valid": valid,
        "section_kind": format!("{kind:?}"),
        "section_count": section_count,
        "error": error,
    }))?;
    Ok(valid)
}

fn encode_value(logical: CoveLogicalType, value: &Value) -> Result<(ValueTag, Vec<u8>), CoveError> {
    let encoded = match logical {
        CoveLogicalType::Null => CanonicalValue::Null.encode(),
        CoveLogicalType::Bool => CanonicalValue::Bool(json_bool(value)?).encode(),
        CoveLogicalType::Int8
        | CoveLogicalType::Int16
        | CoveLogicalType::Int32
        | CoveLogicalType::Int64 => CanonicalValue::Int {
            width: integer_width(logical),
            value: i128::from(json_i64(value)?),
        }
        .encode(),
        CoveLogicalType::UInt8
        | CoveLogicalType::UInt16
        | CoveLogicalType::UInt32
        | CoveLogicalType::UInt64 => CanonicalValue::Uint {
            width: integer_width(logical),
            value: u128::from(json_u64(value)?),
        }
        .encode(),
        CoveLogicalType::Float32 => CanonicalValue::Float32(json_f64(value)? as f32).encode(),
        CoveLogicalType::Float64 => CanonicalValue::Float64(json_f64(value)?).encode(),
        CoveLogicalType::Decimal64 => CanonicalValue::Decimal64(json_i64(value)?).encode(),
        CoveLogicalType::Decimal128 => {
            let raw = value
                .as_str()
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| value.to_string());
            let parsed = raw.parse::<i128>().map_err(|_| {
                CoveError::BadSection("Decimal128 JSON value must be an integer string".into())
            })?;
            CanonicalValue::Decimal128(parsed).encode()
        }
        CoveLogicalType::DateDays => CanonicalValue::DateDays(json_i64(value)? as i32).encode(),
        CoveLogicalType::TimestampMicros => {
            CanonicalValue::TimestampMicros(json_i64(value)?).encode()
        }
        CoveLogicalType::TimestampNanos => {
            CanonicalValue::TimestampNanos(json_i64(value)?).encode()
        }
        CoveLogicalType::Utf8 => {
            let string = json_string(value)?;
            CanonicalValue::Utf8(string).encode()
        }
        CoveLogicalType::Binary => {
            let bytes = json_string(value)
                .and_then(hex_decode_cove)
                .or_else(|_| json_string(value).map(|string| string.as_bytes().to_vec()))?;
            let canonical = CanonicalValue::Bytes(&bytes);
            return canonical
                .encode()
                .map(|payload| (canonical.value_tag(), payload));
        }
        CoveLogicalType::Json => {
            let raw = value.to_string();
            let canonical = CanonicalValue::Json(&raw);
            return canonical
                .encode()
                .map(|payload| (canonical.value_tag(), payload));
        }
        CoveLogicalType::Uuid => {
            let bytes = hex_decode_cove(json_string(value)?)?;
            let uuid: [u8; 16] = bytes.try_into().map_err(|_| {
                CoveError::BadSection("Uuid JSON value must decode to 16 bytes".into())
            })?;
            CanonicalValue::Uuid(uuid).encode()
        }
        _ => Err(CoveError::UnsupportedEncoding(format!(
            "encode-json does not support {logical:?}"
        ))),
    }?;
    let tag = logical_value_tag(logical, value)?;
    Ok((tag, encoded))
}

fn logical_value_tag(logical: CoveLogicalType, value: &Value) -> Result<ValueTag, CoveError> {
    match logical {
        CoveLogicalType::Null => Ok(ValueTag::Null),
        CoveLogicalType::Bool => Ok(if json_bool(value)? {
            ValueTag::BoolTrue
        } else {
            ValueTag::BoolFalse
        }),
        CoveLogicalType::Int8
        | CoveLogicalType::Int16
        | CoveLogicalType::Int32
        | CoveLogicalType::Int64 => Ok(ValueTag::Int64),
        CoveLogicalType::UInt8
        | CoveLogicalType::UInt16
        | CoveLogicalType::UInt32
        | CoveLogicalType::UInt64 => Ok(ValueTag::UInt64),
        CoveLogicalType::Float32 => Ok(ValueTag::Float32Bits),
        CoveLogicalType::Float64 => Ok(ValueTag::Float64Bits),
        CoveLogicalType::Decimal64 => Ok(ValueTag::Decimal64),
        CoveLogicalType::Decimal128 => Ok(ValueTag::Decimal128),
        CoveLogicalType::DateDays => Ok(ValueTag::DateDays),
        CoveLogicalType::TimestampMicros => Ok(ValueTag::TimestampMicros),
        CoveLogicalType::TimestampNanos => Ok(ValueTag::TimestampNanos),
        CoveLogicalType::Utf8 => Ok(ValueTag::Utf8),
        CoveLogicalType::Binary => Ok(ValueTag::Binary),
        CoveLogicalType::Uuid => Ok(ValueTag::Uuid),
        CoveLogicalType::Json => Ok(ValueTag::Json),
        _ => Err(CoveError::UnsupportedEncoding(format!(
            "no scalar canonical tag for {logical:?}"
        ))),
    }
}

fn parse_value_tag(raw: &str) -> Result<ValueTag, String> {
    if let Ok(number) = raw.parse::<u16>() {
        return ValueTag::from_u16(number).ok_or_else(|| format!("unknown value tag {number}"));
    }
    match raw {
        "null" => Ok(ValueTag::Null),
        "bool-false" => Ok(ValueTag::BoolFalse),
        "bool-true" => Ok(ValueTag::BoolTrue),
        "int64" => Ok(ValueTag::Int64),
        "uint64" => Ok(ValueTag::UInt64),
        "float32" | "float32-bits" => Ok(ValueTag::Float32Bits),
        "float64" | "float64-bits" => Ok(ValueTag::Float64Bits),
        "decimal64" => Ok(ValueTag::Decimal64),
        "decimal128" => Ok(ValueTag::Decimal128),
        "date-days" => Ok(ValueTag::DateDays),
        "timestamp-micros" => Ok(ValueTag::TimestampMicros),
        "timestamp-nanos" => Ok(ValueTag::TimestampNanos),
        "utf8" => Ok(ValueTag::Utf8),
        "binary" => Ok(ValueTag::Binary),
        "uuid" => Ok(ValueTag::Uuid),
        "json" => Ok(ValueTag::Json),
        "list" => Ok(ValueTag::List),
        "struct" => Ok(ValueTag::Struct),
        "map" => Ok(ValueTag::Map),
        _ => Err(format!("unknown value tag {raw}")),
    }
}

fn parse_logical(raw: &str) -> Result<CoveLogicalType, String> {
    match raw {
        "null" => Ok(CoveLogicalType::Null),
        "bool" => Ok(CoveLogicalType::Bool),
        "int8" => Ok(CoveLogicalType::Int8),
        "int16" => Ok(CoveLogicalType::Int16),
        "int32" => Ok(CoveLogicalType::Int32),
        "int64" => Ok(CoveLogicalType::Int64),
        "uint8" => Ok(CoveLogicalType::UInt8),
        "uint16" => Ok(CoveLogicalType::UInt16),
        "uint32" => Ok(CoveLogicalType::UInt32),
        "uint64" => Ok(CoveLogicalType::UInt64),
        "float32" => Ok(CoveLogicalType::Float32),
        "float64" => Ok(CoveLogicalType::Float64),
        "decimal64" => Ok(CoveLogicalType::Decimal64),
        "decimal128" => Ok(CoveLogicalType::Decimal128),
        "date-days" => Ok(CoveLogicalType::DateDays),
        "timestamp-micros" => Ok(CoveLogicalType::TimestampMicros),
        "timestamp-nanos" => Ok(CoveLogicalType::TimestampNanos),
        "utf8" => Ok(CoveLogicalType::Utf8),
        "binary" => Ok(CoveLogicalType::Binary),
        "uuid" => Ok(CoveLogicalType::Uuid),
        "json" => Ok(CoveLogicalType::Json),
        _ => Err(format!("unknown logical type {raw}")),
    }
}

fn json_bool(value: &Value) -> Result<bool, CoveError> {
    value
        .as_bool()
        .ok_or_else(|| CoveError::BadSection("JSON value must be a bool".into()))
}

fn json_i64(value: &Value) -> Result<i64, CoveError> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|raw| raw.parse::<i64>().ok()))
        .ok_or_else(|| CoveError::BadSection("JSON value must be an i64".into()))
}

fn json_u64(value: &Value) -> Result<u64, CoveError> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|raw| raw.parse::<u64>().ok()))
        .ok_or_else(|| CoveError::BadSection("JSON value must be a u64".into()))
}

fn json_f64(value: &Value) -> Result<f64, CoveError> {
    let parsed = value
        .as_f64()
        .or_else(|| value.as_str().and_then(|raw| raw.parse::<f64>().ok()))
        .ok_or_else(|| CoveError::BadSection("JSON value must be an f64".into()))?;
    if parsed.is_nan() {
        return Err(CoveError::BadSection(
            "NaN canonical JSON values are not supported".into(),
        ));
    }
    Ok(parsed)
}

fn json_string(value: &Value) -> Result<&str, CoveError> {
    value
        .as_str()
        .ok_or_else(|| CoveError::BadSection("JSON value must be a string".into()))
}

fn integer_width(logical: CoveLogicalType) -> u8 {
    match logical {
        CoveLogicalType::Int8 | CoveLogicalType::UInt8 => 1,
        CoveLogicalType::Int16 | CoveLogicalType::UInt16 => 2,
        CoveLogicalType::Int32 | CoveLogicalType::UInt32 | CoveLogicalType::DateDays => 4,
        _ => 8,
    }
}

fn hex_decode(raw: &str) -> Result<Vec<u8>, String> {
    hex_decode_cove(raw).map_err(|err| err.to_string())
}

fn hex_decode_cove(raw: &str) -> Result<Vec<u8>, CoveError> {
    let raw = raw.strip_prefix("0x").unwrap_or(raw);
    if !raw.len().is_multiple_of(2) {
        return Err(CoveError::BadSection("hex input has odd length".into()));
    }
    let mut out = Vec::with_capacity(raw.len() / 2);
    for pair in raw.as_bytes().chunks_exact(2) {
        out.push((hex_digit(pair[0])? << 4) | hex_digit(pair[1])?);
    }
    Ok(out)
}

fn hex_digit(byte: u8) -> Result<u8, CoveError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(CoveError::BadSection("invalid hex digit".into())),
    }
}

fn next_value<'a>(
    iter: &mut impl Iterator<Item = &'a String>,
    option: &str,
) -> Result<&'a str, String> {
    iter.next()
        .map(String::as_str)
        .ok_or_else(|| format!("{option} requires a value"))
}

fn print_json(value: Value) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(&value)
            .map_err(|err| format!("cannot serialize report: {err}"))?
    );
    Ok(())
}

fn print_usage() {
    eprintln!(
        "usage: cove-canonicalise validate-payload --tag <tag> --hex <payload> | encode-json --logical <type> --value <json> | check-domain <file.cove> | check-trust <file.cove>"
    );
}
