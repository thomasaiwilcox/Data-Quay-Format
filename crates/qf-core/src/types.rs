//! Quay Format (QF) v1.0 — Logical/physical type compatibility and NumCode helpers.
//!
//! Corresponds to Sections 18–19 of the QF v1.0 specification.

use crate::{
    constants::{QfLogicalType, QfPhysicalKind},
    QfError,
};

// ── Compatibility validation ───────────────────────────────────────────────────

/// Returns `Ok(())` if `logical` and `physical` form a valid combination, or
/// `Err(QfError::BadLogicalPhysicalPair)` if they are incompatible.
///
/// Compatibility matrix (Section 19):
///
/// | Physical kind | Allowed logical types |
/// |---------------|-----------------------|
/// | `FileCode`    | any non-container type (Null … Json, plus Uuid, Decimal128) |
/// | `NumCode`     | Bool, Int8–64, UInt8–64, Float32/64, Decimal64, DateDays, TimestampMicros/Nanos |
/// | `Boolean`     | Bool |
/// | `FixedBytes`  | Uuid, Decimal128 |
/// | `VarBytes`    | Utf8, Binary, Json |
/// | `List`        | List |
/// | `Struct`      | Struct |
/// | `Map`         | Map |
pub fn validate_logical_physical_pair(
    logical: QfLogicalType,
    physical: QfPhysicalKind,
) -> Result<(), QfError> {
    let ok = match physical {
        // FileCode accepts any scalar / variable-length / special type; the
        // dictionary maps it to a canonical logical value.  Container types
        // (List/Struct/Map) cannot be represented as flat dictionary codes.
        QfPhysicalKind::FileCode => !matches!(
            logical,
            QfLogicalType::List | QfLogicalType::Struct | QfLogicalType::Map
        ),

        // NumCode is restricted to numeric/temporal types only.
        // Bool is excluded: Spec §19.1 only permits Bool with NumCode when it
        // is "explicitly declared numeric", a constraint that cannot be enforced
        // from logical/physical kinds alone.
        QfPhysicalKind::NumCode => matches!(
            logical,
            QfLogicalType::Int8
                | QfLogicalType::Int16
                | QfLogicalType::Int32
                | QfLogicalType::Int64
                | QfLogicalType::UInt8
                | QfLogicalType::UInt16
                | QfLogicalType::UInt32
                | QfLogicalType::UInt64
                | QfLogicalType::Float32
                | QfLogicalType::Float64
                | QfLogicalType::Decimal64
                | QfLogicalType::DateDays
                | QfLogicalType::TimestampMicros
                | QfLogicalType::TimestampNanos
        ),

        // Boolean physical storage only makes sense for the Bool logical type.
        QfPhysicalKind::Boolean => matches!(logical, QfLogicalType::Bool),

        // FixedBytes stores fixed-width opaque byte sequences.
        QfPhysicalKind::FixedBytes => {
            matches!(logical, QfLogicalType::Uuid | QfLogicalType::Decimal128)
        }

        // VarBytes stores variable-length byte sequences.
        QfPhysicalKind::VarBytes => matches!(
            logical,
            QfLogicalType::Utf8 | QfLogicalType::Binary | QfLogicalType::Json
        ),

        // Container physical kinds must match their corresponding logical kind.
        QfPhysicalKind::List => matches!(logical, QfLogicalType::List),
        QfPhysicalKind::Struct => matches!(logical, QfLogicalType::Struct),
        QfPhysicalKind::Map => matches!(logical, QfLogicalType::Map),
    };

    if ok {
        Ok(())
    } else {
        Err(QfError::BadLogicalPhysicalPair)
    }
}

/// Returns `Ok(())` if `logical` is a valid logical type for a NumCode column,
/// or `Err(QfError::BadNumCode)` for unsupported types (e.g. Utf8, Binary,
/// Json, List, Struct, Map).
///
/// `Bool` is excluded: Spec §19.1 only permits it when "explicitly declared
/// numeric", a condition that cannot be expressed through logical type alone.
///
/// Per Spec §19.1, NumCode MUST NOT be dictionary-resolved and NumCode(0) is
/// an ordinary value — it MUST NOT be treated as null.
pub fn validate_numcode_logical_type(
    logical: QfLogicalType,
    bool_declared_numeric: bool,
) -> Result<(), QfError> {
    match logical {
        QfLogicalType::Bool if bool_declared_numeric => Ok(()),
        QfLogicalType::Int8
        | QfLogicalType::Int16
        | QfLogicalType::Int32
        | QfLogicalType::Int64
        | QfLogicalType::UInt8
        | QfLogicalType::UInt16
        | QfLogicalType::UInt32
        | QfLogicalType::UInt64
        | QfLogicalType::Float32
        | QfLogicalType::Float64
        | QfLogicalType::Decimal64
        | QfLogicalType::DateDays
        | QfLogicalType::TimestampMicros
        | QfLogicalType::TimestampNanos => Ok(()),
        _ => Err(QfError::BadNumCode),
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
            QfLogicalType::Int8,
            QfLogicalType::Int16,
            QfLogicalType::Int32,
            QfLogicalType::Int64,
            QfLogicalType::UInt8,
            QfLogicalType::UInt16,
            QfLogicalType::UInt32,
            QfLogicalType::UInt64,
            QfLogicalType::Float32,
            QfLogicalType::Float64,
            QfLogicalType::Decimal64,
            QfLogicalType::DateDays,
            QfLogicalType::TimestampMicros,
            QfLogicalType::TimestampNanos,
        ];
        for &lt in &allowed {
            assert!(
                validate_logical_physical_pair(lt, QfPhysicalKind::NumCode).is_ok(),
                "expected NumCode to accept {lt:?}"
            );
        }
    }

    #[test]
    fn numcode_rejects_unsupported_logical_types() {
        let rejected = [
            QfLogicalType::Null,
            QfLogicalType::Bool,
            QfLogicalType::Utf8,
            QfLogicalType::Binary,
            QfLogicalType::Json,
            QfLogicalType::List,
            QfLogicalType::Struct,
            QfLogicalType::Map,
            QfLogicalType::Decimal128,
            QfLogicalType::Uuid,
        ];
        for &lt in &rejected {
            assert_eq!(
                validate_logical_physical_pair(lt, QfPhysicalKind::NumCode),
                Err(QfError::BadLogicalPhysicalPair),
                "expected NumCode to reject {lt:?}"
            );
        }
    }

    #[test]
    fn filecode_accepts_scalar_types() {
        let allowed = [
            QfLogicalType::Null,
            QfLogicalType::Bool,
            QfLogicalType::Int64,
            QfLogicalType::UInt32,
            QfLogicalType::Float64,
            QfLogicalType::Decimal64,
            QfLogicalType::Decimal128,
            QfLogicalType::DateDays,
            QfLogicalType::TimestampMicros,
            QfLogicalType::TimestampNanos,
            QfLogicalType::Utf8,
            QfLogicalType::Binary,
            QfLogicalType::Json,
            QfLogicalType::Uuid,
        ];
        for &lt in &allowed {
            assert!(
                validate_logical_physical_pair(lt, QfPhysicalKind::FileCode).is_ok(),
                "expected FileCode to accept {lt:?}"
            );
        }
    }

    #[test]
    fn filecode_rejects_container_types() {
        for &lt in &[
            QfLogicalType::List,
            QfLogicalType::Struct,
            QfLogicalType::Map,
        ] {
            assert_eq!(
                validate_logical_physical_pair(lt, QfPhysicalKind::FileCode),
                Err(QfError::BadLogicalPhysicalPair),
                "expected FileCode to reject {lt:?}"
            );
        }
    }

    #[test]
    fn boolean_only_accepts_bool() {
        assert!(
            validate_logical_physical_pair(QfLogicalType::Bool, QfPhysicalKind::Boolean).is_ok()
        );
        assert_eq!(
            validate_logical_physical_pair(QfLogicalType::Int8, QfPhysicalKind::Boolean),
            Err(QfError::BadLogicalPhysicalPair)
        );
    }

    #[test]
    fn fixed_bytes_accepts_uuid_and_decimal128() {
        assert!(
            validate_logical_physical_pair(QfLogicalType::Uuid, QfPhysicalKind::FixedBytes).is_ok()
        );
        assert!(validate_logical_physical_pair(
            QfLogicalType::Decimal128,
            QfPhysicalKind::FixedBytes
        )
        .is_ok());
        assert_eq!(
            validate_logical_physical_pair(QfLogicalType::Int64, QfPhysicalKind::FixedBytes),
            Err(QfError::BadLogicalPhysicalPair)
        );
    }

    #[test]
    fn varbytes_accepts_text_and_binary() {
        for &lt in &[
            QfLogicalType::Utf8,
            QfLogicalType::Binary,
            QfLogicalType::Json,
        ] {
            assert!(
                validate_logical_physical_pair(lt, QfPhysicalKind::VarBytes).is_ok(),
                "expected VarBytes to accept {lt:?}"
            );
        }
        assert_eq!(
            validate_logical_physical_pair(QfLogicalType::Int64, QfPhysicalKind::VarBytes),
            Err(QfError::BadLogicalPhysicalPair)
        );
    }

    #[test]
    fn container_physical_kinds_match_their_logical_kind() {
        assert!(validate_logical_physical_pair(QfLogicalType::List, QfPhysicalKind::List).is_ok());
        assert!(
            validate_logical_physical_pair(QfLogicalType::Struct, QfPhysicalKind::Struct).is_ok()
        );
        assert!(validate_logical_physical_pair(QfLogicalType::Map, QfPhysicalKind::Map).is_ok());

        assert_eq!(
            validate_logical_physical_pair(QfLogicalType::Int64, QfPhysicalKind::List),
            Err(QfError::BadLogicalPhysicalPair)
        );
    }

    // ── validate_numcode_logical_type ─────────────────────────────────────────

    #[test]
    fn numcode_logical_allowed() {
        let allowed = [
            QfLogicalType::Int8,
            QfLogicalType::Int16,
            QfLogicalType::Int32,
            QfLogicalType::Int64,
            QfLogicalType::UInt8,
            QfLogicalType::UInt16,
            QfLogicalType::UInt32,
            QfLogicalType::UInt64,
            QfLogicalType::Float32,
            QfLogicalType::Float64,
            QfLogicalType::Decimal64,
            QfLogicalType::DateDays,
            QfLogicalType::TimestampMicros,
            QfLogicalType::TimestampNanos,
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
            QfLogicalType::Null,
            QfLogicalType::Bool,
            QfLogicalType::Utf8,
            QfLogicalType::Binary,
            QfLogicalType::Json,
            QfLogicalType::List,
            QfLogicalType::Struct,
            QfLogicalType::Map,
            QfLogicalType::Decimal128,
            QfLogicalType::Uuid,
        ];
        for &lt in &rejected {
            assert_eq!(
                validate_numcode_logical_type(lt, false),
                Err(QfError::BadNumCode),
                "expected NumCode logical type to reject {lt:?}"
            );
        }
    }

    // ── NumCode interpretation helpers ────────────────────────────────────────


    #[test]
    fn numcode_bool_allowed_when_explicitly_declared_numeric() {
        assert!(validate_numcode_logical_type(QfLogicalType::Bool, true).is_ok());
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
