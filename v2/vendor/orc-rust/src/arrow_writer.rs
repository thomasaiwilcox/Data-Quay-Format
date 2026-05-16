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

use arrow::{
    array::RecordBatch,
    datatypes::{DataType as ArrowDataType, FieldRef, SchemaRef},
};
use prost::Message;
use snafu::{ensure, ResultExt};

use crate::{
    error::{IoSnafu, Result, UnexpectedSnafu},
    memory::EstimateMemory,
    proto,
    writer::stripe::{StripeInformation, StripeWriter},
};

/// Construct an [`ArrowWriter`] to encode [`RecordBatch`]es into a single
/// ORC file.
pub struct ArrowWriterBuilder<W> {
    writer: W,
    schema: SchemaRef,
    batch_size: usize,
    stripe_byte_size: usize,
}

impl<W: Write> ArrowWriterBuilder<W> {
    /// Create a new [`ArrowWriterBuilder`], which will write an ORC file to
    /// the provided writer, with the expected Arrow schema.
    pub fn new(writer: W, schema: SchemaRef) -> Self {
        Self {
            writer,
            schema,
            batch_size: 1024,
            // 64 MiB
            stripe_byte_size: 64 * 1024 * 1024,
        }
    }

    /// Batch size controls the encoding behaviour, where `batch_size` values
    /// are encoded at a time. Default is `1024`.
    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size;
        self
    }

    /// The approximate size of stripes. Default is `64MiB`.
    pub fn with_stripe_byte_size(mut self, stripe_byte_size: usize) -> Self {
        self.stripe_byte_size = stripe_byte_size;
        self
    }

    /// Construct an [`ArrowWriter`] ready to encode [`RecordBatch`]es into
    /// an ORC file.
    pub fn try_build(mut self) -> Result<ArrowWriter<W>> {
        // Required magic "ORC" bytes at start of file
        self.writer.write_all(b"ORC").context(IoSnafu)?;
        let writer = StripeWriter::new(self.writer, &self.schema);
        Ok(ArrowWriter {
            writer,
            schema: self.schema,
            batch_size: self.batch_size,
            stripe_byte_size: self.stripe_byte_size,
            written_stripes: vec![],
            // Accounting for the 3 magic bytes above
            total_bytes_written: 3,
        })
    }
}

/// Encodes [`RecordBatch`]es into an ORC file. Will encode `batch_size` rows
/// at a time into a stripe, flushing the stripe to the underlying writer when
/// it's estimated memory footprint exceeds the configures `stripe_byte_size`.
pub struct ArrowWriter<W> {
    writer: StripeWriter<W>,
    schema: SchemaRef,
    batch_size: usize,
    stripe_byte_size: usize,
    written_stripes: Vec<StripeInformation>,
    /// Used to keep track of progress in file so far (instead of needing Seek on the writer)
    total_bytes_written: u64,
}

impl<W: Write> ArrowWriter<W> {
    /// Encode the provided batch at `batch_size` rows at a time, flushing any
    /// stripes that exceed the configured stripe size.
    pub fn write(&mut self, batch: &RecordBatch) -> Result<()> {
        ensure!(
            batch.schema() == self.schema,
            UnexpectedSnafu {
                msg: "RecordBatch doesn't match expected schema"
            }
        );

        for offset in (0..batch.num_rows()).step_by(self.batch_size) {
            let length = self.batch_size.min(batch.num_rows() - offset);
            let batch = batch.slice(offset, length);
            self.writer.encode_batch(&batch)?;

            // TODO: be able to flush whilst writing a batch (instead of between batches)
            // Flush stripe when it exceeds estimated configured size
            if self.writer.estimate_memory_size() > self.stripe_byte_size {
                self.flush_stripe()?;
            }
        }
        Ok(())
    }

    /// Flush any buffered data that hasn't been written, and write the stripe
    /// footer metadata.
    pub fn flush_stripe(&mut self) -> Result<()> {
        let info = self.writer.finish_stripe(self.total_bytes_written)?;
        self.total_bytes_written += info.total_byte_size();
        self.written_stripes.push(info);
        Ok(())
    }

    /// Flush the current stripe if it is still in progress, and write the tail
    /// metadata and close the writer.
    pub fn close(mut self) -> Result<()> {
        // Flush in-progress stripe
        if self.writer.row_count > 0 {
            self.flush_stripe()?;
        }
        let footer = serialize_footer(&self.written_stripes, &self.schema);
        let footer = footer.encode_to_vec();
        let postscript = serialize_postscript(footer.len() as u64);
        let postscript = postscript.encode_to_vec();
        let postscript_len = postscript.len() as u8;

        let mut writer = self.writer.finish();
        writer.write_all(&footer).context(IoSnafu)?;
        writer.write_all(&postscript).context(IoSnafu)?;
        // Postscript length as last byte
        writer.write_all(&[postscript_len]).context(IoSnafu)?;

        // TODO: return file metadata
        Ok(())
    }
}

fn serialize_schema(schema: &SchemaRef) -> Vec<proto::Type> {
    let mut types = vec![proto::Type::default()];
    let subtypes = schema
        .fields()
        .iter()
        .map(|field| append_type(field, &mut types))
        .collect::<Vec<_>>();
    let field_names = schema
        .fields()
        .iter()
        .map(|f| f.name().to_owned())
        .collect();
    types[0] = proto::Type {
        kind: Some(proto::r#type::Kind::Struct.into()),
        subtypes,
        field_names,
        maximum_length: None,
        precision: None,
        scale: None,
        attributes: vec![],
    };
    types
}

fn append_type(field: &FieldRef, types: &mut Vec<proto::Type>) -> u32 {
    let id = types.len() as u32;
    types.push(proto::Type::default());
    let ty = match field.data_type() {
        ArrowDataType::Float32 => proto_type(proto::r#type::Kind::Float),
        ArrowDataType::Float64 => proto_type(proto::r#type::Kind::Double),
        ArrowDataType::Int8 => proto_type(proto::r#type::Kind::Byte),
        ArrowDataType::Int16 => proto_type(proto::r#type::Kind::Short),
        ArrowDataType::Int32 => proto_type(proto::r#type::Kind::Int),
        ArrowDataType::Int64 => proto_type(proto::r#type::Kind::Long),
        ArrowDataType::UInt8 | ArrowDataType::UInt16 | ArrowDataType::UInt32 => {
            let mut ty = proto_type(proto::r#type::Kind::Long);
            ty.attributes.push(string_pair(
                "cove.logical_type",
                match field.data_type() {
                    ArrowDataType::UInt8 => "uint8",
                    ArrowDataType::UInt16 => "uint16",
                    ArrowDataType::UInt32 => "uint32",
                    _ => unreachable!(),
                },
            ));
            ty
        }
        ArrowDataType::UInt64 => {
            let mut ty = proto_type(proto::r#type::Kind::Decimal);
            ty.precision = Some(20);
            ty.scale = Some(0);
            ty.attributes
                .push(string_pair("cove.logical_type", "uint64"));
            ty
        }
        ArrowDataType::Decimal128(precision, scale) => {
            let mut ty = proto_type(proto::r#type::Kind::Decimal);
            ty.precision = Some(*precision as u32);
            ty.scale = Some(*scale as u32);
            ty
        }
        ArrowDataType::Date32 => proto_type(proto::r#type::Kind::Date),
        ArrowDataType::Timestamp(_, Some(tz)) if tz.as_ref() == "UTC" => {
            proto_type(proto::r#type::Kind::TimestampInstant)
        }
        ArrowDataType::Timestamp(_, _) => proto_type(proto::r#type::Kind::Timestamp),
        ArrowDataType::Utf8 | ArrowDataType::LargeUtf8 => proto_type(proto::r#type::Kind::String),
        ArrowDataType::Binary | ArrowDataType::LargeBinary => {
            proto_type(proto::r#type::Kind::Binary)
        }
        ArrowDataType::FixedSizeBinary(width) => {
            let mut ty = proto_type(proto::r#type::Kind::Binary);
            ty.attributes.push(string_pair(
                "cove.fixed_size_binary_width",
                &width.to_string(),
            ));
            ty
        }
        ArrowDataType::Boolean => proto_type(proto::r#type::Kind::Boolean),
        ArrowDataType::Struct(fields) => {
            let subtypes = fields
                .iter()
                .map(|field| append_type(field, types))
                .collect::<Vec<_>>();
            let field_names = fields.iter().map(|field| field.name().clone()).collect();
            proto::Type {
                kind: Some(proto::r#type::Kind::Struct.into()),
                subtypes,
                field_names,
                ..Default::default()
            }
        }
        ArrowDataType::List(child) => {
            let child_id = append_type(child, types);
            proto::Type {
                kind: Some(proto::r#type::Kind::List.into()),
                subtypes: vec![child_id],
                ..Default::default()
            }
        }
        ArrowDataType::Map(entries, _) => {
            let ArrowDataType::Struct(fields) = entries.data_type() else {
                unreachable!("Arrow Map child must be a struct")
            };
            let key_id = append_type(&fields[0], types);
            let value_id = append_type(&fields[1], types);
            proto::Type {
                kind: Some(proto::r#type::Kind::Map.into()),
                subtypes: vec![key_id, value_id],
                ..Default::default()
            }
        }
        _ => unimplemented!("unsupported datatype"),
    };
    types[id as usize] = ty;
    id
}

fn proto_type(kind: proto::r#type::Kind) -> proto::Type {
    proto::Type {
        kind: Some(kind.into()),
        ..Default::default()
    }
}

fn string_pair(key: &str, value: &str) -> proto::StringPair {
    proto::StringPair {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

fn serialize_footer(stripes: &[StripeInformation], schema: &SchemaRef) -> proto::Footer {
    let body_length = stripes
        .iter()
        .map(|s| s.index_length + s.data_length + s.footer_length)
        .sum::<u64>();
    let number_of_rows = stripes.iter().map(|s| s.row_count as u64).sum::<u64>();
    let stripes = stripes.iter().map(From::from).collect();
    let types = serialize_schema(schema);
    proto::Footer {
        header_length: Some(3),
        content_length: Some(body_length + 3),
        stripes,
        types,
        metadata: vec![],
        number_of_rows: Some(number_of_rows),
        statistics: vec![],
        row_index_stride: None,
        writer: Some(u32::MAX),
        encryption: None,
        calendar: None,
        software_version: None,
    }
}

fn serialize_postscript(footer_length: u64) -> proto::PostScript {
    proto::PostScript {
        footer_length: Some(footer_length),
        compression: Some(proto::CompressionKind::None.into()), // TODO: support compression
        compression_block_size: None,
        version: vec![0, 12],
        metadata_length: Some(0),       // TODO: statistics
        writer_version: Some(u32::MAX), // TODO: check which version to use
        stripe_statistics_length: None,
        magic: Some("ORC".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow::{
        array::{
            Array, ArrayRef, BinaryArray, BooleanArray, Date32Array, Decimal128Array,
            FixedSizeBinaryArray, Float32Array, Float64Array, Int16Array, Int32Array, Int64Array,
            Int8Array, LargeBinaryArray, LargeStringArray, ListArray, MapArray, RecordBatchReader,
            StringArray, StructArray, TimestampMicrosecondArray, UInt32Array, UInt64Array,
        },
        buffer::{NullBuffer, OffsetBuffer},
        compute::concat_batches,
        datatypes::{DataType as ArrowDataType, Field, Fields, Schema},
    };
    use bytes::Bytes;

    use crate::{stripe::Stripe, ArrowReaderBuilder};

    use super::*;

    fn roundtrip(batches: &[RecordBatch]) -> Vec<RecordBatch> {
        let mut f = vec![];
        let mut writer = ArrowWriterBuilder::new(&mut f, batches[0].schema())
            .try_build()
            .unwrap();
        for batch in batches {
            writer.write(batch).unwrap();
        }
        writer.close().unwrap();

        let f = Bytes::from(f);
        let reader = ArrowReaderBuilder::try_new(f).unwrap().build();
        reader.collect::<Result<Vec<_>, _>>().unwrap()
    }

    #[test]
    fn test_roundtrip_write() {
        let f32_array = Arc::new(Float32Array::from(vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0]));
        let f64_array = Arc::new(Float64Array::from(vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0]));
        let int8_array = Arc::new(Int8Array::from(vec![0, 1, 2, 3, 4, 5, 6]));
        let int16_array = Arc::new(Int16Array::from(vec![0, 1, 2, 3, 4, 5, 6]));
        let int32_array = Arc::new(Int32Array::from(vec![0, 1, 2, 3, 4, 5, 6]));
        let int64_array = Arc::new(Int64Array::from(vec![0, 1, 2, 3, 4, 5, 6]));
        let utf8_array = Arc::new(StringArray::from(vec![
            "Hello",
            "there",
            "楡井希実",
            "💯",
            "ORC",
            "",
            "123",
        ]));
        let binary_array = Arc::new(BinaryArray::from(vec![
            "Hello".as_bytes(),
            "there".as_bytes(),
            "楡井希実".as_bytes(),
            "💯".as_bytes(),
            "ORC".as_bytes(),
            "".as_bytes(),
            "123".as_bytes(),
        ]));
        let boolean_array = Arc::new(BooleanArray::from(vec![
            true, false, true, false, true, true, false,
        ]));
        let schema = Schema::new(vec![
            Field::new("f32", ArrowDataType::Float32, false),
            Field::new("f64", ArrowDataType::Float64, false),
            Field::new("int8", ArrowDataType::Int8, false),
            Field::new("int16", ArrowDataType::Int16, false),
            Field::new("int32", ArrowDataType::Int32, false),
            Field::new("int64", ArrowDataType::Int64, false),
            Field::new("utf8", ArrowDataType::Utf8, false),
            Field::new("binary", ArrowDataType::Binary, false),
            Field::new("boolean", ArrowDataType::Boolean, false),
        ]);

        let batch = RecordBatch::try_new(
            Arc::new(schema),
            vec![
                f32_array,
                f64_array,
                int8_array,
                int16_array,
                int32_array,
                int64_array,
                utf8_array,
                binary_array,
                boolean_array,
            ],
        )
        .unwrap();

        let rows = roundtrip(std::slice::from_ref(&batch));
        assert_eq!(batch, rows[0]);
    }

    #[test]
    fn test_roundtrip_write_extended_scalar_types() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("u32", ArrowDataType::UInt32, true),
            Field::new("u64", ArrowDataType::UInt64, true),
            Field::new("decimal", ArrowDataType::Decimal128(20, 2), true),
            Field::new("date32", ArrowDataType::Date32, true),
            Field::new(
                "ts_us",
                ArrowDataType::Timestamp(arrow::datatypes::TimeUnit::Microsecond, None),
                true,
            ),
            Field::new("fixed", ArrowDataType::FixedSizeBinary(3), true),
        ]));
        let fixed = FixedSizeBinaryArray::try_from_sparse_iter_with_size(
            vec![Some(b"abc".as_slice()), None, Some(b"xyz".as_slice())].into_iter(),
            3,
        )
        .unwrap();
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(UInt32Array::from(vec![Some(1), None, Some(u32::MAX)])) as ArrayRef,
                Arc::new(UInt64Array::from(vec![Some(1), None, Some(u64::MAX)])) as ArrayRef,
                Arc::new(
                    Decimal128Array::from(vec![Some(12345), None, Some(-999)])
                        .with_precision_and_scale(20, 2)
                        .unwrap(),
                ) as ArrayRef,
                Arc::new(Date32Array::from(vec![Some(20_000), None, Some(20_001)])) as ArrayRef,
                Arc::new(TimestampMicrosecondArray::from(vec![
                    Some(1_700_000_000_000_000),
                    None,
                    Some(1_700_000_000_000_001),
                ])) as ArrayRef,
                Arc::new(fixed) as ArrayRef,
            ],
        )
        .unwrap();

        let rows = roundtrip(std::slice::from_ref(&batch));
        assert_eq!(rows[0].num_rows(), 3);
        assert_eq!(rows[0].num_columns(), 6);
    }

    #[test]
    fn test_roundtrip_write_nested_types() {
        let list_field = Arc::new(Field::new("item", ArrowDataType::Int64, true));
        let list = ListArray::try_new(
            Arc::clone(&list_field),
            OffsetBuffer::new(vec![0, 2, 2, 3].into()),
            Arc::new(Int64Array::from(vec![Some(10), Some(20), Some(30)])) as ArrayRef,
            None,
        )
        .unwrap();
        let schema = Arc::new(Schema::new(vec![Field::new(
            "items",
            ArrowDataType::List(Arc::clone(&list_field)),
            true,
        )]));
        let batch = RecordBatch::try_new(schema, vec![Arc::new(list) as ArrayRef]).unwrap();
        let rows = roundtrip(std::slice::from_ref(&batch));
        assert_eq!(rows[0].num_rows(), 3);
        assert_eq!(rows[0].num_columns(), 1);
    }

    #[test]
    fn test_roundtrip_write_struct_and_map_types() {
        let list_field = Arc::new(Field::new("item", ArrowDataType::Int64, true));
        let list = ListArray::try_new(
            Arc::clone(&list_field),
            OffsetBuffer::new(vec![0, 2, 2, 3].into()),
            Arc::new(Int64Array::from(vec![Some(10), Some(20), Some(30)])) as ArrayRef,
            None,
        )
        .unwrap();
        let struct_fields = Fields::from(vec![
            Field::new("active", ArrowDataType::Boolean, true),
            Field::new("level", ArrowDataType::Int64, true),
        ]);
        let struct_array = StructArray::try_new(
            struct_fields.clone(),
            vec![
                Arc::new(BooleanArray::from(vec![
                    Some(true),
                    Some(true),
                    Some(false),
                ])) as ArrayRef,
                Arc::new(Int64Array::from(vec![Some(7), Some(0), Some(8)])) as ArrayRef,
            ],
            None,
        )
        .unwrap();
        let entry_fields = Fields::from(vec![
            Field::new("key", ArrowDataType::Utf8, false),
            Field::new("value", ArrowDataType::Int64, true),
        ]);
        let entries = StructArray::try_new(
            entry_fields.clone(),
            vec![
                Arc::new(StringArray::from(vec!["logic", "math", "logic"])) as ArrayRef,
                Arc::new(Int64Array::from(vec![Some(100), Some(99), Some(101)])) as ArrayRef,
            ],
            None,
        )
        .unwrap();
        let map_field = Arc::new(Field::new(
            "entries",
            ArrowDataType::Struct(entry_fields),
            false,
        ));
        let map = MapArray::try_new(
            map_field,
            OffsetBuffer::new(vec![0, 2, 2, 3].into()),
            entries,
            None,
            false,
        )
        .unwrap();
        let schema = Arc::new(Schema::new(vec![
            Field::new("items", ArrowDataType::List(Arc::clone(&list_field)), true),
            Field::new("profile", ArrowDataType::Struct(struct_fields), true),
            Field::new_map(
                "scores",
                "entries",
                Arc::new(Field::new("key", ArrowDataType::Utf8, false)),
                Arc::new(Field::new("value", ArrowDataType::Int64, true)),
                false,
                true,
            ),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(list) as ArrayRef,
                Arc::new(struct_array) as ArrayRef,
                Arc::new(map) as ArrayRef,
            ],
        )
        .unwrap();

        let rows = roundtrip(std::slice::from_ref(&batch));
        assert_eq!(rows[0].num_rows(), 3);
        assert_eq!(rows[0].num_columns(), 3);
        assert!(matches!(
            rows[0].schema().field(0).data_type(),
            ArrowDataType::List(_)
        ));
        assert!(matches!(
            rows[0].schema().field(1).data_type(),
            ArrowDataType::Struct(_)
        ));
        assert!(matches!(
            rows[0].schema().field(2).data_type(),
            ArrowDataType::Map(_, _)
        ));
    }

    #[test]
    fn test_roundtrip_write_large_type() {
        let large_utf8_array = Arc::new(LargeStringArray::from(vec![
            "Hello",
            "there",
            "楡井希実",
            "💯",
            "ORC",
            "",
            "123",
        ]));
        let large_binary_array = Arc::new(LargeBinaryArray::from(vec![
            "Hello".as_bytes(),
            "there".as_bytes(),
            "楡井希実".as_bytes(),
            "💯".as_bytes(),
            "ORC".as_bytes(),
            "".as_bytes(),
            "123".as_bytes(),
        ]));
        let schema = Schema::new(vec![
            Field::new("large_utf8", ArrowDataType::LargeUtf8, false),
            Field::new("large_binary", ArrowDataType::LargeBinary, false),
        ]);
        let batch =
            RecordBatch::try_new(Arc::new(schema), vec![large_utf8_array, large_binary_array])
                .unwrap();

        let rows = roundtrip(&[batch]);

        // Currently we read all String/Binary columns from ORC as plain StringArray/BinaryArray
        let utf8_array = Arc::new(StringArray::from(vec![
            "Hello",
            "there",
            "楡井希実",
            "💯",
            "ORC",
            "",
            "123",
        ]));
        let binary_array = Arc::new(BinaryArray::from(vec![
            "Hello".as_bytes(),
            "there".as_bytes(),
            "楡井希実".as_bytes(),
            "💯".as_bytes(),
            "ORC".as_bytes(),
            "".as_bytes(),
            "123".as_bytes(),
        ]));
        let schema = Schema::new(vec![
            Field::new("large_utf8", ArrowDataType::Utf8, false),
            Field::new("large_binary", ArrowDataType::Binary, false),
        ]);
        let batch = RecordBatch::try_new(Arc::new(schema), vec![utf8_array, binary_array]).unwrap();
        assert_eq!(batch, rows[0]);
    }

    #[test]
    fn test_write_small_stripes() {
        // Set small stripe size to ensure writing across multiple stripes works
        let data: Vec<i64> = (0..1_000_000).collect();
        let int64_array = Arc::new(Int64Array::from(data));
        let schema = Schema::new(vec![Field::new("int64", ArrowDataType::Int64, true)]);

        let batch = RecordBatch::try_new(Arc::new(schema), vec![int64_array]).unwrap();

        let mut f = vec![];
        let mut writer = ArrowWriterBuilder::new(&mut f, batch.schema())
            .with_stripe_byte_size(256)
            .try_build()
            .unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();

        let f = Bytes::from(f);
        let reader = ArrowReaderBuilder::try_new(f).unwrap().build();
        let schema = reader.schema();
        // Current reader doesn't read a batch across stripe boundaries, so we expect
        // more than one batch to prove multiple stripes are being written here
        let rows = reader.collect::<Result<Vec<_>, _>>().unwrap();
        assert!(
            rows.len() > 1,
            "must have written more than 1 stripe (each stripe read as separate recordbatch)"
        );
        let actual = concat_batches(&schema, rows.iter()).unwrap();
        assert_eq!(batch, actual);
    }

    #[test]
    fn test_write_inconsistent_null_buffers() {
        // When writing arrays where null buffer can appear/disappear between writes
        let schema = Arc::new(Schema::new(vec![Field::new(
            "int64",
            ArrowDataType::Int64,
            true,
        )]));

        // Ensure first batch has array with no null buffer
        let array_no_nulls = Arc::new(Int64Array::from(vec![1, 2, 3]));
        assert!(array_no_nulls.nulls().is_none());
        // But subsequent batch has array with null buffer
        let array_with_nulls = Arc::new(Int64Array::from(vec![None, Some(4), None]));
        assert!(array_with_nulls.nulls().is_some());

        let batch1 = RecordBatch::try_new(schema.clone(), vec![array_no_nulls]).unwrap();
        let batch2 = RecordBatch::try_new(schema.clone(), vec![array_with_nulls]).unwrap();

        // ORC writer should be able to handle this gracefully
        let expected_array = Arc::new(Int64Array::from(vec![
            Some(1),
            Some(2),
            Some(3),
            None,
            Some(4),
            None,
        ]));
        let expected_batch = RecordBatch::try_new(schema, vec![expected_array]).unwrap();

        let rows = roundtrip(&[batch1, batch2]);
        assert_eq!(expected_batch, rows[0]);
    }

    #[test]
    fn test_empty_null_buffers() {
        // Create an ORC file with present streams, but which have no nulls.
        // When this file is read then the resulting Arrow arrays show have
        // NO null buffer, even though there is a present stream.
        let schema = Arc::new(Schema::new(vec![Field::new(
            "int64",
            ArrowDataType::Int64,
            true,
        )]));

        // Array with null buffer but has no nulls
        let array_empty_nulls = Arc::new(Int64Array::from_iter_values_with_nulls(
            vec![1],
            Some(NullBuffer::from_iter(vec![true])),
        ));
        assert!(array_empty_nulls.nulls().is_some());
        assert!(array_empty_nulls.null_count() == 0);

        let batch = RecordBatch::try_new(schema, vec![array_empty_nulls]).unwrap();

        // Encoding to bytes
        let mut f = vec![];
        let mut writer = ArrowWriterBuilder::new(&mut f, batch.schema())
            .try_build()
            .unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();
        let mut f = Bytes::from(f);
        let builder = ArrowReaderBuilder::try_new(f.clone()).unwrap();

        // Ensure the ORC file we wrote indeed has a present stream
        let stripe = Stripe::new(
            &mut f,
            &builder.file_metadata,
            builder.file_metadata().root_data_type(),
            &builder.file_metadata().stripe_metadatas()[0],
        )
        .unwrap();
        assert_eq!(stripe.columns().len(), 1);
        // Make sure we're getting the right column
        assert_eq!(stripe.columns()[0].name(), "int64");
        // Then check present stream
        let present_stream = stripe
            .stream_map()
            .get_opt(&stripe.columns()[0], proto::stream::Kind::Present);
        assert!(present_stream.is_some());

        // Decoding from bytes
        let reader = builder.build();
        let rows = reader.collect::<Result<Vec<_>, _>>().unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].num_columns(), 1);
        // Ensure read array has no null buffer
        assert!(rows[0].column(0).nulls().is_none());
    }
}
