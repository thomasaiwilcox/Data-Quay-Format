//! Spec §23 — ColumnDomain (spec-exact wire format).
//!
//! A ColumnDomain defines logical ordering for FileCode columns. Raw
//! FileCode numeric order has no semantic meaning; range pruning MUST go
//! through the ColumnDomain rank map.
//!
//! Wire layout of a `ColumnDomain` section payload:
//!
//! ```text
//! [ColumnDomainHeaderV1            : 40 bytes]
//! [sorted_file_codes  : FileCode[domain_count]   at sorted_file_codes_offset]
//! [file_code_to_rank  : u32[rank_map_entry_count] at file_code_to_rank_offset]
//! ```
//!
//! Offsets are relative to the start of the section payload. `FileCode` is
//! a `u32` per Spec §6.1.
//!
//! Spec §23 Rules enforced by this module:
//! * `sorted_file_codes` MUST be sorted in ascending order; duplicates are
//!   rejected.
//! * `file_code_to_rank` maps `FileCode` → domain rank.
//! * Values absent from the column MAY map to [`INVALID_RANK`].
//! * Readers MUST validate ranks before using domain min/max — see
//!   [`ColumnDomain::is_safe`].
//! * If no safe ordering exists, range pushdown MUST be disabled.

use crate::checksum;
use crate::error::QfError;

// ── Constants ────────────────────────────────────────────────────────────────

/// Encoded length of [`ColumnDomainHeaderV1`] in bytes.
///
/// Layout: table_or_object_id(4) + column_or_property_id(4) + logical_type(2)
///       + collation_id(2) + domain_count(4) + sorted_file_codes_offset(8)
///       + file_code_to_rank_offset(8) + flags(4) + checksum(4) = 40.
pub const COLUMN_DOMAIN_HEADER_LEN: usize = 40;

/// Sentinel rank value indicating that a FileCode is not in the domain.
/// Spec §23 permits values absent from the column to map to `INVALID_RANK`.
pub const INVALID_RANK: u32 = u32::MAX;

/// Flag bit: domain belongs to a QF-O object/property pair (cleared = QF-T
/// table/column). Implementation-defined helper bit.
pub const FLAG_OBJECT_DOMAIN: u32 = 1 << 0;

// ── ColumnDomainHeaderV1 ─────────────────────────────────────────────────────

/// Spec §23 `ColumnDomainHeaderV1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnDomainHeaderV1 {
    pub table_or_object_id: u32,
    pub column_or_property_id: u32,
    pub logical_type: u16,
    pub collation_id: u16,
    pub domain_count: u32,
    pub sorted_file_codes_offset: u64,
    pub file_code_to_rank_offset: u64,
    pub flags: u32,
    /// CRC32C of the 40-byte header with `checksum` zeroed.
    pub checksum: u32,
}

impl ColumnDomainHeaderV1 {
    pub fn serialize(&self) -> [u8; COLUMN_DOMAIN_HEADER_LEN] {
        let mut buf = [0u8; COLUMN_DOMAIN_HEADER_LEN];
        buf[0..4].copy_from_slice(&self.table_or_object_id.to_le_bytes());
        buf[4..8].copy_from_slice(&self.column_or_property_id.to_le_bytes());
        buf[8..10].copy_from_slice(&self.logical_type.to_le_bytes());
        buf[10..12].copy_from_slice(&self.collation_id.to_le_bytes());
        buf[12..16].copy_from_slice(&self.domain_count.to_le_bytes());
        buf[16..24].copy_from_slice(&self.sorted_file_codes_offset.to_le_bytes());
        buf[24..32].copy_from_slice(&self.file_code_to_rank_offset.to_le_bytes());
        buf[32..36].copy_from_slice(&self.flags.to_le_bytes());
        // [36..40] = checksum, zero during CRC.
        let crc = checksum::crc32c(&buf);
        buf[36..40].copy_from_slice(&crc.to_le_bytes());
        buf
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < COLUMN_DOMAIN_HEADER_LEN {
            return Err(QfError::BufferTooShort);
        }
        let bytes = &bytes[..COLUMN_DOMAIN_HEADER_LEN];
        let table_or_object_id = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let column_or_property_id = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let logical_type = u16::from_le_bytes(bytes[8..10].try_into().unwrap());
        let collation_id = u16::from_le_bytes(bytes[10..12].try_into().unwrap());
        let domain_count = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
        let sorted_file_codes_offset = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
        let file_code_to_rank_offset = u64::from_le_bytes(bytes[24..32].try_into().unwrap());
        let flags = u32::from_le_bytes(bytes[32..36].try_into().unwrap());
        let checksum_field = u32::from_le_bytes(bytes[36..40].try_into().unwrap());

        let mut for_crc = [0u8; COLUMN_DOMAIN_HEADER_LEN];
        for_crc.copy_from_slice(bytes);
        for_crc[36..40].fill(0);
        if checksum::crc32c(&for_crc) != checksum_field {
            return Err(QfError::ChecksumMismatch);
        }

        Ok(Self {
            table_or_object_id,
            column_or_property_id,
            logical_type,
            collation_id,
            domain_count,
            sorted_file_codes_offset,
            file_code_to_rank_offset,
            flags,
            checksum: checksum_field,
        })
    }
}

// ── ColumnDomain ─────────────────────────────────────────────────────────────

/// Parsed ColumnDomain section payload (Spec §23).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnDomain {
    pub header: ColumnDomainHeaderV1,
    /// `FileCode`s in ascending logical order under `collation_id`.
    pub sorted_file_codes: Vec<u32>,
    /// `file_code_to_rank[file_code]` = rank of that file_code, or
    /// [`INVALID_RANK`] if absent.
    pub file_code_to_rank: Vec<u32>,
}

impl ColumnDomain {
    /// Parse a ColumnDomain section payload (header + payload bytes).
    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        let header = ColumnDomainHeaderV1::parse(bytes)?;

        // Bounds-check sorted_file_codes region.
        let sfc_off =
            usize::try_from(header.sorted_file_codes_offset).map_err(|_| QfError::OffsetRange)?;
        let sfc_bytes = (header.domain_count as usize)
            .checked_mul(4)
            .ok_or(QfError::ArithOverflow)?;
        let sfc_end = sfc_off
            .checked_add(sfc_bytes)
            .ok_or(QfError::ArithOverflow)?;
        if sfc_end > bytes.len() {
            return Err(QfError::OffsetRange);
        }
        if sfc_off < COLUMN_DOMAIN_HEADER_LEN {
            return Err(QfError::BadDomain);
        }

        let mut sorted_file_codes = Vec::with_capacity(header.domain_count as usize);
        for i in 0..header.domain_count as usize {
            let off = sfc_off + i * 4;
            sorted_file_codes.push(u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()));
        }
        // Spec §23: sorted_file_codes MUST be sorted by logical value order.
        // We require strict ascending (no duplicates) since duplicates would
        // collapse two ranks onto one FileCode.
        for w in sorted_file_codes.windows(2) {
            if w[0] >= w[1] {
                return Err(QfError::BadDomain);
            }
        }

        // Bounds-check file_code_to_rank region. Length is derived from the
        // remaining section bytes after the rank-map offset, since the
        // dictionary entry count is not embedded in the section.
        let rank_off =
            usize::try_from(header.file_code_to_rank_offset).map_err(|_| QfError::OffsetRange)?;
        if rank_off < COLUMN_DOMAIN_HEADER_LEN || rank_off > bytes.len() {
            return Err(QfError::OffsetRange);
        }
        let rank_region = &bytes[rank_off..];
        if !rank_region.len().is_multiple_of(4) {
            return Err(QfError::BadDomain);
        }
        let rank_count = rank_region.len() / 4;
        let mut file_code_to_rank = Vec::with_capacity(rank_count);
        for i in 0..rank_count {
            let off = i * 4;
            file_code_to_rank.push(u32::from_le_bytes(
                rank_region[off..off + 4].try_into().unwrap(),
            ));
        }

        Ok(Self {
            header,
            sorted_file_codes,
            file_code_to_rank,
        })
    }

    /// Serialise to a section payload using the canonical layout
    /// `[header][sorted_file_codes][file_code_to_rank]`. The header's
    /// offsets and `domain_count` are recomputed; `checksum` is recomputed.
    pub fn serialize(&self) -> Result<Vec<u8>, QfError> {
        let domain_count =
            u32::try_from(self.sorted_file_codes.len()).map_err(|_| QfError::BadDomain)?;
        let sfc_off = COLUMN_DOMAIN_HEADER_LEN as u64;
        let sfc_bytes = (self.sorted_file_codes.len() * 4) as u64;
        let rank_off = sfc_off + sfc_bytes;

        let mut header = self.header.clone();
        header.domain_count = domain_count;
        header.sorted_file_codes_offset = sfc_off;
        header.file_code_to_rank_offset = rank_off;

        let mut out = Vec::new();
        out.extend_from_slice(&header.serialize());
        for code in &self.sorted_file_codes {
            out.extend_from_slice(&code.to_le_bytes());
        }
        for rank in &self.file_code_to_rank {
            out.extend_from_slice(&rank.to_le_bytes());
        }
        Ok(out)
    }

    /// Look up the rank of a FileCode through the dense rank map.
    /// Returns `None` if the FileCode is out of range for the rank map or
    /// maps to [`INVALID_RANK`].
    pub fn rank_of(&self, file_code: u32) -> Option<u32> {
        let r = *self.file_code_to_rank.get(file_code as usize)?;
        if r == INVALID_RANK {
            None
        } else {
            Some(r)
        }
    }

    /// Spec §23 safety check: every rank in `file_code_to_rank` is either
    /// [`INVALID_RANK`] or a valid index into `sorted_file_codes`, and the
    /// declared rank for each FileCode actually points back to that
    /// FileCode in `sorted_file_codes`. Without this guarantee, range
    /// pushdown using domain min/max would be unsafe (Spec §23 Rules).
    pub fn is_safe(&self) -> bool {
        let n = self.sorted_file_codes.len() as u32;
        for (file_code, &rank) in self.file_code_to_rank.iter().enumerate() {
            if rank == INVALID_RANK {
                continue;
            }
            if rank >= n {
                return false;
            }
            if self.sorted_file_codes[rank as usize] != file_code as u32 {
                return false;
            }
        }
        true
    }

    /// Validate `is_safe` and return [`QfError::BadDomain`] otherwise.
    pub fn validate(&self) -> Result<(), QfError> {
        if self.is_safe() {
            Ok(())
        } else {
            Err(QfError::BadDomain)
        }
    }

    /// Build a ColumnDomain from a list of FileCodes that are present in
    /// the column, in ascending logical order. Builds the inverse rank map
    /// over a dense FileCode space of `dictionary_entry_count` entries.
    pub fn from_sorted_present_codes(
        sorted_codes: &[u32],
        dictionary_entry_count: u32,
        table_or_object_id: u32,
        column_or_property_id: u32,
        logical_type: u16,
        collation_id: u16,
        flags: u32,
    ) -> Result<Self, QfError> {
        for w in sorted_codes.windows(2) {
            if w[0] >= w[1] {
                return Err(QfError::BadDomain);
            }
        }
        if let Some(&max) = sorted_codes.iter().max() {
            if max >= dictionary_entry_count {
                return Err(QfError::BadFileCode);
            }
        }
        let mut file_code_to_rank = vec![INVALID_RANK; dictionary_entry_count as usize];
        for (rank, &code) in sorted_codes.iter().enumerate() {
            file_code_to_rank[code as usize] = rank as u32;
        }
        Ok(Self {
            header: ColumnDomainHeaderV1 {
                table_or_object_id,
                column_or_property_id,
                logical_type,
                collation_id,
                domain_count: sorted_codes.len() as u32,
                sorted_file_codes_offset: COLUMN_DOMAIN_HEADER_LEN as u64,
                file_code_to_rank_offset: (COLUMN_DOMAIN_HEADER_LEN + sorted_codes.len() * 4)
                    as u64,
                flags,
                checksum: 0,
            },
            sorted_file_codes: sorted_codes.to_vec(),
            file_code_to_rank,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ColumnDomain {
        // Dictionary has 5 entries (0..=4). Column uses codes {1, 3, 4} in
        // ascending logical order; codes 0 and 2 are absent.
        ColumnDomain::from_sorted_present_codes(&[1, 3, 4], 5, 7, 11, 0x0010, 0, 0).unwrap()
    }

    #[test]
    fn header_roundtrip_and_checksum() {
        let h = ColumnDomainHeaderV1 {
            table_or_object_id: 1,
            column_or_property_id: 2,
            logical_type: 3,
            collation_id: 4,
            domain_count: 5,
            sorted_file_codes_offset: 40,
            file_code_to_rank_offset: 80,
            flags: 6,
            checksum: 0,
        };
        let bytes = h.serialize();
        let h2 = ColumnDomainHeaderV1::parse(&bytes).unwrap();
        assert_eq!(h2.table_or_object_id, 1);
        assert_eq!(h2.column_or_property_id, 2);
        assert_eq!(h2.logical_type, 3);
        assert_eq!(h2.collation_id, 4);
        assert_eq!(h2.domain_count, 5);
        assert_eq!(h2.sorted_file_codes_offset, 40);
        assert_eq!(h2.file_code_to_rank_offset, 80);
        assert_eq!(h2.flags, 6);
        assert_ne!(h2.checksum, 0);
    }

    #[test]
    fn header_rejects_flipped_checksum() {
        let h = ColumnDomainHeaderV1 {
            table_or_object_id: 0,
            column_or_property_id: 0,
            logical_type: 0,
            collation_id: 0,
            domain_count: 0,
            sorted_file_codes_offset: 40,
            file_code_to_rank_offset: 40,
            flags: 0,
            checksum: 0,
        };
        let mut bytes = h.serialize();
        bytes[36] ^= 0xFF;
        assert_eq!(
            ColumnDomainHeaderV1::parse(&bytes),
            Err(QfError::ChecksumMismatch)
        );
    }

    #[test]
    fn domain_roundtrip_and_rank_lookup() {
        let d = sample();
        let bytes = d.serialize().unwrap();
        let d2 = ColumnDomain::parse(&bytes).unwrap();
        assert_eq!(d2.sorted_file_codes, vec![1u32, 3, 4]);
        assert_eq!(d2.rank_of(1), Some(0));
        assert_eq!(d2.rank_of(3), Some(1));
        assert_eq!(d2.rank_of(4), Some(2));
        assert_eq!(d2.rank_of(0), None);
        assert_eq!(d2.rank_of(2), None);
        assert!(d2.is_safe());
    }

    #[test]
    fn rejects_unsorted_sorted_file_codes() {
        let mut d = sample();
        d.sorted_file_codes = vec![3, 1, 4];
        let bytes = d.serialize().unwrap();
        assert_eq!(ColumnDomain::parse(&bytes), Err(QfError::BadDomain));
    }

    #[test]
    fn rejects_duplicate_sorted_file_codes() {
        let mut d = sample();
        d.sorted_file_codes = vec![1, 1, 4];
        let bytes = d.serialize().unwrap();
        assert_eq!(ColumnDomain::parse(&bytes), Err(QfError::BadDomain));
    }

    #[test]
    fn detects_unsafe_rank_map() {
        let mut d = sample();
        // Point rank 0 (FileCode 1) at rank index 99 — unsafe.
        d.file_code_to_rank[1] = 99;
        assert!(!d.is_safe());
        assert_eq!(d.validate(), Err(QfError::BadDomain));
    }

    #[test]
    fn detects_inconsistent_inverse_map() {
        let mut d = sample();
        // FileCode 1 claims rank 0 in `file_code_to_rank`, but
        // sorted_file_codes[0] != 1 once we tamper.
        d.sorted_file_codes[0] = 2;
        assert!(!d.is_safe());
    }

    #[test]
    fn from_sorted_rejects_out_of_range_codes() {
        let err =
            ColumnDomain::from_sorted_present_codes(&[1, 3, 99], 5, 0, 0, 0, 0, 0).unwrap_err();
        assert_eq!(err, QfError::BadFileCode);
    }

    #[test]
    fn parse_rejects_offset_inside_header() {
        let mut d = sample();
        d.header.sorted_file_codes_offset = 10; // inside the 40-byte header
                                                // serialize() recomputes offsets — manually corrupt instead.
        let mut bytes = d.serialize().unwrap();
        bytes[16..24].copy_from_slice(&10u64.to_le_bytes());
        // Rebuild checksum.
        let mut hdr = bytes[..COLUMN_DOMAIN_HEADER_LEN].to_vec();
        hdr[36..40].fill(0);
        let crc = checksum::crc32c(&hdr);
        bytes[36..40].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(ColumnDomain::parse(&bytes), Err(QfError::BadDomain));
    }
}
