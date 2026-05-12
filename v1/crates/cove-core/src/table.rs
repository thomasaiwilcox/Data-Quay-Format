//! Spec §24 — COVE-T Table Catalog (spec-exact wire format).
//!
//! The table catalog declares every COVE-T table in the file: its stable
//! `table_id`, optional namespace, table name, declared `row_count`,
//! sort/clustering key counts, and ordered column entries. Each column
//! carries its `column_id`, name, logical/physical pair, nullability,
//! sort_order, collation_id, and decimal precision/scale.
//!
//! Wire layout per Spec §24:
//!
//! ```text
//! TableCatalogV1 = u32 table_count, u32 flags, TableEntryV1[table_count]
//!
//! TableEntryV1 = u32  table_id,
//!                u16  namespace_len, [u8; namespace_len] namespace,
//!                u16  table_name_len, [u8; table_name_len] table_name,
//!                u32  column_count, u64 row_count,
//!                u16  primary_sort_key_count, u16 clustering_key_count,
//!                u32  flags,
//!                TableColumnEntryV1[column_count]
//!
//! TableColumnEntryV1 = u32 column_id,
//!                      u16 column_name_len, [u8; column_name_len] column_name,
//!                      u16 logical_type, u8 physical_kind, u8 nullable,
//!                      u16 sort_order, u16 collation_id,
//!                      u16 precision, i16 scale,
//!                      u32 flags
//! ```
//!
//! Spec §24 Rules enforced by this module:
//! * `table_id` MUST be unique across the catalog.
//! * `column_id` MUST be unique within a table.
//! * `logical_type` and `physical_kind` MUST be compatible (Spec §19).
//! * Top-level columns MUST NOT use logical `Null` (Spec §24.4).
//! * `nullable` is `0` or `1`; any other value is a schema error.
//!
//! `sort_order` is the per-column sort indicator (`0` = no declared sort,
//! non-zero values are interpreted in conjunction with the `SortKeyEntryV1`
//! list of Spec §53). `collation_id` references the collation registry
//! (Spec §22). `precision`/`scale` apply to decimal logical types and are
//! ignored otherwise.

use crate::{
    constants::{CoveLogicalType, CovePhysicalKind},
    types::{validate_logical_physical_pair_with_options, LogicalPhysicalOptions},
    CoveError,
};

// ── ColumnEntry ──────────────────────────────────────────────────────────────

pub const COLUMN_FLAG_BOOL_DECLARED_NUMERIC: u32 = 0x0000_0001;

/// Spec §24 `TableColumnEntryV1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnEntry {
    /// Stable column id, unique within the table (Spec §24).
    pub column_id: u32,
    pub name: String,
    /// Logical type (Spec §18).
    pub logical: CoveLogicalType,
    /// Physical kind (Spec §19).
    pub physical: CovePhysicalKind,
    pub nullable: bool,
    /// Per-column sort indicator (Spec §53). `0` means no declared sort.
    pub sort_order: u16,
    /// Collation registry id (Spec §22). `0` means default/identity.
    pub collation_id: u16,
    /// Decimal precision (ignored for non-decimal logical types).
    pub precision: u16,
    /// Decimal scale (ignored for non-decimal logical types).
    pub scale: i16,
    /// Per-column flags (reserved for future use).
    pub flags: u32,
}

impl ColumnEntry {
    /// Encoded length on the wire.
    pub fn encoded_len(&self) -> usize {
        4 + 2 + self.name.len() + 2 + 1 + 1 + 2 + 2 + 2 + 2 + 4
    }

    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        if self.name.len() > u16::MAX as usize {
            return Err(CoveError::BadSchema("column name exceeds u16::MAX".into()));
        }
        let mut out = Vec::with_capacity(self.encoded_len());
        out.extend_from_slice(&self.column_id.to_le_bytes());
        out.extend_from_slice(&(self.name.len() as u16).to_le_bytes());
        out.extend_from_slice(self.name.as_bytes());
        out.extend_from_slice(&(self.logical as u16).to_le_bytes());
        out.push(self.physical as u8);
        out.push(if self.nullable { 1 } else { 0 });
        out.extend_from_slice(&self.sort_order.to_le_bytes());
        out.extend_from_slice(&self.collation_id.to_le_bytes());
        out.extend_from_slice(&self.precision.to_le_bytes());
        out.extend_from_slice(&self.scale.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        Ok(out)
    }

    pub fn parse(bytes: &[u8]) -> Result<(Self, usize), CoveError> {
        if bytes.len() < 4 + 2 {
            return Err(CoveError::BufferTooShort);
        }
        let column_id = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let mut pos = 4usize;
        let name = read_str(bytes, &mut pos, "column name")?;
        if bytes.len() < pos + 2 + 1 + 1 + 2 + 2 + 2 + 2 + 4 {
            return Err(CoveError::BufferTooShort);
        }
        let lt_raw = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap());
        pos += 2;
        let pk_raw = bytes[pos];
        pos += 1;
        let nullable_raw = bytes[pos];
        pos += 1;
        let sort_order = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap());
        pos += 2;
        let collation_id = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap());
        pos += 2;
        let precision = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap());
        pos += 2;
        let scale = i16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap());
        pos += 2;
        let flags = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
        pos += 4;

        let logical = CoveLogicalType::from_u16(lt_raw)
            .ok_or_else(|| CoveError::BadSchema(format!("unknown logical type {lt_raw}")))?;
        let physical = CovePhysicalKind::from_u8(pk_raw)
            .ok_or_else(|| CoveError::BadSchema(format!("unknown physical kind {pk_raw}")))?;
        let nullable = match nullable_raw {
            0 => false,
            1 => true,
            other => {
                return Err(CoveError::BadSchema(format!(
                    "nullable flag must be 0 or 1, got {other}"
                )))
            }
        };

        Ok((
            Self {
                column_id,
                name,
                logical,
                physical,
                nullable,
                sort_order,
                collation_id,
                precision,
                scale,
                flags,
            },
            pos,
        ))
    }
}

// ── TableEntry ───────────────────────────────────────────────────────────────

/// Spec §24 `TableEntryV1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableEntry {
    pub table_id: u32,
    pub namespace: String,
    pub name: String,
    pub row_count: u64,
    pub primary_sort_key_count: u16,
    pub clustering_key_count: u16,
    pub flags: u32,
    pub columns: Vec<ColumnEntry>,
}

impl TableEntry {
    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        if self.namespace.len() > u16::MAX as usize {
            return Err(CoveError::BadSchema("namespace exceeds u16::MAX".into()));
        }
        if self.name.len() > u16::MAX as usize {
            return Err(CoveError::BadSchema("table name exceeds u16::MAX".into()));
        }
        let column_count = u32::try_from(self.columns.len())
            .map_err(|_| CoveError::BadSchema("too many columns".into()))?;
        let mut out = Vec::new();
        out.extend_from_slice(&self.table_id.to_le_bytes());
        out.extend_from_slice(&(self.namespace.len() as u16).to_le_bytes());
        out.extend_from_slice(self.namespace.as_bytes());
        out.extend_from_slice(&(self.name.len() as u16).to_le_bytes());
        out.extend_from_slice(self.name.as_bytes());
        out.extend_from_slice(&column_count.to_le_bytes());
        out.extend_from_slice(&self.row_count.to_le_bytes());
        out.extend_from_slice(&self.primary_sort_key_count.to_le_bytes());
        out.extend_from_slice(&self.clustering_key_count.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        for c in &self.columns {
            out.extend_from_slice(&c.serialize()?);
        }
        Ok(out)
    }

    pub fn parse(bytes: &[u8]) -> Result<(Self, usize), CoveError> {
        if bytes.len() < 4 + 2 {
            return Err(CoveError::BufferTooShort);
        }
        let table_id = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let mut pos = 4usize;
        let namespace = read_str(bytes, &mut pos, "namespace")?;
        let name = read_str(bytes, &mut pos, "table name")?;
        if bytes.len() < pos + 4 + 8 + 2 + 2 + 4 {
            return Err(CoveError::BufferTooShort);
        }
        let column_count = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;
        let row_count = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
        pos += 8;
        let primary_sort_key_count = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap());
        pos += 2;
        let clustering_key_count = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap());
        pos += 2;
        let flags = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
        pos += 4;

        let mut columns = Vec::with_capacity(column_count);
        for _ in 0..column_count {
            let (c, used) = ColumnEntry::parse(&bytes[pos..])?;
            pos = pos.checked_add(used).ok_or(CoveError::ArithOverflow)?;
            columns.push(c);
        }

        Ok((
            Self {
                table_id,
                namespace,
                name,
                row_count,
                primary_sort_key_count,
                clustering_key_count,
                flags,
                columns,
            },
            pos,
        ))
    }
}

// ── TableCatalog ─────────────────────────────────────────────────────────────

/// Spec §24 `TableCatalogV1`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TableCatalog {
    pub flags: u32,
    pub tables: Vec<TableEntry>,
}

impl TableCatalog {
    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        let table_count = u32::try_from(self.tables.len())
            .map_err(|_| CoveError::BadSchema("too many tables".into()))?;
        let mut out = Vec::new();
        out.extend_from_slice(&table_count.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        for t in &self.tables {
            out.extend_from_slice(&t.serialize()?);
        }
        Ok(out)
    }

    /// Parse a TableCatalog section payload (Spec §24).
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < 4 + 4 {
            return Err(CoveError::BufferTooShort);
        }
        let table_count = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let flags = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let mut pos = 8usize;
        let mut tables = Vec::with_capacity(table_count);
        for _ in 0..table_count {
            let (t, used) = TableEntry::parse(&bytes[pos..])?;
            pos = pos.checked_add(used).ok_or(CoveError::ArithOverflow)?;
            tables.push(t);
        }
        let cat = Self { flags, tables };
        cat.validate()?;
        Ok(cat)
    }

    /// Spec §24 invariants:
    /// * Unique `table_id`.
    /// * Unique `column_id` per table.
    /// * Compatible (logical, physical) pair (Spec §19).
    /// * No top-level logical `Null`.
    pub fn validate(&self) -> Result<(), CoveError> {
        let mut seen_tables = std::collections::HashSet::new();
        for t in &self.tables {
            if !seen_tables.insert(t.table_id) {
                return Err(CoveError::BadSchema(format!(
                    "duplicate table_id {} (Spec §24)",
                    t.table_id
                )));
            }
            let mut seen_cols = std::collections::HashSet::new();
            for c in &t.columns {
                if !seen_cols.insert(c.column_id) {
                    return Err(CoveError::BadSchema(format!(
                        "duplicate column_id {} in table {} (Spec §24)",
                        c.column_id, t.table_id
                    )));
                }
                if c.logical == CoveLogicalType::Null {
                    return Err(CoveError::BadSchema(format!(
                        "column {} declares logical Null at top level (Spec §24)",
                        c.column_id
                    )));
                }
                if validate_logical_physical_pair_with_options(
                    c.logical,
                    c.physical,
                    LogicalPhysicalOptions {
                        bool_declared_numeric: c.flags & COLUMN_FLAG_BOOL_DECLARED_NUMERIC != 0,
                    },
                )
                .is_err()
                {
                    return Err(CoveError::BadLogicalPhysicalPair);
                }
            }
        }
        Ok(())
    }
}

fn read_str(bytes: &[u8], pos: &mut usize, what: &str) -> Result<String, CoveError> {
    if *pos + 2 > bytes.len() {
        return Err(CoveError::BufferTooShort);
    }
    let len = u16::from_le_bytes(bytes[*pos..*pos + 2].try_into().unwrap()) as usize;
    *pos += 2;
    let end = pos.checked_add(len).ok_or(CoveError::ArithOverflow)?;
    if end > bytes.len() {
        return Err(CoveError::BufferTooShort);
    }
    let s = std::str::from_utf8(&bytes[*pos..end])
        .map_err(|_| CoveError::BadSchema(format!("{what} is not valid UTF-8")))?
        .to_string();
    *pos = end;
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(
        column_id: u32,
        name: &str,
        logical: CoveLogicalType,
        physical: CovePhysicalKind,
        nullable: bool,
    ) -> ColumnEntry {
        ColumnEntry {
            column_id,
            name: name.into(),
            logical,
            physical,
            nullable,
            sort_order: 0,
            collation_id: 0,
            precision: 0,
            scale: 0,
            flags: 0,
        }
    }

    fn sample_catalog() -> TableCatalog {
        TableCatalog {
            flags: 0,
            tables: vec![TableEntry {
                table_id: 1,
                namespace: "public".into(),
                name: "users".into(),
                row_count: 1000,
                primary_sort_key_count: 1,
                clustering_key_count: 0,
                flags: 0,
                columns: vec![
                    {
                        let mut c = col(
                            10,
                            "id",
                            CoveLogicalType::Int64,
                            CovePhysicalKind::NumCode,
                            false,
                        );
                        c.sort_order = 1;
                        c
                    },
                    col(
                        11,
                        "active",
                        CoveLogicalType::Bool,
                        CovePhysicalKind::Boolean,
                        false,
                    ),
                ],
            }],
        }
    }

    #[test]
    fn catalog_roundtrip() {
        let cat = sample_catalog();
        let bytes = cat.serialize().unwrap();
        let cat2 = TableCatalog::parse(&bytes).unwrap();
        assert_eq!(cat, cat2);
        assert_eq!(cat2.tables[0].namespace, "public");
        assert_eq!(cat2.tables[0].row_count, 1000);
        assert_eq!(cat2.tables[0].primary_sort_key_count, 1);
        assert_eq!(cat2.tables[0].columns[0].sort_order, 1);
    }

    #[test]
    fn rejects_duplicate_table_id() {
        let mut cat = sample_catalog();
        cat.tables.push(cat.tables[0].clone());
        let bytes = cat.serialize().unwrap();
        assert!(matches!(
            TableCatalog::parse(&bytes),
            Err(CoveError::BadSchema(_))
        ));
    }

    #[test]
    fn rejects_duplicate_column_id() {
        let mut cat = sample_catalog();
        cat.tables[0].columns[1].column_id = cat.tables[0].columns[0].column_id;
        let bytes = cat.serialize().unwrap();
        assert!(matches!(
            TableCatalog::parse(&bytes),
            Err(CoveError::BadSchema(_))
        ));
    }

    #[test]
    fn rejects_top_level_null_logical() {
        let mut cat = sample_catalog();
        cat.tables[0].columns[0].logical = CoveLogicalType::Null;
        // Need to use a physical kind compatible with Null to avoid the
        // pair check firing first. Spec puts Null at logical id 0 with no
        // valid physical kind \u2014 the schema check fires first.
        let bytes = cat.serialize().unwrap();
        assert!(matches!(
            TableCatalog::parse(&bytes),
            Err(CoveError::BadSchema(_) | CoveError::BadLogicalPhysicalPair)
        ));
    }

    #[test]
    fn rejects_incompatible_logical_physical_pair() {
        let mut cat = sample_catalog();
        // Bool logical with VarBytes physical — incompatible (Spec §19).
        cat.tables[0].columns[1].physical = CovePhysicalKind::VarBytes;
        let bytes = cat.serialize().unwrap();
        assert_eq!(
            TableCatalog::parse(&bytes),
            Err(CoveError::BadLogicalPhysicalPair)
        );
    }

    #[test]
    fn bool_numcode_requires_numeric_declaration_flag() {
        let mut cat = sample_catalog();
        cat.tables[0].columns[1].physical = CovePhysicalKind::NumCode;
        let bytes = cat.serialize().unwrap();
        assert_eq!(
            TableCatalog::parse(&bytes),
            Err(CoveError::BadLogicalPhysicalPair)
        );

        cat.tables[0].columns[1].flags = COLUMN_FLAG_BOOL_DECLARED_NUMERIC;
        let bytes = cat.serialize().unwrap();
        assert!(TableCatalog::parse(&bytes).is_ok());
    }

    #[test]
    fn rejects_bad_nullable_flag() {
        let cat = sample_catalog();
        let mut bytes = cat.serialize().unwrap();
        // Locate the nullable byte for the first column of the first table.
        // Layout: u32 table_count + u32 flags + table_id(4) + ns_len(2) + ns(6 "public")
        //   + name_len(2) + name(5 "users") + col_count(4) + row_count(8)
        //   + sort_count(2) + clust_count(2) + flags(4)
        //   + col_id(4) + col_name_len(2) + col_name(2 "id")
        //   + logical(2) + physical(1) + nullable(1) ...
        let nullable_offset = 4 + 4 + 4 + 2 + 6 + 2 + 5 + 4 + 8 + 2 + 2 + 4 + 4 + 2 + 2 + 2 + 1;
        bytes[nullable_offset] = 7;
        assert!(matches!(
            TableCatalog::parse(&bytes),
            Err(CoveError::BadSchema(_))
        ));
    }

    #[test]
    fn empty_catalog_roundtrips() {
        let cat = TableCatalog::default();
        let bytes = cat.serialize().unwrap();
        let cat2 = TableCatalog::parse(&bytes).unwrap();
        assert_eq!(cat2.tables.len(), 0);
        assert_eq!(cat2.flags, 0);
    }
}
