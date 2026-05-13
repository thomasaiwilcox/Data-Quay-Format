//! Spec §24/§52 — authoritative child schema metadata for native COVE-T
//! nested columns.
//!
//! Page-local nested layouts describe row shape for one page. This section
//! supplies the stable recursive child schema for each top-level List, Struct,
//! or Map table column so readers can build typed Arrow/DataFusion schemas
//! without guessing from page payloads.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    constants::{CoveLogicalType, CovePhysicalKind},
    table::{ColumnEntry, TableCatalog},
    types::{validate_logical_physical_pair_with_options, LogicalPhysicalOptions},
    CoveError,
};

pub const NESTED_SCHEMA_MAGIC: [u8; 4] = *b"NSC1";
pub const NESTED_SCHEMA_VERSION: u16 = 1;
const NESTED_SCHEMA_HEADER_LEN: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NestedSchemaSectionV1 {
    pub entries: Vec<NestedSchemaEntryV1>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NestedSchemaEntryV1 {
    pub table_id: u32,
    pub column_id: u32,
    pub root: NestedSchemaNodeV1,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NestedSchemaNodeV1 {
    pub name: String,
    pub logical: CoveLogicalType,
    pub physical: CovePhysicalKind,
    pub nullable: bool,
    pub precision: u16,
    pub scale: i16,
    pub collation_id: u16,
    pub flags: u32,
    /// Fixed-size-list length. Zero means ordinary variable-size list.
    pub fixed_size_list_len: u32,
    pub children: Vec<NestedSchemaNodeV1>,
}

impl NestedSchemaSectionV1 {
    pub fn new(entries: Vec<NestedSchemaEntryV1>) -> Self {
        Self { entries }
    }

    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        let entry_count = u32::try_from(self.entries.len())
            .map_err(|_| CoveError::BadSchema("too many nested schema entries".into()))?;
        let mut out = Vec::new();
        out.extend_from_slice(&NESTED_SCHEMA_MAGIC);
        out.extend_from_slice(&NESTED_SCHEMA_VERSION.to_le_bytes());
        out.extend_from_slice(&(NESTED_SCHEMA_HEADER_LEN as u16).to_le_bytes());
        out.extend_from_slice(&entry_count.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        for entry in &self.entries {
            out.extend_from_slice(&entry.table_id.to_le_bytes());
            out.extend_from_slice(&entry.column_id.to_le_bytes());
            entry.root.write_to(&mut out)?;
        }
        Ok(out)
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < NESTED_SCHEMA_HEADER_LEN {
            return Err(CoveError::BufferTooShort);
        }
        if bytes[0..4] != NESTED_SCHEMA_MAGIC {
            return Err(CoveError::BadMagic);
        }
        let version = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
        if version != NESTED_SCHEMA_VERSION {
            return Err(CoveError::BadVersion);
        }
        let header_len = u16::from_le_bytes(bytes[6..8].try_into().unwrap()) as usize;
        if header_len != NESTED_SCHEMA_HEADER_LEN {
            return Err(CoveError::BadSection(format!(
                "NestedSchema header_len must be {NESTED_SCHEMA_HEADER_LEN}, got {header_len}"
            )));
        }
        let entry_count = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let reserved = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
        if reserved != 0 {
            return Err(CoveError::ReservedNotZero);
        }
        let mut pos = header_len;
        let mut entries = Vec::with_capacity(entry_count);
        for _ in 0..entry_count {
            if bytes.len() < pos + 8 {
                return Err(CoveError::BufferTooShort);
            }
            let table_id = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
            pos += 4;
            let column_id = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
            pos += 4;
            let root = NestedSchemaNodeV1::read_from(bytes, &mut pos)?;
            entries.push(NestedSchemaEntryV1 {
                table_id,
                column_id,
                root,
            });
        }
        if pos != bytes.len() {
            return Err(CoveError::BadSection(
                "NestedSchema section has trailing bytes".into(),
            ));
        }
        let section = Self { entries };
        section.validate()?;
        Ok(section)
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        let mut seen = BTreeSet::new();
        for entry in &self.entries {
            if !seen.insert((entry.table_id, entry.column_id)) {
                return Err(CoveError::BadSchema(format!(
                    "duplicate NestedSchema entry for table {} column {}",
                    entry.table_id, entry.column_id
                )));
            }
            entry.root.validate_node(true)?;
        }
        Ok(())
    }

    pub fn validate_for_catalog(&self, catalog: &TableCatalog) -> Result<(), CoveError> {
        self.validate()?;
        let entries = self
            .entries
            .iter()
            .map(|entry| ((entry.table_id, entry.column_id), entry))
            .collect::<BTreeMap<_, _>>();
        let mut nested_columns = BTreeSet::new();
        for table in &catalog.tables {
            for column in &table.columns {
                if !column_uses_nested_schema(column) {
                    continue;
                }
                nested_columns.insert((table.table_id, column.column_id));
                let Some(entry) = entries.get(&(table.table_id, column.column_id)) else {
                    return Err(CoveError::BadSchema(format!(
                        "nested column {}.{} is missing NestedSchema metadata",
                        table.table_id, column.column_id
                    )));
                };
                entry.root.validate_matches_column(column)?;
            }
        }
        for entry in &self.entries {
            if !nested_columns.contains(&(entry.table_id, entry.column_id)) {
                return Err(CoveError::BadSchema(format!(
                    "NestedSchema entry references non-nested or missing column {}.{}",
                    entry.table_id, entry.column_id
                )));
            }
        }
        Ok(())
    }

    pub fn entry(&self, table_id: u32, column_id: u32) -> Option<&NestedSchemaEntryV1> {
        self.entries
            .iter()
            .find(|entry| entry.table_id == table_id && entry.column_id == column_id)
    }
}

impl NestedSchemaNodeV1 {
    pub fn scalar(
        name: impl Into<String>,
        logical: CoveLogicalType,
        physical: CovePhysicalKind,
        nullable: bool,
    ) -> Self {
        Self {
            name: name.into(),
            logical,
            physical,
            nullable,
            precision: 0,
            scale: 0,
            collation_id: 0,
            flags: 0,
            fixed_size_list_len: 0,
            children: Vec::new(),
        }
    }

    pub fn encoded_len(&self) -> Result<usize, CoveError> {
        let mut len = 2usize
            .checked_add(self.name.len())
            .and_then(|v| v.checked_add(2 + 1 + 1 + 2 + 2 + 2 + 2 + 4 + 4))
            .ok_or(CoveError::ArithOverflow)?;
        for child in &self.children {
            len = len
                .checked_add(child.encoded_len()?)
                .ok_or(CoveError::ArithOverflow)?;
        }
        Ok(len)
    }

    fn write_to(&self, out: &mut Vec<u8>) -> Result<(), CoveError> {
        let name_len = u16::try_from(self.name.len())
            .map_err(|_| CoveError::BadSchema("nested schema name exceeds u16::MAX".into()))?;
        let child_count = u16::try_from(self.children.len())
            .map_err(|_| CoveError::BadSchema("nested schema has too many children".into()))?;
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(self.name.as_bytes());
        out.extend_from_slice(&(self.logical as u16).to_le_bytes());
        out.push(self.physical as u8);
        out.push(u8::from(self.nullable));
        out.extend_from_slice(&self.precision.to_le_bytes());
        out.extend_from_slice(&self.scale.to_le_bytes());
        out.extend_from_slice(&self.collation_id.to_le_bytes());
        out.extend_from_slice(&child_count.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        out.extend_from_slice(&self.fixed_size_list_len.to_le_bytes());
        for child in &self.children {
            child.write_to(out)?;
        }
        Ok(())
    }

    fn read_from(bytes: &[u8], pos: &mut usize) -> Result<Self, CoveError> {
        if bytes.len() < *pos + 2 {
            return Err(CoveError::BufferTooShort);
        }
        let name_len = u16::from_le_bytes(bytes[*pos..*pos + 2].try_into().unwrap()) as usize;
        *pos += 2;
        let name_end = (*pos)
            .checked_add(name_len)
            .ok_or(CoveError::ArithOverflow)?;
        if name_end > bytes.len() {
            return Err(CoveError::BufferTooShort);
        }
        let name = std::str::from_utf8(&bytes[*pos..name_end])
            .map_err(|_| CoveError::BadSchema("nested schema name is not valid UTF-8".into()))?
            .to_string();
        *pos = name_end;
        if bytes.len() < *pos + 2 + 1 + 1 + 2 + 2 + 2 + 2 + 4 + 4 {
            return Err(CoveError::BufferTooShort);
        }
        let logical_raw = u16::from_le_bytes(bytes[*pos..*pos + 2].try_into().unwrap());
        *pos += 2;
        let physical_raw = bytes[*pos];
        *pos += 1;
        let nullable = match bytes[*pos] {
            0 => false,
            1 => true,
            other => {
                return Err(CoveError::BadSchema(format!(
                    "nested schema nullable flag must be 0 or 1, got {other}"
                )))
            }
        };
        *pos += 1;
        let precision = u16::from_le_bytes(bytes[*pos..*pos + 2].try_into().unwrap());
        *pos += 2;
        let scale = i16::from_le_bytes(bytes[*pos..*pos + 2].try_into().unwrap());
        *pos += 2;
        let collation_id = u16::from_le_bytes(bytes[*pos..*pos + 2].try_into().unwrap());
        *pos += 2;
        let child_count = u16::from_le_bytes(bytes[*pos..*pos + 2].try_into().unwrap()) as usize;
        *pos += 2;
        let flags = u32::from_le_bytes(bytes[*pos..*pos + 4].try_into().unwrap());
        *pos += 4;
        let fixed_size_list_len = u32::from_le_bytes(bytes[*pos..*pos + 4].try_into().unwrap());
        *pos += 4;
        let logical = CoveLogicalType::from_u16(logical_raw).ok_or_else(|| {
            CoveError::BadSchema(format!("unknown nested logical type {logical_raw}"))
        })?;
        let physical = CovePhysicalKind::from_u8(physical_raw).ok_or_else(|| {
            CoveError::BadSchema(format!("unknown nested physical kind {physical_raw}"))
        })?;
        let mut children = Vec::with_capacity(child_count);
        for _ in 0..child_count {
            children.push(Self::read_from(bytes, pos)?);
        }
        Ok(Self {
            name,
            logical,
            physical,
            nullable,
            precision,
            scale,
            collation_id,
            flags,
            fixed_size_list_len,
            children,
        })
    }

    pub fn is_container(&self) -> bool {
        matches!(
            self.physical,
            CovePhysicalKind::List | CovePhysicalKind::Struct | CovePhysicalKind::Map
        )
    }

    pub fn validate_node(&self, is_root: bool) -> Result<(), CoveError> {
        if self.name.is_empty() {
            return Err(CoveError::BadSchema(
                "nested schema node name must not be empty".into(),
            ));
        }
        validate_logical_physical_pair_with_options(
            self.logical,
            self.physical,
            LogicalPhysicalOptions {
                bool_declared_numeric: self.flags & crate::table::COLUMN_FLAG_BOOL_DECLARED_NUMERIC
                    != 0,
            },
        )?;
        if is_root && !self.is_container() {
            return Err(CoveError::BadSchema(
                "NestedSchema root must be List, Struct, or Map".into(),
            ));
        }
        match self.physical {
            CovePhysicalKind::List => {
                if self.children.len() != 1 {
                    return Err(CoveError::BadSchema(
                        "List nested schema node must have exactly one child".into(),
                    ));
                }
            }
            CovePhysicalKind::Struct => {
                if self.children.is_empty() {
                    return Err(CoveError::BadSchema(
                        "Struct nested schema node must have at least one child".into(),
                    ));
                }
                let mut names = BTreeSet::new();
                for child in &self.children {
                    if !names.insert(child.name.as_str()) {
                        return Err(CoveError::BadSchema(format!(
                            "Struct nested schema contains duplicate child name '{}'",
                            child.name
                        )));
                    }
                }
            }
            CovePhysicalKind::Map => {
                if self.children.len() != 2 {
                    return Err(CoveError::BadSchema(
                        "Map nested schema node must have exactly key and value children".into(),
                    ));
                }
                let key = &self.children[0];
                if key.name != "key" {
                    return Err(CoveError::BadSchema(
                        "Map nested schema first child must be named key".into(),
                    ));
                }
                if key.nullable {
                    return Err(CoveError::BadSchema(
                        "Map nested schema keys must be non-null".into(),
                    ));
                }
                if key.is_container() {
                    return Err(CoveError::BadSchema(
                        "Map nested schema keys must be scalar".into(),
                    ));
                }
                if self.children[1].name != "value" {
                    return Err(CoveError::BadSchema(
                        "Map nested schema second child must be named value".into(),
                    ));
                }
            }
            _ => {
                if !self.children.is_empty() {
                    return Err(CoveError::BadSchema(
                        "Scalar nested schema node must not have children".into(),
                    ));
                }
                if self.fixed_size_list_len != 0 {
                    return Err(CoveError::BadSchema(
                        "fixed_size_list_len is valid only on List nodes".into(),
                    ));
                }
            }
        }
        if self.fixed_size_list_len != 0 && self.physical != CovePhysicalKind::List {
            return Err(CoveError::BadSchema(
                "fixed_size_list_len is valid only on List nodes".into(),
            ));
        }
        for child in &self.children {
            child.validate_node(false)?;
        }
        Ok(())
    }

    fn validate_matches_column(&self, column: &ColumnEntry) -> Result<(), CoveError> {
        if self.name != column.name
            || self.logical != column.logical
            || self.physical != column.physical
            || self.nullable != column.nullable
            || self.precision != column.precision
            || self.scale != column.scale
            || self.collation_id != column.collation_id
            || self.flags != column.flags
        {
            return Err(CoveError::BadSchema(format!(
                "NestedSchema root does not match table column {}",
                column.column_id
            )));
        }
        self.validate_node(true)
    }
}

pub fn column_uses_nested_schema(column: &ColumnEntry) -> bool {
    matches!(
        column.physical,
        CovePhysicalKind::List | CovePhysicalKind::Struct | CovePhysicalKind::Map
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::{ColumnEntry, TableEntry};

    fn list_entry() -> NestedSchemaEntryV1 {
        NestedSchemaEntryV1 {
            table_id: 1,
            column_id: 7,
            root: NestedSchemaNodeV1 {
                name: "items".into(),
                logical: CoveLogicalType::List,
                physical: CovePhysicalKind::List,
                nullable: true,
                precision: 0,
                scale: 0,
                collation_id: 0,
                flags: 0,
                fixed_size_list_len: 0,
                children: vec![NestedSchemaNodeV1::scalar(
                    "item",
                    CoveLogicalType::Int32,
                    CovePhysicalKind::NumCode,
                    true,
                )],
            },
        }
    }

    fn catalog() -> TableCatalog {
        TableCatalog {
            flags: 0,
            tables: vec![TableEntry {
                table_id: 1,
                namespace: "test".into(),
                name: "t".into(),
                row_count: 0,
                primary_sort_key_count: 0,
                clustering_key_count: 0,
                flags: 0,
                columns: vec![ColumnEntry {
                    column_id: 7,
                    name: "items".into(),
                    logical: CoveLogicalType::List,
                    physical: CovePhysicalKind::List,
                    nullable: true,
                    sort_order: 0,
                    collation_id: 0,
                    precision: 0,
                    scale: 0,
                    flags: 0,
                }],
            }],
        }
    }

    #[test]
    fn nested_schema_round_trips() {
        let section = NestedSchemaSectionV1::new(vec![list_entry()]);
        let bytes = section.serialize().unwrap();
        let parsed = NestedSchemaSectionV1::parse(&bytes).unwrap();
        assert_eq!(parsed, section);
        parsed.validate_for_catalog(&catalog()).unwrap();
    }

    #[test]
    fn missing_root_for_nested_column_is_rejected() {
        let section = NestedSchemaSectionV1::new(Vec::new());
        assert!(matches!(
            section.validate_for_catalog(&catalog()),
            Err(CoveError::BadSchema(_))
        ));
    }

    #[test]
    fn mismatched_root_type_is_rejected() {
        let mut section = NestedSchemaSectionV1::new(vec![list_entry()]);
        section.entries[0].root.physical = CovePhysicalKind::Struct;
        assert!(matches!(
            section.validate_for_catalog(&catalog()),
            Err(CoveError::BadSchema(_)) | Err(CoveError::BadLogicalPhysicalPair)
        ));
    }

    #[test]
    fn duplicate_struct_child_names_are_rejected() {
        let node = NestedSchemaNodeV1 {
            name: "s".into(),
            logical: CoveLogicalType::Struct,
            physical: CovePhysicalKind::Struct,
            nullable: false,
            precision: 0,
            scale: 0,
            collation_id: 0,
            flags: 0,
            fixed_size_list_len: 0,
            children: vec![
                NestedSchemaNodeV1::scalar(
                    "x",
                    CoveLogicalType::Int32,
                    CovePhysicalKind::NumCode,
                    false,
                ),
                NestedSchemaNodeV1::scalar(
                    "x",
                    CoveLogicalType::Int64,
                    CovePhysicalKind::NumCode,
                    false,
                ),
            ],
        };
        assert!(matches!(
            node.validate_node(true),
            Err(CoveError::BadSchema(_))
        ));
    }

    #[test]
    fn bad_fixed_size_len_is_rejected() {
        let mut node = NestedSchemaNodeV1::scalar(
            "x",
            CoveLogicalType::Int32,
            CovePhysicalKind::NumCode,
            false,
        );
        node.fixed_size_list_len = 4;
        assert!(matches!(
            node.validate_node(false),
            Err(CoveError::BadSchema(_))
        ));
    }
}
