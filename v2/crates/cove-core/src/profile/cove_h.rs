//! Spec §44 — Harbor profile (registered COVE-E implementation).
//!
//! Harbor is a specific deployment of COVE-E: leased u64 EngineCodes with
//! tenant + code-space scope, epoch tracking, mount cache hints, and a hard
//! invariant that on-disk COVE FileCodes are NEVER Harbor EngineCodes.

use std::collections::BTreeMap;

use crate::{checksum, CoveError};

pub const HARBOR_MOUNT_HINTS_LEN: usize = 44;

/// Harbor mount descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarborMount {
    pub tenant_id: u64,
    pub code_space: u64,
    pub epoch: u64,
    /// Lease expiry (microseconds since the Unix epoch).
    pub lease_expires_at_us: i64,
}

impl HarborMount {
    /// Spec §44.5: Harbor EngineCodes occupy a reserved range that MUST NOT
    /// overlap with the COVE FileCode space. We require `tenant_id != 0` and
    /// `code_space != 0` so the implementation cannot accidentally write a
    /// Harbor code where a FileCode is expected.
    pub fn validate(&self) -> Result<(), CoveError> {
        if self.tenant_id == 0 || self.code_space == 0 {
            return Err(CoveError::HarborMountLease);
        }
        Ok(())
    }

    /// Returns true if a u64 looks like a Harbor EngineCode under this
    /// mount's invariants. Used by the writer to refuse misuse.
    pub fn is_engine_code(&self, code: u64) -> bool {
        // Harbor reserves the high 16 bits for the tenant id.
        ((code >> 48) & 0xFFFF) == (self.tenant_id & 0xFFFF)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarborLeaseEpochRequest {
    pub tenant_id: u64,
    pub code_space: u64,
    pub requested_epoch: u64,
}

pub trait HarborLeaseEpochValidator {
    fn validate_lease_epoch(&self, request: &HarborLeaseEpochRequest) -> Result<(), CoveError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarborCodeMapRequest {
    pub file_id: [u8; 16],
    pub table_id: u32,
    pub tenant_id: u64,
    pub code_space: u64,
    pub lease_epoch: u64,
    pub dictionary_crc32c: u32,
    pub filecode_count: usize,
}

pub trait HarborCodeMapResolver {
    fn resolve_code_map(&self, request: &HarborCodeMapRequest) -> Result<Vec<u64>, CoveError>;
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct HarborCodeMapKey {
    file_id: [u8; 16],
    table_id: u32,
    tenant_id: u64,
    code_space: u64,
    lease_epoch: u64,
    dictionary_crc32c: u32,
}

#[derive(Debug, Clone, Default)]
pub struct MockHarborResolver {
    leases: BTreeMap<(u64, u64), u64>,
    maps: BTreeMap<HarborCodeMapKey, Vec<u64>>,
}

impl MockHarborResolver {
    pub fn with_lease(mut self, tenant_id: u64, code_space: u64, epoch: u64) -> Self {
        self.leases.insert((tenant_id, code_space), epoch);
        self
    }

    pub fn with_code_map(mut self, request: &HarborCodeMapRequest, codes: Vec<u64>) -> Self {
        self.maps.insert(
            HarborCodeMapKey {
                file_id: request.file_id,
                table_id: request.table_id,
                tenant_id: request.tenant_id,
                code_space: request.code_space,
                lease_epoch: request.lease_epoch,
                dictionary_crc32c: request.dictionary_crc32c,
            },
            codes,
        );
        self
    }
}

impl HarborLeaseEpochValidator for MockHarborResolver {
    fn validate_lease_epoch(&self, request: &HarborLeaseEpochRequest) -> Result<(), CoveError> {
        match self.leases.get(&(request.tenant_id, request.code_space)) {
            Some(epoch) if *epoch == request.requested_epoch => Ok(()),
            _ => Err(CoveError::HarborMountLease),
        }
    }
}

impl HarborCodeMapResolver for MockHarborResolver {
    fn resolve_code_map(&self, request: &HarborCodeMapRequest) -> Result<Vec<u64>, CoveError> {
        self.validate_lease_epoch(&HarborLeaseEpochRequest {
            tenant_id: request.tenant_id,
            code_space: request.code_space,
            requested_epoch: request.lease_epoch,
        })?;
        let key = HarborCodeMapKey {
            file_id: request.file_id,
            table_id: request.table_id,
            tenant_id: request.tenant_id,
            code_space: request.code_space,
            lease_epoch: request.lease_epoch,
            dictionary_crc32c: request.dictionary_crc32c,
        };
        let codes = self.maps.get(&key).ok_or(CoveError::HarborMountLease)?;
        if codes.len() != request.filecode_count {
            return Err(CoveError::ExecutionCodeMap);
        }
        Ok(codes.clone())
    }
}

/// Spec §44.1 `HarborMountHintsV1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarborMountHintsV1 {
    pub harbor_profile_version_major: u16,
    pub harbor_profile_version_minor: u16,
    pub tenant_scope_ref: u32,
    pub code_space_ref: u32,
    pub lease_epoch: u64,
    pub dictionary_digest_ref: u32,
    pub catalog_digest_ref: u32,
    pub mount_cache_policy: u8,
    pub reserved: [u8; 7],
    pub private_payload_ref: u32,
    pub checksum: u32,
}

impl HarborMountHintsV1 {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < HARBOR_MOUNT_HINTS_LEN {
            return Err(CoveError::BufferTooShort);
        }
        let harbor_profile_version_major = u16::from_le_bytes(bytes[0..2].try_into().unwrap());
        let harbor_profile_version_minor = u16::from_le_bytes(bytes[2..4].try_into().unwrap());
        let tenant_scope_ref = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let code_space_ref = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        let lease_epoch = u64::from_le_bytes(bytes[12..20].try_into().unwrap());
        let dictionary_digest_ref = u32::from_le_bytes(bytes[20..24].try_into().unwrap());
        let catalog_digest_ref = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
        let mount_cache_policy = bytes[28];
        let mut reserved = [0u8; 7];
        reserved.copy_from_slice(&bytes[29..36]);
        if reserved != [0; 7] {
            return Err(CoveError::ReservedNotZero);
        }
        let private_payload_ref = u32::from_le_bytes(bytes[36..40].try_into().unwrap());
        let checksum_field = u32::from_le_bytes(bytes[40..44].try_into().unwrap());

        let mut for_crc = [0u8; HARBOR_MOUNT_HINTS_LEN];
        for_crc.copy_from_slice(&bytes[..HARBOR_MOUNT_HINTS_LEN]);
        for_crc[40..44].fill(0);
        if checksum::crc32c(&for_crc) != checksum_field {
            return Err(CoveError::ChecksumMismatch);
        }

        Ok(Self {
            harbor_profile_version_major,
            harbor_profile_version_minor,
            tenant_scope_ref,
            code_space_ref,
            lease_epoch,
            dictionary_digest_ref,
            catalog_digest_ref,
            mount_cache_policy,
            reserved,
            private_payload_ref,
            checksum: checksum_field,
        })
    }

    pub fn serialize(&self) -> [u8; HARBOR_MOUNT_HINTS_LEN] {
        let mut buf = [0u8; HARBOR_MOUNT_HINTS_LEN];
        buf[0..2].copy_from_slice(&self.harbor_profile_version_major.to_le_bytes());
        buf[2..4].copy_from_slice(&self.harbor_profile_version_minor.to_le_bytes());
        buf[4..8].copy_from_slice(&self.tenant_scope_ref.to_le_bytes());
        buf[8..12].copy_from_slice(&self.code_space_ref.to_le_bytes());
        buf[12..20].copy_from_slice(&self.lease_epoch.to_le_bytes());
        buf[20..24].copy_from_slice(&self.dictionary_digest_ref.to_le_bytes());
        buf[24..28].copy_from_slice(&self.catalog_digest_ref.to_le_bytes());
        buf[28] = self.mount_cache_policy;
        buf[29..36].copy_from_slice(&self.reserved);
        buf[36..40].copy_from_slice(&self.private_payload_ref.to_le_bytes());
        let crc = checksum::crc32c(&buf);
        buf[40..44].copy_from_slice(&crc.to_le_bytes());
        buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_zero_tenant() {
        let m = HarborMount {
            tenant_id: 0,
            code_space: 1,
            epoch: 1,
            lease_expires_at_us: 0,
        };
        assert_eq!(m.validate(), Err(CoveError::HarborMountLease));
    }

    #[test]
    fn engine_code_detection_is_high_bits() {
        let m = HarborMount {
            tenant_id: 0xAB,
            code_space: 1,
            epoch: 1,
            lease_expires_at_us: 0,
        };
        assert!(m.is_engine_code(0x00AB_0000_0000_1234));
        assert!(!m.is_engine_code(0x0001_0000_0000_1234));
    }

    #[test]
    fn harbor_mount_hints_roundtrip() {
        let hints = HarborMountHintsV1 {
            harbor_profile_version_major: 1,
            harbor_profile_version_minor: 0,
            tenant_scope_ref: 7,
            code_space_ref: 8,
            lease_epoch: 9,
            dictionary_digest_ref: 10,
            catalog_digest_ref: 11,
            mount_cache_policy: 1,
            reserved: [0; 7],
            private_payload_ref: 0,
            checksum: 0,
        };
        let parsed = HarborMountHintsV1::parse(&hints.serialize()).unwrap();
        assert_eq!(parsed.harbor_profile_version_major, 1);
        assert_eq!(parsed.tenant_scope_ref, hints.tenant_scope_ref);
        assert_eq!(parsed.lease_epoch, hints.lease_epoch);
    }

    #[test]
    fn harbor_mount_hints_reject_reserved_bytes() {
        let mut bytes = HarborMountHintsV1 {
            harbor_profile_version_major: 1,
            harbor_profile_version_minor: 0,
            tenant_scope_ref: 7,
            code_space_ref: 8,
            lease_epoch: 9,
            dictionary_digest_ref: 10,
            catalog_digest_ref: 11,
            mount_cache_policy: 1,
            reserved: [0; 7],
            private_payload_ref: 0,
            checksum: 0,
        }
        .serialize();
        bytes[29] = 1;
        assert_eq!(
            HarborMountHintsV1::parse(&bytes),
            Err(CoveError::ReservedNotZero)
        );
    }

    #[test]
    fn mock_harbor_resolver_rejects_lease_mismatch() {
        let resolver = MockHarborResolver::default().with_lease(7, 11, 3);
        let request = HarborLeaseEpochRequest {
            tenant_id: 7,
            code_space: 11,
            requested_epoch: 4,
        };
        assert_eq!(
            resolver.validate_lease_epoch(&request),
            Err(CoveError::HarborMountLease)
        );
    }

    #[test]
    fn mock_harbor_resolver_returns_deterministic_code_map() {
        let request = HarborCodeMapRequest {
            file_id: [9; 16],
            table_id: 5,
            tenant_id: 7,
            code_space: 11,
            lease_epoch: 3,
            dictionary_crc32c: 0xAABB_CCDD,
            filecode_count: 3,
        };
        let resolver = MockHarborResolver::default()
            .with_lease(7, 11, 3)
            .with_code_map(&request, vec![10, 11, 12]);
        assert_eq!(
            resolver.resolve_code_map(&request).unwrap(),
            vec![10, 11, 12]
        );
    }
}
