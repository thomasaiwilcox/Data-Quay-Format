use crate::{
    checksum, compression,
    constants::FEATURE_PAGE_PAYLOAD_ELISION,
    page::{
        page_uses_payload_elision, ColumnPageIndex, ColumnPageIndexEntryV1, PAGE_FLAG_ALL_NON_NULL,
    },
    page_payload::ColumnPagePayloadV1,
    page_validation::{
        validate_column_page_payload, validate_stats_only_constant_page, PageValidationContext,
    },
    segment::{TableColumnDirectoryEntryV1, TABLE_COLUMN_DIRECTORY_ENTRY_LEN},
    CoveError,
};

use super::{
    temporal::{validate_temporal_order, RecordKind, TemporalRowKey},
    TEMPORAL_ROW_ENTRY_LEN, TEMPORAL_SEGMENT_HEADER_LEN,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemporalSegmentHeaderV1 {
    pub segment_id: u32,
    pub object_type_id: u32,
    pub time_range_start_us: i64,
    pub time_range_end_us: i64,
    pub csn_min: u64,
    pub csn_max: u64,
    pub row_count: u32,
    pub morsel_count: u32,
    pub morsel_row_count: u32,
    pub column_count: u32,
    pub row_directory_offset: u64,
    pub column_directory_offset: u64,
    pub page_index_offset: u64,
    pub data_offset: u64,
    pub flags: u32,
    pub checksum: u32,
}

impl TemporalSegmentHeaderV1 {
    pub fn serialize(&self) -> [u8; TEMPORAL_SEGMENT_HEADER_LEN] {
        let mut out = [0u8; TEMPORAL_SEGMENT_HEADER_LEN];
        out[0..4].copy_from_slice(&self.segment_id.to_le_bytes());
        out[4..8].copy_from_slice(&self.object_type_id.to_le_bytes());
        out[8..16].copy_from_slice(&self.time_range_start_us.to_le_bytes());
        out[16..24].copy_from_slice(&self.time_range_end_us.to_le_bytes());
        out[24..32].copy_from_slice(&self.csn_min.to_le_bytes());
        out[32..40].copy_from_slice(&self.csn_max.to_le_bytes());
        out[40..44].copy_from_slice(&self.row_count.to_le_bytes());
        out[44..48].copy_from_slice(&self.morsel_count.to_le_bytes());
        out[48..52].copy_from_slice(&self.morsel_row_count.to_le_bytes());
        out[52..56].copy_from_slice(&self.column_count.to_le_bytes());
        out[56..64].copy_from_slice(&self.row_directory_offset.to_le_bytes());
        out[64..72].copy_from_slice(&self.column_directory_offset.to_le_bytes());
        out[72..80].copy_from_slice(&self.page_index_offset.to_le_bytes());
        out[80..88].copy_from_slice(&self.data_offset.to_le_bytes());
        out[88..92].copy_from_slice(&self.flags.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[92..96].copy_from_slice(&crc.to_le_bytes());
        out
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < TEMPORAL_SEGMENT_HEADER_LEN {
            return Err(CoveError::BufferTooShort);
        }
        let bytes = &bytes[..TEMPORAL_SEGMENT_HEADER_LEN];
        let checksum_field = u32::from_le_bytes(bytes[92..96].try_into().unwrap());
        let mut for_crc = [0u8; TEMPORAL_SEGMENT_HEADER_LEN];
        for_crc.copy_from_slice(bytes);
        for_crc[92..96].fill(0);
        if checksum::crc32c(&for_crc) != checksum_field {
            return Err(CoveError::ChecksumMismatch);
        }

        Ok(Self {
            segment_id: u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
            object_type_id: u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            time_range_start_us: i64::from_le_bytes(bytes[8..16].try_into().unwrap()),
            time_range_end_us: i64::from_le_bytes(bytes[16..24].try_into().unwrap()),
            csn_min: u64::from_le_bytes(bytes[24..32].try_into().unwrap()),
            csn_max: u64::from_le_bytes(bytes[32..40].try_into().unwrap()),
            row_count: u32::from_le_bytes(bytes[40..44].try_into().unwrap()),
            morsel_count: u32::from_le_bytes(bytes[44..48].try_into().unwrap()),
            morsel_row_count: u32::from_le_bytes(bytes[48..52].try_into().unwrap()),
            column_count: u32::from_le_bytes(bytes[52..56].try_into().unwrap()),
            row_directory_offset: u64::from_le_bytes(bytes[56..64].try_into().unwrap()),
            column_directory_offset: u64::from_le_bytes(bytes[64..72].try_into().unwrap()),
            page_index_offset: u64::from_le_bytes(bytes[72..80].try_into().unwrap()),
            data_offset: u64::from_le_bytes(bytes[80..88].try_into().unwrap()),
            flags: u32::from_le_bytes(bytes[88..92].try_into().unwrap()),
            checksum: checksum_field,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoveRecordRefV1 {
    pub segment_id: u32,
    pub row_index: u32,
    pub target_kind: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemporalRowEntryV1 {
    pub timestamp_us: i64,
    pub csn: u64,
    pub branch_key: u64,
    pub goid: [u8; 16],
    pub record_id: [u8; 16],
    pub record_kind: RecordKind,
    pub prev_ref: Option<CoveRecordRefV1>,
}

impl TemporalRowEntryV1 {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < TEMPORAL_ROW_ENTRY_LEN {
            return Err(CoveError::BufferTooShort);
        }
        let bytes = &bytes[..TEMPORAL_ROW_ENTRY_LEN];
        let timestamp_us = i64::from_le_bytes(bytes[0..8].try_into().unwrap());
        let csn = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
        let branch_key = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
        let mut goid = [0u8; 16];
        goid.copy_from_slice(&bytes[24..40]);
        let mut record_id = [0u8; 16];
        record_id.copy_from_slice(&bytes[40..56]);
        let record_kind = RecordKind::from_u8(bytes[56]).ok_or_else(|| {
            CoveError::BadSchema(format!("unknown temporal record kind {}", bytes[56]))
        })?;
        let prev_present = bytes[57];
        let target_kind = bytes[58];
        if bytes[59] != 0 {
            return Err(CoveError::ReservedNotZero);
        }
        if target_kind > 1 {
            return Err(CoveError::RefInvalid);
        }
        let prev_segment_id = u32::from_le_bytes(bytes[60..64].try_into().unwrap());
        let prev_row_index = u32::from_le_bytes(bytes[64..68].try_into().unwrap());
        let prev_ref = match prev_present {
            0 => {
                if target_kind != 0 || prev_segment_id != 0 || prev_row_index != 0 {
                    return Err(CoveError::RefInvalid);
                }
                None
            }
            1 => Some(CoveRecordRefV1 {
                segment_id: prev_segment_id,
                row_index: prev_row_index,
                target_kind,
            }),
            _ => return Err(CoveError::RefInvalid),
        };
        record_kind.validate_published()?;
        Ok(Self {
            timestamp_us,
            csn,
            branch_key,
            goid,
            record_id,
            record_kind,
            prev_ref,
        })
    }

    pub fn serialize(&self) -> [u8; TEMPORAL_ROW_ENTRY_LEN] {
        let mut out = [0u8; TEMPORAL_ROW_ENTRY_LEN];
        out[0..8].copy_from_slice(&self.timestamp_us.to_le_bytes());
        out[8..16].copy_from_slice(&self.csn.to_le_bytes());
        out[16..24].copy_from_slice(&self.branch_key.to_le_bytes());
        out[24..40].copy_from_slice(&self.goid);
        out[40..56].copy_from_slice(&self.record_id);
        out[56] = match self.record_kind {
            RecordKind::Delta => 0,
            RecordKind::Snapshot => 1,
            RecordKind::ReservedLegacyMaterializedDelta => 2,
            RecordKind::Baseline => 3,
            RecordKind::Tombstone => 4,
        };
        if let Some(prev_ref) = self.prev_ref {
            out[57] = 1;
            out[58] = prev_ref.target_kind;
            out[60..64].copy_from_slice(&prev_ref.segment_id.to_le_bytes());
            out[64..68].copy_from_slice(&prev_ref.row_index.to_le_bytes());
        }
        out
    }

    pub fn row_key(&self) -> TemporalRowKey {
        TemporalRowKey {
            timestamp_us: self.timestamp_us,
            csn: self.csn,
            branch_key: self.branch_key,
            goid: self.goid,
            record_id: self.record_id,
        }
    }

    pub fn trust_payload(&self) -> Vec<u8> {
        self.serialize().to_vec()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemporalSegmentData {
    pub header: TemporalSegmentHeaderV1,
    pub rows: Vec<TemporalRowEntryV1>,
    pub property_columns: Vec<TemporalPropertyColumn>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemporalPropertyColumn {
    pub directory: TableColumnDirectoryEntryV1,
    pub page_index: ColumnPageIndex,
    pub pages: Vec<TemporalPropertyPage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemporalPropertyPage {
    pub index_entry: ColumnPageIndexEntryV1,
    pub payload: Option<ColumnPagePayloadV1>,
}

impl TemporalSegmentData {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        Self::parse_inner(bytes, None)
    }

    pub fn parse_with_required_features(
        bytes: &[u8],
        required_features: u64,
    ) -> Result<Self, CoveError> {
        Self::parse_inner(bytes, Some(required_features))
    }

    fn parse_inner(bytes: &[u8], required_features: Option<u64>) -> Result<Self, CoveError> {
        let header = TemporalSegmentHeaderV1::parse(bytes)?;
        if header.row_count == 0 && header.morsel_count != 0 {
            return Err(CoveError::BadSchema(
                "temporal segment with zero rows cannot have morsels".into(),
            ));
        }
        if header.row_count != 0 && header.morsel_row_count == 0 {
            return Err(CoveError::BadSchema(
                "temporal segment with rows must declare morsel_row_count".into(),
            ));
        }
        let row_directory_offset =
            usize::try_from(header.row_directory_offset).map_err(|_| CoveError::OffsetRange)?;
        let column_directory_offset =
            usize::try_from(header.column_directory_offset).map_err(|_| CoveError::OffsetRange)?;
        let page_index_offset =
            usize::try_from(header.page_index_offset).map_err(|_| CoveError::OffsetRange)?;
        let data_offset =
            usize::try_from(header.data_offset).map_err(|_| CoveError::OffsetRange)?;
        if row_directory_offset < TEMPORAL_SEGMENT_HEADER_LEN
            || column_directory_offset < row_directory_offset
            || page_index_offset < column_directory_offset
            || data_offset < page_index_offset
            || data_offset > bytes.len()
        {
            return Err(CoveError::BadSchema(
                "temporal segment offsets are invalid".into(),
            ));
        }
        let row_bytes_len = (header.row_count as usize)
            .checked_mul(TEMPORAL_ROW_ENTRY_LEN)
            .ok_or(CoveError::ArithOverflow)?;
        let row_end = row_directory_offset
            .checked_add(row_bytes_len)
            .ok_or(CoveError::ArithOverflow)?;
        if row_end > column_directory_offset {
            return Err(CoveError::BadSchema(
                "temporal row directory exceeds declared boundary".into(),
            ));
        }
        let column_dir_len = (header.column_count as usize)
            .checked_mul(TABLE_COLUMN_DIRECTORY_ENTRY_LEN)
            .ok_or(CoveError::ArithOverflow)?;
        let column_dir_end = column_directory_offset
            .checked_add(column_dir_len)
            .ok_or(CoveError::ArithOverflow)?;
        if column_dir_end > page_index_offset {
            return Err(CoveError::BadSchema(
                "temporal property column directory exceeds declared boundary".into(),
            ));
        }
        let mut rows = Vec::with_capacity(header.row_count as usize);
        let mut pos = row_directory_offset;
        for _ in 0..header.row_count {
            rows.push(TemporalRowEntryV1::parse(
                &bytes[pos..pos + TEMPORAL_ROW_ENTRY_LEN],
            )?);
            pos += TEMPORAL_ROW_ENTRY_LEN;
        }
        let property_columns = parse_temporal_property_columns(
            bytes,
            &header,
            column_directory_offset,
            required_features,
        )?;
        let segment = Self {
            header,
            rows,
            property_columns,
        };
        segment.validate()?;
        Ok(segment)
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        let row_keys = self
            .rows
            .iter()
            .map(TemporalRowEntryV1::row_key)
            .collect::<Vec<_>>();
        validate_temporal_order(&row_keys)?;
        for pair in self.rows.windows(2) {
            if pair[1].csn < pair[0].csn {
                return Err(CoveError::BadSchema(
                    "temporal segment csn decreases in row order".into(),
                ));
            }
        }

        for (row_index, row) in self.rows.iter().enumerate() {
            if let Some(prev_ref) = row.prev_ref {
                if prev_ref.segment_id == self.header.segment_id
                    && prev_ref.row_index >= row_index as u32
                {
                    return Err(CoveError::RefInvalid);
                }
                if prev_ref.segment_id > self.header.segment_id {
                    return Err(CoveError::RefInvalid);
                }
            }
        }

        if let Some(first) = self.rows.first() {
            if first.timestamp_us < self.header.time_range_start_us
                || first.csn < self.header.csn_min
            {
                return Err(CoveError::BadSchema(
                    "temporal segment row falls before declared min range".into(),
                ));
            }
        }
        if let Some(last) = self.rows.last() {
            if last.timestamp_us > self.header.time_range_end_us || last.csn > self.header.csn_max {
                return Err(CoveError::BadSchema(
                    "temporal segment row falls after declared max range".into(),
                ));
            }
        }

        Ok(())
    }
}

fn parse_temporal_property_columns(
    bytes: &[u8],
    header: &TemporalSegmentHeaderV1,
    column_directory_offset: usize,
    required_features: Option<u64>,
) -> Result<Vec<TemporalPropertyColumn>, CoveError> {
    let page_index_offset =
        usize::try_from(header.page_index_offset).map_err(|_| CoveError::OffsetRange)?;
    let data_offset = usize::try_from(header.data_offset).map_err(|_| CoveError::OffsetRange)?;
    let mut out = Vec::with_capacity(header.column_count as usize);
    let mut pos = column_directory_offset;
    for _ in 0..header.column_count {
        let directory = TableColumnDirectoryEntryV1::parse(
            &bytes[pos..pos + TABLE_COLUMN_DIRECTORY_ENTRY_LEN],
        )?;
        pos += TABLE_COLUMN_DIRECTORY_ENTRY_LEN;

        let page_index_start =
            usize::try_from(directory.page_index_offset).map_err(|_| CoveError::OffsetRange)?;
        let page_index_end = usize::try_from(
            directory
                .page_index_offset
                .checked_add(directory.page_index_length)
                .ok_or(CoveError::ArithOverflow)?,
        )
        .map_err(|_| CoveError::OffsetRange)?;
        if page_index_start < page_index_offset || page_index_end > data_offset {
            return Err(CoveError::SegmentCorrupt);
        }
        let page_index = ColumnPageIndex::parse(&bytes[page_index_start..page_index_end])?;

        let data_start =
            usize::try_from(directory.data_offset).map_err(|_| CoveError::OffsetRange)?;
        let data_end = usize::try_from(
            directory
                .data_offset
                .checked_add(directory.data_length)
                .ok_or(CoveError::ArithOverflow)?,
        )
        .map_err(|_| CoveError::OffsetRange)?;
        if data_start < data_offset || data_end > bytes.len() {
            return Err(CoveError::SegmentCorrupt);
        }

        let mut pages = Vec::with_capacity(page_index.entries.len());
        for page in &page_index.entries {
            if page.column_id != directory.column_id {
                return Err(CoveError::PageCorrupt);
            }
            validate_temporal_property_page_elision_features(page, required_features)?;
            let context = PageValidationContext {
                table_id: None,
                segment_id: Some(header.segment_id),
                column_id: directory.column_id,
                logical_type: directory.logical_type,
                physical_kind: directory.physical_kind,
                dictionary: None,
                zone_stats: None,
                codec_descriptors: &[],
                nested_schema: None,
            };
            if page.page_length == 0 {
                validate_temporal_property_stats_only_page(&context, page)?;
                pages.push(TemporalPropertyPage {
                    index_entry: page.clone(),
                    payload: None,
                });
                continue;
            }
            let page_start =
                usize::try_from(page.page_offset).map_err(|_| CoveError::OffsetRange)?;
            let page_end = usize::try_from(
                page.page_offset
                    .checked_add(page.page_length)
                    .ok_or(CoveError::ArithOverflow)?,
            )
            .map_err(|_| CoveError::OffsetRange)?;
            if page_start < data_start || page_end > data_end {
                return Err(CoveError::PageCorrupt);
            }
            let decoded = compression::column_page_payload(&bytes[page_start..page_end], page)?;
            let payload = ColumnPagePayloadV1::parse(decoded.as_ref())?;
            validate_column_page_payload(&context, page, &payload)?;
            pages.push(TemporalPropertyPage {
                index_entry: page.clone(),
                payload: Some(payload),
            });
        }
        out.push(TemporalPropertyColumn {
            directory,
            page_index,
            pages,
        });
    }
    Ok(out)
}

pub(crate) fn validate_temporal_property_page_elision_features(
    page: &ColumnPageIndexEntryV1,
    required_features: Option<u64>,
) -> Result<(), CoveError> {
    if page_uses_payload_elision(page.flags)
        && required_features.is_some_and(|bits| bits & FEATURE_PAGE_PAYLOAD_ELISION == 0)
    {
        return Err(CoveError::BadSection(
            "COVE-O property page payload-elision flags require FEATURE_PAGE_PAYLOAD_ELISION in required_features"
                .into(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_temporal_property_stats_only_page(
    context: &PageValidationContext<'_>,
    page: &ColumnPageIndexEntryV1,
) -> Result<(), CoveError> {
    validate_stats_only_constant_page(context, page)?;
    if page.flags & PAGE_FLAG_ALL_NON_NULL != 0 {
        return Err(CoveError::PageCorrupt);
    }
    Ok(())
}
