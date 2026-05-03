//! Primitive wire-format helpers for QF-Core.

use crate::QfError;

/// Encodes an unsigned `u64` as LEB128 bytes.
pub fn encode_u64_leb128(mut value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
    out
}

/// Decodes an unsigned `u64` from LEB128 bytes.
///
/// Returns the decoded value and number of bytes consumed.
pub fn decode_u64_leb128(bytes: &[u8]) -> Result<(u64, usize), QfError> {
    let mut value = 0u64;
    let mut shift = 0u32;

    for (i, &byte) in bytes.iter().enumerate() {
        let low = u64::from(byte & 0x7f);
        value |= low.checked_shl(shift).ok_or(QfError::ArithOverflow)?;

        if byte & 0x80 == 0 {
            return Ok((value, i + 1));
        }

        shift = shift.checked_add(7).ok_or(QfError::ArithOverflow)?;
        if shift >= 64 {
            return Err(QfError::ArithOverflow);
        }
    }

    Err(QfError::BufferTooShort)
}

/// ZigZag-encodes an `i64` to `u64`.
pub fn zigzag_encode_i64(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}

/// ZigZag-decodes a `u64` to `i64`.
pub fn zigzag_decode_i64(value: u64) -> i64 {
    ((value >> 1) as i64) ^ (-((value & 1) as i64))
}

/// Reads a bounded range from `bytes` using checked offset arithmetic.
pub fn read_range_checked<'a>(bytes: &'a [u8], offset: usize, len: usize) -> Result<&'a [u8], QfError> {
    let end = offset.checked_add(len).ok_or(QfError::ArithOverflow)?;
    if end > bytes.len() {
        return Err(QfError::OffsetRange);
    }
    Ok(&bytes[offset..end])
}

/// Reads a single byte at an offset.
pub fn read_u8_checked(bytes: &[u8], offset: usize) -> Result<u8, QfError> {
    read_range_checked(bytes, offset, 1).map(|slice| slice[0])
}

/// Parses a strict boolean byte where only `0` and `1` are valid.
pub fn parse_bool_strict(byte: u8) -> Result<bool, QfError> {
    match byte {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(QfError::BadSection(format!("invalid boolean byte: {byte}"))),
    }
}

/// Reads a UUID from canonical 16-byte wire order.
pub fn read_uuid(bytes: &[u8], offset: usize) -> Result<[u8; 16], QfError> {
    let raw = read_range_checked(bytes, offset, 16)?;
    let mut out = [0u8; 16];
    out.copy_from_slice(raw);
    Ok(out)
}

/// Writes a UUID in canonical 16-byte wire order.
pub fn write_uuid(dst: &mut Vec<u8>, uuid: [u8; 16]) {
    dst.extend_from_slice(&uuid);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_round_trips() {
        let values = [
            0u64,
            1,
            127,
            128,
            255,
            300,
            16_384,
            u32::MAX as u64,
            u64::MAX,
        ];

        for &v in &values {
            let encoded = encode_u64_leb128(v);
            let (decoded, consumed) = decode_u64_leb128(&encoded).unwrap();
            assert_eq!(decoded, v);
            assert_eq!(consumed, encoded.len());
        }
    }

    #[test]
    fn malformed_and_truncated_varints() {
        let malformed = [0x80u8; 10];
        assert_eq!(decode_u64_leb128(&malformed), Err(QfError::ArithOverflow));

        let truncated = [0x80u8, 0x80u8, 0x80u8];
        assert_eq!(decode_u64_leb128(&truncated), Err(QfError::BufferTooShort));
    }

    #[test]
    fn zigzag_negative_positive_round_trips() {
        let values = [i64::MIN, -10_000, -1, 0, 1, 42, i64::MAX];
        for &v in &values {
            let encoded = zigzag_encode_i64(v);
            let decoded = zigzag_decode_i64(encoded);
            assert_eq!(decoded, v);
        }
    }

    #[test]
    fn strict_bool_rejects_non_zero_one() {
        assert_eq!(parse_bool_strict(0), Ok(false));
        assert_eq!(parse_bool_strict(1), Ok(true));
        assert!(parse_bool_strict(2).is_err());
        assert!(parse_bool_strict(255).is_err());
    }

    #[test]
    fn checked_range_overflow() {
        let data = [1u8, 2, 3, 4];
        assert_eq!(read_range_checked(&data, 1, 2).unwrap(), &[2, 3]);
        assert_eq!(read_range_checked(&data, 3, 2), Err(QfError::OffsetRange));
        assert_eq!(read_range_checked(&data, usize::MAX, 1), Err(QfError::ArithOverflow));
    }
}
