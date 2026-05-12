//! Cove Format (COVE) v1.0 — Encoded array decoding.
//!
//! This module provides access to individual rows within an encoded column
//! array, supporting the encodings described in Section 20 of the specification.

use crate::{
    constants::{CoveEncodingKind, CoveLogicalType, CovePhysicalKind},
    dictionary::{DictionaryValue, FileDictionary},
    encoding::{
        bit_packed::{BitPacked, BitPackedPayload},
        delta::{Delta, DeltaPayload},
        frame_of_reference::{ForPayload, FrameOfReference},
        local_codebook::{LocalCodebookPayload, LocalCodebookValue},
        patched_base::{PatchedBase, PatchedBasePayload},
        rle::{Rle, RlePayload},
        run_end::{RunEnd, RunEndPayload},
        sparse::{Sparse, SparsePayload},
        Encoding,
    },
    validity::ValidityBitmap,
    wire, CoveError,
};

/// A decoded value from an encoded array row.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum CoveArrayValue<'a> {
    /// The row is null.
    Null,
    /// Raw bytes (PlainFixed, Constant, VarBytes).
    Bytes(&'a [u8]),
    /// Owned raw bytes decoded from an owned child structure such as LocalCodebook.
    OwnedBytes(Vec<u8>),
    /// A decoded LEB128 varint (PlainVarint).
    Varint(u64),
    /// A decoded signed integer from a numeric cascade.
    Int64(i64),
    /// A raw FileCode before dictionary resolution.
    FileCode(u32),
    /// A resolved dictionary value.
    DictValue(DictionaryValue),
    /// A raw NumCode.
    NumCode(u64),
    /// A decoded boolean value.
    Boolean(bool),
    /// The validity bit for this row (Validity-encoded columns).
    ValidityBit(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VariableOffsetKind {
    PlainVarint,
    VarBytes,
    CanonicalLengthPrefixed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VariableOffsets {
    kind: VariableOffsetKind,
    row_offsets: Vec<usize>,
}

impl VariableOffsets {
    fn plain_varint(data: &[u8], row_count: u64) -> Result<Self, CoveError> {
        let mut row_offsets = Vec::with_capacity(expected_row_count(row_count)?);
        let mut pos = 0usize;
        for _ in 0..row_count {
            if pos >= data.len() {
                return Err(CoveError::OffsetRange);
            }
            row_offsets.push(pos);
            let (_value, consumed) = wire::decode_u64_leb128(&data[pos..])?;
            pos = pos.checked_add(consumed).ok_or(CoveError::ArithOverflow)?;
        }
        Ok(Self {
            kind: VariableOffsetKind::PlainVarint,
            row_offsets,
        })
    }

    fn varbytes(data: &[u8], row_count: u64) -> Result<Self, CoveError> {
        let mut row_offsets = Vec::with_capacity(expected_row_count(row_count)?);
        let mut pos = 0usize;
        for _ in 0..row_count {
            row_offsets.push(pos);
            let len = read_u32_le(data, pos)? as usize;
            pos = pos
                .checked_add(4)
                .and_then(|offset| offset.checked_add(len))
                .ok_or(CoveError::ArithOverflow)?;
            if pos > data.len() {
                return Err(CoveError::OffsetRange);
            }
        }
        Ok(Self {
            kind: VariableOffsetKind::VarBytes,
            row_offsets,
        })
    }

    fn canonical_length_prefixed(data: &[u8], row_count: u64) -> Result<Self, CoveError> {
        let mut row_offsets = Vec::with_capacity(expected_row_count(row_count)?);
        let mut pos = 0usize;
        for _ in 0..row_count {
            row_offsets.push(pos);
            let (len, consumed) = wire::decode_u64_leb128(&data[pos..])?;
            let len = usize::try_from(len).map_err(|_| CoveError::ArithOverflow)?;
            pos = pos
                .checked_add(consumed)
                .and_then(|offset| offset.checked_add(len))
                .ok_or(CoveError::ArithOverflow)?;
            if pos > data.len() {
                return Err(CoveError::OffsetRange);
            }
        }
        Ok(Self {
            kind: VariableOffsetKind::CanonicalLengthPrefixed,
            row_offsets,
        })
    }

    fn offset_for_row(&self, row: u64) -> Result<usize, CoveError> {
        self.row_offsets
            .get(row as usize)
            .copied()
            .ok_or(CoveError::OffsetRange)
    }
}

#[derive(Debug, Clone, PartialEq)]
enum PreparedArrayRepr<'a> {
    Direct,
    VariableOffsets(VariableOffsets),
    DecodedValues(Vec<CoveArrayValue<'a>>),
}

/// Page-scoped prepared access for repeated row reads.
///
/// `EncodedArray::decode_row()` remains the one-off scalar API. Hot consumers
/// that touch many rows from the same page should prepare once and then read
/// through this wrapper so variable-width and transform encodings are decoded
/// at page scope instead of per row.
#[derive(Clone)]
pub struct PreparedEncodedArray<'a> {
    array: &'a EncodedArray<'a>,
    repr: PreparedArrayRepr<'a>,
}

impl<'a> PreparedEncodedArray<'a> {
    pub fn new(array: &'a EncodedArray<'a>) -> Result<Self, CoveError> {
        let repr = match array.encoding {
            CoveEncodingKind::PlainVarint => PreparedArrayRepr::VariableOffsets(
                VariableOffsets::plain_varint(array.data, array.row_count)?,
            ),
            CoveEncodingKind::VarBytes => PreparedArrayRepr::VariableOffsets(
                VariableOffsets::varbytes(array.data, array.row_count)?,
            ),
            CoveEncodingKind::Canonical
                if matches!(
                    array.logical,
                    CoveLogicalType::Utf8 | CoveLogicalType::Binary | CoveLogicalType::Json
                ) =>
            {
                PreparedArrayRepr::VariableOffsets(VariableOffsets::canonical_length_prefixed(
                    array.data,
                    array.row_count,
                )?)
            }
            CoveEncodingKind::LocalCodebook
            | CoveEncodingKind::Rle
            | CoveEncodingKind::RunEnd
            | CoveEncodingKind::BitPacked
            | CoveEncodingKind::Delta
            | CoveEncodingKind::FrameOfReference
            | CoveEncodingKind::PatchedBase
            | CoveEncodingKind::Sparse => {
                PreparedArrayRepr::DecodedValues(array.decode_all_rows()?)
            }
            _ => PreparedArrayRepr::Direct,
        };
        Ok(Self { array, repr })
    }

    pub fn array(&self) -> &'a EncodedArray<'a> {
        self.array
    }

    pub fn decode_row(&self, row: u64) -> Result<CoveArrayValue<'a>, CoveError> {
        if row >= self.array.row_count {
            return Err(CoveError::OffsetRange);
        }
        match &self.repr {
            PreparedArrayRepr::Direct => self.array.decode_row(row),
            PreparedArrayRepr::DecodedValues(values) => values
                .get(row as usize)
                .cloned()
                .ok_or(CoveError::OffsetRange),
            PreparedArrayRepr::VariableOffsets(offsets) => {
                if self.array.is_null(row)? {
                    return Ok(CoveArrayValue::Null);
                }
                let offset = offsets.offset_for_row(row)?;
                match offsets.kind {
                    VariableOffsetKind::PlainVarint => {
                        if offset >= self.array.data.len() {
                            return Err(CoveError::OffsetRange);
                        }
                        let (value, _consumed) =
                            wire::decode_u64_leb128(&self.array.data[offset..])?;
                        Ok(CoveArrayValue::Varint(value))
                    }
                    VariableOffsetKind::VarBytes => {
                        let len = read_u32_le(self.array.data, offset)? as usize;
                        let data_start = offset.checked_add(4).ok_or(CoveError::ArithOverflow)?;
                        let slice = wire::read_range_checked(self.array.data, data_start, len)?;
                        Ok(CoveArrayValue::Bytes(slice))
                    }
                    VariableOffsetKind::CanonicalLengthPrefixed => {
                        let (len, consumed) = wire::decode_u64_leb128(&self.array.data[offset..])?;
                        let len = usize::try_from(len).map_err(|_| CoveError::ArithOverflow)?;
                        let data_start = offset
                            .checked_add(consumed)
                            .ok_or(CoveError::ArithOverflow)?;
                        let slice = wire::read_range_checked(self.array.data, data_start, len)?;
                        Ok(CoveArrayValue::Bytes(slice))
                    }
                }
            }
        }
    }

    pub fn decode_selected_rows(
        &self,
        selected_rows: &[u32],
    ) -> Result<Vec<CoveArrayValue<'a>>, CoveError> {
        let mut out = Vec::with_capacity(selected_rows.len());
        for row in selected_rows {
            out.push(self.decode_row(u64::from(*row))?);
        }
        Ok(out)
    }

    pub fn decode_all_rows(&self) -> Result<Vec<CoveArrayValue<'a>>, CoveError> {
        match &self.repr {
            PreparedArrayRepr::DecodedValues(values) => Ok(values.clone()),
            _ => {
                let mut out = Vec::with_capacity(expected_row_count(self.array.row_count)?);
                for row in 0..self.array.row_count {
                    out.push(self.decode_row(row)?);
                }
                Ok(out)
            }
        }
    }
}

/// A view over an encoded column array.
pub struct EncodedArray<'a> {
    /// Logical type of the column.
    pub logical: CoveLogicalType,
    /// Physical kind of the column.
    pub physical: CovePhysicalKind,
    /// Number of logical rows.
    pub row_count: u64,
    /// Encoding applied to the data buffer.
    pub encoding: CoveEncodingKind,
    /// Optional validity (null) bitmap.
    pub validity: Option<ValidityBitmap<'a>>,
    /// Raw encoded data bytes.
    pub data: &'a [u8],
    /// Optional file dictionary for FileCode resolution.
    pub dictionary: Option<&'a FileDictionary>,
}

impl<'a> EncodedArray<'a> {
    /// Constructs a new `EncodedArray`.
    pub fn new(
        logical: CoveLogicalType,
        physical: CovePhysicalKind,
        row_count: u64,
        encoding: CoveEncodingKind,
        validity: Option<ValidityBitmap<'a>>,
        data: &'a [u8],
        dictionary: Option<&'a FileDictionary>,
    ) -> Self {
        Self {
            logical,
            physical,
            row_count,
            encoding,
            validity,
            data,
            dictionary,
        }
    }

    /// Returns `true` if the given row is null.
    ///
    /// If no validity bitmap is present, all rows are non-null.
    ///
    /// # Errors
    ///
    /// Returns [`CoveError::OffsetRange`] if `row >= row_count`.
    pub fn is_null(&self, row: u64) -> Result<bool, CoveError> {
        if row >= self.row_count {
            return Err(CoveError::OffsetRange);
        }
        match &self.validity {
            Some(bm) => bm.is_null(row),
            None => Ok(false),
        }
    }

    /// Decodes the value at `row`.
    ///
    /// This is the one-off scalar access path. Repeated row reads from the
    /// same page should call [`EncodedArray::prepare`] once and then read
    /// through [`PreparedEncodedArray`].
    ///
    /// Returns [`CoveArrayValue::Null`] if the row is null.
    /// Returns [`CoveError::OffsetRange`] if `row >= row_count`.
    /// Returns [`CoveError::UnsupportedEncoding`] for transform/container encodings
    /// not representable as standalone row values.
    pub fn decode_row(&self, row: u64) -> Result<CoveArrayValue<'_>, CoveError> {
        if row >= self.row_count {
            return Err(CoveError::OffsetRange);
        }
        if self.is_null(row)? {
            return Ok(CoveArrayValue::Null);
        }
        self.decode_present_row(row)
    }

    /// Prepares page-scoped access for repeated row reads.
    pub fn prepare(&'a self) -> Result<PreparedEncodedArray<'a>, CoveError> {
        PreparedEncodedArray::new(self)
    }

    /// Decodes the full logical row set in row order.
    ///
    /// This provides a page-scoped path for row-wise consumers so transform
    /// encodings are parsed and decoded once per page instead of once per row.
    pub fn decode_all_rows(&self) -> Result<Vec<CoveArrayValue<'_>>, CoveError> {
        match self.encoding {
            CoveEncodingKind::LocalCodebook => {
                let payload = LocalCodebookPayload::parse(self.data)?;
                self.values_from_local_codebook_values(payload.decode_values()?)
            }
            CoveEncodingKind::Rle => {
                let payload = RlePayload::parse(self.data)?;
                self.values_from_i64_values(Rle::fast_decode(&payload)?)
            }
            CoveEncodingKind::RunEnd => {
                let payload = RunEndPayload::parse(self.data)?;
                self.values_from_i64_values(RunEnd::fast_decode(&payload)?)
            }
            CoveEncodingKind::BitPacked => {
                let payload = BitPackedPayload::parse(self.data)?;
                self.values_from_i64_values(BitPacked::fast_decode(&payload)?)
            }
            CoveEncodingKind::Delta => {
                let payload = DeltaPayload::parse(self.data)?;
                self.values_from_i64_values(Delta::fast_decode(&payload)?)
            }
            CoveEncodingKind::FrameOfReference => {
                let payload = ForPayload::parse(self.data)?;
                self.values_from_i64_values(FrameOfReference::fast_decode(&payload)?)
            }
            CoveEncodingKind::PatchedBase => {
                let payload = PatchedBasePayload::parse(self.data)?;
                self.values_from_i64_values(PatchedBase::fast_decode(&payload)?)
            }
            CoveEncodingKind::Sparse => {
                let payload = SparsePayload::parse(self.data)?;
                self.values_from_i64_values(Sparse::fast_decode(&payload)?)
            }
            CoveEncodingKind::PlainVarint => self.decode_all_plain_varint_rows(),
            CoveEncodingKind::VarBytes => self.decode_all_varbytes_rows(),
            CoveEncodingKind::Canonical => self.decode_all_canonical_rows(),
            _ => self.collect_rows(|row| self.decode_present_row(row)),
        }
    }

    fn decode_present_row(&self, row: u64) -> Result<CoveArrayValue<'_>, CoveError> {
        match self.encoding {
            CoveEncodingKind::Validity => {
                let byte_idx = (row / 8) as usize;
                let bit_idx = (row % 8) as u32;
                let byte = self
                    .data
                    .get(byte_idx)
                    .copied()
                    .ok_or(CoveError::OffsetRange)?;
                let bit = (byte >> bit_idx) & 1 == 1;
                Ok(CoveArrayValue::ValidityBit(bit))
            }
            CoveEncodingKind::Constant => {
                let w = logical_type_fixed_width(self.logical).ok_or_else(|| {
                    CoveError::UnsupportedEncoding(format!(
                        "Constant encoding requires fixed-width logical type, got {:?}",
                        self.logical
                    ))
                })?;
                let slice = wire::read_range_checked(self.data, 0, w)?;
                Ok(CoveArrayValue::Bytes(slice))
            }
            CoveEncodingKind::FileCode => {
                let code = read_u32_le(
                    self.data,
                    (row as usize)
                        .checked_mul(4)
                        .ok_or(CoveError::ArithOverflow)?,
                )?;
                match self.dictionary {
                    Some(dict) => {
                        let val = dict.decode_value(code)?;
                        Ok(CoveArrayValue::DictValue(val))
                    }
                    None => Ok(CoveArrayValue::FileCode(code)),
                }
            }
            CoveEncodingKind::NumCode => {
                let code = read_u64_le(
                    self.data,
                    (row as usize)
                        .checked_mul(8)
                        .ok_or(CoveError::ArithOverflow)?,
                )?;
                Ok(CoveArrayValue::NumCode(code))
            }
            CoveEncodingKind::PlainFixed => {
                let w = logical_type_fixed_width(self.logical).ok_or_else(|| {
                    CoveError::UnsupportedEncoding(format!(
                        "PlainFixed encoding requires fixed-width logical type, got {:?}",
                        self.logical
                    ))
                })?;
                let offset = (row as usize)
                    .checked_mul(w)
                    .ok_or(CoveError::ArithOverflow)?;
                let slice = wire::read_range_checked(self.data, offset, w)?;
                Ok(CoveArrayValue::Bytes(slice))
            }
            CoveEncodingKind::PlainVarint => {
                // O(n): variable-width encoding requires scanning all preceding rows.
                let mut pos = 0usize;
                for _ in 0..row {
                    if pos >= self.data.len() {
                        return Err(CoveError::OffsetRange);
                    }
                    let (_val, consumed) = wire::decode_u64_leb128(&self.data[pos..])?;
                    pos = pos.checked_add(consumed).ok_or(CoveError::ArithOverflow)?;
                }
                if pos >= self.data.len() {
                    return Err(CoveError::OffsetRange);
                }
                let (val, _consumed) = wire::decode_u64_leb128(&self.data[pos..])?;
                Ok(CoveArrayValue::Varint(val))
            }
            CoveEncodingKind::VarBytes => {
                // O(n): variable-width encoding requires scanning all preceding rows.
                let mut pos = 0usize;
                for _ in 0..row {
                    let len = read_u32_le(self.data, pos)? as usize;
                    pos = pos
                        .checked_add(4)
                        .and_then(|p| p.checked_add(len))
                        .ok_or(CoveError::ArithOverflow)?;
                    if pos > self.data.len() {
                        return Err(CoveError::OffsetRange);
                    }
                }
                let len = read_u32_le(self.data, pos)? as usize;
                let data_start = pos.checked_add(4).ok_or(CoveError::ArithOverflow)?;
                let slice = wire::read_range_checked(self.data, data_start, len)?;
                Ok(CoveArrayValue::Bytes(slice))
            }
            CoveEncodingKind::LocalCodebook => {
                let payload = LocalCodebookPayload::parse(self.data)?;
                let values = payload.decode_values()?;
                if values.len() != expected_row_count(self.row_count)? {
                    return Err(CoveError::PageCorrupt);
                }
                let value = values.get(row as usize).ok_or(CoveError::PageCorrupt)?;
                self.value_from_local_codebook(value)
            }
            CoveEncodingKind::Rle => {
                let payload = RlePayload::parse(self.data)?;
                self.value_from_i64_vec(row, Rle::fast_decode(&payload)?)
            }
            CoveEncodingKind::RunEnd => {
                let payload = RunEndPayload::parse(self.data)?;
                self.value_from_i64_vec(row, RunEnd::fast_decode(&payload)?)
            }
            CoveEncodingKind::BitPacked => {
                let payload = BitPackedPayload::parse(self.data)?;
                self.value_from_i64_vec(row, BitPacked::fast_decode(&payload)?)
            }
            CoveEncodingKind::Delta => {
                let payload = DeltaPayload::parse(self.data)?;
                self.value_from_i64_vec(row, Delta::fast_decode(&payload)?)
            }
            CoveEncodingKind::FrameOfReference => {
                let payload = ForPayload::parse(self.data)?;
                self.value_from_i64_vec(row, FrameOfReference::fast_decode(&payload)?)
            }
            CoveEncodingKind::PatchedBase => {
                let payload = PatchedBasePayload::parse(self.data)?;
                self.value_from_i64_vec(row, PatchedBase::fast_decode(&payload)?)
            }
            CoveEncodingKind::Sparse => {
                let payload = SparsePayload::parse(self.data)?;
                self.value_from_i64_vec(row, Sparse::fast_decode(&payload)?)
            }
            CoveEncodingKind::Canonical => self.decode_canonical_row(row),
            CoveEncodingKind::Sequence
            | CoveEncodingKind::Lz4Block
            | CoveEncodingKind::ZstdBlock => Err(CoveError::UnsupportedEncoding(format!(
                "{:?}",
                self.encoding
            ))),
        }
    }

    fn collect_rows<F>(&self, mut decode_row: F) -> Result<Vec<CoveArrayValue<'_>>, CoveError>
    where
        F: FnMut(u64) -> Result<CoveArrayValue<'a>, CoveError>,
    {
        let mut out = Vec::with_capacity(expected_row_count(self.row_count)?);
        for row in 0..self.row_count {
            if self.is_null(row)? {
                out.push(CoveArrayValue::Null);
            } else {
                out.push(decode_row(row)?);
            }
        }
        Ok(out)
    }

    fn values_from_i64_values(
        &self,
        values: Vec<i64>,
    ) -> Result<Vec<CoveArrayValue<'_>>, CoveError> {
        if values.len() != expected_row_count(self.row_count)? {
            return Err(CoveError::PageCorrupt);
        }
        let mut out = Vec::with_capacity(values.len());
        for (row, value) in values.into_iter().enumerate() {
            if self.is_null(row as u64)? {
                out.push(CoveArrayValue::Null);
            } else {
                out.push(self.value_from_i64(value)?);
            }
        }
        Ok(out)
    }

    fn values_from_local_codebook_values(
        &self,
        values: Vec<LocalCodebookValue>,
    ) -> Result<Vec<CoveArrayValue<'_>>, CoveError> {
        if values.len() != expected_row_count(self.row_count)? {
            return Err(CoveError::PageCorrupt);
        }
        let mut out = Vec::with_capacity(values.len());
        for (row, value) in values.iter().enumerate() {
            if self.is_null(row as u64)? {
                out.push(CoveArrayValue::Null);
            } else {
                out.push(self.value_from_local_codebook(value)?);
            }
        }
        Ok(out)
    }

    fn decode_all_plain_varint_rows(&self) -> Result<Vec<CoveArrayValue<'_>>, CoveError> {
        let mut out = Vec::with_capacity(expected_row_count(self.row_count)?);
        let mut pos = 0usize;
        for row in 0..self.row_count {
            if pos >= self.data.len() {
                return Err(CoveError::OffsetRange);
            }
            let (value, consumed) = wire::decode_u64_leb128(&self.data[pos..])?;
            pos = pos.checked_add(consumed).ok_or(CoveError::ArithOverflow)?;
            if self.is_null(row)? {
                out.push(CoveArrayValue::Null);
            } else {
                out.push(CoveArrayValue::Varint(value));
            }
        }
        Ok(out)
    }

    fn decode_all_varbytes_rows(&self) -> Result<Vec<CoveArrayValue<'_>>, CoveError> {
        let mut out = Vec::with_capacity(expected_row_count(self.row_count)?);
        let mut pos = 0usize;
        for row in 0..self.row_count {
            let len = read_u32_le(self.data, pos)? as usize;
            let data_start = pos.checked_add(4).ok_or(CoveError::ArithOverflow)?;
            let slice = wire::read_range_checked(self.data, data_start, len)?;
            pos = data_start
                .checked_add(len)
                .ok_or(CoveError::ArithOverflow)?;
            if self.is_null(row)? {
                out.push(CoveArrayValue::Null);
            } else {
                out.push(CoveArrayValue::Bytes(slice));
            }
        }
        Ok(out)
    }

    fn decode_all_canonical_rows(&self) -> Result<Vec<CoveArrayValue<'_>>, CoveError> {
        match self.logical {
            CoveLogicalType::Null => Ok(vec![
                CoveArrayValue::Null;
                expected_row_count(self.row_count)?
            ]),
            CoveLogicalType::Bool => self.collect_rows(|row| self.decode_present_row(row)),
            CoveLogicalType::Utf8 | CoveLogicalType::Binary | CoveLogicalType::Json => {
                self.decode_all_canonical_length_prefixed_rows()
            }
            logical => {
                let width = logical_type_fixed_width(logical).ok_or_else(|| {
                    CoveError::UnsupportedEncoding(format!(
                        "Canonical row decode unsupported for {:?}",
                        self.logical
                    ))
                })?;
                let mut out = Vec::with_capacity(expected_row_count(self.row_count)?);
                for row in 0..self.row_count {
                    let offset = (row as usize)
                        .checked_mul(width)
                        .ok_or(CoveError::ArithOverflow)?;
                    let slice = wire::read_range_checked(self.data, offset, width)?;
                    if self.is_null(row)? {
                        out.push(CoveArrayValue::Null);
                    } else {
                        out.push(CoveArrayValue::Bytes(slice));
                    }
                }
                Ok(out)
            }
        }
    }

    fn decode_all_canonical_length_prefixed_rows(
        &self,
    ) -> Result<Vec<CoveArrayValue<'_>>, CoveError> {
        let mut out = Vec::with_capacity(expected_row_count(self.row_count)?);
        let mut pos = 0usize;
        for row in 0..self.row_count {
            let (len, consumed) = wire::decode_u64_leb128(&self.data[pos..])?;
            let len = usize::try_from(len).map_err(|_| CoveError::ArithOverflow)?;
            let data_start = pos.checked_add(consumed).ok_or(CoveError::ArithOverflow)?;
            let slice = wire::read_range_checked(self.data, data_start, len)?;
            pos = data_start
                .checked_add(len)
                .ok_or(CoveError::ArithOverflow)?;
            if self.is_null(row)? {
                out.push(CoveArrayValue::Null);
            } else {
                out.push(CoveArrayValue::Bytes(slice));
            }
        }
        Ok(out)
    }

    fn value_from_i64_vec(
        &self,
        row: u64,
        values: Vec<i64>,
    ) -> Result<CoveArrayValue<'_>, CoveError> {
        if values.len() != expected_row_count(self.row_count)? {
            return Err(CoveError::PageCorrupt);
        }
        self.value_from_i64(*values.get(row as usize).ok_or(CoveError::PageCorrupt)?)
    }

    fn value_from_i64(&self, value: i64) -> Result<CoveArrayValue<'_>, CoveError> {
        match self.physical {
            CovePhysicalKind::FileCode => {
                let code = u32::try_from(value).map_err(|_| CoveError::PageCorrupt)?;
                match self.dictionary {
                    Some(dict) => Ok(CoveArrayValue::DictValue(dict.decode_value(code)?)),
                    None => Ok(CoveArrayValue::FileCode(code)),
                }
            }
            CovePhysicalKind::NumCode => {
                let code = u64::try_from(value).map_err(|_| CoveError::PageCorrupt)?;
                Ok(CoveArrayValue::NumCode(code))
            }
            CovePhysicalKind::Boolean => match value {
                0 => Ok(CoveArrayValue::Boolean(false)),
                1 => Ok(CoveArrayValue::Boolean(true)),
                _ => Err(CoveError::PageCorrupt),
            },
            _ => Ok(CoveArrayValue::Int64(value)),
        }
    }

    fn value_from_local_codebook(
        &self,
        value: &LocalCodebookValue,
    ) -> Result<CoveArrayValue<'_>, CoveError> {
        match value {
            LocalCodebookValue::FileCode(code) => match self.dictionary {
                Some(dict) => Ok(CoveArrayValue::DictValue(dict.decode_value(*code)?)),
                None => Ok(CoveArrayValue::FileCode(*code)),
            },
            LocalCodebookValue::NumCode(code) => Ok(CoveArrayValue::NumCode(*code)),
            LocalCodebookValue::Boolean(value) => Ok(CoveArrayValue::Boolean(*value)),
            LocalCodebookValue::VarBytes(value) => Ok(CoveArrayValue::OwnedBytes(value.clone())),
        }
    }

    fn decode_canonical_row(&self, row: u64) -> Result<CoveArrayValue<'_>, CoveError> {
        match self.logical {
            CoveLogicalType::Null => Ok(CoveArrayValue::Null),
            CoveLogicalType::Bool => Err(CoveError::UnsupportedEncoding(
                "Canonical Bool rows are tag-only and need an explicit value-tag stream".into(),
            )),
            CoveLogicalType::Utf8 | CoveLogicalType::Binary | CoveLogicalType::Json => {
                self.decode_canonical_length_prefixed_row(row)
            }
            logical => {
                let width = logical_type_fixed_width(logical).ok_or_else(|| {
                    CoveError::UnsupportedEncoding(format!(
                        "Canonical row decode unsupported for {:?}",
                        self.logical
                    ))
                })?;
                let offset = (row as usize)
                    .checked_mul(width)
                    .ok_or(CoveError::ArithOverflow)?;
                let slice = wire::read_range_checked(self.data, offset, width)?;
                Ok(CoveArrayValue::Bytes(slice))
            }
        }
    }

    fn decode_canonical_length_prefixed_row(
        &self,
        row: u64,
    ) -> Result<CoveArrayValue<'_>, CoveError> {
        let mut pos = 0usize;
        for _ in 0..row {
            let (len, consumed) = wire::decode_u64_leb128(&self.data[pos..])?;
            let len = usize::try_from(len).map_err(|_| CoveError::ArithOverflow)?;
            pos = pos
                .checked_add(consumed)
                .and_then(|offset| offset.checked_add(len))
                .ok_or(CoveError::ArithOverflow)?;
            if pos > self.data.len() {
                return Err(CoveError::OffsetRange);
            }
        }
        let (len, consumed) = wire::decode_u64_leb128(&self.data[pos..])?;
        let len = usize::try_from(len).map_err(|_| CoveError::ArithOverflow)?;
        let data_start = pos.checked_add(consumed).ok_or(CoveError::ArithOverflow)?;
        let slice = wire::read_range_checked(self.data, data_start, len)?;
        Ok(CoveArrayValue::Bytes(slice))
    }
}

/// Returns the fixed byte width for a logical type, or `None` for variable-width types.
pub fn logical_type_fixed_width(logical: CoveLogicalType) -> Option<usize> {
    match logical {
        CoveLogicalType::Bool | CoveLogicalType::Int8 | CoveLogicalType::UInt8 => Some(1),
        CoveLogicalType::Int16 | CoveLogicalType::UInt16 => Some(2),
        CoveLogicalType::Int32
        | CoveLogicalType::UInt32
        | CoveLogicalType::Float32
        | CoveLogicalType::DateDays => Some(4),
        CoveLogicalType::Int64
        | CoveLogicalType::UInt64
        | CoveLogicalType::Float64
        | CoveLogicalType::Decimal64
        | CoveLogicalType::TimestampMicros
        | CoveLogicalType::TimestampNanos => Some(8),
        CoveLogicalType::Decimal128 | CoveLogicalType::Uuid => Some(16),
        _ => None,
    }
}

/// Reads a little-endian `u32` from `bytes` at `offset`.
fn read_u32_le(bytes: &[u8], offset: usize) -> Result<u32, CoveError> {
    let slice = wire::read_range_checked(bytes, offset, 4)?;
    Ok(u32::from_le_bytes(slice.try_into().unwrap()))
}

/// Reads a little-endian `u64` from `bytes` at `offset`.
fn read_u64_le(bytes: &[u8], offset: usize) -> Result<u64, CoveError> {
    let slice = wire::read_range_checked(bytes, offset, 8)?;
    Ok(u64::from_le_bytes(slice.try_into().unwrap()))
}

fn expected_row_count(row_count: u64) -> Result<usize, CoveError> {
    usize::try_from(row_count).map_err(|_| CoveError::ArithOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        constants::{CoveEncodingKind, CoveLogicalType, CovePhysicalKind},
        dictionary::{FileDictionary, FileDictionaryHeaderV1, FileDictionaryIndexEntryV1},
        encoding::{
            bit_packed::BitPackedPayload,
            local_codebook::{LocalCodebookPayload, LocalCodebookValues, LocalIndexPayload},
            rle::RlePayload,
        },
        validity::ValidityBitmapBuilder,
    };

    fn make_array<'a>(
        logical: CoveLogicalType,
        encoding: CoveEncodingKind,
        row_count: u64,
        data: &'a [u8],
        validity: Option<ValidityBitmap<'a>>,
    ) -> EncodedArray<'a> {
        EncodedArray::new(
            logical,
            CovePhysicalKind::FixedBytes,
            row_count,
            encoding,
            validity,
            data,
            None,
        )
    }

    fn make_physical_array<'a>(
        logical: CoveLogicalType,
        physical: CovePhysicalKind,
        encoding: CoveEncodingKind,
        row_count: u64,
        data: &'a [u8],
    ) -> EncodedArray<'a> {
        EncodedArray::new(logical, physical, row_count, encoding, None, data, None)
    }

    #[test]
    fn filecode_zero_decodes_as_raw_filecode_without_dictionary() {
        // FileCode 0 stored as LE u32 = [0,0,0,0]
        let data = 0u32.to_le_bytes();
        let arr = EncodedArray::new(
            CoveLogicalType::UInt64,
            CovePhysicalKind::FileCode,
            1,
            CoveEncodingKind::FileCode,
            None,
            &data,
            None,
        );
        assert_eq!(arr.decode_row(0).unwrap(), CoveArrayValue::FileCode(0));
    }

    #[test]
    fn numcode_zero_decodes_as_ordinary_value() {
        let data = 0u64.to_le_bytes();
        let arr = EncodedArray::new(
            CoveLogicalType::UInt64,
            CovePhysicalKind::NumCode,
            1,
            CoveEncodingKind::NumCode,
            None,
            &data,
            None,
        );
        assert_eq!(arr.decode_row(0).unwrap(), CoveArrayValue::NumCode(0));
    }

    #[test]
    fn null_row_returns_null() {
        let data = 42i32.to_le_bytes();
        let mut builder = ValidityBitmapBuilder::new(1).unwrap();
        builder.set_null(0).unwrap();
        let bitmap_bytes = builder.into_bytes();
        let bm = ValidityBitmap::new(&bitmap_bytes, 1);
        let arr = EncodedArray::new(
            CoveLogicalType::Int32,
            CovePhysicalKind::FixedBytes,
            1,
            CoveEncodingKind::PlainFixed,
            Some(bm),
            &data,
            None,
        );
        assert_eq!(arr.decode_row(0).unwrap(), CoveArrayValue::Null);
    }

    #[test]
    fn out_of_range_row_returns_error() {
        let data = 0i32.to_le_bytes();
        let arr = make_array(
            CoveLogicalType::Int32,
            CoveEncodingKind::PlainFixed,
            1,
            &data,
            None,
        );
        assert_eq!(arr.decode_row(1), Err(CoveError::OffsetRange));
    }

    #[test]
    fn plain_fixed_i32_roundtrip() {
        let v0: i32 = -42;
        let v1: i32 = 1337;
        let mut data = Vec::new();
        data.extend_from_slice(&v0.to_le_bytes());
        data.extend_from_slice(&v1.to_le_bytes());
        let arr = make_array(
            CoveLogicalType::Int32,
            CoveEncodingKind::PlainFixed,
            2,
            &data,
            None,
        );
        assert_eq!(
            arr.decode_row(0).unwrap(),
            CoveArrayValue::Bytes(&v0.to_le_bytes())
        );
        assert_eq!(
            arr.decode_row(1).unwrap(),
            CoveArrayValue::Bytes(&v1.to_le_bytes())
        );
    }

    #[test]
    fn varbytes_decodes_byte_slices() {
        let a = b"hello";
        let b = b"world!!";
        let mut data = Vec::new();
        data.extend_from_slice(&(a.len() as u32).to_le_bytes());
        data.extend_from_slice(a);
        data.extend_from_slice(&(b.len() as u32).to_le_bytes());
        data.extend_from_slice(b);
        let arr = make_array(
            CoveLogicalType::Binary,
            CoveEncodingKind::VarBytes,
            2,
            &data,
            None,
        );
        assert_eq!(
            arr.decode_row(0).unwrap(),
            CoveArrayValue::Bytes(a.as_ref())
        );
        assert_eq!(
            arr.decode_row(1).unwrap(),
            CoveArrayValue::Bytes(b.as_ref())
        );
    }

    #[test]
    fn constant_returns_same_value_for_all_rows() {
        let val: i32 = 99;
        let data = val.to_le_bytes();
        let arr = make_array(
            CoveLogicalType::Int32,
            CoveEncodingKind::Constant,
            5,
            &data,
            None,
        );
        for row in 0..5 {
            assert_eq!(
                arr.decode_row(row).unwrap(),
                CoveArrayValue::Bytes(&val.to_le_bytes())
            );
        }
    }

    #[test]
    fn unsupported_encoding_returns_error() {
        let data = [0u8; 1];
        let arr = make_array(
            CoveLogicalType::Int32,
            CoveEncodingKind::Sequence,
            1,
            &data,
            None,
        );
        assert!(matches!(
            arr.decode_row(0),
            Err(CoveError::UnsupportedEncoding(_))
        ));
        let arr2 = make_array(
            CoveLogicalType::Int32,
            CoveEncodingKind::Lz4Block,
            1,
            &data,
            None,
        );
        assert!(matches!(
            arr2.decode_row(0),
            Err(CoveError::UnsupportedEncoding(_))
        ));
    }

    #[test]
    fn rle_rows_decode_as_signed_values() {
        let payload = RlePayload {
            runs: vec![(-2, 2), (9, 1)],
        };
        let data = payload.encode();
        let arr = make_physical_array(
            CoveLogicalType::Int64,
            CovePhysicalKind::NumCode,
            CoveEncodingKind::Rle,
            3,
            &data,
        );
        assert_eq!(arr.decode_row(0), Err(CoveError::PageCorrupt));

        let arr = make_array(
            CoveLogicalType::Int64,
            CoveEncodingKind::Rle,
            3,
            &data,
            None,
        );
        assert_eq!(arr.decode_row(0).unwrap(), CoveArrayValue::Int64(-2));
        assert_eq!(arr.decode_row(2).unwrap(), CoveArrayValue::Int64(9));
    }

    #[test]
    fn bitpacked_rows_decode_as_numcodes() {
        let payload = BitPackedPayload::pack(&[3, 1, 7, 0], 3).unwrap();
        let data = payload.encode();
        let arr = make_physical_array(
            CoveLogicalType::UInt64,
            CovePhysicalKind::NumCode,
            CoveEncodingKind::BitPacked,
            4,
            &data,
        );
        assert_eq!(arr.decode_row(0).unwrap(), CoveArrayValue::NumCode(3));
        assert_eq!(arr.decode_row(2).unwrap(), CoveArrayValue::NumCode(7));
    }

    #[test]
    fn local_codebook_decodes_typed_values() {
        let indexes = LocalIndexPayload::Rle(RlePayload {
            runs: vec![(0, 1), (1, 2)],
        });
        let payload = LocalCodebookPayload {
            values: LocalCodebookValues::VarBytes(vec![b"red".to_vec(), b"blue".to_vec()]),
            indexes,
        };
        let data = payload.encode();
        let arr = make_physical_array(
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            CoveEncodingKind::LocalCodebook,
            3,
            &data,
        );
        assert_eq!(
            arr.decode_row(0).unwrap(),
            CoveArrayValue::OwnedBytes(b"red".to_vec())
        );
        assert_eq!(
            arr.decode_row(2).unwrap(),
            CoveArrayValue::OwnedBytes(b"blue".to_vec())
        );
    }

    #[test]
    fn canonical_decodes_fixed_and_length_prefixed_rows() {
        let first = 42i32.to_le_bytes();
        let second = (-7i32).to_le_bytes();
        let mut fixed = Vec::new();
        fixed.extend_from_slice(&first);
        fixed.extend_from_slice(&second);
        let arr = make_array(
            CoveLogicalType::Int32,
            CoveEncodingKind::Canonical,
            2,
            &fixed,
            None,
        );
        assert_eq!(arr.decode_row(0).unwrap(), CoveArrayValue::Bytes(&first));
        assert_eq!(arr.decode_row(1).unwrap(), CoveArrayValue::Bytes(&second));

        let mut var = Vec::new();
        var.extend_from_slice(&wire::encode_u64_leb128(2));
        var.extend_from_slice(b"hi");
        var.extend_from_slice(&wire::encode_u64_leb128(5));
        var.extend_from_slice(b"there");
        let arr = make_array(
            CoveLogicalType::Utf8,
            CoveEncodingKind::Canonical,
            2,
            &var,
            None,
        );
        assert_eq!(arr.decode_row(0).unwrap(), CoveArrayValue::Bytes(b"hi"));
        assert_eq!(arr.decode_row(1).unwrap(), CoveArrayValue::Bytes(b"there"));
    }

    #[test]
    fn bulk_decode_rows_handles_transform_encodings_once_per_page() {
        let payload = RlePayload {
            runs: vec![(-2, 2), (9, 1)],
        };
        let data = payload.encode();
        let arr = make_array(
            CoveLogicalType::Int64,
            CoveEncodingKind::Rle,
            3,
            &data,
            None,
        );

        assert_eq!(
            arr.decode_all_rows().unwrap(),
            vec![
                CoveArrayValue::Int64(-2),
                CoveArrayValue::Int64(-2),
                CoveArrayValue::Int64(9),
            ]
        );
    }

    #[test]
    fn bulk_decode_rows_preserves_local_codebook_values() {
        let payload = LocalCodebookPayload {
            values: LocalCodebookValues::VarBytes(vec![b"red".to_vec(), b"blue".to_vec()]),
            indexes: LocalIndexPayload::Rle(RlePayload {
                runs: vec![(0, 1), (1, 2)],
            }),
        };
        let data = payload.encode();
        let arr = make_physical_array(
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            CoveEncodingKind::LocalCodebook,
            3,
            &data,
        );

        assert_eq!(
            arr.decode_all_rows().unwrap(),
            vec![
                CoveArrayValue::OwnedBytes(b"red".to_vec()),
                CoveArrayValue::OwnedBytes(b"blue".to_vec()),
                CoveArrayValue::OwnedBytes(b"blue".to_vec()),
            ]
        );
    }

    #[test]
    fn prepared_access_matches_scalar_and_bulk_varint_varbytes_and_canonical() {
        let varint_bytes = [
            wire::encode_u64_leb128(3),
            wire::encode_u64_leb128(17),
            wire::encode_u64_leb128(255),
        ]
        .concat();
        let varint = make_array(
            CoveLogicalType::UInt64,
            CoveEncodingKind::PlainVarint,
            3,
            &varint_bytes,
            None,
        );
        let prepared = varint.prepare().unwrap();
        assert_eq!(
            prepared.decode_row(2).unwrap(),
            varint.decode_row(2).unwrap()
        );
        assert_eq!(
            prepared.decode_selected_rows(&[2, 0]).unwrap(),
            vec![CoveArrayValue::Varint(255), CoveArrayValue::Varint(3)]
        );
        assert_eq!(
            prepared.decode_all_rows().unwrap(),
            varint.decode_all_rows().unwrap()
        );

        let mut varbytes_bytes = Vec::new();
        varbytes_bytes.extend_from_slice(&2u32.to_le_bytes());
        varbytes_bytes.extend_from_slice(b"hi");
        varbytes_bytes.extend_from_slice(&5u32.to_le_bytes());
        varbytes_bytes.extend_from_slice(b"there");
        varbytes_bytes.extend_from_slice(&3u32.to_le_bytes());
        varbytes_bytes.extend_from_slice(b"bye");
        let varbytes = make_array(
            CoveLogicalType::Utf8,
            CoveEncodingKind::VarBytes,
            3,
            &varbytes_bytes,
            None,
        );
        let prepared = varbytes.prepare().unwrap();
        assert_eq!(
            prepared.decode_row(1).unwrap(),
            varbytes.decode_row(1).unwrap()
        );
        assert_eq!(
            prepared.decode_selected_rows(&[1, 2]).unwrap(),
            vec![
                CoveArrayValue::Bytes(b"there"),
                CoveArrayValue::Bytes(b"bye")
            ]
        );
        assert_eq!(
            prepared.decode_all_rows().unwrap(),
            varbytes.decode_all_rows().unwrap()
        );

        let mut canonical_bytes = Vec::new();
        canonical_bytes.extend_from_slice(&wire::encode_u64_leb128(2));
        canonical_bytes.extend_from_slice(b"hi");
        canonical_bytes.extend_from_slice(&wire::encode_u64_leb128(5));
        canonical_bytes.extend_from_slice(b"there");
        let canonical = make_array(
            CoveLogicalType::Utf8,
            CoveEncodingKind::Canonical,
            2,
            &canonical_bytes,
            None,
        );
        let prepared = canonical.prepare().unwrap();
        assert_eq!(
            prepared.decode_row(1).unwrap(),
            canonical.decode_row(1).unwrap()
        );
        assert_eq!(
            prepared.decode_selected_rows(&[1]).unwrap(),
            vec![CoveArrayValue::Bytes(b"there")]
        );
        assert_eq!(
            prepared.decode_all_rows().unwrap(),
            canonical.decode_all_rows().unwrap()
        );
    }

    #[test]
    fn prepared_access_matches_bulk_transform_decodes() {
        let rle_payload = RlePayload {
            runs: vec![(-2, 2), (9, 1)],
        };
        let rle_bytes = rle_payload.encode();
        let rle = make_array(
            CoveLogicalType::Int64,
            CoveEncodingKind::Rle,
            3,
            &rle_bytes,
            None,
        );
        let prepared = rle.prepare().unwrap();
        assert_eq!(prepared.decode_row(2).unwrap(), CoveArrayValue::Int64(9));
        assert_eq!(
            prepared.decode_all_rows().unwrap(),
            rle.decode_all_rows().unwrap()
        );

        let payload = LocalCodebookPayload {
            values: LocalCodebookValues::VarBytes(vec![b"red".to_vec(), b"blue".to_vec()]),
            indexes: LocalIndexPayload::Rle(RlePayload {
                runs: vec![(0, 1), (1, 2)],
            }),
        };
        let data = payload.encode();
        let local_codebook = make_physical_array(
            CoveLogicalType::Utf8,
            CovePhysicalKind::VarBytes,
            CoveEncodingKind::LocalCodebook,
            3,
            &data,
        );
        let prepared = local_codebook.prepare().unwrap();
        assert_eq!(
            prepared.decode_selected_rows(&[2, 0]).unwrap(),
            vec![
                CoveArrayValue::OwnedBytes(b"blue".to_vec()),
                CoveArrayValue::OwnedBytes(b"red".to_vec())
            ]
        );
        assert_eq!(
            prepared.decode_all_rows().unwrap(),
            local_codebook.decode_all_rows().unwrap()
        );
    }

    #[test]
    fn filecode_out_of_range_fails() {
        // Build a minimal dictionary with 1 entry (FileCode 0 only).
        // Then try to decode FileCode 1 — should fail with BadFileCode.
        let entry = FileDictionaryIndexEntryV1 {
            value_tag: 12,    // Utf8
            storage_class: 0, // Inline
            flags: 0,
            inline_len: 4,
            reserved0: [0; 3],
            inline_data: {
                let mut d = [0u8; 16];
                // Canonical UTF-8 encoding: varint length prefix + bytes.
                d[..4].copy_from_slice(&[0x03, b'f', b'o', b'o']);
                d
            },
            payload_offset: 0,
            payload_length: 0,
            canonical_hash64: 0,
            reserved1: 0,
        };
        let header = FileDictionaryHeaderV1 {
            entry_count: 1,
            flags: 0,
            index_entry_len: FileDictionaryHeaderV1::INDEX_ENTRY_LEN,
            value_hash_algorithm: 0,
            payload_length: 0,
            reserved: [0; 24],
        };
        let mut index_bytes = header.serialize().to_vec();
        index_bytes.extend_from_slice(&entry.serialize());
        let dict = FileDictionary::parse(&index_bytes, &[]).unwrap();

        // Two rows: [0, 1] as u32 LE
        let mut data = Vec::new();
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());

        let arr = EncodedArray::new(
            CoveLogicalType::Utf8,
            CovePhysicalKind::FileCode,
            2,
            CoveEncodingKind::FileCode,
            None,
            &data,
            Some(&dict),
        );
        // FileCode 0 is valid
        assert!(arr.decode_row(0).is_ok());
        // FileCode 1 is out of range
        assert_eq!(arr.decode_row(1), Err(CoveError::BadFileCode));
    }
}
