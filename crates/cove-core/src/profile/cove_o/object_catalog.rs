use crate::{
    constants::{CoveLogicalType, CovePhysicalKind},
    types::{validate_logical_physical_pair_with_options, LogicalPhysicalOptions},
    CoveError,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectTypeCatalog {
    pub flags: u32,
    pub types: Vec<ObjectTypeEntryV1>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectTypeEntryV1 {
    pub object_type_id: u32,
    pub type_name: String,
    pub flags: u32,
    pub properties: Vec<PropertyEntryV1>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyEntryV1 {
    pub property_id: u32,
    pub property_name: String,
    pub logical_type: CoveLogicalType,
    pub physical_kind: CovePhysicalKind,
    pub nullable: bool,
    pub collation_id: u16,
    pub flags: u32,
}

pub const OBJECT_TYPE_FLAG_ENTITY_OBJECT: u32 = 0x0000_0001;
pub const OBJECT_TYPE_FLAG_EVENT_OBJECT: u32 = 0x0000_0002;
pub const OBJECT_TYPE_FLAG_LINK_OBJECT: u32 = 0x0000_0004;
pub const OBJECT_TYPE_FLAG_ASSOCIATION_OBJECT: u32 = 0x0000_0008;
pub const OBJECT_TYPE_FLAG_EVIDENCE_OBJECT: u32 = 0x0000_0010;
pub const OBJECT_TYPE_FLAG_PROJECTION_OBJECT: u32 = 0x0000_0020;

pub const PROPERTY_FLAG_ASSOCIATION_FROM_GOID: u32 = 0x0000_0001;
pub const PROPERTY_FLAG_ASSOCIATION_TO_GOID: u32 = 0x0000_0002;
pub const PROPERTY_FLAG_ASSOCIATION_TYPE: u32 = 0x0000_0004;
pub const PROPERTY_FLAG_ASSOCIATION_VALID_FROM: u32 = 0x0000_0008;
pub const PROPERTY_FLAG_ASSOCIATION_VALID_TO: u32 = 0x0000_0010;
pub const PROPERTY_FLAG_ASSOCIATION_OBSERVED_AT: u32 = 0x0000_0020;
pub const PROPERTY_FLAG_EVIDENCE_REF: u32 = 0x0000_0040;
pub const PROPERTY_FLAG_MAPPING_RULE_REF: u32 = 0x0000_0080;
pub const PROPERTY_FLAG_BOOL_DECLARED_NUMERIC: u32 = 0x0000_0100;

impl ObjectTypeCatalog {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < 8 {
            return Err(CoveError::BufferTooShort);
        }
        let type_count = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let flags = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let mut pos = 8usize;
        let mut types = Vec::with_capacity(type_count);
        for _ in 0..type_count {
            let (entry, used) = ObjectTypeEntryV1::parse(&bytes[pos..])?;
            pos = pos.checked_add(used).ok_or(CoveError::ArithOverflow)?;
            types.push(entry);
        }
        if pos != bytes.len() {
            return Err(CoveError::BadSchema(
                "object type catalog has trailing bytes".into(),
            ));
        }
        let catalog = Self { flags, types };
        catalog.validate()?;
        Ok(catalog)
    }

    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        let count = u32::try_from(self.types.len())
            .map_err(|_| CoveError::BadSchema("too many object types".into()))?;
        let mut out = Vec::new();
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        for ty in &self.types {
            out.extend_from_slice(&ty.serialize()?);
        }
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        let mut seen_types = std::collections::HashSet::new();
        for ty in &self.types {
            if !seen_types.insert(ty.object_type_id) {
                return Err(CoveError::BadSchema(format!(
                    "duplicate object_type_id {} (Spec §56)",
                    ty.object_type_id
                )));
            }
            let mut seen_props = std::collections::HashSet::new();
            for prop in &ty.properties {
                if !seen_props.insert(prop.property_id) {
                    return Err(CoveError::BadSchema(format!(
                        "duplicate property_id {} in object_type_id {} (Spec §56)",
                        prop.property_id, ty.object_type_id
                    )));
                }
                if prop.logical_type == CoveLogicalType::Null {
                    return Err(CoveError::BadSchema(format!(
                        "property {} declares logical Null at top level (Spec §56)",
                        prop.property_id
                    )));
                }
                if validate_logical_physical_pair_with_options(
                    prop.logical_type,
                    prop.physical_kind,
                    LogicalPhysicalOptions {
                        bool_declared_numeric: prop.flags & PROPERTY_FLAG_BOOL_DECLARED_NUMERIC
                            != 0,
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

impl ObjectTypeEntryV1 {
    pub fn parse(bytes: &[u8]) -> Result<(Self, usize), CoveError> {
        if bytes.len() < 4 + 2 {
            return Err(CoveError::BufferTooShort);
        }
        let object_type_id = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let mut pos = 4usize;
        let type_name = read_str(bytes, &mut pos, "object type name")?;
        if bytes.len() < pos + 4 + 2 {
            return Err(CoveError::BufferTooShort);
        }
        let flags = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
        pos += 4;
        let property_count = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap()) as usize;
        pos += 2;
        let mut properties = Vec::with_capacity(property_count);
        for _ in 0..property_count {
            let (property, used) = PropertyEntryV1::parse(&bytes[pos..])?;
            pos = pos.checked_add(used).ok_or(CoveError::ArithOverflow)?;
            properties.push(property);
        }
        Ok((
            Self {
                object_type_id,
                type_name,
                flags,
                properties,
            },
            pos,
        ))
    }

    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        let property_count = u16::try_from(self.properties.len())
            .map_err(|_| CoveError::BadSchema("too many properties".into()))?;
        let mut out = Vec::new();
        out.extend_from_slice(&self.object_type_id.to_le_bytes());
        write_str(&mut out, &self.type_name, "object type name")?;
        out.extend_from_slice(&self.flags.to_le_bytes());
        out.extend_from_slice(&property_count.to_le_bytes());
        for prop in &self.properties {
            out.extend_from_slice(&prop.serialize()?);
        }
        Ok(out)
    }
}

impl PropertyEntryV1 {
    pub fn parse(bytes: &[u8]) -> Result<(Self, usize), CoveError> {
        if bytes.len() < 4 + 2 {
            return Err(CoveError::BufferTooShort);
        }
        let property_id = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let mut pos = 4usize;
        let property_name = read_str(bytes, &mut pos, "property name")?;
        if bytes.len() < pos + 2 + 1 + 1 + 2 + 4 {
            return Err(CoveError::BufferTooShort);
        }
        let logical_raw = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap());
        pos += 2;
        let physical_raw = bytes[pos];
        pos += 1;
        let nullable_raw = bytes[pos];
        pos += 1;
        let collation_id = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap());
        pos += 2;
        let flags = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
        pos += 4;
        let logical_type = CoveLogicalType::from_u16(logical_raw).ok_or_else(|| {
            CoveError::BadSchema(format!(
                "unknown object property logical type {logical_raw}"
            ))
        })?;
        let physical_kind = CovePhysicalKind::from_u8(physical_raw).ok_or_else(|| {
            CoveError::BadSchema(format!(
                "unknown object property physical kind {physical_raw}"
            ))
        })?;
        let nullable = match nullable_raw {
            0 => false,
            1 => true,
            other => {
                return Err(CoveError::BadSchema(format!(
                    "object property nullable flag must be 0 or 1, got {other}"
                )))
            }
        };
        Ok((
            Self {
                property_id,
                property_name,
                logical_type,
                physical_kind,
                nullable,
                collation_id,
                flags,
            },
            pos,
        ))
    }

    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        let mut out = Vec::new();
        out.extend_from_slice(&self.property_id.to_le_bytes());
        write_str(&mut out, &self.property_name, "property name")?;
        out.extend_from_slice(&(self.logical_type as u16).to_le_bytes());
        out.push(self.physical_kind as u8);
        out.push(if self.nullable { 1 } else { 0 });
        out.extend_from_slice(&self.collation_id.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        Ok(out)
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

fn write_str(out: &mut Vec<u8>, s: &str, what: &str) -> Result<(), CoveError> {
    let len = u16::try_from(s.len())
        .map_err(|_| CoveError::BadSchema(format!("{what} exceeds u16::MAX")))?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(s.as_bytes());
    Ok(())
}
