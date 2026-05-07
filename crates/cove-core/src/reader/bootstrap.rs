use crate::{
    checksum, compression,
    constants::MAGIC_COVE,
    footer::CoveFooter,
    header::{CoveHeaderV1, HEADER_SIZE},
    postscript::{CovePostscriptV1, POSTSCRIPT_TOTAL_SIZE},
    CoveError,
};

use super::{IgnoredOptionalSection, OptionalPushdownPolicy, ValidatedCoveFile};

pub(super) fn validate_bytes_with_optional_pushdown_policy(
    data: &[u8],
    optional_pushdown_policy: OptionalPushdownPolicy,
) -> Result<(ValidatedCoveFile, Vec<IgnoredOptionalSection>), CoveError> {
    if data.len() < HEADER_SIZE + POSTSCRIPT_TOTAL_SIZE {
        return Err(CoveError::BufferTooShort);
    }

    let trailing_magic: [u8; 4] = data[data.len() - 4..]
        .try_into()
        .map_err(|_| CoveError::BufferTooShort)?;
    if trailing_magic != MAGIC_COVE {
        return Err(CoveError::BadMagic);
    }

    let postscript = CovePostscriptV1::parse_from_tail(data)?;
    if postscript.file_len != data.len() as u64 {
        return Err(CoveError::OffsetRange);
    }

    let footer_end = postscript.footer.end_offset()?;
    let tail_start = data
        .len()
        .checked_sub(POSTSCRIPT_TOTAL_SIZE)
        .ok_or(CoveError::BufferTooShort)? as u64;
    if postscript.footer.offset < HEADER_SIZE as u64 || footer_end > tail_start {
        return Err(CoveError::OffsetRange);
    }

    let footer_start = postscript.footer.offset as usize;
    let footer_bytes = &data[footer_start..footer_end as usize];
    if checksum::crc32c(footer_bytes) != postscript.footer.crc32c {
        return Err(CoveError::ChecksumMismatch);
    }
    super::validate_footer_codec_feature_advertisement(&postscript)?;
    let footer_payload = compression::section_spec_payload(data, &postscript.footer)?;
    let mut footer = CoveFooter::parse(&footer_payload)?;
    if footer.header.total_len()? != postscript.footer.uncompressed_length {
        return Err(CoveError::BadSection(
            "footer header length does not match postscript footer uncompressed_length".to_string(),
        ));
    }

    let header = CoveHeaderV1::parse(data)?;
    let ignored_optional_sections = super::validate_sections(
        data,
        footer_start,
        &mut footer,
        &header,
        optional_pushdown_policy,
    )?;
    super::validate_required_feature_implementation(&header)?;
    super::validate_primary_profile_features(&header)?;
    if header.required_features != postscript.required_features
        || header.optional_features != postscript.optional_features
    {
        return Err(CoveError::BadSection(
            "header and postscript feature bits differ".to_string(),
        ));
    }

    let validated = ValidatedCoveFile {
        header,
        postscript,
        footer,
    };
    Ok((validated, ignored_optional_sections))
}
