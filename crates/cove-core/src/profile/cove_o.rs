//! Spec §55–§63 — COVE-O object-temporal profile.
//!
//! Tracks an append-only history of object records. Each record carries
//! `(timestamp_us, csn, branch_key, goid, record_id)` and Spec §58 mandates
//! that rows be sorted by that lexicographic key. The trust chain (Spec §63)
//! hashes canonical logical values so that re-encoding a file with new
//! FileCodes preserves the chain.

mod bloom;
mod object_catalog;
mod segment;
mod segment_index;
mod temporal;
mod trust;

pub const TEMPORAL_SEGMENT_HEADER_LEN: usize = 96;
pub const TEMPORAL_SEGMENT_INDEX_ENTRY_LEN: usize = 112;
pub const TEMPORAL_ROW_ENTRY_LEN: usize = 68;
pub const TEMPORAL_BLOOM_ENTRY_LEN: usize = 40;
pub const TRUST_MANIFEST_ENTRY_LEN: usize = 40;

pub use bloom::{TemporalBloomEntryV1, TemporalBloomIndex};
pub use object_catalog::{
    ObjectTypeCatalog, ObjectTypeEntryV1, PropertyEntryV1, OBJECT_TYPE_FLAG_ASSOCIATION_OBJECT,
    OBJECT_TYPE_FLAG_ENTITY_OBJECT, OBJECT_TYPE_FLAG_EVENT_OBJECT,
    OBJECT_TYPE_FLAG_EVIDENCE_OBJECT, OBJECT_TYPE_FLAG_LINK_OBJECT,
    OBJECT_TYPE_FLAG_PROJECTION_OBJECT, PROPERTY_FLAG_ASSOCIATION_FROM_GOID,
    PROPERTY_FLAG_ASSOCIATION_OBSERVED_AT, PROPERTY_FLAG_ASSOCIATION_TO_GOID,
    PROPERTY_FLAG_ASSOCIATION_TYPE, PROPERTY_FLAG_ASSOCIATION_VALID_FROM,
    PROPERTY_FLAG_ASSOCIATION_VALID_TO, PROPERTY_FLAG_BOOL_DECLARED_NUMERIC,
    PROPERTY_FLAG_EVIDENCE_REF, PROPERTY_FLAG_MAPPING_RULE_REF,
};
pub(crate) use segment::{
    validate_temporal_property_page_elision_features, validate_temporal_property_stats_only_page,
};
pub use segment::{
    CoveRecordRefV1, TemporalPropertyColumn, TemporalPropertyPage, TemporalRowEntryV1,
    TemporalSegmentData, TemporalSegmentHeaderV1,
};
pub use segment_index::{TemporalSegmentIndex, TemporalSegmentIndexEntryV1};
pub use temporal::{validate_self_contained, validate_temporal_order, RecordKind, TemporalRowKey};
pub use trust::{TrustManifest, TrustManifestEntryV1};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        checksum,
        constants::{
            CoveLogicalType, CovePhysicalKind, FEATURE_OBJECT_PROFILE, FEATURE_PAGE_PAYLOAD_ELISION,
        },
        page::{
            ColumnPageIndexEntryV1, PAGE_FLAG_ALL_NON_NULL, PAGE_FLAG_ALL_NULL,
            PAGE_FLAG_STATS_ONLY_CONSTANT,
        },
        segment::{TableColumnDirectoryEntryV1, TABLE_COLUMN_DIRECTORY_ENTRY_LEN},
        trust_chain, CoveError,
    };

    fn k(t: i64, csn: u64) -> TemporalRowKey {
        TemporalRowKey {
            timestamp_us: t,
            csn,
            branch_key: 0,
            goid: [0; 16],
            record_id: [0; 16],
        }
    }

    #[test]
    fn spec_58_3_lex_order_validates() {
        let rows = vec![k(1, 1), k(1, 2), k(2, 0)];
        assert!(validate_temporal_order(&rows).is_ok());
    }

    #[test]
    fn spec_58_3_out_of_order_rejected() {
        let rows = vec![k(2, 0), k(1, 0)];
        assert!(matches!(
            validate_temporal_order(&rows),
            Err(CoveError::BadSchema(_))
        ));
    }

    #[test]
    fn spec_59_1_reserved_legacy_record_kind_rejected() {
        assert!(RecordKind::ReservedLegacyMaterializedDelta
            .validate_published()
            .is_err());
        assert!(RecordKind::Delta.validate_published().is_ok());
    }

    #[test]
    fn spec_60_dangling_prev_ref_rejected() {
        let prev = vec![Some(7), Some(99)];
        let local = vec![7];
        assert_eq!(
            validate_self_contained(&prev, &local),
            Err(CoveError::NotSelfContained)
        );
    }

    fn property(property_id: u32) -> PropertyEntryV1 {
        PropertyEntryV1 {
            property_id,
            property_name: "name".into(),
            logical_type: CoveLogicalType::Bool,
            physical_kind: CovePhysicalKind::Boolean,
            nullable: false,
            collation_id: 0,
            flags: 0,
        }
    }

    #[test]
    fn object_type_catalog_roundtrip() {
        let catalog = ObjectTypeCatalog {
            flags: 0,
            types: vec![ObjectTypeEntryV1 {
                object_type_id: 10,
                type_name: "Customer".into(),
                flags: OBJECT_TYPE_FLAG_ENTITY_OBJECT,
                properties: vec![property(1)],
            }],
        };
        let parsed = ObjectTypeCatalog::parse(&catalog.serialize().unwrap()).unwrap();
        assert_eq!(parsed.types[0].type_name, "Customer");
        assert_eq!(parsed.types[0].flags, OBJECT_TYPE_FLAG_ENTITY_OBJECT);
    }

    #[test]
    fn object_type_catalog_rejects_duplicate_property() {
        let catalog = ObjectTypeCatalog {
            flags: 0,
            types: vec![ObjectTypeEntryV1 {
                object_type_id: 10,
                type_name: "Customer".into(),
                flags: OBJECT_TYPE_FLAG_ENTITY_OBJECT,
                properties: vec![property(1), property(1)],
            }],
        };
        assert!(matches!(
            ObjectTypeCatalog::parse(&catalog.serialize().unwrap()),
            Err(CoveError::BadSchema(_))
        ));
    }

    #[test]
    fn object_type_catalog_rejects_logical_null_property() {
        let mut p = property(1);
        p.logical_type = CoveLogicalType::Null;
        p.physical_kind = CovePhysicalKind::FileCode;
        let catalog = ObjectTypeCatalog {
            flags: 0,
            types: vec![ObjectTypeEntryV1 {
                object_type_id: 10,
                type_name: "Customer".into(),
                flags: OBJECT_TYPE_FLAG_ENTITY_OBJECT,
                properties: vec![p],
            }],
        };
        assert!(matches!(
            ObjectTypeCatalog::parse(&catalog.serialize().unwrap()),
            Err(CoveError::BadSchema(_))
        ));
    }

    fn temporal_entry(segment_id: u32, row_count: u32) -> TemporalSegmentIndexEntryV1 {
        TemporalSegmentIndexEntryV1 {
            segment_id,
            object_type_id: 1,
            time_range_start_us: 10,
            time_range_end_us: 20,
            csn_min: 1,
            csn_max: 2,
            row_count,
            delta_count: row_count,
            snapshot_count: 0,
            baseline_count: 0,
            tombstone_count: 0,
            min_goid: [0; 16],
            max_goid: [1; 16],
            offset: 128,
            length: 4096,
            checksum: 0,
        }
    }

    #[test]
    fn temporal_segment_index_roundtrip() {
        let index = TemporalSegmentIndex {
            flags: 0,
            entries: vec![temporal_entry(1, 2)],
        };
        let parsed = TemporalSegmentIndex::parse(&index.serialize().unwrap()).unwrap();
        assert_eq!(parsed.entries[0].row_count, 2);
    }

    #[test]
    fn temporal_segment_index_rejects_duplicate_segment_id() {
        let index = TemporalSegmentIndex {
            flags: 0,
            entries: vec![temporal_entry(1, 1), temporal_entry(1, 1)],
        };
        assert_eq!(
            TemporalSegmentIndex::parse(&index.serialize().unwrap()),
            Err(CoveError::RefInvalid)
        );
    }

    #[test]
    fn temporal_segment_index_rejects_bad_counts() {
        let mut entry = temporal_entry(1, 2);
        entry.tombstone_count = 1;
        let index = TemporalSegmentIndex {
            flags: 0,
            entries: vec![entry],
        };
        assert!(matches!(
            TemporalSegmentIndex::parse(&index.serialize().unwrap()),
            Err(CoveError::BadSchema(_))
        ));
    }

    fn temporal_row(timestamp_us: i64, csn: u64) -> TemporalRowEntryV1 {
        TemporalRowEntryV1 {
            timestamp_us,
            csn,
            branch_key: 0,
            goid: [0; 16],
            record_id: [0; 16],
            record_kind: RecordKind::Delta,
            prev_ref: None,
        }
    }

    fn temporal_segment_bytes(rows: &[TemporalRowEntryV1]) -> Vec<u8> {
        let row_directory_offset = TEMPORAL_SEGMENT_HEADER_LEN as u64;
        let row_bytes = (rows.len() * TEMPORAL_ROW_ENTRY_LEN) as u64;
        let row_end = row_directory_offset + row_bytes;
        let header = TemporalSegmentHeaderV1 {
            segment_id: 7,
            object_type_id: 1,
            time_range_start_us: rows.first().map(|row| row.timestamp_us).unwrap_or(0),
            time_range_end_us: rows.last().map(|row| row.timestamp_us).unwrap_or(0),
            csn_min: rows.first().map(|row| row.csn).unwrap_or(0),
            csn_max: rows.last().map(|row| row.csn).unwrap_or(0),
            row_count: rows.len() as u32,
            morsel_count: u32::from(!rows.is_empty()),
            morsel_row_count: if rows.is_empty() {
                0
            } else {
                rows.len() as u32
            },
            column_count: 0,
            row_directory_offset,
            column_directory_offset: row_end,
            page_index_offset: row_end,
            data_offset: row_end,
            flags: 0,
            checksum: 0,
        };
        let mut bytes = header.serialize().to_vec();
        for row in rows {
            bytes.extend_from_slice(&row.serialize());
        }
        bytes
    }

    fn temporal_segment_with_stats_only_property_page(
        rows: &[TemporalRowEntryV1],
        non_null_count: u32,
        null_count: u32,
        flags: u32,
    ) -> Vec<u8> {
        let row_directory_offset = TEMPORAL_SEGMENT_HEADER_LEN as u64;
        let row_bytes = (rows.len() * TEMPORAL_ROW_ENTRY_LEN) as u64;
        let row_end = row_directory_offset + row_bytes;
        let column_directory_offset = row_end;
        let page_index_offset = column_directory_offset + TABLE_COLUMN_DIRECTORY_ENTRY_LEN as u64;
        let page_index_length = crate::page::COLUMN_PAGE_INDEX_ENTRY_LEN as u64;
        let data_offset = page_index_offset + page_index_length;
        let header = TemporalSegmentHeaderV1 {
            segment_id: 7,
            object_type_id: 1,
            time_range_start_us: rows.first().map(|row| row.timestamp_us).unwrap_or(0),
            time_range_end_us: rows.last().map(|row| row.timestamp_us).unwrap_or(0),
            csn_min: rows.first().map(|row| row.csn).unwrap_or(0),
            csn_max: rows.last().map(|row| row.csn).unwrap_or(0),
            row_count: rows.len() as u32,
            morsel_count: u32::from(!rows.is_empty()),
            morsel_row_count: if rows.is_empty() {
                0
            } else {
                rows.len() as u32
            },
            column_count: 1,
            row_directory_offset,
            column_directory_offset,
            page_index_offset,
            data_offset,
            flags: 0,
            checksum: 0,
        };
        let directory = TableColumnDirectoryEntryV1 {
            column_id: 1,
            logical_type: CoveLogicalType::Bool,
            physical_kind: CovePhysicalKind::Boolean,
            flags: 0,
            page_index_offset,
            page_index_length,
            data_offset,
            data_length: 0,
            stats_ref: u32::MAX,
            domain_ref: u32::MAX,
            checksum: 0,
        };
        let page = ColumnPageIndexEntryV1 {
            column_id: 1,
            morsel_id: 0,
            row_count: rows.len() as u32,
            non_null_count,
            null_count,
            encoding_root: u32::MAX,
            page_offset: 0,
            page_length: 0,
            uncompressed_length: 0,
            stats_ref: 0,
            flags,
            checksum: checksum::crc32c(&[]),
        };

        let mut bytes = header.serialize().to_vec();
        for row in rows {
            bytes.extend_from_slice(&row.serialize());
        }
        bytes.extend_from_slice(&directory.serialize());
        bytes.extend_from_slice(&page.serialize());
        bytes
    }

    #[test]
    fn temporal_segment_data_roundtrip_validates() {
        let bytes = temporal_segment_bytes(&[temporal_row(10, 1), temporal_row(20, 2)]);
        let parsed = TemporalSegmentData::parse(&bytes).unwrap();
        assert_eq!(parsed.rows.len(), 2);
        assert_eq!(parsed.header.segment_id, 7);
    }

    #[test]
    fn temporal_property_all_null_stats_only_requires_elision_feature() {
        let bytes = temporal_segment_with_stats_only_property_page(
            &[temporal_row(10, 1)],
            0,
            1,
            PAGE_FLAG_STATS_ONLY_CONSTANT | PAGE_FLAG_ALL_NULL,
        );
        assert!(matches!(
            TemporalSegmentData::parse_with_required_features(&bytes, FEATURE_OBJECT_PROFILE),
            Err(CoveError::BadSection(_))
        ));
        assert!(TemporalSegmentData::parse_with_required_features(
            &bytes,
            FEATURE_OBJECT_PROFILE | FEATURE_PAGE_PAYLOAD_ELISION
        )
        .is_ok());
    }

    #[test]
    fn temporal_property_all_non_null_stats_only_requires_validated_stats() {
        let bytes = temporal_segment_with_stats_only_property_page(
            &[temporal_row(10, 1)],
            1,
            0,
            PAGE_FLAG_STATS_ONLY_CONSTANT | PAGE_FLAG_ALL_NON_NULL,
        );
        assert_eq!(
            TemporalSegmentData::parse_with_required_features(
                &bytes,
                FEATURE_OBJECT_PROFILE | FEATURE_PAGE_PAYLOAD_ELISION
            ),
            Err(CoveError::PageCorrupt)
        );
    }

    #[test]
    fn temporal_segment_data_rejects_out_of_order_rows() {
        let bytes = temporal_segment_bytes(&[temporal_row(20, 2), temporal_row(10, 1)]);
        assert!(matches!(
            TemporalSegmentData::parse(&bytes),
            Err(CoveError::BadSchema(_))
        ));
    }

    #[test]
    fn temporal_segment_data_rejects_csn_decrease_in_row_order() {
        let bytes = temporal_segment_bytes(&[temporal_row(10, 100), temporal_row(20, 50)]);
        assert_eq!(
            TemporalSegmentData::parse(&bytes),
            Err(CoveError::BadSchema(
                "temporal segment csn decreases in row order".into()
            ))
        );
    }

    #[test]
    fn temporal_segment_data_rejects_forward_prev_ref() {
        let mut first = temporal_row(10, 1);
        first.prev_ref = Some(CoveRecordRefV1 {
            segment_id: 7,
            row_index: 1,
            target_kind: 0,
        });
        let bytes = temporal_segment_bytes(&[first, temporal_row(20, 2)]);
        assert_eq!(
            TemporalSegmentData::parse(&bytes),
            Err(CoveError::RefInvalid)
        );
    }

    #[test]
    fn temporal_segment_data_allows_backward_cross_segment_prev_ref() {
        let mut row = temporal_row(20, 2);
        row.prev_ref = Some(CoveRecordRefV1 {
            segment_id: 6,
            row_index: 0,
            target_kind: 0,
        });
        let bytes = temporal_segment_bytes(&[row]);
        let parsed = TemporalSegmentData::parse(&bytes).unwrap();
        assert_eq!(parsed.rows[0].prev_ref.unwrap().segment_id, 6);
    }

    fn temporal_bloom_bytes() -> Vec<u8> {
        let filter_offset = 8 + TEMPORAL_BLOOM_ENTRY_LEN as u64;
        let filter = [1u8, 2, 3, 4];
        let mut entry = [0u8; TEMPORAL_BLOOM_ENTRY_LEN];
        entry[0..4].copy_from_slice(&7u32.to_le_bytes());
        entry[4..12].copy_from_slice(&10i64.to_le_bytes());
        entry[12..20].copy_from_slice(&20i64.to_le_bytes());
        entry[20..28].copy_from_slice(&filter_offset.to_le_bytes());
        entry[28..36].copy_from_slice(&(filter.len() as u64).to_le_bytes());
        let crc = checksum::crc32c(&entry);
        entry[36..40].copy_from_slice(&crc.to_le_bytes());

        let mut bytes = 1u32.to_le_bytes().to_vec();
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&entry);
        bytes.extend_from_slice(&filter);
        bytes
    }

    #[test]
    fn temporal_bloom_index_roundtrip_validates() {
        let parsed = TemporalBloomIndex::parse(&temporal_bloom_bytes()).unwrap();
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(parsed.entries[0].segment_id, 7);
    }

    #[cfg(feature = "digest-sha2")]
    fn trust_manifest_bytes(segment: &TemporalSegmentData) -> Vec<u8> {
        let mut bytes = 2u32.to_le_bytes().to_vec();
        let mut prev = [0u8; 32];
        for (row_index, row) in segment.rows.iter().enumerate() {
            bytes.extend_from_slice(&segment.header.segment_id.to_le_bytes());
            bytes.extend_from_slice(&(row_index as u32).to_le_bytes());
            prev = trust_chain::chain(&prev, &row.trust_payload()).unwrap();
            bytes.extend_from_slice(&prev);
        }
        bytes
    }

    #[cfg(feature = "digest-sha2")]
    #[test]
    fn trust_manifest_verifies_temporal_rows() {
        let segment = TemporalSegmentData::parse(&temporal_segment_bytes(&[
            temporal_row(10, 1),
            temporal_row(20, 2),
        ]))
        .unwrap();
        let manifest = TrustManifest::parse(&trust_manifest_bytes(&segment)).unwrap();
        assert!(manifest.verify_against(&[segment]).is_ok());
    }

    #[cfg(feature = "digest-sha2")]
    #[test]
    fn trust_manifest_rejects_bad_digest() {
        let segment = TemporalSegmentData::parse(&temporal_segment_bytes(&[
            temporal_row(10, 1),
            temporal_row(20, 2),
        ]))
        .unwrap();
        let mut bytes = trust_manifest_bytes(&segment);
        *bytes.last_mut().unwrap() ^= 0xFF;
        let manifest = TrustManifest::parse(&bytes).unwrap();
        assert_eq!(
            manifest.verify_against(&[segment]),
            Err(CoveError::DigestMismatch)
        );
    }

    #[test]
    fn trust_payload_matches_temporal_row_wire_encoding() {
        let mut row = temporal_row(20, 2);
        row.prev_ref = Some(CoveRecordRefV1 {
            segment_id: 7,
            row_index: 1,
            target_kind: 1,
        });
        assert_eq!(row.trust_payload(), row.serialize());
    }

    #[test]
    fn trust_manifest_serialize_round_trip() {
        let m = TrustManifest {
            entries: vec![
                TrustManifestEntryV1 {
                    segment_id: 1,
                    row_index: 0,
                    expected_hash: [0xAA; 32],
                },
                TrustManifestEntryV1 {
                    segment_id: 2,
                    row_index: 5,
                    expected_hash: [0xBB; 32],
                },
            ],
        };
        let bytes = m.serialize().unwrap();
        assert_eq!(TrustManifest::parse(&bytes).unwrap(), m);
    }
}
