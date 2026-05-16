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

use std::io::Write;

use arrow::array::RecordBatch;
use arrow::datatypes::{DataType as ArrowDataType, FieldRef, SchemaRef};
use prost::Message;
use snafu::ResultExt;

use crate::error::{IoSnafu, Result};
use crate::memory::EstimateMemory;
use crate::proto;

use super::column::{
    BinaryColumnEncoder, BooleanColumnEncoder, ByteColumnEncoder, ColumnStripeEncoder,
    Date32ColumnEncoder, DecimalColumnEncoder, DoubleColumnEncoder, FixedSizeBinaryColumnEncoder,
    FloatColumnEncoder, Int16ColumnEncoder, Int32ColumnEncoder, Int64ColumnEncoder,
    LargeBinaryColumnEncoder, LargeStringColumnEncoder, ListColumnEncoder, MapColumnEncoder,
    StringColumnEncoder, StructColumnEncoder, TimestampColumnEncoder, UInt64DecimalColumnEncoder,
    UnsignedLongColumnEncoder, UnsignedLongWidth,
};
use super::{ColumnEncoding, StreamType};

#[derive(Copy, Clone, Eq, Debug, PartialEq)]
pub struct StripeInformation {
    pub start_offset: u64,
    pub index_length: u64,
    pub data_length: u64,
    pub footer_length: u64,
    pub row_count: usize,
}

impl StripeInformation {
    pub fn total_byte_size(&self) -> u64 {
        self.index_length + self.data_length + self.footer_length
    }
}

impl From<&StripeInformation> for proto::StripeInformation {
    fn from(value: &StripeInformation) -> Self {
        proto::StripeInformation {
            offset: Some(value.start_offset),
            index_length: Some(value.index_length),
            data_length: Some(value.data_length),
            footer_length: Some(value.footer_length),
            number_of_rows: Some(value.row_count as u64),
            encrypt_stripe_id: None,
            encrypted_local_keys: vec![],
        }
    }
}

/// Encode a stripe. Will encode columns into an in-memory buffer before flushing
/// entire stripe to the underlying writer.
pub struct StripeWriter<W> {
    writer: W,
    /// Top-level encoders; nested children are owned recursively.
    columns: Vec<Box<dyn ColumnStripeEncoder>>,
    column_encodings: Vec<ColumnEncoding>,
    pub row_count: usize,
}

impl<W> EstimateMemory for StripeWriter<W> {
    /// Used to estimate when stripe size is over threshold and should be flushed
    /// to the writer and a new stripe started.
    fn estimate_memory_size(&self) -> usize {
        self.columns.iter().map(|c| c.estimate_memory_size()).sum()
    }
}

impl<W: Write> StripeWriter<W> {
    pub fn new(writer: W, schema: &SchemaRef) -> Self {
        let columns = schema.fields().iter().map(create_encoder).collect();
        let mut column_encodings = vec![ColumnEncoding::Direct];
        for field in schema.fields() {
            append_column_encodings(field, &mut column_encodings);
        }
        Self {
            writer,
            columns,
            column_encodings,
            row_count: 0,
        }
    }

    /// Attempt to encode entire [`RecordBatch`]. Relies on caller slicing the batch
    /// to required batch size.
    pub fn encode_batch(&mut self, batch: &RecordBatch) -> Result<()> {
        // TODO: consider how to handle nested types (including parent nullability)
        for (array, encoder) in batch.columns().iter().zip(self.columns.iter_mut()) {
            encoder.encode_array(array)?;
        }
        self.row_count += batch.num_rows();
        Ok(())
    }

    /// Flush streams to the writer, and write the stripe footer to finish
    /// the stripe. After this, the [`StripeWriter`] will be reset and ready
    /// to write a fresh new stripe.
    ///
    /// `start_offset` is used to manually keep track of position in the writer (instead
    /// of relying on Seek).
    pub fn finish_stripe(&mut self, start_offset: u64) -> Result<StripeInformation> {
        let column_encodings = self.column_encodings.iter().map(From::from).collect();

        // Root type won't have any streams
        let mut written_streams = vec![];
        let mut data_length = 0;
        let mut stream_columns = Vec::new();
        let mut next_column = 1usize;
        for c in self.columns.iter_mut() {
            c.finish_columns(&mut next_column, &mut stream_columns);
        }
        for (column, stream) in stream_columns {
            let (kind, bytes) = stream.into_parts();
            let length = bytes.len();
            self.writer.write_all(&bytes).context(IoSnafu)?;
            data_length += length as u64;
            written_streams.push(WrittenStream {
                kind,
                column,
                length,
            });
        }
        let streams = written_streams.iter().map(From::from).collect();
        let stripe_footer = proto::StripeFooter {
            streams,
            columns: column_encodings,
            writer_timezone: None,
            encryption: vec![],
        };

        let footer_bytes = stripe_footer.encode_to_vec();
        let footer_length = footer_bytes.len() as u64;
        let row_count = self.row_count;
        self.writer.write_all(&footer_bytes).context(IoSnafu)?;

        // Reset state for next stripe
        self.row_count = 0;

        Ok(StripeInformation {
            start_offset,
            index_length: 0,
            data_length,
            footer_length,
            row_count,
        })
    }

    /// When finished writing all stripes, return the inner writer.
    pub fn finish(self) -> W {
        self.writer
    }
}

fn create_encoder(field: &FieldRef) -> Box<dyn ColumnStripeEncoder> {
    match field.data_type() {
        ArrowDataType::Float32 => Box::new(FloatColumnEncoder::new(ColumnEncoding::Direct)),
        ArrowDataType::Float64 => Box::new(DoubleColumnEncoder::new(ColumnEncoding::Direct)),
        ArrowDataType::Int8 => Box::new(ByteColumnEncoder::new(ColumnEncoding::Direct)),
        ArrowDataType::Int16 => Box::new(Int16ColumnEncoder::new(ColumnEncoding::DirectV2)),
        ArrowDataType::Int32 => Box::new(Int32ColumnEncoder::new(ColumnEncoding::DirectV2)),
        ArrowDataType::Int64 => Box::new(Int64ColumnEncoder::new(ColumnEncoding::DirectV2)),
        ArrowDataType::UInt8 => Box::new(UnsignedLongColumnEncoder::new(UnsignedLongWidth::U8)),
        ArrowDataType::UInt16 => Box::new(UnsignedLongColumnEncoder::new(UnsignedLongWidth::U16)),
        ArrowDataType::UInt32 => Box::new(UnsignedLongColumnEncoder::new(UnsignedLongWidth::U32)),
        ArrowDataType::UInt64 => Box::new(UInt64DecimalColumnEncoder::new()),
        ArrowDataType::Decimal128(_, scale) => Box::new(DecimalColumnEncoder::new(*scale as i32)),
        ArrowDataType::Date32 => Box::new(Date32ColumnEncoder::new(ColumnEncoding::DirectV2)),
        ArrowDataType::Timestamp(unit, _) => Box::new(TimestampColumnEncoder::new(*unit)),
        ArrowDataType::Utf8 => Box::new(StringColumnEncoder::new()),
        ArrowDataType::LargeUtf8 => Box::new(LargeStringColumnEncoder::new()),
        ArrowDataType::Binary => Box::new(BinaryColumnEncoder::new()),
        ArrowDataType::LargeBinary => Box::new(LargeBinaryColumnEncoder::new()),
        ArrowDataType::FixedSizeBinary(_) => Box::new(FixedSizeBinaryColumnEncoder::new()),
        ArrowDataType::Boolean => Box::new(BooleanColumnEncoder::new()),
        ArrowDataType::Struct(fields) => Box::new(StructColumnEncoder::new(
            fields.iter().map(create_encoder).collect(),
        )),
        ArrowDataType::List(child) => Box::new(ListColumnEncoder::new(create_encoder(child))),
        ArrowDataType::Map(entries, _) => {
            let ArrowDataType::Struct(fields) = entries.data_type() else {
                unreachable!("Arrow Map child must be a struct")
            };
            Box::new(MapColumnEncoder::new(
                create_encoder(&fields[0]),
                create_encoder(&fields[1]),
            ))
        }
        _ => unimplemented!("unsupported datatype"),
    }
}

fn append_column_encodings(field: &FieldRef, out: &mut Vec<ColumnEncoding>) {
    match field.data_type() {
        ArrowDataType::Float32
        | ArrowDataType::Float64
        | ArrowDataType::Int8
        | ArrowDataType::Boolean
        | ArrowDataType::Struct(_) => out.push(ColumnEncoding::Direct),
        ArrowDataType::Utf8
        | ArrowDataType::LargeUtf8
        | ArrowDataType::Binary
        | ArrowDataType::LargeBinary
        | ArrowDataType::FixedSizeBinary(_)
        | ArrowDataType::Int16
        | ArrowDataType::Int32
        | ArrowDataType::Int64
        | ArrowDataType::UInt8
        | ArrowDataType::UInt16
        | ArrowDataType::UInt32
        | ArrowDataType::UInt64
        | ArrowDataType::Decimal128(_, _)
        | ArrowDataType::Date32
        | ArrowDataType::Timestamp(_, _)
        | ArrowDataType::List(_)
        | ArrowDataType::Map(_, _) => out.push(ColumnEncoding::DirectV2),
        _ => out.push(ColumnEncoding::DirectV2),
    }
    match field.data_type() {
        ArrowDataType::Struct(fields) => {
            for child in fields {
                append_column_encodings(child, out);
            }
        }
        ArrowDataType::List(child) => append_column_encodings(child, out),
        ArrowDataType::Map(entries, _) => {
            if let ArrowDataType::Struct(fields) = entries.data_type() {
                append_column_encodings(&fields[0], out);
                append_column_encodings(&fields[1], out);
            }
        }
        _ => {}
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
struct WrittenStream {
    kind: StreamType,
    column: usize,
    length: usize,
}

impl From<&WrittenStream> for proto::Stream {
    fn from(value: &WrittenStream) -> Self {
        let kind = proto::stream::Kind::from(value.kind);
        proto::Stream {
            kind: Some(kind.into()),
            column: Some(value.column as u32),
            length: Some(value.length as u64),
        }
    }
}
