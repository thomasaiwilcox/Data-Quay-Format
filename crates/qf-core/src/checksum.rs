//! Quay Format (QF) v1.0 — CRC32C utilities.
//!
//! QF uses CRC32C (Castagnoli) for corruption detection (Section 8.6).
//! CRC fields are computed over the covered byte range with the CRC field itself
//! set to zero when the covered structure contains its own CRC field.

/// Compute CRC32C over a byte slice.
///
/// Uses hardware-accelerated CRC32C (SSE 4.2 / ARM crypto) when available,
/// falling back to a software implementation.
pub fn crc32c(data: &[u8]) -> u32 {
    crc32c::crc32c(data)
}

/// Compute CRC32C over two disjoint slices, as if they were concatenated.
///
/// This is useful for computing a checksum over a structure while treating
/// the CRC field itself as zero.
pub fn crc32c_combine(a: &[u8], b: &[u8]) -> u32 {
    let partial = crc32c::crc32c(a);
    crc32c::crc32c_append(partial, b)
}

/// Verify that the CRC32C of `data` equals `expected`.
pub fn verify_crc32c(data: &[u8], expected: u32) -> bool {
    crc32c(data) == expected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32c_empty() {
        // CRC32C of empty input is 0.
        assert_eq!(crc32c(&[]), 0);
    }

    #[test]
    fn crc32c_known_value() {
        // CRC32C("123456789") == 0xe3069283 (standard test vector).
        assert_eq!(crc32c(b"123456789"), 0xe306_9283);
    }

    #[test]
    fn crc32c_combine_matches_contiguous() {
        let a = b"hello, ";
        let b = b"quay format";
        let mut combined_buf = Vec::new();
        combined_buf.extend_from_slice(a);
        combined_buf.extend_from_slice(b);
        let combined_direct = crc32c(&combined_buf);
        let combined_split = crc32c_combine(a, b);
        assert_eq!(combined_direct, combined_split);
    }

    #[test]
    fn verify_crc32c_passes_correct_checksum() {
        let data = b"some data bytes";
        let expected = crc32c(data);
        assert!(verify_crc32c(data, expected));
    }

    #[test]
    fn verify_crc32c_fails_wrong_checksum() {
        let data = b"some data bytes";
        assert!(!verify_crc32c(data, 0xdeadbeef));
    }
}
