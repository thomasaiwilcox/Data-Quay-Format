//! Primitive wire-format helpers for COVE-Core.

use crate::CoveError;

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
pub fn decode_u64_leb128(bytes: &[u8]) -> Result<(u64, usize), CoveError> {
    let mut value = 0u64;
    let mut shift = 0u32;

    for (i, &byte) in bytes.iter().enumerate() {
        let low = u64::from(byte & 0x7f);

        // Guard the shift count before use: shift is maintained < 64 by the
        // end-of-iteration check below, but we assert it explicitly here so the
        // invariant is visible at the point of use.
        if shift >= 64 {
            return Err(CoveError::ArithOverflow);
        }

        // When fewer than 7 bits remain in u64, the 7-bit chunk must fit in
        // those remaining bits. This is the case on the 10th byte (shift == 63)
        // where only bit 63 is available, so `low` must be 0 or 1.
        // `checked_shl` only validates the shift count, not whether significant
        // bits would be discarded, so we must guard explicitly.
        let remaining_bits = 64u32 - shift;
        if remaining_bits < 7 && low >= (1u64 << remaining_bits) {
            return Err(CoveError::ArithOverflow);
        }

        value |= low << shift;

        if byte & 0x80 == 0 {
            return Ok((value, i + 1));
        }

        shift = shift.checked_add(7).ok_or(CoveError::ArithOverflow)?;
        if shift >= 64 {
            return Err(CoveError::ArithOverflow);
        }
    }

    Err(CoveError::BufferTooShort)
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
pub fn read_range_checked<'a>(
    bytes: &'a [u8],
    offset: usize,
    len: usize,
) -> Result<&'a [u8], CoveError> {
    let end = offset.checked_add(len).ok_or(CoveError::ArithOverflow)?;
    if end > bytes.len() {
        return Err(CoveError::OffsetRange);
    }
    Ok(&bytes[offset..end])
}

/// Reads a single byte at an offset.
pub fn read_u8_checked(bytes: &[u8], offset: usize) -> Result<u8, CoveError> {
    read_range_checked(bytes, offset, 1).map(|slice| slice[0])
}

/// Parses a strict boolean byte where only `0` and `1` are valid.
pub fn parse_bool_strict(byte: u8) -> Result<bool, CoveError> {
    match byte {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(CoveError::BadSection(format!(
            "invalid boolean byte: {byte}"
        ))),
    }
}

/// Reads a UUID from canonical 16-byte wire order.
pub fn read_uuid(bytes: &[u8], offset: usize) -> Result<[u8; 16], CoveError> {
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
        // 10 continuation bytes — shift overflows past 63 after byte 9.
        let malformed = [0x80u8; 10];
        assert_eq!(decode_u64_leb128(&malformed), Err(CoveError::ArithOverflow));

        // 10th byte has low=2 at shift=63: 2 << 63 would discard significant
        // bits, so the decoder must reject this before accepting the value.
        let overflow_high = {
            let b = [0xffu8; 9];
            let mut v = b.to_vec();
            v.push(0x02); // low=2, continuation=0; shift=63, remaining=1, 2 >= 2^1 → overflow
            v
        };
        assert_eq!(
            decode_u64_leb128(&overflow_high),
            Err(CoveError::ArithOverflow)
        );

        // 10th byte has low=1 at shift=63: exactly bit 63 — this is u64::MAX.
        let max_valid = {
            let mut v = vec![0xffu8; 9];
            v.push(0x01);
            v
        };
        let (val, consumed) = decode_u64_leb128(&max_valid).unwrap();
        assert_eq!(val, u64::MAX);
        assert_eq!(consumed, 10);

        let truncated = [0x80u8, 0x80u8, 0x80u8];
        assert_eq!(
            decode_u64_leb128(&truncated),
            Err(CoveError::BufferTooShort)
        );
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
        assert_eq!(read_range_checked(&data, 3, 2), Err(CoveError::OffsetRange));
        assert_eq!(
            read_range_checked(&data, usize::MAX, 1),
            Err(CoveError::ArithOverflow)
        );
    }
}
