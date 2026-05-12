use super::*;

impl MinimalCoveWriter {
    /// Serialize and durably publish the file to `path` using Spec §75.
    pub fn publish_durable(&self, path: &Path) -> Result<PathBuf, CoveError> {
        durable::durable_replace_with_writer(path, |file| self.write_to(file))
    }

    /// Validate builder inputs that have strict on-disk bounds in v1.
    fn validate_inputs(&self) -> Result<(), CoveError> {
        metadata::validate(&self.metadata_json)?;
        if self.sections.len() > u32::MAX as usize {
            return Err(CoveError::ArithOverflow);
        }
        if PrimaryProfile::from_u8(self.primary_profile).is_none() {
            return Err(CoveError::BadSection(format!(
                "unknown primary_profile {}",
                self.primary_profile
            )));
        }
        if ProducerScopeKind::from_u16(self.producer_scope_kind).is_none() {
            return Err(CoveError::BadSection(format!(
                "unknown producer_scope_kind {}",
                self.producer_scope_kind
            )));
        }
        if self.required_features & !KNOWN_FEATURE_BITS_MASK != 0 {
            return Err(CoveError::UnknownRequiredFeature(
                self.required_features & !KNOWN_FEATURE_BITS_MASK,
            ));
        }
        for section in &self.sections {
            if SectionKind::from_u16(section.section_kind).is_none() {
                return Err(CoveError::BadSection(format!(
                    "unknown section_kind {}",
                    section.section_kind
                )));
            }
            if PrimaryProfile::from_u8(section.profile).is_none() {
                return Err(CoveError::BadSection(format!(
                    "unknown section profile {}",
                    section.profile
                )));
            }
            if CompressionCodec::from_u8(section.compression).is_none() {
                return Err(CoveError::BadSection(format!(
                    "unknown compression codec {}",
                    section.compression
                )));
            }
            if section.required_features & !KNOWN_FEATURE_BITS_MASK != 0 {
                return Err(CoveError::UnknownRequiredFeature(
                    section.required_features & !KNOWN_FEATURE_BITS_MASK,
                ));
            }
        }
        Ok(())
    }

    /// Create a writer with all-zero defaults (empty table-scan file).
    pub fn new() -> Self {
        Self {
            created_at_us: 0,
            file_id: [0u8; 16],
            producer_scope_id: [0u8; 16],
            producer_scope_kind: 0,
            primary_profile: PrimaryProfile::TableScan as u8,
            required_features: FEATURE_TABLE_PROFILE,
            optional_features: 0,
            metadata_json: vec![],
            sections: vec![],
        }
    }

    /// Stream the file to `writer`.
    ///
    /// The writer must be positioned at byte 0 for a new, truncated output
    /// target. COVE offsets are absolute from the start of the file.
    pub fn write_to<W: Write + Seek>(&self, writer: &mut W) -> Result<(), CoveError> {
        self.validate_inputs()?;
        if writer.stream_position()? != 0 {
            return Err(CoveError::BadSection(
                "MinimalCoveWriter::write_to requires a writer positioned at byte 0".into(),
            ));
        }

        writer.write_all(&[0u8; HEADER_SIZE])?;

        let mut section_entries: Vec<CoveSectionEntryV1> = Vec::new();
        for (idx, section) in self.sections.iter().enumerate() {
            let section_offset = writer.stream_position()?;
            let section_data =
                compression::encode_payload_for_codec(&section.data, section.compression)?;
            let section_len =
                u64::try_from(section_data.len()).map_err(|_| CoveError::ArithOverflow)?;
            let section_uncompressed_len =
                u64::try_from(section.data.len()).map_err(|_| CoveError::ArithOverflow)?;
            let section_crc = checksum::crc32c(&section_data);

            writer.write_all(&section_data)?;

            section_entries.push(CoveSectionEntryV1 {
                section_id: u32::try_from(idx + 1).map_err(|_| CoveError::ArithOverflow)?,
                section_kind: section.section_kind,
                profile: section.profile,
                flags: section.flags,
                offset: section_offset,
                length: section_len,
                uncompressed_length: section_uncompressed_len,
                item_count: section.item_count,
                row_count: section.row_count,
                compression: section.compression,
                encryption: 0,
                alignment_log2: section.alignment_log2,
                reserved0: 0,
                required_features: section.required_features,
                optional_features: section.optional_features,
                crc32c: section_crc,
                reserved1: 0,
            });
        }

        let footer_offset = writer.stream_position()?;
        let section_count =
            u32::try_from(section_entries.len()).map_err(|_| CoveError::ArithOverflow)?;
        let metadata_len =
            u32::try_from(self.metadata_json.len()).map_err(|_| CoveError::ArithOverflow)?;

        let footer_header = CoveFooterHeaderV1 {
            footer_magic: MAGIC_COVE_FOOTER,
            footer_version: FOOTER_VERSION_V1,
            header_len: FOOTER_HEADER_SIZE as u16,
            section_count,
            section_entry_len: SECTION_ENTRY_LEN,
            flags: 0,
            metadata_len,
            reserved: [0u8; 24],
        };
        let mut footer_bytes = Vec::with_capacity(
            FOOTER_HEADER_SIZE
                + section_entries.len() * usize::from(SECTION_ENTRY_LEN)
                + self.metadata_json.len(),
        );
        footer_bytes.extend_from_slice(&footer_header.serialize());
        for entry in &section_entries {
            footer_bytes.extend_from_slice(&entry.serialize());
        }
        footer_bytes.extend_from_slice(&self.metadata_json);
        let footer_len = u64::try_from(footer_bytes.len()).map_err(|_| CoveError::ArithOverflow)?;
        let footer_crc = checksum::crc32c(&footer_bytes);
        writer.write_all(&footer_bytes)?;

        // file_len includes the entire postscript tail (payload + version + len + magic).
        let file_len_before_tail = writer.stream_position()?;
        let total_file_len = file_len_before_tail
            .checked_add(POSTSCRIPT_SIZE as u64)
            .and_then(|len| len.checked_add(2 + 2 + 4))
            .ok_or(CoveError::ArithOverflow)?;

        let postscript = CovePostscriptV1 {
            required_features: self.required_features,
            optional_features: self.optional_features,
            file_len: total_file_len,
            footer: CoveSectionSpecV1 {
                offset: footer_offset,
                length: footer_len,
                uncompressed_length: footer_len,
                compression: 0,
                encryption: 0,
                alignment_log2: 0,
                flags: 0,
                crc32c: footer_crc,
                reserved: 0,
            },
            checksum: 0,
        };
        writer.write_all(&postscript.serialize_tail())?;

        let header = CoveHeaderV1 {
            magic: MAGIC_COVE,
            header_len: HEADER_LEN_V1,
            version_major: VERSION_MAJOR_V1,
            version_minor: 0,
            primary_profile: self.primary_profile,
            endianness: ENDIANNESS_LITTLE,
            flags: 0,
            required_features: self.required_features,
            optional_features: self.optional_features,
            file_id: self.file_id,
            producer_scope_id: self.producer_scope_id,
            producer_scope_kind: self.producer_scope_kind,
            reserved_scope_flags: 0,
            created_at_us: self.created_at_us,
            feature_set_section_id: 0,
            profile_capability_section_id: 0,
            fast_metadata_section_id: 0,
            v2_flags: 0,
            reserved: [0u8; 64],
            checksum: 0,
        };
        let header_bytes = header.serialize();
        // INVARIANT: the header checksum covers final feature bits and IDs, and
        // the placeholder may be replaced only after every offset and file_len
        // has been computed from bytes already written to the stream.
        writer.seek(SeekFrom::Start(0))?;
        writer.write_all(&header_bytes)?;
        writer.seek(SeekFrom::Start(total_file_len))?;
        Ok(())
    }

    /// Serialise the file to a byte vector.
    ///
    /// Layout:
    /// ```text
    /// [Header: 128 bytes]
    /// [Section payloads ...]
    /// [Footer header: 44 bytes]
    /// [Section entries: section_count × 76 bytes]
    /// [Metadata JSON: metadata_len bytes]
    /// [Postscript: 64 bytes]
    /// [postscript_version: u16]
    /// [postscript_len: u16]
    /// [Magic: "COV1"]
    /// ```
    pub fn write(&self) -> Result<Vec<u8>, CoveError> {
        let mut cursor = Cursor::new(Vec::new());
        self.write_to(&mut cursor)?;
        Ok(cursor.into_inner())
    }

    /// Convenience wrapper: write an empty COVE-T file with no sections.
    pub fn write_empty_file() -> Result<Vec<u8>, CoveError> {
        Self::new().write()
    }
}

impl Default for MinimalCoveWriter {
    fn default() -> Self {
        Self::new()
    }
}
