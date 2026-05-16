//! COVE-COVERAGE provider, set, and proof validation for COVE v2.

use std::collections::{BTreeMap, BTreeSet};

use cove_core::{checksum, CoveError};

const ABSENT_ID: u32 = u32::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum CoverageGranularityV2 {
    Dataset = 0,
    Object = 1,
    File = 2,
    Segment = 3,
    RowGroup = 4,
    Page = 5,
    Morsel = 6,
    RowRange = 7,
    RowOrdinalSet = 8,
    MapNode = 9,
    DimensionalBucket = 10,
    ObjectPath = 11,
    Association = 12,
    ProjectionFragment = 13,
    ExternalFragment = 255,
}

impl CoverageGranularityV2 {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Dataset),
            1 => Some(Self::Object),
            2 => Some(Self::File),
            3 => Some(Self::Segment),
            4 => Some(Self::RowGroup),
            5 => Some(Self::Page),
            6 => Some(Self::Morsel),
            7 => Some(Self::RowRange),
            8 => Some(Self::RowOrdinalSet),
            9 => Some(Self::MapNode),
            10 => Some(Self::DimensionalBucket),
            11 => Some(Self::ObjectPath),
            12 => Some(Self::Association),
            13 => Some(Self::ProjectionFragment),
            255 => Some(Self::ExternalFragment),
            _ => None,
        }
    }

    pub fn from_u16(value: u16) -> Option<Self> {
        if value <= u8::MAX as u16 {
            Self::from_u8(value as u8)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CoverageProofStrengthV2 {
    ExactTight = 0,
    ExactConservative = 1,
    ProbabilisticConservative = 2,
    AdvisoryOnly = 3,
    EngineLocal = 4,
    ApproximateMayUnderInclude = 5,
}

impl CoverageProofStrengthV2 {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::ExactTight),
            1 => Some(Self::ExactConservative),
            2 => Some(Self::ProbabilisticConservative),
            3 => Some(Self::AdvisoryOnly),
            4 => Some(Self::EngineLocal),
            5 => Some(Self::ApproximateMayUnderInclude),
            _ => None,
        }
    }

    pub fn allows_pruning(self) -> bool {
        matches!(
            self,
            Self::ExactTight | Self::ExactConservative | Self::ProbabilisticConservative
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CoverageExactnessV2 {
    Exact = 0,
    ApproximateOverInclusiveOnly = 1,
    ApproximateMayUnderInclude = 2,
    Unknown = 255,
}

impl CoverageExactnessV2 {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Exact),
            1 => Some(Self::ApproximateOverInclusiveOnly),
            2 => Some(Self::ApproximateMayUnderInclude),
            255 => Some(Self::Unknown),
            _ => None,
        }
    }

    pub fn may_under_include(self) -> bool {
        matches!(self, Self::ApproximateMayUnderInclude | Self::Unknown)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum CoverageProofKindV2 {
    MinMaxExclusion = 0,
    DictionaryMembership = 1,
    BloomMaybe = 2,
    ZoneMap = 3,
    ExactSet = 4,
    ValueToFragmentIndex = 5,
    RangeBucketLayout = 6,
    SemanticPathMapping = 7,
    ObjectDimensionMapping = 8,
    AggregateSynopsis = 9,
    LookupIndex = 10,
    CompositeZone = 11,
    EngineObservedCache = 12,
    ExternalIndex = 13,
    RuntimeHint = 14,
    VendorDefined = 255,
}

impl CoverageProofKindV2 {
    pub fn from_u16(value: u16) -> Option<Self> {
        match value {
            0 => Some(Self::MinMaxExclusion),
            1 => Some(Self::DictionaryMembership),
            2 => Some(Self::BloomMaybe),
            3 => Some(Self::ZoneMap),
            4 => Some(Self::ExactSet),
            5 => Some(Self::ValueToFragmentIndex),
            6 => Some(Self::RangeBucketLayout),
            7 => Some(Self::SemanticPathMapping),
            8 => Some(Self::ObjectDimensionMapping),
            9 => Some(Self::AggregateSynopsis),
            10 => Some(Self::LookupIndex),
            11 => Some(Self::CompositeZone),
            12 => Some(Self::EngineObservedCache),
            13 => Some(Self::ExternalIndex),
            14 => Some(Self::RuntimeHint),
            255 => Some(Self::VendorDefined),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum PredicateFormKindV2 {
    PredicateAst = 0,
    PredicateCnf = 1,
    IntervalPredicateForm = 2,
    EncodedPredicateForm = 3,
    EnginePrivate = 255,
}

impl PredicateFormKindV2 {
    pub fn from_u16(value: u16) -> Option<Self> {
        match value {
            0 => Some(Self::PredicateAst),
            1 => Some(Self::PredicateCnf),
            2 => Some(Self::IntervalPredicateForm),
            3 => Some(Self::EncodedPredicateForm),
            255 => Some(Self::EnginePrivate),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredicateNormalFormV2 {
    pub predicate_form_id: u32,
    pub form_kind: PredicateFormKindV2,
    pub flags: u16,
    pub logical_context_ref: u32,
    pub payload_offset: u64,
    pub payload_length: u64,
    pub checksum: u32,
}

impl PredicateNormalFormV2 {
    pub const LEN: usize = 32;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let form = Self {
            predicate_form_id: read_u32(bytes, 0)?,
            form_kind: PredicateFormKindV2::from_u16(read_u16(bytes, 4)?)
                .ok_or(CoveError::BadCoverage)?,
            flags: read_u16(bytes, 6)?,
            logical_context_ref: read_u32(bytes, 8)?,
            payload_offset: read_u64(bytes, 12)?,
            payload_length: read_u64(bytes, 20)?,
            checksum: read_u32(bytes, 28)?,
        };
        verify_crc(&bytes[..Self::LEN], 28, form.checksum)?;
        form.validate()?;
        Ok(form)
    }

    pub fn parse_many(bytes: &[u8]) -> Result<Vec<Self>, CoveError> {
        if !bytes.len().is_multiple_of(Self::LEN) {
            return Err(CoveError::BadCoverage);
        }
        let mut ids = BTreeSet::new();
        let mut forms = Vec::new();
        for chunk in bytes.chunks_exact(Self::LEN) {
            let form = Self::parse(chunk)?;
            if !ids.insert(form.predicate_form_id) {
                return Err(CoveError::BadCoverage);
            }
            forms.push(form);
        }
        Ok(forms)
    }

    pub fn serialize(&self) -> Result<[u8; Self::LEN], CoveError> {
        self.validate()?;
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.predicate_form_id.to_le_bytes());
        out[4..6].copy_from_slice(&(self.form_kind as u16).to_le_bytes());
        out[6..8].copy_from_slice(&self.flags.to_le_bytes());
        out[8..12].copy_from_slice(&self.logical_context_ref.to_le_bytes());
        out[12..20].copy_from_slice(&self.payload_offset.to_le_bytes());
        out[20..28].copy_from_slice(&self.payload_length.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[28..32].copy_from_slice(&crc.to_le_bytes());
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        if self.logical_context_ref == ABSENT_ID {
            return Err(CoveError::BadCoverage);
        }
        checked_end(self.payload_offset, self.payload_length)?;
        if self.payload_length == 0 && self.payload_offset != 0 {
            return Err(CoveError::BadCoverage);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredicateNormalFormWithPayloadV2 {
    pub form: PredicateNormalFormV2,
    pub payload: Vec<u8>,
}

impl PredicateNormalFormWithPayloadV2 {
    pub fn parse_many(bytes: &[u8]) -> Result<Vec<Self>, CoveError> {
        if bytes.is_empty() {
            return Ok(Vec::new());
        }
        if bytes.len().is_multiple_of(PredicateNormalFormV2::LEN) {
            let forms = PredicateNormalFormV2::parse_many(bytes)?;
            if forms.iter().all(|form| form.payload_length == 0) {
                return Ok(forms
                    .into_iter()
                    .map(|form| Self {
                        form,
                        payload: Vec::new(),
                    })
                    .collect());
            }
        }

        let first = PredicateNormalFormV2::parse(bytes)?;
        let mut table_end = if first.payload_length == 0 {
            bytes.len()
        } else {
            first.payload_offset as usize
        };
        if table_end == 0
            || table_end > bytes.len()
            || !table_end.is_multiple_of(PredicateNormalFormV2::LEN)
        {
            return Err(CoveError::BadCoverage);
        }

        let mut forms = Vec::new();
        let mut ids = BTreeSet::new();
        let mut cursor = 0usize;
        while cursor < table_end {
            let next = cursor
                .checked_add(PredicateNormalFormV2::LEN)
                .ok_or(CoveError::ArithOverflow)?;
            if next > bytes.len() {
                return Err(CoveError::BufferTooShort);
            }
            let form = PredicateNormalFormV2::parse(&bytes[cursor..next])?;
            if !ids.insert(form.predicate_form_id) {
                return Err(CoveError::BadCoverage);
            }
            if form.payload_length != 0 {
                let end = checked_end(form.payload_offset, form.payload_length)? as usize;
                let offset = form.payload_offset as usize;
                if offset < next || end > bytes.len() {
                    return Err(CoveError::BadCoverage);
                }
                table_end = table_end.min(offset);
            }
            forms.push(form);
            cursor = next;
        }

        forms
            .into_iter()
            .map(|form| {
                let payload = if form.payload_length == 0 {
                    Vec::new()
                } else {
                    let offset = form.payload_offset as usize;
                    let end = checked_end(form.payload_offset, form.payload_length)? as usize;
                    bytes
                        .get(offset..end)
                        .ok_or(CoveError::BufferTooShort)?
                        .to_vec()
                };
                Ok(Self { form, payload })
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum PredicateOpV2 {
    TrueLiteral = 0,
    FalseLiteral = 1,
    IsNull = 2,
    IsNotNull = 3,
    Eq = 4,
    NotEq = 5,
    Lt = 6,
    LtEq = 7,
    Gt = 8,
    GtEq = 9,
    Between = 10,
    InSet = 11,
    And = 12,
    Or = 13,
    Not = 14,
    LikePrefix = 15,
    Contains = 16,
    IsNaN = 17,
    IsFinite = 18,
    FunctionCall = 19,
    LiteralValue = 20,
    ColumnRef = 21,
    Extension = 255,
}

impl PredicateOpV2 {
    pub fn from_u16(value: u16) -> Option<Self> {
        match value {
            0 => Some(Self::TrueLiteral),
            1 => Some(Self::FalseLiteral),
            2 => Some(Self::IsNull),
            3 => Some(Self::IsNotNull),
            4 => Some(Self::Eq),
            5 => Some(Self::NotEq),
            6 => Some(Self::Lt),
            7 => Some(Self::LtEq),
            8 => Some(Self::Gt),
            9 => Some(Self::GtEq),
            10 => Some(Self::Between),
            11 => Some(Self::InSet),
            12 => Some(Self::And),
            13 => Some(Self::Or),
            14 => Some(Self::Not),
            15 => Some(Self::LikePrefix),
            16 => Some(Self::Contains),
            17 => Some(Self::IsNaN),
            18 => Some(Self::IsFinite),
            19 => Some(Self::FunctionCall),
            20 => Some(Self::LiteralValue),
            21 => Some(Self::ColumnRef),
            255 => Some(Self::Extension),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PredicateNullPolicyV2 {
    SqlWhere = 0,
    NullExcluded = 1,
    NullIncluded = 2,
    NullOnly = 3,
    NullRejected = 4,
    ExtensionDefined = 255,
}

impl PredicateNullPolicyV2 {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::SqlWhere),
            1 => Some(Self::NullExcluded),
            2 => Some(Self::NullIncluded),
            3 => Some(Self::NullOnly),
            4 => Some(Self::NullRejected),
            255 => Some(Self::ExtensionDefined),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PredicateOperandKindV2 {
    Node = 0,
    Literal = 1,
    LiteralList = 2,
    ColumnOrPath = 3,
    Function = 4,
    IntervalSet = 5,
    Extension = 255,
}

impl PredicateOperandKindV2 {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Node),
            1 => Some(Self::Literal),
            2 => Some(Self::LiteralList),
            3 => Some(Self::ColumnOrPath),
            4 => Some(Self::Function),
            5 => Some(Self::IntervalSet),
            255 => Some(Self::Extension),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredicateAstPayloadHeaderV2 {
    pub root_node_id: u32,
    pub node_count: u32,
    pub literal_count: u32,
    pub literal_list_count: u32,
    pub function_count: u32,
    pub operand_ref_count: u32,
    pub node_offset: u64,
    pub literal_offset: u64,
    pub literal_list_offset: u64,
    pub function_offset: u64,
    pub operand_ref_offset: u64,
    pub flags: u32,
    pub checksum: u32,
}

impl PredicateAstPayloadHeaderV2 {
    pub const LEN: usize = 72;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let header = Self {
            root_node_id: read_u32(bytes, 0)?,
            node_count: read_u32(bytes, 4)?,
            literal_count: read_u32(bytes, 8)?,
            literal_list_count: read_u32(bytes, 12)?,
            function_count: read_u32(bytes, 16)?,
            operand_ref_count: read_u32(bytes, 20)?,
            node_offset: read_u64(bytes, 24)?,
            literal_offset: read_u64(bytes, 32)?,
            literal_list_offset: read_u64(bytes, 40)?,
            function_offset: read_u64(bytes, 48)?,
            operand_ref_offset: read_u64(bytes, 56)?,
            flags: read_u32(bytes, 64)?,
            checksum: read_u32(bytes, 68)?,
        };
        verify_crc(&bytes[..72], 68, header.checksum)?;
        Ok(header)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredicateAstOperandRefV2 {
    pub parent_node_id: u32,
    pub ordinal: u16,
    pub operand_kind: PredicateOperandKindV2,
    pub flags: u8,
    pub ref_id: u32,
    pub checksum: u32,
}

impl PredicateAstOperandRefV2 {
    pub const LEN: usize = 16;
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let item = Self {
            parent_node_id: read_u32(bytes, 0)?,
            ordinal: read_u16(bytes, 4)?,
            operand_kind: PredicateOperandKindV2::from_u8(read_u8(bytes, 6)?)
                .ok_or(CoveError::BadCoverage)?,
            flags: read_u8(bytes, 7)?,
            ref_id: read_u32(bytes, 8)?,
            checksum: read_u32(bytes, 12)?,
        };
        verify_crc(&bytes[..Self::LEN], 12, item.checksum)?;
        Ok(item)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredicateAstNodeV2 {
    pub node_id: u32,
    pub op: PredicateOpV2,
    pub flags: u16,
    pub result_logical_type: u16,
    pub collation_id: u16,
    pub null_policy: PredicateNullPolicyV2,
    pub reserved0: u8,
    pub operand_count: u16,
    pub first_operand_index: u32,
    pub column_or_path_ref: u32,
    pub literal_ref: u32,
    pub function_ref: u32,
    pub aux_ref: u32,
    pub checksum: u32,
}

impl PredicateAstNodeV2 {
    pub const LEN: usize = 40;
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let item = Self {
            node_id: read_u32(bytes, 0)?,
            op: PredicateOpV2::from_u16(read_u16(bytes, 4)?).ok_or(CoveError::BadCoverage)?,
            flags: read_u16(bytes, 6)?,
            result_logical_type: read_u16(bytes, 8)?,
            collation_id: read_u16(bytes, 10)?,
            null_policy: PredicateNullPolicyV2::from_u8(read_u8(bytes, 12)?)
                .ok_or(CoveError::BadCoverage)?,
            reserved0: read_u8(bytes, 13)?,
            operand_count: read_u16(bytes, 14)?,
            first_operand_index: read_u32(bytes, 16)?,
            column_or_path_ref: read_u32(bytes, 20)?,
            literal_ref: read_u32(bytes, 24)?,
            function_ref: read_u32(bytes, 28)?,
            aux_ref: read_u32(bytes, 32)?,
            checksum: read_u32(bytes, 36)?,
        };
        verify_crc(&bytes[..Self::LEN], 36, item.checksum)?;
        if item.reserved0 != 0 {
            return Err(CoveError::BadCoverage);
        }
        Ok(item)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredicateLiteralV2 {
    pub literal_id: u32,
    pub value_tag: u16,
    pub logical_type: u16,
    pub flags: u32,
    pub canonical_value_offset: u64,
    pub canonical_value_length: u32,
    pub checksum: u32,
}

impl PredicateLiteralV2 {
    pub const LEN: usize = 28;
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let item = Self {
            literal_id: read_u32(bytes, 0)?,
            value_tag: read_u16(bytes, 4)?,
            logical_type: read_u16(bytes, 6)?,
            flags: read_u32(bytes, 8)?,
            canonical_value_offset: read_u64(bytes, 12)?,
            canonical_value_length: read_u32(bytes, 20)?,
            checksum: read_u32(bytes, 24)?,
        };
        verify_crc(&bytes[..Self::LEN], 24, item.checksum)?;
        checked_end(
            item.canonical_value_offset,
            u64::from(item.canonical_value_length),
        )?;
        Ok(item)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredicateLiteralListV2 {
    pub literal_list_id: u32,
    pub first_literal_index: u32,
    pub literal_count: u32,
    pub flags: u32,
    pub checksum: u32,
}

impl PredicateLiteralListV2 {
    pub const LEN: usize = 20;
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let item = Self {
            literal_list_id: read_u32(bytes, 0)?,
            first_literal_index: read_u32(bytes, 4)?,
            literal_count: read_u32(bytes, 8)?,
            flags: read_u32(bytes, 12)?,
            checksum: read_u32(bytes, 16)?,
        };
        verify_crc(&bytes[..Self::LEN], 16, item.checksum)?;
        Ok(item)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredicateFunctionRefV2 {
    pub function_ref: u32,
    pub namespace: String,
    pub name: String,
    pub version_major: u16,
    pub version_minor: u16,
    pub deterministic: u8,
    pub flags: u8,
    pub required_extension_ref: u32,
    pub checksum: u32,
}

impl PredicateFunctionRefV2 {
    pub fn parse_one(bytes: &[u8]) -> Result<(Self, usize), CoveError> {
        let mut cursor = Cursor::new(bytes);
        let function_ref = cursor.u32()?;
        let namespace_len = cursor.u16()? as usize;
        let namespace = cursor.string(namespace_len)?;
        let name_len = cursor.u16()? as usize;
        let name = cursor.string(name_len)?;
        let version_major = cursor.u16()?;
        let version_minor = cursor.u16()?;
        let deterministic = cursor.u8()?;
        let flags = cursor.u8()?;
        let required_extension_ref = cursor.u32()?;
        let checksum_offset = cursor.position;
        let checksum = cursor.u32()?;
        let consumed = cursor.position;
        verify_crc(&bytes[..consumed], checksum_offset, checksum)?;
        if deterministic > 1 {
            return Err(CoveError::BadCoverage);
        }
        Ok((
            Self {
                function_ref,
                namespace,
                name,
                version_major,
                version_minor,
                deterministic,
                flags,
                required_extension_ref,
                checksum,
            },
            consumed,
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredicateAstPayloadV2 {
    pub header: PredicateAstPayloadHeaderV2,
    pub nodes: Vec<PredicateAstNodeV2>,
    pub literals: Vec<PredicateLiteralV2>,
    pub literal_lists: Vec<PredicateLiteralListV2>,
    pub functions: Vec<PredicateFunctionRefV2>,
    pub operand_refs: Vec<PredicateAstOperandRefV2>,
}

impl PredicateAstPayloadV2 {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let header = PredicateAstPayloadHeaderV2::parse(bytes)?;
        let nodes = parse_fixed_table(
            bytes,
            header.node_offset,
            header.node_count,
            PredicateAstNodeV2::LEN,
            PredicateAstNodeV2::parse,
        )?;
        let literals = parse_fixed_table(
            bytes,
            header.literal_offset,
            header.literal_count,
            PredicateLiteralV2::LEN,
            PredicateLiteralV2::parse,
        )?;
        let literal_lists = parse_fixed_table(
            bytes,
            header.literal_list_offset,
            header.literal_list_count,
            PredicateLiteralListV2::LEN,
            PredicateLiteralListV2::parse,
        )?;
        let operand_refs = parse_fixed_table(
            bytes,
            header.operand_ref_offset,
            header.operand_ref_count,
            PredicateAstOperandRefV2::LEN,
            PredicateAstOperandRefV2::parse,
        )?;
        let functions = parse_function_table(bytes, header.function_offset, header.function_count)?;
        let payload = Self {
            header,
            nodes,
            literals,
            literal_lists,
            functions,
            operand_refs,
        };
        payload.validate()?;
        Ok(payload)
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        validate_unique_dense_ids(&self.nodes, |node| node.node_id)?;
        validate_unique_dense_ids(&self.literals, |literal| literal.literal_id)?;
        validate_unique_dense_ids(&self.literal_lists, |list| list.literal_list_id)?;
        validate_unique_dense_ids(&self.functions, |function| function.function_ref)?;
        if self.header.root_node_id as usize >= self.nodes.len() {
            return Err(CoveError::BadCoverage);
        }
        for node in &self.nodes {
            validate_node_operands(node, &self.operand_refs)?;
            validate_node_arity(node)?;
        }
        validate_ast_acyclic(self.header.root_node_id, &self.nodes, &self.operand_refs)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PredicateNormalisationKindV2 {
    Cnf = 0,
    Dnf = 1,
    FlatConjunction = 2,
    FlatDisjunction = 3,
}

impl PredicateNormalisationKindV2 {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Cnf),
            1 => Some(Self::Dnf),
            2 => Some(Self::FlatConjunction),
            3 => Some(Self::FlatDisjunction),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredicateNormalisedPayloadHeaderV2 {
    pub normalisation_kind: PredicateNormalisationKindV2,
    pub flags: u8,
    pub reserved: u16,
    pub clause_count: u32,
    pub term_count: u32,
    pub clause_offset: u64,
    pub term_offset: u64,
    pub checksum: u32,
}

impl PredicateNormalisedPayloadHeaderV2 {
    pub const LEN: usize = 32;
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let item = Self {
            normalisation_kind: PredicateNormalisationKindV2::from_u8(read_u8(bytes, 0)?)
                .ok_or(CoveError::BadCoverage)?,
            flags: read_u8(bytes, 1)?,
            reserved: read_u16(bytes, 2)?,
            clause_count: read_u32(bytes, 4)?,
            term_count: read_u32(bytes, 8)?,
            clause_offset: read_u64(bytes, 12)?,
            term_offset: read_u64(bytes, 20)?,
            checksum: read_u32(bytes, 28)?,
        };
        verify_crc(&bytes[..Self::LEN], 28, item.checksum)?;
        if item.reserved != 0 {
            return Err(CoveError::BadCoverage);
        }
        Ok(item)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredicateClauseEntryV2 {
    pub clause_id: u32,
    pub first_term_index: u32,
    pub term_count: u32,
    pub flags: u32,
    pub checksum: u32,
}

impl PredicateClauseEntryV2 {
    pub const LEN: usize = 20;
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let item = Self {
            clause_id: read_u32(bytes, 0)?,
            first_term_index: read_u32(bytes, 4)?,
            term_count: read_u32(bytes, 8)?,
            flags: read_u32(bytes, 12)?,
            checksum: read_u32(bytes, 16)?,
        };
        verify_crc(&bytes[..Self::LEN], 16, item.checksum)?;
        Ok(item)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredicateTermV2 {
    pub term_id: u32,
    pub ast_node_ref: u32,
    pub negated: u8,
    pub null_policy: PredicateNullPolicyV2,
    pub proof_safe: u8,
    pub reserved: u8,
    pub checksum: u32,
}

impl PredicateTermV2 {
    pub const LEN: usize = 16;
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let item = Self {
            term_id: read_u32(bytes, 0)?,
            ast_node_ref: read_u32(bytes, 4)?,
            negated: read_u8(bytes, 8)?,
            null_policy: PredicateNullPolicyV2::from_u8(read_u8(bytes, 9)?)
                .ok_or(CoveError::BadCoverage)?,
            proof_safe: read_u8(bytes, 10)?,
            reserved: read_u8(bytes, 11)?,
            checksum: read_u32(bytes, 12)?,
        };
        verify_crc(&bytes[..Self::LEN], 12, item.checksum)?;
        validate_bool(item.negated)?;
        validate_bool(item.proof_safe)?;
        if item.reserved != 0 {
            return Err(CoveError::BadCoverage);
        }
        Ok(item)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredicateNormalisedPayloadV2 {
    pub header: PredicateNormalisedPayloadHeaderV2,
    pub clauses: Vec<PredicateClauseEntryV2>,
    pub terms: Vec<PredicateTermV2>,
}

impl PredicateNormalisedPayloadV2 {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let header = PredicateNormalisedPayloadHeaderV2::parse(bytes)?;
        let clauses = parse_fixed_table(
            bytes,
            header.clause_offset,
            header.clause_count,
            PredicateClauseEntryV2::LEN,
            PredicateClauseEntryV2::parse,
        )?;
        let terms = parse_fixed_table(
            bytes,
            header.term_offset,
            header.term_count,
            PredicateTermV2::LEN,
            PredicateTermV2::parse,
        )?;
        validate_unique_dense_ids(&clauses, |clause| clause.clause_id)?;
        validate_unique_dense_ids(&terms, |term| term.term_id)?;
        for clause in &clauses {
            let end = clause
                .first_term_index
                .checked_add(clause.term_count)
                .ok_or(CoveError::ArithOverflow)?;
            if end as usize > terms.len() {
                return Err(CoveError::BadCoverage);
            }
        }
        Ok(Self {
            header,
            clauses,
            terms,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IntervalNullPolicyV2 {
    NullExcluded = 0,
    NullIncluded = 1,
    SqlUnknown = 2,
    ExtensionDefined = 3,
}

impl IntervalNullPolicyV2 {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::NullExcluded),
            1 => Some(Self::NullIncluded),
            2 => Some(Self::SqlUnknown),
            3 => Some(Self::ExtensionDefined),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IntervalBoundKindV2 {
    LowerUpper = 0,
    Point = 1,
    OpenRange = 2,
    MultiIntervalRef = 3,
}

impl IntervalBoundKindV2 {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::LowerUpper),
            1 => Some(Self::Point),
            2 => Some(Self::OpenRange),
            3 => Some(Self::MultiIntervalRef),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntervalPredicateV2 {
    pub column_or_path_ref: u32,
    pub logical_type: u16,
    pub collation_id: u16,
    pub null_policy: IntervalNullPolicyV2,
    pub bound_kind: IntervalBoundKindV2,
    pub flags: u16,
    pub lower_inclusive: u8,
    pub upper_inclusive: u8,
    pub reserved: u16,
    pub lower_value_ref: u32,
    pub upper_value_ref: u32,
    pub checksum: u32,
}

impl IntervalPredicateV2 {
    pub const LEN: usize = 28;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let interval = Self {
            column_or_path_ref: read_u32(bytes, 0)?,
            logical_type: read_u16(bytes, 4)?,
            collation_id: read_u16(bytes, 6)?,
            null_policy: IntervalNullPolicyV2::from_u8(read_u8(bytes, 8)?)
                .ok_or(CoveError::BadCoverage)?,
            bound_kind: IntervalBoundKindV2::from_u8(read_u8(bytes, 9)?)
                .ok_or(CoveError::BadCoverage)?,
            flags: read_u16(bytes, 10)?,
            lower_inclusive: read_u8(bytes, 12)?,
            upper_inclusive: read_u8(bytes, 13)?,
            reserved: read_u16(bytes, 14)?,
            lower_value_ref: read_u32(bytes, 16)?,
            upper_value_ref: read_u32(bytes, 20)?,
            checksum: read_u32(bytes, 24)?,
        };
        verify_crc(&bytes[..Self::LEN], 24, interval.checksum)?;
        interval.validate()?;
        Ok(interval)
    }

    pub fn parse_many(bytes: &[u8]) -> Result<Vec<Self>, CoveError> {
        if !bytes.len().is_multiple_of(Self::LEN) {
            return Err(CoveError::BadCoverage);
        }
        bytes.chunks_exact(Self::LEN).map(Self::parse).collect()
    }

    pub fn serialize(&self) -> Result<[u8; Self::LEN], CoveError> {
        self.validate()?;
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.column_or_path_ref.to_le_bytes());
        out[4..6].copy_from_slice(&self.logical_type.to_le_bytes());
        out[6..8].copy_from_slice(&self.collation_id.to_le_bytes());
        out[8] = self.null_policy as u8;
        out[9] = self.bound_kind as u8;
        out[10..12].copy_from_slice(&self.flags.to_le_bytes());
        out[12] = self.lower_inclusive;
        out[13] = self.upper_inclusive;
        out[14..16].copy_from_slice(&self.reserved.to_le_bytes());
        out[16..20].copy_from_slice(&self.lower_value_ref.to_le_bytes());
        out[20..24].copy_from_slice(&self.upper_value_ref.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[24..28].copy_from_slice(&crc.to_le_bytes());
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        if self.column_or_path_ref == ABSENT_ID {
            return Err(CoveError::BadCoverage);
        }
        validate_bool(self.lower_inclusive)?;
        validate_bool(self.upper_inclusive)?;
        if self.reserved != 0 {
            return Err(CoveError::BadCoverage);
        }
        match self.bound_kind {
            IntervalBoundKindV2::Point => {
                if self.lower_value_ref == ABSENT_ID || self.lower_value_ref != self.upper_value_ref
                {
                    return Err(CoveError::BadCoverage);
                }
            }
            IntervalBoundKindV2::LowerUpper | IntervalBoundKindV2::OpenRange => {
                if self.lower_value_ref == ABSENT_ID && self.upper_value_ref == ABSENT_ID {
                    return Err(CoveError::BadCoverage);
                }
                if self.lower_value_ref != ABSENT_ID
                    && self.upper_value_ref != ABSENT_ID
                    && self.lower_value_ref > self.upper_value_ref
                {
                    return Err(CoveError::BadCoverage);
                }
            }
            IntervalBoundKindV2::MultiIntervalRef => {
                if self.lower_value_ref == ABSENT_ID || self.upper_value_ref != ABSENT_ID {
                    return Err(CoveError::BadCoverage);
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntervalPredicateSetV2 {
    pub interval_set_id: u32,
    pub column_or_path_ref: u32,
    pub logical_type: u16,
    pub collation_id: u16,
    pub null_policy: IntervalNullPolicyV2,
    pub flags: u8,
    pub interval_count: u32,
    pub intervals_offset: u64,
    pub checksum: u32,
}

impl IntervalPredicateSetV2 {
    pub const LEN: usize = 30;
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let item = Self {
            interval_set_id: read_u32(bytes, 0)?,
            column_or_path_ref: read_u32(bytes, 4)?,
            logical_type: read_u16(bytes, 8)?,
            collation_id: read_u16(bytes, 10)?,
            null_policy: IntervalNullPolicyV2::from_u8(read_u8(bytes, 12)?)
                .ok_or(CoveError::BadCoverage)?,
            flags: read_u8(bytes, 13)?,
            interval_count: read_u32(bytes, 14)?,
            intervals_offset: read_u64(bytes, 18)?,
            checksum: read_u32(bytes, 26)?,
        };
        verify_crc(&bytes[..Self::LEN], 26, item.checksum)?;
        require_present(item.column_or_path_ref)?;
        Ok(item)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct IntervalBoundPairV2 {
    pub lower_value_ref: u32,
    pub upper_value_ref: u32,
    pub lower_inclusive: u8,
    pub upper_inclusive: u8,
    pub flags: u16,
    pub checksum: u32,
}

impl IntervalBoundPairV2 {
    pub const LEN: usize = 16;
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let item = Self {
            lower_value_ref: read_u32(bytes, 0)?,
            upper_value_ref: read_u32(bytes, 4)?,
            lower_inclusive: read_u8(bytes, 8)?,
            upper_inclusive: read_u8(bytes, 9)?,
            flags: read_u16(bytes, 10)?,
            checksum: read_u32(bytes, 12)?,
        };
        verify_crc(&bytes[..Self::LEN], 12, item.checksum)?;
        validate_bool(item.lower_inclusive)?;
        validate_bool(item.upper_inclusive)?;
        if item.lower_value_ref != ABSENT_ID
            && item.upper_value_ref != ABSENT_ID
            && item.lower_value_ref > item.upper_value_ref
        {
            return Err(CoveError::BadCoverage);
        }
        Ok(item)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiIntervalPredicatePayloadV2 {
    pub header: IntervalPredicateSetV2,
    pub intervals: Vec<IntervalBoundPairV2>,
}

impl MultiIntervalPredicatePayloadV2 {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let header = IntervalPredicateSetV2::parse(bytes)?;
        let intervals = parse_fixed_table(
            bytes,
            header.intervals_offset,
            header.interval_count,
            IntervalBoundPairV2::LEN,
            IntervalBoundPairV2::parse,
        )?;
        for pair in intervals.windows(2) {
            if pair[1] <= pair[0] {
                return Err(CoveError::BadCoverage);
            }
        }
        Ok(Self { header, intervals })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedPredicateFormV2 {
    pub encoded_predicate_id: u32,
    pub baseline_predicate_ref: u32,
    pub table_id: u32,
    pub column_id: u32,
    pub logical_type: u16,
    pub physical_kind: u8,
    pub encoding_kind: u16,
    pub codec_id: u32,
    pub flags: u32,
    pub equivalence_kind: u8,
    pub null_semantics: u8,
    pub collation_id: u16,
    pub params_offset: u64,
    pub params_length: u64,
    pub checksum: u32,
}

impl EncodedPredicateFormV2 {
    pub const LEN: usize = 53;
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let item = Self {
            encoded_predicate_id: read_u32(bytes, 0)?,
            baseline_predicate_ref: read_u32(bytes, 4)?,
            table_id: read_u32(bytes, 8)?,
            column_id: read_u32(bytes, 12)?,
            logical_type: read_u16(bytes, 16)?,
            physical_kind: read_u8(bytes, 18)?,
            encoding_kind: read_u16(bytes, 19)?,
            codec_id: read_u32(bytes, 21)?,
            flags: read_u32(bytes, 25)?,
            equivalence_kind: read_u8(bytes, 29)?,
            null_semantics: read_u8(bytes, 30)?,
            collation_id: read_u16(bytes, 31)?,
            params_offset: read_u64(bytes, 33)?,
            params_length: read_u64(bytes, 41)?,
            checksum: read_u32(bytes, 49)?,
        };
        verify_crc(&bytes[..Self::LEN], 49, item.checksum)?;
        if item.baseline_predicate_ref == ABSENT_ID || item.equivalence_kind > 2 {
            return Err(CoveError::BadCoverage);
        }
        validate_null_semantics(item.null_semantics)?;
        checked_end(item.params_offset, item.params_length)?;
        Ok(item)
    }

    pub fn is_pruning_safe(&self) -> bool {
        matches!(self.equivalence_kind, 0 | 1) && self.null_semantics != 255
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CoverageFallbackPolicyV2 {
    AdvisoryOnly = 0,
    FallbackRequired = 1,
    FullScanFallback = 2,
    WiderCoverageFallback = 3,
    RejectIfRequired = 4,
}

impl CoverageFallbackPolicyV2 {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::AdvisoryOnly),
            1 => Some(Self::FallbackRequired),
            2 => Some(Self::FullScanFallback),
            3 => Some(Self::WiderCoverageFallback),
            4 => Some(Self::RejectIfRequired),
            _ => None,
        }
    }
}

pub const COVERAGE_PLAN_FLAG_PRUNING_CANDIDATE: u16 = 1 << 0;
pub const COVERAGE_PLAN_FLAG_MAY_UNDER_INCLUDE: u16 = 1 << 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoveragePlanCandidateV2 {
    pub candidate_id: u32,
    pub predicate_fragment_ref: u32,
    pub provider_id: u32,
    pub provider_type: u16,
    pub flags: u16,
    pub estimated_lookup_cost_ns: u64,
    pub estimated_coverage_size_bytes: u64,
    pub estimated_read_cost_ns: u64,
    pub estimated_decode_cost_ns: u64,
    pub estimated_materialisation_cost_ns: u64,
    pub baseline_scan_cost_estimate_ns: u64,
    pub max_allowed_estimated_cost_ns: u64,
    pub confidence_ppm: u32,
    pub calibration_epoch: u64,
    pub observed_error_bounds_ref: u32,
    pub fallback_policy: CoverageFallbackPolicyV2,
    pub reserved: [u8; 3],
    pub checksum: u32,
}

impl CoveragePlanCandidateV2 {
    pub const LEN: usize = 96;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let candidate = Self {
            candidate_id: read_u32(bytes, 0)?,
            predicate_fragment_ref: read_u32(bytes, 4)?,
            provider_id: read_u32(bytes, 8)?,
            provider_type: read_u16(bytes, 12)?,
            flags: read_u16(bytes, 14)?,
            estimated_lookup_cost_ns: read_u64(bytes, 16)?,
            estimated_coverage_size_bytes: read_u64(bytes, 24)?,
            estimated_read_cost_ns: read_u64(bytes, 32)?,
            estimated_decode_cost_ns: read_u64(bytes, 40)?,
            estimated_materialisation_cost_ns: read_u64(bytes, 48)?,
            baseline_scan_cost_estimate_ns: read_u64(bytes, 56)?,
            max_allowed_estimated_cost_ns: read_u64(bytes, 64)?,
            confidence_ppm: read_u32(bytes, 72)?,
            calibration_epoch: read_u64(bytes, 76)?,
            observed_error_bounds_ref: read_u32(bytes, 84)?,
            fallback_policy: CoverageFallbackPolicyV2::from_u8(read_u8(bytes, 88)?)
                .ok_or(CoveError::BadCoverage)?,
            reserved: read_array(bytes, 89)?,
            checksum: read_u32(bytes, 92)?,
        };
        verify_crc(&bytes[..Self::LEN], 92, candidate.checksum)?;
        candidate.validate()?;
        Ok(candidate)
    }

    pub fn parse_many(bytes: &[u8]) -> Result<Vec<Self>, CoveError> {
        if !bytes.len().is_multiple_of(Self::LEN) {
            return Err(CoveError::BadCoverage);
        }
        let mut ids = BTreeSet::new();
        let mut candidates = Vec::new();
        for chunk in bytes.chunks_exact(Self::LEN) {
            let candidate = Self::parse(chunk)?;
            if !ids.insert(candidate.candidate_id) {
                return Err(CoveError::BadCoverage);
            }
            candidates.push(candidate);
        }
        Ok(candidates)
    }

    pub fn serialize(&self) -> Result<[u8; Self::LEN], CoveError> {
        self.validate()?;
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.candidate_id.to_le_bytes());
        out[4..8].copy_from_slice(&self.predicate_fragment_ref.to_le_bytes());
        out[8..12].copy_from_slice(&self.provider_id.to_le_bytes());
        out[12..14].copy_from_slice(&self.provider_type.to_le_bytes());
        out[14..16].copy_from_slice(&self.flags.to_le_bytes());
        out[16..24].copy_from_slice(&self.estimated_lookup_cost_ns.to_le_bytes());
        out[24..32].copy_from_slice(&self.estimated_coverage_size_bytes.to_le_bytes());
        out[32..40].copy_from_slice(&self.estimated_read_cost_ns.to_le_bytes());
        out[40..48].copy_from_slice(&self.estimated_decode_cost_ns.to_le_bytes());
        out[48..56].copy_from_slice(&self.estimated_materialisation_cost_ns.to_le_bytes());
        out[56..64].copy_from_slice(&self.baseline_scan_cost_estimate_ns.to_le_bytes());
        out[64..72].copy_from_slice(&self.max_allowed_estimated_cost_ns.to_le_bytes());
        out[72..76].copy_from_slice(&self.confidence_ppm.to_le_bytes());
        out[76..84].copy_from_slice(&self.calibration_epoch.to_le_bytes());
        out[84..88].copy_from_slice(&self.observed_error_bounds_ref.to_le_bytes());
        out[88] = self.fallback_policy as u8;
        out[89..92].copy_from_slice(&self.reserved);
        let crc = checksum::crc32c(&out);
        out[92..96].copy_from_slice(&crc.to_le_bytes());
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        if self.predicate_fragment_ref == ABSENT_ID || self.provider_id == ABSENT_ID {
            return Err(CoveError::BadCoverage);
        }
        if self.provider_type == 0 || self.confidence_ppm > 1_000_000 {
            return Err(CoveError::BadCoverage);
        }
        if self.reserved != [0, 0, 0] {
            return Err(CoveError::BadCoverage);
        }
        if self.flags & COVERAGE_PLAN_FLAG_PRUNING_CANDIDATE != 0
            && self.flags & COVERAGE_PLAN_FLAG_MAY_UNDER_INCLUDE != 0
        {
            return Err(CoveError::BadCoverage);
        }
        let estimated = self
            .estimated_lookup_cost_ns
            .checked_add(self.estimated_read_cost_ns)
            .and_then(|value| value.checked_add(self.estimated_decode_cost_ns))
            .and_then(|value| value.checked_add(self.estimated_materialisation_cost_ns))
            .ok_or(CoveError::ArithOverflow)?;
        if self.max_allowed_estimated_cost_ns != 0 && estimated > self.max_allowed_estimated_cost_ns
        {
            return Err(CoveError::BadCoverage);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageProofRecordV2 {
    pub proof_id: u32,
    pub provider_id: u32,
    pub coverage_set_id: u32,
    pub predicate_form_ref: u32,
    pub proof_kind: CoverageProofKindV2,
    pub proof_strength: CoverageProofStrengthV2,
    pub exactness: CoverageExactnessV2,
    pub granularity: CoverageGranularityV2,
    pub null_semantics: u8,
    pub flags: u16,
    pub snapshot_validity_ref: u32,
    pub coverage_set_checksum: u32,
    pub proof_payload_ref: u32,
    pub checksum: u32,
}

impl CoverageProofRecordV2 {
    pub const LEN: usize = 40;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let record = Self {
            proof_id: read_u32(bytes, 0)?,
            provider_id: read_u32(bytes, 4)?,
            coverage_set_id: read_u32(bytes, 8)?,
            predicate_form_ref: read_u32(bytes, 12)?,
            proof_kind: CoverageProofKindV2::from_u16(read_u16(bytes, 16)?)
                .ok_or(CoveError::BadCoverage)?,
            proof_strength: CoverageProofStrengthV2::from_u8(read_u8(bytes, 18)?)
                .ok_or(CoveError::BadCoverage)?,
            exactness: CoverageExactnessV2::from_u8(read_u8(bytes, 19)?)
                .ok_or(CoveError::BadCoverage)?,
            granularity: CoverageGranularityV2::from_u8(read_u8(bytes, 20)?)
                .ok_or(CoveError::BadCoverage)?,
            null_semantics: read_u8(bytes, 21)?,
            flags: read_u16(bytes, 22)?,
            snapshot_validity_ref: read_u32(bytes, 24)?,
            coverage_set_checksum: read_u32(bytes, 28)?,
            proof_payload_ref: read_u32(bytes, 32)?,
            checksum: read_u32(bytes, 36)?,
        };
        verify_crc(&bytes[..Self::LEN], 36, record.checksum)?;
        record.validate()?;
        Ok(record)
    }

    pub fn parse_many(bytes: &[u8]) -> Result<Vec<Self>, CoveError> {
        if !bytes.len().is_multiple_of(Self::LEN) {
            return Err(CoveError::BadCoverage);
        }
        let mut ids = BTreeSet::new();
        let mut records = Vec::new();
        for chunk in bytes.chunks_exact(Self::LEN) {
            let record = Self::parse(chunk)?;
            if !ids.insert(record.proof_id) {
                return Err(CoveError::BadCoverage);
            }
            records.push(record);
        }
        Ok(records)
    }

    pub fn serialize(&self) -> Result<[u8; Self::LEN], CoveError> {
        self.validate()?;
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.proof_id.to_le_bytes());
        out[4..8].copy_from_slice(&self.provider_id.to_le_bytes());
        out[8..12].copy_from_slice(&self.coverage_set_id.to_le_bytes());
        out[12..16].copy_from_slice(&self.predicate_form_ref.to_le_bytes());
        out[16..18].copy_from_slice(&(self.proof_kind as u16).to_le_bytes());
        out[18] = self.proof_strength as u8;
        out[19] = self.exactness as u8;
        out[20] = self.granularity as u8;
        out[21] = self.null_semantics;
        out[22..24].copy_from_slice(&self.flags.to_le_bytes());
        out[24..28].copy_from_slice(&self.snapshot_validity_ref.to_le_bytes());
        out[28..32].copy_from_slice(&self.coverage_set_checksum.to_le_bytes());
        out[32..36].copy_from_slice(&self.proof_payload_ref.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[36..40].copy_from_slice(&crc.to_le_bytes());
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        require_present(self.provider_id)?;
        require_present(self.coverage_set_id)?;
        require_present(self.predicate_form_ref)?;
        require_present(self.snapshot_validity_ref)?;
        validate_null_semantics(self.null_semantics)?;
        if self.exactness.may_under_include() && self.proof_strength.allows_pruning() {
            return Err(CoveError::BadCoverage);
        }
        if self.null_semantics == 255 && self.proof_strength.allows_pruning() {
            return Err(CoveError::BadCoverage);
        }
        Ok(())
    }

    pub fn validate_against_coverage_set_bytes(
        &self,
        coverage_set_bytes: &[u8],
    ) -> Result<(), CoveError> {
        let set = CoverageSetV2::parse(coverage_set_bytes)?;
        self.validate_against_coverage_set(&set, coverage_set_payload_checksum(coverage_set_bytes))
    }

    pub fn validate_against_coverage_set(
        &self,
        set: &CoverageSetV2,
        actual_coverage_set_checksum: u32,
    ) -> Result<(), CoveError> {
        if self.coverage_set_checksum != actual_coverage_set_checksum {
            return Err(CoveError::ChecksumMismatch);
        }
        if self.coverage_set_id != set.header.coverage_set_id
            || self.provider_id != set.header.provider_id
            || self.predicate_form_ref != set.header.predicate_form_ref
            || self.snapshot_validity_ref != set.header.snapshot_validity_ref
            || self.proof_strength != set.header.proof_strength
            || self.exactness != set.header.exactness
            || self.granularity != set.header.granularity
        {
            return Err(CoveError::BadCoverage);
        }
        Ok(())
    }
}

pub fn coverage_set_payload_checksum(bytes: &[u8]) -> u32 {
    checksum::crc32c(bytes)
}

pub fn can_use_proof_for_pruning(record: &CoverageProofRecordV2) -> bool {
    record.proof_strength.allows_pruning()
        && !record.exactness.may_under_include()
        && record.null_semantics != 255
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageProviderDescriptorV2 {
    pub provider_id: u32,
    pub provider_kind: u16,
    pub profile: u8,
    pub granularity: CoverageGranularityV2,
    pub proof_strength: CoverageProofStrengthV2,
    pub exactness: CoverageExactnessV2,
    pub flags: u16,
    pub referenced_table_id: u32,
    pub referenced_column_id: u32,
    pub referenced_path_ref: u32,
    pub logical_type: u16,
    pub collation_id: u16,
    pub null_semantics: u8,
    pub snapshot_validity_ref: u32,
    pub predicate_form_ref: u32,
    pub producer_ref: u32,
    pub checksum: u32,
}

impl CoverageProviderDescriptorV2 {
    pub const LEN: usize = 45;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let provider = Self {
            provider_id: read_u32(bytes, 0)?,
            provider_kind: read_u16(bytes, 4)?,
            profile: read_u8(bytes, 6)?,
            granularity: CoverageGranularityV2::from_u8(read_u8(bytes, 7)?)
                .ok_or(CoveError::BadCoverage)?,
            proof_strength: CoverageProofStrengthV2::from_u8(read_u8(bytes, 8)?)
                .ok_or(CoveError::BadCoverage)?,
            exactness: CoverageExactnessV2::from_u8(read_u8(bytes, 9)?)
                .ok_or(CoveError::BadCoverage)?,
            flags: read_u16(bytes, 10)?,
            referenced_table_id: read_u32(bytes, 12)?,
            referenced_column_id: read_u32(bytes, 16)?,
            referenced_path_ref: read_u32(bytes, 20)?,
            logical_type: read_u16(bytes, 24)?,
            collation_id: read_u16(bytes, 26)?,
            null_semantics: read_u8(bytes, 28)?,
            snapshot_validity_ref: read_u32(bytes, 29)?,
            predicate_form_ref: read_u32(bytes, 33)?,
            producer_ref: read_u32(bytes, 37)?,
            checksum: read_u32(bytes, 41)?,
        };
        verify_crc(&bytes[..Self::LEN], 41, provider.checksum)?;
        provider.validate()?;
        Ok(provider)
    }

    pub fn serialize(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.provider_id.to_le_bytes());
        out[4..6].copy_from_slice(&self.provider_kind.to_le_bytes());
        out[6] = self.profile;
        out[7] = self.granularity as u8;
        out[8] = self.proof_strength as u8;
        out[9] = self.exactness as u8;
        out[10..12].copy_from_slice(&self.flags.to_le_bytes());
        out[12..16].copy_from_slice(&self.referenced_table_id.to_le_bytes());
        out[16..20].copy_from_slice(&self.referenced_column_id.to_le_bytes());
        out[20..24].copy_from_slice(&self.referenced_path_ref.to_le_bytes());
        out[24..26].copy_from_slice(&self.logical_type.to_le_bytes());
        out[26..28].copy_from_slice(&self.collation_id.to_le_bytes());
        out[28] = self.null_semantics;
        out[29..33].copy_from_slice(&self.snapshot_validity_ref.to_le_bytes());
        out[33..37].copy_from_slice(&self.predicate_form_ref.to_le_bytes());
        out[37..41].copy_from_slice(&self.producer_ref.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[41..45].copy_from_slice(&crc.to_le_bytes());
        out
    }

    pub fn parse_many(bytes: &[u8]) -> Result<Vec<Self>, CoveError> {
        if !bytes.len().is_multiple_of(Self::LEN) {
            return Err(CoveError::BadCoverage);
        }
        let mut providers = Vec::new();
        let mut ids = BTreeSet::new();
        for chunk in bytes.chunks_exact(Self::LEN) {
            let provider = Self::parse(chunk)?;
            if !ids.insert(provider.provider_id) {
                return Err(CoveError::BadCoverage);
            }
            providers.push(provider);
        }
        Ok(providers)
    }

    pub fn validate(&self) -> Result<(), CoveError> {
        if self.profile > 11 {
            return Err(CoveError::BadCoverage);
        }
        if self.exactness.may_under_include() && self.proof_strength.allows_pruning() {
            return Err(CoveError::BadCoverage);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageSetHeaderV2 {
    pub coverage_set_id: u32,
    pub provider_id: u32,
    pub granularity: CoverageGranularityV2,
    pub proof_strength: CoverageProofStrengthV2,
    pub exactness: CoverageExactnessV2,
    pub flags: u8,
    pub predicate_form_ref: u32,
    pub snapshot_validity_ref: u32,
    pub total_fragment_count: u64,
    pub covered_fragment_count: u64,
    pub required_fragment_count_estimate: u64,
    pub coverage_degree_ppm: u32,
    pub tightness_degree_ppm: u32,
    pub entries_offset: u64,
    pub entries_length: u64,
    pub checksum: u32,
}

impl CoverageSetHeaderV2 {
    pub const LEN: usize = 72;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let header = Self {
            coverage_set_id: read_u32(bytes, 0)?,
            provider_id: read_u32(bytes, 4)?,
            granularity: CoverageGranularityV2::from_u8(read_u8(bytes, 8)?)
                .ok_or(CoveError::BadCoverage)?,
            proof_strength: CoverageProofStrengthV2::from_u8(read_u8(bytes, 9)?)
                .ok_or(CoveError::BadCoverage)?,
            exactness: CoverageExactnessV2::from_u8(read_u8(bytes, 10)?)
                .ok_or(CoveError::BadCoverage)?,
            flags: read_u8(bytes, 11)?,
            predicate_form_ref: read_u32(bytes, 12)?,
            snapshot_validity_ref: read_u32(bytes, 16)?,
            total_fragment_count: read_u64(bytes, 20)?,
            covered_fragment_count: read_u64(bytes, 28)?,
            required_fragment_count_estimate: read_u64(bytes, 36)?,
            coverage_degree_ppm: read_u32(bytes, 44)?,
            tightness_degree_ppm: read_u32(bytes, 48)?,
            entries_offset: read_u64(bytes, 52)?,
            entries_length: read_u64(bytes, 60)?,
            checksum: read_u32(bytes, 68)?,
        };
        verify_crc(&bytes[..Self::LEN], 68, header.checksum)?;
        if header.coverage_degree_ppm > 1_000_000 || header.tightness_degree_ppm > 1_000_000 {
            return Err(CoveError::BadCoverage);
        }
        if header.exactness.may_under_include() && header.proof_strength.allows_pruning() {
            return Err(CoveError::BadCoverage);
        }
        Ok(header)
    }

    pub fn serialize(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0..4].copy_from_slice(&self.coverage_set_id.to_le_bytes());
        out[4..8].copy_from_slice(&self.provider_id.to_le_bytes());
        out[8] = self.granularity as u8;
        out[9] = self.proof_strength as u8;
        out[10] = self.exactness as u8;
        out[11] = self.flags;
        out[12..16].copy_from_slice(&self.predicate_form_ref.to_le_bytes());
        out[16..20].copy_from_slice(&self.snapshot_validity_ref.to_le_bytes());
        out[20..28].copy_from_slice(&self.total_fragment_count.to_le_bytes());
        out[28..36].copy_from_slice(&self.covered_fragment_count.to_le_bytes());
        out[36..44].copy_from_slice(&self.required_fragment_count_estimate.to_le_bytes());
        out[44..48].copy_from_slice(&self.coverage_degree_ppm.to_le_bytes());
        out[48..52].copy_from_slice(&self.tightness_degree_ppm.to_le_bytes());
        out[52..60].copy_from_slice(&self.entries_offset.to_le_bytes());
        out[60..68].copy_from_slice(&self.entries_length.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[68..72].copy_from_slice(&crc.to_le_bytes());
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CoverageSetEntryV2 {
    pub target_kind: CoverageGranularityV2,
    pub flags: u16,
    pub file_ref: u32,
    pub table_id: u32,
    pub segment_id: u32,
    pub morsel_id: u32,
    pub page_ref: u32,
    pub object_type_id: u32,
    pub path_ref: u32,
    pub dimensional_bucket_ref: u32,
    pub row_start: u64,
    pub row_count: u64,
    pub row_ordinal_bitmap_ref: u32,
    pub byte_range_ref: u32,
    pub checksum: u32,
}

impl CoverageSetEntryV2 {
    pub const LEN: usize = 64;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::LEN {
            return Err(CoveError::BufferTooShort);
        }
        let target_kind =
            CoverageGranularityV2::from_u16(read_u16(bytes, 0)?).ok_or(CoveError::BadCoverage)?;
        let entry = Self {
            target_kind,
            flags: read_u16(bytes, 2)?,
            file_ref: read_u32(bytes, 4)?,
            table_id: read_u32(bytes, 8)?,
            segment_id: read_u32(bytes, 12)?,
            morsel_id: read_u32(bytes, 16)?,
            page_ref: read_u32(bytes, 20)?,
            object_type_id: read_u32(bytes, 24)?,
            path_ref: read_u32(bytes, 28)?,
            dimensional_bucket_ref: read_u32(bytes, 32)?,
            row_start: read_u64(bytes, 36)?,
            row_count: read_u64(bytes, 44)?,
            row_ordinal_bitmap_ref: read_u32(bytes, 52)?,
            byte_range_ref: read_u32(bytes, 56)?,
            checksum: read_u32(bytes, 60)?,
        };
        verify_crc(&bytes[..Self::LEN], 60, entry.checksum)?;
        entry.validate_absent_fields()?;
        Ok(entry)
    }

    pub fn serialize(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0..2].copy_from_slice(&(self.target_kind as u16).to_le_bytes());
        out[2..4].copy_from_slice(&self.flags.to_le_bytes());
        out[4..8].copy_from_slice(&self.file_ref.to_le_bytes());
        out[8..12].copy_from_slice(&self.table_id.to_le_bytes());
        out[12..16].copy_from_slice(&self.segment_id.to_le_bytes());
        out[16..20].copy_from_slice(&self.morsel_id.to_le_bytes());
        out[20..24].copy_from_slice(&self.page_ref.to_le_bytes());
        out[24..28].copy_from_slice(&self.object_type_id.to_le_bytes());
        out[28..32].copy_from_slice(&self.path_ref.to_le_bytes());
        out[32..36].copy_from_slice(&self.dimensional_bucket_ref.to_le_bytes());
        out[36..44].copy_from_slice(&self.row_start.to_le_bytes());
        out[44..52].copy_from_slice(&self.row_count.to_le_bytes());
        out[52..56].copy_from_slice(&self.row_ordinal_bitmap_ref.to_le_bytes());
        out[56..60].copy_from_slice(&self.byte_range_ref.to_le_bytes());
        let crc = checksum::crc32c(&out);
        out[60..64].copy_from_slice(&crc.to_le_bytes());
        out
    }

    fn validate_absent_fields(&self) -> Result<(), CoveError> {
        match self.target_kind {
            CoverageGranularityV2::Dataset => {
                require_absent(self.file_ref)?;
                require_absent(self.table_id)?;
                require_absent(self.segment_id)?;
                require_absent(self.morsel_id)?;
                require_absent(self.page_ref)?;
                require_absent(self.object_type_id)?;
                require_absent(self.path_ref)?;
                require_absent(self.dimensional_bucket_ref)?;
                if self.row_start != 0 || self.row_count != 0 {
                    return Err(CoveError::BadCoverage);
                }
            }
            CoverageGranularityV2::File => require_present(self.file_ref)?,
            CoverageGranularityV2::Segment => {
                require_present(self.file_ref)?;
                require_present(self.table_id)?;
                require_present(self.segment_id)?;
            }
            CoverageGranularityV2::Page => {
                require_present(self.file_ref)?;
                require_present(self.table_id)?;
                require_present(self.segment_id)?;
                require_present(self.page_ref)?;
            }
            CoverageGranularityV2::Morsel => {
                require_present(self.file_ref)?;
                require_present(self.table_id)?;
                require_present(self.segment_id)?;
                require_present(self.morsel_id)?;
            }
            CoverageGranularityV2::RowRange => {
                require_present(self.file_ref)?;
                require_present(self.table_id)?;
                require_present(self.segment_id)?;
                if self.row_count == 0 {
                    return Err(CoveError::BadCoverage);
                }
            }
            CoverageGranularityV2::RowOrdinalSet => {
                require_present(self.file_ref)?;
                require_present(self.table_id)?;
                require_present(self.row_ordinal_bitmap_ref)?;
            }
            CoverageGranularityV2::MapNode | CoverageGranularityV2::ObjectPath => {
                require_present(self.path_ref)?;
            }
            CoverageGranularityV2::DimensionalBucket => {
                require_present(self.dimensional_bucket_ref)?;
            }
            CoverageGranularityV2::Object
            | CoverageGranularityV2::RowGroup
            | CoverageGranularityV2::Association
            | CoverageGranularityV2::ProjectionFragment
            | CoverageGranularityV2::ExternalFragment => {}
        }
        checked_end(self.row_start, self.row_count)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageSetV2 {
    pub header: CoverageSetHeaderV2,
    pub entries: Vec<CoverageSetEntryV2>,
}

impl CoverageSetV2 {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        let header = CoverageSetHeaderV2::parse(bytes)?;
        let start = usize::try_from(header.entries_offset).map_err(|_| CoveError::OffsetRange)?;
        let len = usize::try_from(header.entries_length).map_err(|_| CoveError::OffsetRange)?;
        let end = start.checked_add(len).ok_or(CoveError::ArithOverflow)?;
        if start < CoverageSetHeaderV2::LEN
            || end != bytes.len()
            || len % CoverageSetEntryV2::LEN != 0
        {
            return Err(CoveError::BadCoverage);
        }
        let mut entries = Vec::new();
        for chunk in bytes[start..end].chunks_exact(CoverageSetEntryV2::LEN) {
            entries.push(CoverageSetEntryV2::parse(chunk)?);
        }
        validate_coverage_entries(&entries)?;
        if header.covered_fragment_count != entries.len() as u64 {
            return Err(CoveError::BadCoverage);
        }
        if header.covered_fragment_count > header.total_fragment_count {
            return Err(CoveError::BadCoverage);
        }
        Ok(Self { header, entries })
    }

    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        validate_coverage_entries(&self.entries)?;
        let entries_length = self
            .entries
            .len()
            .checked_mul(CoverageSetEntryV2::LEN)
            .ok_or(CoveError::ArithOverflow)?;
        let covered_fragment_count =
            u64::try_from(self.entries.len()).map_err(|_| CoveError::ArithOverflow)?;
        if covered_fragment_count > self.header.total_fragment_count {
            return Err(CoveError::BadCoverage);
        }

        let mut header = self.header.clone();
        header.covered_fragment_count = covered_fragment_count;
        header.entries_offset = CoverageSetHeaderV2::LEN as u64;
        header.entries_length =
            u64::try_from(entries_length).map_err(|_| CoveError::ArithOverflow)?;

        let total_len = CoverageSetHeaderV2::LEN
            .checked_add(entries_length)
            .ok_or(CoveError::ArithOverflow)?;
        let mut out = Vec::with_capacity(total_len);
        out.extend_from_slice(&header.serialize());
        for entry in &self.entries {
            out.extend_from_slice(&entry.serialize());
        }
        Ok(out)
    }
}

pub fn validate_coverage_entries(entries: &[CoverageSetEntryV2]) -> Result<(), CoveError> {
    let mut prev: Option<&CoverageSetEntryV2> = None;
    let mut by_range_scope: BTreeMap<(u32, u32, u32, u32), (u64, u64)> = BTreeMap::new();
    for entry in entries {
        if let Some(previous) = prev {
            if entry <= previous {
                return Err(CoveError::BadCoverage);
            }
        }
        if entry.target_kind == CoverageGranularityV2::RowRange {
            let scope = (
                entry.file_ref,
                entry.table_id,
                entry.segment_id,
                entry.morsel_id,
            );
            if let Some((start, count)) =
                by_range_scope.insert(scope, (entry.row_start, entry.row_count))
            {
                let previous_end = start.checked_add(count).ok_or(CoveError::ArithOverflow)?;
                if entry.row_start < previous_end {
                    return Err(CoveError::BadCoverage);
                }
            }
        }
        prev = Some(entry);
    }
    Ok(())
}

pub fn coverage_union(
    left: &[CoverageSetEntryV2],
    right: &[CoverageSetEntryV2],
) -> Result<Vec<CoverageSetEntryV2>, CoveError> {
    let mut out = left.to_vec();
    out.extend_from_slice(right);
    out.sort();
    out.dedup();
    coalesce_row_ranges(out)
}

pub fn coverage_intersection(
    left: &[CoverageSetEntryV2],
    right: &[CoverageSetEntryV2],
) -> Result<Vec<CoverageSetEntryV2>, CoveError> {
    let right_set = right.iter().collect::<BTreeSet<_>>();
    let out = left
        .iter()
        .filter(|entry| right_set.contains(entry))
        .cloned()
        .collect::<Vec<_>>();
    validate_coverage_entries(&out)?;
    Ok(out)
}

pub fn coverage_coarsen(
    entries: &[CoverageSetEntryV2],
) -> Result<Vec<CoverageSetEntryV2>, CoveError> {
    coalesce_row_ranges(entries.to_vec())
}

pub fn coverage_refine(
    entries: &[CoverageSetEntryV2],
    refinement: &[CoverageSetEntryV2],
) -> Result<Vec<CoverageSetEntryV2>, CoveError> {
    coverage_intersection(entries, refinement)
}

fn coalesce_row_ranges(
    mut entries: Vec<CoverageSetEntryV2>,
) -> Result<Vec<CoverageSetEntryV2>, CoveError> {
    entries.sort();
    let mut out: Vec<CoverageSetEntryV2> = Vec::new();
    for entry in entries {
        if let Some(last) = out.last_mut() {
            if last.target_kind == CoverageGranularityV2::RowRange
                && entry.target_kind == CoverageGranularityV2::RowRange
                && last.file_ref == entry.file_ref
                && last.table_id == entry.table_id
                && last.segment_id == entry.segment_id
                && last.morsel_id == entry.morsel_id
            {
                let last_end = last
                    .row_start
                    .checked_add(last.row_count)
                    .ok_or(CoveError::ArithOverflow)?;
                if entry.row_start <= last_end {
                    let entry_end = entry
                        .row_start
                        .checked_add(entry.row_count)
                        .ok_or(CoveError::ArithOverflow)?;
                    last.row_count = entry_end.saturating_sub(last.row_start).max(last.row_count);
                    continue;
                }
            }
        }
        out.push(entry);
    }
    validate_coverage_entries(&out)?;
    Ok(out)
}

pub fn can_use_for_pruning(header: &CoverageSetHeaderV2) -> bool {
    header.proof_strength.allows_pruning() && !header.exactness.may_under_include()
}

fn parse_fixed_table<T, F>(
    bytes: &[u8],
    offset: u64,
    count: u32,
    item_len: usize,
    parse: F,
) -> Result<Vec<T>, CoveError>
where
    F: Fn(&[u8]) -> Result<T, CoveError>,
{
    if count == 0 {
        if offset != 0 {
            return Err(CoveError::BadCoverage);
        }
        return Ok(Vec::new());
    }
    let start = usize::try_from(offset).map_err(|_| CoveError::OffsetRange)?;
    let len = usize::try_from(count)
        .map_err(|_| CoveError::ArithOverflow)?
        .checked_mul(item_len)
        .ok_or(CoveError::ArithOverflow)?;
    let end = start.checked_add(len).ok_or(CoveError::ArithOverflow)?;
    if end > bytes.len() {
        return Err(CoveError::OffsetRange);
    }
    bytes[start..end]
        .chunks_exact(item_len)
        .map(parse)
        .collect()
}

fn parse_function_table(
    bytes: &[u8],
    offset: u64,
    count: u32,
) -> Result<Vec<PredicateFunctionRefV2>, CoveError> {
    if count == 0 {
        if offset != 0 {
            return Err(CoveError::BadCoverage);
        }
        return Ok(Vec::new());
    }
    let mut cursor = usize::try_from(offset).map_err(|_| CoveError::OffsetRange)?;
    let mut out = Vec::new();
    for _ in 0..count {
        let (item, consumed) = PredicateFunctionRefV2::parse_one(&bytes[cursor..])?;
        if consumed == 0 {
            return Err(CoveError::BadCoverage);
        }
        out.push(item);
        cursor = cursor
            .checked_add(consumed)
            .ok_or(CoveError::ArithOverflow)?;
        if cursor > bytes.len() {
            return Err(CoveError::OffsetRange);
        }
    }
    Ok(out)
}

fn validate_unique_dense_ids<T, F>(items: &[T], id: F) -> Result<(), CoveError>
where
    F: Fn(&T) -> u32,
{
    let mut seen = BTreeSet::new();
    for (index, item) in items.iter().enumerate() {
        let item_id = id(item);
        if item_id as usize != index || !seen.insert(item_id) {
            return Err(CoveError::BadCoverage);
        }
    }
    Ok(())
}

fn validate_node_operands(
    node: &PredicateAstNodeV2,
    operands: &[PredicateAstOperandRefV2],
) -> Result<(), CoveError> {
    if node.operand_count == 0 {
        if node.first_operand_index != ABSENT_ID {
            return Err(CoveError::BadCoverage);
        }
        return Ok(());
    }
    if node.first_operand_index == ABSENT_ID {
        return Err(CoveError::BadCoverage);
    }
    let start = node.first_operand_index as usize;
    let end = start
        .checked_add(node.operand_count as usize)
        .ok_or(CoveError::ArithOverflow)?;
    if end > operands.len() {
        return Err(CoveError::BadCoverage);
    }
    for (expected, operand) in operands[start..end].iter().enumerate() {
        if operand.parent_node_id != node.node_id || operand.ordinal as usize != expected {
            return Err(CoveError::BadCoverage);
        }
    }
    validate_mirror(
        node,
        operands,
        PredicateOperandKindV2::ColumnOrPath,
        node.column_or_path_ref,
    )?;
    validate_mirror(
        node,
        operands,
        PredicateOperandKindV2::Literal,
        node.literal_ref,
    )?;
    validate_mirror(
        node,
        operands,
        PredicateOperandKindV2::Function,
        node.function_ref,
    )?;
    Ok(())
}

fn validate_mirror(
    node: &PredicateAstNodeV2,
    operands: &[PredicateAstOperandRefV2],
    kind: PredicateOperandKindV2,
    mirror_ref: u32,
) -> Result<(), CoveError> {
    if mirror_ref == ABSENT_ID {
        return Ok(());
    }
    let start = node.first_operand_index as usize;
    let end = start + node.operand_count as usize;
    if operands[start..end]
        .iter()
        .any(|operand| operand.operand_kind == kind && operand.ref_id == mirror_ref)
    {
        Ok(())
    } else {
        Err(CoveError::BadCoverage)
    }
}

fn validate_node_arity(node: &PredicateAstNodeV2) -> Result<(), CoveError> {
    let count = node.operand_count;
    let valid = match node.op {
        PredicateOpV2::TrueLiteral | PredicateOpV2::FalseLiteral => count == 0,
        PredicateOpV2::LiteralValue | PredicateOpV2::ColumnRef => count == 1,
        PredicateOpV2::IsNull
        | PredicateOpV2::IsNotNull
        | PredicateOpV2::IsNaN
        | PredicateOpV2::IsFinite
        | PredicateOpV2::Not => count == 1,
        PredicateOpV2::Eq
        | PredicateOpV2::NotEq
        | PredicateOpV2::Lt
        | PredicateOpV2::LtEq
        | PredicateOpV2::Gt
        | PredicateOpV2::GtEq
        | PredicateOpV2::LikePrefix
        | PredicateOpV2::Contains
        | PredicateOpV2::InSet => count == 2,
        PredicateOpV2::Between => count == 3,
        PredicateOpV2::And | PredicateOpV2::Or => count >= 2,
        PredicateOpV2::FunctionCall => count >= 1,
        PredicateOpV2::Extension => false,
    };
    if valid {
        Ok(())
    } else {
        Err(CoveError::BadCoverage)
    }
}

fn validate_ast_acyclic(
    root_node_id: u32,
    nodes: &[PredicateAstNodeV2],
    operands: &[PredicateAstOperandRefV2],
) -> Result<(), CoveError> {
    fn visit(
        node_id: u32,
        nodes: &[PredicateAstNodeV2],
        operands: &[PredicateAstOperandRefV2],
        visiting: &mut BTreeSet<u32>,
        visited: &mut BTreeSet<u32>,
    ) -> Result<(), CoveError> {
        if visited.contains(&node_id) {
            return Ok(());
        }
        if !visiting.insert(node_id) {
            return Err(CoveError::BadCoverage);
        }
        let node = nodes.get(node_id as usize).ok_or(CoveError::BadCoverage)?;
        if node.node_id != node_id {
            return Err(CoveError::BadCoverage);
        }
        if node.operand_count > 0 {
            let start = node.first_operand_index as usize;
            let end = start + node.operand_count as usize;
            for operand in &operands[start..end] {
                if operand.operand_kind == PredicateOperandKindV2::Node {
                    visit(operand.ref_id, nodes, operands, visiting, visited)?;
                }
            }
        }
        visiting.remove(&node_id);
        visited.insert(node_id);
        Ok(())
    }

    visit(
        root_node_id,
        nodes,
        operands,
        &mut BTreeSet::new(),
        &mut BTreeSet::new(),
    )
}

struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn bytes(&mut self, len: usize) -> Result<&'a [u8], CoveError> {
        let end = self
            .position
            .checked_add(len)
            .ok_or(CoveError::ArithOverflow)?;
        if end > self.bytes.len() {
            return Err(CoveError::BufferTooShort);
        }
        let out = &self.bytes[self.position..end];
        self.position = end;
        Ok(out)
    }

    fn string(&mut self, len: usize) -> Result<String, CoveError> {
        std::str::from_utf8(self.bytes(len)?)
            .map(|value| value.to_string())
            .map_err(|_| CoveError::BadCoverage)
    }

    fn u8(&mut self) -> Result<u8, CoveError> {
        Ok(self.bytes(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, CoveError> {
        Ok(u16::from_le_bytes(self.bytes(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32, CoveError> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
}

fn require_absent(value: u32) -> Result<(), CoveError> {
    if value == ABSENT_ID {
        Ok(())
    } else {
        Err(CoveError::BadCoverage)
    }
}

fn require_present(value: u32) -> Result<(), CoveError> {
    if value == ABSENT_ID {
        Err(CoveError::BadCoverage)
    } else {
        Ok(())
    }
}

fn verify_crc(bytes: &[u8], checksum_offset: usize, expected: u32) -> Result<(), CoveError> {
    let mut check = bytes.to_vec();
    check[checksum_offset..checksum_offset + 4].fill(0);
    if checksum::crc32c(&check) != expected {
        return Err(CoveError::ChecksumMismatch);
    }
    Ok(())
}

fn read_u8(bytes: &[u8], offset: usize) -> Result<u8, CoveError> {
    if offset >= bytes.len() {
        return Err(CoveError::BufferTooShort);
    }
    Ok(bytes[offset])
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, CoveError> {
    Ok(u16::from_le_bytes(read_array(bytes, offset)?))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, CoveError> {
    Ok(u32::from_le_bytes(read_array(bytes, offset)?))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, CoveError> {
    Ok(u64::from_le_bytes(read_array(bytes, offset)?))
}

fn read_array<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N], CoveError> {
    let end = offset.checked_add(N).ok_or(CoveError::ArithOverflow)?;
    if end > bytes.len() {
        return Err(CoveError::BufferTooShort);
    }
    Ok(bytes[offset..end].try_into().unwrap())
}

fn checked_end(offset: u64, length: u64) -> Result<u64, CoveError> {
    offset.checked_add(length).ok_or(CoveError::ArithOverflow)
}

fn validate_bool(value: u8) -> Result<(), CoveError> {
    match value {
        0 | 1 => Ok(()),
        _ => Err(CoveError::BadCoverage),
    }
}

fn validate_null_semantics(value: u8) -> Result<(), CoveError> {
    match value {
        0..=4 | 255 => Ok(()),
        _ => Err(CoveError::BadCoverage),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn predicate_form(id: u32) -> PredicateNormalFormV2 {
        PredicateNormalFormV2 {
            predicate_form_id: id,
            form_kind: PredicateFormKindV2::IntervalPredicateForm,
            flags: 0,
            logical_context_ref: 1,
            payload_offset: 0,
            payload_length: 0,
            checksum: 0,
        }
    }

    fn interval() -> IntervalPredicateV2 {
        IntervalPredicateV2 {
            column_or_path_ref: 1,
            logical_type: 7,
            collation_id: 0,
            null_policy: IntervalNullPolicyV2::SqlUnknown,
            bound_kind: IntervalBoundKindV2::LowerUpper,
            flags: 0,
            lower_inclusive: 1,
            upper_inclusive: 1,
            reserved: 0,
            lower_value_ref: 1,
            upper_value_ref: 2,
            checksum: 0,
        }
    }

    fn plan_candidate(id: u32) -> CoveragePlanCandidateV2 {
        CoveragePlanCandidateV2 {
            candidate_id: id,
            predicate_fragment_ref: 1,
            provider_id: 1,
            provider_type: 1,
            flags: COVERAGE_PLAN_FLAG_PRUNING_CANDIDATE,
            estimated_lookup_cost_ns: 10,
            estimated_coverage_size_bytes: 1024,
            estimated_read_cost_ns: 20,
            estimated_decode_cost_ns: 30,
            estimated_materialisation_cost_ns: 40,
            baseline_scan_cost_estimate_ns: 1000,
            max_allowed_estimated_cost_ns: 200,
            confidence_ppm: 900_000,
            calibration_epoch: 1,
            observed_error_bounds_ref: ABSENT_ID,
            fallback_policy: CoverageFallbackPolicyV2::FullScanFallback,
            reserved: [0; 3],
            checksum: 0,
        }
    }

    fn coverage_set_payload_with_predicate() -> Vec<u8> {
        let entry = dataset_entry();
        let header = CoverageSetHeaderV2 {
            coverage_set_id: 1,
            provider_id: 1,
            granularity: CoverageGranularityV2::Dataset,
            proof_strength: CoverageProofStrengthV2::ExactConservative,
            exactness: CoverageExactnessV2::Exact,
            flags: 0,
            predicate_form_ref: 1,
            snapshot_validity_ref: 1,
            total_fragment_count: 1,
            covered_fragment_count: 1,
            required_fragment_count_estimate: 1,
            coverage_degree_ppm: 1_000_000,
            tightness_degree_ppm: 1_000_000,
            entries_offset: CoverageSetHeaderV2::LEN as u64,
            entries_length: CoverageSetEntryV2::LEN as u64,
            checksum: 0,
        };
        let mut bytes = header.serialize().to_vec();
        bytes.extend_from_slice(&entry.serialize());
        bytes
    }

    fn proof_record(id: u32, coverage_set_bytes: &[u8]) -> CoverageProofRecordV2 {
        CoverageProofRecordV2 {
            proof_id: id,
            provider_id: 1,
            coverage_set_id: 1,
            predicate_form_ref: 1,
            proof_kind: CoverageProofKindV2::ZoneMap,
            proof_strength: CoverageProofStrengthV2::ExactConservative,
            exactness: CoverageExactnessV2::Exact,
            granularity: CoverageGranularityV2::Dataset,
            null_semantics: 0,
            flags: 0,
            snapshot_validity_ref: 1,
            coverage_set_checksum: coverage_set_payload_checksum(coverage_set_bytes),
            proof_payload_ref: ABSENT_ID,
            checksum: 0,
        }
    }

    #[test]
    fn predicate_form_round_trips_and_rejects_bad_context_ref() {
        let bytes = predicate_form(1).serialize().unwrap();
        assert_eq!(
            PredicateNormalFormV2::parse(&bytes)
                .unwrap()
                .predicate_form_id,
            1
        );
        let mut form = predicate_form(2);
        form.logical_context_ref = ABSENT_ID;
        assert!(matches!(form.serialize(), Err(CoveError::BadCoverage)));
    }

    #[test]
    fn predicate_form_with_payload_preserves_payload_and_flat_compatibility() {
        let flat = predicate_form(1).serialize().unwrap();
        let parsed = PredicateNormalFormWithPayloadV2::parse_many(&flat).unwrap();
        assert_eq!(parsed.len(), 1);
        assert!(parsed[0].payload.is_empty());

        let payload = b"payload".to_vec();
        let mut form = predicate_form(2);
        form.form_kind = PredicateFormKindV2::PredicateAst;
        form.payload_offset = PredicateNormalFormV2::LEN as u64;
        form.payload_length = payload.len() as u64;
        let mut bytes = form.serialize().unwrap().to_vec();
        bytes.extend_from_slice(&payload);

        let parsed = PredicateNormalFormWithPayloadV2::parse_many(&bytes).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].form.predicate_form_id, 2);
        assert_eq!(parsed[0].payload, payload);
        assert!(PredicateNormalFormV2::parse_many(&bytes).is_err());
    }

    #[test]
    fn interval_predicate_round_trips_and_rejects_bad_bounds() {
        let bytes = interval().serialize().unwrap();
        assert_eq!(
            IntervalPredicateV2::parse(&bytes).unwrap().upper_value_ref,
            2
        );
        let mut bad = interval();
        bad.lower_value_ref = 9;
        bad.upper_value_ref = 2;
        assert!(matches!(bad.serialize(), Err(CoveError::BadCoverage)));
    }

    #[test]
    fn coverage_plan_candidates_round_trip_and_reject_unsafe_pruning() {
        let mut bytes = plan_candidate(1).serialize().unwrap().to_vec();
        bytes.extend_from_slice(&plan_candidate(2).serialize().unwrap());
        assert_eq!(
            CoveragePlanCandidateV2::parse_many(&bytes).unwrap().len(),
            2
        );

        let mut bad = plan_candidate(3);
        bad.flags |= COVERAGE_PLAN_FLAG_MAY_UNDER_INCLUDE;
        assert!(matches!(bad.serialize(), Err(CoveError::BadCoverage)));
    }

    #[test]
    fn coverage_proof_records_round_trip_and_validate_set_binding() {
        let set_bytes = coverage_set_payload_with_predicate();
        let record = proof_record(1, &set_bytes);
        let bytes = record.serialize().unwrap();
        let parsed = CoverageProofRecordV2::parse(&bytes).unwrap();
        parsed
            .validate_against_coverage_set_bytes(&set_bytes)
            .unwrap();
        assert!(can_use_proof_for_pruning(&parsed));
    }

    #[test]
    fn coverage_proof_records_reject_duplicates_and_bad_set_checksum() {
        let set_bytes = coverage_set_payload_with_predicate();
        let mut bytes = proof_record(1, &set_bytes).serialize().unwrap().to_vec();
        bytes.extend_from_slice(&proof_record(1, &set_bytes).serialize().unwrap());
        assert!(matches!(
            CoverageProofRecordV2::parse_many(&bytes),
            Err(CoveError::BadCoverage)
        ));

        let mut bad = proof_record(2, &set_bytes);
        bad.coverage_set_checksum ^= 1;
        let parsed = CoverageProofRecordV2::parse(&bad.serialize().unwrap()).unwrap();
        assert!(matches!(
            parsed.validate_against_coverage_set_bytes(&set_bytes),
            Err(CoveError::ChecksumMismatch)
        ));
    }

    #[test]
    fn coverage_proof_records_reject_underinclusive_pruning_and_unsafe_nulls() {
        let set_bytes = coverage_set_payload_with_predicate();
        let mut underinclusive = proof_record(1, &set_bytes);
        underinclusive.exactness = CoverageExactnessV2::ApproximateMayUnderInclude;
        assert!(matches!(
            underinclusive.serialize(),
            Err(CoveError::BadCoverage)
        ));

        let mut unsafe_nulls = proof_record(2, &set_bytes);
        unsafe_nulls.null_semantics = 255;
        assert!(matches!(
            unsafe_nulls.serialize(),
            Err(CoveError::BadCoverage)
        ));
    }

    fn dataset_entry() -> CoverageSetEntryV2 {
        CoverageSetEntryV2 {
            target_kind: CoverageGranularityV2::Dataset,
            flags: 0,
            file_ref: ABSENT_ID,
            table_id: ABSENT_ID,
            segment_id: ABSENT_ID,
            morsel_id: ABSENT_ID,
            page_ref: ABSENT_ID,
            object_type_id: ABSENT_ID,
            path_ref: ABSENT_ID,
            dimensional_bucket_ref: ABSENT_ID,
            row_start: 0,
            row_count: 0,
            row_ordinal_bitmap_ref: ABSENT_ID,
            byte_range_ref: ABSENT_ID,
            checksum: 0,
        }
    }

    #[test]
    fn coverage_set_round_trips() {
        let entry = dataset_entry();
        let header = CoverageSetHeaderV2 {
            coverage_set_id: 1,
            provider_id: 1,
            granularity: CoverageGranularityV2::Dataset,
            proof_strength: CoverageProofStrengthV2::ExactConservative,
            exactness: CoverageExactnessV2::Exact,
            flags: 0,
            predicate_form_ref: ABSENT_ID,
            snapshot_validity_ref: 1,
            total_fragment_count: 1,
            covered_fragment_count: 1,
            required_fragment_count_estimate: 1,
            coverage_degree_ppm: 1_000_000,
            tightness_degree_ppm: 1_000_000,
            entries_offset: CoverageSetHeaderV2::LEN as u64,
            entries_length: CoverageSetEntryV2::LEN as u64,
            checksum: 0,
        };
        let mut bytes = header.serialize().to_vec();
        bytes.extend_from_slice(&entry.serialize());
        let parsed = CoverageSetV2::parse(&bytes).unwrap();
        assert!(can_use_for_pruning(&parsed.header));
        assert_eq!(parsed.entries.len(), 1);
    }

    #[test]
    fn under_inclusive_pruning_is_rejected() {
        let provider = CoverageProviderDescriptorV2 {
            provider_id: 1,
            provider_kind: 1,
            profile: 2,
            granularity: CoverageGranularityV2::Page,
            proof_strength: CoverageProofStrengthV2::ExactConservative,
            exactness: CoverageExactnessV2::ApproximateMayUnderInclude,
            flags: 0,
            referenced_table_id: 1,
            referenced_column_id: 1,
            referenced_path_ref: ABSENT_ID,
            logical_type: 0,
            collation_id: 0,
            null_semantics: 0,
            snapshot_validity_ref: 1,
            predicate_form_ref: ABSENT_ID,
            producer_ref: ABSENT_ID,
            checksum: 0,
        };
        assert!(matches!(provider.validate(), Err(CoveError::BadCoverage)));
    }

    #[test]
    fn provider_registry_rejects_duplicate_provider_ids() {
        let provider = CoverageProviderDescriptorV2 {
            provider_id: 1,
            provider_kind: 1,
            profile: 2,
            granularity: CoverageGranularityV2::Page,
            proof_strength: CoverageProofStrengthV2::ExactConservative,
            exactness: CoverageExactnessV2::Exact,
            flags: 0,
            referenced_table_id: 1,
            referenced_column_id: 1,
            referenced_path_ref: ABSENT_ID,
            logical_type: 0,
            collation_id: 0,
            null_semantics: 0,
            snapshot_validity_ref: 1,
            predicate_form_ref: ABSENT_ID,
            producer_ref: ABSENT_ID,
            checksum: 0,
        };
        let mut bytes = provider.serialize().to_vec();
        bytes.extend_from_slice(&provider.serialize());
        assert!(matches!(
            CoverageProviderDescriptorV2::parse_many(&bytes),
            Err(CoveError::BadCoverage)
        ));
    }
}
