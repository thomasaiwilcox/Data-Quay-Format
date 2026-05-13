//! Cove Format (COVE) v2.0 — Section decompression layer.
//!
//! Implements Spec §66 codec dispatch: section payloads MAY be compressed
//! with `None`, `LZ4`, or `Zstd`. The codec is feature-gated so that small
//! reference builds can opt out of decompression, but the default build
//! ships every v1 codec exactly as the spec requires.

use std::borrow::Cow;

use crate::{
    checksum,
    constants::CompressionCodec,
    footer::CoveSectionEntryV1,
    page::{page_flag_codec, ColumnPageIndexEntryV1},
    postscript::CoveSectionSpecV1,
    retained_bytes::RetainedBytes,
    CoveError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageChecksumValidation {
    Verify,
    Trusted,
}

/// Returns the on-disk payload bytes for a writer-side section payload.
///
/// This mirrors the read-side codec dispatch so section writers can produce the
/// exact raw bytes that readers later validate and decompress.
pub fn encode_payload_for_codec(payload: &[u8], compression: u8) -> Result<Vec<u8>, CoveError> {
    let codec = CompressionCodec::from_u8(compression)
        .ok_or_else(|| CoveError::BadSection(format!("unknown compression codec {compression}")))?;
    match codec {
        CompressionCodec::None => Ok(payload.to_vec()),
        CompressionCodec::Lz4 => lz4_compress(payload),
        CompressionCodec::Zstd => zstd_compress(payload),
    }
}

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

/// Returns the decompressed payload bytes for an already-isolated section.
///
/// INVARIANT: callers that use range I/O must validate the section's absolute
/// offset and bounds before fetching `raw`; this helper validates the fetched
/// byte count, checksum, codec, and uncompressed length.
pub fn section_payload_from_raw<'a>(
    raw: &'a [u8],
    length: u64,
    uncompressed_length: u64,
    compression: u8,
    crc32c: u32,
) -> Result<Cow<'a, [u8]>, CoveError> {
    if raw.len() as u64 != length {
        return Err(CoveError::BadSection(format!(
            "section raw length {} does not match declared length {}",
            raw.len(),
            length
        )));
    }
    if checksum::crc32c(raw) != crc32c {
        return Err(CoveError::ChecksumMismatch);
    }
    payload_from_raw(raw, length, uncompressed_length, compression)
}

fn payload_from_spec<'a>(
    file_data: &'a [u8],
    offset: u64,
    length: u64,
    uncompressed_length: u64,
    compression: u8,
) -> Result<Cow<'a, [u8]>, CoveError> {
    let raw = payload_raw_bytes(file_data, offset, length)?;
    payload_from_raw(raw, length, uncompressed_length, compression)
}

fn payload_from_raw<'a>(
    raw: &'a [u8],
    length: u64,
    uncompressed_length: u64,
    compression: u8,
) -> Result<Cow<'a, [u8]>, CoveError> {
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

/// Returns the on-disk wire bytes for a column page payload (Spec §27.3 /
/// §66). Mirrors [`encode_payload_for_codec`] but is named for callers that
/// are working at the page level rather than the section level.
pub fn encode_page_payload(payload: &[u8], codec: CompressionCodec) -> Result<Vec<u8>, CoveError> {
    encode_payload_for_codec(payload, codec as u8)
}

/// Returns the decompressed payload bytes for a column page (Spec §27.3 /
/// §66). The caller passes the page-relative payload slice (already isolated
/// out of the section) so that this routine handles only codec dispatch and
/// length validation against `entry.uncompressed_length`.
pub fn column_page_payload<'a>(
    page_bytes: &'a [u8],
    entry: &ColumnPageIndexEntryV1,
) -> Result<Cow<'a, [u8]>, CoveError> {
    column_page_payload_with_checksum_validation(page_bytes, entry, PageChecksumValidation::Verify)
}

/// Returns the decompressed payload bytes for a column page with caller-selected
/// page-wire checksum handling.
///
/// INVARIANT: `Trusted` skips only the page-wire CRC check. Length, codec, and
/// uncompressed-length framing remain validated before decoded bytes are
/// returned.
pub fn column_page_payload_with_checksum_validation<'a>(
    page_bytes: &'a [u8],
    entry: &ColumnPageIndexEntryV1,
    checksum_validation: PageChecksumValidation,
) -> Result<Cow<'a, [u8]>, CoveError> {
    if page_bytes.len() as u64 != entry.page_length {
        return Err(CoveError::BadSection(format!(
            "page payload length {} does not match page_length {}",
            page_bytes.len(),
            entry.page_length
        )));
    }
    if checksum_validation == PageChecksumValidation::Verify
        && checksum::crc32c(page_bytes) != entry.checksum
    {
        return Err(CoveError::ChecksumMismatch);
    }
    let codec = page_flag_codec(entry.flags)?;
    match codec {
        CompressionCodec::None => {
            // §13.2 mirror: parse() already enforces equal lengths, but
            // re-verify defensively to keep this routine total.
            if entry.uncompressed_length != entry.page_length {
                return Err(CoveError::BadSection(
                    "uncompressed_length must equal page_length when page codec=None".into(),
                ));
            }
            Ok(Cow::Borrowed(page_bytes))
        }
        CompressionCodec::Lz4 => {
            lz4_decompress(page_bytes, entry.uncompressed_length).map(Cow::Owned)
        }
        CompressionCodec::Zstd => {
            zstd_decompress(page_bytes, entry.uncompressed_length).map(Cow::Owned)
        }
    }
}

/// Returns owned decompressed payload bytes for a column page.
///
/// This is scan-path plumbing for range readers that already own the page wire
/// buffer. For uncompressed pages it validates the same framing as
/// [`column_page_payload_with_checksum_validation`] and then returns the input
/// buffer without an additional copy.
pub fn column_page_payload_vec_with_checksum_validation(
    page_bytes: Vec<u8>,
    entry: &ColumnPageIndexEntryV1,
    checksum_validation: PageChecksumValidation,
) -> Result<Vec<u8>, CoveError> {
    Ok(column_page_payload_retained_with_checksum_validation(
        RetainedBytes::from_vec(page_bytes),
        entry,
        checksum_validation,
    )?
    .to_vec())
}

/// Returns retained decompressed payload bytes for a column page.
///
/// INVARIANT: for uncompressed pages, the returned value is the caller-owned
/// page slice without copying. For compressed pages, the returned owner is the
/// single decompressed allocation.
pub fn column_page_payload_retained_with_checksum_validation(
    page_bytes: RetainedBytes,
    entry: &ColumnPageIndexEntryV1,
    checksum_validation: PageChecksumValidation,
) -> Result<RetainedBytes, CoveError> {
    if page_bytes.len() as u64 != entry.page_length {
        return Err(CoveError::BadSection(format!(
            "page payload length {} does not match page_length {}",
            page_bytes.len(),
            entry.page_length
        )));
    }
    if checksum_validation == PageChecksumValidation::Verify
        && checksum::crc32c(page_bytes.as_slice()) != entry.checksum
    {
        return Err(CoveError::ChecksumMismatch);
    }
    let codec = page_flag_codec(entry.flags)?;
    match codec {
        CompressionCodec::None => {
            if entry.uncompressed_length != entry.page_length {
                return Err(CoveError::BadSection(
                    "uncompressed_length must equal page_length when page codec=None".into(),
                ));
            }
            Ok(page_bytes)
        }
        CompressionCodec::Lz4 => lz4_decompress(page_bytes.as_slice(), entry.uncompressed_length)
            .map(RetainedBytes::from_vec),
        CompressionCodec::Zstd => zstd_decompress(page_bytes.as_slice(), entry.uncompressed_length)
            .map(RetainedBytes::from_vec),
    }
}

#[cfg(feature = "compression-lz4")]
fn lz4_decompress(raw: &[u8], expected_len: u64) -> Result<Vec<u8>, CoveError> {
    let expected = usize::try_from(expected_len).map_err(|_| CoveError::ArithOverflow)?;
    lz4_flex::block::decompress(raw, expected)
        .map_err(|e| CoveError::BadSection(format!("LZ4 decompression failed: {e}")))
}

#[cfg(feature = "compression-lz4")]
fn lz4_compress(payload: &[u8]) -> Result<Vec<u8>, CoveError> {
    Ok(lz4_flex::block::compress(payload))
}

#[cfg(not(feature = "compression-lz4"))]
fn lz4_decompress(_raw: &[u8], _expected_len: u64) -> Result<Vec<u8>, CoveError> {
    Err(CoveError::UnsupportedEncoding(
        "LZ4 decompression is not enabled in this build (enable feature `compression-lz4`)".into(),
    ))
}

#[cfg(not(feature = "compression-lz4"))]
fn lz4_compress(_payload: &[u8]) -> Result<Vec<u8>, CoveError> {
    Err(CoveError::UnsupportedEncoding(
        "LZ4 compression is not enabled in this build (enable feature `compression-lz4`)".into(),
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

#[cfg(feature = "compression-zstd")]
fn zstd_compress(payload: &[u8]) -> Result<Vec<u8>, CoveError> {
    zstd::stream::encode_all(std::io::Cursor::new(payload), 0)
        .map_err(|e| CoveError::BadSection(format!("Zstd compression failed: {e}")))
}

#[cfg(not(feature = "compression-zstd"))]
fn zstd_decompress(_raw: &[u8], _expected_len: u64) -> Result<Vec<u8>, CoveError> {
    Err(CoveError::UnsupportedEncoding(
        "Zstd decompression is not enabled in this build (enable feature `compression-zstd`)"
            .into(),
    ))
}

#[cfg(not(feature = "compression-zstd"))]
fn zstd_compress(_payload: &[u8]) -> Result<Vec<u8>, CoveError> {
    Err(CoveError::UnsupportedEncoding(
        "Zstd compression is not enabled in this build (enable feature `compression-zstd`)".into(),
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

    #[cfg(feature = "compression-lz4")]
    #[test]
    fn lz4_writer_payload_round_trip() {
        let payload = b"Cove compression round-trip payload";
        let compressed = encode_payload_for_codec(payload, CompressionCodec::Lz4 as u8).unwrap();
        let mut file = vec![0u8; 8];
        let offset = file.len() as u64;
        file.extend_from_slice(&compressed);
        let entry = make_entry(offset, compressed.len() as u64, payload.len() as u64, 1);
        let decoded = section_payload(&file, &entry).unwrap();
        assert_eq!(&*decoded, payload);
    }

    #[cfg(not(feature = "compression-lz4"))]
    #[test]
    fn disabled_lz4_returns_unsupported_encoding() {
        let payload = b"not decoded without lz4 support";
        let entry = make_entry(0, payload.len() as u64, 128, CompressionCodec::Lz4 as u8);
        assert!(matches!(
            section_payload(payload, &entry),
            Err(CoveError::UnsupportedEncoding(_))
        ));
        assert!(matches!(
            encode_payload_for_codec(payload, CompressionCodec::Lz4 as u8),
            Err(CoveError::UnsupportedEncoding(_))
        ));
    }

    #[cfg(feature = "compression-zstd")]
    #[test]
    fn zstd_writer_payload_round_trip() {
        let payload = b"Cove zstd compression round-trip payload";
        let compressed = encode_payload_for_codec(payload, CompressionCodec::Zstd as u8).unwrap();
        let mut file = vec![0u8; 8];
        let offset = file.len() as u64;
        file.extend_from_slice(&compressed);
        let entry = make_entry(offset, compressed.len() as u64, payload.len() as u64, 2);
        let decoded = section_payload(&file, &entry).unwrap();
        assert_eq!(&*decoded, payload);
    }

    #[cfg(not(feature = "compression-zstd"))]
    #[test]
    fn disabled_zstd_returns_unsupported_encoding() {
        let payload = b"not decoded without zstd support";
        let entry = make_entry(0, payload.len() as u64, 128, CompressionCodec::Zstd as u8);
        assert!(matches!(
            section_payload(payload, &entry),
            Err(CoveError::UnsupportedEncoding(_))
        ));
        assert!(matches!(
            encode_payload_for_codec(payload, CompressionCodec::Zstd as u8),
            Err(CoveError::UnsupportedEncoding(_))
        ));
    }

    fn page_entry(
        page_length: u64,
        uncompressed_length: u64,
        codec: CompressionCodec,
        checksum: u32,
    ) -> ColumnPageIndexEntryV1 {
        ColumnPageIndexEntryV1 {
            column_id: 1,
            morsel_id: 0,
            row_count: 1,
            non_null_count: 1,
            null_count: 0,
            encoding_root: 0,
            page_offset: 0,
            page_length,
            uncompressed_length,
            stats_ref: 0,
            flags: codec as u32,
            checksum,
        }
    }

    #[test]
    fn page_payload_none_returns_borrowed_slice() {
        let payload = b"raw page bytes";
        let entry = page_entry(
            payload.len() as u64,
            payload.len() as u64,
            CompressionCodec::None,
            checksum::crc32c(payload),
        );
        let decoded = column_page_payload(payload, &entry).unwrap();
        assert_eq!(&*decoded, payload);
    }

    #[test]
    fn retained_page_payload_none_returns_input_owner_slice() {
        let payload = b"raw page bytes";
        let mut owner = b"prefix".to_vec();
        let offset = owner.len();
        owner.extend_from_slice(payload);
        owner.extend_from_slice(b"suffix");
        let retained =
            RetainedBytes::from_arc_slice(std::sync::Arc::new(owner), offset, payload.len())
                .unwrap();
        let entry = page_entry(
            payload.len() as u64,
            payload.len() as u64,
            CompressionCodec::None,
            checksum::crc32c(payload),
        );

        let decoded = column_page_payload_retained_with_checksum_validation(
            retained.clone(),
            &entry,
            PageChecksumValidation::Verify,
        )
        .unwrap();
        assert!(decoded.shares_owner(&retained));
        assert_eq!(decoded.owner_offset(), retained.owner_offset());
        assert_eq!(decoded.as_slice(), payload);
    }

    #[test]
    fn page_payload_length_mismatch_rejected() {
        let payload = b"raw page bytes";
        let entry = page_entry(
            payload.len() as u64 + 1,
            payload.len() as u64 + 1,
            CompressionCodec::None,
            checksum::crc32c(payload),
        );
        assert!(matches!(
            column_page_payload(payload, &entry),
            Err(CoveError::BadSection(_))
        ));
    }

    #[cfg(feature = "compression-lz4")]
    #[test]
    fn page_payload_lz4_round_trip() {
        let payload =
            b"Cove page-level LZ4 round trip payload Cove page-level LZ4 round trip payload";
        let wire = encode_page_payload(payload, CompressionCodec::Lz4).unwrap();
        let entry = page_entry(
            wire.len() as u64,
            payload.len() as u64,
            CompressionCodec::Lz4,
            checksum::crc32c(&wire),
        );
        let decoded = column_page_payload(&wire, &entry).unwrap();
        assert_eq!(&*decoded, payload);
    }

    #[cfg(feature = "compression-zstd")]
    #[test]
    fn page_payload_zstd_round_trip() {
        let payload = b"Cove page-level Zstd round trip payload";
        let wire = encode_page_payload(payload, CompressionCodec::Zstd).unwrap();
        let entry = page_entry(
            wire.len() as u64,
            payload.len() as u64,
            CompressionCodec::Zstd,
            checksum::crc32c(&wire),
        );
        let decoded = column_page_payload(&wire, &entry).unwrap();
        assert_eq!(&*decoded, payload);
    }

    #[cfg(feature = "compression-lz4")]
    #[test]
    fn page_payload_lz4_truncated_rejected() {
        let payload = b"Cove page-level LZ4 corruption sentinel sentinel sentinel";
        let mut wire = encode_page_payload(payload, CompressionCodec::Lz4).unwrap();
        // Truncate the compressed wire bytes; the entry still claims the full
        // uncompressed length so decompression must fail rather than silently
        // produce a short payload (Spec §66 robustness requirement).
        wire.truncate(wire.len().saturating_sub(2));
        let entry = page_entry(
            wire.len() as u64,
            payload.len() as u64,
            CompressionCodec::Lz4,
            checksum::crc32c(&wire),
        );
        assert!(column_page_payload(&wire, &entry).is_err());
    }

    #[test]
    fn page_payload_checksum_mismatch_rejected() {
        let payload = b"raw page bytes";
        let entry = page_entry(
            payload.len() as u64,
            payload.len() as u64,
            CompressionCodec::None,
            checksum::crc32c(b"different"),
        );
        assert_eq!(
            column_page_payload(payload, &entry),
            Err(CoveError::ChecksumMismatch)
        );
    }
}
