//! Cove Format (COVE) v2.0 — Logical/physical type compatibility and NumCode helpers.
//!
//! Corresponds to Sections 18–19 of the COVE v2.0 specification.

use crate::{
    constants::{CoveLogicalType, CovePhysicalKind},
    CoveError,
};

// ── Compatibility validation ───────────────────────────────────────────────────

/// Context needed to validate logical/physical pairs whose compatibility
/// depends on an explicit declaration outside the logical/physical fields.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LogicalPhysicalOptions {
    pub bool_declared_numeric: bool,
}

/// Returns `Ok(())` if `logical` and `physical` form a valid combination, or
/// `Err(CoveError::BadLogicalPhysicalPair)` if they are incompatible.
///
/// Compatibility matrix (Section 19):
///
/// | Physical kind | Allowed logical types |
/// |---------------|-----------------------|
/// | `FileCode`    | any logical type encoded through the file dictionary |
/// | `NumCode`     | Bool, Int8–64, UInt8–64, Float32/64, Decimal64, DateDays, TimestampMicros/Nanos |
/// | `Boolean`     | Bool |
/// | `FixedBytes`  | Uuid, Decimal128 |
/// | `VarBytes`    | Utf8, Binary, Json |
/// | `List`        | List |
/// | `Struct`      | Struct |
/// | `Map`         | Map |
pub fn validate_logical_physical_pair(
    logical: CoveLogicalType,
    physical: CovePhysicalKind,
) -> Result<(), CoveError> {
    validate_logical_physical_pair_with_options(
        logical,
        physical,
        LogicalPhysicalOptions::default(),
    )
}

pub fn validate_logical_physical_pair_with_options(
    logical: CoveLogicalType,
    physical: CovePhysicalKind,
    options: LogicalPhysicalOptions,
) -> Result<(), CoveError> {
    let ok = match physical {
        // FileCode accepts every logical type; the dictionary maps each code
        // to a canonical logical value, including nested List/Struct/Map
        // payloads.
        CovePhysicalKind::FileCode => true,

        // NumCode is restricted to numeric/temporal types only. Bool requires
        // an explicit numeric declaration carried by the owning column/property.
        CovePhysicalKind::NumCode => {
            matches!(
                logical,
                CoveLogicalType::Bool if options.bool_declared_numeric
            ) || matches!(
                logical,
                CoveLogicalType::Int8
                    | CoveLogicalType::Int16
                    | CoveLogicalType::Int32
                    | CoveLogicalType::Int64
                    | CoveLogicalType::UInt8
                    | CoveLogicalType::UInt16
                    | CoveLogicalType::UInt32
                    | CoveLogicalType::UInt64
                    | CoveLogicalType::Float32
                    | CoveLogicalType::Float64
                    | CoveLogicalType::Decimal64
                    | CoveLogicalType::DateDays
                    | CoveLogicalType::TimestampMicros
                    | CoveLogicalType::TimestampNanos
            )
        }

        // Boolean physical storage only makes sense for the Bool logical type.
        CovePhysicalKind::Boolean => matches!(logical, CoveLogicalType::Bool),

        // FixedBytes stores fixed-width opaque byte sequences.
        CovePhysicalKind::FixedBytes => {
            matches!(logical, CoveLogicalType::Uuid | CoveLogicalType::Decimal128)
        }

        // VarBytes stores variable-length byte sequences.
        CovePhysicalKind::VarBytes => matches!(
            logical,
            CoveLogicalType::Utf8 | CoveLogicalType::Binary | CoveLogicalType::Json
        ),

        // Container physical kinds must match their corresponding logical kind.
        CovePhysicalKind::List => matches!(logical, CoveLogicalType::List),
        CovePhysicalKind::Struct => matches!(logical, CoveLogicalType::Struct),
        CovePhysicalKind::Map => matches!(logical, CoveLogicalType::Map),
    };

    if ok {
        Ok(())
    } else {
        Err(CoveError::BadLogicalPhysicalPair)
    }
}

/// Returns `Ok(())` if `logical` is a valid logical type for a NumCode column,
/// or `Err(CoveError::BadNumCode)` for unsupported types (e.g. Utf8, Binary,
/// Json, List, Struct, Map).
///
/// `Bool` is excluded: Spec §19.1 only permits it when "explicitly declared
/// numeric", a condition that cannot be expressed through logical type alone.
///
/// Per Spec §19.1, NumCode MUST NOT be dictionary-resolved and NumCode(0) is
/// an ordinary value — it MUST NOT be treated as null.
pub fn validate_numcode_logical_type(
    logical: CoveLogicalType,
    bool_declared_numeric: bool,
) -> Result<(), CoveError> {
    match logical {
        CoveLogicalType::Bool if bool_declared_numeric => Ok(()),
        CoveLogicalType::Int8
        | CoveLogicalType::Int16
        | CoveLogicalType::Int32
        | CoveLogicalType::Int64
        | CoveLogicalType::UInt8
        | CoveLogicalType::UInt16
        | CoveLogicalType::UInt32
        | CoveLogicalType::UInt64
        | CoveLogicalType::Float32
        | CoveLogicalType::Float64
        | CoveLogicalType::Decimal64
        | CoveLogicalType::DateDays
        | CoveLogicalType::TimestampMicros
        | CoveLogicalType::TimestampNanos => Ok(()),
        _ => Err(CoveError::BadNumCode),
    }
}

// ── NumCode interpretation helpers ────────────────────────────────────────────
//
// All helpers accept a raw `u64` NumCode value and reinterpret its bits
// according to the declared logical type.  Per spec §6.4:
//   - NumCode(0) is an ordinary value.
//   - NumCode(0) MUST NOT be treated as null.
//   - NumCode MUST NOT be dictionary-resolved.

/// Interprets the low 8 bits of `code` as an `i8`.
#[inline]
pub fn numcode_as_i8(code: u64) -> i8 {
    (code & 0xff) as i8
}

/// Interprets the low 16 bits of `code` as an `i16`.
#[inline]
pub fn numcode_as_i16(code: u64) -> i16 {
    (code & 0xffff) as i16
}

/// Interprets the low 32 bits of `code` as an `i32`.
#[inline]
pub fn numcode_as_i32(code: u64) -> i32 {
    (code & 0xffff_ffff) as i32
}

/// Interprets all 64 bits of `code` as an `i64`.
#[inline]
pub fn numcode_as_i64(code: u64) -> i64 {
    code as i64
}

/// Interprets the low 8 bits of `code` as a `u8`.
#[inline]
pub fn numcode_as_u8(code: u64) -> u8 {
    (code & 0xff) as u8
}

/// Interprets the low 16 bits of `code` as a `u16`.
#[inline]
pub fn numcode_as_u16(code: u64) -> u16 {
    (code & 0xffff) as u16
}

/// Interprets the low 32 bits of `code` as a `u32`.
#[inline]
pub fn numcode_as_u32(code: u64) -> u32 {
    (code & 0xffff_ffff) as u32
}

/// Returns the raw `u64` NumCode value (UInt64 logical type).
#[inline]
pub fn numcode_as_u64(code: u64) -> u64 {
    code
}

/// Interprets the low 32 bits of `code` as an IEEE 754 single-precision float.
///
/// Per spec §19.1: float values preserve raw IEEE bit patterns; NaN is valid.
#[inline]
pub fn numcode_as_f32(code: u64) -> f32 {
    f32::from_bits((code & 0xffff_ffff) as u32)
}

/// Interprets all 64 bits of `code` as an IEEE 754 double-precision float.
///
/// Per spec §19.1: float values preserve raw IEEE bit patterns; NaN is valid.
#[inline]
pub fn numcode_as_f64(code: u64) -> f64 {
    f64::from_bits(code)
}

/// Interprets all 64 bits of `code` as a signed `i64` for a `Decimal64` value.
///
/// Decimal64 stores a signed unscaled integer. The caller is responsible for
/// applying precision/scale metadata from the column schema to produce the
/// final decimal value.
#[inline]
pub fn numcode_as_decimal64(code: u64) -> i64 {
    code as i64
}

/// Interprets the low 32 bits of `code` as a signed day offset from the Unix
/// epoch (1970-01-01) for the `DateDays` logical type.
#[inline]
pub fn numcode_as_date_days(code: u64) -> i32 {
    (code & 0xffff_ffff) as i32
}

/// Interprets all 64 bits of `code` as a signed microsecond offset from the
/// Unix epoch for the `TimestampMicros` logical type.
#[inline]
pub fn numcode_as_timestamp_micros(code: u64) -> i64 {
    code as i64
}

/// Interprets all 64 bits of `code` as a signed nanosecond offset from the
/// Unix epoch for the `TimestampNanos` logical type.
#[inline]
pub fn numcode_as_timestamp_nanos(code: u64) -> i64 {
    code as i64
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── validate_logical_physical_pair ────────────────────────────────────────

    #[test]
    fn numcode_allows_numeric_and_temporal_types() {
        let allowed = [
            CoveLogicalType::Int8,
            CoveLogicalType::Int16,
            CoveLogicalType::Int32,
            CoveLogicalType::Int64,
            CoveLogicalType::UInt8,
            CoveLogicalType::UInt16,
            CoveLogicalType::UInt32,
            CoveLogicalType::UInt64,
            CoveLogicalType::Float32,
            CoveLogicalType::Float64,
            CoveLogicalType::Decimal64,
            CoveLogicalType::DateDays,
            CoveLogicalType::TimestampMicros,
            CoveLogicalType::TimestampNanos,
        ];
        for &lt in &allowed {
            assert!(
                validate_logical_physical_pair(lt, CovePhysicalKind::NumCode).is_ok(),
                "expected NumCode to accept {lt:?}"
            );
        }
    }

    #[test]
    fn numcode_rejects_unsupported_logical_types() {
        let rejected = [
            CoveLogicalType::Null,
            CoveLogicalType::Bool,
            CoveLogicalType::Utf8,
            CoveLogicalType::Binary,
            CoveLogicalType::Json,
            CoveLogicalType::List,
            CoveLogicalType::Struct,
            CoveLogicalType::Map,
            CoveLogicalType::Decimal128,
            CoveLogicalType::Uuid,
        ];
        for &lt in &rejected {
            assert_eq!(
                validate_logical_physical_pair(lt, CovePhysicalKind::NumCode),
                Err(CoveError::BadLogicalPhysicalPair),
                "expected NumCode to reject {lt:?}"
            );
        }
    }

    #[test]
    fn numcode_bool_pair_accepts_explicit_numeric_declaration() {
        assert!(validate_logical_physical_pair_with_options(
            CoveLogicalType::Bool,
            CovePhysicalKind::NumCode,
            LogicalPhysicalOptions {
                bool_declared_numeric: true,
            },
        )
        .is_ok());
    }

    #[test]
    fn filecode_accepts_dictionary_types() {
        let allowed = [
            CoveLogicalType::Null,
            CoveLogicalType::Bool,
            CoveLogicalType::Int64,
            CoveLogicalType::UInt32,
            CoveLogicalType::Float64,
            CoveLogicalType::Decimal64,
            CoveLogicalType::Decimal128,
            CoveLogicalType::DateDays,
            CoveLogicalType::TimestampMicros,
            CoveLogicalType::TimestampNanos,
            CoveLogicalType::Utf8,
            CoveLogicalType::Binary,
            CoveLogicalType::Json,
            CoveLogicalType::Uuid,
            CoveLogicalType::List,
            CoveLogicalType::Struct,
            CoveLogicalType::Map,
        ];
        for &lt in &allowed {
            assert!(
                validate_logical_physical_pair(lt, CovePhysicalKind::FileCode).is_ok(),
                "expected FileCode to accept {lt:?}"
            );
        }
    }

    #[test]
    fn boolean_only_accepts_bool() {
        assert!(
            validate_logical_physical_pair(CoveLogicalType::Bool, CovePhysicalKind::Boolean)
                .is_ok()
        );
        assert_eq!(
            validate_logical_physical_pair(CoveLogicalType::Int8, CovePhysicalKind::Boolean),
            Err(CoveError::BadLogicalPhysicalPair)
        );
    }

    #[test]
    fn fixed_bytes_accepts_uuid_and_decimal128() {
        assert!(validate_logical_physical_pair(
            CoveLogicalType::Uuid,
            CovePhysicalKind::FixedBytes
        )
        .is_ok());
        assert!(validate_logical_physical_pair(
            CoveLogicalType::Decimal128,
            CovePhysicalKind::FixedBytes
        )
        .is_ok());
        assert_eq!(
            validate_logical_physical_pair(CoveLogicalType::Int64, CovePhysicalKind::FixedBytes),
            Err(CoveError::BadLogicalPhysicalPair)
        );
    }

    #[test]
    fn varbytes_accepts_text_and_binary() {
        for &lt in &[
            CoveLogicalType::Utf8,
            CoveLogicalType::Binary,
            CoveLogicalType::Json,
        ] {
            assert!(
                validate_logical_physical_pair(lt, CovePhysicalKind::VarBytes).is_ok(),
                "expected VarBytes to accept {lt:?}"
            );
        }
        assert_eq!(
            validate_logical_physical_pair(CoveLogicalType::Int64, CovePhysicalKind::VarBytes),
            Err(CoveError::BadLogicalPhysicalPair)
        );
    }

    #[test]
    fn container_physical_kinds_match_their_logical_kind() {
        assert!(
            validate_logical_physical_pair(CoveLogicalType::List, CovePhysicalKind::List).is_ok()
        );
        assert!(
            validate_logical_physical_pair(CoveLogicalType::Struct, CovePhysicalKind::Struct)
                .is_ok()
        );
        assert!(
            validate_logical_physical_pair(CoveLogicalType::Map, CovePhysicalKind::Map).is_ok()
        );

        assert_eq!(
            validate_logical_physical_pair(CoveLogicalType::Int64, CovePhysicalKind::List),
            Err(CoveError::BadLogicalPhysicalPair)
        );
    }

    // ── validate_numcode_logical_type ─────────────────────────────────────────

    #[test]
    fn numcode_logical_allowed() {
        let allowed = [
            CoveLogicalType::Int8,
            CoveLogicalType::Int16,
            CoveLogicalType::Int32,
            CoveLogicalType::Int64,
            CoveLogicalType::UInt8,
            CoveLogicalType::UInt16,
            CoveLogicalType::UInt32,
            CoveLogicalType::UInt64,
            CoveLogicalType::Float32,
            CoveLogicalType::Float64,
            CoveLogicalType::Decimal64,
            CoveLogicalType::DateDays,
            CoveLogicalType::TimestampMicros,
            CoveLogicalType::TimestampNanos,
        ];
        for &lt in &allowed {
            assert!(
                validate_numcode_logical_type(lt, false).is_ok(),
                "expected NumCode logical type to accept {lt:?}"
            );
        }
    }

    #[test]
    fn numcode_logical_rejected() {
        let rejected = [
            CoveLogicalType::Null,
            CoveLogicalType::Bool,
            CoveLogicalType::Utf8,
            CoveLogicalType::Binary,
            CoveLogicalType::Json,
            CoveLogicalType::List,
            CoveLogicalType::Struct,
            CoveLogicalType::Map,
            CoveLogicalType::Decimal128,
            CoveLogicalType::Uuid,
        ];
        for &lt in &rejected {
            assert_eq!(
                validate_numcode_logical_type(lt, false),
                Err(CoveError::BadNumCode),
                "expected NumCode logical type to reject {lt:?}"
            );
        }
    }

    // ── NumCode interpretation helpers ────────────────────────────────────────

    #[test]
    fn numcode_bool_allowed_when_explicitly_declared_numeric() {
        assert!(validate_numcode_logical_type(CoveLogicalType::Bool, true).is_ok());
    }
    #[test]
    fn numcode_zero_is_ordinary_value() {
        // Spec §6.4: NumCode(0) is an ordinary value — MUST NOT be treated as null.
        assert_eq!(numcode_as_i8(0), 0i8);
        assert_eq!(numcode_as_i16(0), 0i16);
        assert_eq!(numcode_as_i32(0), 0i32);
        assert_eq!(numcode_as_i64(0), 0i64);
        assert_eq!(numcode_as_u8(0), 0u8);
        assert_eq!(numcode_as_u16(0), 0u16);
        assert_eq!(numcode_as_u32(0), 0u32);
        assert_eq!(numcode_as_u64(0), 0u64);
        assert_eq!(numcode_as_f32(0), 0.0f32);
        assert_eq!(numcode_as_f64(0), 0.0f64);
        assert_eq!(numcode_as_decimal64(0), 0i64);
        assert_eq!(numcode_as_date_days(0), 0i32);
        assert_eq!(numcode_as_timestamp_micros(0), 0i64);
        assert_eq!(numcode_as_timestamp_nanos(0), 0i64);
    }

    #[test]
    fn signed_integer_interpretation() {
        // i8: -1 stored as 0xff in the low byte.
        let neg1_u64 = 0x0000_0000_0000_00ffu64;
        assert_eq!(numcode_as_i8(neg1_u64), -1i8);

        // i16: -1 stored as 0xffff in the low 16 bits.
        let neg1_i16 = 0x0000_0000_0000_ffffu64;
        assert_eq!(numcode_as_i16(neg1_i16), -1i16);

        // i32: i32::MIN stored in the low 32 bits.
        let i32_min = (i32::MIN as u32) as u64;
        assert_eq!(numcode_as_i32(i32_min), i32::MIN);

        // i64: i64::MIN
        assert_eq!(numcode_as_i64(i64::MIN as u64), i64::MIN);
    }

    #[test]
    fn unsigned_integer_interpretation() {
        assert_eq!(numcode_as_u8(255), 255u8);
        assert_eq!(numcode_as_u16(0xffff), 0xffffu16);
        assert_eq!(numcode_as_u32(0xffff_ffff), u32::MAX);
        assert_eq!(numcode_as_u64(u64::MAX), u64::MAX);
    }

    #[test]
    fn float_interpretation_preserves_raw_bits() {
        // 1.0f32 → known bit pattern 0x3F80_0000
        let f32_one_bits: u64 = 0x3F80_0000;
        assert_eq!(numcode_as_f32(f32_one_bits), 1.0f32);

        // 1.0f64 → known bit pattern 0x3FF0_0000_0000_0000
        let f64_one_bits: u64 = 0x3FF0_0000_0000_0000;
        assert_eq!(numcode_as_f64(f64_one_bits), 1.0f64);
    }

    #[test]
    fn float_nan_is_valid() {
        // NaN values must be accepted (Spec §19.1 Float rules).
        let f32_nan_bits: u64 = f32::NAN.to_bits() as u64;
        assert!(numcode_as_f32(f32_nan_bits).is_nan());

        let f64_nan_bits: u64 = f64::NAN.to_bits();
        assert!(numcode_as_f64(f64_nan_bits).is_nan());
    }

    #[test]
    fn date_days_interpretation() {
        // Day 1 = 1970-01-02
        assert_eq!(numcode_as_date_days(1), 1i32);
        // Negative days = dates before epoch
        let neg = (-365i32) as u32 as u64;
        assert_eq!(numcode_as_date_days(neg), -365i32);
    }

    #[test]
    fn timestamp_interpretation() {
        let micros = 1_700_000_000_000_000u64;
        assert_eq!(numcode_as_timestamp_micros(micros), micros as i64);

        let nanos = 1_700_000_000_000_000_000u64;
        assert_eq!(numcode_as_timestamp_nanos(nanos), nanos as i64);

        // Negative (pre-epoch)
        let neg_micros = (-1_000_000i64) as u64;
        assert_eq!(numcode_as_timestamp_micros(neg_micros), -1_000_000i64);
    }

    #[test]
    fn decimal64_interprets_as_signed() {
        // A positive unscaled value round-trips through the signed interpretation.
        let pos: u64 = 0x0001_2345_6789_ABCD;
        assert_eq!(numcode_as_decimal64(pos), pos as i64);

        // A negative unscaled value is correctly reconstructed from two's complement.
        let neg_raw = (-1_000_000_i64) as u64;
        assert_eq!(numcode_as_decimal64(neg_raw), -1_000_000_i64);
    }
}
