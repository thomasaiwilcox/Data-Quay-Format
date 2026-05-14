//! Spec §34 — Aggregate synopsis.
//!
//! Aggregate synopsis sections are optional acceleration metadata. Count-only
//! entries remain payload-free for backwards compatibility; all other synopsis
//! kinds use a checksummed COVE-owned payload after the entry table.

use std::cmp::Ordering;

use crate::{canonical::validate_canonical_payload, checksum, constants::ValueTag, CoveError};

use super::{checked_region, verify_checksum_field};

pub const AGGREGATE_SYNOPSIS_ENTRY_LEN: usize = 48;
pub const AGGREGATE_PAYLOAD_HEADER_LEN: usize = 28;
pub const DEFAULT_TOPK_K: u32 = 64;
pub const DEFAULT_HLL_PRECISION: u8 = 14;
pub const DEFAULT_KLL_K: u32 = 200;

const AGGREGATE_PAYLOAD_MAGIC: [u8; 4] = *b"AGS2";
const AGGREGATE_PAYLOAD_VERSION: u8 = 1;
const ABSENT_VALUE_TAG: u16 = u16::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
#[non_exhaustive]
pub enum SynopsisKind {
    Count,
    MinMax,
    Sum,
    SumAndCount,
    BoolTrueFalseCounts,
    FileCodeHistogram,
    NumCodeHistogram,
    DistinctSketch,
    QuantileSketch,
    TopK,
}

impl SynopsisKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Count),
            1 => Some(Self::MinMax),
            2 => Some(Self::Sum),
            3 => Some(Self::SumAndCount),
            4 => Some(Self::BoolTrueFalseCounts),
            5 => Some(Self::FileCodeHistogram),
            6 => Some(Self::NumCodeHistogram),
            7 => Some(Self::DistinctSketch),
            8 => Some(Self::QuantileSketch),
            9 => Some(Self::TopK),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
#[non_exhaustive]
pub enum SynopsisAccuracy {
    Exact = 0,
    Approximate = 1,
}

impl SynopsisAccuracy {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Exact),
            1 => Some(Self::Approximate),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
#[non_exhaustive]
pub enum NumericAggregateOverflowPolicy {
    CheckedExact = 0,
    Saturating = 1,
    Wrapping = 2,
    DecimalWidened = 3,
}

impl NumericAggregateOverflowPolicy {
    pub fn from_u32(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::CheckedExact),
            1 => Some(Self::Saturating),
            2 => Some(Self::Wrapping),
            3 => Some(Self::DecimalWidened),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaggedCanonicalValue {
    pub value_tag: ValueTag,
    pub payload: Vec<u8>,
}

impl TaggedCanonicalValue {
    pub fn new(value_tag: ValueTag, payload: Vec<u8>) -> Result<Self, CoveError> {
        validate_canonical_payload(value_tag, &payload)?;
        Ok(Self { value_tag, payload })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistogramBucket {
    pub key: u64,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AggregatePayloadHeader {
    pub kind: SynopsisKind,
    pub flags: u16,
    pub item_count: u32,
    pub aux0: u32,
    pub aux1: u32,
    pub data_len: u32,
    pub checksum: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AggregatePayloadV2 {
    None,
    MinMax {
        min: Option<TaggedCanonicalValue>,
        max: Option<TaggedCanonicalValue>,
    },
    Sum {
        overflow_policy: NumericAggregateOverflowPolicy,
        sum: TaggedCanonicalValue,
    },
    SumAndCount {
        overflow_policy: NumericAggregateOverflowPolicy,
        non_null_count: u64,
        sum: TaggedCanonicalValue,
    },
    BoolTrueFalseCounts {
        true_count: u64,
        false_count: u64,
    },
    FileCodeHistogram {
        buckets: Vec<HistogramBucket>,
    },
    NumCodeHistogram {
        buckets: Vec<HistogramBucket>,
    },
    DistinctSketch {
        precision: u8,
        registers: Vec<u8>,
    },
    QuantileSketch {
        k: u32,
        value_tag: ValueTag,
        level_offsets: Vec<u32>,
        values: Vec<Vec<u8>>,
    },
    TopK {
        k: u32,
        entries: Vec<HistogramBucket>,
    },
}

impl Default for AggregatePayloadV2 {
    fn default() -> Self {
        Self::None
    }
}

impl AggregatePayloadV2 {
    pub fn min_max(min: Option<TaggedCanonicalValue>, max: Option<TaggedCanonicalValue>) -> Self {
        Self::MinMax { min, max }
    }

    pub fn sum(overflow_policy: NumericAggregateOverflowPolicy, sum: TaggedCanonicalValue) -> Self {
        Self::Sum {
            overflow_policy,
            sum,
        }
    }

    pub fn checked_sum(sum: TaggedCanonicalValue) -> Self {
        Self::sum(NumericAggregateOverflowPolicy::CheckedExact, sum)
    }

    pub fn sum_and_count(
        overflow_policy: NumericAggregateOverflowPolicy,
        non_null_count: u64,
        sum: TaggedCanonicalValue,
    ) -> Self {
        Self::SumAndCount {
            overflow_policy,
            non_null_count,
            sum,
        }
    }

    pub fn checked_sum_and_count(non_null_count: u64, sum: TaggedCanonicalValue) -> Self {
        Self::sum_and_count(
            NumericAggregateOverflowPolicy::CheckedExact,
            non_null_count,
            sum,
        )
    }

    pub fn bool_true_false_counts(true_count: u64, false_count: u64) -> Self {
        Self::BoolTrueFalseCounts {
            true_count,
            false_count,
        }
    }

    pub fn filecode_histogram(buckets: Vec<HistogramBucket>) -> Self {
        Self::FileCodeHistogram { buckets }
    }

    pub fn numcode_histogram(buckets: Vec<HistogramBucket>) -> Self {
        Self::NumCodeHistogram { buckets }
    }

    pub fn topk(k: u32, entries: Vec<HistogramBucket>) -> Self {
        Self::TopK { k, entries }
    }

    pub fn distinct_sketch(precision: u8, registers: Vec<u8>) -> Self {
        Self::DistinctSketch {
            precision,
            registers,
        }
    }

    pub fn distinct_sketch_from_hashes(
        precision: u8,
        hashes: impl IntoIterator<Item = u64>,
    ) -> Result<Self, CoveError> {
        Ok(Self::distinct_sketch(
            precision,
            hll_registers_from_hashes(precision, hashes)?,
        ))
    }

    pub fn quantile_sketch(
        k: u32,
        value_tag: ValueTag,
        level_offsets: Vec<u32>,
        values: Vec<Vec<u8>>,
    ) -> Self {
        Self::QuantileSketch {
            k,
            value_tag,
            level_offsets,
            values,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AggregateEntry {
    pub table_id: u32,
    pub segment_id: u32,
    pub morsel_id: u32,
    pub column_id: u32,
    pub synopsis_kind: SynopsisKind,
    pub key_kind: u8,
    pub accuracy: SynopsisAccuracy,
    pub flags: u8,
    pub row_count: u32,
    pub null_count: u32,
    pub payload_offset: u64,
    pub payload_length: u64,
    pub checksum: u32,
}

impl AggregateEntry {
    pub fn new(
        table_id: u32,
        segment_id: u32,
        morsel_id: u32,
        column_id: u32,
        synopsis_kind: SynopsisKind,
        key_kind: u8,
        accuracy: SynopsisAccuracy,
        row_count: u32,
        null_count: u32,
    ) -> Result<Self, CoveError> {
        if null_count > row_count {
            return Err(CoveError::BadIndex);
        }
        Ok(Self {
            table_id,
            segment_id,
            morsel_id,
            column_id,
            synopsis_kind,
            key_kind,
            accuracy,
            flags: 0,
            row_count,
            null_count,
            payload_offset: 0,
            payload_length: 0,
            checksum: 0,
        })
    }

    pub fn serialize(&self) -> [u8; AGGREGATE_SYNOPSIS_ENTRY_LEN] {
        let mut out = [0u8; AGGREGATE_SYNOPSIS_ENTRY_LEN];
        out[0..4].copy_from_slice(&self.table_id.to_le_bytes());
        out[4..8].copy_from_slice(&self.segment_id.to_le_bytes());
        out[8..12].copy_from_slice(&self.morsel_id.to_le_bytes());
        out[12..16].copy_from_slice(&self.column_id.to_le_bytes());
        out[16] = self.synopsis_kind as u8;
        out[17] = self.key_kind;
        out[18] = self.accuracy as u8;
        out[19] = self.flags;
        out[20..24].copy_from_slice(&self.row_count.to_le_bytes());
        out[24..28].copy_from_slice(&self.null_count.to_le_bytes());
        out[28..36].copy_from_slice(&self.payload_offset.to_le_bytes());
        out[36..44].copy_from_slice(&self.payload_length.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[44..48].copy_from_slice(&crc.to_le_bytes());
        out
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < AGGREGATE_SYNOPSIS_ENTRY_LEN {
            return Err(CoveError::BufferTooShort);
        }
        let bytes = &bytes[..AGGREGATE_SYNOPSIS_ENTRY_LEN];
        let checksum = verify_checksum_field(bytes, 44)?;
        let synopsis_kind = SynopsisKind::from_u8(bytes[16]).ok_or(CoveError::BadIndex)?;
        let accuracy = SynopsisAccuracy::from_u8(bytes[18]).ok_or(CoveError::BadIndex)?;
        let row_count = u32::from_le_bytes(bytes[20..24].try_into().unwrap());
        let null_count = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
        if null_count > row_count {
            return Err(CoveError::BadIndex);
        }
        Ok(Self {
            table_id: u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
            segment_id: u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            morsel_id: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
            column_id: u32::from_le_bytes(bytes[12..16].try_into().unwrap()),
            synopsis_kind,
            key_kind: bytes[17],
            accuracy,
            flags: bytes[19],
            row_count,
            null_count,
            payload_offset: u64::from_le_bytes(bytes[28..36].try_into().unwrap()),
            payload_length: u64::from_le_bytes(bytes[36..44].try_into().unwrap()),
            checksum,
        })
    }

    pub fn non_null_count(&self) -> Result<u64, CoveError> {
        u64::from(self.row_count)
            .checked_sub(u64::from(self.null_count))
            .ok_or(CoveError::ArithOverflow)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AggregateSynopsis {
    pub entries: Vec<AggregateEntry>,
    pub payloads: Vec<AggregatePayloadV2>,
}

impl AggregateSynopsis {
    pub fn from_entries(entries: Vec<AggregateEntry>) -> Self {
        Self {
            entries,
            payloads: Vec::new(),
        }
    }

    pub fn from_parts(
        entries: Vec<AggregateEntry>,
        payloads: Vec<AggregatePayloadV2>,
    ) -> Result<Self, CoveError> {
        let synopsis = Self { entries, payloads };
        synopsis.validate()?;
        Ok(synopsis)
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < AGGREGATE_SYNOPSIS_ENTRY_LEN {
            return Err(CoveError::BufferTooShort);
        }
        let entry_count = infer_entry_count(bytes)?;
        let mut entries = Vec::with_capacity(entry_count);
        for index in 0..entry_count {
            let start = index
                .checked_mul(AGGREGATE_SYNOPSIS_ENTRY_LEN)
                .ok_or(CoveError::ArithOverflow)?;
            entries.push(AggregateEntry::parse(
                &bytes[start..start + AGGREGATE_SYNOPSIS_ENTRY_LEN],
            )?);
        }

        let table_len = entry_count
            .checked_mul(AGGREGATE_SYNOPSIS_ENTRY_LEN)
            .ok_or(CoveError::ArithOverflow)?;
        let mut payloads = Vec::with_capacity(entries.len());
        for entry in &entries {
            if entry.payload_length == 0 {
                payloads.push(AggregatePayloadV2::None);
                continue;
            }
            let start = usize::try_from(entry.payload_offset).map_err(aggregate_payload_error)?;
            let len = usize::try_from(entry.payload_length).map_err(aggregate_payload_error)?;
            if start < table_len {
                return Err(CoveError::BadIndex);
            }
            checked_region(bytes, entry.payload_offset, entry.payload_length)
                .map_err(aggregate_payload_error)?;
            payloads.push(
                parse_payload(entry, &bytes[start..start + len])
                    .map_err(aggregate_payload_error)?,
            );
        }

        let synopsis = Self { entries, payloads };
        synopsis.validate()?;
        Ok(synopsis)
    }

    /// Inverse of [`Self::parse`]; produces canonical bytes that round-trip.
    pub fn serialize(&self) -> Vec<u8> {
        if self.payloads.is_empty() {
            let mut out = Vec::with_capacity(self.entries.len() * AGGREGATE_SYNOPSIS_ENTRY_LEN);
            for entry in &self.entries {
                out.extend_from_slice(&entry.serialize());
            }
            return out;
        }

        let table_len = self.entries.len() * AGGREGATE_SYNOPSIS_ENTRY_LEN;
        let mut entry_bytes = Vec::with_capacity(table_len);
        let mut payload_bytes = Vec::new();
        for (entry, payload) in self.entries.iter().zip(&self.payloads) {
            let encoded_payload = encode_payload(entry, payload);
            let mut canonical_entry = entry.clone();
            if encoded_payload.is_empty() {
                canonical_entry.payload_offset = 0;
                canonical_entry.payload_length = 0;
            } else {
                canonical_entry.payload_offset = (table_len + payload_bytes.len()) as u64;
                canonical_entry.payload_length = encoded_payload.len() as u64;
                payload_bytes.extend_from_slice(&encoded_payload);
            }
            entry_bytes.extend_from_slice(&canonical_entry.serialize());
        }
        entry_bytes.extend_from_slice(&payload_bytes);
        entry_bytes
    }

    pub fn payload_for_entry(&self, entry_index: usize) -> Option<&AggregatePayloadV2> {
        self.payloads.get(entry_index)
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        if self.payloads.is_empty() {
            for entry in &self.entries {
                validate_payload(entry, &AggregatePayloadV2::None)?;
            }
            return Ok(());
        }
        if self.entries.len() != self.payloads.len() {
            return Err(CoveError::BadIndex);
        }
        for (entry, payload) in self.entries.iter().zip(&self.payloads) {
            validate_payload(entry, payload)?;
        }
        Ok(())
    }
}

fn aggregate_payload_error<T>(_err: T) -> CoveError {
    CoveError::BadIndex
}

pub fn hll_registers_from_hashes(
    precision: u8,
    hashes: impl IntoIterator<Item = u64>,
) -> Result<Vec<u8>, CoveError> {
    validate_hll_precision(precision)?;
    let register_count = 1usize
        .checked_shl(u32::from(precision))
        .ok_or(CoveError::ArithOverflow)?;
    let mut registers = vec![0u8; register_count];
    let index_shift = 64u32
        .checked_sub(u32::from(precision))
        .ok_or(CoveError::ArithOverflow)?;
    for hash in hashes {
        let index = (hash >> index_shift) as usize;
        let suffix = hash << u32::from(precision);
        let rank = suffix.leading_zeros().saturating_add(1).min(64) as u8;
        registers[index] = registers[index].max(rank);
    }
    Ok(registers)
}

pub fn hll_estimate(precision: u8, registers: &[u8]) -> Result<f64, CoveError> {
    validate_hll(precision, registers)?;
    let m = registers.len() as f64;
    let alpha = match registers.len() {
        16 => 0.673,
        32 => 0.697,
        64 => 0.709,
        _ => 0.7213 / (1.0 + 1.079 / m),
    };
    let harmonic = registers
        .iter()
        .map(|rank| 2f64.powi(-i32::from(*rank)))
        .sum::<f64>();
    let raw = alpha * m * m / harmonic;
    let zeros = registers.iter().filter(|rank| **rank == 0).count() as f64;
    if raw <= 2.5 * m && zeros > 0.0 {
        Ok(m * (m / zeros).ln())
    } else {
        Ok(raw)
    }
}

pub fn kll_compactors_from_values(
    k: u32,
    value_tag: ValueTag,
    values: Vec<Vec<u8>>,
) -> Result<(Vec<u32>, Vec<Vec<u8>>), CoveError> {
    for value in &values {
        validate_canonical_payload(value_tag, value)?;
    }
    if k < 8 {
        return Err(CoveError::BadIndex);
    }
    let capacity = usize::try_from(k).map_err(|_| CoveError::ArithOverflow)?;
    let mut levels = vec![values];
    let mut level_index = 0usize;
    while level_index < levels.len() {
        if levels[level_index].len() <= capacity {
            level_index += 1;
            continue;
        }
        let mut sorted = std::mem::take(&mut levels[level_index]);
        sorted.sort_by(|left, right| {
            compare_canonical_payload(value_tag, left, right)
                .unwrap_or_else(|_| left.as_slice().cmp(right.as_slice()))
        });
        let offset = level_index % 2;
        let promoted = sorted
            .into_iter()
            .enumerate()
            .filter_map(|(index, value)| (index % 2 == offset).then_some(value))
            .collect::<Vec<_>>();
        if level_index + 1 == levels.len() {
            levels.push(Vec::new());
        }
        levels[level_index + 1].extend(promoted);
        if level_index > 0 {
            level_index -= 1;
        }
    }

    let mut offsets = Vec::with_capacity(levels.len() + 1);
    let mut flattened = Vec::new();
    offsets.push(0);
    for level in levels {
        flattened.extend(level);
        offsets.push(u32::try_from(flattened.len()).map_err(|_| CoveError::ArithOverflow)?);
    }
    validate_kll(k, value_tag, &offsets, &flattened)?;
    Ok((offsets, flattened))
}

pub fn cove_sketch_hash(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes.iter().fold(OFFSET, |hash, byte| {
        hash.wrapping_mul(PRIME) ^ u64::from(*byte)
    })
}

fn compare_canonical_payload(
    value_tag: ValueTag,
    left: &[u8],
    right: &[u8],
) -> Result<Ordering, CoveError> {
    validate_canonical_payload(value_tag, left)?;
    validate_canonical_payload(value_tag, right)?;
    let ordering =
        match value_tag {
            ValueTag::Null | ValueTag::BoolFalse | ValueTag::BoolTrue => Ordering::Equal,
            ValueTag::Int64
            | ValueTag::Decimal64
            | ValueTag::TimestampMicros
            | ValueTag::TimestampNanos => i64::from_le_bytes(fixed_payload(left)?)
                .cmp(&i64::from_le_bytes(fixed_payload(right)?)),
            ValueTag::UInt64 => u64::from_le_bytes(fixed_payload(left)?)
                .cmp(&u64::from_le_bytes(fixed_payload(right)?)),
            ValueTag::Float32Bits => {
                let left = f32::from_bits(u32::from_le_bytes(fixed_payload(left)?));
                let right = f32::from_bits(u32::from_le_bytes(fixed_payload(right)?));
                left.total_cmp(&right)
            }
            ValueTag::Float64Bits => {
                let left = f64::from_bits(u64::from_le_bytes(fixed_payload(left)?));
                let right = f64::from_bits(u64::from_le_bytes(fixed_payload(right)?));
                left.total_cmp(&right)
            }
            ValueTag::Decimal128 => i128::from_le_bytes(fixed_payload(left)?)
                .cmp(&i128::from_le_bytes(fixed_payload(right)?)),
            ValueTag::DateDays => i32::from_le_bytes(fixed_payload(left)?)
                .cmp(&i32::from_le_bytes(fixed_payload(right)?)),
            ValueTag::Utf8 | ValueTag::Binary | ValueTag::Json => {
                let (left_payload, _) = decode_length_prefixed_payload(left)?;
                let (right_payload, _) = decode_length_prefixed_payload(right)?;
                left_payload.cmp(right_payload)
            }
            ValueTag::Uuid | ValueTag::List | ValueTag::Struct | ValueTag::Map => left.cmp(right),
        };
    Ok(ordering)
}

fn fixed_payload<const N: usize>(bytes: &[u8]) -> Result<[u8; N], CoveError> {
    bytes.try_into().map_err(|_| CoveError::BadIndex)
}

fn decode_length_prefixed_payload(bytes: &[u8]) -> Result<(&[u8], usize), CoveError> {
    let (len, used) = crate::wire::decode_u64_leb128(bytes)?;
    let len = usize::try_from(len).map_err(|_| CoveError::ArithOverflow)?;
    let end = used.checked_add(len).ok_or(CoveError::ArithOverflow)?;
    if end > bytes.len() {
        return Err(CoveError::BufferTooShort);
    }
    Ok((&bytes[used..end], end))
}

fn infer_entry_count(bytes: &[u8]) -> Result<usize, CoveError> {
    let max_entries = bytes.len() / AGGREGATE_SYNOPSIS_ENTRY_LEN;
    let mut first_parse_error = None;
    for entry_count in 1..=max_entries {
        let table_len = entry_count
            .checked_mul(AGGREGATE_SYNOPSIS_ENTRY_LEN)
            .ok_or(CoveError::ArithOverflow)?;
        let mut entries = Vec::with_capacity(entry_count);
        let mut parse_ok = true;
        for index in 0..entry_count {
            let start = index
                .checked_mul(AGGREGATE_SYNOPSIS_ENTRY_LEN)
                .ok_or(CoveError::ArithOverflow)?;
            match AggregateEntry::parse(&bytes[start..start + AGGREGATE_SYNOPSIS_ENTRY_LEN]) {
                Ok(entry) => entries.push(entry),
                Err(err) => {
                    first_parse_error.get_or_insert(err);
                    parse_ok = false;
                    break;
                }
            }
        }
        if !parse_ok {
            continue;
        }
        let payload_offsets = entries
            .iter()
            .filter(|entry| entry.payload_length != 0)
            .map(|entry| usize::try_from(entry.payload_offset).map_err(|_| CoveError::OffsetRange))
            .collect::<Result<Vec<_>, _>>()?;
        if payload_offsets.is_empty() {
            if table_len == bytes.len() {
                return Ok(entry_count);
            }
            continue;
        }
        let Some(min_payload_offset) = payload_offsets.iter().min().copied() else {
            continue;
        };
        if min_payload_offset != table_len {
            continue;
        }
        let mut valid_ranges = true;
        for entry in &entries {
            if entry.payload_length == 0 {
                continue;
            }
            let start =
                usize::try_from(entry.payload_offset).map_err(|_| CoveError::OffsetRange)?;
            let len = usize::try_from(entry.payload_length).map_err(|_| CoveError::OffsetRange)?;
            let Some(end) = start.checked_add(len) else {
                valid_ranges = false;
                break;
            };
            if start < table_len || end > bytes.len() {
                valid_ranges = false;
                break;
            }
        }
        if valid_ranges {
            return Ok(entry_count);
        }
    }
    if let Some(err) = first_parse_error {
        return Err(err);
    }
    Err(CoveError::BadIndex)
}

fn validate_payload(entry: &AggregateEntry, payload: &AggregatePayloadV2) -> Result<(), CoveError> {
    let non_null_count = entry.non_null_count()?;
    match (entry.synopsis_kind, payload) {
        (SynopsisKind::Count, AggregatePayloadV2::None) => Ok(()),
        (SynopsisKind::MinMax, AggregatePayloadV2::MinMax { min, max }) => {
            if entry.accuracy != SynopsisAccuracy::Exact {
                return Err(CoveError::BadIndex);
            }
            match (non_null_count == 0, min.is_some(), max.is_some()) {
                (true, false, false) | (false, true, true) => Ok(()),
                _ => Err(CoveError::BadIndex),
            }
        }
        (SynopsisKind::Sum, AggregatePayloadV2::Sum { sum, .. }) => {
            if entry.accuracy != SynopsisAccuracy::Exact {
                return Err(CoveError::BadIndex);
            }
            validate_numeric_sum_tag(sum.value_tag)
        }
        (
            SynopsisKind::SumAndCount,
            AggregatePayloadV2::SumAndCount {
                non_null_count: declared,
                sum,
                ..
            },
        ) => {
            if entry.accuracy != SynopsisAccuracy::Exact || *declared != non_null_count {
                return Err(CoveError::BadIndex);
            }
            validate_numeric_sum_tag(sum.value_tag)
        }
        (
            SynopsisKind::BoolTrueFalseCounts,
            AggregatePayloadV2::BoolTrueFalseCounts {
                true_count,
                false_count,
            },
        ) => {
            if entry.accuracy != SynopsisAccuracy::Exact {
                return Err(CoveError::BadIndex);
            }
            let total = true_count
                .checked_add(*false_count)
                .ok_or(CoveError::ArithOverflow)?;
            if total == non_null_count {
                Ok(())
            } else {
                Err(CoveError::BadIndex)
            }
        }
        (SynopsisKind::FileCodeHistogram, AggregatePayloadV2::FileCodeHistogram { buckets })
        | (SynopsisKind::NumCodeHistogram, AggregatePayloadV2::NumCodeHistogram { buckets }) => {
            validate_histogram(buckets, entry.accuracy, non_null_count)
        }
        (
            SynopsisKind::DistinctSketch,
            AggregatePayloadV2::DistinctSketch {
                precision,
                registers,
            },
        ) => {
            if entry.accuracy != SynopsisAccuracy::Approximate {
                return Err(CoveError::BadIndex);
            }
            validate_hll(*precision, registers)
        }
        (
            SynopsisKind::QuantileSketch,
            AggregatePayloadV2::QuantileSketch {
                k,
                value_tag,
                level_offsets,
                values,
            },
        ) => {
            if entry.accuracy != SynopsisAccuracy::Approximate {
                return Err(CoveError::BadIndex);
            }
            validate_kll(*k, *value_tag, level_offsets, values)
        }
        (SynopsisKind::TopK, AggregatePayloadV2::TopK { k, entries }) => {
            if *k == 0 || entries.len() > *k as usize {
                return Err(CoveError::BadIndex);
            }
            validate_topk(entries)
        }
        _ => Err(CoveError::BadIndex),
    }
}

fn validate_numeric_sum_tag(value_tag: ValueTag) -> Result<(), CoveError> {
    match value_tag {
        ValueTag::Int64
        | ValueTag::UInt64
        | ValueTag::Decimal64
        | ValueTag::Decimal128
        | ValueTag::Float32Bits
        | ValueTag::Float64Bits => Ok(()),
        _ => Err(CoveError::BadIndex),
    }
}

fn validate_histogram(
    buckets: &[HistogramBucket],
    accuracy: SynopsisAccuracy,
    non_null_count: u64,
) -> Result<(), CoveError> {
    let mut previous_key = None;
    let mut total = 0u64;
    for bucket in buckets {
        if bucket.count == 0 || previous_key.map(|key| bucket.key <= key).unwrap_or(false) {
            return Err(CoveError::BadIndex);
        }
        previous_key = Some(bucket.key);
        total = total
            .checked_add(bucket.count)
            .ok_or(CoveError::ArithOverflow)?;
    }
    if accuracy == SynopsisAccuracy::Exact && total != non_null_count {
        return Err(CoveError::BadIndex);
    }
    Ok(())
}

fn validate_topk(entries: &[HistogramBucket]) -> Result<(), CoveError> {
    let mut previous: Option<&HistogramBucket> = None;
    for entry in entries {
        if entry.count == 0 {
            return Err(CoveError::BadIndex);
        }
        if let Some(prev) = previous {
            let ordered =
                prev.count > entry.count || (prev.count == entry.count && prev.key < entry.key);
            if !ordered {
                return Err(CoveError::BadIndex);
            }
        }
        previous = Some(entry);
    }
    Ok(())
}

fn validate_hll_precision(precision: u8) -> Result<(), CoveError> {
    if (4..=18).contains(&precision) {
        Ok(())
    } else {
        Err(CoveError::BadIndex)
    }
}

fn validate_hll(precision: u8, registers: &[u8]) -> Result<(), CoveError> {
    validate_hll_precision(precision)?;
    let expected = 1usize
        .checked_shl(u32::from(precision))
        .ok_or(CoveError::ArithOverflow)?;
    if registers.len() != expected {
        return Err(CoveError::BadIndex);
    }
    let max_rank = 64u8
        .checked_sub(precision)
        .and_then(|rank| rank.checked_add(1))
        .ok_or(CoveError::ArithOverflow)?;
    if registers.iter().all(|rank| *rank <= max_rank) {
        Ok(())
    } else {
        Err(CoveError::BadIndex)
    }
}

fn validate_kll(
    k: u32,
    value_tag: ValueTag,
    level_offsets: &[u32],
    values: &[Vec<u8>],
) -> Result<(), CoveError> {
    if k < 8 || level_offsets.len() < 2 {
        return Err(CoveError::BadIndex);
    }
    let mut previous = 0u32;
    for offset in level_offsets {
        if *offset < previous
            || usize::try_from(*offset).map_err(|_| CoveError::OffsetRange)? > values.len()
        {
            return Err(CoveError::BadIndex);
        }
        previous = *offset;
    }
    if level_offsets.last().copied().unwrap_or(0) as usize != values.len() {
        return Err(CoveError::BadIndex);
    }
    for value in values {
        validate_canonical_payload(value_tag, value)?;
    }
    Ok(())
}

fn encode_payload(entry: &AggregateEntry, payload: &AggregatePayloadV2) -> Vec<u8> {
    match payload {
        AggregatePayloadV2::None => Vec::new(),
        AggregatePayloadV2::MinMax { min, max } => encode_payload_with_header(
            entry.synopsis_kind,
            0,
            2,
            0,
            0,
            [encode_optional_tagged(min), encode_optional_tagged(max)].concat(),
        ),
        AggregatePayloadV2::Sum {
            overflow_policy,
            sum,
        } => encode_payload_with_header(
            entry.synopsis_kind,
            0,
            1,
            *overflow_policy as u32,
            0,
            encode_tagged(sum),
        ),
        AggregatePayloadV2::SumAndCount {
            overflow_policy,
            non_null_count,
            sum,
        } => {
            let mut data = Vec::new();
            data.extend_from_slice(&non_null_count.to_le_bytes());
            data.extend_from_slice(&encode_tagged(sum));
            encode_payload_with_header(entry.synopsis_kind, 0, 1, *overflow_policy as u32, 0, data)
        }
        AggregatePayloadV2::BoolTrueFalseCounts {
            true_count,
            false_count,
        } => {
            let mut data = Vec::with_capacity(16);
            data.extend_from_slice(&true_count.to_le_bytes());
            data.extend_from_slice(&false_count.to_le_bytes());
            encode_payload_with_header(entry.synopsis_kind, 0, 2, 0, 0, data)
        }
        AggregatePayloadV2::FileCodeHistogram { buckets }
        | AggregatePayloadV2::NumCodeHistogram { buckets } => encode_payload_with_header(
            entry.synopsis_kind,
            0,
            buckets.len() as u32,
            0,
            0,
            encode_buckets(buckets),
        ),
        AggregatePayloadV2::DistinctSketch {
            precision,
            registers,
        } => encode_payload_with_header(
            entry.synopsis_kind,
            0,
            registers.len() as u32,
            u32::from(*precision),
            0,
            registers.clone(),
        ),
        AggregatePayloadV2::QuantileSketch {
            k,
            value_tag,
            level_offsets,
            values,
        } => {
            let mut data = Vec::new();
            data.extend_from_slice(&(*value_tag as u16).to_le_bytes());
            data.extend_from_slice(&0u16.to_le_bytes());
            data.extend_from_slice(&(level_offsets.len() as u32).to_le_bytes());
            for offset in level_offsets {
                data.extend_from_slice(&offset.to_le_bytes());
            }
            for value in values {
                data.extend_from_slice(&(value.len() as u32).to_le_bytes());
                data.extend_from_slice(value);
            }
            encode_payload_with_header(entry.synopsis_kind, 0, values.len() as u32, *k, 0, data)
        }
        AggregatePayloadV2::TopK { k, entries } => encode_payload_with_header(
            entry.synopsis_kind,
            0,
            entries.len() as u32,
            *k,
            0,
            encode_buckets(entries),
        ),
    }
}

fn encode_payload_with_header(
    kind: SynopsisKind,
    flags: u16,
    item_count: u32,
    aux0: u32,
    aux1: u32,
    data: Vec<u8>,
) -> Vec<u8> {
    let mut out = vec![0u8; AGGREGATE_PAYLOAD_HEADER_LEN];
    out[0..4].copy_from_slice(&AGGREGATE_PAYLOAD_MAGIC);
    out[4] = kind as u8;
    out[5] = AGGREGATE_PAYLOAD_VERSION;
    out[6..8].copy_from_slice(&flags.to_le_bytes());
    out[8..12].copy_from_slice(&item_count.to_le_bytes());
    out[12..16].copy_from_slice(&aux0.to_le_bytes());
    out[16..20].copy_from_slice(&aux1.to_le_bytes());
    out[20..24].copy_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&data);
    let crc = checksum::crc32c(&out);
    out[24..28].copy_from_slice(&crc.to_le_bytes());
    out
}

fn parse_payload(entry: &AggregateEntry, bytes: &[u8]) -> Result<AggregatePayloadV2, CoveError> {
    let (header, data) = parse_payload_header(bytes)?;
    if header.kind != entry.synopsis_kind {
        return Err(CoveError::BadIndex);
    }
    let payload = match header.kind {
        SynopsisKind::Count => return Err(CoveError::BadIndex),
        SynopsisKind::MinMax => {
            if header.item_count != 2 {
                return Err(CoveError::BadIndex);
            }
            let mut cursor = Cursor::new(data);
            AggregatePayloadV2::MinMax {
                min: parse_optional_tagged(&mut cursor)?,
                max: parse_optional_tagged(&mut cursor)?,
            }
        }
        SynopsisKind::Sum => AggregatePayloadV2::Sum {
            overflow_policy: NumericAggregateOverflowPolicy::from_u32(header.aux0)
                .ok_or(CoveError::BadIndex)?,
            sum: parse_single_tagged_payload(data)?,
        },
        SynopsisKind::SumAndCount => {
            let mut cursor = Cursor::new(data);
            let non_null_count = cursor.u64()?;
            AggregatePayloadV2::SumAndCount {
                overflow_policy: NumericAggregateOverflowPolicy::from_u32(header.aux0)
                    .ok_or(CoveError::BadIndex)?,
                non_null_count,
                sum: parse_tagged(&mut cursor)?,
            }
        }
        SynopsisKind::BoolTrueFalseCounts => {
            if data.len() != 16 || header.item_count != 2 {
                return Err(CoveError::BadIndex);
            }
            AggregatePayloadV2::BoolTrueFalseCounts {
                true_count: u64::from_le_bytes(data[0..8].try_into().unwrap()),
                false_count: u64::from_le_bytes(data[8..16].try_into().unwrap()),
            }
        }
        SynopsisKind::FileCodeHistogram => AggregatePayloadV2::FileCodeHistogram {
            buckets: parse_buckets(data, header.item_count)?,
        },
        SynopsisKind::NumCodeHistogram => AggregatePayloadV2::NumCodeHistogram {
            buckets: parse_buckets(data, header.item_count)?,
        },
        SynopsisKind::DistinctSketch => {
            let precision = u8::try_from(header.aux0).map_err(|_| CoveError::BadIndex)?;
            if header.item_count as usize != data.len() {
                return Err(CoveError::BadIndex);
            }
            AggregatePayloadV2::DistinctSketch {
                precision,
                registers: data.to_vec(),
            }
        }
        SynopsisKind::QuantileSketch => {
            let mut cursor = Cursor::new(data);
            let value_tag_raw = cursor.u16()?;
            let value_tag = ValueTag::from_u16(value_tag_raw).ok_or(CoveError::BadIndex)?;
            if cursor.u16()? != 0 {
                return Err(CoveError::BadIndex);
            }
            let level_count = cursor.u32()? as usize;
            if level_count < 2 {
                return Err(CoveError::BadIndex);
            }
            let mut level_offsets = Vec::with_capacity(level_count);
            for _ in 0..level_count {
                level_offsets.push(cursor.u32()?);
            }
            let mut values = Vec::with_capacity(header.item_count as usize);
            for _ in 0..header.item_count {
                let len = cursor.u32()? as usize;
                values.push(cursor.bytes(len)?.to_vec());
            }
            if !cursor.is_empty() {
                return Err(CoveError::BadIndex);
            }
            AggregatePayloadV2::QuantileSketch {
                k: header.aux0,
                value_tag,
                level_offsets,
                values,
            }
        }
        SynopsisKind::TopK => AggregatePayloadV2::TopK {
            k: header.aux0,
            entries: parse_buckets(data, header.item_count)?,
        },
    };
    validate_payload(entry, &payload)?;
    Ok(payload)
}

fn parse_payload_header(bytes: &[u8]) -> Result<(AggregatePayloadHeader, &[u8]), CoveError> {
    if bytes.len() < AGGREGATE_PAYLOAD_HEADER_LEN {
        return Err(CoveError::BufferTooShort);
    }
    if bytes[0..4] != AGGREGATE_PAYLOAD_MAGIC {
        return Err(CoveError::BadIndex);
    }
    if bytes[5] != AGGREGATE_PAYLOAD_VERSION {
        return Err(CoveError::BadVersion);
    }
    let kind = SynopsisKind::from_u8(bytes[4]).ok_or(CoveError::BadIndex)?;
    let data_len = u32::from_le_bytes(bytes[20..24].try_into().unwrap());
    let checksum = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
    let data_len_usize = usize::try_from(data_len).map_err(|_| CoveError::OffsetRange)?;
    let expected_len = AGGREGATE_PAYLOAD_HEADER_LEN
        .checked_add(data_len_usize)
        .ok_or(CoveError::ArithOverflow)?;
    if expected_len != bytes.len() {
        return Err(CoveError::BadIndex);
    }
    let mut zeroed = bytes.to_vec();
    zeroed[24..28].fill(0);
    if checksum::crc32c(&zeroed) != checksum {
        return Err(CoveError::ChecksumMismatch);
    }
    Ok((
        AggregatePayloadHeader {
            kind,
            flags: u16::from_le_bytes(bytes[6..8].try_into().unwrap()),
            item_count: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
            aux0: u32::from_le_bytes(bytes[12..16].try_into().unwrap()),
            aux1: u32::from_le_bytes(bytes[16..20].try_into().unwrap()),
            data_len,
            checksum,
        },
        &bytes[AGGREGATE_PAYLOAD_HEADER_LEN..],
    ))
}

fn encode_optional_tagged(value: &Option<TaggedCanonicalValue>) -> Vec<u8> {
    match value {
        Some(value) => encode_tagged(value),
        None => {
            let mut out = Vec::with_capacity(6);
            out.extend_from_slice(&ABSENT_VALUE_TAG.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            out
        }
    }
}

fn encode_tagged(value: &TaggedCanonicalValue) -> Vec<u8> {
    let mut out = Vec::with_capacity(6 + value.payload.len());
    out.extend_from_slice(&(value.value_tag as u16).to_le_bytes());
    out.extend_from_slice(&(value.payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&value.payload);
    out
}

fn parse_single_tagged_payload(data: &[u8]) -> Result<TaggedCanonicalValue, CoveError> {
    let mut cursor = Cursor::new(data);
    let value = parse_tagged(&mut cursor)?;
    if !cursor.is_empty() {
        return Err(CoveError::BadIndex);
    }
    Ok(value)
}

fn parse_optional_tagged(
    cursor: &mut Cursor<'_>,
) -> Result<Option<TaggedCanonicalValue>, CoveError> {
    let raw_tag = cursor.u16()?;
    let len = cursor.u32()? as usize;
    if raw_tag == ABSENT_VALUE_TAG {
        if len == 0 {
            return Ok(None);
        }
        return Err(CoveError::BadIndex);
    }
    let value_tag = ValueTag::from_u16(raw_tag).ok_or(CoveError::BadIndex)?;
    let payload = cursor.bytes(len)?.to_vec();
    validate_canonical_payload(value_tag, &payload)?;
    Ok(Some(TaggedCanonicalValue { value_tag, payload }))
}

fn parse_tagged(cursor: &mut Cursor<'_>) -> Result<TaggedCanonicalValue, CoveError> {
    parse_optional_tagged(cursor)?.ok_or(CoveError::BadIndex)
}

fn encode_buckets(buckets: &[HistogramBucket]) -> Vec<u8> {
    let mut out = Vec::with_capacity(buckets.len() * 16);
    for bucket in buckets {
        out.extend_from_slice(&bucket.key.to_le_bytes());
        out.extend_from_slice(&bucket.count.to_le_bytes());
    }
    out
}

fn parse_buckets(data: &[u8], item_count: u32) -> Result<Vec<HistogramBucket>, CoveError> {
    let expected = (item_count as usize)
        .checked_mul(16)
        .ok_or(CoveError::ArithOverflow)?;
    if data.len() != expected {
        return Err(CoveError::BadIndex);
    }
    let mut buckets = Vec::with_capacity(item_count as usize);
    for chunk in data.chunks_exact(16) {
        buckets.push(HistogramBucket {
            key: u64::from_le_bytes(chunk[0..8].try_into().unwrap()),
            count: u64::from_le_bytes(chunk[8..16].try_into().unwrap()),
        });
    }
    Ok(buckets)
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn is_empty(&self) -> bool {
        self.pos == self.bytes.len()
    }

    fn bytes(&mut self, len: usize) -> Result<&'a [u8], CoveError> {
        let end = self.pos.checked_add(len).ok_or(CoveError::ArithOverflow)?;
        let out = self
            .bytes
            .get(self.pos..end)
            .ok_or(CoveError::OffsetRange)?;
        self.pos = end;
        Ok(out)
    }

    fn u16(&mut self) -> Result<u16, CoveError> {
        let bytes: [u8; 2] = self.bytes(2)?.try_into().unwrap();
        Ok(u16::from_le_bytes(bytes))
    }

    fn u32(&mut self) -> Result<u32, CoveError> {
        let bytes: [u8; 4] = self.bytes(4)?.try_into().unwrap();
        Ok(u32::from_le_bytes(bytes))
    }

    fn u64(&mut self) -> Result<u64, CoveError> {
        let bytes: [u8; 8] = self.bytes(8)?.try_into().unwrap();
        Ok(u64::from_le_bytes(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_entry(kind: SynopsisKind, row_count: u32, null_count: u32) -> AggregateEntry {
        AggregateEntry {
            table_id: 1,
            segment_id: 2,
            morsel_id: u32::MAX,
            column_id: 3,
            synopsis_kind: kind,
            key_kind: 0,
            accuracy: if matches!(
                kind,
                SynopsisKind::DistinctSketch | SynopsisKind::QuantileSketch
            ) {
                SynopsisAccuracy::Approximate
            } else {
                SynopsisAccuracy::Exact
            },
            flags: 0,
            row_count,
            null_count,
            payload_offset: 0,
            payload_length: 0,
            checksum: 0,
        }
    }

    fn int_value(value: i64) -> TaggedCanonicalValue {
        TaggedCanonicalValue::new(ValueTag::Int64, value.to_le_bytes().to_vec()).unwrap()
    }

    fn refresh_payload_crc(bytes: &mut [u8], payload_start: usize) {
        let crc_offset = payload_start + 24;
        bytes[crc_offset..crc_offset + 4].fill(0);
        let crc = checksum::crc32c(&bytes[payload_start..]);
        bytes[crc_offset..crc_offset + 4].copy_from_slice(&crc.to_le_bytes());
    }

    #[test]
    fn round_trip_count_synopsis() {
        let bytes = count_entry(SynopsisKind::Count, 12345, 0)
            .serialize()
            .to_vec();
        let s = AggregateSynopsis::parse(&bytes).unwrap();
        assert_eq!(s.entries[0].synopsis_kind, SynopsisKind::Count);
        assert_eq!(s.entries[0].row_count, 12345);
        assert_eq!(s.payloads[0], AggregatePayloadV2::None);
    }

    #[test]
    fn round_trips_payload_kinds() {
        let entries = vec![
            count_entry(SynopsisKind::Count, 3, 0),
            count_entry(SynopsisKind::MinMax, 3, 0),
            count_entry(SynopsisKind::Sum, 3, 0),
            count_entry(SynopsisKind::SumAndCount, 3, 0),
            count_entry(SynopsisKind::BoolTrueFalseCounts, 3, 0),
            count_entry(SynopsisKind::FileCodeHistogram, 3, 0),
            count_entry(SynopsisKind::NumCodeHistogram, 3, 0),
            count_entry(SynopsisKind::DistinctSketch, 3, 0),
            count_entry(SynopsisKind::QuantileSketch, 3, 0),
            count_entry(SynopsisKind::TopK, 3, 0),
        ];
        let payloads = vec![
            AggregatePayloadV2::None,
            AggregatePayloadV2::MinMax {
                min: Some(int_value(1)),
                max: Some(int_value(3)),
            },
            AggregatePayloadV2::Sum {
                overflow_policy: NumericAggregateOverflowPolicy::CheckedExact,
                sum: int_value(6),
            },
            AggregatePayloadV2::SumAndCount {
                overflow_policy: NumericAggregateOverflowPolicy::CheckedExact,
                non_null_count: 3,
                sum: int_value(6),
            },
            AggregatePayloadV2::BoolTrueFalseCounts {
                true_count: 2,
                false_count: 1,
            },
            AggregatePayloadV2::FileCodeHistogram {
                buckets: vec![
                    HistogramBucket { key: 1, count: 1 },
                    HistogramBucket { key: 2, count: 2 },
                ],
            },
            AggregatePayloadV2::NumCodeHistogram {
                buckets: vec![
                    HistogramBucket { key: 7, count: 1 },
                    HistogramBucket { key: 8, count: 2 },
                ],
            },
            AggregatePayloadV2::DistinctSketch {
                precision: 4,
                registers: vec![0; 16],
            },
            AggregatePayloadV2::QuantileSketch {
                k: 8,
                value_tag: ValueTag::Int64,
                level_offsets: vec![0, 3],
                values: vec![
                    1i64.to_le_bytes().to_vec(),
                    2i64.to_le_bytes().to_vec(),
                    3i64.to_le_bytes().to_vec(),
                ],
            },
            AggregatePayloadV2::TopK {
                k: 64,
                entries: vec![
                    HistogramBucket { key: 2, count: 2 },
                    HistogramBucket { key: 1, count: 1 },
                ],
            },
        ];
        let synopsis = AggregateSynopsis::from_parts(entries, payloads.clone()).unwrap();
        let parsed = AggregateSynopsis::parse(&synopsis.serialize()).unwrap();
        assert_eq!(parsed.payloads, payloads);
    }

    #[test]
    fn rejects_unknown_kind() {
        let mut bytes = count_entry(SynopsisKind::Count, 1, 0).serialize();
        bytes[16] = 99;
        bytes[44..48].fill(0);
        let crc = checksum::crc32c(&bytes);
        bytes[44..48].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(AggregateSynopsis::parse(&bytes), Err(CoveError::BadIndex));
    }

    #[test]
    fn rejects_checksum_mismatch() {
        let mut bytes = count_entry(SynopsisKind::Count, 1, 0).serialize();
        bytes[44] ^= 0xff;
        assert_eq!(
            AggregateSynopsis::parse(&bytes),
            Err(CoveError::ChecksumMismatch)
        );
    }

    #[test]
    fn rejects_payload_checksum_mismatch() {
        let synopsis = AggregateSynopsis::from_parts(
            vec![count_entry(SynopsisKind::BoolTrueFalseCounts, 2, 0)],
            vec![AggregatePayloadV2::BoolTrueFalseCounts {
                true_count: 1,
                false_count: 1,
            }],
        )
        .unwrap();
        let mut bytes = synopsis.serialize();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        assert_eq!(AggregateSynopsis::parse(&bytes), Err(CoveError::BadIndex));
    }

    #[test]
    fn rejects_invalid_canonical_payload_as_bad_index() {
        let synopsis = AggregateSynopsis::from_parts(
            vec![count_entry(SynopsisKind::Sum, 1, 0)],
            vec![AggregatePayloadV2::Sum {
                overflow_policy: NumericAggregateOverflowPolicy::CheckedExact,
                sum: int_value(1),
            }],
        )
        .unwrap();
        let mut bytes = synopsis.serialize();
        let payload_start = AGGREGATE_SYNOPSIS_ENTRY_LEN;
        let sum_value_len_offset = payload_start + AGGREGATE_PAYLOAD_HEADER_LEN + 2;
        bytes[sum_value_len_offset..sum_value_len_offset + 4].copy_from_slice(&4u32.to_le_bytes());
        refresh_payload_crc(&mut bytes, payload_start);
        assert_eq!(AggregateSynopsis::parse(&bytes), Err(CoveError::BadIndex));
    }

    #[test]
    fn rejects_unsorted_histogram_keys() {
        let synopsis = AggregateSynopsis::from_parts(
            vec![count_entry(SynopsisKind::FileCodeHistogram, 2, 0)],
            vec![AggregatePayloadV2::FileCodeHistogram {
                buckets: vec![
                    HistogramBucket { key: 2, count: 1 },
                    HistogramBucket { key: 1, count: 1 },
                ],
            }],
        );
        assert_eq!(synopsis, Err(CoveError::BadIndex));
    }

    #[test]
    fn rejects_count_sum_mismatch() {
        let synopsis = AggregateSynopsis::from_parts(
            vec![count_entry(SynopsisKind::BoolTrueFalseCounts, 3, 0)],
            vec![AggregatePayloadV2::BoolTrueFalseCounts {
                true_count: 1,
                false_count: 1,
            }],
        );
        assert_eq!(synopsis, Err(CoveError::BadIndex));
    }

    #[test]
    fn rejects_bad_hll_precision() {
        let synopsis = AggregateSynopsis::from_parts(
            vec![count_entry(SynopsisKind::DistinctSketch, 3, 0)],
            vec![AggregatePayloadV2::DistinctSketch {
                precision: 3,
                registers: vec![0; 8],
            }],
        );
        assert_eq!(synopsis, Err(CoveError::BadIndex));
    }

    #[test]
    fn hll_estimator_accepts_default_precision() {
        let registers = hll_registers_from_hashes(
            DEFAULT_HLL_PRECISION,
            [cove_sketch_hash(b"a"), cove_sketch_hash(b"b")],
        )
        .unwrap();
        assert!(hll_estimate(DEFAULT_HLL_PRECISION, &registers).unwrap() > 0.0);
    }

    #[test]
    fn kll_compaction_is_deterministic_and_valid() {
        let values = (0..32i64)
            .map(|value| value.to_le_bytes().to_vec())
            .collect::<Vec<_>>();
        let first = kll_compactors_from_values(8, ValueTag::Int64, values.clone()).unwrap();
        let second = kll_compactors_from_values(8, ValueTag::Int64, values).unwrap();
        assert_eq!(first, second);
        assert!(first.0.len() > 2);
        assert!(first.1.len() < 32);
    }

    #[test]
    fn serialize_round_trip_multiple_count_entries() {
        let mk = |morsel_id| {
            let mut entry = count_entry(SynopsisKind::Count, morsel_id, 0);
            entry.morsel_id = morsel_id;
            entry
        };
        let synopsis = AggregateSynopsis::from_entries(vec![mk(10), mk(20), mk(30)]);
        let bytes = synopsis.serialize();
        let parsed = AggregateSynopsis::parse(&bytes).unwrap();
        assert_eq!(parsed.entries.len(), 3);
        assert_eq!(parsed.entries[2].row_count, 30);
    }

    #[test]
    fn varint_hash_is_deterministic() {
        let encoded = crate::wire::encode_u64_leb128(42);
        assert_eq!(cove_sketch_hash(&encoded), cove_sketch_hash(&encoded));
    }
}
