//! Quay Format (QF) v1.0 — Section decompression layer.

use std::borrow::Cow;

use crate::{footer::QfSectionEntryV1, QfError};

/// Returns the decompressed payload bytes for a section.
///
/// - [`CompressionCodec::None`]: validates `length == uncompressed_length`, returns a borrowed slice.
/// - [`CompressionCodec::Lz4`]: not yet supported; returns [`QfError::UnsupportedEncoding`].
/// - [`CompressionCodec::Zstd`]: not yet supported; returns [`QfError::UnsupportedEncoding`].
/// - Unknown codec: returns [`QfError::BadSection`].
pub fn section_payload<'a>(
    file_data: &'a [u8],
    entry: &QfSectionEntryV1,
) -> Result<Cow<'a, [u8]>, QfError> {
    match entry.compression {
        0 => {
            // CompressionCodec::None
            let end = entry
                .offset
                .checked_add(entry.length)
                .ok_or(QfError::ArithOverflow)?;
            if end as usize > file_data.len() {
                return Err(QfError::OffsetRange);
            }
            if entry.uncompressed_length != entry.length {
                return Err(QfError::BadSection(
                    "uncompressed_length mismatch for uncompressed section".into(),
                ));
            }
            Ok(Cow::Borrowed(
                &file_data[entry.offset as usize..end as usize],
            ))
        }
        1 => Err(QfError::UnsupportedEncoding(
            "LZ4 decompression is not enabled in this build".into(),
        )),
        2 => Err(QfError::UnsupportedEncoding(
            "Zstd decompression is not enabled in this build".into(),
        )),
        other => Err(QfError::BadSection(format!(
            "unknown compression codec {}",
            other
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::footer::QfSectionEntryV1;

    fn make_entry(
        offset: u64,
        length: u64,
        uncompressed_length: u64,
        compression: u8,
    ) -> QfSectionEntryV1 {
        QfSectionEntryV1 {
            section_id: 1,
            section_kind: 1,
            profile: 0,
            flags: 0,
            offset,
            length,
            uncompressed_length,
            item_count: 0,
            row_count: 0,
            compression,
            encryption: 0,
            alignment_log2: 0,
            reserved0: 0,
            required_features: 0,
            optional_features: 0,
            crc32c: 0,
            reserved1: 0,
        }
    }

    #[test]
    fn none_compression_returns_borrowed_slice() {
        let data = b"hello world";
        let entry = make_entry(0, 5, 5, 0);
        let result = section_payload(data, &entry).unwrap();
        assert_eq!(&*result, b"hello");
    }

    #[test]
    fn uncompressed_length_mismatch_rejected() {
        let data = b"hello world";
        let entry = make_entry(0, 5, 6, 0); // length=5, uncompressed_length=6 mismatch
        assert!(matches!(
            section_payload(data, &entry),
            Err(QfError::BadSection(_))
        ));
    }

    #[test]
    fn lz4_returns_unsupported() {
        let data = b"hello";
        let entry = make_entry(0, 5, 10, 1);
        assert!(matches!(
            section_payload(data, &entry),
            Err(QfError::UnsupportedEncoding(_))
        ));
    }

    #[test]
    fn out_of_bounds_section_rejected() {
        let data = b"hi";
        let entry = make_entry(0, 10, 10, 0); // length=10, data only has 2 bytes
        assert_eq!(section_payload(data, &entry), Err(QfError::OffsetRange));
    }
}
