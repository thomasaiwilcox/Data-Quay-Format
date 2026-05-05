//! Quay Format (QF) v1.0 — Optional indexes (Spec §30–§36).
//!
//! Indexes are *optional acceleration*. Spec §73 mandates that a scan MUST
//! always be able to produce correct results even if every optional index is
//! corrupt or missing — readers fall back to decode-and-filter on any
//! checksum or staleness failure.
//!
//! Each submodule implements one of the seven approved v1 indexes:
//! * [`exact_set`] — Spec §30.
//! * [`bloom`] — Spec §31.
//! * [`inverted`] — Spec §32.
//! * [`lookup`] — Spec §33.
//! * [`aggregate`] — Spec §34.
//! * [`composite`] — Spec §35.
//! * [`topn`] — Spec §36.

pub mod aggregate;
pub mod bloom;
pub mod composite;
pub mod exact_set;
pub mod inverted;
pub mod lookup;
pub mod topn;

use crate::{checksum, QfError};

pub(crate) fn verify_checksum_field(bytes: &[u8], checksum_offset: usize) -> Result<u32, QfError> {
    if checksum_offset
        .checked_add(4)
        .ok_or(QfError::ArithOverflow)?
        > bytes.len()
    {
        return Err(QfError::BufferTooShort);
    }
    let checksum_field = u32::from_le_bytes(
        bytes[checksum_offset..checksum_offset + 4]
            .try_into()
            .unwrap(),
    );
    let mut for_crc = bytes.to_vec();
    for_crc[checksum_offset..checksum_offset + 4].fill(0);
    if checksum::crc32c(&for_crc) != checksum_field {
        return Err(QfError::ChecksumMismatch);
    }
    Ok(checksum_field)
}

pub(crate) fn checked_region(bytes: &[u8], offset: u64, length: u64) -> Result<&[u8], QfError> {
    let start = usize::try_from(offset).map_err(|_| QfError::OffsetRange)?;
    let len = usize::try_from(length).map_err(|_| QfError::OffsetRange)?;
    let end = start.checked_add(len).ok_or(QfError::ArithOverflow)?;
    if end > bytes.len() {
        return Err(QfError::OffsetRange);
    }
    Ok(&bytes[start..end])
}
