//! Spec §44 — Harbor profile (registered QF-E implementation).
//!
//! Harbor is a specific deployment of QF-E: leased u64 EngineCodes with
//! tenant + code-space scope, epoch tracking, mount cache hints, and a hard
//! invariant that on-disk QF FileCodes are NEVER Harbor EngineCodes.

use crate::{checksum, QfError};

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
    /// overlap with the QF FileCode space. We require `tenant_id != 0` and
    /// `code_space != 0` so the implementation cannot accidentally write a
    /// Harbor code where a FileCode is expected.
    pub fn validate(&self) -> Result<(), QfError> {
        if self.tenant_id == 0 || self.code_space == 0 {
            return Err(QfError::HarborMountLease);
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
    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < HARBOR_MOUNT_HINTS_LEN {
            return Err(QfError::BufferTooShort);
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
            return Err(QfError::ReservedNotZero);
        }
        let private_payload_ref = u32::from_le_bytes(bytes[36..40].try_into().unwrap());
        let checksum_field = u32::from_le_bytes(bytes[40..44].try_into().unwrap());

        let mut for_crc = [0u8; HARBOR_MOUNT_HINTS_LEN];
        for_crc.copy_from_slice(&bytes[..HARBOR_MOUNT_HINTS_LEN]);
        for_crc[40..44].fill(0);
        if checksum::crc32c(&for_crc) != checksum_field {
            return Err(QfError::ChecksumMismatch);
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
        assert_eq!(m.validate(), Err(QfError::HarborMountLease));
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
            Err(QfError::ReservedNotZero)
        );
    }
}
