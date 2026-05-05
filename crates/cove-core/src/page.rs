//! Cove Format (COVE) v1.0 — Column page index and page header (Spec §27).
//!
//! A *page* is the smallest physically encoded unit in a column. Each page
//! header records its row count, null count, encoded byte length, encoding
//! kind, and CRC32C. The reader uses page headers to bounds-check decode and
//! to drive the canonical / fast / kernel decode path triad described in
//! Spec §20.

use crate::{checksum::crc32c, constants::CoveEncodingKind, CoveError};

/// One column page entry in the page index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageEntry {
    pub page_id: u32,
    pub morsel_id: u32,
    /// Number of rows encoded by this page.
    pub row_count: u32,
    /// Number of null rows in this page (Spec §27.3).
    pub null_count: u32,
    /// Byte offset within the section payload where this page starts.
    pub offset: u64,
    /// Encoded byte length of this page.
    pub length: u64,
    /// Encoding kind used for this page (Spec §20.1).
    pub encoding: CoveEncodingKind,
    /// CRC32C of the page bytes (Spec §27.4).
    pub crc32c: u32,
}

impl PageEntry {
    /// Validate this page's CRC against `payload`.
    pub fn verify_crc(&self, payload: &[u8]) -> Result<(), CoveError> {
        let end = self
            .offset
            .checked_add(self.length)
            .ok_or(CoveError::ArithOverflow)?;
        if end as usize > payload.len() {
            return Err(CoveError::OffsetRange);
        }
        let actual = crc32c(&payload[self.offset as usize..end as usize]);
        if actual != self.crc32c {
            Err(CoveError::PageCorrupt)
        } else {
            Ok(())
        }
    }

    /// Number of non-null rows. The §27.3 invariant
    /// `null_count + non_null_count == row_count` is enforced at parse time.
    pub fn non_null_count(&self) -> u32 {
        self.row_count - self.null_count
    }
}

/// A parsed column page index for a single column.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageIndex {
    pub entries: Vec<PageEntry>,
}

impl PageIndex {
    /// Wire format (LE):
    ///   `u32` count
    ///   For each entry: `u32` page_id, `u32` morsel_id, `u32` row_count,
    ///                   `u32` null_count, `u64` offset, `u64` length,
    ///                   `u16` encoding, `u32` crc32c.
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < 4 {
            return Err(CoveError::BufferTooShort);
        }
        let count = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let entry_size = 4usize + 4 + 4 + 4 + 8 + 8 + 2 + 4;
        let entries_bytes = count
            .checked_mul(entry_size)
            .ok_or(CoveError::ArithOverflow)?;
        let required_len = 4usize
            .checked_add(entries_bytes)
            .ok_or(CoveError::ArithOverflow)?;
        if required_len > bytes.len() {
            return Err(CoveError::BufferTooShort);
        }
        let mut entries = Vec::with_capacity(count);
        let mut pos = 4usize;
        for _ in 0..count {
            let page_id = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
            pos += 4;
            let morsel_id = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
            pos += 4;
            let row_count = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
            pos += 4;
            let null_count = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
            pos += 4;
            let offset = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
            pos += 8;
            let length = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
            pos += 8;
            let enc_raw = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap());
            pos += 2;
            let crc = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
            pos += 4;
            if null_count > row_count {
                return Err(CoveError::PageCorrupt);
            }
            let encoding = CoveEncodingKind::from_u16(enc_raw)
                .ok_or_else(|| CoveError::BadSection(format!("unknown encoding {enc_raw}")))?;
            entries.push(PageEntry {
                page_id,
                morsel_id,
                row_count,
                null_count,
                offset,
                length,
                encoding,
                crc32c: crc,
            });
        }
        Ok(Self { entries })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_page_bytes(entries: &[(u32, u32, u32, u32, u64, u64, u16, u32)]) -> Vec<u8> {
        let mut out = (entries.len() as u32).to_le_bytes().to_vec();
        for (pid, mid, rc, nc, off, len, enc, crc) in entries {
            out.extend_from_slice(&pid.to_le_bytes());
            out.extend_from_slice(&mid.to_le_bytes());
            out.extend_from_slice(&rc.to_le_bytes());
            out.extend_from_slice(&nc.to_le_bytes());
            out.extend_from_slice(&off.to_le_bytes());
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&enc.to_le_bytes());
            out.extend_from_slice(&crc.to_le_bytes());
        }
        out
    }

    #[test]
    fn round_trip_index() {
        let payload = b"some bytes for a fake page";
        let crc = crc32c(payload);
        let bytes = make_page_bytes(&[(0, 0, 4, 1, 0, payload.len() as u64, 0, crc)]);
        let idx = PageIndex::parse(&bytes).unwrap();
        assert_eq!(idx.entries[0].non_null_count(), 3);
        assert!(idx.entries[0].verify_crc(payload).is_ok());
    }

    #[test]
    fn rejects_null_count_above_row_count() {
        let bytes = make_page_bytes(&[(0, 0, 4, 5, 0, 0, 0, 0)]);
        assert_eq!(PageIndex::parse(&bytes), Err(CoveError::PageCorrupt));
    }

    #[test]
    fn rejects_unknown_encoding() {
        let bytes = make_page_bytes(&[(0, 0, 1, 0, 0, 0, 0xfffe, 0)]);
        assert!(matches!(
            PageIndex::parse(&bytes),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn detects_page_crc_mismatch() {
        let bytes = make_page_bytes(&[(0, 0, 1, 0, 0, 5, 0, 0)]);
        let idx = PageIndex::parse(&bytes).unwrap();
        assert_eq!(
            idx.entries[0].verify_crc(b"hello"),
            Err(CoveError::PageCorrupt)
        );
    }

    #[test]
    fn rejects_oversized_entry_count_before_allocating() {
        let bytes = u32::MAX.to_le_bytes().to_vec();
        assert_eq!(PageIndex::parse(&bytes), Err(CoveError::BufferTooShort));
    }
}
