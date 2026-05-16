use std::collections::BTreeMap;

use serde_json::{json, Number, Value};
use sha2::{Digest, Sha256};

use crate::{
    array::{CoveArrayValue, EncodedArray},
    compression,
    constants::{CoveLogicalType, CovePhysicalKind, SectionKind, ValueTag},
    dictionary::{DictionaryValue, FileDictionary},
    page::{PAGE_FLAG_ALL_NULL, PAGE_FLAG_STATS_ONLY_CONSTANT},
    page_payload::PageBufferKind,
    profile::{
        cove_map::{
            parse_embedded_section, EmbeddedMapSection, MapEvidenceIndex, MapProjectionCatalog,
        },
        cove_o::{
            CoveRecordRefV1, ObjectTypeCatalog, ObjectTypeEntryV1, PropertyEntryV1, RecordKind,
            TemporalPropertyColumn, TemporalSegmentData, OBJECT_TYPE_FLAG_ASSOCIATION_OBJECT,
            OBJECT_TYPE_FLAG_LINK_OBJECT, PROPERTY_FLAG_ASSOCIATION_FROM_GOID,
            PROPERTY_FLAG_ASSOCIATION_TO_GOID, PROPERTY_FLAG_ASSOCIATION_TYPE,
            PROPERTY_FLAG_EVIDENCE_REF, PROPERTY_FLAG_MAPPING_RULE_REF,
        },
    },
    reader::{validate_bytes_with_options, ValidationOptions},
    validity::ValidityBitmap,
    wire,
    zone_stats::{StatKind, StatScalar, ZoneStatFlags, ZoneStatsEntry, ZoneStatsSection},
    CoveError,
};

#[derive(Debug, Clone, PartialEq)]
pub struct CoveObjectSurface {
    pub object_types: Vec<ObjectTypeEntryV1>,
    pub records: Vec<CoveObjectRecord>,
    pub projection_catalog: Option<MapProjectionCatalog>,
    pub evidence_index: Option<MapEvidenceIndex>,
    pub embedded_map_sections: Vec<EmbeddedMapSection>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CoveObjectRecord {
    pub object_type_id: u32,
    pub object_type_name: String,
    pub object_type_flags: u32,
    pub segment_id: u32,
    pub row_index: u32,
    pub timestamp_us: i64,
    pub csn: u64,
    pub branch_key: u64,
    pub goid: [u8; 16],
    pub record_id: [u8; 16],
    pub record_kind: RecordKind,
    pub prev_ref: Option<CoveRecordRefV1>,
    pub properties: Vec<CoveObjectPropertyValue>,
    pub association: Option<CoveAssociationMetadata>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CoveObjectPropertyValue {
    pub property_id: u32,
    pub property_name: String,
    pub logical_type: CoveLogicalType,
    pub physical_kind: CovePhysicalKind,
    pub flags: u32,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoveAssociationMetadata {
    pub association_type: Option<String>,
    pub source_goid: Option<String>,
    pub target_goid: Option<String>,
    pub evidence_ref: Option<String>,
    pub mapping_rule_ref: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CoveObjectReadOptions {
    pub requested_property_ids: Vec<u32>,
    pub requested_property_names: Vec<String>,
}

impl CoveObjectReadOptions {
    pub fn all_properties() -> Self {
        Self::default()
    }

    pub fn requested_property_ids(property_ids: impl IntoIterator<Item = u32>) -> Self {
        Self {
            requested_property_ids: property_ids.into_iter().collect(),
            requested_property_names: Vec::new(),
        }
    }

    pub fn requested_property_names(
        property_names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            requested_property_ids: Vec::new(),
            requested_property_names: property_names.into_iter().map(Into::into).collect(),
        }
    }

    fn requests_property(&self, property: &PropertyEntryV1) -> bool {
        if self.requested_property_ids.is_empty() && self.requested_property_names.is_empty() {
            return true;
        }
        self.requested_property_ids.contains(&property.property_id)
            || self
                .requested_property_names
                .iter()
                .any(|name| name == &property.property_name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoveObjectTemporalCut {
    LatestCommitted,
    TimestampUs(i64),
    Csn(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoveObjectReconstructionOptions {
    pub temporal_cut: CoveObjectTemporalCut,
    pub branch_key: Option<u64>,
    pub include_tombstones: bool,
}

impl Default for CoveObjectReconstructionOptions {
    fn default() -> Self {
        Self {
            temporal_cut: CoveObjectTemporalCut::LatestCommitted,
            branch_key: None,
            include_tombstones: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoveObjectTombstoneStatus {
    Live,
    Tombstoned,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CoveObjectState {
    pub object_type_id: u32,
    pub object_type_name: String,
    pub object_type_flags: u32,
    pub branch_key: u64,
    pub goid: [u8; 16],
    pub latest_record_id: [u8; 16],
    pub latest_segment_id: u32,
    pub latest_row_index: u32,
    pub timestamp_us: i64,
    pub csn: u64,
    pub record_kind: RecordKind,
    pub tombstone_status: CoveObjectTombstoneStatus,
    pub properties: Vec<CoveObjectPropertyValue>,
    pub association: Option<CoveAssociationMetadata>,
}

pub fn read_object_surface_from_bytes(bytes: &[u8]) -> Result<CoveObjectSurface, CoveError> {
    read_object_surface_from_bytes_with_options(bytes, &CoveObjectReadOptions::default())
}

pub fn read_object_surface_from_bytes_with_options(
    bytes: &[u8],
    options: &CoveObjectReadOptions,
) -> Result<CoveObjectSurface, CoveError> {
    let report = validate_bytes_with_options(
        bytes,
        ValidationOptions {
            semantic: true,
            verify_digests: false,
            allow_unknown_optional_extensions: true,
            ..ValidationOptions::default()
        },
    )?;

    let mut catalog = None;
    let mut segments = Vec::new();
    let mut projection_catalog = None;
    let mut evidence_index = None;
    let mut embedded_map_sections = Vec::new();
    let mut dictionary_index = None::<Vec<u8>>;
    let mut dictionary_payload = None::<Vec<u8>>;
    let mut zone_stats = Vec::<ZoneStatsEntry>::new();

    for entry in &report.validated.footer.sections {
        let Some(kind) = SectionKind::from_u16(entry.section_kind) else {
            continue;
        };
        let payload = compression::section_payload(bytes, entry)?;
        match kind {
            SectionKind::ObjectTypeCatalog => {
                catalog = Some(ObjectTypeCatalog::parse(payload.as_ref())?);
            }
            SectionKind::TemporalSegmentData => {
                segments.push(TemporalSegmentData::parse_with_required_features(
                    payload.as_ref(),
                    entry.required_features,
                )?);
            }
            SectionKind::FileDictionaryIndex => {
                dictionary_index = Some(payload.as_ref().to_vec());
            }
            SectionKind::FileDictionaryPayload => {
                dictionary_payload = Some(payload.as_ref().to_vec());
            }
            SectionKind::ZoneStats => {
                zone_stats.extend(ZoneStatsSection::parse(payload.as_ref())?.entries);
            }
            kind if is_map_section(kind) => {
                let embedded = parse_embedded_section(kind, payload.as_ref())?;
                match &embedded {
                    EmbeddedMapSection::ProjectionCatalog(catalog) => {
                        projection_catalog = Some(catalog.clone());
                    }
                    EmbeddedMapSection::EvidenceIndex(index) => {
                        evidence_index = Some(index.clone());
                    }
                    _ => {}
                }
                embedded_map_sections.push(embedded);
            }
            _ => {}
        }
    }
    let dictionary = match dictionary_index {
        Some(index) => Some(FileDictionary::parse(
            &index,
            dictionary_payload.as_deref().unwrap_or(&[]),
        )?),
        None => None,
    };

    let catalog = catalog.ok_or_else(|| {
        CoveError::BadSchema("COVE-O readback requires OBJECT_TYPE_CATALOG".into())
    })?;
    let object_types_by_id = catalog
        .types
        .iter()
        .map(|ty| (ty.object_type_id, ty))
        .collect::<BTreeMap<_, _>>();
    let mut records = Vec::new();
    for segment in segments {
        let object_type = object_types_by_id
            .get(&segment.header.object_type_id)
            .copied()
            .ok_or_else(|| {
                CoveError::BadSchema(format!(
                    "temporal segment references missing object_type_id {}",
                    segment.header.object_type_id
                ))
            })?;
        records.extend(records_from_segment(
            &segment,
            object_type,
            dictionary.as_ref(),
            &zone_stats,
            options,
        )?);
    }
    if let Some(catalog) = &projection_catalog {
        apply_projection_nested_shapes(&mut records, catalog)?;
    }

    Ok(CoveObjectSurface {
        object_types: catalog.types,
        records,
        projection_catalog,
        evidence_index,
        embedded_map_sections,
    })
}

fn records_from_segment(
    segment: &TemporalSegmentData,
    object_type: &ObjectTypeEntryV1,
    dictionary: Option<&FileDictionary>,
    zone_stats: &[ZoneStatsEntry],
    options: &CoveObjectReadOptions,
) -> Result<Vec<CoveObjectRecord>, CoveError> {
    let mut values_by_row = vec![Vec::new(); segment.rows.len()];
    let properties_by_id = object_type
        .properties
        .iter()
        .map(|property| (property.property_id, property))
        .collect::<BTreeMap<_, _>>();

    for column in &segment.property_columns {
        let property = properties_by_id
            .get(&column.directory.column_id)
            .copied()
            .ok_or_else(|| {
                CoveError::BadSchema(format!(
                    "temporal property column references missing property_id {}",
                    column.directory.column_id
                ))
            })?;
        if !options.requests_property(property) {
            continue;
        }
        let values = decode_property_column(segment, property, column, dictionary, zone_stats)?;
        for (row_values, value) in values_by_row.iter_mut().zip(values) {
            row_values.push(CoveObjectPropertyValue {
                property_id: property.property_id,
                property_name: property.property_name.clone(),
                logical_type: property.logical_type,
                physical_kind: property.physical_kind,
                flags: property.flags,
                value,
            });
        }
    }

    let mut records = Vec::with_capacity(segment.rows.len());
    for (row_index, row) in segment.rows.iter().enumerate() {
        let properties = std::mem::take(&mut values_by_row[row_index]);
        let association = association_metadata(object_type, &properties);
        records.push(CoveObjectRecord {
            object_type_id: object_type.object_type_id,
            object_type_name: object_type.type_name.clone(),
            object_type_flags: object_type.flags,
            segment_id: segment.header.segment_id,
            row_index: row_index as u32,
            timestamp_us: row.timestamp_us,
            csn: row.csn,
            branch_key: row.branch_key,
            goid: row.goid,
            record_id: row.record_id,
            record_kind: row.record_kind,
            prev_ref: row.prev_ref,
            properties,
            association,
        });
    }
    Ok(records)
}

#[derive(Debug, Clone)]
enum ProjectionNestedShape {
    Scalar(CoveLogicalType),
    List(Box<ProjectionNestedShape>),
    Struct(Vec<ProjectionNestedField>),
    Map {
        key: Box<ProjectionNestedShape>,
        value: Box<ProjectionNestedShape>,
    },
}

#[derive(Debug, Clone)]
struct ProjectionNestedField {
    field_id: u64,
    name: String,
    shape: ProjectionNestedShape,
}

fn apply_projection_nested_shapes(
    records: &mut [CoveObjectRecord],
    catalog: &MapProjectionCatalog,
) -> Result<(), CoveError> {
    let lookup = projection_nested_shape_lookup(catalog)?;
    if lookup.is_empty() {
        return Ok(());
    }
    for record in records {
        for property in &mut record.properties {
            let Some(shape) = lookup.get(&(
                record.object_type_name.clone(),
                property.property_name.clone(),
            )) else {
                continue;
            };
            property.value = restore_nested_projection_value(&property.value, shape)?;
        }
    }
    Ok(())
}

fn projection_nested_shape_lookup(
    catalog: &MapProjectionCatalog,
) -> Result<BTreeMap<(String, String), ProjectionNestedShape>, CoveError> {
    let mut lookup = BTreeMap::new();
    for projection in &catalog.projections {
        let output_table = projection
            .output_table
            .as_deref()
            .unwrap_or(&projection.projection_id);
        for column in &projection.columns {
            let Some(shape) = column.nested_shape.as_deref() else {
                continue;
            };
            let shape = parse_projection_nested_shape(column.logical_type.as_deref(), shape)?;
            lookup.insert((output_table.to_string(), column.name.clone()), shape);
        }
    }
    Ok(lookup)
}

fn parse_projection_nested_shape(
    logical_type: Option<&str>,
    shape: &str,
) -> Result<ProjectionNestedShape, CoveError> {
    let value: Value = serde_json::from_str(shape)
        .map_err(|_| CoveError::BadSchema("projection nested_shape must be valid JSON".into()))?;
    let mut shape = parse_projection_nested_shape_value(&value)?;
    if let Some(logical_type) = logical_type {
        let expected = projection_logical_type_from_name(logical_type)?;
        shape = ensure_projection_shape_logical(shape, expected)?;
    }
    Ok(shape)
}

fn ensure_projection_shape_logical(
    shape: ProjectionNestedShape,
    expected: CoveLogicalType,
) -> Result<ProjectionNestedShape, CoveError> {
    let matches = matches!(
        (&shape, expected),
        (ProjectionNestedShape::List(_), CoveLogicalType::List)
            | (ProjectionNestedShape::Struct(_), CoveLogicalType::Struct)
            | (ProjectionNestedShape::Map { .. }, CoveLogicalType::Map)
    );
    if matches {
        Ok(shape)
    } else {
        Err(CoveError::BadSchema(
            "projection nested_shape does not match logical_type".into(),
        ))
    }
}

fn parse_projection_nested_shape_value(value: &Value) -> Result<ProjectionNestedShape, CoveError> {
    let object = value
        .as_object()
        .ok_or_else(|| CoveError::BadSchema("nested_shape must be an object".into()))?;
    let kind = object
        .get("type")
        .or_else(|| object.get("kind"))
        .or_else(|| object.get("logical_type"))
        .or_else(|| object.get("logical"))
        .and_then(Value::as_str)
        .ok_or_else(|| CoveError::BadSchema("nested_shape requires type".into()))?;
    match kind {
        "list" => {
            let item = object
                .get("item")
                .or_else(|| object.get("element"))
                .ok_or_else(|| CoveError::BadSchema("list nested_shape requires item".into()))?;
            Ok(ProjectionNestedShape::List(Box::new(
                parse_projection_nested_shape_value(item)?,
            )))
        }
        "struct" => {
            let fields = object
                .get("fields")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    CoveError::BadSchema("struct nested_shape requires fields array".into())
                })?;
            let mut out = Vec::with_capacity(fields.len());
            for (index, field) in fields.iter().enumerate() {
                let field_object = field.as_object().ok_or_else(|| {
                    CoveError::BadSchema("struct nested_shape field must be an object".into())
                })?;
                let name = field_object
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CoveError::BadSchema("struct nested_shape field requires name".into())
                    })?;
                out.push(ProjectionNestedField {
                    field_id: stable_projection_field_id(name, index as u32 + 1) as u64,
                    name: name.to_string(),
                    shape: parse_projection_nested_shape_value(field)?,
                });
            }
            Ok(ProjectionNestedShape::Struct(out))
        }
        "map" => {
            let key = object
                .get("key")
                .ok_or_else(|| CoveError::BadSchema("map nested_shape requires key".into()))?;
            let value = object
                .get("value")
                .ok_or_else(|| CoveError::BadSchema("map nested_shape requires value".into()))?;
            Ok(ProjectionNestedShape::Map {
                key: Box::new(parse_projection_nested_shape_value(key)?),
                value: Box::new(parse_projection_nested_shape_value(value)?),
            })
        }
        _ => Ok(ProjectionNestedShape::Scalar(
            projection_logical_type_from_name(kind)?,
        )),
    }
}

fn restore_nested_projection_value(
    value: &Value,
    shape: &ProjectionNestedShape,
) -> Result<Value, CoveError> {
    if value.is_null() {
        return Ok(Value::Null);
    }
    match shape {
        ProjectionNestedShape::Scalar(logical) => {
            let _ = logical;
            Ok(value.clone())
        }
        ProjectionNestedShape::List(item_shape) => {
            let items = value.as_array().ok_or(CoveError::BadFileCode)?;
            Ok(Value::Array(
                items
                    .iter()
                    .map(|item| restore_nested_projection_value(item, item_shape))
                    .collect::<Result<Vec<_>, _>>()?,
            ))
        }
        ProjectionNestedShape::Struct(fields) => {
            let object = value.as_object().ok_or(CoveError::BadFileCode)?;
            let mut out = serde_json::Map::new();
            for field in fields {
                let raw = object
                    .get(&field.field_id.to_string())
                    .unwrap_or(&Value::Null);
                out.insert(
                    field.name.clone(),
                    restore_nested_projection_value(raw, &field.shape)?,
                );
            }
            Ok(Value::Object(out))
        }
        ProjectionNestedShape::Map { key, value: item } => {
            let entries = value.as_array().ok_or(CoveError::BadFileCode)?;
            if matches!(
                key.as_ref(),
                ProjectionNestedShape::Scalar(CoveLogicalType::Utf8)
            ) {
                let mut out = serde_json::Map::new();
                for entry in entries {
                    let pair = entry.as_array().ok_or(CoveError::BadFileCode)?;
                    if pair.len() != 2 {
                        return Err(CoveError::BadFileCode);
                    }
                    let Some(key) = pair[0].as_str() else {
                        return Err(CoveError::BadFileCode);
                    };
                    out.insert(
                        key.to_string(),
                        restore_nested_projection_value(&pair[1], item)?,
                    );
                }
                Ok(Value::Object(out))
            } else {
                Ok(Value::Array(
                    entries
                        .iter()
                        .map(|entry| {
                            let pair = entry.as_array().ok_or(CoveError::BadFileCode)?;
                            if pair.len() != 2 {
                                return Err(CoveError::BadFileCode);
                            }
                            Ok(Value::Array(vec![
                                restore_nested_projection_value(&pair[0], key)?,
                                restore_nested_projection_value(&pair[1], item)?,
                            ]))
                        })
                        .collect::<Result<Vec<_>, CoveError>>()?,
                ))
            }
        }
    }
}

fn projection_logical_type_from_name(name: &str) -> Result<CoveLogicalType, CoveError> {
    match name {
        "null" => Ok(CoveLogicalType::Null),
        "bool" | "boolean" => Ok(CoveLogicalType::Bool),
        "int8" => Ok(CoveLogicalType::Int8),
        "int16" => Ok(CoveLogicalType::Int16),
        "int32" => Ok(CoveLogicalType::Int32),
        "int64" | "int" => Ok(CoveLogicalType::Int64),
        "uint8" => Ok(CoveLogicalType::UInt8),
        "uint16" => Ok(CoveLogicalType::UInt16),
        "uint32" => Ok(CoveLogicalType::UInt32),
        "uint64" | "uint" => Ok(CoveLogicalType::UInt64),
        "float32" => Ok(CoveLogicalType::Float32),
        "float64" | "float" => Ok(CoveLogicalType::Float64),
        "decimal64" => Ok(CoveLogicalType::Decimal64),
        "decimal128" | "decimal" => Ok(CoveLogicalType::Decimal128),
        "date_days" | "date32" | "date" => Ok(CoveLogicalType::DateDays),
        "timestamp_micros" | "timestamp_us" => Ok(CoveLogicalType::TimestampMicros),
        "timestamp_nanos" | "timestamp_ns" => Ok(CoveLogicalType::TimestampNanos),
        "utf8" | "string" => Ok(CoveLogicalType::Utf8),
        "binary" => Ok(CoveLogicalType::Binary),
        "uuid" => Ok(CoveLogicalType::Uuid),
        "json" => Ok(CoveLogicalType::Json),
        "list" => Ok(CoveLogicalType::List),
        "struct" => Ok(CoveLogicalType::Struct),
        "map" => Ok(CoveLogicalType::Map),
        other => Err(CoveError::BadSchema(format!(
            "unsupported nested_shape logical_type '{other}'"
        ))),
    }
}

fn stable_projection_field_id(text: &str, fallback: u32) -> u32 {
    let digest = Sha256::digest(text.as_bytes());
    let value = u32::from_le_bytes(digest[..4].try_into().unwrap());
    if value == 0 {
        fallback
    } else {
        value
    }
}

pub fn reconstruct_object_states(
    surface: &CoveObjectSurface,
    options: &CoveObjectReconstructionOptions,
) -> Result<Vec<CoveObjectState>, CoveError> {
    validate_prev_refs(&surface.records)?;
    let mut grouped = BTreeMap::<(u32, u64, [u8; 16]), Vec<&CoveObjectRecord>>::new();
    for record in &surface.records {
        if options
            .branch_key
            .is_some_and(|branch_key| record.branch_key != branch_key)
        {
            continue;
        }
        if !record_visible_at_cut(record, options.temporal_cut) {
            continue;
        }
        grouped
            .entry((record.object_type_id, record.branch_key, record.goid))
            .or_default()
            .push(record);
    }

    let mut states = Vec::with_capacity(grouped.len());
    for ((_object_type_id, _branch_key, _goid), mut records) in grouped {
        records.sort_by_key(|record| record_sort_key(record));
        let mut current: Option<CoveObjectState> = None;
        for record in records {
            validate_record_chain_step(record, current.as_ref())?;
            match record.record_kind {
                RecordKind::Baseline | RecordKind::Snapshot => {
                    current = Some(state_from_full_record(record));
                }
                RecordKind::Delta => {
                    if let Some(state) = current.as_mut() {
                        apply_delta_record(state, record);
                    } else {
                        current = Some(state_from_full_record(record));
                    }
                }
                RecordKind::Tombstone => {
                    if let Some(state) = current.as_mut() {
                        state.latest_record_id = record.record_id;
                        state.latest_segment_id = record.segment_id;
                        state.latest_row_index = record.row_index;
                        state.timestamp_us = record.timestamp_us;
                        state.csn = record.csn;
                        state.record_kind = record.record_kind;
                        state.tombstone_status = CoveObjectTombstoneStatus::Tombstoned;
                    } else {
                        let mut state = state_from_full_record(record);
                        state.tombstone_status = CoveObjectTombstoneStatus::Tombstoned;
                        current = Some(state);
                    }
                }
                RecordKind::ReservedLegacyMaterializedDelta => {
                    return Err(CoveError::BadSchema(
                        "reserved legacy materialized delta cannot be reconstructed".into(),
                    ))
                }
            }
        }
        if let Some(state) = current {
            if options.include_tombstones
                || state.tombstone_status == CoveObjectTombstoneStatus::Live
            {
                states.push(state);
            }
        }
    }
    states.sort_by_key(|state| {
        (
            state.object_type_id,
            state.branch_key,
            state.goid,
            state.timestamp_us,
            state.csn,
        )
    });
    Ok(states)
}

fn validate_prev_refs(records: &[CoveObjectRecord]) -> Result<(), CoveError> {
    let by_ref = records
        .iter()
        .map(|record| ((record.segment_id, record.row_index), record))
        .collect::<BTreeMap<_, _>>();
    for record in records {
        let Some(prev_ref) = record.prev_ref else {
            continue;
        };
        if prev_ref.target_kind > 1 {
            return Err(CoveError::RefInvalid);
        }
        let Some(prev) = by_ref
            .get(&(prev_ref.segment_id, prev_ref.row_index))
            .copied()
        else {
            return Err(CoveError::RefInvalid);
        };
        if prev.object_type_id != record.object_type_id
            || prev.branch_key != record.branch_key
            || prev.goid != record.goid
            || record_sort_key(prev) >= record_sort_key(record)
        {
            return Err(CoveError::RefInvalid);
        }
    }
    Ok(())
}

fn validate_record_chain_step(
    record: &CoveObjectRecord,
    current: Option<&CoveObjectState>,
) -> Result<(), CoveError> {
    if let Some(prev_ref) = record.prev_ref {
        let Some(current) = current else {
            return Err(CoveError::RefInvalid);
        };
        if current.latest_segment_id != prev_ref.segment_id
            || current.latest_row_index != prev_ref.row_index
        {
            return Err(CoveError::RefInvalid);
        }
    }
    Ok(())
}

fn state_from_full_record(record: &CoveObjectRecord) -> CoveObjectState {
    let tombstone_status = if record.record_kind == RecordKind::Tombstone {
        CoveObjectTombstoneStatus::Tombstoned
    } else {
        CoveObjectTombstoneStatus::Live
    };
    CoveObjectState {
        object_type_id: record.object_type_id,
        object_type_name: record.object_type_name.clone(),
        object_type_flags: record.object_type_flags,
        branch_key: record.branch_key,
        goid: record.goid,
        latest_record_id: record.record_id,
        latest_segment_id: record.segment_id,
        latest_row_index: record.row_index,
        timestamp_us: record.timestamp_us,
        csn: record.csn,
        record_kind: record.record_kind,
        tombstone_status,
        properties: record.properties.clone(),
        association: record.association.clone(),
    }
}

fn apply_delta_record(state: &mut CoveObjectState, record: &CoveObjectRecord) {
    state.latest_record_id = record.record_id;
    state.latest_segment_id = record.segment_id;
    state.latest_row_index = record.row_index;
    state.timestamp_us = record.timestamp_us;
    state.csn = record.csn;
    state.record_kind = record.record_kind;
    state.tombstone_status = CoveObjectTombstoneStatus::Live;
    for property in &record.properties {
        match state
            .properties
            .iter_mut()
            .find(|existing| existing.property_id == property.property_id)
        {
            Some(existing) => *existing = property.clone(),
            None => state.properties.push(property.clone()),
        }
    }
    state.association = association_metadata_from_state(state);
}

fn association_metadata_from_state(state: &CoveObjectState) -> Option<CoveAssociationMetadata> {
    let object_type = ObjectTypeEntryV1 {
        object_type_id: state.object_type_id,
        flags: state.object_type_flags,
        type_name: state.object_type_name.clone(),
        properties: Vec::new(),
    };
    association_metadata(&object_type, &state.properties)
}

fn record_visible_at_cut(record: &CoveObjectRecord, cut: CoveObjectTemporalCut) -> bool {
    match cut {
        CoveObjectTemporalCut::LatestCommitted => true,
        CoveObjectTemporalCut::TimestampUs(timestamp_us) => record.timestamp_us <= timestamp_us,
        CoveObjectTemporalCut::Csn(csn) => record.csn <= csn,
    }
}

fn record_sort_key(record: &CoveObjectRecord) -> (i64, u64, u32, u32, [u8; 16]) {
    (
        record.timestamp_us,
        record.csn,
        record.segment_id,
        record.row_index,
        record.record_id,
    )
}

fn decode_property_column(
    segment: &TemporalSegmentData,
    property: &PropertyEntryV1,
    column: &TemporalPropertyColumn,
    dictionary: Option<&FileDictionary>,
    zone_stats: &[ZoneStatsEntry],
) -> Result<Vec<Value>, CoveError> {
    let mut values = vec![Value::Null; segment.rows.len()];
    for page in &column.pages {
        let page_row_count = page.index_entry.row_count as usize;
        let row_start = (page.index_entry.morsel_id as usize)
            .checked_mul(segment.header.morsel_row_count as usize)
            .ok_or(CoveError::ArithOverflow)?;
        let row_end = row_start
            .checked_add(page_row_count)
            .ok_or(CoveError::ArithOverflow)?;
        if row_end > values.len() {
            return Err(CoveError::PageCorrupt);
        }

        let Some(payload) = &page.payload else {
            if page.index_entry.non_null_count == 0
                || page.index_entry.null_count == page.index_entry.row_count
                || page.index_entry.flags & PAGE_FLAG_ALL_NULL != 0
            {
                continue;
            }
            if page.index_entry.flags & PAGE_FLAG_STATS_ONLY_CONSTANT != 0 {
                let value = stats_only_constant_value(
                    segment,
                    property,
                    &page.index_entry,
                    dictionary,
                    zone_stats,
                )?;
                for row in &mut values[row_start..row_end] {
                    *row = value.clone();
                }
                continue;
            }
            return Err(CoveError::PageCorrupt);
        };

        if payload.header.row_count != page.index_entry.row_count {
            return Err(CoveError::PageCorrupt);
        }
        let root = payload.root_node()?;
        if root.logical_type != property.logical_type
            || root.physical_kind != property.physical_kind
        {
            return Err(CoveError::PageCorrupt);
        }
        let null_bitmap = payload.buffer_bytes(PageBufferKind::NullBitmap)?;
        let validity = null_bitmap
            .map(|bytes| ValidityBitmap::new(bytes, u64::from(page.index_entry.row_count)));
        if let Some(validity) = validity {
            validity.validate_len(u64::from(page.index_entry.row_count))?;
        }
        let value_bytes = payload.buffer_bytes(PageBufferKind::Values)?.unwrap_or(&[]);
        let array = EncodedArray::new(
            property.logical_type,
            property.physical_kind,
            u64::from(page.index_entry.row_count),
            root.encoding_kind,
            validity,
            value_bytes,
            None,
        );
        let prepared = array.prepare()?;
        for local_row in 0..page.index_entry.row_count {
            values[row_start + local_row as usize] = decode_property_value(
                property,
                prepared.decode_row(u64::from(local_row))?,
                dictionary,
            )?;
        }
    }
    Ok(values)
}

fn decode_property_value(
    property: &PropertyEntryV1,
    value: CoveArrayValue<'_>,
    dictionary: Option<&FileDictionary>,
) -> Result<Value, CoveError> {
    match value {
        CoveArrayValue::Null => Ok(Value::Null),
        CoveArrayValue::Boolean(value) | CoveArrayValue::ValidityBit(value) => {
            Ok(Value::Bool(value))
        }
        CoveArrayValue::NumCode(value) | CoveArrayValue::Varint(value) => Ok(json!(value)),
        CoveArrayValue::Int64(value) => Ok(Value::Number(Number::from(value))),
        CoveArrayValue::Bytes(bytes) => decode_bytes_value(property, bytes),
        CoveArrayValue::OwnedBytes(bytes) => decode_bytes_value(property, &bytes),
        CoveArrayValue::FileCode(code) => decode_file_code_value(property, code, dictionary),
        CoveArrayValue::DictValue(DictionaryValue::RawBytes(bytes)) => {
            decode_canonical_dictionary_bytes(property, property.logical_type, &bytes)
        }
        CoveArrayValue::DictValue(DictionaryValue::RedactedPresent) => {
            Err(CoveError::UnsupportedEncoding(
                "COVE-O readback refuses to expose redacted FileCode payload bytes".into(),
            ))
        }
    }
}

fn decode_file_code_value(
    property: &PropertyEntryV1,
    code: u32,
    dictionary: Option<&FileDictionary>,
) -> Result<Value, CoveError> {
    let dictionary = dictionary.ok_or_else(|| {
        CoveError::UnsupportedEncoding("FileCode property requires FILE_DICTIONARY sections".into())
    })?;
    let entry = dictionary.get_entry(code)?;
    let value_tag = ValueTag::from_u16(entry.value_tag).ok_or(CoveError::BadFileCode)?;
    match dictionary.decode_value(code)? {
        DictionaryValue::RawBytes(bytes) => decode_canonical_value_tag(property, value_tag, &bytes),
        DictionaryValue::RedactedPresent => Err(CoveError::UnsupportedEncoding(
            "COVE-O readback refuses to expose redacted FileCode payload bytes".into(),
        )),
    }
}

fn stats_only_constant_value(
    segment: &TemporalSegmentData,
    property: &PropertyEntryV1,
    page: &crate::page::ColumnPageIndexEntryV1,
    dictionary: Option<&FileDictionary>,
    zone_stats: &[ZoneStatsEntry],
) -> Result<Value, CoveError> {
    if page.non_null_count == 0
        || page.null_count == page.row_count
        || page.flags & PAGE_FLAG_ALL_NULL != 0
    {
        return Ok(Value::Null);
    }
    let stats_ref = usize::try_from(page.stats_ref).map_err(|_| CoveError::ArithOverflow)?;
    let entry = zone_stats.get(stats_ref).ok_or_else(|| {
        CoveError::UnsupportedEncoding(
            "COVE-O readback needs exact untruncated zone stats for this stats-only property"
                .into(),
        )
    })?;
    if entry.segment_id != segment.header.segment_id
        || entry.morsel_id != page.morsel_id
        || entry.column_id != property.property_id
        || entry.stats.row_count != u64::from(page.row_count)
        || entry.stats.null_count != 0
        || entry.non_null_count != page.row_count
        || page.null_count != 0
        || page.non_null_count != page.row_count
        || !entry.stats.flags.contains(ZoneStatFlags::CONSTANT)
        || !entry.stats.flags.contains(ZoneStatFlags::HAS_MIN_MAX)
        || entry.stats.flags.contains(ZoneStatFlags::MINMAX_TRUNCATED)
    {
        return Err(CoveError::UnsupportedEncoding(
            "COVE-O readback cannot prove a canonical stats-only constant for this property".into(),
        ));
    }
    let (Some(min), Some(max)) = (&entry.stats.min, &entry.stats.max) else {
        return Err(CoveError::UnsupportedEncoding(
            "COVE-O readback cannot prove a canonical stats-only constant for this property".into(),
        ));
    };
    if min.truncated || max.truncated || min != max {
        return Err(CoveError::UnsupportedEncoding(
            "COVE-O readback refuses ambiguous or truncated stats-only property values".into(),
        ));
    }
    decode_stat_scalar_value(property, min, dictionary)
}

fn decode_stat_scalar_value(
    property: &PropertyEntryV1,
    scalar: &StatScalar,
    dictionary: Option<&FileDictionary>,
) -> Result<Value, CoveError> {
    match scalar.kind {
        StatKind::Int64 | StatKind::TimestampMicros | StatKind::TimestampNanos => {
            if scalar.bytes.len() != 8 {
                return Err(CoveError::BadStats);
            }
            Ok(json!(i64::from_le_bytes(
                scalar.bytes[..8].try_into().unwrap()
            )))
        }
        StatKind::UInt64 => {
            if scalar.bytes.len() != 8 {
                return Err(CoveError::BadStats);
            }
            let value = u64::from_le_bytes(scalar.bytes[..8].try_into().unwrap());
            if property.physical_kind == CovePhysicalKind::Boolean {
                return match value {
                    0 => Ok(Value::Bool(false)),
                    1 => Ok(Value::Bool(true)),
                    _ => Err(CoveError::BadStats),
                };
            }
            if property.physical_kind == CovePhysicalKind::FileCode {
                let code = u32::try_from(value).map_err(|_| CoveError::BadFileCode)?;
                return decode_file_code_value(property, code, dictionary);
            }
            Ok(json!(value))
        }
        StatKind::Float64Bits => {
            if scalar.bytes.len() != 8 {
                return Err(CoveError::BadStats);
            }
            let value = f64::from_bits(u64::from_le_bytes(scalar.bytes[..8].try_into().unwrap()));
            Number::from_f64(value)
                .map(Value::Number)
                .ok_or(CoveError::BadStats)
        }
        StatKind::Decimal128 => {
            if scalar.bytes.len() != 16 {
                return Err(CoveError::BadStats);
            }
            Ok(Value::String(
                i128::from_le_bytes(scalar.bytes[..16].try_into().unwrap()).to_string(),
            ))
        }
        StatKind::DateDays => {
            if scalar.bytes.len() != 4 {
                return Err(CoveError::BadStats);
            }
            Ok(json!(i32::from_le_bytes(
                scalar.bytes[..4].try_into().unwrap()
            )))
        }
        StatKind::FixedBytes => decode_bytes_value(property, &scalar.bytes),
        StatKind::None => Ok(Value::Null),
    }
}

fn decode_bytes_value(property: &PropertyEntryV1, bytes: &[u8]) -> Result<Value, CoveError> {
    match property.physical_kind {
        CovePhysicalKind::Boolean => {
            if bytes.len() != 1 {
                return Err(CoveError::PageCorrupt);
            }
            match bytes[0] {
                0 => Ok(Value::Bool(false)),
                1 => Ok(Value::Bool(true)),
                _ => Err(CoveError::PageCorrupt),
            }
        }
        CovePhysicalKind::FixedBytes => match property.logical_type {
            CoveLogicalType::Uuid => {
                if bytes.len() != 16 {
                    return Err(CoveError::PageCorrupt);
                }
                Ok(Value::String(hex_encode(bytes)))
            }
            CoveLogicalType::Decimal64 => {
                if bytes.len() != 8 {
                    return Err(CoveError::PageCorrupt);
                }
                let value = i64::from_le_bytes(bytes.try_into().unwrap());
                Ok(Value::String(value.to_string()))
            }
            CoveLogicalType::Decimal128 => {
                if bytes.len() != 16 {
                    return Err(CoveError::PageCorrupt);
                }
                let value = i128::from_le_bytes(bytes.try_into().unwrap());
                Ok(Value::String(value.to_string()))
            }
            _ => Ok(Value::String(hex_encode(bytes))),
        },
        CovePhysicalKind::VarBytes => match property.logical_type {
            CoveLogicalType::Utf8 => String::from_utf8(bytes.to_vec())
                .map(Value::String)
                .map_err(|_| CoveError::PageCorrupt),
            CoveLogicalType::Json => {
                serde_json::from_slice(bytes).map_err(|_| CoveError::PageCorrupt)
            }
            CoveLogicalType::Binary => match std::str::from_utf8(bytes) {
                Ok(text) => Ok(Value::String(text.to_string())),
                Err(_) => Ok(Value::String(hex_encode(bytes))),
            },
            _ => Ok(Value::String(hex_encode(bytes))),
        },
        _ => Err(CoveError::UnsupportedEncoding(format!(
            "COVE-O readback cannot decode bytes for physical kind {:?}",
            property.physical_kind
        ))),
    }
}

fn decode_canonical_dictionary_bytes(
    property: &PropertyEntryV1,
    logical_type: CoveLogicalType,
    bytes: &[u8],
) -> Result<Value, CoveError> {
    let value_tag = match logical_type {
        CoveLogicalType::Null => ValueTag::Null,
        CoveLogicalType::Bool => {
            return match bytes {
                [] => Ok(Value::Bool(false)),
                [0] => Ok(Value::Bool(false)),
                [1] => Ok(Value::Bool(true)),
                _ => Err(CoveError::BadFileCode),
            }
        }
        CoveLogicalType::Int8
        | CoveLogicalType::Int16
        | CoveLogicalType::Int32
        | CoveLogicalType::Int64 => ValueTag::Int64,
        CoveLogicalType::UInt8
        | CoveLogicalType::UInt16
        | CoveLogicalType::UInt32
        | CoveLogicalType::UInt64 => ValueTag::UInt64,
        CoveLogicalType::Float32 => ValueTag::Float32Bits,
        CoveLogicalType::Float64 => ValueTag::Float64Bits,
        CoveLogicalType::Decimal64 => ValueTag::Decimal64,
        CoveLogicalType::Decimal128 => ValueTag::Decimal128,
        CoveLogicalType::DateDays => ValueTag::DateDays,
        CoveLogicalType::TimestampMicros => ValueTag::TimestampMicros,
        CoveLogicalType::TimestampNanos => ValueTag::TimestampNanos,
        CoveLogicalType::Utf8 => ValueTag::Utf8,
        CoveLogicalType::Binary => ValueTag::Binary,
        CoveLogicalType::Uuid => ValueTag::Uuid,
        CoveLogicalType::Json => ValueTag::Json,
        CoveLogicalType::List => ValueTag::List,
        CoveLogicalType::Struct => ValueTag::Struct,
        CoveLogicalType::Map => ValueTag::Map,
    };
    decode_canonical_value_tag(property, value_tag, bytes)
}

fn decode_canonical_value_tag(
    property: &PropertyEntryV1,
    value_tag: ValueTag,
    bytes: &[u8],
) -> Result<Value, CoveError> {
    match value_tag {
        ValueTag::Null => Ok(Value::Null),
        ValueTag::BoolFalse => Ok(Value::Bool(false)),
        ValueTag::BoolTrue => Ok(Value::Bool(true)),
        ValueTag::Int64 | ValueTag::TimestampMicros | ValueTag::TimestampNanos => {
            if bytes.len() != 8 {
                return Err(CoveError::BadFileCode);
            }
            Ok(json!(i64::from_le_bytes(bytes.try_into().unwrap())))
        }
        ValueTag::UInt64 => {
            if bytes.len() != 8 {
                return Err(CoveError::BadFileCode);
            }
            Ok(json!(u64::from_le_bytes(bytes.try_into().unwrap())))
        }
        ValueTag::Float32Bits => {
            if bytes.len() != 4 {
                return Err(CoveError::BadFileCode);
            }
            Number::from_f64(f32::from_bits(u32::from_le_bytes(bytes.try_into().unwrap())) as f64)
                .map(Value::Number)
                .ok_or(CoveError::BadFileCode)
        }
        ValueTag::Float64Bits => {
            if bytes.len() != 8 {
                return Err(CoveError::BadFileCode);
            }
            Number::from_f64(f64::from_bits(u64::from_le_bytes(
                bytes.try_into().unwrap(),
            )))
            .map(Value::Number)
            .ok_or(CoveError::BadFileCode)
        }
        ValueTag::Decimal64 => {
            if bytes.len() != 8 {
                return Err(CoveError::BadFileCode);
            }
            Ok(Value::String(
                i64::from_le_bytes(bytes.try_into().unwrap()).to_string(),
            ))
        }
        ValueTag::Decimal128 => {
            if bytes.len() != 16 {
                return Err(CoveError::BadFileCode);
            }
            Ok(Value::String(
                i128::from_le_bytes(bytes.try_into().unwrap()).to_string(),
            ))
        }
        ValueTag::DateDays => {
            if bytes.len() != 4 {
                return Err(CoveError::BadFileCode);
            }
            Ok(json!(i32::from_le_bytes(bytes.try_into().unwrap())))
        }
        ValueTag::Uuid => {
            if bytes.len() != 16 {
                return Err(CoveError::BadFileCode);
            }
            Ok(Value::String(hex_encode(bytes)))
        }
        ValueTag::Utf8 => {
            let payload = decode_canonical_length_prefixed(bytes)?;
            std::str::from_utf8(payload)
                .map(|value| Value::String(value.to_string()))
                .map_err(|_| CoveError::BadFileCode)
        }
        ValueTag::Binary => {
            let (payload, consumed) = decode_canonical_length_prefixed_consumed(bytes)?;
            if consumed != bytes.len() {
                return Err(CoveError::BadFileCode);
            }
            decode_canonical_binary_value(property, payload)
        }
        ValueTag::Json => {
            let payload = decode_canonical_length_prefixed(bytes)?;
            serde_json::from_slice(payload).map_err(|_| CoveError::BadFileCode)
        }
        ValueTag::List | ValueTag::Struct | ValueTag::Map => {
            let (value, consumed) = decode_canonical_payload_value(property, value_tag, bytes)?;
            if consumed != bytes.len() {
                return Err(CoveError::BadFileCode);
            }
            Ok(value)
        }
    }
}

fn decode_canonical_length_prefixed(bytes: &[u8]) -> Result<&[u8], CoveError> {
    let (payload, consumed) = decode_canonical_length_prefixed_consumed(bytes)?;
    if consumed != bytes.len() {
        return Err(CoveError::BadFileCode);
    }
    Ok(payload)
}

fn decode_canonical_length_prefixed_consumed(bytes: &[u8]) -> Result<(&[u8], usize), CoveError> {
    let (len, consumed) = wire::decode_u64_leb128(bytes)?;
    let len = usize::try_from(len).map_err(|_| CoveError::ArithOverflow)?;
    let end = consumed.checked_add(len).ok_or(CoveError::ArithOverflow)?;
    if end > bytes.len() {
        return Err(CoveError::BadFileCode);
    }
    Ok((&bytes[consumed..end], end))
}

fn decode_canonical_tagged_value(
    property: &PropertyEntryV1,
    bytes: &[u8],
) -> Result<(ValueTag, Value, usize), CoveError> {
    let (raw_tag, tag_len) = wire::decode_u64_leb128(bytes)?;
    let raw_tag = u16::try_from(raw_tag).map_err(|_| CoveError::BadFileCode)?;
    let value_tag = ValueTag::from_u16(raw_tag).ok_or(CoveError::BadFileCode)?;
    let (value, payload_len) =
        decode_canonical_payload_value(property, value_tag, &bytes[tag_len..])?;
    let consumed = tag_len
        .checked_add(payload_len)
        .ok_or(CoveError::ArithOverflow)?;
    Ok((value_tag, value, consumed))
}

fn decode_canonical_payload_value(
    property: &PropertyEntryV1,
    value_tag: ValueTag,
    bytes: &[u8],
) -> Result<(Value, usize), CoveError> {
    match value_tag {
        ValueTag::Null => Ok((Value::Null, 0)),
        ValueTag::BoolFalse => Ok((Value::Bool(false), 0)),
        ValueTag::BoolTrue => Ok((Value::Bool(true), 0)),
        ValueTag::Int64 | ValueTag::TimestampMicros | ValueTag::TimestampNanos => {
            let payload = fixed_canonical_payload(bytes, 8)?;
            Ok((json!(i64::from_le_bytes(payload.try_into().unwrap())), 8))
        }
        ValueTag::UInt64 => {
            let payload = fixed_canonical_payload(bytes, 8)?;
            Ok((json!(u64::from_le_bytes(payload.try_into().unwrap())), 8))
        }
        ValueTag::Float32Bits => {
            let payload = fixed_canonical_payload(bytes, 4)?;
            let value = f32::from_bits(u32::from_le_bytes(payload.try_into().unwrap())) as f64;
            Number::from_f64(value)
                .map(|value| (Value::Number(value), 4))
                .ok_or(CoveError::BadFileCode)
        }
        ValueTag::Float64Bits => {
            let payload = fixed_canonical_payload(bytes, 8)?;
            let value = f64::from_bits(u64::from_le_bytes(payload.try_into().unwrap()));
            Number::from_f64(value)
                .map(|value| (Value::Number(value), 8))
                .ok_or(CoveError::BadFileCode)
        }
        ValueTag::Decimal64 => {
            let payload = fixed_canonical_payload(bytes, 8)?;
            Ok((
                Value::String(i64::from_le_bytes(payload.try_into().unwrap()).to_string()),
                8,
            ))
        }
        ValueTag::Decimal128 => {
            let payload = fixed_canonical_payload(bytes, 16)?;
            Ok((
                Value::String(i128::from_le_bytes(payload.try_into().unwrap()).to_string()),
                16,
            ))
        }
        ValueTag::DateDays => {
            let payload = fixed_canonical_payload(bytes, 4)?;
            Ok((json!(i32::from_le_bytes(payload.try_into().unwrap())), 4))
        }
        ValueTag::Uuid => {
            let payload = fixed_canonical_payload(bytes, 16)?;
            Ok((Value::String(hex_encode(payload)), 16))
        }
        ValueTag::Utf8 => {
            let (payload, consumed) = decode_canonical_length_prefixed_consumed(bytes)?;
            let value = std::str::from_utf8(payload)
                .map(|value| Value::String(value.to_string()))
                .map_err(|_| CoveError::BadFileCode)?;
            Ok((value, consumed))
        }
        ValueTag::Binary => {
            let (payload, consumed) = decode_canonical_length_prefixed_consumed(bytes)?;
            Ok((decode_canonical_binary_value(property, payload)?, consumed))
        }
        ValueTag::Json => {
            let (payload, consumed) = decode_canonical_length_prefixed_consumed(bytes)?;
            Ok((
                serde_json::from_slice(payload).map_err(|_| CoveError::BadFileCode)?,
                consumed,
            ))
        }
        ValueTag::List => {
            let (element_count, mut pos) = wire::decode_u64_leb128(bytes)?;
            let mut elements = Vec::with_capacity(
                usize::try_from(element_count).map_err(|_| CoveError::ArithOverflow)?,
            );
            for _ in 0..element_count {
                let (_, value, consumed) = decode_canonical_tagged_value(property, &bytes[pos..])?;
                pos = pos.checked_add(consumed).ok_or(CoveError::ArithOverflow)?;
                elements.push(value);
            }
            Ok((Value::Array(elements), pos))
        }
        ValueTag::Struct => {
            let (field_count, mut pos) = wire::decode_u64_leb128(bytes)?;
            let mut previous_field_id = None;
            let mut object = serde_json::Map::new();
            for _ in 0..field_count {
                let (field_id, consumed) = wire::decode_u64_leb128(&bytes[pos..])?;
                pos = pos.checked_add(consumed).ok_or(CoveError::ArithOverflow)?;
                if previous_field_id.is_some_and(|previous| field_id <= previous) {
                    return Err(CoveError::BadFileCode);
                }
                previous_field_id = Some(field_id);
                let (_, value, consumed) = decode_canonical_tagged_value(property, &bytes[pos..])?;
                pos = pos.checked_add(consumed).ok_or(CoveError::ArithOverflow)?;
                object.insert(field_id.to_string(), value);
            }
            Ok((Value::Object(object), pos))
        }
        ValueTag::Map => {
            let (pair_count, mut pos) = wire::decode_u64_leb128(bytes)?;
            let mut previous_key = None::<Vec<u8>>;
            let mut entries = Vec::with_capacity(
                usize::try_from(pair_count).map_err(|_| CoveError::ArithOverflow)?,
            );
            for _ in 0..pair_count {
                let key_start = pos;
                let (key_tag, key, consumed) =
                    decode_canonical_tagged_value(property, &bytes[pos..])?;
                if matches!(key_tag, ValueTag::List | ValueTag::Struct | ValueTag::Map) {
                    return Err(CoveError::BadFileCode);
                }
                pos = pos.checked_add(consumed).ok_or(CoveError::ArithOverflow)?;
                let key_bytes = bytes[key_start..pos].to_vec();
                if let Some(previous) = &previous_key {
                    if key_bytes <= *previous {
                        return Err(CoveError::BadFileCode);
                    }
                }
                previous_key = Some(key_bytes);
                let (_, value, consumed) = decode_canonical_tagged_value(property, &bytes[pos..])?;
                pos = pos.checked_add(consumed).ok_or(CoveError::ArithOverflow)?;
                entries.push(Value::Array(vec![key, value]));
            }
            Ok((Value::Array(entries), pos))
        }
    }
}

fn fixed_canonical_payload(bytes: &[u8], width: usize) -> Result<&[u8], CoveError> {
    if bytes.len() < width {
        return Err(CoveError::BadFileCode);
    }
    Ok(&bytes[..width])
}

fn decode_canonical_binary_value(
    property: &PropertyEntryV1,
    bytes: &[u8],
) -> Result<Value, CoveError> {
    if property.physical_kind == CovePhysicalKind::VarBytes {
        return decode_bytes_value(property, bytes);
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => Ok(Value::String(text.to_string())),
        Err(_) => Ok(Value::String(hex_encode(bytes))),
    }
}

fn association_metadata(
    object_type: &ObjectTypeEntryV1,
    properties: &[CoveObjectPropertyValue],
) -> Option<CoveAssociationMetadata> {
    let is_association = object_type.flags
        & (OBJECT_TYPE_FLAG_ASSOCIATION_OBJECT | OBJECT_TYPE_FLAG_LINK_OBJECT)
        != 0
        || object_type.type_name.starts_with("Association:");
    if !is_association {
        return None;
    }
    Some(CoveAssociationMetadata {
        association_type: property_string_by_flag(properties, PROPERTY_FLAG_ASSOCIATION_TYPE)
            .or_else(|| {
                object_type
                    .type_name
                    .strip_prefix("Association:")
                    .map(str::to_string)
            }),
        source_goid: property_string_by_flag(properties, PROPERTY_FLAG_ASSOCIATION_FROM_GOID),
        target_goid: property_string_by_flag(properties, PROPERTY_FLAG_ASSOCIATION_TO_GOID),
        evidence_ref: property_string_by_flag(properties, PROPERTY_FLAG_EVIDENCE_REF),
        mapping_rule_ref: property_string_by_flag(properties, PROPERTY_FLAG_MAPPING_RULE_REF),
    })
}

fn property_string_by_flag(properties: &[CoveObjectPropertyValue], flag: u32) -> Option<String> {
    properties
        .iter()
        .find(|property| property.flags & flag != 0)
        .and_then(|property| json_value_to_string(&property.value))
}

fn json_value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(value) => Some(value.clone()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(value) => Some(value.to_string()),
        other => Some(other.to_string()),
    }
}

fn is_map_section(kind: SectionKind) -> bool {
    matches!(
        kind,
        SectionKind::MapSourceCatalog
            | SectionKind::MapFunctionRegistry
            | SectionKind::MapIdentityRuleCatalog
            | SectionKind::MapRowSemanticsCatalog
            | SectionKind::MapAssertionLog
            | SectionKind::MapIdentityEquivalenceIndex
            | SectionKind::MapEvidenceIndex
            | SectionKind::MapConversionReport
            | SectionKind::MapProjectionCatalog
    )
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::{CanonicalField, CanonicalValue};

    fn property(id: u32, name: &str, value: Value) -> CoveObjectPropertyValue {
        CoveObjectPropertyValue {
            property_id: id,
            property_name: name.into(),
            logical_type: CoveLogicalType::Utf8,
            physical_kind: CovePhysicalKind::VarBytes,
            flags: 0,
            value,
        }
    }

    fn property_entry(logical_type: CoveLogicalType) -> PropertyEntryV1 {
        PropertyEntryV1 {
            property_id: 1,
            property_name: "nested".into(),
            logical_type,
            physical_kind: CovePhysicalKind::FileCode,
            nullable: true,
            collation_id: 0,
            flags: 0,
        }
    }

    fn tagged(tag: ValueTag, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        wire::append_u64_leb128(&mut out, tag as u64);
        out.extend_from_slice(payload);
        out
    }

    fn length_prefixed(payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        wire::append_u64_leb128(&mut out, payload.len() as u64);
        out.extend_from_slice(payload);
        out
    }

    fn record(
        row_index: u32,
        csn: u64,
        kind: RecordKind,
        prev_ref: Option<CoveRecordRefV1>,
        properties: Vec<CoveObjectPropertyValue>,
    ) -> CoveObjectRecord {
        CoveObjectRecord {
            object_type_id: 7,
            object_type_name: "Person".into(),
            object_type_flags: 0,
            segment_id: 1,
            row_index,
            timestamp_us: csn as i64,
            csn,
            branch_key: 0,
            goid: [0x11; 16],
            record_id: [row_index as u8; 16],
            record_kind: kind,
            prev_ref,
            properties,
            association: None,
        }
    }

    fn surface(records: Vec<CoveObjectRecord>) -> CoveObjectSurface {
        CoveObjectSurface {
            object_types: Vec::new(),
            records,
            projection_catalog: None,
            evidence_index: None,
            embedded_map_sections: Vec::new(),
        }
    }

    #[test]
    fn reconstructs_baseline_delta_and_tombstone() {
        let baseline = record(
            0,
            1,
            RecordKind::Baseline,
            None,
            vec![property(1, "name", json!("Ada"))],
        );
        let delta = record(
            1,
            2,
            RecordKind::Delta,
            Some(CoveRecordRefV1 {
                segment_id: 1,
                row_index: 0,
                target_kind: 0,
            }),
            vec![property(2, "city", json!("London"))],
        );
        let states = reconstruct_object_states(
            &surface(vec![baseline.clone(), delta.clone()]),
            &Default::default(),
        )
        .unwrap();
        assert_eq!(states.len(), 1);
        assert_eq!(states[0].properties.len(), 2);
        assert_eq!(states[0].latest_row_index, 1);

        let tombstone = record(
            2,
            3,
            RecordKind::Tombstone,
            Some(CoveRecordRefV1 {
                segment_id: 1,
                row_index: 1,
                target_kind: 0,
            }),
            Vec::new(),
        );
        let live_states = reconstruct_object_states(
            &surface(vec![baseline, delta, tombstone]),
            &Default::default(),
        )
        .unwrap();
        assert!(live_states.is_empty());
    }

    #[test]
    fn rejects_invalid_prev_ref_chain() {
        let delta = record(
            1,
            2,
            RecordKind::Delta,
            Some(CoveRecordRefV1 {
                segment_id: 1,
                row_index: 99,
                target_kind: 0,
            }),
            vec![property(1, "name", json!("Ada"))],
        );
        assert!(matches!(
            reconstruct_object_states(&surface(vec![delta]), &Default::default()),
            Err(CoveError::RefInvalid)
        ));
    }

    #[test]
    fn materializes_nested_canonical_dictionary_values() {
        let property = property_entry(CoveLogicalType::List);
        let bytes = CanonicalValue::List(vec![
            CanonicalValue::Utf8("Ada"),
            CanonicalValue::Struct(vec![CanonicalField {
                field_id: 7,
                value: CanonicalValue::Bool(true),
            }]),
            CanonicalValue::Map(vec![
                (
                    CanonicalValue::Utf8("a"),
                    CanonicalValue::Int { width: 8, value: 1 },
                ),
                (CanonicalValue::Utf8("b"), CanonicalValue::Utf8("two")),
            ]),
        ])
        .encode()
        .unwrap();
        let value = decode_canonical_value_tag(&property, ValueTag::List, &bytes).unwrap();
        assert_eq!(
            value,
            json!([
                "Ada",
                {"7": true},
                [["a", 1], ["b", "two"]]
            ])
        );
    }

    #[test]
    fn rejects_malformed_canonical_nested_payloads() {
        let property = property_entry(CoveLogicalType::Map);
        let key = tagged(ValueTag::Utf8, &length_prefixed(b"k"));
        let first_value = tagged(ValueTag::Utf8, &length_prefixed(b"v1"));
        let second_value = tagged(ValueTag::Utf8, &length_prefixed(b"v2"));
        let mut bad = Vec::new();
        wire::append_u64_leb128(&mut bad, 2);
        bad.extend_from_slice(&key);
        bad.extend_from_slice(&first_value);
        bad.extend_from_slice(&key);
        bad.extend_from_slice(&second_value);

        assert_eq!(
            decode_canonical_value_tag(&property, ValueTag::Map, &bad),
            Err(CoveError::BadFileCode)
        );
    }
}
