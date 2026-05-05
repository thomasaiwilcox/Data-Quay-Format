//! Spec §49 — Arrow interop helpers.
//!
//! COVE stores nulls as a *null* bitmap (bit set ⇒ null), Arrow stores them as
//! a *validity* bitmap (bit set ⇒ valid). This module owns the bit inversion
//! and byte-aligned conversion required to bridge the two.

use crate::CoveError;

/// Invert a COVE null bitmap into an Arrow validity bitmap with the same byte
/// length. Per Spec §49.2, the row count MUST be preserved exactly.
pub fn cove_null_to_arrow_validity(
    cove_null: &[u8],
    row_count: usize,
) -> Result<Vec<u8>, CoveError> {
    let needed = (row_count + 7) / 8;
    if cove_null.len() < needed {
        return Err(CoveError::BufferTooShort);
    }
    let mut out = vec![0u8; needed];
    for row in 0..row_count {
        let byte = row / 8;
        let bit = 1u8 << (row % 8);
        let is_null = (cove_null[byte] & bit) != 0;
        if !is_null {
            out[byte] |= bit;
        }
    }
    Ok(out)
}

/// Invert an Arrow validity bitmap into a COVE null bitmap.
pub fn arrow_validity_to_cove_null(
    arrow_validity: &[u8],
    row_count: usize,
) -> Result<Vec<u8>, CoveError> {
    let needed = (row_count + 7) / 8;
    if arrow_validity.len() < needed {
        return Err(CoveError::BufferTooShort);
    }
    let mut out = vec![0u8; needed];
    for row in 0..row_count {
        let byte = row / 8;
        let bit = 1u8 << (row % 8);
        let is_valid = (arrow_validity[byte] & bit) != 0;
        if !is_valid {
            out[byte] |= bit;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_inversion_preserves_payload() {
        let cove = vec![0b0000_1010u8]; // rows 1 and 3 are null
        let arrow = cove_null_to_arrow_validity(&cove, 8).unwrap();
        // Arrow: bits 1 and 3 should be 0 (invalid), others 1 (valid).
        assert_eq!(arrow[0], !cove[0]);
        let back = arrow_validity_to_cove_null(&arrow, 8).unwrap();
        assert_eq!(back, cove);
    }

    #[test]
    fn partial_byte_only_iterates_row_count() {
        let cove = vec![0b1111_0000u8];
        let arrow = cove_null_to_arrow_validity(&cove, 4).unwrap();
        // Only the lower 4 bits of byte 0 are touched; high bits stay 0.
        assert_eq!(arrow[0] & 0b0000_1111, 0b0000_1111);
        assert_eq!(arrow[0] & 0b1111_0000, 0);
    }

    #[test]
    fn rejects_short_cove_null_bitmap() {
        assert_eq!(
            cove_null_to_arrow_validity(&[], 1),
            Err(CoveError::BufferTooShort)
        );
    }

    #[test]
    fn rejects_short_arrow_validity_bitmap() {
        assert_eq!(
            arrow_validity_to_cove_null(&[], 1),
            Err(CoveError::BufferTooShort)
        );
    }
}
