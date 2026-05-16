// Licensed to the Apache Software Foundation (ASF) under one
// or more contributor license agreements.  See the NOTICE file
// distributed with this work for additional information
// regarding copyright ownership.  The ASF licenses this file
// to you under the Apache License, Version 2.0 (the
// "License"); you may not use this file except in compliance
// with the License.  You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing,
// software distributed under the License is distributed on an
// "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.  See the License for the
// specific language governing permissions and limitations
// under the License.

use std::marker::PhantomData;

use arrow::{
    array::{Array, ArrayRef, AsArray, Decimal128Array, FixedSizeBinaryArray},
    buffer::NullBuffer,
    datatypes::{
        ArrowPrimitiveType, ByteArrayType, Date32Type, Float32Type, Float64Type, GenericBinaryType,
        GenericStringType, Int16Type, Int32Type, Int64Type, Int8Type, TimeUnit,
        TimestampMicrosecondType, TimestampMillisecondType, TimestampNanosecondType,
        TimestampSecondType, UInt16Type, UInt32Type, UInt64Type, UInt8Type,
    },
};
use bytes::{BufMut, BytesMut};
use snafu::OptionExt;

use crate::{
    encoding::{
        boolean::BooleanEncoder,
        byte::ByteRleEncoder,
        float::FloatEncoder,
        integer::{rle_v2::RleV2Encoder, NInt, SignedEncoding, UnsignedEncoding},
        PrimitiveValueEncoder,
    },
    error::{Result, UnexpectedSnafu},
    memory::EstimateMemory,
    writer::StreamType,
};

use super::{ColumnEncoding, Stream};

const ORC_EPOCH_UTC_SECONDS_SINCE_UNIX_EPOCH: i64 = 1_420_070_400;

fn write_i128_zigzag_varint(writer: &mut BytesMut, value: i128) {
    let mut encoded = ((value << 1) ^ (value >> 127)) as u128;
    while encoded >= 0x80 {
        writer.put_u8((encoded as u8) | 0x80);
        encoded >>= 7;
    }
    writer.put_u8(encoded as u8);
}

/// Encodes a specific ORC column node for a stripe.
pub trait ColumnStripeEncoder: EstimateMemory {
    fn encode_array(&mut self, array: &ArrayRef) -> Result<()> {
        self.encode_array_with_parent(array, None)
    }

    fn encode_array_with_parent(
        &mut self,
        array: &ArrayRef,
        parent_present: Option<&NullBuffer>,
    ) -> Result<()>;

    #[allow(dead_code)]
    fn column_encoding(&self) -> ColumnEncoding;

    fn finish(&mut self) -> Vec<Stream>;

    fn finish_columns(&mut self, next_column: &mut usize, out: &mut Vec<(usize, Stream)>) {
        let column = *next_column;
        *next_column += 1;
        for stream in self.finish() {
            out.push((column, stream));
        }
    }
}

fn valid_indices(len: usize, parent_present: Option<&NullBuffer>) -> Vec<usize> {
    match parent_present {
        Some(parent) => parent.valid_indices().collect(),
        None => (0..len).collect(),
    }
}

fn combined_present(array: &dyn Array, parent_present: Option<&NullBuffer>) -> Option<NullBuffer> {
    let mut bits = Vec::with_capacity(array.len());
    let mut has_null = false;
    for index in 0..array.len() {
        let parent_valid = parent_present.map_or(true, |parent| parent.is_valid(index));
        let valid = parent_valid && array.is_valid(index);
        has_null |= !valid;
        bits.push(valid);
    }
    has_null.then(|| NullBuffer::from(bits))
}

fn encode_validity(
    present: &mut Option<BooleanEncoder>,
    encoded_count: &mut usize,
    array: &dyn Array,
    parent_present: Option<&NullBuffer>,
) -> Vec<usize> {
    let indices = valid_indices(array.len(), parent_present);
    let has_own_validity =
        array.nulls().is_some() || indices.iter().any(|index| array.is_null(*index));
    if has_own_validity || present.is_some() {
        if present.is_none() {
            let mut stream = BooleanEncoder::new();
            stream.extend_present(*encoded_count);
            *present = Some(stream);
        }
        if let Some(stream) = present.as_mut() {
            for index in &indices {
                stream.extend_boolean(array.is_valid(*index));
            }
        }
    }
    let non_null_indices = indices
        .into_iter()
        .filter(|index| array.is_valid(*index))
        .collect::<Vec<_>>();
    *encoded_count += non_null_indices.len();
    non_null_indices
}

fn finish_with_present(
    present: &mut Option<BooleanEncoder>,
    data_streams: impl IntoIterator<Item = Stream>,
) -> Vec<Stream> {
    let mut streams = data_streams.into_iter().collect::<Vec<_>>();
    if let Some(present) = present {
        streams.push(Stream {
            kind: StreamType::Present,
            bytes: present.finish(),
        });
    }
    streams
}

pub struct PrimitiveColumnEncoder<T: ArrowPrimitiveType, E: PrimitiveValueEncoder<T::Native>> {
    encoder: E,
    #[allow(dead_code)]
    column_encoding: ColumnEncoding,
    present: Option<BooleanEncoder>,
    encoded_count: usize,
    _phantom: PhantomData<T>,
}

impl<T: ArrowPrimitiveType, E: PrimitiveValueEncoder<T::Native>> PrimitiveColumnEncoder<T, E> {
    pub fn new(column_encoding: ColumnEncoding) -> Self {
        Self {
            encoder: E::new(),
            column_encoding,
            present: None,
            encoded_count: 0,
            _phantom: Default::default(),
        }
    }
}

impl<T: ArrowPrimitiveType, E: PrimitiveValueEncoder<T::Native>> EstimateMemory
    for PrimitiveColumnEncoder<T, E>
{
    fn estimate_memory_size(&self) -> usize {
        self.encoder.estimate_memory_size()
            + self
                .present
                .as_ref()
                .map(|p| p.estimate_memory_size())
                .unwrap_or(0)
    }
}

impl<T: ArrowPrimitiveType, E: PrimitiveValueEncoder<T::Native>> ColumnStripeEncoder
    for PrimitiveColumnEncoder<T, E>
{
    fn encode_array_with_parent(
        &mut self,
        array: &ArrayRef,
        parent_present: Option<&NullBuffer>,
    ) -> Result<()> {
        let array = array.as_primitive::<T>();
        for index in encode_validity(
            &mut self.present,
            &mut self.encoded_count,
            array,
            parent_present,
        ) {
            self.encoder.write_one(array.value(index));
        }
        Ok(())
    }

    fn column_encoding(&self) -> ColumnEncoding {
        self.column_encoding
    }

    fn finish(&mut self) -> Vec<Stream> {
        let bytes = self.encoder.take_inner();
        self.encoded_count = 0;
        finish_with_present(
            &mut self.present,
            [Stream {
                kind: StreamType::Data,
                bytes,
            }],
        )
    }
}

pub struct BooleanColumnEncoder {
    encoder: BooleanEncoder,
    present: Option<BooleanEncoder>,
    encoded_count: usize,
}

impl BooleanColumnEncoder {
    pub fn new() -> Self {
        Self {
            encoder: BooleanEncoder::new(),
            present: None,
            encoded_count: 0,
        }
    }
}

impl EstimateMemory for BooleanColumnEncoder {
    fn estimate_memory_size(&self) -> usize {
        self.encoder.estimate_memory_size()
            + self
                .present
                .as_ref()
                .map(|p| p.estimate_memory_size())
                .unwrap_or(0)
    }
}

impl ColumnStripeEncoder for BooleanColumnEncoder {
    fn encode_array_with_parent(
        &mut self,
        array: &ArrayRef,
        parent_present: Option<&NullBuffer>,
    ) -> Result<()> {
        let array = array.as_boolean();
        for index in encode_validity(
            &mut self.present,
            &mut self.encoded_count,
            array,
            parent_present,
        ) {
            self.encoder.extend_boolean(array.value(index));
        }
        Ok(())
    }

    fn column_encoding(&self) -> ColumnEncoding {
        ColumnEncoding::DirectV2
    }

    fn finish(&mut self) -> Vec<Stream> {
        let bytes = self.encoder.finish();
        self.encoded_count = 0;
        finish_with_present(
            &mut self.present,
            [Stream {
                kind: StreamType::Data,
                bytes,
            }],
        )
    }
}

pub struct GenericBinaryColumnEncoder<T: ByteArrayType>
where
    T::Offset: NInt,
{
    string_bytes: BytesMut,
    length_encoder: RleV2Encoder<T::Offset, UnsignedEncoding>,
    present: Option<BooleanEncoder>,
    encoded_count: usize,
    _phantom: PhantomData<T>,
}

impl<T: ByteArrayType> GenericBinaryColumnEncoder<T>
where
    T::Offset: NInt,
{
    pub fn new() -> Self {
        Self {
            string_bytes: BytesMut::new(),
            length_encoder: RleV2Encoder::new(),
            present: None,
            encoded_count: 0,
            _phantom: Default::default(),
        }
    }
}

impl<T: ByteArrayType> EstimateMemory for GenericBinaryColumnEncoder<T>
where
    T::Offset: NInt,
{
    fn estimate_memory_size(&self) -> usize {
        self.string_bytes.len()
            + self.length_encoder.estimate_memory_size()
            + self
                .present
                .as_ref()
                .map(|p| p.estimate_memory_size())
                .unwrap_or(0)
    }
}

impl<T: ByteArrayType> ColumnStripeEncoder for GenericBinaryColumnEncoder<T>
where
    T::Offset: NInt,
{
    fn encode_array_with_parent(
        &mut self,
        array: &ArrayRef,
        parent_present: Option<&NullBuffer>,
    ) -> Result<()> {
        if array.is_empty() {
            return Ok(());
        }
        let array = array.as_bytes::<T>();
        for index in encode_validity(
            &mut self.present,
            &mut self.encoded_count,
            array,
            parent_present,
        ) {
            self.length_encoder.write_one(array.value_length(index));
            self.string_bytes.put_slice(array.value(index).as_ref());
        }
        Ok(())
    }

    fn column_encoding(&self) -> ColumnEncoding {
        ColumnEncoding::DirectV2
    }

    fn finish(&mut self) -> Vec<Stream> {
        let data_bytes = std::mem::take(&mut self.string_bytes);
        let length_bytes = self.length_encoder.take_inner();
        self.encoded_count = 0;
        finish_with_present(
            &mut self.present,
            [
                Stream {
                    kind: StreamType::Data,
                    bytes: data_bytes.into(),
                },
                Stream {
                    kind: StreamType::Length,
                    bytes: length_bytes,
                },
            ],
        )
    }
}

pub struct FixedSizeBinaryColumnEncoder {
    bytes: BytesMut,
    length_encoder: RleV2Encoder<i64, UnsignedEncoding>,
    present: Option<BooleanEncoder>,
    encoded_count: usize,
}

impl FixedSizeBinaryColumnEncoder {
    pub fn new() -> Self {
        Self {
            bytes: BytesMut::new(),
            length_encoder: RleV2Encoder::new(),
            present: None,
            encoded_count: 0,
        }
    }
}

impl EstimateMemory for FixedSizeBinaryColumnEncoder {
    fn estimate_memory_size(&self) -> usize {
        self.bytes.len() + self.length_encoder.estimate_memory_size()
    }
}

impl ColumnStripeEncoder for FixedSizeBinaryColumnEncoder {
    fn encode_array_with_parent(
        &mut self,
        array: &ArrayRef,
        parent_present: Option<&NullBuffer>,
    ) -> Result<()> {
        let array = array
            .as_any()
            .downcast_ref::<FixedSizeBinaryArray>()
            .context(UnexpectedSnafu {
                msg: "expected FixedSizeBinaryArray",
            })?;
        for index in encode_validity(
            &mut self.present,
            &mut self.encoded_count,
            array,
            parent_present,
        ) {
            let value = array.value(index);
            self.length_encoder.write_one(value.len() as i64);
            self.bytes.put_slice(value);
        }
        Ok(())
    }

    fn column_encoding(&self) -> ColumnEncoding {
        ColumnEncoding::DirectV2
    }

    fn finish(&mut self) -> Vec<Stream> {
        let data = std::mem::take(&mut self.bytes);
        let length = self.length_encoder.take_inner();
        self.encoded_count = 0;
        finish_with_present(
            &mut self.present,
            [
                Stream {
                    kind: StreamType::Data,
                    bytes: data.into(),
                },
                Stream {
                    kind: StreamType::Length,
                    bytes: length,
                },
            ],
        )
    }
}

pub struct UInt64DecimalColumnEncoder {
    data: BytesMut,
    scale: RleV2Encoder<i32, SignedEncoding>,
    present: Option<BooleanEncoder>,
    encoded_count: usize,
}

impl UInt64DecimalColumnEncoder {
    pub fn new() -> Self {
        Self {
            data: BytesMut::new(),
            scale: RleV2Encoder::new(),
            present: None,
            encoded_count: 0,
        }
    }
}

impl EstimateMemory for UInt64DecimalColumnEncoder {
    fn estimate_memory_size(&self) -> usize {
        self.data.len() + self.scale.estimate_memory_size()
    }
}

impl ColumnStripeEncoder for UInt64DecimalColumnEncoder {
    fn encode_array_with_parent(
        &mut self,
        array: &ArrayRef,
        parent_present: Option<&NullBuffer>,
    ) -> Result<()> {
        let array = array.as_primitive::<UInt64Type>();
        for index in encode_validity(
            &mut self.present,
            &mut self.encoded_count,
            array,
            parent_present,
        ) {
            write_i128_zigzag_varint(&mut self.data, i128::from(array.value(index)));
            self.scale.write_one(0);
        }
        Ok(())
    }

    fn column_encoding(&self) -> ColumnEncoding {
        ColumnEncoding::DirectV2
    }

    fn finish(&mut self) -> Vec<Stream> {
        let data = std::mem::take(&mut self.data);
        let scale = self.scale.take_inner();
        self.encoded_count = 0;
        finish_with_present(
            &mut self.present,
            [
                Stream {
                    kind: StreamType::Data,
                    bytes: data.into(),
                },
                Stream {
                    kind: StreamType::Secondary,
                    bytes: scale,
                },
            ],
        )
    }
}

pub enum UnsignedLongWidth {
    U8,
    U16,
    U32,
}

pub struct UnsignedLongColumnEncoder {
    width: UnsignedLongWidth,
    encoder: RleV2Encoder<i64, SignedEncoding>,
    present: Option<BooleanEncoder>,
    encoded_count: usize,
}

impl UnsignedLongColumnEncoder {
    pub fn new(width: UnsignedLongWidth) -> Self {
        Self {
            width,
            encoder: RleV2Encoder::new(),
            present: None,
            encoded_count: 0,
        }
    }

    fn value(&self, array: &ArrayRef, index: usize) -> i64 {
        match self.width {
            UnsignedLongWidth::U8 => i64::from(array.as_primitive::<UInt8Type>().value(index)),
            UnsignedLongWidth::U16 => i64::from(array.as_primitive::<UInt16Type>().value(index)),
            UnsignedLongWidth::U32 => i64::from(array.as_primitive::<UInt32Type>().value(index)),
        }
    }
}

impl EstimateMemory for UnsignedLongColumnEncoder {
    fn estimate_memory_size(&self) -> usize {
        self.encoder.estimate_memory_size()
    }
}

impl ColumnStripeEncoder for UnsignedLongColumnEncoder {
    fn encode_array_with_parent(
        &mut self,
        array: &ArrayRef,
        parent_present: Option<&NullBuffer>,
    ) -> Result<()> {
        for index in encode_validity(
            &mut self.present,
            &mut self.encoded_count,
            array.as_ref(),
            parent_present,
        ) {
            self.encoder.write_one(self.value(array, index));
        }
        Ok(())
    }

    fn column_encoding(&self) -> ColumnEncoding {
        ColumnEncoding::DirectV2
    }

    fn finish(&mut self) -> Vec<Stream> {
        let bytes = self.encoder.take_inner();
        self.encoded_count = 0;
        finish_with_present(
            &mut self.present,
            [Stream {
                kind: StreamType::Data,
                bytes,
            }],
        )
    }
}

pub struct DecimalColumnEncoder {
    data: BytesMut,
    scale: RleV2Encoder<i32, SignedEncoding>,
    fixed_scale: i32,
    present: Option<BooleanEncoder>,
    encoded_count: usize,
}

impl DecimalColumnEncoder {
    pub fn new(fixed_scale: i32) -> Self {
        Self {
            data: BytesMut::new(),
            scale: RleV2Encoder::new(),
            fixed_scale,
            present: None,
            encoded_count: 0,
        }
    }
}

impl EstimateMemory for DecimalColumnEncoder {
    fn estimate_memory_size(&self) -> usize {
        self.data.len() + self.scale.estimate_memory_size()
    }
}

impl ColumnStripeEncoder for DecimalColumnEncoder {
    fn encode_array_with_parent(
        &mut self,
        array: &ArrayRef,
        parent_present: Option<&NullBuffer>,
    ) -> Result<()> {
        let array = array
            .as_any()
            .downcast_ref::<Decimal128Array>()
            .context(UnexpectedSnafu {
                msg: "expected Decimal128Array",
            })?;
        for index in encode_validity(
            &mut self.present,
            &mut self.encoded_count,
            array,
            parent_present,
        ) {
            write_i128_zigzag_varint(&mut self.data, array.value(index));
            self.scale.write_one(self.fixed_scale);
        }
        Ok(())
    }

    fn column_encoding(&self) -> ColumnEncoding {
        ColumnEncoding::DirectV2
    }

    fn finish(&mut self) -> Vec<Stream> {
        let data = std::mem::take(&mut self.data);
        let scale = self.scale.take_inner();
        self.encoded_count = 0;
        finish_with_present(
            &mut self.present,
            [
                Stream {
                    kind: StreamType::Data,
                    bytes: data.into(),
                },
                Stream {
                    kind: StreamType::Secondary,
                    bytes: scale,
                },
            ],
        )
    }
}

pub struct TimestampColumnEncoder {
    unit: TimeUnit,
    data: RleV2Encoder<i64, SignedEncoding>,
    secondary: RleV2Encoder<i64, UnsignedEncoding>,
    present: Option<BooleanEncoder>,
    encoded_count: usize,
}

impl TimestampColumnEncoder {
    pub fn new(unit: TimeUnit) -> Self {
        Self {
            unit,
            data: RleV2Encoder::new(),
            secondary: RleV2Encoder::new(),
            present: None,
            encoded_count: 0,
        }
    }

    fn timestamp_ns(&self, array: &ArrayRef, index: usize) -> i128 {
        match self.unit {
            TimeUnit::Second => {
                i128::from(array.as_primitive::<TimestampSecondType>().value(index)) * 1_000_000_000
            }
            TimeUnit::Millisecond => {
                i128::from(
                    array
                        .as_primitive::<TimestampMillisecondType>()
                        .value(index),
                ) * 1_000_000
            }
            TimeUnit::Microsecond => {
                i128::from(
                    array
                        .as_primitive::<TimestampMicrosecondType>()
                        .value(index),
                ) * 1_000
            }
            TimeUnit::Nanosecond => {
                i128::from(array.as_primitive::<TimestampNanosecondType>().value(index))
            }
        }
    }
}

impl EstimateMemory for TimestampColumnEncoder {
    fn estimate_memory_size(&self) -> usize {
        self.data.estimate_memory_size() + self.secondary.estimate_memory_size()
    }
}

impl ColumnStripeEncoder for TimestampColumnEncoder {
    fn encode_array_with_parent(
        &mut self,
        array: &ArrayRef,
        parent_present: Option<&NullBuffer>,
    ) -> Result<()> {
        for index in encode_validity(
            &mut self.present,
            &mut self.encoded_count,
            array.as_ref(),
            parent_present,
        ) {
            let ns = self.timestamp_ns(array, index);
            let seconds = ns.div_euclid(1_000_000_000);
            let nanos = ns.rem_euclid(1_000_000_000);
            let seconds = i64::try_from(seconds).map_err(|_| {
                UnexpectedSnafu {
                    msg: "timestamp seconds exceed ORC writer range",
                }
                .build()
            })?;
            let nanos = i64::try_from(nanos).map_err(|_| {
                UnexpectedSnafu {
                    msg: "timestamp nanoseconds exceed ORC writer range",
                }
                .build()
            })?;
            self.data
                .write_one(seconds - ORC_EPOCH_UTC_SECONDS_SINCE_UNIX_EPOCH);
            self.secondary.write_one(nanos << 3);
        }
        Ok(())
    }

    fn column_encoding(&self) -> ColumnEncoding {
        ColumnEncoding::DirectV2
    }

    fn finish(&mut self) -> Vec<Stream> {
        let data = self.data.take_inner();
        let secondary = self.secondary.take_inner();
        self.encoded_count = 0;
        finish_with_present(
            &mut self.present,
            [
                Stream {
                    kind: StreamType::Data,
                    bytes: data,
                },
                Stream {
                    kind: StreamType::Secondary,
                    bytes: secondary,
                },
            ],
        )
    }
}

pub struct StructColumnEncoder {
    children: Vec<Box<dyn ColumnStripeEncoder>>,
    present: Option<BooleanEncoder>,
    encoded_count: usize,
}

impl StructColumnEncoder {
    pub fn new(children: Vec<Box<dyn ColumnStripeEncoder>>) -> Self {
        Self {
            children,
            present: None,
            encoded_count: 0,
        }
    }
}

impl EstimateMemory for StructColumnEncoder {
    fn estimate_memory_size(&self) -> usize {
        self.children
            .iter()
            .map(|child| child.estimate_memory_size())
            .sum::<usize>()
            + self
                .present
                .as_ref()
                .map(|p| p.estimate_memory_size())
                .unwrap_or(0)
    }
}

impl ColumnStripeEncoder for StructColumnEncoder {
    fn encode_array_with_parent(
        &mut self,
        array: &ArrayRef,
        parent_present: Option<&NullBuffer>,
    ) -> Result<()> {
        let array = array.as_struct();
        let _ = encode_validity(
            &mut self.present,
            &mut self.encoded_count,
            array,
            parent_present,
        );
        let child_parent = combined_present(array, parent_present);
        for (child_array, child_encoder) in array.columns().iter().zip(&mut self.children) {
            child_encoder.encode_array_with_parent(child_array, child_parent.as_ref())?;
        }
        Ok(())
    }

    fn column_encoding(&self) -> ColumnEncoding {
        ColumnEncoding::DirectV2
    }

    fn finish(&mut self) -> Vec<Stream> {
        self.encoded_count = 0;
        let mut streams = finish_with_present(&mut self.present, []);
        for child in &mut self.children {
            streams.extend(child.finish());
        }
        streams
    }

    fn finish_columns(&mut self, next_column: &mut usize, out: &mut Vec<(usize, Stream)>) {
        let column = *next_column;
        *next_column += 1;
        self.encoded_count = 0;
        for stream in finish_with_present(&mut self.present, []) {
            out.push((column, stream));
        }
        for child in &mut self.children {
            child.finish_columns(next_column, out);
        }
    }
}

pub struct ListColumnEncoder {
    child: Box<dyn ColumnStripeEncoder>,
    lengths: RleV2Encoder<i64, UnsignedEncoding>,
    present: Option<BooleanEncoder>,
    encoded_count: usize,
}

impl ListColumnEncoder {
    pub fn new(child: Box<dyn ColumnStripeEncoder>) -> Self {
        Self {
            child,
            lengths: RleV2Encoder::new(),
            present: None,
            encoded_count: 0,
        }
    }
}

impl EstimateMemory for ListColumnEncoder {
    fn estimate_memory_size(&self) -> usize {
        self.child.estimate_memory_size() + self.lengths.estimate_memory_size()
    }
}

impl ColumnStripeEncoder for ListColumnEncoder {
    fn encode_array_with_parent(
        &mut self,
        array: &ArrayRef,
        parent_present: Option<&NullBuffer>,
    ) -> Result<()> {
        let array = array.as_list::<i32>();
        let values = array.values();
        for index in encode_validity(
            &mut self.present,
            &mut self.encoded_count,
            array,
            parent_present,
        ) {
            let start = array.value_offsets()[index] as usize;
            let end = array.value_offsets()[index + 1] as usize;
            let len = end.saturating_sub(start);
            self.lengths.write_one(len as i64);
            if len != 0 {
                let child = values.slice(start, len);
                self.child.encode_array(&child)?;
            }
        }
        Ok(())
    }

    fn column_encoding(&self) -> ColumnEncoding {
        ColumnEncoding::Direct
    }

    fn finish(&mut self) -> Vec<Stream> {
        let lengths = self.lengths.take_inner();
        self.encoded_count = 0;
        let mut streams = finish_with_present(
            &mut self.present,
            [Stream {
                kind: StreamType::Length,
                bytes: lengths,
            }],
        );
        streams.extend(self.child.finish());
        streams
    }

    fn finish_columns(&mut self, next_column: &mut usize, out: &mut Vec<(usize, Stream)>) {
        let column = *next_column;
        *next_column += 1;
        let lengths = self.lengths.take_inner();
        self.encoded_count = 0;
        for stream in finish_with_present(
            &mut self.present,
            [Stream {
                kind: StreamType::Length,
                bytes: lengths,
            }],
        ) {
            out.push((column, stream));
        }
        self.child.finish_columns(next_column, out);
    }
}

pub struct MapColumnEncoder {
    key: Box<dyn ColumnStripeEncoder>,
    value: Box<dyn ColumnStripeEncoder>,
    lengths: RleV2Encoder<i64, UnsignedEncoding>,
    present: Option<BooleanEncoder>,
    encoded_count: usize,
}

impl MapColumnEncoder {
    pub fn new(key: Box<dyn ColumnStripeEncoder>, value: Box<dyn ColumnStripeEncoder>) -> Self {
        Self {
            key,
            value,
            lengths: RleV2Encoder::new(),
            present: None,
            encoded_count: 0,
        }
    }
}

impl EstimateMemory for MapColumnEncoder {
    fn estimate_memory_size(&self) -> usize {
        self.key.estimate_memory_size()
            + self.value.estimate_memory_size()
            + self.lengths.estimate_memory_size()
    }
}

impl ColumnStripeEncoder for MapColumnEncoder {
    fn encode_array_with_parent(
        &mut self,
        array: &ArrayRef,
        parent_present: Option<&NullBuffer>,
    ) -> Result<()> {
        let array = array.as_map();
        let keys = array.keys();
        let values = array.values();
        for index in encode_validity(
            &mut self.present,
            &mut self.encoded_count,
            array,
            parent_present,
        ) {
            let start = array.value_offsets()[index] as usize;
            let end = array.value_offsets()[index + 1] as usize;
            let len = end.saturating_sub(start);
            self.lengths.write_one(len as i64);
            if len != 0 {
                self.key.encode_array(&keys.slice(start, len))?;
                self.value.encode_array(&values.slice(start, len))?;
            }
        }
        Ok(())
    }

    fn column_encoding(&self) -> ColumnEncoding {
        ColumnEncoding::Direct
    }

    fn finish(&mut self) -> Vec<Stream> {
        let lengths = self.lengths.take_inner();
        self.encoded_count = 0;
        let mut streams = finish_with_present(
            &mut self.present,
            [Stream {
                kind: StreamType::Length,
                bytes: lengths,
            }],
        );
        streams.extend(self.key.finish());
        streams.extend(self.value.finish());
        streams
    }

    fn finish_columns(&mut self, next_column: &mut usize, out: &mut Vec<(usize, Stream)>) {
        let column = *next_column;
        *next_column += 1;
        let lengths = self.lengths.take_inner();
        self.encoded_count = 0;
        for stream in finish_with_present(
            &mut self.present,
            [Stream {
                kind: StreamType::Length,
                bytes: lengths,
            }],
        ) {
            out.push((column, stream));
        }
        self.key.finish_columns(next_column, out);
        self.value.finish_columns(next_column, out);
    }
}

pub type FloatColumnEncoder = PrimitiveColumnEncoder<Float32Type, FloatEncoder<f32>>;
pub type DoubleColumnEncoder = PrimitiveColumnEncoder<Float64Type, FloatEncoder<f64>>;
pub type ByteColumnEncoder = PrimitiveColumnEncoder<Int8Type, ByteRleEncoder>;
pub type Int16ColumnEncoder = PrimitiveColumnEncoder<Int16Type, RleV2Encoder<i16, SignedEncoding>>;
pub type Int32ColumnEncoder = PrimitiveColumnEncoder<Int32Type, RleV2Encoder<i32, SignedEncoding>>;
pub type Int64ColumnEncoder = PrimitiveColumnEncoder<Int64Type, RleV2Encoder<i64, SignedEncoding>>;
pub type Date32ColumnEncoder =
    PrimitiveColumnEncoder<Date32Type, RleV2Encoder<i32, SignedEncoding>>;
pub type StringColumnEncoder = GenericBinaryColumnEncoder<GenericStringType<i32>>;
pub type LargeStringColumnEncoder = GenericBinaryColumnEncoder<GenericStringType<i64>>;
pub type BinaryColumnEncoder = GenericBinaryColumnEncoder<GenericBinaryType<i32>>;
pub type LargeBinaryColumnEncoder = GenericBinaryColumnEncoder<GenericBinaryType<i64>>;
