//! Cove Format (COVE) v1.0 — Validity / null bitmap support.
//!
//! Null bitmap convention (Spec §6.6):
//! - bit 1 means **null**
//! - bit 0 means **non-null**
//!
//! Bits are packed LSB-first within each byte: row `i` is located at
//! byte `i / 8`, bit position `i % 8`.

use crate::CoveError;

/// A read-only view of a packed null bitmap.
///
/// Each bit corresponds to one logical row.  Per the COVE specification:
/// * bit = **1** → the row is **null**
/// * bit = **0** → the row is **non-null**
///
/// # Examples
///
/// ```
/// use cove_core::validity::ValidityBitmap;
///
/// // Two-byte bitmap for 10 rows; row 0 is null (bit 0 of byte 0 is set).
/// let bytes = [0b0000_0001u8, 0b0000_0000u8];
/// let bm = ValidityBitmap::new(&bytes, 10);
/// assert_eq!(bm.is_null(0).unwrap(), true);
/// assert_eq!(bm.is_null(1).unwrap(), false);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct ValidityBitmap<'a> {
    bytes: &'a [u8],
    row_count: u64,
}

impl<'a> ValidityBitmap<'a> {
    /// Creates a new `ValidityBitmap` view over `bytes` covering `row_count` rows.
    ///
    /// # Panics
    ///
    /// Does not panic.  [`is_null`](Self::is_null) will return
    /// [`CoveError::OffsetRange`] if an out-of-bounds row index is queried.
    pub fn new(bytes: &'a [u8], row_count: u64) -> Self {
        Self { bytes, row_count }
    }

    /// Returns the number of logical rows covered by this bitmap.
    pub fn row_count(&self) -> u64 {
        self.row_count
    }

    /// Returns `true` if the row at `row` is **null**, `false` if it is
    /// **non-null**.
    ///
    /// # Errors
    ///
    /// Returns [`CoveError::OffsetRange`] if `row >= row_count` or if the
    /// bitmap byte buffer is too small for the given `row`.
    pub fn is_null(&self, row: u64) -> Result<bool, CoveError> {
        if row >= self.row_count {
            return Err(CoveError::OffsetRange);
        }
        let byte_idx = (row / 8) as usize;
        let bit_idx = (row % 8) as u32;
        let byte = self
            .bytes
            .get(byte_idx)
            .copied()
            .ok_or(CoveError::OffsetRange)?;
        Ok((byte >> bit_idx) & 1 == 1)
    }

    /// Returns `true` if the row at `row` is **non-null**.
    ///
    /// This is the logical inverse of [`is_null`](Self::is_null).
    ///
    /// # Errors
    ///
    /// Returns [`CoveError::OffsetRange`] if `row >= row_count`.
    pub fn is_valid(&self, row: u64) -> Result<bool, CoveError> {
        self.is_null(row).map(|null| !null)
    }

    /// Validates that `self.bytes` is long enough to cover `row_count` rows.
    /// Returns `CoveError::ArithOverflow` on arithmetic overflow, `CoveError::BufferTooShort` if too small.
    pub fn validate_len(&self, row_count: u64) -> Result<(), CoveError> {
        let needed_bytes = row_count.checked_add(7).ok_or(CoveError::ArithOverflow)? / 8;
        if self.bytes.len() < needed_bytes as usize {
            return Err(CoveError::BufferTooShort);
        }
        Ok(())
    }

    /// Returns the null count by counting set bits over the entire bitmap.
    ///
    /// # Errors
    ///
    /// Returns [`CoveError::BufferTooShort`] if `bytes` is too short to cover
    /// `row_count` rows.
    pub fn null_count(&self) -> Result<u64, CoveError> {
        let needed_bytes = self
            .row_count
            .checked_add(7)
            .ok_or(CoveError::ArithOverflow)?
            / 8;
        let needed_bytes = needed_bytes as usize;
        if self.bytes.len() < needed_bytes {
            return Err(CoveError::BufferTooShort);
        }
        let full_words = needed_bytes / 8;
        let mut count = 0u64;
        for word_bytes in self.bytes[..full_words * 8].chunks_exact(8) {
            let word = u64::from_le_bytes(word_bytes.try_into().unwrap());
            count += word.count_ones() as u64;
        }
        let tail = &self.bytes[full_words * 8..needed_bytes];
        if !tail.is_empty() {
            let mut buf = [0u8; 8];
            buf[..tail.len()].copy_from_slice(tail);
            let mut word = u64::from_le_bytes(buf);
            let tail_bits = (self.row_count as usize).saturating_sub(full_words * 64);
            if tail_bits < 64 {
                let mask = (1u64 << tail_bits) - 1;
                word &= mask;
            }
            count += word.count_ones() as u64;
        }
        Ok(count)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Builder for constructing a null bitmap incrementally.
pub struct ValidityBitmapBuilder {
    bytes: Vec<u8>,
    row_count: u64,
}

impl ValidityBitmapBuilder {
    /// Creates a zeroed bitmap for `row_count` rows.
    /// Returns [`CoveError::ArithOverflow`] if the byte count overflows.
    pub fn new(row_count: u64) -> Result<Self, CoveError> {
        let needed_bytes = row_count.checked_add(7).ok_or(CoveError::ArithOverflow)? / 8;
        let bytes = vec![0u8; needed_bytes as usize];
        Ok(Self { bytes, row_count })
    }

    /// Marks `row` as null (sets bit to 1).
    /// Returns [`CoveError::OffsetRange`] if `row >= row_count`.
    pub fn set_null(&mut self, row: u64) -> Result<(), CoveError> {
        if row >= self.row_count {
            return Err(CoveError::OffsetRange);
        }
        let byte_idx = (row / 8) as usize;
        let bit_idx = (row % 8) as u32;
        self.bytes[byte_idx] |= 1 << bit_idx;
        Ok(())
    }

    /// Borrows the builder as a read-only [`ValidityBitmap`].
    pub fn as_bitmap(&self) -> ValidityBitmap<'_> {
        ValidityBitmap::new(&self.bytes, self.row_count)
    }

    /// Consumes the builder and returns the raw bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// Consumes the builder and returns `(row_count, bytes)`.
    pub fn finish(self) -> (u64, Vec<u8>) {
        (self.row_count, self.bytes)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_non_null() {
        let bytes = [0u8; 2];
        let bm = ValidityBitmap::new(&bytes, 16);
        for row in 0..16 {
            assert_eq!(
                bm.is_null(row).unwrap(),
                false,
                "row {row} should be non-null"
            );
        }
        assert_eq!(bm.null_count().unwrap(), 0);
    }

    #[test]
    fn all_null() {
        let bytes = [0xffu8; 2];
        let bm = ValidityBitmap::new(&bytes, 16);
        for row in 0..16 {
            assert_eq!(bm.is_null(row).unwrap(), true, "row {row} should be null");
        }
        assert_eq!(bm.null_count().unwrap(), 16);
    }

    #[test]
    fn alternating_bits() {
        // bits: 0b1010_1010 → rows 1,3,5,7 are null
        let bytes = [0b1010_1010u8];
        let bm = ValidityBitmap::new(&bytes, 8);
        for row in 0..8u64 {
            let expected_null = (row % 2) == 1;
            assert_eq!(
                bm.is_null(row).unwrap(),
                expected_null,
                "row {row} null mismatch"
            );
        }
        assert_eq!(bm.null_count().unwrap(), 4);
    }

    #[test]
    fn row_zero_null() {
        // bit 0 of byte 0 is set → row 0 is null
        let bytes = [0b0000_0001u8];
        let bm = ValidityBitmap::new(&bytes, 8);
        assert_eq!(bm.is_null(0).unwrap(), true);
        assert_eq!(bm.is_null(1).unwrap(), false);
        assert_eq!(bm.is_valid(0).unwrap(), false);
        assert_eq!(bm.is_valid(1).unwrap(), true);
    }

    #[test]
    fn out_of_range_row_returns_error() {
        let bytes = [0xffu8];
        let bm = ValidityBitmap::new(&bytes, 8);
        assert_eq!(bm.is_null(8), Err(CoveError::OffsetRange));
        assert_eq!(bm.is_null(100), Err(CoveError::OffsetRange));
    }

    #[test]
    fn partial_last_byte_null_count() {
        // 10 rows; byte 0 = 0b1111_1111 (rows 0-7 all null),
        //           byte 1 = 0b1111_1111 (only rows 8-9 count; top 6 bits ignored)
        let bytes = [0xffu8, 0xffu8];
        let bm = ValidityBitmap::new(&bytes, 10);
        assert_eq!(bm.null_count().unwrap(), 10);

        // byte 1 = 0b0000_0011 → rows 8 and 9 are null
        let bytes2 = [0xffu8, 0b0000_0011u8];
        let bm2 = ValidityBitmap::new(&bytes2, 10);
        assert_eq!(bm2.null_count().unwrap(), 10);

        // byte 1 = 0b0000_0000 → rows 8 and 9 are non-null
        let bytes3 = [0xffu8, 0b0000_0000u8];
        let bm3 = ValidityBitmap::new(&bytes3, 10);
        assert_eq!(bm3.null_count().unwrap(), 8);
    }

    #[test]
    fn buffer_too_short_returns_error() {
        // 10 rows need 2 bytes, but only 1 byte provided
        let bytes = [0xffu8];
        let bm = ValidityBitmap::new(&bytes, 10);
        assert_eq!(bm.null_count(), Err(CoveError::BufferTooShort));
    }

    #[test]
    fn single_row_bitmap() {
        let bytes_null = [0b0000_0001u8];
        let bm = ValidityBitmap::new(&bytes_null, 1);
        assert_eq!(bm.is_null(0).unwrap(), true);
        assert_eq!(bm.null_count().unwrap(), 1);

        let bytes_nonnull = [0b0000_0000u8];
        let bm2 = ValidityBitmap::new(&bytes_nonnull, 1);
        assert_eq!(bm2.is_null(0).unwrap(), false);
        assert_eq!(bm2.null_count().unwrap(), 0);
    }

    #[test]
    fn multi_byte_boundary_rows() {
        // Row 7 is in byte 0 bit 7; row 8 is in byte 1 bit 0.
        let bytes = [0b1000_0000u8, 0b0000_0001u8];
        let bm = ValidityBitmap::new(&bytes, 16);
        // Row 7: byte 0, bit 7 → set → null
        assert_eq!(bm.is_null(7).unwrap(), true);
        // Row 8: byte 1, bit 0 → set → null
        assert_eq!(bm.is_null(8).unwrap(), true);
        // Row 6: byte 0, bit 6 → unset → non-null
        assert_eq!(bm.is_null(6).unwrap(), false);
        // Row 9: byte 1, bit 1 → unset → non-null
        assert_eq!(bm.is_null(9).unwrap(), false);
    }

    #[test]
    fn validate_len_accepts_sufficient_buffer() {
        // 10 rows needs ceil(10/8)=2 bytes
        let bytes = [0u8; 2];
        let bm = ValidityBitmap::new(&bytes, 10);
        assert!(bm.validate_len(10).is_ok());
    }

    #[test]
    fn validate_len_rejects_short_buffer() {
        // 10 rows needs 2 bytes, but only 1 provided
        let bytes = [0u8; 1];
        let bm = ValidityBitmap::new(&bytes, 10);
        assert_eq!(bm.validate_len(10), Err(CoveError::BufferTooShort));
    }

    #[test]
    fn builder_sets_nulls_correctly() {
        let mut builder = ValidityBitmapBuilder::new(16).unwrap();
        builder.set_null(0).unwrap();
        builder.set_null(5).unwrap();
        builder.set_null(15).unwrap();
        let bm = builder.as_bitmap();
        assert_eq!(bm.is_null(0).unwrap(), true);
        assert_eq!(bm.is_null(1).unwrap(), false);
        assert_eq!(bm.is_null(5).unwrap(), true);
        assert_eq!(bm.is_null(15).unwrap(), true);
        assert_eq!(bm.is_null(14).unwrap(), false);
    }

    #[test]
    fn filecode_zero_and_numcode_zero_are_non_null_values() {
        // FileCode 0 and NumCode 0 are ordinary values — not null sentinels — per spec §6.4
        let bytes = [0u8; 1]; // all bits = 0, so no nulls
        let bm = ValidityBitmap::new(&bytes, 8);
        assert_eq!(bm.is_null(0).unwrap(), false);
    }
}
