//! Cove Format (COVE) v1.0 — Section decompression layer.
//!
//! Implements Spec §66 codec dispatch: section payloads MAY be compressed
//! with `None`, `LZ4`, or `Zstd`. The codec is feature-gated so that small
//! reference builds can opt out of decompression, but the default build
//! ships every v1 codec exactly as the spec requires.

use std::borrow::Cow;

use crate::{
    constants::CompressionCodec, footer::CoveSectionEntryV1, postscript::CoveSectionSpecV1,
    CoveError,
};

/// Returns the decompressed payload bytes for a section.
///
/// Behavior per [`CompressionCodec`] (Spec §66):
///
/// * [`CompressionCodec::None`] — validates `length == uncompressed_length`
///   (Spec §13.2) and returns a borrowed slice over the file bytes.
/// * [`CompressionCodec::Lz4`] — decompresses with the `lz4_flex` block format
///   when the `compression-lz4` feature is enabled.
/// * [`CompressionCodec::Zstd`] — decompresses with the pure-Rust `ruzstd`
///   decoder when the `compression-zstd` feature is enabled.
///
/// Unknown codec values are reported as [`CoveError::BadSection`].
pub fn section_payload<'a>(
    file_data: &'a [u8],
    entry: &CoveSectionEntryV1,
) -> Result<Cow<'a, [u8]>, CoveError> {
    payload_from_spec(
        file_data,
        entry.offset,
        entry.length,
        entry.uncompressed_length,
        entry.compression,
    )
}

/// Returns the decompressed payload bytes for the footer/postscript section
/// spec. This shares the same codec rules as ordinary footer directory
/// sections.
pub fn section_spec_payload<'a>(
    file_data: &'a [u8],
    spec: &CoveSectionSpecV1,
) -> Result<Cow<'a, [u8]>, CoveError> {
    payload_from_spec(
        file_data,
        spec.offset,
        spec.length,
        spec.uncompressed_length,
        spec.compression,
    )
}

fn payload_from_spec<'a>(
    file_data: &'a [u8],
    offset: u64,
    length: u64,
    uncompressed_length: u64,
    compression: u8,
) -> Result<Cow<'a, [u8]>, CoveError> {
    let raw = payload_raw_bytes(file_data, offset, length)?;
    let codec = CompressionCodec::from_u8(compression)
        .ok_or_else(|| CoveError::BadSection(format!("unknown compression codec {compression}")))?;
    match codec {
        CompressionCodec::None => {
            if uncompressed_length != length {
                return Err(CoveError::BadSection(
                    "uncompressed_length must equal length when codec=None".into(),
                ));
            }
            Ok(Cow::Borrowed(raw))
        }
        CompressionCodec::Lz4 => lz4_decompress(raw, uncompressed_length).map(Cow::Owned),
        CompressionCodec::Zstd => zstd_decompress(raw, uncompressed_length).map(Cow::Owned),
    }
}

fn payload_raw_bytes<'a>(
    file_data: &'a [u8],
    offset: u64,
    length: u64,
) -> Result<&'a [u8], CoveError> {
    let end = offset.checked_add(length).ok_or(CoveError::ArithOverflow)?;
    if end as usize > file_data.len() {
        return Err(CoveError::OffsetRange);
    }
    Ok(&file_data[offset as usize..end as usize])
}

#[cfg(feature = "compression-lz4")]
fn lz4_decompress(raw: &[u8], expected_len: u64) -> Result<Vec<u8>, CoveError> {
    let expected = usize::try_from(expected_len).map_err(|_| CoveError::ArithOverflow)?;
    lz4_flex::block::decompress(raw, expected)
        .map_err(|e| CoveError::BadSection(format!("LZ4 decompression failed: {e}")))
}

#[cfg(not(feature = "compression-lz4"))]
fn lz4_decompress(_raw: &[u8], _expected_len: u64) -> Result<Vec<u8>, CoveError> {
    Err(CoveError::UnsupportedEncoding(
        "LZ4 decompression is not enabled in this build (enable feature `compression-lz4`)".into(),
    ))
}

#[cfg(feature = "compression-zstd")]
fn zstd_decompress(raw: &[u8], expected_len: u64) -> Result<Vec<u8>, CoveError> {
    use std::io::Read;
    let expected = usize::try_from(expected_len).map_err(|_| CoveError::ArithOverflow)?;
    let mut decoder = ruzstd::StreamingDecoder::new(raw)
        .map_err(|e| CoveError::BadSection(format!("Zstd decoder init failed: {e}")))?;
    let mut out = Vec::with_capacity(expected);
    decoder
        .read_to_end(&mut out)
        .map_err(|e| CoveError::BadSection(format!("Zstd decompression failed: {e}")))?;
    if out.len() != expected {
        return Err(CoveError::BadSection(format!(
            "Zstd produced {} bytes but section declares uncompressed_length={}",
            out.len(),
            expected
        )));
    }
    Ok(out)
}

#[cfg(not(feature = "compression-zstd"))]
fn zstd_decompress(_raw: &[u8], _expected_len: u64) -> Result<Vec<u8>, CoveError> {
    Err(CoveError::UnsupportedEncoding(
        "Zstd decompression is not enabled in this build (enable feature `compression-zstd`)"
            .into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::footer::CoveSectionEntryV1;
    use crate::postscript::CoveSectionSpecV1;

    fn make_entry(
        offset: u64,
        length: u64,
        uncompressed_length: u64,
        compression: u8,
    ) -> CoveSectionEntryV1 {
        CoveSectionEntryV1 {
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

    fn make_spec(
        offset: u64,
        length: u64,
        uncompressed_length: u64,
        compression: u8,
    ) -> CoveSectionSpecV1 {
        CoveSectionSpecV1 {
            offset,
            length,
            uncompressed_length,
            compression,
            encryption: 0,
            alignment_log2: 0,
            flags: 0,
            crc32c: 0,
            reserved: 0,
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
    fn section_spec_none_returns_borrowed_slice() {
        let data = b"hello world";
        let spec = make_spec(6, 5, 5, 0);
        let result = section_spec_payload(data, &spec).unwrap();
        assert_eq!(&*result, b"world");
    }

    #[test]
    fn uncompressed_length_mismatch_rejected() {
        let data = b"hello world";
        let entry = make_entry(0, 5, 6, 0);
        assert!(matches!(
            section_payload(data, &entry),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn out_of_bounds_section_rejected() {
        let data = b"hi";
        let entry = make_entry(0, 10, 10, 0);
        assert_eq!(section_payload(data, &entry), Err(CoveError::OffsetRange));
    }

    #[cfg(feature = "compression-lz4")]
    #[test]
    fn lz4_round_trip_decompresses_payload() {
        let payload = b"Cove Format reference implementation showcase payload payload payload";
        let compressed = lz4_flex::block::compress(payload);
        let mut file = vec![0u8; 16];
        let offset = file.len() as u64;
        file.extend_from_slice(&compressed);
        let entry = make_entry(offset, compressed.len() as u64, payload.len() as u64, 1);
        let result = section_payload(&file, &entry).unwrap();
        assert_eq!(&*result, payload);
    }

    #[cfg(feature = "compression-lz4")]
    #[test]
    fn lz4_corrupt_payload_rejected() {
        let entry = make_entry(0, 4, 1024, 1);
        let bytes = [0x00u8, 0x00, 0x00, 0x00];
        assert!(matches!(
            section_payload(&bytes, &entry),
            Err(CoveError::BadSection(_))
        ));
    }
}
