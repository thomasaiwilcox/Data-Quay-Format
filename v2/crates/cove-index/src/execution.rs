use std::collections::BTreeMap;

use cove_core::{constants::DigestAlgorithm, CoveError};
use cove_coverage::CoverageProofStrengthV2;

use crate::{
    CoviAggregateAnswerBlockV2, CoviAggregateAnswerV2, CoviArtifactV2, CoviByteRangePostingV2,
    CoviDimensionalBucketPostingV2, CoviEntryBlockV2, CoviFileRefPostingV2, CoviIndexEntryV2,
    CoviIndexKindV2, CoviIndexRootV2, CoviIndexedTargetKindV2, CoviKeyBlockV2,
    CoviKeyEncodingKindV2, CoviMorselRefPostingV2, CoviObjectPathPostingV2, CoviPageRefPostingV2,
    CoviPostingRepresentationV2, CoviPostingsBlockV2, CoviReferencedFileV2, CoviRowRangePostingV2,
    CoviSectionKindV2, CoviSegmentRefPostingV2, CoviSnapshotValidityV2, IndexCapabilityExactnessV2,
    IndexCapabilityV2,
};

const ABSENT_U32: u32 = u32::MAX;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoviFileDigestV2 {
    pub algorithm: DigestAlgorithm,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoviValidationContextV2 {
    pub file_id: [u8; 16],
    pub file_len: u64,
    pub footer_crc32c: u32,
    pub file_digest: Option<CoviFileDigestV2>,
    pub dataset_id: Option<[u8; 16]>,
    pub snapshot_id: Option<[u8; 16]>,
    pub schema_fingerprint_ref: Option<u32>,
    pub semantic_map_fingerprint_ref: Option<u32>,
    pub external_visibility_ref: Option<u32>,
    pub now_us: Option<i64>,
    pub allow_file_code_keys: bool,
    pub require_exact: bool,
}

impl CoviValidationContextV2 {
    pub fn for_file(file_id: [u8; 16], file_len: u64, footer_crc32c: u32) -> Self {
        Self {
            file_id,
            file_len,
            footer_crc32c,
            file_digest: None,
            dataset_id: None,
            snapshot_id: None,
            schema_fingerprint_ref: None,
            semantic_map_fingerprint_ref: None,
            external_visibility_ref: None,
            now_us: None,
            allow_file_code_keys: false,
            require_exact: true,
        }
    }

    pub fn with_dataset_id(mut self, dataset_id: [u8; 16]) -> Self {
        self.dataset_id = Some(dataset_id);
        self
    }

    pub fn with_snapshot_id(mut self, snapshot_id: [u8; 16]) -> Self {
        self.snapshot_id = Some(snapshot_id);
        self
    }

    pub fn with_schema_fingerprint_ref(mut self, schema_fingerprint_ref: u32) -> Self {
        self.schema_fingerprint_ref = Some(schema_fingerprint_ref);
        self
    }

    pub fn with_semantic_map_fingerprint_ref(mut self, semantic_map_fingerprint_ref: u32) -> Self {
        self.semantic_map_fingerprint_ref = Some(semantic_map_fingerprint_ref);
        self
    }

    pub fn with_external_visibility_ref(mut self, external_visibility_ref: u32) -> Self {
        self.external_visibility_ref = Some(external_visibility_ref);
        self
    }

    pub fn with_file_digest(mut self, algorithm: DigestAlgorithm, bytes: Vec<u8>) -> Self {
        self.file_digest = Some(CoviFileDigestV2 { algorithm, bytes });
        self
    }

    pub fn with_now_us(mut self, now_us: i64) -> Self {
        self.now_us = Some(now_us);
        self
    }

    pub fn with_file_code_keys(mut self, allowed: bool) -> Self {
        self.allow_file_code_keys = allowed;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCoviArtifactV2 {
    artifact: CoviArtifactV2,
    host_file_ref: u32,
    roots: BTreeMap<u32, CoviIndexRootV2>,
    capabilities: BTreeMap<u32, IndexCapabilityV2>,
    snapshot_validity: BTreeMap<u32, CoviSnapshotValidityV2>,
    key_blocks: BTreeMap<u32, CoviKeyBlockV2>,
    entry_blocks: BTreeMap<u32, CoviEntryBlockV2>,
    postings_blocks: BTreeMap<u32, CoviPostingsBlockV2>,
    aggregate_blocks: BTreeMap<u32, CoviAggregateAnswerBlockV2>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoviLookupOpV2 {
    Eq,
    Range {
        lower_inclusive: bool,
        upper_inclusive: bool,
    },
    Membership,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoviLookupKeyV2 {
    CanonicalValueBytes(Vec<u8>),
    FileCode(u32),
    CanonicalHash {
        hash64: u64,
        canonical_value_bytes: Vec<u8>,
    },
}

impl CoviLookupKeyV2 {
    fn key_bytes(&self) -> Vec<u8> {
        match self {
            Self::CanonicalValueBytes(bytes) => bytes.clone(),
            Self::FileCode(code) => code.to_le_bytes().to_vec(),
            Self::CanonicalHash {
                canonical_value_bytes,
                ..
            } => canonical_value_bytes.clone(),
        }
    }

    fn hash64(&self) -> Option<u64> {
        match self {
            Self::CanonicalHash { hash64, .. } => Some(*hash64),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoviLookupTargetV2 {
    TableColumn {
        table_id: u32,
        column_id: u32,
    },
    ObjectProperty {
        object_type_id: u32,
        property_id: u32,
    },
    ObjectPath {
        object_type_id: u32,
        path_ref: u32,
    },
    SemanticDimension {
        semantic_dimension_ref: u32,
    },
    DimensionalTuple {
        semantic_dimension_ref: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoviLookupRequestV2 {
    pub table_id: u32,
    pub column_id: u32,
    pub target: CoviLookupTargetV2,
    pub op: CoviLookupOpV2,
    pub lower_key: CoviLookupKeyV2,
    pub upper_key: Option<CoviLookupKeyV2>,
    pub membership_keys: Vec<CoviLookupKeyV2>,
    pub require_exact: bool,
}

impl CoviLookupRequestV2 {
    pub fn eq(table_id: u32, column_id: u32, key: CoviLookupKeyV2) -> Self {
        Self {
            table_id,
            column_id,
            target: CoviLookupTargetV2::TableColumn {
                table_id,
                column_id,
            },
            op: CoviLookupOpV2::Eq,
            lower_key: key,
            upper_key: None,
            membership_keys: Vec::new(),
            require_exact: true,
        }
    }

    pub fn eq_target(target: CoviLookupTargetV2, key: CoviLookupKeyV2) -> Self {
        let (table_id, column_id) = match target {
            CoviLookupTargetV2::TableColumn {
                table_id,
                column_id,
            } => (table_id, column_id),
            _ => (ABSENT_U32, ABSENT_U32),
        };
        Self {
            table_id,
            column_id,
            target,
            op: CoviLookupOpV2::Eq,
            lower_key: key,
            upper_key: None,
            membership_keys: Vec::new(),
            require_exact: true,
        }
    }

    pub fn membership(
        table_id: u32,
        column_id: u32,
        keys: impl IntoIterator<Item = CoviLookupKeyV2>,
    ) -> Self {
        Self::membership_target(
            CoviLookupTargetV2::TableColumn {
                table_id,
                column_id,
            },
            keys,
        )
    }

    pub fn membership_target(
        target: CoviLookupTargetV2,
        keys: impl IntoIterator<Item = CoviLookupKeyV2>,
    ) -> Self {
        let mut keys = keys.into_iter().collect::<Vec<_>>();
        let lower_key = keys
            .first()
            .cloned()
            .unwrap_or_else(|| CoviLookupKeyV2::CanonicalValueBytes(Vec::new()));
        if !keys.is_empty() {
            keys.remove(0);
        }
        let (table_id, column_id) = match target {
            CoviLookupTargetV2::TableColumn {
                table_id,
                column_id,
            } => (table_id, column_id),
            _ => (ABSENT_U32, ABSENT_U32),
        };
        Self {
            table_id,
            column_id,
            target,
            op: CoviLookupOpV2::Membership,
            lower_key,
            upper_key: None,
            membership_keys: keys,
            require_exact: true,
        }
    }

    pub fn table_column_membership(
        table_id: u32,
        column_id: u32,
        keys: impl IntoIterator<Item = CoviLookupKeyV2>,
    ) -> Self {
        Self::membership(table_id, column_id, keys)
    }

    pub fn object_property_membership(
        object_type_id: u32,
        property_id: u32,
        keys: impl IntoIterator<Item = CoviLookupKeyV2>,
    ) -> Self {
        Self::membership_target(
            CoviLookupTargetV2::ObjectProperty {
                object_type_id,
                property_id,
            },
            keys,
        )
    }

    pub fn object_path_membership(
        object_type_id: u32,
        path_ref: u32,
        keys: impl IntoIterator<Item = CoviLookupKeyV2>,
    ) -> Self {
        Self::membership_target(
            CoviLookupTargetV2::ObjectPath {
                object_type_id,
                path_ref,
            },
            keys,
        )
    }

    pub fn semantic_dimension_membership(
        semantic_dimension_ref: u32,
        keys: impl IntoIterator<Item = CoviLookupKeyV2>,
    ) -> Self {
        Self::membership_target(
            CoviLookupTargetV2::SemanticDimension {
                semantic_dimension_ref,
            },
            keys,
        )
    }

    pub fn dimensional_tuple_membership(
        semantic_dimension_ref: u32,
        keys: impl IntoIterator<Item = CoviLookupKeyV2>,
    ) -> Self {
        Self::membership_target(
            CoviLookupTargetV2::DimensionalTuple {
                semantic_dimension_ref,
            },
            keys,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoviCandidateSetV2 {
    pub exactness: IndexCapabilityExactnessV2,
    pub proof_strength: CoverageProofStrengthV2,
    pub row_ranges: Vec<CoviRowRangePostingV2>,
    pub byte_ranges: Vec<CoviByteRangePostingV2>,
    pub object_paths: Vec<CoviObjectPathPostingV2>,
    pub dimensional_buckets: Vec<CoviDimensionalBucketPostingV2>,
    pub row_ordinal_set_refs: Vec<u32>,
    pub file_refs: Vec<CoviFileRefPostingV2>,
    pub segment_refs: Vec<CoviSegmentRefPostingV2>,
    pub morsel_refs: Vec<CoviMorselRefPostingV2>,
    pub page_refs: Vec<CoviPageRefPostingV2>,
}

impl CoviCandidateSetV2 {
    pub fn is_empty(&self) -> bool {
        self.row_ranges.is_empty()
            && self.byte_ranges.is_empty()
            && self.object_paths.is_empty()
            && self.dimensional_buckets.is_empty()
            && self.row_ordinal_set_refs.is_empty()
            && self.file_refs.is_empty()
            && self.segment_refs.is_empty()
            && self.morsel_refs.is_empty()
            && self.page_refs.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum CoviAggregateKindV2 {
    Count = 0,
    Min = 1,
    Max = 2,
    Exists = 3,
    DistinctCount = 4,
    Membership = 5,
}

impl CoviAggregateKindV2 {
    pub fn from_u16(value: u16) -> Option<Self> {
        match value {
            0 => Some(Self::Count),
            1 => Some(Self::Min),
            2 => Some(Self::Max),
            3 => Some(Self::Exists),
            4 => Some(Self::DistinctCount),
            5 => Some(Self::Membership),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoviIndexOnlyRequestV2 {
    pub table_id: u32,
    pub column_id: Option<u32>,
    pub aggregate_kind: CoviAggregateKindV2,
    pub predicate_form_ref: Option<u32>,
    pub require_exact: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoviIndexOnlyAnswerV2 {
    pub aggregate_kind: CoviAggregateKindV2,
    pub row_count: u64,
    pub null_count: u64,
    pub non_null_count: u64,
    pub value: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoviMembershipAnswerV2 {
    pub exactness: IndexCapabilityExactnessV2,
    pub proof_strength: CoverageProofStrengthV2,
    pub requested_key_count: usize,
    pub present_key_count: usize,
    pub present_keys: Vec<Vec<u8>>,
}

impl ValidatedCoviArtifactV2 {
    pub fn parse_and_validate(
        bytes: &[u8],
        context: CoviValidationContextV2,
    ) -> Result<Self, CoveError> {
        let artifact = CoviArtifactV2::parse(bytes)?;
        Self::validate_with_artifact_bytes(artifact, Some(bytes), context)
    }

    pub fn validate(
        artifact: CoviArtifactV2,
        context: CoviValidationContextV2,
    ) -> Result<Self, CoveError> {
        Self::validate_with_artifact_bytes(artifact, None, context)
    }

    fn validate_with_artifact_bytes(
        artifact: CoviArtifactV2,
        artifact_bytes: Option<&[u8]>,
        context: CoviValidationContextV2,
    ) -> Result<Self, CoveError> {
        let host_file_ref = validate_referenced_file(&artifact, artifact_bytes, &context)?.file_ref;
        let snapshot_validity = artifact
            .snapshot_validity
            .iter()
            .map(|entry| (entry.snapshot_validity_ref, entry.clone()))
            .collect::<BTreeMap<_, _>>();
        for entry in snapshot_validity.values() {
            validate_snapshot(entry, &context)?;
        }

        let mut roots = BTreeMap::new();
        for root in &artifact.index_roots {
            if roots.insert(root.index_root_id, root.clone()).is_some() {
                return Err(CoveError::BadCovi);
            }
            validate_snapshot_ref(root.snapshot_validity_ref, &snapshot_validity)?;
        }
        let mut capabilities = BTreeMap::new();
        for capability in &artifact.capabilities {
            if capabilities
                .insert(capability.index_root_id, capability.clone())
                .is_some()
            {
                return Err(CoveError::BadCovi);
            }
            if !roots.contains_key(&capability.index_root_id) {
                return Err(CoveError::BadCovi);
            }
            validate_snapshot_ref(capability.snapshot_validity_ref, &snapshot_validity)?;
        }

        let mut key_blocks = BTreeMap::new();
        let mut entry_blocks = BTreeMap::new();
        let mut postings_blocks = BTreeMap::new();
        let mut aggregate_blocks = BTreeMap::new();
        let mut key_index = 0usize;
        let mut entry_index = 0usize;
        let mut postings_index = 0usize;
        let mut aggregate_index = 0usize;
        for section in &artifact.sections {
            match section.section_kind {
                CoviSectionKindV2::KeyBlock => {
                    let block = artifact
                        .key_blocks
                        .get(key_index)
                        .ok_or(CoveError::BadCovi)?;
                    key_blocks.insert(section.section_id, block.clone());
                    key_index += 1;
                }
                CoviSectionKindV2::EntryBlock => {
                    let block = artifact
                        .entry_blocks
                        .get(entry_index)
                        .ok_or(CoveError::BadCovi)?;
                    entry_blocks.insert(section.section_id, block.clone());
                    entry_index += 1;
                }
                CoviSectionKindV2::PostingsBlock => {
                    let block = artifact
                        .postings_blocks
                        .get(postings_index)
                        .ok_or(CoveError::BadCovi)?;
                    postings_blocks.insert(section.section_id, block.clone());
                    postings_index += 1;
                }
                CoviSectionKindV2::AggregateAnswerBlock => {
                    let block = artifact
                        .aggregate_answer_blocks
                        .get(aggregate_index)
                        .ok_or(CoveError::BadCovi)?;
                    aggregate_blocks.insert(section.section_id, block.clone());
                    aggregate_index += 1;
                }
                _ => {}
            }
        }

        for root in roots.values() {
            if !key_blocks.contains_key(&root.key_block_section_id)
                || !entry_blocks.contains_key(&root.entry_block_section_id)
                || !postings_blocks.contains_key(&root.postings_block_section_id)
            {
                return Err(CoveError::BadCovi);
            }
            if root.aggregate_block_section_id != ABSENT_U32
                && !aggregate_blocks.contains_key(&root.aggregate_block_section_id)
            {
                return Err(CoveError::BadCovi);
            }
            if root.key_encoding_kind == CoviKeyEncodingKindV2::FileCode as u8
                && !context.allow_file_code_keys
            {
                return Err(CoveError::BadCovi);
            }
            validate_root_blocks(
                root,
                &artifact.referenced_files,
                &snapshot_validity,
                &capabilities,
                &key_blocks,
                &entry_blocks,
                &postings_blocks,
                &aggregate_blocks,
            )?;
        }

        Ok(Self {
            artifact,
            host_file_ref,
            roots,
            capabilities,
            snapshot_validity,
            key_blocks,
            entry_blocks,
            postings_blocks,
            aggregate_blocks,
        })
    }

    pub fn artifact(&self) -> &CoviArtifactV2 {
        &self.artifact
    }

    pub fn lookup(&self, request: &CoviLookupRequestV2) -> Result<CoviCandidateSetV2, CoveError> {
        let root = self.lookup_root(request)?;
        let capability = self
            .capabilities
            .get(&root.index_root_id)
            .ok_or(CoveError::BadCovi)?;
        if request.require_exact && capability.exactness != IndexCapabilityExactnessV2::Exact {
            return Err(CoveError::BadCovi);
        }
        match request.op {
            CoviLookupOpV2::Eq if capability.supports_eq == 0 => return Err(CoveError::BadCovi),
            CoviLookupOpV2::Range { .. } if capability.supports_range == 0 => {
                return Err(CoveError::BadCovi)
            }
            CoviLookupOpV2::Membership if capability.supports_membership == 0 => {
                return Err(CoveError::BadCovi)
            }
            _ => {}
        }
        if matches!(request.op, CoviLookupOpV2::Membership)
            && request.membership_keys.is_empty()
            && matches!(&request.lower_key, CoviLookupKeyV2::CanonicalValueBytes(bytes) if bytes.is_empty())
        {
            return Err(CoveError::BadCovi);
        }

        let key_block = self
            .key_blocks
            .get(&root.key_block_section_id)
            .ok_or(CoveError::BadCovi)?;
        let entry_block = self
            .entry_blocks
            .get(&root.entry_block_section_id)
            .ok_or(CoveError::BadCovi)?;
        let postings_block = self
            .postings_blocks
            .get(&root.postings_block_section_id)
            .ok_or(CoveError::BadCovi)?;
        let lower = request.lower_key.key_bytes();
        let upper = request.upper_key.as_ref().map(CoviLookupKeyV2::key_bytes);
        let membership_keys = membership_key_bytes(request);
        let mut rows = Vec::new();
        let mut byte_ranges = Vec::new();
        let mut object_paths = Vec::new();
        let mut dimensional_buckets = Vec::new();
        let mut row_ordinal_set_refs = Vec::new();
        let mut file_refs = Vec::new();
        let mut segment_refs = Vec::new();
        let mut morsel_refs = Vec::new();
        let mut page_refs = Vec::new();
        for entry in &entry_block.entries {
            if entry.index_root_id != root.index_root_id {
                continue;
            }
            let key = key_bytes_for_entry(key_block, entry)?;
            if let Some(hash) = request.lower_key.hash64() {
                if entry.key_hash64 != hash && key == lower.as_slice() {
                    return Err(CoveError::BadCovi);
                }
            }
            if !key_matches(&request.op, key, &lower, upper.as_deref(), &membership_keys) {
                continue;
            }
            let posting = postings_block
                .postings
                .get(entry.postings_ref as usize)
                .ok_or(CoveError::BadCovi)?;
            let payload = postings_block.posting_payload(posting)?;
            match posting.representation {
                CoviPostingRepresentationV2::RowRangeList => {
                    rows.extend(crate::parse_covi_row_range_postings(payload)?);
                }
                CoviPostingRepresentationV2::ByteRangeList => {
                    byte_ranges.extend(parse_fixed_payload(
                        payload,
                        CoviByteRangePostingV2::LEN,
                        CoviByteRangePostingV2::parse,
                    )?);
                }
                CoviPostingRepresentationV2::ObjectPathRefs => {
                    object_paths.extend(parse_fixed_payload(
                        payload,
                        CoviObjectPathPostingV2::LEN,
                        CoviObjectPathPostingV2::parse,
                    )?);
                }
                CoviPostingRepresentationV2::DimensionalBucketRefs => {
                    dimensional_buckets.extend(parse_fixed_payload(
                        payload,
                        CoviDimensionalBucketPostingV2::LEN,
                        CoviDimensionalBucketPostingV2::parse,
                    )?);
                }
                CoviPostingRepresentationV2::RowOrdinalBitmap
                | CoviPostingRepresentationV2::RowOrdinalDeltaVarint => {
                    row_ordinal_set_refs.extend(parse_u32_refs(payload)?);
                }
                CoviPostingRepresentationV2::SortedFileRefs => {
                    file_refs.extend(parse_fixed_payload(
                        payload,
                        CoviFileRefPostingV2::LEN,
                        CoviFileRefPostingV2::parse,
                    )?);
                }
                CoviPostingRepresentationV2::SortedSegmentRefs => {
                    segment_refs.extend(parse_fixed_payload(
                        payload,
                        CoviSegmentRefPostingV2::LEN,
                        CoviSegmentRefPostingV2::parse,
                    )?);
                }
                CoviPostingRepresentationV2::SortedMorselRefs => {
                    morsel_refs.extend(parse_fixed_payload(
                        payload,
                        CoviMorselRefPostingV2::LEN,
                        CoviMorselRefPostingV2::parse,
                    )?);
                }
                CoviPostingRepresentationV2::SortedPageRefs => {
                    page_refs.extend(parse_fixed_payload(
                        payload,
                        CoviPageRefPostingV2::LEN,
                        CoviPageRefPostingV2::parse,
                    )?);
                }
                CoviPostingRepresentationV2::CoverageSetRef
                | CoviPostingRepresentationV2::Extension => return Err(CoveError::BadCovi),
            }
        }
        normalize_row_ranges(&mut rows)?;
        byte_ranges.sort();
        byte_ranges.dedup();
        object_paths.sort();
        object_paths.dedup();
        dimensional_buckets.sort();
        dimensional_buckets.dedup();
        row_ordinal_set_refs.sort_unstable();
        row_ordinal_set_refs.dedup();
        file_refs.sort();
        file_refs.dedup();
        segment_refs.sort();
        segment_refs.dedup();
        morsel_refs.sort();
        morsel_refs.dedup();
        page_refs.sort();
        page_refs.dedup();
        Ok(CoviCandidateSetV2 {
            exactness: capability.exactness,
            proof_strength: capability.proof_strength,
            row_ranges: rows,
            byte_ranges,
            object_paths,
            dimensional_buckets,
            row_ordinal_set_refs,
            file_refs,
            segment_refs,
            morsel_refs,
            page_refs,
        })
    }

    pub fn exact_membership_answer(
        &self,
        request: &CoviLookupRequestV2,
    ) -> Result<CoviMembershipAnswerV2, CoveError> {
        if request.op != CoviLookupOpV2::Membership || !request.require_exact {
            return Err(CoveError::BadCovi);
        }
        let root = self.lookup_root(request)?;
        let capability = self
            .capabilities
            .get(&root.index_root_id)
            .ok_or(CoveError::BadCovi)?;
        if capability.exactness != IndexCapabilityExactnessV2::Exact
            || capability.supports_membership == 0
        {
            return Err(CoveError::BadCovi);
        }
        let key_block = self
            .key_blocks
            .get(&root.key_block_section_id)
            .ok_or(CoveError::BadCovi)?;
        let entry_block = self
            .entry_blocks
            .get(&root.entry_block_section_id)
            .ok_or(CoveError::BadCovi)?;
        let requested = membership_key_bytes(request);
        if requested.is_empty() {
            return Err(CoveError::BadCovi);
        }
        let requested_set = requested
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let mut present = Vec::new();
        for entry in &entry_block.entries {
            if entry.index_root_id != root.index_root_id {
                continue;
            }
            let key = key_bytes_for_entry(key_block, entry)?;
            if requested_set.contains(key) {
                present.push(key.to_vec());
            }
        }
        present.sort();
        present.dedup();
        Ok(CoviMembershipAnswerV2 {
            exactness: capability.exactness,
            proof_strength: capability.proof_strength,
            requested_key_count: requested.len(),
            present_key_count: present.len(),
            present_keys: present,
        })
    }

    pub fn index_only_answer(
        &self,
        request: &CoviIndexOnlyRequestV2,
    ) -> Result<Option<CoviIndexOnlyAnswerV2>, CoveError> {
        let Some(root) = self.roots.values().find(|root| {
            root.table_id == request.table_id
                && request
                    .column_id
                    .map(|column_id| root.column_id == column_id)
                    .unwrap_or(true)
        }) else {
            return Ok(None);
        };
        let Some(capability) = self.capabilities.get(&root.index_root_id) else {
            return Ok(None);
        };
        if capability.supports_index_only == 0 {
            return Ok(None);
        }
        if request.require_exact && capability.exactness != IndexCapabilityExactnessV2::Exact {
            return Err(CoveError::BadCovi);
        }
        if root.aggregate_block_section_id == ABSENT_U32 {
            return Ok(None);
        }
        let Some(block) = self.aggregate_blocks.get(&root.aggregate_block_section_id) else {
            return Ok(None);
        };
        let Some(answer) = block.answers.iter().find(|answer| {
            answer.index_root_id == root.index_root_id
                && CoviAggregateKindV2::from_u16(answer.aggregate_kind)
                    == Some(request.aggregate_kind)
                && request
                    .predicate_form_ref
                    .map(|predicate| answer.predicate_form_ref == predicate)
                    .unwrap_or(answer.predicate_form_ref == ABSENT_U32)
        }) else {
            return Ok(None);
        };
        if request.require_exact && answer.exactness != IndexCapabilityExactnessV2::Exact as u8 {
            return Err(CoveError::BadCovi);
        }
        Ok(Some(answer_to_public(answer, block)?))
    }

    fn lookup_root(&self, request: &CoviLookupRequestV2) -> Result<&CoviIndexRootV2, CoveError> {
        self.roots
            .values()
            .find(|root| root_matches_target(root, request.target))
            .ok_or(CoveError::BadCovi)
    }
}

fn root_matches_target(root: &CoviIndexRootV2, target: CoviLookupTargetV2) -> bool {
    match target {
        CoviLookupTargetV2::TableColumn {
            table_id,
            column_id,
        } => {
            root.indexed_target_kind == CoviIndexedTargetKindV2::TableColumn
                && root.table_id == table_id
                && root.column_id == column_id
        }
        CoviLookupTargetV2::ObjectProperty {
            object_type_id,
            property_id,
        } => {
            root.indexed_target_kind == CoviIndexedTargetKindV2::ObjectProperty
                && root.object_type_id == object_type_id
                && root.property_id == property_id
        }
        CoviLookupTargetV2::ObjectPath {
            object_type_id,
            path_ref,
        } => {
            root.indexed_target_kind == CoviIndexedTargetKindV2::ObjectPath
                && root.object_type_id == object_type_id
                && root.path_ref == path_ref
        }
        CoviLookupTargetV2::SemanticDimension {
            semantic_dimension_ref,
        } => {
            root.indexed_target_kind == CoviIndexedTargetKindV2::SemanticDimension
                && root.semantic_dimension_ref == semantic_dimension_ref
        }
        CoviLookupTargetV2::DimensionalTuple {
            semantic_dimension_ref,
        } => {
            root.indexed_target_kind == CoviIndexedTargetKindV2::DimensionalTuple
                && root.semantic_dimension_ref == semantic_dimension_ref
        }
    }
}

fn validate_referenced_file<'a>(
    artifact: &'a CoviArtifactV2,
    artifact_bytes: Option<&[u8]>,
    context: &CoviValidationContextV2,
) -> Result<&'a CoviReferencedFileV2, CoveError> {
    let file = artifact
        .referenced_files
        .iter()
        .find(|file| {
            file.file_id == context.file_id
                && file.file_len == context.file_len
                && file.footer_crc32c == context.footer_crc32c
        })
        .ok_or(CoveError::BadCovi)?;
    if let Some(expected) = &context.file_digest {
        let declared_algorithm =
            DigestAlgorithm::from_u16(file.digest_algorithm).ok_or(CoveError::BadCovi)?;
        if declared_algorithm != DigestAlgorithm::None {
            if declared_algorithm != expected.algorithm {
                return Err(CoveError::DigestMismatch);
            }
            let Some(bytes) = artifact_bytes else {
                return Err(CoveError::BadCovi);
            };
            if artifact.header.string_table_section_ref == ABSENT_U32 {
                return Err(CoveError::BadCovi);
            }
            let string_table = artifact
                .section_payload_from_bytes(bytes, artifact.header.string_table_section_ref)?;
            let start = usize::try_from(file.digest_offset).map_err(|_| CoveError::OffsetRange)?;
            let len = usize::from(file.digest_len);
            let end = start.checked_add(len).ok_or(CoveError::ArithOverflow)?;
            if end > string_table.len() {
                return Err(CoveError::OffsetRange);
            }
            if &string_table[start..end] != expected.bytes.as_slice() {
                return Err(CoveError::DigestMismatch);
            }
        }
    }
    Ok(file)
}

fn validate_snapshot(
    snapshot: &CoviSnapshotValidityV2,
    context: &CoviValidationContextV2,
) -> Result<(), CoveError> {
    if let Some(dataset_id) = context.dataset_id {
        if snapshot.dataset_id != dataset_id {
            return Err(CoveError::BadCovi);
        }
    }
    if let Some(snapshot_id) = context.snapshot_id {
        if snapshot.snapshot_id != snapshot_id {
            return Err(CoveError::BadCovi);
        }
    }
    if let Some(schema) = context.schema_fingerprint_ref {
        if snapshot.schema_fingerprint_ref != schema {
            return Err(CoveError::BadCovi);
        }
    }
    if let Some(map) = context.semantic_map_fingerprint_ref {
        if snapshot.semantic_map_fingerprint_ref != map {
            return Err(CoveError::BadCovi);
        }
    }
    if let Some(visibility) = context.external_visibility_ref {
        if snapshot.external_visibility_ref != visibility {
            return Err(CoveError::BadCovi);
        }
    }
    if let Some(now_us) = context.now_us {
        if now_us < snapshot.valid_from_us || now_us >= snapshot.valid_until_us {
            return Err(CoveError::BadCovi);
        }
    }
    Ok(())
}

fn validate_root_blocks(
    root: &CoviIndexRootV2,
    referenced_files: &[CoviReferencedFileV2],
    snapshots: &BTreeMap<u32, CoviSnapshotValidityV2>,
    capabilities: &BTreeMap<u32, IndexCapabilityV2>,
    key_blocks: &BTreeMap<u32, CoviKeyBlockV2>,
    entry_blocks: &BTreeMap<u32, CoviEntryBlockV2>,
    postings_blocks: &BTreeMap<u32, CoviPostingsBlockV2>,
    aggregate_blocks: &BTreeMap<u32, CoviAggregateAnswerBlockV2>,
) -> Result<(), CoveError> {
    let key_block = key_blocks
        .get(&root.key_block_section_id)
        .ok_or(CoveError::BadCovi)?;
    let entry_block = entry_blocks
        .get(&root.entry_block_section_id)
        .ok_or(CoveError::BadCovi)?;
    let postings_block = postings_blocks
        .get(&root.postings_block_section_id)
        .ok_or(CoveError::BadCovi)?;
    let aggregate_block = if root.aggregate_block_section_id == ABSENT_U32 {
        None
    } else {
        Some(
            aggregate_blocks
                .get(&root.aggregate_block_section_id)
                .ok_or(CoveError::BadCovi)?,
        )
    };
    if key_block.header.index_root_id != root.index_root_id
        || entry_block.header.index_root_id != root.index_root_id
        || postings_block.header.index_root_id != root.index_root_id
        || key_block.header.encoding_kind as u8 != root.key_encoding_kind
        || key_block.header.comparator_kind as u16 != root.comparator_kind
        || key_block.header.key_count != entry_block.entries.len() as u64
        || entry_block.header.key_block_id != key_block.header.key_block_id
        || entry_block.header.postings_block_id != postings_block.header.postings_block_id
    {
        return Err(CoveError::BadCovi);
    }
    if let Some(aggregate_block) = aggregate_block {
        if aggregate_block.header.index_root_id != root.index_root_id
            || entry_block.header.aggregate_block_id != aggregate_block.header.aggregate_block_id
        {
            return Err(CoveError::BadCovi);
        }
    } else if entry_block.header.aggregate_block_id != ABSENT_U32 {
        return Err(CoveError::BadCovi);
    }
    let capability = capabilities
        .get(&root.index_root_id)
        .ok_or(CoveError::BadCovi)?;
    if root.capability_ref == ABSENT_U32 || capability.capability_id != root.capability_ref {
        return Err(CoveError::BadCovi);
    }

    validate_entries_for_root(
        root,
        key_block,
        entry_block,
        postings_block,
        aggregate_block,
    )?;
    validate_postings_for_root(root, referenced_files, postings_block)?;
    if let Some(block) = aggregate_block {
        validate_aggregate_block(root, snapshots, block)?;
    }
    Ok(())
}

fn validate_entries_for_root(
    root: &CoviIndexRootV2,
    key_block: &CoviKeyBlockV2,
    entry_block: &CoviEntryBlockV2,
    postings_block: &CoviPostingsBlockV2,
    aggregate_block: Option<&CoviAggregateAnswerBlockV2>,
) -> Result<(), CoveError> {
    let sorted = matches!(
        root.index_kind,
        CoviIndexKindV2::Sorted | CoviIndexKindV2::SparseSorted
    );
    let mut previous_key: Option<Vec<u8>> = None;
    let mut previous_entry: Option<&CoviIndexEntryV2> = None;
    for entry in &entry_block.entries {
        if entry.index_root_id != root.index_root_id
            || entry.key_kind as u8 != root.key_encoding_kind
            || entry.comparator_kind as u16 != root.comparator_kind
        {
            return Err(CoveError::BadCovi);
        }
        if entry.postings_ref != ABSENT_U32
            && entry.postings_ref as usize >= postings_block.postings.len()
        {
            return Err(CoveError::BadCovi);
        }
        if entry.aggregate_answer_ref != ABSENT_U32 {
            let Some(block) = aggregate_block else {
                return Err(CoveError::BadCovi);
            };
            if entry.aggregate_answer_ref as usize >= block.answers.len() {
                return Err(CoveError::BadCovi);
            }
        }
        if entry.coverage_set_ref != ABSENT_U32 && root.coverage_set_ref == ABSENT_U32 {
            return Err(CoveError::BadCovi);
        }
        let key = key_bytes_for_entry(key_block, entry)?.to_vec();
        if sorted {
            if let Some(previous) = previous_key.as_ref() {
                if key < *previous {
                    return Err(CoveError::BadCovi);
                }
                if key == *previous {
                    let Some(previous_entry) = previous_entry else {
                        return Err(CoveError::BadCovi);
                    };
                    let chained = previous_entry.next_duplicate_ref == entry.entry_ref;
                    let shared_posting = previous_entry.postings_ref != ABSENT_U32
                        && previous_entry.postings_ref == entry.postings_ref;
                    if !chained && !shared_posting {
                        return Err(CoveError::BadCovi);
                    }
                }
            }
        }
        previous_key = Some(key);
        previous_entry = Some(entry);
    }
    Ok(())
}

fn validate_postings_for_root(
    root: &CoviIndexRootV2,
    referenced_files: &[CoviReferencedFileV2],
    postings_block: &CoviPostingsBlockV2,
) -> Result<(), CoveError> {
    for posting in &postings_block.postings {
        if posting.index_root_id != root.index_root_id {
            return Err(CoveError::BadCovi);
        }
        let payload = postings_block.posting_payload(posting)?;
        match posting.representation {
            CoviPostingRepresentationV2::SortedFileRefs => {
                for chunk in payload.chunks_exact(crate::CoviFileRefPostingV2::LEN) {
                    let item = crate::CoviFileRefPostingV2::parse(chunk)?;
                    validate_file_ref(item.file_ref, referenced_files)?;
                }
            }
            CoviPostingRepresentationV2::SortedSegmentRefs => {
                for chunk in payload.chunks_exact(crate::CoviSegmentRefPostingV2::LEN) {
                    let item = crate::CoviSegmentRefPostingV2::parse(chunk)?;
                    validate_file_ref(item.file_ref, referenced_files)?;
                }
            }
            CoviPostingRepresentationV2::SortedPageRefs => {
                for chunk in payload.chunks_exact(crate::CoviPageRefPostingV2::LEN) {
                    let item = crate::CoviPageRefPostingV2::parse(chunk)?;
                    validate_file_ref(item.file_ref, referenced_files)?;
                }
            }
            CoviPostingRepresentationV2::SortedMorselRefs => {
                for chunk in payload.chunks_exact(crate::CoviMorselRefPostingV2::LEN) {
                    let item = crate::CoviMorselRefPostingV2::parse(chunk)?;
                    validate_file_ref(item.file_ref, referenced_files)?;
                }
            }
            CoviPostingRepresentationV2::RowRangeList => {
                for row in crate::parse_covi_row_range_postings(payload)? {
                    validate_file_ref(row.file_ref, referenced_files)?;
                }
            }
            CoviPostingRepresentationV2::ByteRangeList => {
                for chunk in payload.chunks_exact(crate::CoviByteRangePostingV2::LEN) {
                    let item = crate::CoviByteRangePostingV2::parse(chunk)?;
                    let file = validate_file_ref(item.file_ref, referenced_files)?;
                    let end = item
                        .offset
                        .checked_add(item.length)
                        .ok_or(CoveError::ArithOverflow)?;
                    if end > file.file_len {
                        return Err(CoveError::BadCovi);
                    }
                }
            }
            CoviPostingRepresentationV2::ObjectPathRefs => {
                for chunk in payload.chunks_exact(crate::CoviObjectPathPostingV2::LEN) {
                    let item = crate::CoviObjectPathPostingV2::parse(chunk)?;
                    validate_file_ref(item.file_ref, referenced_files)?;
                }
            }
            CoviPostingRepresentationV2::DimensionalBucketRefs => {
                for chunk in payload.chunks_exact(crate::CoviDimensionalBucketPostingV2::LEN) {
                    let item = crate::CoviDimensionalBucketPostingV2::parse(chunk)?;
                    validate_file_ref(item.file_ref, referenced_files)?;
                }
            }
            CoviPostingRepresentationV2::RowOrdinalBitmap
            | CoviPostingRepresentationV2::RowOrdinalDeltaVarint
            | CoviPostingRepresentationV2::CoverageSetRef
            | CoviPostingRepresentationV2::Extension => {}
        }
    }
    Ok(())
}

fn validate_file_ref(
    file_ref: u32,
    referenced_files: &[CoviReferencedFileV2],
) -> Result<&CoviReferencedFileV2, CoveError> {
    referenced_files
        .get(file_ref as usize)
        .filter(|file| file.file_ref == file_ref)
        .ok_or(CoveError::BadCovi)
}

fn validate_aggregate_block(
    root: &CoviIndexRootV2,
    snapshots: &BTreeMap<u32, CoviSnapshotValidityV2>,
    block: &CoviAggregateAnswerBlockV2,
) -> Result<(), CoveError> {
    for (index, answer) in block.answers.iter().enumerate() {
        if answer.aggregate_answer_ref as usize != index
            || answer.index_root_id != root.index_root_id
            || CoviAggregateKindV2::from_u16(answer.aggregate_kind).is_none()
            || IndexCapabilityExactnessV2::from_u8(answer.exactness).is_none()
        {
            return Err(CoveError::BadCovi);
        }
        validate_snapshot_ref(answer.snapshot_validity_ref, snapshots)?;
        if answer.value_ref != ABSENT_U32 && answer.value_ref as usize > block.payload.len() {
            return Err(CoveError::OffsetRange);
        }
    }
    Ok(())
}

fn validate_snapshot_ref(
    snapshot_ref: u32,
    snapshots: &BTreeMap<u32, CoviSnapshotValidityV2>,
) -> Result<(), CoveError> {
    if snapshot_ref == ABSENT_U32 || !snapshots.contains_key(&snapshot_ref) {
        return Err(CoveError::BadCovi);
    }
    Ok(())
}

fn key_bytes_for_entry<'a>(
    key_block: &'a CoviKeyBlockV2,
    entry: &CoviIndexEntryV2,
) -> Result<&'a [u8], CoveError> {
    let start = usize::try_from(entry.key_offset).map_err(|_| CoveError::OffsetRange)?;
    let len = usize::try_from(entry.key_length).map_err(|_| CoveError::OffsetRange)?;
    let end = start.checked_add(len).ok_or(CoveError::ArithOverflow)?;
    if end > key_block.key_data.len() {
        return Err(CoveError::OffsetRange);
    }
    Ok(&key_block.key_data[start..end])
}

fn membership_key_bytes(request: &CoviLookupRequestV2) -> Vec<Vec<u8>> {
    let mut keys = Vec::with_capacity(1 + request.membership_keys.len());
    let lower = request.lower_key.key_bytes();
    if !matches!(request.op, CoviLookupOpV2::Membership)
        || !matches!(&request.lower_key, CoviLookupKeyV2::CanonicalValueBytes(bytes) if bytes.is_empty())
    {
        keys.push(lower);
    }
    keys.extend(
        request
            .membership_keys
            .iter()
            .map(CoviLookupKeyV2::key_bytes),
    );
    keys.sort();
    keys.dedup();
    keys
}

fn key_matches(
    op: &CoviLookupOpV2,
    key: &[u8],
    lower: &[u8],
    upper: Option<&[u8]>,
    membership_keys: &[Vec<u8>],
) -> bool {
    match op {
        CoviLookupOpV2::Eq => key == lower,
        CoviLookupOpV2::Membership => membership_keys.iter().any(|candidate| candidate == key),
        CoviLookupOpV2::Range {
            lower_inclusive,
            upper_inclusive,
        } => {
            let lower_ok = if *lower_inclusive {
                key >= lower
            } else {
                key > lower
            };
            let upper_ok = match upper {
                Some(upper) if *upper_inclusive => key <= upper,
                Some(upper) => key < upper,
                None => true,
            };
            lower_ok && upper_ok
        }
    }
}

fn normalize_row_ranges(rows: &mut Vec<CoviRowRangePostingV2>) -> Result<(), CoveError> {
    rows.sort_by_key(|row| {
        (
            row.file_ref,
            row.table_id,
            row.segment_id,
            row.morsel_id,
            row.row_start,
        )
    });
    let mut out: Vec<CoviRowRangePostingV2> = Vec::with_capacity(rows.len());
    for row in rows.drain(..) {
        if row.row_count == 0 {
            return Err(CoveError::BadCovi);
        }
        if let Some(last) = out.last_mut() {
            let same_scope = last.file_ref == row.file_ref
                && last.table_id == row.table_id
                && last.segment_id == row.segment_id
                && last.morsel_id == row.morsel_id;
            let last_end = last
                .row_start
                .checked_add(last.row_count)
                .ok_or(CoveError::ArithOverflow)?;
            if same_scope && row.row_start <= last_end {
                let row_end = row
                    .row_start
                    .checked_add(row.row_count)
                    .ok_or(CoveError::ArithOverflow)?;
                last.row_count = row_end
                    .checked_sub(last.row_start)
                    .ok_or(CoveError::ArithOverflow)?;
                continue;
            }
        }
        out.push(row);
    }
    *rows = out;
    Ok(())
}

fn parse_fixed_payload<T>(
    payload: &[u8],
    width: usize,
    parse: impl Fn(&[u8]) -> Result<T, CoveError>,
) -> Result<Vec<T>, CoveError> {
    if payload.len() % width != 0 {
        return Err(CoveError::BadCovi);
    }
    payload.chunks_exact(width).map(parse).collect()
}

fn parse_u32_refs(payload: &[u8]) -> Result<Vec<u32>, CoveError> {
    if payload.len() % 4 != 0 {
        return Err(CoveError::BadCovi);
    }
    Ok(payload
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes(chunk.try_into().unwrap()))
        .collect())
}

fn answer_to_public(
    answer: &CoviAggregateAnswerV2,
    block: &CoviAggregateAnswerBlockV2,
) -> Result<CoviIndexOnlyAnswerV2, CoveError> {
    let value = if answer.value_ref == ABSENT_U32 {
        None
    } else {
        let start = usize::try_from(answer.value_ref).map_err(|_| CoveError::OffsetRange)?;
        if start > block.payload.len() {
            return Err(CoveError::OffsetRange);
        }
        let end = block
            .answers
            .iter()
            .filter_map(|candidate| {
                (candidate.value_ref != ABSENT_U32)
                    .then_some(candidate.value_ref as usize)
                    .filter(|offset| *offset > start)
            })
            .min()
            .unwrap_or(block.payload.len());
        Some(block.payload[start..end].to_vec())
    };
    Ok(CoviIndexOnlyAnswerV2 {
        aggregate_kind: CoviAggregateKindV2::from_u16(answer.aggregate_kind)
            .ok_or(CoveError::BadCovi)?,
        row_count: answer.row_count,
        null_count: answer.null_count,
        non_null_count: answer.non_null_count,
        value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn membership_request_preserves_typed_target_and_keys() {
        let request = CoviLookupRequestV2::object_property_membership(
            7,
            9,
            [
                CoviLookupKeyV2::CanonicalValueBytes(b"alpha".to_vec()),
                CoviLookupKeyV2::CanonicalValueBytes(b"beta".to_vec()),
            ],
        );
        assert_eq!(
            request.target,
            CoviLookupTargetV2::ObjectProperty {
                object_type_id: 7,
                property_id: 9
            }
        );
        assert_eq!(request.op, CoviLookupOpV2::Membership);
        assert_eq!(
            membership_key_bytes(&request),
            vec![b"alpha".to_vec(), b"beta".to_vec()]
        );
    }

    #[test]
    fn membership_key_match_checks_any_requested_key() {
        let request = CoviLookupRequestV2::membership(
            1,
            2,
            [
                CoviLookupKeyV2::CanonicalValueBytes(b"a".to_vec()),
                CoviLookupKeyV2::CanonicalValueBytes(b"c".to_vec()),
            ],
        );
        let keys = membership_key_bytes(&request);
        assert!(key_matches(
            &CoviLookupOpV2::Membership,
            b"c",
            b"a",
            None,
            &keys
        ));
        assert!(!key_matches(
            &CoviLookupOpV2::Membership,
            b"b",
            b"a",
            None,
            &keys
        ));
    }
}
