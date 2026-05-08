use super::*;

/// Invert a COVE null bitmap into an Arrow validity bitmap with the same byte
/// length. Per Spec §49.2, the row count MUST be preserved exactly.
pub fn cove_null_to_arrow_validity(
    cove_null: &[u8],
    row_count: usize,
) -> Result<Vec<u8>, CoveError> {
    let needed = row_count.div_ceil(8);
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
    let needed = row_count.div_ceil(8);
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
