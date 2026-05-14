use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use arrow_array::{
    new_empty_array, Array, ArrayRef, BinaryArray, BooleanArray, Date32Array, Decimal128Array,
    Decimal64Array, FixedSizeBinaryArray, Float32Array, Float64Array, Int16Array, Int32Array,
    Int64Array, Int8Array, RecordBatch, StringArray, TimestampMicrosecondArray,
    TimestampNanosecondArray, UInt16Array, UInt32Array, UInt64Array, UInt8Array,
};
use arrow_ipc::writer::FileWriter;
use arrow_json::ReaderBuilder as JsonReaderBuilder;
use arrow_schema::{DataType, Field, Fields, Schema, TimeUnit};
use cove_core::artifact::covemap::{
    CovemapFile, CovemapHeaderV1, CovemapPayloadEncodingV2, CovemapPostscriptV1, CovemapSection,
    CovemapSectionEntryV1,
};
use cove_core::profile::{
    cove_map::{EmbeddedMapSection, MapEvidenceEntry, MapProjectionColumn},
    cove_o::{
        read_object_surface_from_bytes_with_options, reconstruct_object_states,
        CoveObjectReadOptions, CoveObjectRecord, CoveObjectState, CoveObjectSurface, RecordKind,
        OBJECT_TYPE_FLAG_ASSOCIATION_OBJECT, OBJECT_TYPE_FLAG_LINK_OBJECT,
        PROPERTY_FLAG_ASSOCIATION_FROM_GOID, PROPERTY_FLAG_ASSOCIATION_OBSERVED_AT,
        PROPERTY_FLAG_ASSOCIATION_TO_GOID, PROPERTY_FLAG_ASSOCIATION_TYPE,
        PROPERTY_FLAG_ASSOCIATION_VALID_FROM, PROPERTY_FLAG_ASSOCIATION_VALID_TO,
        PROPERTY_FLAG_EVIDENCE_REF, PROPERTY_FLAG_MAPPING_RULE_REF,
    },
};
use cove_core::{
    constants::{CoveLogicalType, CovePhysicalKind},
    encoding::nested::{
        ListLayout, ListLayoutPayload, MapLayout, MapLayoutPayload, StructLayout,
        StructLayoutPayload,
    },
    nested_schema::{NestedSchemaEntryV1, NestedSchemaNodeV1, NestedSchemaSectionV1},
    page_payload::{CoveEncodingNodeV1, PageBufferKind},
    table::{ColumnEntry, TableCatalog, TableEntry},
    writer::{ScanPageSpec, ScanProfileCoveWriter, ScanSegment},
};
use serde_json::{json, Map, Value};

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionFormat {
    Json,
    CoveO,
    Arrow,
    CoveT,
    Sql,
}

impl ProjectionFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::CoveO => "cove-o",
            Self::Arrow => "arrow",
            Self::CoveT => "cove-t",
            Self::Sql => "sql",
        }
    }
}

#[derive(Debug, Clone)]
struct ProjectedColumn {
    name: String,
    logical: CoveLogicalType,
    nested_shape: Option<String>,
}

#[derive(Debug, Clone)]
struct ProjectedTable {
    mapping_id: String,
    mapping_version: String,
    projection_id: String,
    output_table: String,
    columns: Vec<ProjectedColumn>,
    rows: Vec<Map<String, Value>>,
}

pub(crate) fn diff_maps(left: &CovemapFile, right: &CovemapFile) -> Value {
    let left_sections = section_set(left);
    let right_sections = section_set(right);
    let added = right_sections
        .difference(&left_sections)
        .cloned()
        .collect::<Vec<_>>();
    let removed = left_sections
        .difference(&right_sections)
        .cloned()
        .collect::<Vec<_>>();
    let changed = left
        .sections
        .iter()
        .filter_map(|left_section| {
            right
                .sections
                .iter()
                .find(|right_section| {
                    right_section.entry.section_id == left_section.entry.section_id
                })
                .and_then(|right_section| {
                    (sha256_hex(&left_section.payload) != sha256_hex(&right_section.payload))
                        .then(|| section_kind(left_section.entry.section_id))
                })
        })
        .collect::<Vec<_>>();
    json!({
        "mapping_version_changed": left.mapping_version != right.mapping_version,
        "added_sections": added,
        "removed_sections": removed,
        "changed_sections": changed,
    })
}

pub(crate) fn project_rows(file: &CovemapFile, rows: &[SourceRow]) -> Result<Value, String> {
    project_rows_with_source_states(file, rows, &[])
}

pub(crate) fn project_rows_with_source_states(
    file: &CovemapFile,
    rows: &[SourceRow],
    source_states: &[ObservedSourceState],
) -> Result<Value, String> {
    let bytes = project_rows_with_source_states_output(
        file,
        rows,
        source_states,
        ProjectionFormat::Json,
        None,
    )?;
    serde_json::from_slice(&bytes).map_err(|err| format!("projection JSON encoding failed: {err}"))
}

pub(crate) fn project_rows_with_source_states_output(
    file: &CovemapFile,
    rows: &[SourceRow],
    source_states: &[ObservedSourceState],
    format: ProjectionFormat,
    projection_id: Option<&str>,
) -> Result<Vec<u8>, String> {
    let materialized = materialize_with_source_states(file, rows, source_states)?;
    let model = ProjectionModel::from_materialized(&materialized);
    let projection_catalog = projection_catalog(file)?
        .ok_or_else(|| "project requires a MAP_PROJECTION_CATALOG section".to_string())?;
    let function_ids = function_registry(file)?;
    let tables = project_tables(
        &model,
        &projection_catalog,
        &function_ids,
        projection_id,
        format,
    )?;
    encode_projection_output(format, &projection_catalog, &tables)
}

pub(crate) fn project_cove_o_path(object: &Path, mapping: Option<&Path>) -> Result<Value, String> {
    let bytes = project_cove_o_path_output(object, mapping, ProjectionFormat::Json, None)?;
    serde_json::from_slice(&bytes).map_err(|err| format!("projection JSON encoding failed: {err}"))
}

pub(crate) fn project_cove_o_path_output(
    object: &Path,
    mapping: Option<&Path>,
    format: ProjectionFormat,
    projection_id: Option<&str>,
) -> Result<Vec<u8>, String> {
    let bytes =
        fs::read(object).map_err(|err| format!("cannot read {}: {err}", object.display()))?;
    let catalog_surface = read_object_surface_from_bytes_with_options(
        &bytes,
        &CoveObjectReadOptions::requested_property_ids([u32::MAX]),
    )
    .map_err(|err| format!("{}: {err}", object.display()))?;
    let projection_catalog = match &catalog_surface.projection_catalog {
        Some(catalog) => catalog.clone(),
        None => {
            let mapping = mapping.ok_or_else(|| {
                "project-cove-o requires embedded MAP_PROJECTION_CATALOG or --mapping <mapping.covemap>"
                    .to_string()
            })?;
            let file = parse_map(mapping)?;
            projection_catalog(&file)?.ok_or_else(|| {
                "fallback mapping requires a MAP_PROJECTION_CATALOG section".to_string()
            })?
        }
    };
    let read_options = CoveObjectReadOptions::requested_property_names(
        requested_property_names_for_catalog(&projection_catalog),
    );
    let surface = read_object_surface_from_bytes_with_options(&bytes, &read_options)
        .map_err(|err| format!("{}: {err}", object.display()))?;
    let model = ProjectionModel::from_surface(&surface).map_err(|err| err.to_string())?;
    let function_ids = match mapping {
        Some(mapping) => function_registry(&parse_map(mapping)?)?,
        None => embedded_function_registry(&surface.embedded_map_sections),
    };
    let tables = project_tables(
        &model,
        &projection_catalog,
        &function_ids,
        projection_id,
        format,
    )?;
    encode_projection_output(format, &projection_catalog, &tables)
}

fn projection_catalog(file: &CovemapFile) -> Result<Option<MapProjectionCatalog>, String> {
    for section in embedded_sections(file)? {
        if let EmbeddedMapSection::ProjectionCatalog(catalog) = section {
            return Ok(Some(catalog));
        }
    }
    Ok(None)
}

fn function_registry(file: &CovemapFile) -> Result<std::collections::BTreeSet<String>, String> {
    Ok(embedded_function_registry(&embedded_sections(file)?))
}

fn embedded_function_registry(
    sections: &[EmbeddedMapSection],
) -> std::collections::BTreeSet<String> {
    let mut ids = std::collections::BTreeSet::new();
    for section in sections {
        if let EmbeddedMapSection::FunctionRegistry(registry) = section {
            ids.extend(
                registry
                    .functions
                    .iter()
                    .map(|function| function.function_id.clone()),
            );
        }
    }
    ids
}

fn project_tables(
    model: &ProjectionModel,
    catalog: &MapProjectionCatalog,
    function_ids: &std::collections::BTreeSet<String>,
    projection_id: Option<&str>,
    format: ProjectionFormat,
) -> Result<Vec<ProjectedTable>, String> {
    let selected = catalog
        .projections
        .iter()
        .filter(|projection| {
            projection_id
                .map(|requested| projection.projection_id == requested)
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err(match projection_id {
            Some(id) => format!("projection_id '{id}' was not found"),
            None => "projection catalog contains no projections".to_string(),
        });
    }
    if matches!(format, ProjectionFormat::Arrow | ProjectionFormat::CoveT)
        && projection_id.is_none()
        && selected.len() != 1
    {
        return Err("--projection-id is required for Arrow or COVE-T output when a catalog contains multiple projections".into());
    }

    let mut tables = Vec::new();
    for projection in selected {
        validate_executable_projection(projection, model, function_ids)?;
        ensure_projection_declares_format(projection, format)?;
        let rows = project_one(model, projection)?
            .into_iter()
            .map(|value| match value {
                Value::Object(row) => Ok(projected_table_row(projection, row)),
                _ => Err("projection produced a non-object row".to_string()),
            })
            .collect::<Result<Vec<_>, _>>()?;
        tables.push(ProjectedTable {
            mapping_id: catalog.mapping_id.clone(),
            mapping_version: catalog.mapping_version.clone(),
            projection_id: projection.projection_id.clone(),
            output_table: projection
                .output_table
                .clone()
                .unwrap_or_else(|| projection.projection_id.clone()),
            columns: projection
                .columns
                .iter()
                .map(projected_column_from_entry)
                .collect::<Result<Vec<_>, _>>()?,
            rows,
        });
    }
    Ok(tables)
}

fn projected_column_from_entry(column: &MapProjectionColumn) -> Result<ProjectedColumn, String> {
    let logical = projection_column_logical_type(column)?;
    if matches!(logical, CoveLogicalType::Null) {
        return Err(format!(
            "projection column '{}' declares null logical type; use a concrete scalar logical_type",
            column.name
        ));
    }
    if matches!(
        logical,
        CoveLogicalType::List | CoveLogicalType::Struct | CoveLogicalType::Map
    ) && column.nested_shape.is_none()
    {
        return Err(format!(
            "projection column '{}' declares nested logical type {:?} without nested_shape",
            column.name, logical
        ));
    }
    Ok(ProjectedColumn {
        name: column.name.clone(),
        logical,
        nested_shape: column.nested_shape.clone(),
    })
}

fn projection_column_logical_type(column: &MapProjectionColumn) -> Result<CoveLogicalType, String> {
    match column.logical_type.as_deref().unwrap_or("utf8") {
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
        other => Err(format!(
            "projection column '{}' declares unsupported logical_type '{other}'",
            column.name
        )),
    }
}

fn projected_table_row(
    projection: &MapProjectionEntry,
    mut row: Map<String, Value>,
) -> Map<String, Value> {
    let mut ordered = Map::new();
    for column in &projection.columns {
        ordered.insert(
            column.name.clone(),
            row.remove(&column.name).unwrap_or(Value::Null),
        );
    }
    ordered
}

fn ensure_projection_declares_format(
    projection: &MapProjectionEntry,
    format: ProjectionFormat,
) -> Result<(), String> {
    let mode = format.as_str();
    if projection.output_modes.iter().any(|value| value == mode) {
        Ok(())
    } else {
        Err(format!(
            "projection '{}' does not declare executable output mode '{mode}'",
            projection.projection_id
        ))
    }
}

fn encode_projection_output(
    format: ProjectionFormat,
    catalog: &MapProjectionCatalog,
    tables: &[ProjectedTable],
) -> Result<Vec<u8>, String> {
    match format {
        ProjectionFormat::Json => serde_json::to_vec_pretty(&json!({
            "format": "json",
            "mapping_id": catalog.mapping_id,
            "mapping_version": catalog.mapping_version,
            "rows": tables.iter()
                .flat_map(|table| table.rows.iter().map(|row| {
                    let mut out = Map::new();
                    out.insert("projection_id".into(), json!(table.projection_id));
                    out.insert("output_table".into(), json!(table.output_table));
                    for (key, value) in row {
                        out.insert(key.clone(), value.clone());
                    }
                    Value::Object(out)
                }))
                .collect::<Vec<_>>(),
        }))
        .map_err(|err| format!("cannot encode projection JSON: {err}")),
        ProjectionFormat::CoveO => encode_cove_o_projection(tables),
        ProjectionFormat::Sql => encode_sql_projection(tables),
        ProjectionFormat::Arrow => {
            let table = single_projection_table(tables, "Arrow")?;
            encode_arrow_projection(table)
        }
        ProjectionFormat::CoveT => {
            let table = single_projection_table(tables, "COVE-T")?;
            encode_cove_t_projection(table)
        }
    }
}

fn single_projection_table<'a>(
    tables: &'a [ProjectedTable],
    label: &str,
) -> Result<&'a ProjectedTable, String> {
    match tables {
        [table] => Ok(table),
        _ => Err(format!(
            "{label} projection output requires exactly one projection"
        )),
    }
}

fn encode_cove_o_projection(tables: &[ProjectedTable]) -> Result<Vec<u8>, String> {
    if tables.is_empty() {
        return Err("COVE-O projection output requires at least one projection".into());
    }
    let (mapping_id, mapping_version) = (&tables[0].mapping_id, &tables[0].mapping_version);
    let mut sources = Vec::new();
    let mut identity_rules = Vec::new();
    let mut row_rules = Vec::new();
    let mut rows = Vec::new();
    let mut states = Vec::new();

    for table in tables {
        let source_id = projection_source_id(table);
        sources.push(json!({
            "source_id": source_id,
            "schema_fingerprint": projection_schema_fingerprint(table),
            "snapshot_digest": projection_snapshot_digest(table),
            "row_identity_rules": [projection_identity_rule_id(table)],
            "replay_claimed": true
        }));
        identity_rules.push(json!({
            "rule_id": projection_identity_rule_id(table),
            "object_type": table.output_table,
            "semantic_role": "projection_row",
            "confidence_class": "synthetic",
            "candidate_only": false,
            "property_conflicts_declared": true,
            "function_ids": ["identity"],
            "join_keys": [{
                "role_id": "projection_row",
                "source_column": "__projection_key",
                "logical_type": "utf8",
                "canonicalization": "identity",
                "null_policy": "reject",
                "ordering": "asc"
            }]
        }));
        row_rules.push(json!({
            "rule_id": projection_row_rule_id(table),
            "source_id": source_id,
            "identity_rule_id": projection_identity_rule_id(table),
            "row_semantics_kind": "Object",
            "source_operation_kind": "Upsert",
            "assertion_kinds": ["object", "property", "evidence"],
            "property_bindings": table.columns.iter().map(|column| json!({
                "assertion_id": format!("assert_{}_{}", table.projection_id, column.name),
                "property_id": column.name,
                "property_name": column.name,
                "source_column": column.name,
                "logical_type": cove_o_projection_logical_name(column),
                "physical_kind": "auto",
                "nullable": true,
                "missing_policy": "null",
                "conflict_policy": "reject_conflict"
            })).collect::<Vec<_>>()
        }));

        for (ordinal, row) in table.rows.iter().enumerate() {
            let mut values = row.clone().into_iter().collect::<BTreeMap<_, _>>();
            values.insert(
                "__projection_key".into(),
                json!(format!(
                    "{}:{}:{}:{}:{}",
                    table.mapping_id,
                    table.mapping_version,
                    table.projection_id,
                    table.output_table,
                    ordinal
                )),
            );
            rows.push(SourceRow {
                source_id: source_id.clone(),
                row_index: ordinal,
                values,
            });
        }
        states.push(ObservedSourceState {
            source_id,
            source_kind: "cove-map-projection".into(),
            schema_fingerprint: projection_schema_fingerprint(table),
            snapshot_digest: projection_snapshot_digest(table),
        });
    }

    let file = CovemapFile {
        header: CovemapHeaderV1::new(first_16(&sha256_array(mapping_id.as_bytes())), 0),
        mapping_version: mapping_version.clone(),
        sections: vec![
            projection_covemap_section(
                SectionKind::MapSourceCatalog,
                json!({
                    "mapping_id": mapping_id,
                    "mapping_version": mapping_version,
                    "sources": sources
                }),
            )?,
            projection_covemap_section(
                SectionKind::MapFunctionRegistry,
                json!({
                    "mapping_id": mapping_id,
                    "mapping_version": mapping_version,
                    "functions": [{
                        "function_id": "identity",
                        "version": "1.0.0",
                        "deterministic": true,
                        "dependency": "pure"
                    }]
                }),
            )?,
            projection_covemap_section(
                SectionKind::MapIdentityRuleCatalog,
                json!({
                    "mapping_id": mapping_id,
                    "mapping_version": mapping_version,
                    "identity_rules": identity_rules,
                    "do_not_merge": []
                }),
            )?,
            projection_covemap_section(
                SectionKind::MapRowSemanticsCatalog,
                json!({
                    "mapping_id": mapping_id,
                    "mapping_version": mapping_version,
                    "rules": row_rules
                }),
            )?,
            projection_covemap_section(
                SectionKind::MapProjectionCatalog,
                json!({
                    "mapping_id": mapping_id,
                    "mapping_version": mapping_version,
                    "projections": tables.iter().map(projected_table_catalog_entry).collect::<Vec<_>>()
                }),
            )?,
        ],
        postscript: CovemapPostscriptV1 {
            required_features: 0,
            optional_features: 0,
            file_len: 0,
            header_offset: 0,
            header_length: 0,
            checksum: 0,
        },
    };
    build_cove_o_with_source_states(&file, &rows, &states)
}

fn projection_source_id(table: &ProjectedTable) -> String {
    format!("projection.{}", table.projection_id)
}

fn projection_identity_rule_id(table: &ProjectedTable) -> String {
    format!("identity_{}", table.projection_id)
}

fn projection_row_rule_id(table: &ProjectedTable) -> String {
    format!("materialize_{}", table.projection_id)
}

fn projection_schema_fingerprint(table: &ProjectedTable) -> String {
    format!(
        "cove-map-projection-schema-v1:{}",
        sha256_hex(
            serde_json::to_string(&projected_table_catalog_entry(table))
                .unwrap_or_default()
                .as_bytes()
        )
    )
}

fn projection_snapshot_digest(table: &ProjectedTable) -> String {
    let rows = table
        .rows
        .iter()
        .cloned()
        .map(Value::Object)
        .collect::<Vec<_>>();
    format!(
        "sha256:{}",
        sha256_hex(serde_json::to_string(&rows).unwrap_or_default().as_bytes())
    )
}

fn projected_table_catalog_entry(table: &ProjectedTable) -> Value {
    json!({
        "projection_id": table.projection_id,
        "output_table": table.output_table,
        "row_grain": "one_row_per_object",
        "multi_value_policy": "reject",
        "columns": table.columns.iter().map(|column| {
            let mut value = json!({
                "name": column.name,
                "value": format!("property.{}", column.name),
                "logical_type": projection_logical_type_name(column.logical),
                "missing_policy": "null"
            });
            if let (Some(object), Some(shape)) = (value.as_object_mut(), &column.nested_shape) {
                object.insert("nested_shape".into(), json!(shape));
            }
            value
        }).collect::<Vec<_>>(),
        "output_modes": ["cove-o", "json"]
    })
}

fn projection_covemap_section(
    kind: SectionKind,
    mut value: Value,
) -> Result<CovemapSection, String> {
    if let Value::Object(object) = &mut value {
        object.insert(
            "schema_id".to_string(),
            Value::String("org.coveformat.covemap.v2".to_string()),
        );
        object.insert(
            "section_id".to_string(),
            Value::Number((kind as u16).into()),
        );
    }
    let payload = serde_json::to_vec_pretty(&value)
        .map_err(|err| format!("cannot encode synthetic projection COVE-MAP section: {err}"))?;
    Ok(CovemapSection {
        entry: CovemapSectionEntryV1 {
            section_id: kind as u32,
            offset: 0,
            length: payload.len() as u64,
            uncompressed_length: payload.len() as u64,
            compression: 0,
            payload_encoding: CovemapPayloadEncodingV2::CoveMapJsonV2 as u8,
            required: true,
            reserved: 0,
            checksum: 0,
        },
        payload,
    })
}

fn cove_o_projection_logical_name(column: &ProjectedColumn) -> &'static str {
    projection_logical_type_name(column.logical)
}

fn projection_logical_type_name(logical: CoveLogicalType) -> &'static str {
    match logical {
        CoveLogicalType::Null => "null",
        CoveLogicalType::Bool => "bool",
        CoveLogicalType::Int8 => "int8",
        CoveLogicalType::Int16 => "int16",
        CoveLogicalType::Int32 => "int32",
        CoveLogicalType::Int64 => "int64",
        CoveLogicalType::UInt8 => "uint8",
        CoveLogicalType::UInt16 => "uint16",
        CoveLogicalType::UInt32 => "uint32",
        CoveLogicalType::UInt64 => "uint64",
        CoveLogicalType::Float32 => "float32",
        CoveLogicalType::Float64 => "float64",
        CoveLogicalType::Decimal64 => "decimal64",
        CoveLogicalType::Decimal128 => "decimal128",
        CoveLogicalType::DateDays => "date_days",
        CoveLogicalType::TimestampMicros => "timestamp_micros",
        CoveLogicalType::TimestampNanos => "timestamp_nanos",
        CoveLogicalType::Utf8 => "utf8",
        CoveLogicalType::Binary => "binary",
        CoveLogicalType::Uuid => "uuid",
        CoveLogicalType::Json => "json",
        CoveLogicalType::List => "list",
        CoveLogicalType::Struct => "struct",
        CoveLogicalType::Map => "map",
        _ => "json",
    }
}

fn projection_logical_type_from_name(name: &str) -> Result<CoveLogicalType, String> {
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
        other => Err(format!("unsupported nested_shape logical_type '{other}'")),
    }
}

fn encode_arrow_projection(table: &ProjectedTable) -> Result<Vec<u8>, String> {
    let fields = table
        .columns
        .iter()
        .map(|column| Ok::<Field, String>(Field::new(&column.name, arrow_data_type(column)?, true)))
        .collect::<Result<Vec<_>, _>>()?;
    let schema = Arc::new(Schema::new(fields));
    let arrays = table
        .columns
        .iter()
        .map(|column| encode_arrow_column(table, column))
        .collect::<Result<Vec<_>, _>>()?;
    let batch = RecordBatch::try_new(Arc::clone(&schema), arrays)
        .map_err(|err| format!("cannot build Arrow record batch: {err}"))?;
    let mut bytes = Vec::new();
    {
        let mut writer = FileWriter::try_new(&mut bytes, &schema)
            .map_err(|err| format!("cannot create Arrow IPC writer: {err}"))?;
        writer
            .write(&batch)
            .map_err(|err| format!("cannot write Arrow IPC batch: {err}"))?;
        writer
            .finish()
            .map_err(|err| format!("cannot finish Arrow IPC file: {err}"))?;
    }
    Ok(bytes)
}

fn arrow_data_type(column: &ProjectedColumn) -> Result<DataType, String> {
    match column.logical {
        CoveLogicalType::List | CoveLogicalType::Struct | CoveLogicalType::Map => {
            nested_arrow_data_type(column)
        }
        logical => arrow_data_type_for_logical(logical),
    }
}

fn arrow_data_type_for_logical(logical: CoveLogicalType) -> Result<DataType, String> {
    match logical {
        CoveLogicalType::Null | CoveLogicalType::Utf8 => Ok(DataType::Utf8),
        CoveLogicalType::Bool => Ok(DataType::Boolean),
        CoveLogicalType::Int8 => Ok(DataType::Int8),
        CoveLogicalType::Int16 => Ok(DataType::Int16),
        CoveLogicalType::Int32 => Ok(DataType::Int32),
        CoveLogicalType::Int64 => Ok(DataType::Int64),
        CoveLogicalType::UInt8 => Ok(DataType::UInt8),
        CoveLogicalType::UInt16 => Ok(DataType::UInt16),
        CoveLogicalType::UInt32 => Ok(DataType::UInt32),
        CoveLogicalType::UInt64 => Ok(DataType::UInt64),
        CoveLogicalType::Float32 => Ok(DataType::Float32),
        CoveLogicalType::Float64 => Ok(DataType::Float64),
        CoveLogicalType::Decimal64 => Ok(DataType::Decimal64(18, 0)),
        CoveLogicalType::Decimal128 => Ok(DataType::Decimal128(38, 0)),
        CoveLogicalType::DateDays => Ok(DataType::Date32),
        CoveLogicalType::TimestampMicros => Ok(DataType::Timestamp(TimeUnit::Microsecond, None)),
        CoveLogicalType::TimestampNanos => Ok(DataType::Timestamp(TimeUnit::Nanosecond, None)),
        CoveLogicalType::Binary | CoveLogicalType::Json => Ok(DataType::Binary),
        CoveLogicalType::Uuid => Ok(DataType::FixedSizeBinary(16)),
        CoveLogicalType::List | CoveLogicalType::Struct | CoveLogicalType::Map => Err(format!(
            "nested logical type {logical:?} requires nested_shape"
        )),
        _ => Err(format!(
            "unknown projection logical type {logical:?} is not supported for Arrow output"
        )),
    }
}

fn nested_arrow_data_type(column: &ProjectedColumn) -> Result<DataType, String> {
    let shape = column
        .nested_shape
        .as_deref()
        .ok_or_else(|| format!("projection column '{}' requires nested_shape", column.name))?;
    let value: Value = serde_json::from_str(shape).map_err(|err| {
        format!(
            "projection column '{}' has invalid nested_shape JSON: {err}",
            column.name
        )
    })?;
    let data_type = nested_shape_data_type(&value)?;
    match (&column.logical, &data_type) {
        (CoveLogicalType::List, DataType::List(_))
        | (CoveLogicalType::Struct, DataType::Struct(_))
        | (CoveLogicalType::Map, DataType::Map(_, _)) => Ok(data_type),
        _ => Err(format!(
            "projection column '{}' nested_shape does not match logical type {:?}",
            column.name, column.logical
        )),
    }
}

fn nested_shape_data_type(value: &Value) -> Result<DataType, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "nested_shape must be a JSON object".to_string())?;
    let kind = object
        .get("type")
        .or_else(|| object.get("kind"))
        .and_then(Value::as_str)
        .ok_or_else(|| "nested_shape requires type".to_string())?;
    match kind {
        "list" => {
            let item = object
                .get("item")
                .or_else(|| object.get("element"))
                .ok_or_else(|| "list nested_shape requires item".to_string())?;
            let field = nested_shape_field("item", item, true)?;
            Ok(DataType::List(Arc::new(field)))
        }
        "struct" => {
            let fields = object
                .get("fields")
                .and_then(Value::as_array)
                .ok_or_else(|| "struct nested_shape requires fields array".to_string())?;
            let fields = fields
                .iter()
                .map(|field| {
                    let name = field
                        .get("name")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "struct field nested_shape requires name".to_string())?;
                    nested_shape_field(name, field, true)
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(DataType::Struct(Fields::from(fields)))
        }
        "map" => {
            let key = object
                .get("key")
                .ok_or_else(|| "map nested_shape requires key".to_string())?;
            let value = object
                .get("value")
                .ok_or_else(|| "map nested_shape requires value".to_string())?;
            let key = nested_shape_field("key", key, false)?;
            let value = nested_shape_field("value", value, true)?;
            Ok(Field::new_map(
                "map",
                "entries",
                Arc::new(key),
                Arc::new(value),
                false,
                true,
            )
            .data_type()
            .clone())
        }
        other => Err(format!("unsupported nested_shape type '{other}'")),
    }
}

fn nested_shape_field(name: &str, value: &Value, default_nullable: bool) -> Result<Field, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "nested_shape field must be an object".to_string())?;
    let nullable = object
        .get("nullable")
        .and_then(Value::as_bool)
        .unwrap_or(default_nullable);
    let data_type = if object
        .get("type")
        .or_else(|| object.get("kind"))
        .and_then(Value::as_str)
        .is_some_and(|kind| matches!(kind, "list" | "struct" | "map"))
    {
        nested_shape_data_type(value)?
    } else {
        let logical = object
            .get("logical_type")
            .or_else(|| object.get("logical"))
            .or_else(|| object.get("type"))
            .and_then(Value::as_str)
            .ok_or_else(|| "nested_shape field requires logical_type".to_string())?;
        arrow_data_type_for_logical(projection_logical_type_from_name(logical)?)?
    };
    Ok(Field::new(name, data_type, nullable))
}

fn encode_arrow_column(
    table: &ProjectedTable,
    column: &ProjectedColumn,
) -> Result<ArrayRef, String> {
    let values = table
        .rows
        .iter()
        .map(|row| row.get(&column.name).unwrap_or(&Value::Null))
        .collect::<Vec<_>>();
    match column.logical {
        CoveLogicalType::Null | CoveLogicalType::Utf8 => Ok(Arc::new(StringArray::from(
            values
                .iter()
                .map(|value| typed_string_value(value, column.logical))
                .collect::<Result<Vec<_>, _>>()?,
        )) as ArrayRef),
        CoveLogicalType::Bool => Ok(Arc::new(BooleanArray::from(
            values
                .iter()
                .map(|value| typed_bool_value(value))
                .collect::<Result<Vec<_>, _>>()?,
        )) as ArrayRef),
        CoveLogicalType::Int8 => primitive_array::<Int8Array, i8>(&values, column.logical),
        CoveLogicalType::Int16 => primitive_array::<Int16Array, i16>(&values, column.logical),
        CoveLogicalType::Int32 => primitive_array::<Int32Array, i32>(&values, column.logical),
        CoveLogicalType::Int64 => primitive_array::<Int64Array, i64>(&values, column.logical),
        CoveLogicalType::UInt8 => primitive_array::<UInt8Array, u8>(&values, column.logical),
        CoveLogicalType::UInt16 => primitive_array::<UInt16Array, u16>(&values, column.logical),
        CoveLogicalType::UInt32 => primitive_array::<UInt32Array, u32>(&values, column.logical),
        CoveLogicalType::UInt64 => primitive_array::<UInt64Array, u64>(&values, column.logical),
        CoveLogicalType::Float32 => Ok(Arc::new(Float32Array::from(
            values
                .iter()
                .map(|value| typed_f64_value(value).map(|value| value.map(|value| value as f32)))
                .collect::<Result<Vec<_>, _>>()?,
        )) as ArrayRef),
        CoveLogicalType::Float64 => Ok(Arc::new(Float64Array::from(
            values
                .iter()
                .map(|value| typed_f64_value(value))
                .collect::<Result<Vec<_>, _>>()?,
        )) as ArrayRef),
        CoveLogicalType::Decimal64 => Ok(Arc::new(
            Decimal64Array::from(
                values
                    .iter()
                    .map(|value| {
                        typed_i128_value(value, column.logical).and_then(|value| {
                            value
                                .map(|value| {
                                    i64::try_from(value).map_err(|_| {
                                        "decimal64 projection value is outside i64 range"
                                            .to_string()
                                    })
                                })
                                .transpose()
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            )
            .with_precision_and_scale(18, 0)
            .map_err(|err| format!("cannot assign decimal64 precision/scale: {err}"))?,
        ) as ArrayRef),
        CoveLogicalType::Decimal128 => Ok(Arc::new(
            Decimal128Array::from(
                values
                    .iter()
                    .map(|value| typed_i128_value(value, column.logical))
                    .collect::<Result<Vec<_>, _>>()?,
            )
            .with_precision_and_scale(38, 0)
            .map_err(|err| format!("cannot assign decimal128 precision/scale: {err}"))?,
        ) as ArrayRef),
        CoveLogicalType::DateDays => primitive_array::<Date32Array, i32>(&values, column.logical),
        CoveLogicalType::TimestampMicros => {
            primitive_array::<TimestampMicrosecondArray, i64>(&values, column.logical)
        }
        CoveLogicalType::TimestampNanos => {
            primitive_array::<TimestampNanosecondArray, i64>(&values, column.logical)
        }
        CoveLogicalType::Binary | CoveLogicalType::Json => {
            let owned = values
                .iter()
                .map(|value| typed_bytes_value(value, column.logical))
                .collect::<Result<Vec<_>, _>>()?;
            let borrowed = owned
                .iter()
                .map(|value| value.as_deref())
                .collect::<Vec<_>>();
            Ok(Arc::new(BinaryArray::from_opt_vec(borrowed)) as ArrayRef)
        }
        CoveLogicalType::Uuid => {
            let owned = values
                .iter()
                .map(|value| typed_uuid_value(value).map(|value| value.map(Vec::from)))
                .collect::<Result<Vec<_>, _>>()?;
            let borrowed = owned
                .iter()
                .map(|value| value.as_deref())
                .collect::<Vec<_>>();
            Ok(Arc::new(FixedSizeBinaryArray::from(borrowed)) as ArrayRef)
        }
        CoveLogicalType::List | CoveLogicalType::Struct | CoveLogicalType::Map => {
            encode_arrow_nested_column(table, column, &values)
        }
        _ => Err(format!(
            "unknown projection logical type {:?} is not supported for Arrow output",
            column.logical
        )),
    }
}

fn encode_arrow_nested_column(
    _table: &ProjectedTable,
    column: &ProjectedColumn,
    values: &[&Value],
) -> Result<ArrayRef, String> {
    let data_type = arrow_data_type(column)?;
    if values.is_empty() {
        return Ok(new_empty_array(&data_type));
    }
    let field = Arc::new(Field::new(&column.name, data_type, true));
    let mut json_lines = String::new();
    for value in values {
        json_lines.push_str(
            &serde_json::to_string(value)
                .map_err(|err| format!("cannot encode nested projection JSON value: {err}"))?,
        );
        json_lines.push('\n');
    }
    let mut reader = JsonReaderBuilder::new_with_field(field)
        .with_batch_size(values.len().max(1))
        .build(json_lines.as_bytes())
        .map_err(|err| format!("cannot build nested Arrow projection decoder: {err}"))?;
    match reader.next() {
        Some(Ok(batch)) => Ok(batch.column(0).clone()),
        Some(Err(err)) => Err(format!(
            "cannot encode nested Arrow projection column: {err}"
        )),
        None => Ok(new_empty_array(&arrow_data_type(column)?)),
    }
}

fn primitive_array<A, T>(values: &[&Value], logical: CoveLogicalType) -> Result<ArrayRef, String>
where
    A: From<Vec<Option<T>>> + Array + 'static,
    T: TryFrom<i128>,
{
    let converted = values
        .iter()
        .map(|value| {
            typed_i128_value(value, logical).and_then(|value| {
                value
                    .map(|value| {
                        T::try_from(value)
                            .map_err(|_| format!("projection value is outside {logical:?} range"))
                    })
                    .transpose()
            })
        })
        .collect::<Vec<_>>();
    let converted = converted.into_iter().collect::<Result<Vec<_>, _>>()?;
    Ok(Arc::new(A::from(converted)) as ArrayRef)
}

fn encode_cove_t_projection(table: &ProjectedTable) -> Result<Vec<u8>, String> {
    let columns = table
        .columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            Ok(ColumnEntry {
                column_id: (index + 1) as u32,
                name: column.name.clone(),
                logical: column.logical,
                physical: projection_physical_kind(column.logical)?,
                nullable: true,
                sort_order: 0,
                collation_id: 0,
                precision: 0,
                scale: 0,
                flags: 0,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let row_count =
        u32::try_from(table.rows.len()).map_err(|_| "too many projection rows".to_string())?;
    let catalog = TableCatalog {
        flags: 0,
        tables: vec![TableEntry {
            table_id: 1,
            namespace: table.mapping_id.clone(),
            name: table.output_table.clone(),
            row_count: table.rows.len() as u64,
            primary_sort_key_count: 0,
            clustering_key_count: 0,
            flags: 0,
            columns,
        }],
    };
    let mut segment = ScanSegment::new(1, 0, 0, row_count, table.columns.len() as u32);
    segment.morsel_row_count = row_count.max(1);
    for (index, column) in table.columns.iter().enumerate() {
        let physical = projection_physical_kind(column.logical)?;
        let (payload, null_count) = if matches!(
            physical,
            CovePhysicalKind::List | CovePhysicalKind::Struct | CovePhysicalKind::Map
        ) {
            (
                projection_nested_payload(table, column, (index + 1) as u32)?,
                0,
            )
        } else {
            projection_physical_payload(table, column, physical)?
        };
        let page = ScanPageSpec::new(row_count, payload)
            .with_encoding_root(projection_encoding_kind(physical) as u32)
            .with_counts(row_count.saturating_sub(null_count), null_count);
        segment.set_column_pages((index + 1) as u32, vec![page]);
    }
    let mut writer = ScanProfileCoveWriter::new(catalog);
    let nested_entries = table
        .columns
        .iter()
        .enumerate()
        .filter(|(_, column)| {
            matches!(
                column.logical,
                CoveLogicalType::List | CoveLogicalType::Struct | CoveLogicalType::Map
            )
        })
        .map(|(index, column)| {
            Ok(NestedSchemaEntryV1 {
                table_id: 1,
                column_id: (index + 1) as u32,
                root: nested_schema_node_for_column(column)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    if !nested_entries.is_empty() {
        writer
            .push_nested_schema(&NestedSchemaSectionV1::new(nested_entries))
            .map_err(|err| err.to_string())?;
    }
    writer.metadata_json = serde_json::to_vec(&json!({
        "projection_id": table.projection_id,
        "mapping_id": table.mapping_id,
        "mapping_version": table.mapping_version,
    }))
    .map_err(|err| err.to_string())?;
    writer.push_segment(segment);
    writer.write().map_err(|err| err.to_string())
}

fn projection_physical_kind(logical: CoveLogicalType) -> Result<CovePhysicalKind, String> {
    match logical {
        CoveLogicalType::Bool => Ok(CovePhysicalKind::Boolean),
        CoveLogicalType::Int8
        | CoveLogicalType::Int16
        | CoveLogicalType::Int32
        | CoveLogicalType::Int64
        | CoveLogicalType::UInt8
        | CoveLogicalType::UInt16
        | CoveLogicalType::UInt32
        | CoveLogicalType::UInt64
        | CoveLogicalType::Float32
        | CoveLogicalType::Float64
        | CoveLogicalType::Decimal64
        | CoveLogicalType::DateDays
        | CoveLogicalType::TimestampMicros
        | CoveLogicalType::TimestampNanos => Ok(CovePhysicalKind::NumCode),
        CoveLogicalType::Decimal128 | CoveLogicalType::Uuid => Ok(CovePhysicalKind::FixedBytes),
        CoveLogicalType::Utf8 | CoveLogicalType::Binary | CoveLogicalType::Json => {
            Ok(CovePhysicalKind::VarBytes)
        }
        CoveLogicalType::List => Ok(CovePhysicalKind::List),
        CoveLogicalType::Struct => Ok(CovePhysicalKind::Struct),
        CoveLogicalType::Map => Ok(CovePhysicalKind::Map),
        CoveLogicalType::Null => Err(format!(
            "projection logical type {logical:?} is not supported for COVE-T output"
        )),
        _ => Err(format!("unknown projection logical type {logical:?}")),
    }
}

fn projection_encoding_kind(physical: CovePhysicalKind) -> CoveEncodingKind {
    match physical {
        CovePhysicalKind::Boolean | CovePhysicalKind::FixedBytes => CoveEncodingKind::PlainFixed,
        CovePhysicalKind::NumCode => CoveEncodingKind::NumCode,
        CovePhysicalKind::VarBytes => CoveEncodingKind::VarBytes,
        CovePhysicalKind::FileCode => CoveEncodingKind::FileCode,
        CovePhysicalKind::List | CovePhysicalKind::Struct | CovePhysicalKind::Map => {
            CoveEncodingKind::Canonical
        }
        _ => CoveEncodingKind::Canonical,
    }
}

fn projection_physical_payload(
    table: &ProjectedTable,
    column: &ProjectedColumn,
    physical: CovePhysicalKind,
) -> Result<(Vec<u8>, u32), String> {
    let mut null_bitmap = vec![0u8; (table.rows.len() + 7) / 8];
    let mut values = Vec::new();
    let mut null_count = 0u32;
    for (row_index, row) in table.rows.iter().enumerate() {
        let value = row.get(&column.name).unwrap_or(&Value::Null);
        if value.is_null() {
            null_count = null_count
                .checked_add(1)
                .ok_or_else(|| "projection null count overflow".to_string())?;
            null_bitmap[row_index / 8] |= 1u8 << (row_index % 8);
        }
        append_projection_physical_value(column.logical, physical, value, &mut values)?;
    }
    let mut payload = Vec::new();
    if null_count != 0 {
        payload.extend_from_slice(&null_bitmap);
    }
    payload.extend_from_slice(&values);
    Ok((payload, null_count))
}

struct NestedPayloadBuild {
    logical_len: u32,
}

fn projection_nested_payload(
    table: &ProjectedTable,
    column: &ProjectedColumn,
    _column_id: u32,
) -> Result<Vec<u8>, String> {
    let schema = nested_schema_node_for_column(column)?;
    let values = table
        .rows
        .iter()
        .map(|row| row.get(&column.name).unwrap_or(&Value::Null).clone())
        .collect::<Vec<_>>();
    let mut nodes = Vec::new();
    let mut buffers = Vec::new();
    let mut next_node_id = 0u16;
    let root = build_nested_payload_node(
        &schema,
        &values,
        &mut next_node_id,
        &mut nodes,
        &mut buffers,
    )?;
    ColumnPagePayloadV1::build_tree(root.logical_len, 0, nodes, buffers)
        .map_err(|err| err.to_string())
}

fn build_nested_payload_node(
    schema: &NestedSchemaNodeV1,
    values: &[Value],
    next_node_id: &mut u16,
    nodes: &mut Vec<CoveEncodingNodeV1>,
    buffers: &mut Vec<(PageBufferKind, Vec<u8>)>,
) -> Result<NestedPayloadBuild, String> {
    let node_id = *next_node_id;
    *next_node_id = next_node_id
        .checked_add(1)
        .ok_or_else(|| "nested projection node id overflow".to_string())?;
    match schema.physical {
        CovePhysicalKind::List => {
            let child_schema = schema
                .children
                .first()
                .ok_or_else(|| "list nested_shape requires one child".to_string())?;
            let mut offsets = Vec::with_capacity(values.len() + 1);
            offsets.push(0u32);
            let mut child_values = Vec::new();
            for value in values {
                if value.is_null() {
                    offsets.push(child_values.len() as u32);
                    continue;
                }
                let array = value
                    .as_array()
                    .ok_or_else(|| "list projection value must be an array".to_string())?;
                child_values.extend(array.iter().cloned());
                offsets.push(child_values.len() as u32);
            }
            let layout = ListLayoutPayload {
                layout: ListLayout { offsets },
                child_row_count: child_values.len() as u32,
            };
            nodes.push(CoveEncodingNodeV1 {
                node_id,
                encoding_kind: CoveEncodingKind::Canonical,
                logical_type: schema.logical,
                physical_kind: schema.physical,
                flags: 0,
                logical_len: values.len() as u32,
                child_count: 1,
                buffer_count: 1,
                params_offset: 0,
                params_length: 0,
                stats_id: 0,
                reserved: 0,
            });
            buffers.push((PageBufferKind::ChildLayout, layout.encode()));
            build_nested_payload_node(child_schema, &child_values, next_node_id, nodes, buffers)?;
            Ok(NestedPayloadBuild {
                logical_len: values.len() as u32,
            })
        }
        CovePhysicalKind::Struct => {
            let row_count = values.len() as u64;
            let layout = StructLayoutPayload {
                layout: StructLayout {
                    field_row_counts: vec![row_count; schema.children.len()],
                },
                parent_null_handling_declared: true,
            };
            nodes.push(CoveEncodingNodeV1 {
                node_id,
                encoding_kind: CoveEncodingKind::Canonical,
                logical_type: schema.logical,
                physical_kind: schema.physical,
                flags: 0,
                logical_len: values.len() as u32,
                child_count: schema.children.len() as u16,
                buffer_count: 1,
                params_offset: 0,
                params_length: 0,
                stats_id: 0,
                reserved: 0,
            });
            buffers.push((PageBufferKind::ChildLayout, layout.encode()));
            for child in &schema.children {
                let child_values = values
                    .iter()
                    .map(|value| {
                        if value.is_null() {
                            Value::Null
                        } else {
                            value
                                .as_object()
                                .and_then(|object| object.get(&child.name))
                                .cloned()
                                .unwrap_or(Value::Null)
                        }
                    })
                    .collect::<Vec<_>>();
                build_nested_payload_node(child, &child_values, next_node_id, nodes, buffers)?;
            }
            Ok(NestedPayloadBuild {
                logical_len: values.len() as u32,
            })
        }
        CovePhysicalKind::Map => {
            if schema.children.len() != 2 {
                return Err("map nested_shape requires key and value children".into());
            }
            let mut offsets = Vec::with_capacity(values.len() + 1);
            let mut key_values = Vec::new();
            let mut value_values = Vec::new();
            let mut canonical_keys = Vec::new();
            offsets.push(0u32);
            for value in values {
                if value.is_null() {
                    offsets.push(key_values.len() as u32);
                    continue;
                }
                let object = value
                    .as_object()
                    .ok_or_else(|| "map projection value must be a JSON object".to_string())?;
                for (key, value) in object {
                    key_values.push(Value::String(key.clone()));
                    value_values.push(value.clone());
                    canonical_keys.push(key.as_bytes().to_vec());
                }
                offsets.push(key_values.len() as u32);
            }
            let layout = MapLayoutPayload {
                layout: MapLayout {
                    offsets,
                    key_row_count: key_values.len() as u32,
                    value_row_count: value_values.len() as u32,
                    keys_are_scalar: true,
                    allow_duplicate_keys: false,
                    canonical_keys,
                },
            };
            nodes.push(CoveEncodingNodeV1 {
                node_id,
                encoding_kind: CoveEncodingKind::Canonical,
                logical_type: schema.logical,
                physical_kind: schema.physical,
                flags: 0,
                logical_len: values.len() as u32,
                child_count: 2,
                buffer_count: 1,
                params_offset: 0,
                params_length: 0,
                stats_id: 0,
                reserved: 0,
            });
            buffers.push((PageBufferKind::ChildLayout, layout.encode()));
            build_nested_payload_node(
                &schema.children[0],
                &key_values,
                next_node_id,
                nodes,
                buffers,
            )?;
            build_nested_payload_node(
                &schema.children[1],
                &value_values,
                next_node_id,
                nodes,
                buffers,
            )?;
            Ok(NestedPayloadBuild {
                logical_len: values.len() as u32,
            })
        }
        _ => {
            let mut null_bitmap = vec![0u8; (values.len() + 7) / 8];
            let mut null_count = 0usize;
            let mut encoded_values = Vec::new();
            for (index, value) in values.iter().enumerate() {
                if value.is_null() {
                    null_count += 1;
                    null_bitmap[index / 8] |= 1u8 << (index % 8);
                }
                append_projection_physical_value(
                    schema.logical,
                    schema.physical,
                    value,
                    &mut encoded_values,
                )?;
            }
            let mut direct_buffers = Vec::new();
            if null_count != 0 {
                direct_buffers.push((PageBufferKind::NullBitmap, null_bitmap));
            }
            if !encoded_values.is_empty() {
                direct_buffers.push((PageBufferKind::Values, encoded_values));
            }
            nodes.push(CoveEncodingNodeV1 {
                node_id,
                encoding_kind: projection_encoding_kind(schema.physical),
                logical_type: schema.logical,
                physical_kind: schema.physical,
                flags: 0,
                logical_len: values.len() as u32,
                child_count: 0,
                buffer_count: direct_buffers.len() as u16,
                params_offset: 0,
                params_length: 0,
                stats_id: 0,
                reserved: 0,
            });
            buffers.extend(direct_buffers);
            Ok(NestedPayloadBuild {
                logical_len: values.len() as u32,
            })
        }
    }
}

fn nested_schema_node_for_column(column: &ProjectedColumn) -> Result<NestedSchemaNodeV1, String> {
    let shape = column
        .nested_shape
        .as_deref()
        .ok_or_else(|| format!("projection column '{}' requires nested_shape", column.name))?;
    let value: Value = serde_json::from_str(shape).map_err(|err| {
        format!(
            "projection column '{}' has invalid nested_shape JSON: {err}",
            column.name
        )
    })?;
    let mut root = nested_schema_node_from_shape(&column.name, &value, true)?;
    root.name = column.name.clone();
    root.logical = column.logical;
    root.physical = projection_physical_kind(column.logical)?;
    Ok(root)
}

pub(crate) fn nested_schema_node_from_shape(
    fallback_name: &str,
    value: &Value,
    default_nullable: bool,
) -> Result<NestedSchemaNodeV1, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "nested_shape node must be a JSON object".to_string())?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(fallback_name)
        .to_string();
    let nullable = object
        .get("nullable")
        .and_then(Value::as_bool)
        .unwrap_or(default_nullable);
    let kind = object
        .get("type")
        .or_else(|| object.get("kind"))
        .and_then(Value::as_str)
        .unwrap_or("scalar");
    match kind {
        "list" => {
            let item = object
                .get("item")
                .or_else(|| object.get("element"))
                .ok_or_else(|| "list nested_shape requires item".to_string())?;
            Ok(NestedSchemaNodeV1 {
                name,
                logical: CoveLogicalType::List,
                physical: CovePhysicalKind::List,
                nullable,
                precision: 0,
                scale: 0,
                collation_id: 0,
                flags: 0,
                fixed_size_list_len: object
                    .get("fixed_size_list_len")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32,
                children: vec![nested_schema_node_from_shape("item", item, true)?],
            })
        }
        "struct" => {
            let fields = object
                .get("fields")
                .and_then(Value::as_array)
                .ok_or_else(|| "struct nested_shape requires fields array".to_string())?;
            let children = fields
                .iter()
                .map(|field| {
                    let name = field
                        .get("name")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "struct field nested_shape requires name".to_string())?;
                    nested_schema_node_from_shape(name, field, true)
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(NestedSchemaNodeV1 {
                name,
                logical: CoveLogicalType::Struct,
                physical: CovePhysicalKind::Struct,
                nullable,
                precision: 0,
                scale: 0,
                collation_id: 0,
                flags: 0,
                fixed_size_list_len: 0,
                children,
            })
        }
        "map" => {
            let key = object
                .get("key")
                .ok_or_else(|| "map nested_shape requires key".to_string())?;
            let value = object
                .get("value")
                .ok_or_else(|| "map nested_shape requires value".to_string())?;
            Ok(NestedSchemaNodeV1 {
                name,
                logical: CoveLogicalType::Map,
                physical: CovePhysicalKind::Map,
                nullable,
                precision: 0,
                scale: 0,
                collation_id: 0,
                flags: 0,
                fixed_size_list_len: 0,
                children: vec![
                    nested_schema_node_from_shape("key", key, false)?,
                    nested_schema_node_from_shape("value", value, true)?,
                ],
            })
        }
        _ => {
            let logical = object
                .get("logical_type")
                .or_else(|| object.get("logical"))
                .or_else(|| object.get("type"))
                .and_then(Value::as_str)
                .ok_or_else(|| "scalar nested_shape requires logical_type".to_string())
                .and_then(projection_logical_type_from_name)?;
            Ok(NestedSchemaNodeV1::scalar(
                name,
                logical,
                projection_physical_kind(logical)?,
                nullable,
            ))
        }
    }
}

fn append_projection_physical_value(
    logical: CoveLogicalType,
    physical: CovePhysicalKind,
    value: &Value,
    out: &mut Vec<u8>,
) -> Result<(), String> {
    if value.is_null() {
        append_projection_null_placeholder(logical, physical, out)?;
        return Ok(());
    }
    match physical {
        CovePhysicalKind::Boolean => out.push(u8::from(typed_bool_value(value)?.unwrap_or(false))),
        CovePhysicalKind::NumCode => {
            out.extend_from_slice(&projection_numcode(logical, value)?.to_le_bytes())
        }
        CovePhysicalKind::FixedBytes => {
            out.extend_from_slice(&projection_fixed_bytes(logical, value)?)
        }
        CovePhysicalKind::VarBytes => {
            let bytes = typed_bytes_value(value, logical)?.unwrap_or_default();
            let len = u32::try_from(bytes.len())
                .map_err(|_| "projection value exceeds COVE-T VarBytes limit".to_string())?;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&bytes);
        }
        CovePhysicalKind::FileCode
        | CovePhysicalKind::List
        | CovePhysicalKind::Struct
        | CovePhysicalKind::Map => {
            return Err(format!(
                "projection physical kind {physical:?} is not supported for COVE-T output"
            ))
        }
        _ => return Err(format!("unknown projection physical kind {physical:?}")),
    }
    Ok(())
}

fn append_projection_null_placeholder(
    logical: CoveLogicalType,
    physical: CovePhysicalKind,
    out: &mut Vec<u8>,
) -> Result<(), String> {
    match physical {
        CovePhysicalKind::Boolean => out.push(0),
        CovePhysicalKind::NumCode => out.extend_from_slice(&0u64.to_le_bytes()),
        CovePhysicalKind::FixedBytes => {
            let width = match logical {
                CoveLogicalType::Decimal128 | CoveLogicalType::Uuid => 16,
                _ => {
                    return Err(format!(
                        "unsupported fixed-width projection logical type {logical:?}"
                    ))
                }
            };
            out.resize(out.len() + width, 0);
        }
        CovePhysicalKind::VarBytes => out.extend_from_slice(&0u32.to_le_bytes()),
        CovePhysicalKind::FileCode
        | CovePhysicalKind::List
        | CovePhysicalKind::Struct
        | CovePhysicalKind::Map => {
            return Err(format!(
                "projection physical kind {physical:?} is not supported for null placeholders"
            ))
        }
        _ => return Err(format!("unknown projection physical kind {physical:?}")),
    }
    Ok(())
}

fn projection_numcode(logical: CoveLogicalType, value: &Value) -> Result<u64, String> {
    Ok(match logical {
        CoveLogicalType::Int8 => i8::try_from(typed_i128_required(value, logical)?)
            .map_err(|_| "int8 projection value is out of range".to_string())?
            as u8 as u64,
        CoveLogicalType::Int16 => i16::try_from(typed_i128_required(value, logical)?)
            .map_err(|_| "int16 projection value is out of range".to_string())?
            as u16 as u64,
        CoveLogicalType::Int32 | CoveLogicalType::DateDays => {
            i32::try_from(typed_i128_required(value, logical)?)
                .map_err(|_| format!("{logical:?} projection value is out of range"))?
                as u32 as u64
        }
        CoveLogicalType::Int64
        | CoveLogicalType::TimestampMicros
        | CoveLogicalType::TimestampNanos
        | CoveLogicalType::Decimal64 => i64::try_from(typed_i128_required(value, logical)?)
            .map_err(|_| format!("{logical:?} projection value is out of range"))?
            as u64,
        CoveLogicalType::UInt8 => u8::try_from(typed_i128_required(value, logical)?)
            .map_err(|_| "uint8 projection value is out of range".to_string())?
            as u64,
        CoveLogicalType::UInt16 => u16::try_from(typed_i128_required(value, logical)?)
            .map_err(|_| "uint16 projection value is out of range".to_string())?
            as u64,
        CoveLogicalType::UInt32 => u32::try_from(typed_i128_required(value, logical)?)
            .map_err(|_| "uint32 projection value is out of range".to_string())?
            as u64,
        CoveLogicalType::UInt64 => u64::try_from(typed_i128_required(value, logical)?)
            .map_err(|_| "uint64 projection value is out of range".to_string())?,
        CoveLogicalType::Float32 => (typed_f64_value(value)?
            .ok_or_else(|| "float32 projection value is null".to_string())?
            as f32)
            .to_bits() as u64,
        CoveLogicalType::Float64 => typed_f64_value(value)?
            .ok_or_else(|| "float64 projection value is null".to_string())?
            .to_bits(),
        _ => {
            return Err(format!(
                "logical type {logical:?} is not NumCode-backed in projection output"
            ))
        }
    })
}

fn projection_fixed_bytes(logical: CoveLogicalType, value: &Value) -> Result<Vec<u8>, String> {
    match logical {
        CoveLogicalType::Decimal128 => {
            Ok(typed_i128_required(value, logical)?.to_le_bytes().to_vec())
        }
        CoveLogicalType::Uuid => typed_uuid_value(value)?
            .map(|value| value.to_vec())
            .ok_or_else(|| "uuid projection value is null".to_string()),
        _ => Err(format!(
            "logical type {logical:?} is not fixed-width projection output"
        )),
    }
}

fn typed_i128_required(value: &Value, logical: CoveLogicalType) -> Result<i128, String> {
    typed_i128_value(value, logical)?.ok_or_else(|| format!("{logical:?} projection value is null"))
}

fn typed_i128_value(value: &Value, logical: CoveLogicalType) -> Result<Option<i128>, String> {
    if value.is_null() {
        return Ok(None);
    }
    let parsed = match value {
        Value::Number(number) => number
            .as_i64()
            .map(i128::from)
            .or_else(|| number.as_u64().map(|value| value as i128))
            .ok_or_else(|| format!("{logical:?} projection value must be an integer"))?,
        Value::String(text) => text
            .parse::<i128>()
            .map_err(|_| format!("{logical:?} projection value must be an integer"))?,
        Value::Bool(value) if logical == CoveLogicalType::Bool => i128::from(*value),
        _ => {
            return Err(format!(
                "{logical:?} projection value has incompatible JSON type"
            ))
        }
    };
    Ok(Some(parsed))
}

fn typed_f64_value(value: &Value) -> Result<Option<f64>, String> {
    if value.is_null() {
        return Ok(None);
    }
    let parsed = match value {
        Value::Number(number) => number
            .as_f64()
            .ok_or_else(|| "floating projection value is not finite JSON number".to_string())?,
        Value::String(text) => text
            .parse::<f64>()
            .map_err(|_| "floating projection value must be numeric".to_string())?,
        _ => return Err("floating projection value has incompatible JSON type".into()),
    };
    Ok(Some(parsed))
}

fn typed_bool_value(value: &Value) -> Result<Option<bool>, String> {
    if value.is_null() {
        return Ok(None);
    }
    match value {
        Value::Bool(value) => Ok(Some(*value)),
        Value::String(text) if text.eq_ignore_ascii_case("true") => Ok(Some(true)),
        Value::String(text) if text.eq_ignore_ascii_case("false") => Ok(Some(false)),
        _ => Err("bool projection value must be boolean or true/false string".into()),
    }
}

fn typed_string_value(value: &Value, logical: CoveLogicalType) -> Result<Option<String>, String> {
    if value.is_null() {
        return Ok(None);
    }
    match value {
        Value::String(value) => Ok(Some(value.clone())),
        Value::Bool(_) | Value::Number(_) if logical == CoveLogicalType::Utf8 => {
            Ok(json_value_to_output_string(value))
        }
        Value::Array(_) | Value::Object(_) if logical == CoveLogicalType::Utf8 => {
            Ok(Some(value.to_string()))
        }
        _ => Err(format!(
            "{logical:?} projection value cannot be encoded as string"
        )),
    }
}

fn typed_bytes_value(value: &Value, logical: CoveLogicalType) -> Result<Option<Vec<u8>>, String> {
    if value.is_null() {
        return Ok(None);
    }
    match logical {
        CoveLogicalType::Utf8 => Ok(typed_string_value(value, logical)?.map(String::into_bytes)),
        CoveLogicalType::Json => serde_json::to_vec(value)
            .map(Some)
            .map_err(|err| format!("cannot encode projection JSON value: {err}")),
        CoveLogicalType::Binary => match value {
            Value::String(text) => Ok(Some(text.as_bytes().to_vec())),
            Value::Array(values) => values
                .iter()
                .map(|value| {
                    value
                        .as_u64()
                        .and_then(|value| u8::try_from(value).ok())
                        .ok_or_else(|| "binary projection array values must be u8".to_string())
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Some),
            _ => Err("binary projection value must be string or u8 array".into()),
        },
        other => Err(format!("{other:?} projection value is not byte-backed")),
    }
}

fn typed_uuid_value(value: &Value) -> Result<Option<[u8; 16]>, String> {
    if value.is_null() {
        return Ok(None);
    }
    let text = value
        .as_str()
        .ok_or_else(|| "uuid projection value must be a 32-character hex string".to_string())?;
    hex_decode_16(text).map(Some)
}

fn encode_sql_projection(tables: &[ProjectedTable]) -> Result<Vec<u8>, String> {
    let mut out = String::new();
    for table in tables {
        out.push_str(&format!(
            "CREATE TABLE {} (\n",
            quote_sql_identifier(&table.output_table)
        ));
        for (index, column) in table.columns.iter().enumerate() {
            let suffix = if index + 1 == table.columns.len() {
                "\n"
            } else {
                ",\n"
            };
            out.push_str(&format!(
                "  {} {}{}",
                quote_sql_identifier(&column.name),
                sql_type_name(column.logical)?,
                suffix
            ));
        }
        out.push_str(");\n");
        for row in &table.rows {
            let values = table
                .columns
                .iter()
                .map(|column| {
                    let value = row.get(&column.name).unwrap_or(&Value::Null);
                    sql_literal(value, column.logical)
                })
                .collect::<Result<Vec<_>, _>>()?;
            out.push_str(&format!(
                "INSERT INTO {} ({}) VALUES ({});\n",
                quote_sql_identifier(&table.output_table),
                table
                    .columns
                    .iter()
                    .map(|column| quote_sql_identifier(&column.name))
                    .collect::<Vec<_>>()
                    .join(", "),
                values.join(", ")
            ));
        }
    }
    Ok(out.into_bytes())
}

fn quote_sql_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn sql_type_name(logical: CoveLogicalType) -> Result<&'static str, String> {
    match logical {
        CoveLogicalType::Bool => Ok("BOOLEAN"),
        CoveLogicalType::Int8 => Ok("TINYINT"),
        CoveLogicalType::Int16 => Ok("SMALLINT"),
        CoveLogicalType::Int32 | CoveLogicalType::DateDays => Ok("INTEGER"),
        CoveLogicalType::Int64
        | CoveLogicalType::TimestampMicros
        | CoveLogicalType::TimestampNanos => Ok("BIGINT"),
        CoveLogicalType::UInt8 => Ok("UTINYINT"),
        CoveLogicalType::UInt16 => Ok("USMALLINT"),
        CoveLogicalType::UInt32 => Ok("UINTEGER"),
        CoveLogicalType::UInt64 => Ok("UBIGINT"),
        CoveLogicalType::Float32 => Ok("REAL"),
        CoveLogicalType::Float64 => Ok("DOUBLE"),
        CoveLogicalType::Decimal64 => Ok("DECIMAL(18,0)"),
        CoveLogicalType::Decimal128 => Ok("DECIMAL(38,0)"),
        CoveLogicalType::Utf8 | CoveLogicalType::Uuid => Ok("TEXT"),
        CoveLogicalType::Binary => Ok("BLOB"),
        CoveLogicalType::Json => Ok("JSON"),
        CoveLogicalType::Null
        | CoveLogicalType::List
        | CoveLogicalType::Struct
        | CoveLogicalType::Map => Err(format!(
            "projection logical type {logical:?} has no SQL output type"
        )),
        _ => Err(format!("unknown projection logical type {logical:?}")),
    }
}

fn sql_literal(value: &Value, logical: CoveLogicalType) -> Result<String, String> {
    if value.is_null() {
        return Ok("NULL".into());
    }
    match logical {
        CoveLogicalType::Bool => Ok(if typed_bool_value(value)?.unwrap_or(false) {
            "TRUE".into()
        } else {
            "FALSE".into()
        }),
        CoveLogicalType::Int8
        | CoveLogicalType::Int16
        | CoveLogicalType::Int32
        | CoveLogicalType::Int64
        | CoveLogicalType::UInt8
        | CoveLogicalType::UInt16
        | CoveLogicalType::UInt32
        | CoveLogicalType::UInt64
        | CoveLogicalType::Decimal64
        | CoveLogicalType::Decimal128
        | CoveLogicalType::DateDays
        | CoveLogicalType::TimestampMicros
        | CoveLogicalType::TimestampNanos => Ok(typed_i128_required(value, logical)?.to_string()),
        CoveLogicalType::Float32 | CoveLogicalType::Float64 => {
            let value =
                typed_f64_value(value)?.ok_or_else(|| "float SQL value is null".to_string())?;
            if value.is_finite() {
                Ok(value.to_string())
            } else {
                Err("non-finite float projection values cannot be emitted as SQL literals".into())
            }
        }
        CoveLogicalType::Utf8 | CoveLogicalType::Uuid => Ok(quote_sql_string(
            &typed_string_value(value, CoveLogicalType::Utf8)?.unwrap_or_default(),
        )),
        CoveLogicalType::Json => Ok(quote_sql_string(&value.to_string())),
        CoveLogicalType::Binary => {
            let bytes = typed_bytes_value(value, logical)?.unwrap_or_default();
            Ok(format!("X'{}'", hex_encode(&bytes)))
        }
        CoveLogicalType::Null
        | CoveLogicalType::List
        | CoveLogicalType::Struct
        | CoveLogicalType::Map => Err(format!(
            "projection logical type {logical:?} cannot be emitted as SQL"
        )),
        _ => Err(format!("unknown projection logical type {logical:?}")),
    }
}

fn quote_sql_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn json_value_to_output_string(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(value) => Some(value.clone()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(value) => Some(value.to_string()),
        Value::Array(_) | Value::Object(_) => Some(value.to_string()),
    }
}

fn requested_property_names_for_catalog(catalog: &MapProjectionCatalog) -> Vec<String> {
    let mut names = std::collections::BTreeSet::from([
        "source_goid".to_string(),
        "target_goid".to_string(),
        "association_type".to_string(),
        "mapping_rule_id".to_string(),
        "source_evidence_id".to_string(),
        "source_role".to_string(),
        "target_role".to_string(),
        "valid_from".to_string(),
        "valid_to".to_string(),
        "observed_at".to_string(),
        "cardinality_policy".to_string(),
    ]);
    for projection in &catalog.projections {
        for column in &projection.columns {
            collect_property_names_from_expression(&column.value, &mut names);
        }
        for ordering in &projection.ordering {
            collect_property_names_from_expression(ordering_expression(ordering), &mut names);
        }
    }
    names.into_iter().collect()
}

fn collect_property_names_from_expression(
    expression: &str,
    names: &mut std::collections::BTreeSet<String>,
) {
    let expression = expression.trim();
    if expression.is_empty()
        || literal_value(expression).is_some()
        || known_projection_path(expression)
        || expression.starts_with("evidence.")
        || expression == "value"
    {
        return;
    }
    if let Some(traversal) = parse_association_traversal(expression) {
        names.insert(traversal.property_name.to_string());
        return;
    }
    if let Some((_function, args)) = parse_function_call(expression) {
        for arg in args {
            collect_property_names_from_expression(&arg, names);
        }
        return;
    }
    if let Some(property_name) = expression.rsplit('.').next() {
        if !property_name.is_empty() {
            names.insert(property_name.to_string());
        }
    }
}

#[derive(Debug, Clone)]
struct ProjectionModel {
    rows: Vec<ProjectionRow>,
    reconstructed_rows: Vec<ProjectionRow>,
    evidence_entries: Vec<Value>,
    persisted: bool,
}

#[derive(Debug, Clone)]
struct ProjectionRow {
    object_type_id: u32,
    object_type: String,
    object_type_flags: u32,
    goid: [u8; 16],
    record_id: [u8; 16],
    branch_key: u64,
    record_kind: RecordKind,
    timestamp_us: i64,
    csn: u64,
    segment_id: u32,
    row_index: u32,
    properties: Vec<ProjectionProperty>,
}

#[derive(Debug, Clone)]
struct ProjectionProperty {
    property_id: u32,
    property_name: String,
    flags: u32,
    value: Value,
}

impl ProjectionModel {
    fn from_materialized(materialized: &MaterializedModel) -> Self {
        let type_flags = materialized
            .object_types
            .iter()
            .map(|ty| (ty.object_type_id, ty.flags))
            .collect::<BTreeMap<_, _>>();
        let rows: Vec<ProjectionRow> = materialized
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| ProjectionRow {
                object_type_id: row.object_type_id,
                object_type: row.object_type.clone(),
                object_type_flags: type_flags
                    .get(&row.object_type_id)
                    .copied()
                    .unwrap_or_default(),
                goid: row.goid,
                record_id: row.record_id,
                branch_key: 0,
                record_kind: row.record_kind,
                timestamp_us: 0,
                csn: index as u64,
                segment_id: 0,
                row_index: index as u32,
                properties: row
                    .properties
                    .values()
                    .map(|property| ProjectionProperty {
                        property_id: property.entry.property_id,
                        property_name: property.entry.property_name.clone(),
                        flags: property.entry.flags,
                        value: property.value.clone(),
                    })
                    .collect(),
            })
            .collect();
        Self {
            reconstructed_rows: rows.clone(),
            rows,
            evidence_entries: materialized.evidence_entries.clone(),
            persisted: false,
        }
    }

    fn from_surface(surface: &CoveObjectSurface) -> Result<Self, cove_core::CoveError> {
        let rows = surface
            .records
            .iter()
            .map(row_from_surface_record)
            .collect::<Vec<_>>();
        let reconstructed_rows = reconstruct_object_states(surface, &Default::default())?
            .iter()
            .map(row_from_reconstructed_state)
            .collect::<Vec<_>>();
        let evidence_entries = surface
            .evidence_index
            .as_ref()
            .map(|index| index.entries.iter().map(evidence_entry_value).collect())
            .unwrap_or_default();
        Ok(Self {
            rows,
            reconstructed_rows,
            evidence_entries,
            persisted: true,
        })
    }

    fn rows_for_projection(
        &self,
        projection: &MapProjectionEntry,
    ) -> Result<Vec<ProjectionRow>, String> {
        if !self.persisted {
            return Ok(self.rows.clone());
        }
        match parse_projection_temporal_mode(
            projection
                .temporal_mode
                .as_deref()
                .unwrap_or("latest_committed"),
        )
        .ok_or_else(|| {
            format!(
                "projection '{}' uses unsupported temporal_mode '{}'",
                projection.projection_id,
                projection.temporal_mode.as_deref().unwrap_or_default()
            )
        })? {
            ProjectionTemporalMode::LatestCommitted => {
                let mut rows = self.reconstructed_rows.clone();
                rows.sort_by_key(temporal_sort_key);
                Ok(rows)
            }
            ProjectionTemporalMode::FullHistory | ProjectionTemporalMode::CommitOrder => {
                let mut rows = self.rows.clone();
                rows.sort_by_key(temporal_sort_key);
                Ok(rows)
            }
            ProjectionTemporalMode::ValidTime => {
                ensure_temporal_surface_fields(
                    &self.reconstructed_rows,
                    PROPERTY_FLAG_ASSOCIATION_VALID_FROM,
                    "valid_from",
                    "valid_time",
                )?;
                let mut rows = self.reconstructed_rows.clone();
                rows.sort_by_key(temporal_sort_key);
                Ok(rows)
            }
            ProjectionTemporalMode::ObservedTime => {
                ensure_temporal_surface_fields(
                    &self.reconstructed_rows,
                    PROPERTY_FLAG_ASSOCIATION_OBSERVED_AT,
                    "observed_at",
                    "observed_time",
                )?;
                let mut rows = self.reconstructed_rows.clone();
                rows.sort_by_key(temporal_sort_key);
                Ok(rows)
            }
            ProjectionTemporalMode::AsOfTimestamp(timestamp_us) => {
                Ok(reconstruct_projection_rows_at_cut(&self.rows, |row| {
                    row.timestamp_us <= timestamp_us
                }))
            }
            ProjectionTemporalMode::AsOfCsn(csn) => {
                Ok(reconstruct_projection_rows_at_cut(&self.rows, |row| {
                    row.csn <= csn
                }))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectionTemporalMode {
    LatestCommitted,
    FullHistory,
    CommitOrder,
    ValidTime,
    ObservedTime,
    AsOfTimestamp(i64),
    AsOfCsn(u64),
}

fn parse_projection_temporal_mode(value: &str) -> Option<ProjectionTemporalMode> {
    match value {
        "latest_committed" => Some(ProjectionTemporalMode::LatestCommitted),
        "full_history" => Some(ProjectionTemporalMode::FullHistory),
        "commit_order" => Some(ProjectionTemporalMode::CommitOrder),
        "valid_time" => Some(ProjectionTemporalMode::ValidTime),
        "observed_time" => Some(ProjectionTemporalMode::ObservedTime),
        _ => parse_temporal_cut_value(value),
    }
}

fn parse_temporal_cut_value(value: &str) -> Option<ProjectionTemporalMode> {
    for prefix in [
        "as_of_timestamp_us:",
        "as_of_timestamp_us=",
        "timestamp_us:",
        "timestamp_us=",
        "as_of_time:",
        "as_of_time=",
    ] {
        if let Some(raw) = value.strip_prefix(prefix) {
            return raw.parse().ok().map(ProjectionTemporalMode::AsOfTimestamp);
        }
    }
    for prefix in ["as_of_csn:", "as_of_csn=", "csn:", "csn="] {
        if let Some(raw) = value.strip_prefix(prefix) {
            return raw.parse().ok().map(ProjectionTemporalMode::AsOfCsn);
        }
    }
    None
}

fn ensure_temporal_surface_fields(
    rows: &[ProjectionRow],
    flag: u32,
    name: &str,
    mode: &str,
) -> Result<(), String> {
    if rows.is_empty() {
        return Ok(());
    }
    if rows.iter().any(|row| {
        row.properties
            .iter()
            .any(|property| property.flags & flag != 0 || property.property_name == name)
    }) {
        return Ok(());
    }
    Err(format!(
        "temporal_mode '{mode}' requires declared '{name}' fields on the projected surface"
    ))
}

fn reconstruct_projection_rows_at_cut(
    rows: &[ProjectionRow],
    visible: impl Fn(&ProjectionRow) -> bool,
) -> Vec<ProjectionRow> {
    let mut grouped = BTreeMap::<(u32, u64, [u8; 16]), Vec<ProjectionRow>>::new();
    for row in rows.iter().filter(|row| visible(row)) {
        grouped
            .entry((row.object_type_id, row.branch_key, row.goid))
            .or_default()
            .push(row.clone());
    }
    let mut out = Vec::new();
    for (_key, mut group) in grouped {
        group.sort_by_key(temporal_sort_key);
        let mut current = None::<ProjectionRow>;
        for row in group {
            match row.record_kind {
                RecordKind::Baseline | RecordKind::Snapshot => current = Some(row),
                RecordKind::Delta => match current.as_mut() {
                    Some(state) => apply_projection_delta(state, &row),
                    None => current = Some(row),
                },
                RecordKind::Tombstone => current = None,
                RecordKind::ReservedLegacyMaterializedDelta => {}
                _ => {}
            }
        }
        if let Some(row) = current {
            out.push(row);
        }
    }
    out.sort_by_key(temporal_sort_key);
    out
}

fn apply_projection_delta(state: &mut ProjectionRow, delta: &ProjectionRow) {
    state.record_id = delta.record_id;
    state.record_kind = delta.record_kind;
    state.timestamp_us = delta.timestamp_us;
    state.csn = delta.csn;
    state.segment_id = delta.segment_id;
    state.row_index = delta.row_index;
    for property in &delta.properties {
        match state
            .properties
            .iter_mut()
            .find(|existing| existing.property_id == property.property_id)
        {
            Some(existing) => *existing = property.clone(),
            None => state.properties.push(property.clone()),
        }
    }
}

fn row_from_surface_record(record: &CoveObjectRecord) -> ProjectionRow {
    ProjectionRow {
        object_type_id: record.object_type_id,
        object_type: record.object_type_name.clone(),
        object_type_flags: record.object_type_flags,
        goid: record.goid,
        record_id: record.record_id,
        branch_key: record.branch_key,
        record_kind: record.record_kind,
        timestamp_us: record.timestamp_us,
        csn: record.csn,
        segment_id: record.segment_id,
        row_index: record.row_index,
        properties: record
            .properties
            .iter()
            .map(|property| ProjectionProperty {
                property_id: property.property_id,
                property_name: property.property_name.clone(),
                flags: property.flags,
                value: property.value.clone(),
            })
            .collect(),
    }
}

fn row_from_reconstructed_state(state: &CoveObjectState) -> ProjectionRow {
    ProjectionRow {
        object_type_id: state.object_type_id,
        object_type: state.object_type_name.clone(),
        object_type_flags: state.object_type_flags,
        goid: state.goid,
        record_id: state.latest_record_id,
        branch_key: state.branch_key,
        record_kind: state.record_kind,
        timestamp_us: state.timestamp_us,
        csn: state.csn,
        segment_id: state.latest_segment_id,
        row_index: state.latest_row_index,
        properties: state
            .properties
            .iter()
            .map(|property| ProjectionProperty {
                property_id: property.property_id,
                property_name: property.property_name.clone(),
                flags: property.flags,
                value: property.value.clone(),
            })
            .collect(),
    }
}

fn evidence_entry_value(entry: &MapEvidenceEntry) -> Value {
    let mut value = json!({
        "source_id": entry.source_id,
        "source_row_identity": entry.source_row_identity,
        "rule_id": entry.rule_id,
        "assertion_id": entry.assertion_id,
        "output_object_id": entry.output_object_id,
        "observed_schema_fingerprint": entry.observed_schema_fingerprint,
        "observed_snapshot_digest": entry.observed_snapshot_digest,
    });
    if let Some(object) = value.as_object_mut() {
        for (key, metadata_value) in &entry.operation_metadata {
            object.insert(key.clone(), metadata_value.clone());
        }
    }
    value
}

fn temporal_sort_key(row: &ProjectionRow) -> (i64, u64, u32, u32, [u8; 16]) {
    (
        row.timestamp_us,
        row.csn,
        row.segment_id,
        row.row_index,
        row.record_id,
    )
}

fn validate_executable_projection(
    projection: &MapProjectionEntry,
    model: &ProjectionModel,
    function_ids: &std::collections::BTreeSet<String>,
) -> Result<(), String> {
    if projection.output_table.is_none()
        || projection.row_grain.is_none()
        || projection.anchor.is_none()
        || projection.temporal_mode.is_none()
        || projection.multi_value_policy.is_none()
        || projection.columns.is_empty()
        || projection.output_modes.is_empty()
    {
        return Err(format!(
            "projection '{}' uses the legacy preview schema; add output_table, row_grain, anchor, temporal_mode, multi_value_policy, columns, and output_modes",
            projection.projection_id
        ));
    }
    let temporal_mode = projection.temporal_mode.as_deref().unwrap_or_default();
    if parse_projection_temporal_mode(temporal_mode).is_none() {
        return Err(format!(
            "projection '{}' uses unsupported temporal_mode '{temporal_mode}'",
            projection.projection_id
        ));
    }
    let policy = projection.multi_value_policy.as_deref().unwrap_or_default();
    let row_grain = projection.row_grain.as_deref().unwrap_or_default();
    match policy {
        "first" | "last" if projection.ordering.is_empty() => {
            return Err(format!(
                "projection '{}' multi_value_policy '{policy}' requires explicit ordering",
                projection.projection_id
            ));
        }
        "reject" | "explode" | "aggregate" | "first" | "last" | "list" => {}
        _ => {
            return Err(format!(
                "projection '{}' uses unsupported multi_value_policy '{policy}' for row_grain '{row_grain}'",
                projection.projection_id
            ));
        }
    }
    for column in &projection.columns {
        validate_projection_expression(model, projection, function_ids, &column.value)?;
    }
    for ordering in &projection.ordering {
        let expression = ordering_expression(ordering);
        if expression != "value" {
            validate_projection_expression(model, projection, function_ids, expression)?;
        }
    }
    Ok(())
}

fn validate_projection_expression(
    model: &ProjectionModel,
    projection: &MapProjectionEntry,
    function_ids: &std::collections::BTreeSet<String>,
    expression: &str,
) -> Result<(), String> {
    let expression = expression.trim();
    if expression.is_empty()
        || literal_value(expression).is_some()
        || known_projection_path(expression)
    {
        return Ok(());
    }
    if expression.starts_with("evidence.") {
        return Ok(());
    }
    if let Some(traversal) = parse_association_traversal(expression) {
        let association_type = traversal.association_type;
        let property_name = traversal.property_name;
        if association_type.is_empty() || property_name.is_empty() {
            return Err(format!("invalid association traversal '{expression}'"));
        }
        return Ok(());
    }
    if let Some((function, args)) = parse_function_call(expression) {
        if !projection_builtin_operator(function)
            && !function_ids.is_empty()
            && !function_ids.contains(function)
        {
            return Err(format!("undeclared projection function '{function}'"));
        }
        if !runtime_projection_function(function) {
            if function_ids.contains(function) {
                return Err(format!(
                    "projection function '{function}' is declared but has no reference executor"
                ));
            }
            return Err(format!("undeclared projection function '{function}'"));
        }
        if matches!(
            function,
            "if" | "ifelse"
                | "count"
                | "min"
                | "max"
                | "sum"
                | "avg"
                | "distinct_count"
                | "list"
                | "identity"
                | "trim"
                | "lower"
                | "lowercase"
                | "upper"
                | "uppercase"
                | "exists"
                | "coalesce"
                | "association"
        ) {
            if function == "association" {
                if args.len() != 1 || args[0].trim().is_empty() {
                    return Err("projection function 'association' expects one argument".into());
                }
                return Ok(());
            }
            for arg in args {
                if !condition_like_expression(&arg) {
                    validate_projection_expression(model, projection, function_ids, &arg)?;
                }
            }
            return Ok(());
        }
    }
    validate_projection_path(model, projection, expression)
}

fn runtime_projection_function(function: &str) -> bool {
    matches!(
        function,
        "if" | "ifelse"
            | "count"
            | "min"
            | "max"
            | "sum"
            | "avg"
            | "distinct_count"
            | "list"
            | "identity"
            | "trim"
            | "lower"
            | "lowercase"
            | "upper"
            | "uppercase"
            | "exists"
            | "coalesce"
            | "association"
    )
}

fn projection_builtin_operator(function: &str) -> bool {
    matches!(
        function,
        "if" | "ifelse"
            | "count"
            | "min"
            | "max"
            | "sum"
            | "avg"
            | "distinct_count"
            | "list"
            | "exists"
            | "association"
    )
}

fn condition_like_expression(expression: &str) -> bool {
    ["==", "!=", ">=", "<=", ">", "<"]
        .iter()
        .any(|op| expression.contains(op))
}

fn known_projection_path(expression: &str) -> bool {
    matches!(
        expression,
        "goid"
            | "object.goid"
            | "Object.goid"
            | "association.goid"
            | "record.id"
            | "record.record_id"
            | "record.kind"
            | "object.type_id"
            | "object_type_id"
            | "temporal.timestamp_us"
            | "timestamp_us"
            | "temporal.csn"
            | "csn"
            | "temporal.branch_key"
            | "branch_key"
            | "object_type"
            | "object.type"
            | "Object.type"
            | "association.source_goid"
            | "association.target_goid"
            | "association.association_type"
            | "association.mapping_rule_id"
            | "association.source_evidence_id"
            | "association.source_role"
            | "association.target_role"
            | "association.valid_from"
            | "association.valid_to"
            | "association.observed_at"
            | "association.cardinality_policy"
    )
}

fn validate_projection_path(
    model: &ProjectionModel,
    projection: &MapProjectionEntry,
    expression: &str,
) -> Result<(), String> {
    let property_name = expression
        .rsplit('.')
        .next()
        .ok_or_else(|| format!("unsupported projection expression '{expression}'"))?;
    if property_name.is_empty() {
        return Err(format!("unsupported projection expression '{expression}'"));
    }
    let Some(anchor) = &projection.anchor else {
        return Ok(());
    };
    let matching = model
        .rows
        .iter()
        .chain(model.reconstructed_rows.iter())
        .filter(|row| {
            anchor
                .object_type
                .as_ref()
                .map(|object_type| &row.object_type == object_type)
                .unwrap_or_else(|| {
                    anchor
                        .association_type
                        .as_ref()
                        .map(|association_type| row_matches_association(row, association_type))
                        .unwrap_or(true)
                })
        })
        .collect::<Vec<_>>();
    if matching.is_empty()
        || matching.iter().any(|row| {
            row.properties
                .iter()
                .any(|property| property.property_name == property_name)
        })
    {
        Ok(())
    } else {
        Err(format!(
            "projection '{}' references undeclared path '{expression}'",
            projection.projection_id
        ))
    }
}

fn ordering_expression(ordering: &str) -> &str {
    let value = ordering.trim();
    let value = value.strip_prefix('-').unwrap_or(value).trim();
    for suffix in [" desc", " asc", ":desc", ":asc"] {
        if let Some(stripped) = value.strip_suffix(suffix) {
            return stripped.trim();
        }
    }
    value
}

fn ordering_descending(ordering: &str) -> bool {
    let value = ordering.trim();
    value.starts_with('-') || value.ends_with(" desc") || value.ends_with(":desc")
}

fn sort_projection_rows_by_ordering(
    rows: &mut [ProjectionRow],
    projection: &MapProjectionEntry,
    anchor_row: Option<&ProjectionRow>,
) -> Result<(), String> {
    for ordering in projection.ordering.iter().rev() {
        let expression = ordering_expression(ordering);
        if expression == "value" {
            continue;
        }
        let descending = ordering_descending(ordering);
        rows.sort_by(|left, right| {
            let left_value = ordering_value(left, expression, anchor_row);
            let right_value = ordering_value(right, expression, anchor_row);
            let ordering = compare_json_order(&left_value, &right_value);
            if descending {
                ordering.reverse()
            } else {
                ordering
            }
        });
    }
    Ok(())
}

fn ordering_value(
    row: &ProjectionRow,
    expression: &str,
    _anchor_row: Option<&ProjectionRow>,
) -> Value {
    match expression {
        "temporal.timestamp_us" | "timestamp_us" => json!(row.timestamp_us),
        "temporal.csn" | "csn" => json!(row.csn),
        "temporal.branch_key" | "branch_key" => json!(row.branch_key),
        "record.id" | "record.record_id" => json!(hex_encode(&row.record_id)),
        "record.kind" => json!(record_kind_name(row.record_kind)),
        "segment_id" => json!(row.segment_id),
        "row_index" => json!(row.row_index),
        "goid" | "object.goid" | "association.goid" => json!(hex_encode(&row.goid)),
        other => {
            let property_name = other.rsplit('.').next().unwrap_or(other);
            projection_property_by_name(row, property_name)
        }
    }
}

fn project_one(
    model: &ProjectionModel,
    projection: &MapProjectionEntry,
) -> Result<Vec<Value>, String> {
    let row_grain = projection
        .row_grain
        .as_deref()
        .ok_or_else(|| "projection row_grain is required".to_string())?;
    match row_grain {
        "one_row_per_object" => project_object_rows(model, projection, false),
        "one_row_per_event_object" | "one_row_per_object_as_of_time" => {
            project_object_rows(model, projection, false)
        }
        "one_row_per_association" | "one_row_per_link_object" => {
            project_object_rows(model, projection, true)
        }
        "one_row_per_property_version" => project_property_versions(model, projection),
        "one_row_per_evidence_assertion" => project_evidence_rows(model, projection),
        other => Err(format!("unsupported projection row_grain '{other}'")),
    }
}

fn project_object_rows(
    model: &ProjectionModel,
    projection: &MapProjectionEntry,
    associations: bool,
) -> Result<Vec<Value>, String> {
    let anchor = projection
        .anchor
        .as_ref()
        .ok_or_else(|| "projection anchor is required".to_string())?;
    let mut rows = Vec::new();
    for row in model.rows_for_projection(projection)? {
        if associations {
            let Some(association_type) = &anchor.association_type else {
                continue;
            };
            if !row_matches_association(&row, association_type) {
                continue;
            }
        } else {
            let Some(object_type) = &anchor.object_type else {
                continue;
            };
            if &row.object_type != object_type {
                continue;
            }
        }
        let mut out = Map::new();
        out.insert("projection_id".into(), json!(projection.projection_id));
        if let Some(output_table) = &projection.output_table {
            out.insert("output_table".into(), json!(output_table));
        }
        rows.extend(project_columns_for_row(model, projection, &row, out)?);
    }
    Ok(rows)
}

fn project_columns_for_row(
    model: &ProjectionModel,
    projection: &MapProjectionEntry,
    row: &ProjectionRow,
    base: Map<String, Value>,
) -> Result<Vec<Value>, String> {
    let mut rows = vec![base];
    for column in &projection.columns {
        let value = projection_value(model, projection, row, &column.value)?;
        rows = apply_multi_value_policy(rows, &column.name, value, projection)?;
    }
    Ok(rows.into_iter().map(Value::Object).collect())
}

fn apply_multi_value_policy(
    rows: Vec<Map<String, Value>>,
    column_name: &str,
    value: Value,
    projection: &MapProjectionEntry,
) -> Result<Vec<Map<String, Value>>, String> {
    let policy = projection.multi_value_policy.as_deref().unwrap_or("reject");
    let Some(values) = value.as_array() else {
        return Ok(rows
            .into_iter()
            .map(|mut row| {
                row.insert(column_name.to_string(), value.clone());
                row
            })
            .collect());
    };
    let values = ordered_multi_values(values, projection);
    match policy {
        "explode" => {
            if values.is_empty() {
                return Ok(rows
                    .into_iter()
                    .map(|mut row| {
                        row.insert(column_name.to_string(), Value::Null);
                        row
                    })
                    .collect());
            }
            let mut out = Vec::with_capacity(rows.len() * values.len());
            for row in rows {
                for value in &values {
                    let mut row = row.clone();
                    row.insert(column_name.to_string(), value.clone());
                    out.push(row);
                }
            }
            Ok(out)
        }
        "list" | "aggregate" => Ok(rows
            .into_iter()
            .map(|mut row| {
                row.insert(column_name.to_string(), Value::Array(values.clone()));
                row
            })
            .collect()),
        "first" => {
            let selected = values.first().cloned().unwrap_or(Value::Null);
            Ok(rows
                .into_iter()
                .map(|mut row| {
                    row.insert(column_name.to_string(), selected.clone());
                    row
                })
                .collect())
        }
        "last" => {
            let selected = values.last().cloned().unwrap_or(Value::Null);
            Ok(rows
                .into_iter()
                .map(|mut row| {
                    row.insert(column_name.to_string(), selected.clone());
                    row
                })
                .collect())
        }
        "reject" if values.len() <= 1 => {
            let selected = values.first().cloned().unwrap_or(Value::Null);
            Ok(rows
                .into_iter()
                .map(|mut row| {
                    row.insert(column_name.to_string(), selected.clone());
                    row
                })
                .collect())
        }
        "reject" => Err(format!(
            "projection '{}' column '{column_name}' produced {} values with multi_value_policy='reject'",
            projection.projection_id,
            values.len()
        )),
        other => Err(format!(
            "projection '{}' uses unsupported multi_value_policy '{other}'",
            projection.projection_id
        )),
    }
}

fn ordered_multi_values(values: &[Value], projection: &MapProjectionEntry) -> Vec<Value> {
    let mut out = values.to_vec();
    for ordering in projection.ordering.iter().rev() {
        if ordering_expression(ordering) != "value" {
            continue;
        }
        let descending = ordering_descending(ordering);
        out.sort_by(|left, right| {
            let ordering = compare_json_order(left, right);
            if descending {
                ordering.reverse()
            } else {
                ordering
            }
        });
    }
    out
}

fn project_property_versions(
    model: &ProjectionModel,
    projection: &MapProjectionEntry,
) -> Result<Vec<Value>, String> {
    let mut rows = Vec::new();
    for row in model.rows_for_projection(projection)? {
        for property in &row.properties {
            let mut out = Map::new();
            out.insert("projection_id".into(), json!(projection.projection_id));
            out.insert("object_goid".into(), json!(hex_encode(&row.goid)));
            out.insert("property_id".into(), json!(property.property_id));
            out.insert("property_name".into(), json!(property.property_name));
            out.insert("value".into(), property.value.clone());
            rows.push(Value::Object(out));
        }
    }
    Ok(rows)
}

fn project_evidence_rows(
    model: &ProjectionModel,
    projection: &MapProjectionEntry,
) -> Result<Vec<Value>, String> {
    let mut rows = Vec::new();
    for evidence in &model.evidence_entries {
        let mut out = Map::new();
        out.insert("projection_id".into(), json!(projection.projection_id));
        for column in &projection.columns {
            let key = column
                .value
                .strip_prefix("evidence.")
                .ok_or_else(|| format!("unsupported evidence expression '{}'", column.value))?;
            out.insert(
                column.name.clone(),
                evidence.get(key).cloned().unwrap_or(Value::Null),
            );
        }
        rows.push(Value::Object(out));
    }
    Ok(rows)
}

fn projection_value(
    model: &ProjectionModel,
    projection: &MapProjectionEntry,
    row: &ProjectionRow,
    expression: &str,
) -> Result<Value, String> {
    match expression {
        "goid" | "object.goid" | "Object.goid" | "association.goid" => {
            return Ok(json!(hex_encode(&row.goid)));
        }
        "record.id" | "record.record_id" => return Ok(json!(hex_encode(&row.record_id))),
        "record.kind" => return Ok(json!(record_kind_name(row.record_kind))),
        "object.type_id" | "object_type_id" => return Ok(json!(row.object_type_id)),
        "temporal.timestamp_us" | "timestamp_us" => return Ok(json!(row.timestamp_us)),
        "temporal.csn" | "csn" => return Ok(json!(row.csn)),
        "temporal.branch_key" | "branch_key" => return Ok(json!(row.branch_key)),
        "object_type" | "object.type" | "Object.type" => return Ok(json!(row.object_type)),
        "association.source_goid" => {
            return Ok(projection_property_by_flag_or_name(
                row,
                PROPERTY_FLAG_ASSOCIATION_FROM_GOID,
                "source_goid",
            ))
        }
        "association.target_goid" => {
            return Ok(projection_property_by_flag_or_name(
                row,
                PROPERTY_FLAG_ASSOCIATION_TO_GOID,
                "target_goid",
            ))
        }
        "association.association_type" => {
            return Ok(projection_property_by_flag_or_name(
                row,
                PROPERTY_FLAG_ASSOCIATION_TYPE,
                "association_type",
            ))
        }
        "association.mapping_rule_id" => {
            return Ok(projection_property_by_flag_or_name(
                row,
                PROPERTY_FLAG_MAPPING_RULE_REF,
                "mapping_rule_id",
            ))
        }
        "association.source_evidence_id" => {
            return Ok(projection_property_by_flag_or_name(
                row,
                PROPERTY_FLAG_EVIDENCE_REF,
                "source_evidence_id",
            ))
        }
        "association.source_role" => return Ok(projection_property_by_name(row, "source_role")),
        "association.target_role" => return Ok(projection_property_by_name(row, "target_role")),
        "association.valid_from" => {
            return Ok(projection_property_by_flag_or_name(
                row,
                PROPERTY_FLAG_ASSOCIATION_VALID_FROM,
                "valid_from",
            ))
        }
        "association.valid_to" => {
            return Ok(projection_property_by_flag_or_name(
                row,
                PROPERTY_FLAG_ASSOCIATION_VALID_TO,
                "valid_to",
            ))
        }
        "association.observed_at" => {
            return Ok(projection_property_by_flag_or_name(
                row,
                PROPERTY_FLAG_ASSOCIATION_OBSERVED_AT,
                "observed_at",
            ))
        }
        "association.cardinality_policy" => {
            return Ok(projection_property_by_name(row, "cardinality_policy"))
        }
        _ => {}
    }
    if let Some(literal) = literal_value(expression) {
        return Ok(literal);
    }
    if let Some(value) = conditional_expression(model, projection, row, expression)? {
        return Ok(value);
    }
    if let Some(inner) = expression
        .strip_prefix("count(association(")
        .and_then(|rest| rest.strip_suffix("))"))
    {
        let (association_type, endpoint_role) = parse_association_call_args(inner);
        let count = associated_rows(model, projection, row, association_type, endpoint_role)?.len();
        return Ok(json!(count));
    }
    if let Some((function, args)) = parse_function_call(expression) {
        return projection_function_value(model, projection, row, function, &args);
    }
    if let Some(traversal) = parse_association_traversal(expression) {
        let values = associated_rows(
            model,
            projection,
            row,
            traversal.association_type,
            traversal.endpoint_role,
        )?
        .into_iter()
        .map(|candidate| association_projection_value(&candidate, traversal.property_name))
        .collect::<Vec<_>>();
        return Ok(Value::Array(values));
    }
    let property_name = expression
        .rsplit('.')
        .next()
        .ok_or_else(|| format!("unsupported projection expression '{expression}'"))?;
    Ok(projection_property_by_name(row, property_name))
}

fn projection_function_value(
    model: &ProjectionModel,
    projection: &MapProjectionEntry,
    row: &ProjectionRow,
    function: &str,
    args: &[String],
) -> Result<Value, String> {
    match function {
        "identity" => unary_arg(model, projection, row, function, args),
        "trim" => string_unary(model, projection, row, function, args, |value| {
            value.trim().to_string()
        }),
        "lower" | "lowercase" => string_unary(model, projection, row, function, args, |value| {
            value.to_ascii_lowercase()
        }),
        "upper" | "uppercase" => string_unary(model, projection, row, function, args, |value| {
            value.to_ascii_uppercase()
        }),
        "exists" => {
            let value = unary_arg(model, projection, row, function, args)?;
            Ok(json!(
                !value.is_null() && !matches!(&value, Value::Array(values) if values.is_empty())
            ))
        }
        "coalesce" => {
            for arg in args {
                let value = projection_value(model, projection, row, arg)?;
                if !value.is_null() {
                    return Ok(value);
                }
            }
            Ok(Value::Null)
        }
        "association" => {
            if args.len() != 1 {
                return Err("projection function 'association' expects one argument".into());
            }
            let (association_type, endpoint_role) = parse_association_call_args(&args[0]);
            Ok(Value::Array(
                associated_rows(model, projection, row, association_type, endpoint_role)?
                    .into_iter()
                    .map(|candidate| json!(hex_encode(&candidate.goid)))
                    .collect(),
            ))
        }
        "count" | "min" | "max" | "sum" | "avg" | "distinct_count" | "list" => {
            aggregate_function_value(model, projection, row, function, args)
        }
        other => Err(format!("unsupported projection function '{other}'")),
    }
}

fn unary_arg(
    model: &ProjectionModel,
    projection: &MapProjectionEntry,
    row: &ProjectionRow,
    function: &str,
    args: &[String],
) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(format!(
            "projection function '{function}' expects one argument"
        ));
    }
    projection_value(model, projection, row, &args[0])
}

fn string_unary(
    model: &ProjectionModel,
    projection: &MapProjectionEntry,
    row: &ProjectionRow,
    function: &str,
    args: &[String],
    op: impl FnOnce(&str) -> String,
) -> Result<Value, String> {
    let value = unary_arg(model, projection, row, function, args)?;
    Ok(value
        .as_str()
        .map(|text| json!(op(text)))
        .unwrap_or(Value::Null))
}

fn aggregate_function_value(
    model: &ProjectionModel,
    projection: &MapProjectionEntry,
    row: &ProjectionRow,
    function: &str,
    args: &[String],
) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(format!(
            "projection aggregate '{function}' expects one argument"
        ));
    }
    let values = if let Some(traversal) = parse_association_traversal(&args[0]) {
        associated_rows(
            model,
            projection,
            row,
            traversal.association_type,
            traversal.endpoint_role,
        )?
        .into_iter()
        .map(|candidate| association_projection_value(&candidate, traversal.property_name))
        .collect::<Vec<_>>()
    } else if let Some((association_function, association_args)) = parse_function_call(&args[0]) {
        if association_function == "association" && association_args.len() == 1 {
            let (association_type, endpoint_role) =
                parse_association_call_args(&association_args[0]);
            associated_rows(model, projection, row, association_type, endpoint_role)?
                .into_iter()
                .map(|candidate| json!(hex_encode(&candidate.goid)))
                .collect::<Vec<_>>()
        } else {
            vec![projection_value(model, projection, row, &args[0])?]
        }
    } else {
        vec![projection_value(model, projection, row, &args[0])?]
    };
    match function {
        "count" => Ok(json!(values
            .iter()
            .filter(|value| !value.is_null())
            .count())),
        "list" => Ok(Value::Array(values)),
        "distinct_count" => {
            let set = values
                .into_iter()
                .filter(|value| !value.is_null())
                .map(|value| value.to_string())
                .collect::<std::collections::BTreeSet<_>>();
            Ok(json!(set.len()))
        }
        "min" => Ok(min_max_json(values, true)),
        "max" => Ok(min_max_json(values, false)),
        "sum" => Ok(json!(values
            .into_iter()
            .filter_map(json_number_f64)
            .sum::<f64>())),
        "avg" => {
            let numbers = values
                .into_iter()
                .filter_map(json_number_f64)
                .collect::<Vec<_>>();
            if numbers.is_empty() {
                Ok(Value::Null)
            } else {
                Ok(json!(numbers.iter().sum::<f64>() / numbers.len() as f64))
            }
        }
        other => Err(format!("unsupported projection aggregate '{other}'")),
    }
}

fn conditional_expression(
    model: &ProjectionModel,
    projection: &MapProjectionEntry,
    row: &ProjectionRow,
    expression: &str,
) -> Result<Option<Value>, String> {
    let Some((function, args)) = parse_function_call(expression) else {
        return Ok(None);
    };
    if !matches!(function, "if" | "ifelse") {
        return Ok(None);
    }
    if args.len() != 3 {
        return Err(format!(
            "projection conditional '{function}' expects three arguments"
        ));
    }
    let condition = projection_condition(model, projection, row, &args[0])?;
    Ok(Some(projection_value(
        model,
        projection,
        row,
        if condition { &args[1] } else { &args[2] },
    )?))
}

fn projection_condition(
    model: &ProjectionModel,
    projection: &MapProjectionEntry,
    row: &ProjectionRow,
    expression: &str,
) -> Result<bool, String> {
    for op in ["==", "!=", ">=", "<=", ">", "<"] {
        if let Some((left, right)) = expression.split_once(op) {
            let left = projection_value(model, projection, row, left.trim())?;
            let right = projection_value(model, projection, row, right.trim())?;
            return Ok(compare_json_values(&left, &right, op));
        }
    }
    Ok(json_truthy(&projection_value(
        model, projection, row, expression,
    )?))
}

fn associated_rows(
    model: &ProjectionModel,
    projection: &MapProjectionEntry,
    row: &ProjectionRow,
    association_type: &str,
    endpoint_role: Option<&str>,
) -> Result<Vec<ProjectionRow>, String> {
    let mut rows = model
        .rows_for_projection_for_aggregate()?
        .into_iter()
        .filter(|candidate| row_matches_association(candidate, association_type))
        .filter(|candidate| association_endpoint_matches(candidate, row, endpoint_role))
        .collect::<Vec<_>>();
    sort_projection_rows_by_ordering(&mut rows, projection, Some(row))?;
    Ok(rows)
}

fn association_endpoint_matches(
    candidate: &ProjectionRow,
    anchor: &ProjectionRow,
    endpoint_role: Option<&str>,
) -> bool {
    let anchor_goid = json!(hex_encode(&anchor.goid));
    let source_goid = projection_property_by_flag_or_name(
        candidate,
        PROPERTY_FLAG_ASSOCIATION_FROM_GOID,
        "source_goid",
    );
    let target_goid = projection_property_by_flag_or_name(
        candidate,
        PROPERTY_FLAG_ASSOCIATION_TO_GOID,
        "target_goid",
    );
    let Some(role) = endpoint_role.map(str::trim).filter(|role| !role.is_empty()) else {
        return source_goid == anchor_goid;
    };
    match role {
        "source" | "from" => source_goid == anchor_goid,
        "target" | "to" => target_goid == anchor_goid,
        other => {
            (source_goid == anchor_goid
                && projection_property_by_name(candidate, "source_role").as_str() == Some(other))
                || (target_goid == anchor_goid
                    && projection_property_by_name(candidate, "target_role").as_str()
                        == Some(other))
        }
    }
}

fn association_projection_value(row: &ProjectionRow, property_name: &str) -> Value {
    match property_name {
        "goid" => json!(hex_encode(&row.goid)),
        "source_goid" => projection_property_by_flag_or_name(
            row,
            PROPERTY_FLAG_ASSOCIATION_FROM_GOID,
            "source_goid",
        ),
        "target_goid" => projection_property_by_flag_or_name(
            row,
            PROPERTY_FLAG_ASSOCIATION_TO_GOID,
            "target_goid",
        ),
        "association_type" => projection_property_by_flag_or_name(
            row,
            PROPERTY_FLAG_ASSOCIATION_TYPE,
            "association_type",
        ),
        other => projection_property_by_name(row, other),
    }
}

#[derive(Debug, Clone, Copy)]
struct AssociationTraversal<'a> {
    association_type: &'a str,
    endpoint_role: Option<&'a str>,
    property_name: &'a str,
}

fn parse_association_traversal(expression: &str) -> Option<AssociationTraversal<'_>> {
    let expression = expression.trim();
    let rest = expression.strip_prefix("association(")?;
    let (association_type, rest) = rest.split_once(").")?;
    let (association_type, endpoint_role) = match association_type.split_once(',') {
        Some((association_type, endpoint_role)) => {
            (association_type.trim(), Some(endpoint_role.trim()))
        }
        None => (association_type.trim(), None),
    };
    (!association_type.is_empty() && !rest.trim().is_empty()).then_some(AssociationTraversal {
        association_type,
        endpoint_role: endpoint_role.filter(|role| !role.is_empty()),
        property_name: rest.trim(),
    })
}

fn parse_association_call_args(input: &str) -> (&str, Option<&str>) {
    match input.split_once(',') {
        Some((association_type, endpoint_role)) => (
            association_type.trim(),
            Some(endpoint_role.trim()).filter(|role| !role.is_empty()),
        ),
        None => (input.trim(), None),
    }
}

fn parse_function_call(expression: &str) -> Option<(&str, Vec<String>)> {
    let expression = expression.trim();
    let open = expression.find('(')?;
    if !expression.ends_with(')') {
        return None;
    }
    let function = expression[..open].trim();
    if function.is_empty()
        || !function
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return None;
    }
    let inner = &expression[open + 1..expression.len() - 1];
    Some((function, split_args(inner)))
}

fn split_args(input: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut start = 0usize;
    let mut depth = 0i32;
    let mut quote = None;
    let bytes = input.as_bytes();
    for (index, ch) in input.char_indices() {
        if let Some(active) = quote {
            if ch == active && bytes.get(index.wrapping_sub(1)) != Some(&b'\\') {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                args.push(input[start..index].trim().to_string());
                start = index + 1;
            }
            _ => {}
        }
    }
    let tail = input[start..].trim();
    if !tail.is_empty() || !input.is_empty() {
        args.push(tail.to_string());
    }
    args
}

fn literal_value(expression: &str) -> Option<Value> {
    let expression = expression.trim();
    if expression == "null" {
        return Some(Value::Null);
    }
    if expression == "true" {
        return Some(Value::Bool(true));
    }
    if expression == "false" {
        return Some(Value::Bool(false));
    }
    if (expression.starts_with('"') && expression.ends_with('"'))
        || (expression.starts_with('\'') && expression.ends_with('\''))
    {
        return Some(Value::String(
            expression[1..expression.len() - 1].to_string(),
        ));
    }
    if let Ok(value) = expression.parse::<i64>() {
        return Some(json!(value));
    }
    if let Ok(value) = expression.parse::<f64>() {
        return Some(json!(value));
    }
    None
}

fn min_max_json(values: Vec<Value>, min: bool) -> Value {
    values
        .into_iter()
        .filter(|value| !value.is_null())
        .min_by(|left, right| {
            let ordering = compare_json_order(left, right);
            if min {
                ordering
            } else {
                ordering.reverse()
            }
        })
        .unwrap_or(Value::Null)
}

fn compare_json_order(left: &Value, right: &Value) -> std::cmp::Ordering {
    match (
        json_number_f64(left.clone()),
        json_number_f64(right.clone()),
    ) {
        (Some(left), Some(right)) => left.total_cmp(&right),
        _ => left.to_string().cmp(&right.to_string()),
    }
}

fn compare_json_values(left: &Value, right: &Value, op: &str) -> bool {
    let ordering = compare_json_order(left, right);
    match op {
        "==" => left == right,
        "!=" => left != right,
        ">" => ordering.is_gt(),
        ">=" => !ordering.is_lt(),
        "<" => ordering.is_lt(),
        "<=" => !ordering.is_gt(),
        _ => false,
    }
}

fn json_number_f64(value: Value) -> Option<f64> {
    value.as_f64()
}

fn json_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(values) => !values.is_empty(),
        Value::Object(values) => !values.is_empty(),
    }
}

fn record_kind_name(kind: RecordKind) -> &'static str {
    match kind {
        RecordKind::Delta => "delta",
        RecordKind::Snapshot => "snapshot",
        RecordKind::ReservedLegacyMaterializedDelta => "reserved_legacy_materialized_delta",
        RecordKind::Baseline => "baseline",
        RecordKind::Tombstone => "tombstone",
        _ => "unknown",
    }
}

impl ProjectionModel {
    fn rows_for_projection_for_aggregate(&self) -> Result<Vec<ProjectionRow>, String> {
        if !self.persisted {
            return Ok(self.rows.clone());
        }
        Ok(self.reconstructed_rows.clone())
    }
}

#[cfg(test)]
pub(crate) fn property_by_name(row: &ObjectRow, property_name: &str) -> Value {
    row.properties
        .values()
        .find(|property| property.entry.property_name == property_name)
        .map(|property| property.value.clone())
        .unwrap_or(Value::Null)
}

fn projection_property_by_name(row: &ProjectionRow, property_name: &str) -> Value {
    row.properties
        .iter()
        .find(|property| property.property_name == property_name)
        .map(|property| property.value.clone())
        .unwrap_or(Value::Null)
}

fn projection_property_by_flag_or_name(
    row: &ProjectionRow,
    flag: u32,
    property_name: &str,
) -> Value {
    row.properties
        .iter()
        .find(|property| property.flags & flag != 0)
        .map(|property| property.value.clone())
        .unwrap_or_else(|| projection_property_by_name(row, property_name))
}

fn row_matches_association(row: &ProjectionRow, association_type: &str) -> bool {
    if row.object_type_flags & (OBJECT_TYPE_FLAG_ASSOCIATION_OBJECT | OBJECT_TYPE_FLAG_LINK_OBJECT)
        != 0
    {
        let flagged = projection_property_by_flag_or_name(
            row,
            PROPERTY_FLAG_ASSOCIATION_TYPE,
            "association_type",
        );
        if flagged.as_str() == Some(association_type) {
            return true;
        }
        if row.object_type.strip_prefix("Association:") == Some(association_type) {
            return true;
        }
    }
    row.object_type == format!("Association:{association_type}")
}

pub fn run_fixture_path(path: &Path) -> Result<(), String> {
    let bytes = fs::read(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    let fixture: Value = serde_json::from_slice(&bytes)
        .map_err(|err| format!("fixture {} is not valid JSON: {err}", path.display()))?;
    let map = PathBuf::from(required_str(&fixture, "mapping")?);
    let sources = fixture
        .get("sources")
        .and_then(Value::as_array)
        .ok_or_else(|| "fixture.sources must be an array".to_string())?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(PathBuf::from)
                .ok_or_else(|| "fixture.sources entries must be strings".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let file = parse_map(&map)?;
    let rows = read_sources(&sources)?;
    if let Some(expected_rows) = fixture.get("expected_projected_rows") {
        let projected = project_rows(&file, &rows)?;
        if &projected["rows"] != expected_rows {
            return Err("fixture projected rows did not match".into());
        }
    }
    println!("{}", json!({"ok": true, "fixture": path}));
    Ok(())
}
