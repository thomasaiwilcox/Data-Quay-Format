//! Spec §70 — standalone `.covemap` reusable mapping artifact.
//!
//! The artifact uses the same tail-discovery pattern as COVX/COVM, but its
//! postscript points to a header region that contains the fixed header, the
//! mapping-version string, and the section directory for MAP_* payloads.

use std::borrow::Cow;

use crate::{
    checksum, compression,
    constants::{
        CompressionCodec, SectionKind, FEATURE_CODEC_LZ4, FEATURE_CODEC_ZSTD, FEATURE_SEMANTIC_MAP,
        KNOWN_FEATURE_BITS_MASK, MAGIC_COVEMAP, POSTSCRIPT_VERSION_V1,
    },
    postscript::CoveSectionSpecV1,
    profile::cove_map::{parse_embedded_section, validate_embedded_sections, EmbeddedMapSection},
    CoveError,
};

/// Encoded length of [`CovemapHeaderV1`] in bytes.
pub const COVEMAP_HEADER_LEN: u16 = 98;

/// Encoded length of [`CovemapSectionEntryV1`] in bytes.
pub const COVEMAP_SECTION_ENTRY_LEN: u16 = 36;

/// Required `version_major` for `.covemap` v1.
pub const COVEMAP_VERSION_MAJOR_V1: u16 = 1;

/// Required `version_minor` for `.covemap` v1.
pub const COVEMAP_VERSION_MINOR_V1: u16 = 0;

/// Encoded length of [`CovemapPostscriptV1`] in bytes.
pub const COVEMAP_POSTSCRIPT_LEN: u16 = 44;

/// Postscript version field value for `.covemap` v1.
pub const COVEMAP_POSTSCRIPT_VERSION_V1: u16 = POSTSCRIPT_VERSION_V1;

/// Size of the fixed tail after the postscript payload.
pub const COVEMAP_POSTSCRIPT_TAIL_SIZE: usize = 2 + 2 + 4;

/// Spec §70.1 fixed `CovemapHeaderV1` prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CovemapHeaderV1 {
    pub magic: [u8; 4],
    pub header_len: u16,
    pub version_major: u16,
    pub version_minor: u16,
    pub flags: u32,
    pub mapping_id: [u8; 16],
    pub required_features: u64,
    pub optional_features: u64,
    pub section_count: u32,
    pub mapping_version_len: u16,
    pub reserved0: u16,
    pub created_at_us: i64,
    pub reserved: [u8; 32],
    pub checksum: u32,
}

impl CovemapHeaderV1 {
    pub fn new(mapping_id: [u8; 16], created_at_us: i64) -> Self {
        Self {
            magic: MAGIC_COVEMAP,
            header_len: COVEMAP_HEADER_LEN,
            version_major: COVEMAP_VERSION_MAJOR_V1,
            version_minor: COVEMAP_VERSION_MINOR_V1,
            flags: 0,
            mapping_id,
            required_features: FEATURE_SEMANTIC_MAP,
            optional_features: 0,
            section_count: 0,
            mapping_version_len: 0,
            reserved0: 0,
            created_at_us,
            reserved: [0u8; 32],
            checksum: 0,
        }
    }

    pub fn serialize(&self) -> [u8; COVEMAP_HEADER_LEN as usize] {
        let mut buf = [0u8; COVEMAP_HEADER_LEN as usize];
        buf[0..4].copy_from_slice(&self.magic);
        buf[4..6].copy_from_slice(&self.header_len.to_le_bytes());
        buf[6..8].copy_from_slice(&self.version_major.to_le_bytes());
        buf[8..10].copy_from_slice(&self.version_minor.to_le_bytes());
        buf[10..14].copy_from_slice(&self.flags.to_le_bytes());
        buf[14..30].copy_from_slice(&self.mapping_id);
        buf[30..38].copy_from_slice(&self.required_features.to_le_bytes());
        buf[38..46].copy_from_slice(&self.optional_features.to_le_bytes());
        buf[46..50].copy_from_slice(&self.section_count.to_le_bytes());
        buf[50..52].copy_from_slice(&self.mapping_version_len.to_le_bytes());
        buf[52..54].copy_from_slice(&self.reserved0.to_le_bytes());
        buf[54..62].copy_from_slice(&self.created_at_us.to_le_bytes());
        buf[62..94].copy_from_slice(&self.reserved);
        let crc = checksum::crc32c(&buf[..94]);
        buf[94..98].copy_from_slice(&crc.to_le_bytes());
        buf
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < COVEMAP_HEADER_LEN as usize {
            return Err(CoveError::BufferTooShort);
        }
        let bytes = &bytes[..COVEMAP_HEADER_LEN as usize];

        let mut magic = [0u8; 4];
        magic.copy_from_slice(&bytes[0..4]);
        if magic != MAGIC_COVEMAP {
            return Err(CoveError::BadMagic);
        }

        let header_len = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
        if header_len != COVEMAP_HEADER_LEN {
            return Err(CoveError::BadSection(format!(
                "COVEMAP header_len must be {COVEMAP_HEADER_LEN}, got {header_len}"
            )));
        }

        let version_major = u16::from_le_bytes(bytes[6..8].try_into().unwrap());
        let version_minor = u16::from_le_bytes(bytes[8..10].try_into().unwrap());
        if version_major != COVEMAP_VERSION_MAJOR_V1 || version_minor != COVEMAP_VERSION_MINOR_V1 {
            return Err(CoveError::BadVersion);
        }

        let flags = u32::from_le_bytes(bytes[10..14].try_into().unwrap());
        let mut mapping_id = [0u8; 16];
        mapping_id.copy_from_slice(&bytes[14..30]);
        let required_features = u64::from_le_bytes(bytes[30..38].try_into().unwrap());
        let optional_features = u64::from_le_bytes(bytes[38..46].try_into().unwrap());
        let section_count = u32::from_le_bytes(bytes[46..50].try_into().unwrap());
        let mapping_version_len = u16::from_le_bytes(bytes[50..52].try_into().unwrap());
        let reserved0 = u16::from_le_bytes(bytes[52..54].try_into().unwrap());
        if reserved0 != 0 {
            return Err(CoveError::ReservedNotZero);
        }
        let created_at_us = i64::from_le_bytes(bytes[54..62].try_into().unwrap());
        let mut reserved = [0u8; 32];
        reserved.copy_from_slice(&bytes[62..94]);
        if reserved.iter().any(|byte| *byte != 0) {
            return Err(CoveError::ReservedNotZero);
        }
        let checksum_field = u32::from_le_bytes(bytes[94..98].try_into().unwrap());
        if checksum::crc32c(&bytes[..94]) != checksum_field {
            return Err(CoveError::ChecksumMismatch);
        }

        Ok(Self {
            magic,
            header_len,
            version_major,
            version_minor,
            flags,
            mapping_id,
            required_features,
            optional_features,
            section_count,
            mapping_version_len,
            reserved0,
            created_at_us,
            reserved,
            checksum: checksum_field,
        })
    }
}

/// Spec §70.1 `CovemapSectionEntryV1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CovemapSectionEntryV1 {
    pub section_id: u32,
    pub offset: u64,
    pub length: u64,
    pub uncompressed_length: u64,
    pub compression: u8,
    pub required: bool,
    pub reserved: u16,
    pub checksum: u32,
}

impl CovemapSectionEntryV1 {
    pub fn serialize(&self) -> [u8; COVEMAP_SECTION_ENTRY_LEN as usize] {
        let mut buf = [0u8; COVEMAP_SECTION_ENTRY_LEN as usize];
        buf[0..4].copy_from_slice(&self.section_id.to_le_bytes());
        buf[4..12].copy_from_slice(&self.offset.to_le_bytes());
        buf[12..20].copy_from_slice(&self.length.to_le_bytes());
        buf[20..28].copy_from_slice(&self.uncompressed_length.to_le_bytes());
        buf[28] = self.compression;
        buf[29] = u8::from(self.required);
        buf[30..32].copy_from_slice(&self.reserved.to_le_bytes());
        buf[32..36].copy_from_slice(&self.checksum.to_le_bytes());
        buf
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < COVEMAP_SECTION_ENTRY_LEN as usize {
            return Err(CoveError::BufferTooShort);
        }
        let bytes = &bytes[..COVEMAP_SECTION_ENTRY_LEN as usize];

        let section_id = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let offset = u64::from_le_bytes(bytes[4..12].try_into().unwrap());
        let length = u64::from_le_bytes(bytes[12..20].try_into().unwrap());
        let uncompressed_length = u64::from_le_bytes(bytes[20..28].try_into().unwrap());
        let compression = bytes[28];
        CompressionCodec::from_u8(compression).ok_or_else(|| {
            CoveError::BadSection(format!("unknown COVEMAP compression codec {compression}"))
        })?;
        let required = match bytes[29] {
            0 => false,
            1 => true,
            other => {
                return Err(CoveError::BadSection(format!(
                    "COVEMAP section required flag must be 0 or 1, got {other}"
                )))
            }
        };
        let reserved = u16::from_le_bytes(bytes[30..32].try_into().unwrap());
        if reserved != 0 {
            return Err(CoveError::ReservedNotZero);
        }
        let checksum = u32::from_le_bytes(bytes[32..36].try_into().unwrap());

        if length == 0 && uncompressed_length != 0 {
            return Err(CoveError::BadSection(
                "COVEMAP section length=0 requires uncompressed_length=0".into(),
            ));
        }
        if compression == CompressionCodec::None as u8 && length != uncompressed_length {
            return Err(CoveError::BadSection(
                "COVEMAP uncompressed section must have length == uncompressed_length".into(),
            ));
        }

        Ok(Self {
            section_id,
            offset,
            length,
            uncompressed_length,
            compression,
            required,
            reserved,
            checksum,
        })
    }
}

/// One section inside a `.covemap` artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CovemapSection {
    pub entry: CovemapSectionEntryV1,
    pub payload: Vec<u8>,
}

/// Spec §70.1 `CovemapPostscriptV1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CovemapPostscriptV1 {
    pub required_features: u64,
    pub optional_features: u64,
    pub file_len: u64,
    pub header_offset: u64,
    pub header_length: u64,
    pub checksum: u32,
}

impl CovemapPostscriptV1 {
    pub fn serialize(&self) -> [u8; COVEMAP_POSTSCRIPT_LEN as usize] {
        let mut buf = [0u8; COVEMAP_POSTSCRIPT_LEN as usize];
        buf[0..8].copy_from_slice(&self.required_features.to_le_bytes());
        buf[8..16].copy_from_slice(&self.optional_features.to_le_bytes());
        buf[16..24].copy_from_slice(&self.file_len.to_le_bytes());
        buf[24..32].copy_from_slice(&self.header_offset.to_le_bytes());
        buf[32..40].copy_from_slice(&self.header_length.to_le_bytes());
        let crc = checksum::crc32c(&buf[..40]);
        buf[40..44].copy_from_slice(&crc.to_le_bytes());
        buf
    }

    pub fn serialize_tail(
        &self,
    ) -> [u8; COVEMAP_POSTSCRIPT_LEN as usize + COVEMAP_POSTSCRIPT_TAIL_SIZE] {
        let mut tail = [0u8; COVEMAP_POSTSCRIPT_LEN as usize + COVEMAP_POSTSCRIPT_TAIL_SIZE];
        let payload = self.serialize();
        tail[..COVEMAP_POSTSCRIPT_LEN as usize].copy_from_slice(&payload);
        let n = COVEMAP_POSTSCRIPT_LEN as usize;
        tail[n..n + 2].copy_from_slice(&COVEMAP_POSTSCRIPT_VERSION_V1.to_le_bytes());
        tail[n + 2..n + 4].copy_from_slice(&COVEMAP_POSTSCRIPT_LEN.to_le_bytes());
        tail[n + 4..n + 8].copy_from_slice(&MAGIC_COVEMAP);
        tail
    }

    pub fn parse_from_tail(file_data: &[u8]) -> Result<Self, CoveError> {
        let total = COVEMAP_POSTSCRIPT_LEN as usize + COVEMAP_POSTSCRIPT_TAIL_SIZE;
        if file_data.len() < total {
            return Err(CoveError::BufferTooShort);
        }
        let start = file_data.len() - total;
        let tail = &file_data[start..];
        let n = COVEMAP_POSTSCRIPT_LEN as usize;

        let version = u16::from_le_bytes(tail[n..n + 2].try_into().unwrap());
        let len = u16::from_le_bytes(tail[n + 2..n + 4].try_into().unwrap());
        let mut magic = [0u8; 4];
        magic.copy_from_slice(&tail[n + 4..n + 8]);

        if magic != MAGIC_COVEMAP {
            return Err(CoveError::BadMagic);
        }
        if version != COVEMAP_POSTSCRIPT_VERSION_V1 {
            return Err(CoveError::BadVersion);
        }
        if len != COVEMAP_POSTSCRIPT_LEN {
            return Err(CoveError::BadSection(format!(
                "COVEMAP postscript_len must be {COVEMAP_POSTSCRIPT_LEN}, got {len}"
            )));
        }

        let payload: [u8; COVEMAP_POSTSCRIPT_LEN as usize] = tail[..n].try_into().unwrap();
        if checksum::crc32c(&payload[..40])
            != u32::from_le_bytes(payload[40..44].try_into().unwrap())
        {
            return Err(CoveError::ChecksumMismatch);
        }

        Ok(Self {
            required_features: u64::from_le_bytes(payload[0..8].try_into().unwrap()),
            optional_features: u64::from_le_bytes(payload[8..16].try_into().unwrap()),
            file_len: u64::from_le_bytes(payload[16..24].try_into().unwrap()),
            header_offset: u64::from_le_bytes(payload[24..32].try_into().unwrap()),
            header_length: u64::from_le_bytes(payload[32..40].try_into().unwrap()),
            checksum: u32::from_le_bytes(payload[40..44].try_into().unwrap()),
        })
    }
}

/// Parsed `.covemap` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CovemapFile {
    pub header: CovemapHeaderV1,
    pub mapping_version: String,
    pub sections: Vec<CovemapSection>,
    pub postscript: CovemapPostscriptV1,
}

impl CovemapFile {
    pub fn parse(file_data: &[u8]) -> Result<Self, CoveError> {
        let postscript = CovemapPostscriptV1::parse_from_tail(file_data)?;
        if postscript.file_len != file_data.len() as u64 {
            return Err(CoveError::BadSection(format!(
                "COVEMAP postscript file_len {} does not match actual file length {}",
                postscript.file_len,
                file_data.len()
            )));
        }

        let header_offset =
            usize::try_from(postscript.header_offset).map_err(|_| CoveError::OffsetRange)?;
        let header_length =
            usize::try_from(postscript.header_length).map_err(|_| CoveError::OffsetRange)?;
        let header_end = header_offset
            .checked_add(header_length)
            .ok_or(CoveError::ArithOverflow)?;
        if header_end > file_data.len() {
            return Err(CoveError::OffsetRange);
        }
        let header_region = &file_data[header_offset..header_end];
        let header = CovemapHeaderV1::parse(header_region)?;
        if header.required_features != postscript.required_features
            || header.optional_features != postscript.optional_features
        {
            return Err(CoveError::BadSection(
                "COVEMAP header and postscript feature bits disagree".into(),
            ));
        }
        validate_covemap_feature_bits(header.required_features, header.optional_features)?;
        let advertised_features = header.required_features | header.optional_features;

        let fixed_len = usize::from(header.header_len);
        let mapping_version_len = usize::from(header.mapping_version_len);
        let entries_len = usize::try_from(header.section_count)
            .map_err(|_| CoveError::ArithOverflow)?
            .checked_mul(COVEMAP_SECTION_ENTRY_LEN as usize)
            .ok_or(CoveError::ArithOverflow)?;
        let expected_region_len = fixed_len
            .checked_add(mapping_version_len)
            .and_then(|len| len.checked_add(entries_len))
            .ok_or(CoveError::ArithOverflow)?;
        if expected_region_len != header_region.len() {
            return Err(CoveError::BadSection(
                "COVEMAP header_length disagrees with header contents".into(),
            ));
        }

        let version_start = fixed_len;
        let version_end = version_start
            .checked_add(mapping_version_len)
            .ok_or(CoveError::ArithOverflow)?;
        let mapping_version = std::str::from_utf8(&header_region[version_start..version_end])
            .map_err(|_| CoveError::BadSection("COVEMAP mapping_version is not UTF-8".into()))?
            .to_string();
        if mapping_version.trim().is_empty() {
            return Err(CoveError::BadSection(
                "COVEMAP mapping_version must not be empty".into(),
            ));
        }

        let mut sections = Vec::with_capacity(header.section_count as usize);
        let mut entry_offset = version_end;
        for _ in 0..header.section_count {
            let entry = CovemapSectionEntryV1::parse(&header_region[entry_offset..])?;
            validate_covemap_section_codec_feature_advertisement(advertised_features, &entry)?;
            entry_offset = entry_offset
                .checked_add(COVEMAP_SECTION_ENTRY_LEN as usize)
                .ok_or(CoveError::ArithOverflow)?;
            let payload = decode_section_payload(file_data, &entry)?.into_owned();
            sections.push(CovemapSection { entry, payload });
        }

        Ok(Self {
            header,
            mapping_version,
            sections,
            postscript,
        })
    }

    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        if self.mapping_version.trim().is_empty() {
            return Err(CoveError::BadSection(
                "COVEMAP mapping_version must not be empty".into(),
            ));
        }

        let mapping_version_bytes = self.mapping_version.as_bytes();
        let mapping_version_len = u16::try_from(mapping_version_bytes.len()).map_err(|_| {
            CoveError::BadSection("COVEMAP mapping_version exceeds u16::MAX".into())
        })?;
        let section_count = u32::try_from(self.sections.len())
            .map_err(|_| CoveError::BadSection("too many COVEMAP sections".into()))?;

        let header_region_len = usize::from(COVEMAP_HEADER_LEN)
            .checked_add(mapping_version_bytes.len())
            .and_then(|len| {
                len.checked_add(self.sections.len() * usize::from(COVEMAP_SECTION_ENTRY_LEN))
            })
            .ok_or(CoveError::ArithOverflow)?;

        let mut encoded_sections = Vec::with_capacity(self.sections.len());
        let mut section_entries = Vec::with_capacity(self.sections.len());
        let mut payload_offset = header_region_len as u64;
        let mut required_codec_features = 0u64;
        let mut optional_codec_features = 0u64;

        for section in &self.sections {
            let encoded =
                compression::encode_payload_for_codec(&section.payload, section.entry.compression)?;
            let codec_feature = covemap_codec_feature_bit(section.entry.compression)?;
            if section.entry.required {
                required_codec_features |= codec_feature;
            } else {
                optional_codec_features |= codec_feature;
            }
            let length = encoded.len() as u64;
            let uncompressed_length = section.payload.len() as u64;
            if length == 0 && uncompressed_length != 0 {
                return Err(CoveError::BadSection(
                    "COVEMAP empty encoded payload requires empty uncompressed payload".into(),
                ));
            }
            let entry = CovemapSectionEntryV1 {
                section_id: section.entry.section_id,
                offset: payload_offset,
                length,
                uncompressed_length,
                compression: section.entry.compression,
                required: section.entry.required,
                reserved: 0,
                checksum: checksum::crc32c(&encoded),
            };
            payload_offset = payload_offset
                .checked_add(length)
                .ok_or(CoveError::ArithOverflow)?;
            encoded_sections.push(encoded);
            section_entries.push(entry);
        }

        let postscript_total =
            u64::from(COVEMAP_POSTSCRIPT_LEN) + COVEMAP_POSTSCRIPT_TAIL_SIZE as u64;
        let file_len = payload_offset
            .checked_add(postscript_total)
            .ok_or(CoveError::ArithOverflow)?;

        let mut header = self.header.clone();
        header.header_len = COVEMAP_HEADER_LEN;
        header.version_major = COVEMAP_VERSION_MAJOR_V1;
        header.version_minor = COVEMAP_VERSION_MINOR_V1;
        header.section_count = section_count;
        header.mapping_version_len = mapping_version_len;
        header.reserved0 = 0;
        header.reserved = [0u8; 32];
        header.required_features |= FEATURE_SEMANTIC_MAP | required_codec_features;
        header.optional_features |= optional_codec_features;
        header.optional_features &= !header.required_features;
        validate_covemap_feature_bits(header.required_features, header.optional_features)?;
        let header_bytes = header.serialize();

        let mut out = Vec::with_capacity(file_len as usize);
        out.extend_from_slice(&header_bytes);
        out.extend_from_slice(mapping_version_bytes);
        for entry in &section_entries {
            out.extend_from_slice(&entry.serialize());
        }
        for encoded in &encoded_sections {
            out.extend_from_slice(encoded);
        }

        let postscript = CovemapPostscriptV1 {
            required_features: header.required_features,
            optional_features: header.optional_features,
            file_len,
            header_offset: 0,
            header_length: header_region_len as u64,
            checksum: 0,
        };
        out.extend_from_slice(&postscript.serialize_tail());
        debug_assert_eq!(out.len() as u64, file_len);
        Ok(out)
    }

    pub fn validate_map_sections(&self) -> Result<(), CoveError> {
        let mut map_sections = Vec::<EmbeddedMapSection>::with_capacity(self.sections.len());
        for section in &self.sections {
            let section_kind = u16::try_from(section.entry.section_id)
                .ok()
                .and_then(SectionKind::from_u16)
                .ok_or(CoveError::MapInvalid)?;
            match section_kind {
                SectionKind::MapSourceCatalog
                | SectionKind::MapFunctionRegistry
                | SectionKind::MapIdentityRuleCatalog
                | SectionKind::MapRowSemanticsCatalog
                | SectionKind::MapAssertionLog
                | SectionKind::MapIdentityEquivalenceIndex
                | SectionKind::MapEvidenceIndex
                | SectionKind::MapConversionReport
                | SectionKind::MapProjectionCatalog => {
                    map_sections.push(parse_embedded_section(section_kind, &section.payload)?);
                }
                _ => return Err(CoveError::MapInvalid),
            }
        }
        validate_embedded_sections(&map_sections)
    }

    pub fn parse_validated(file_data: &[u8]) -> Result<Self, CoveError> {
        let file = Self::parse(file_data)?;
        file.validate_map_sections()?;
        Ok(file)
    }
}

fn decode_section_payload<'a>(
    file_data: &'a [u8],
    entry: &CovemapSectionEntryV1,
) -> Result<Cow<'a, [u8]>, CoveError> {
    let end = entry
        .offset
        .checked_add(entry.length)
        .ok_or(CoveError::ArithOverflow)?;
    if end as usize > file_data.len() {
        return Err(CoveError::OffsetRange);
    }
    let raw = &file_data[entry.offset as usize..end as usize];
    if checksum::crc32c(raw) != entry.checksum {
        return Err(CoveError::ChecksumMismatch);
    }
    let spec = CoveSectionSpecV1 {
        offset: entry.offset,
        length: entry.length,
        uncompressed_length: entry.uncompressed_length,
        compression: entry.compression,
        encryption: 0,
        alignment_log2: 0,
        flags: 0,
        crc32c: entry.checksum,
        reserved: 0,
    };
    compression::section_spec_payload(file_data, &spec)
}

fn validate_covemap_feature_bits(
    required_features: u64,
    _optional_features: u64,
) -> Result<(), CoveError> {
    let unknown_required = required_features & !KNOWN_FEATURE_BITS_MASK;
    if unknown_required != 0 {
        return Err(CoveError::UnknownRequiredFeature(unknown_required));
    }
    if required_features & FEATURE_SEMANTIC_MAP == 0 {
        return Err(CoveError::BadSection(
            "COVEMAP artifacts require FEATURE_SEMANTIC_MAP in required_features".into(),
        ));
    }
    Ok(())
}

fn validate_covemap_section_codec_feature_advertisement(
    advertised_features: u64,
    entry: &CovemapSectionEntryV1,
) -> Result<(), CoveError> {
    let codec_feature = covemap_codec_feature_bit(entry.compression)?;
    if codec_feature != 0 && advertised_features & codec_feature == 0 {
        return Err(CoveError::BadSection(format!(
            "COVEMAP section {} uses compression codec {} but codec feature bit is not advertised",
            entry.section_id, entry.compression
        )));
    }
    Ok(())
}

fn covemap_codec_feature_bit(compression: u8) -> Result<u64, CoveError> {
    match CompressionCodec::from_u8(compression).ok_or_else(|| {
        CoveError::BadSection(format!("unknown COVEMAP compression codec {compression}"))
    })? {
        CompressionCodec::None => Ok(0),
        CompressionCodec::Lz4 => Ok(FEATURE_CODEC_LZ4),
        CompressionCodec::Zstd => Ok(FEATURE_CODEC_ZSTD),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "compression-lz4")]
    use crate::constants::FEATURE_CODEC_LZ4;
    #[cfg(feature = "compression-zstd")]
    use crate::constants::FEATURE_CODEC_ZSTD;

    fn sample_file() -> CovemapFile {
        CovemapFile {
            header: CovemapHeaderV1::new([0xA5; 16], 1_700_000_000_000_000),
            mapping_version: "example/v1".into(),
            sections: vec![
                CovemapSection {
                    entry: CovemapSectionEntryV1 {
                        section_id: 60,
                        offset: 0,
                        length: 0,
                        uncompressed_length: 0,
                        compression: CompressionCodec::None as u8,
                        required: true,
                        reserved: 0,
                        checksum: 0,
                    },
                    payload: br#"{"mapping_id":"m1","mapping_version":"example/v1"}"#.to_vec(),
                },
                CovemapSection {
                    entry: CovemapSectionEntryV1 {
                        section_id: 61,
                        offset: 0,
                        length: 0,
                        uncompressed_length: 0,
                        compression: CompressionCodec::None as u8,
                        required: false,
                        reserved: 0,
                        checksum: 0,
                    },
                    payload:
                        br#"{"mapping_id":"m1","mapping_version":"example/v1","functions":[]}"#
                            .to_vec(),
                },
            ],
            postscript: CovemapPostscriptV1 {
                required_features: FEATURE_SEMANTIC_MAP,
                optional_features: 0,
                file_len: 0,
                header_offset: 0,
                header_length: 0,
                checksum: 0,
            },
        }
    }

    fn rewrite_covemap_feature_bits(
        bytes: &mut [u8],
        required_features: u64,
        optional_features: u64,
    ) {
        let mut header = CovemapHeaderV1::parse(bytes).unwrap();
        header.required_features = required_features;
        header.optional_features = optional_features;
        bytes[..COVEMAP_HEADER_LEN as usize].copy_from_slice(&header.serialize());

        let mut postscript = CovemapPostscriptV1::parse_from_tail(bytes).unwrap();
        postscript.required_features = required_features;
        postscript.optional_features = optional_features;
        let tail_start =
            bytes.len() - (COVEMAP_POSTSCRIPT_LEN as usize + COVEMAP_POSTSCRIPT_TAIL_SIZE);
        bytes[tail_start..].copy_from_slice(&postscript.serialize_tail());
    }

    #[test]
    fn header_roundtrip_and_checksum() {
        let mut header = CovemapHeaderV1::new([0x11; 16], 42);
        header.section_count = 2;
        header.mapping_version_len = 10;
        let bytes = header.serialize();
        let parsed = CovemapHeaderV1::parse(&bytes).unwrap();
        assert_eq!(parsed.mapping_id, [0x11; 16]);
        assert_eq!(parsed.created_at_us, 42);
        assert_eq!(parsed.section_count, 2);
        assert_eq!(parsed.mapping_version_len, 10);
    }

    #[test]
    fn section_entry_roundtrip() {
        let entry = CovemapSectionEntryV1 {
            section_id: 67,
            offset: 123,
            length: 45,
            uncompressed_length: 45,
            compression: CompressionCodec::None as u8,
            required: true,
            reserved: 0,
            checksum: 0xDEADBEEF,
        };
        let parsed = CovemapSectionEntryV1::parse(&entry.serialize()).unwrap();
        assert_eq!(parsed, entry);
    }

    #[test]
    fn file_roundtrip_two_sections() {
        let file = sample_file();
        let bytes = file.serialize().unwrap();
        let parsed = CovemapFile::parse(&bytes).unwrap();
        assert_eq!(parsed.mapping_version, "example/v1");
        assert_eq!(parsed.sections.len(), 2);
        assert_eq!(parsed.sections[0].entry.section_id, 60);
        assert_eq!(
            parsed.sections[1].payload,
            br#"{"mapping_id":"m1","mapping_version":"example/v1","functions":[]}"#
        );
        assert_eq!(parsed.postscript.file_len, bytes.len() as u64);
    }

    #[test]
    fn file_rejects_unknown_required_feature() {
        let mut bytes = sample_file().serialize().unwrap();
        rewrite_covemap_feature_bits(&mut bytes, FEATURE_SEMANTIC_MAP | (1u64 << 63), 0);
        assert!(matches!(
            CovemapFile::parse(&bytes),
            Err(CoveError::UnknownRequiredFeature(bits)) if bits == 1u64 << 63
        ));
    }

    #[test]
    fn file_requires_semantic_map_feature() {
        let mut bytes = sample_file().serialize().unwrap();
        rewrite_covemap_feature_bits(&mut bytes, 0, 0);
        assert!(matches!(
            CovemapFile::parse(&bytes),
            Err(CoveError::BadSection(_))
        ));
    }

    #[cfg(feature = "compression-lz4")]
    #[test]
    fn file_rejects_compressed_section_without_codec_feature() {
        let mut file = sample_file();
        file.sections[0].entry.compression = CompressionCodec::Lz4 as u8;
        let mut bytes = file.serialize().unwrap();
        rewrite_covemap_feature_bits(&mut bytes, FEATURE_SEMANTIC_MAP, 0);
        assert!(matches!(
            CovemapFile::parse(&bytes),
            Err(CoveError::BadSection(_))
        ));
    }

    #[cfg(all(feature = "compression-lz4", feature = "compression-zstd"))]
    #[test]
    fn serialize_advertises_required_and_optional_section_codecs() {
        let mut file = sample_file();
        file.sections[0].entry.compression = CompressionCodec::Lz4 as u8;
        file.sections[1].entry.compression = CompressionCodec::Zstd as u8;
        let bytes = file.serialize().unwrap();
        let parsed = CovemapFile::parse(&bytes).unwrap();
        assert_ne!(parsed.header.required_features & FEATURE_CODEC_LZ4, 0);
        assert_eq!(parsed.header.optional_features & FEATURE_CODEC_LZ4, 0);
        assert_ne!(parsed.header.optional_features & FEATURE_CODEC_ZSTD, 0);
        assert_eq!(parsed.sections[0].payload, file.sections[0].payload);
        assert_eq!(parsed.sections[1].payload, file.sections[1].payload);
    }

    #[test]
    fn validated_file_roundtrip_checks_map_payloads() {
        let bytes = sample_file().serialize().unwrap();
        let parsed = CovemapFile::parse_validated(&bytes).unwrap();
        assert_eq!(parsed.sections.len(), 2);
    }

    #[test]
    fn standalone_map_validation_rejects_bad_payload_schema() {
        let mut file = sample_file();
        file.sections[0].payload = br#"{"mapping_version":"example/v1","sources":[]}"#.to_vec();
        let bytes = file.serialize().unwrap();
        assert_eq!(
            CovemapFile::parse_validated(&bytes),
            Err(CoveError::MapInvalid)
        );
    }

    #[test]
    fn file_rejects_flipped_tail_magic() {
        let mut bytes = sample_file().serialize().unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        assert_eq!(CovemapFile::parse(&bytes), Err(CoveError::BadMagic));
    }

    #[test]
    fn file_rejects_corrupted_section_checksum() {
        let mut bytes = sample_file().serialize().unwrap();
        let payload_offset = bytes
            .windows(br#"{"mapping_id":"m1","mapping_version":"example/v1"}"#.len())
            .position(|window| window == br#"{"mapping_id":"m1","mapping_version":"example/v1"}"#)
            .unwrap();
        bytes[payload_offset] ^= 0xFF;
        assert_eq!(CovemapFile::parse(&bytes), Err(CoveError::ChecksumMismatch));
    }

    #[test]
    fn section_entry_rejects_invalid_required_flag() {
        let mut bytes = CovemapSectionEntryV1 {
            section_id: 60,
            offset: 0,
            length: 0,
            uncompressed_length: 0,
            compression: CompressionCodec::None as u8,
            required: false,
            reserved: 0,
            checksum: 0,
        }
        .serialize();
        bytes[29] = 2;
        let err = CovemapSectionEntryV1::parse(&bytes).unwrap_err();
        assert!(matches!(err, CoveError::BadSection(_)));
    }
}
