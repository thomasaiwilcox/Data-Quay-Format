//! Reference COVE-H Harbor mount profile entrypoint.
//!
//! The profile primitives live in `cove-core`; this crate provides the stable
//! package named by the v2 reference-suite spec and keeps Harbor-facing users
//! off internal module paths.

use cove_core::{compression, constants::SectionKind, reader, CoveError};

pub use cove_core::profile::cove_h::*;
pub use cove_engine::mount;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarborMountHintInspection {
    pub version_major: u16,
    pub version_minor: u16,
    pub tenant_scope_ref: u32,
    pub code_space_ref: u32,
    pub lease_epoch: u64,
    pub mount_cache_policy: u8,
    pub private_payload_ref: u32,
}

pub fn validate_mount_profile(profile: &HarborMount) -> Result<(), CoveError> {
    profile.validate()
}

pub fn inspect_mount_hints(bytes: &[u8]) -> Result<HarborMountHintInspection, CoveError> {
    let hints = HarborMountHintsV1::parse(bytes)?;
    Ok(HarborMountHintInspection {
        version_major: hints.harbor_profile_version_major,
        version_minor: hints.harbor_profile_version_minor,
        tenant_scope_ref: hints.tenant_scope_ref,
        code_space_ref: hints.code_space_ref,
        lease_epoch: hints.lease_epoch,
        mount_cache_policy: hints.mount_cache_policy,
        private_payload_ref: hints.private_payload_ref,
    })
}

pub fn mount_profile_from_cove(bytes: &[u8]) -> Result<Option<HarborMountHintsV1>, CoveError> {
    let validated = reader::validate_bytes(bytes)?;
    let mut parsed = None;
    for entry in &validated.footer.sections {
        if SectionKind::from_u16(entry.section_kind) != Some(SectionKind::HarborMountHints) {
            continue;
        }
        if parsed.is_some() {
            return Err(CoveError::BadEngineProfile);
        }
        let payload = compression::section_payload(bytes, entry)?;
        parsed = Some(HarborMountHintsV1::parse(&payload)?);
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harbor_profile_facade_exports_mount_hints_type() {
        let hints = HarborMountHintsV1 {
            harbor_profile_version_major: 1,
            harbor_profile_version_minor: 0,
            tenant_scope_ref: 1,
            code_space_ref: 2,
            lease_epoch: 3,
            dictionary_digest_ref: 4,
            catalog_digest_ref: 5,
            mount_cache_policy: 0,
            reserved: [0; 7],
            private_payload_ref: 0,
            checksum: 0,
        };
        assert_eq!(hints.harbor_profile_version_major, 1);
    }

    #[test]
    fn harbor_facade_validates_and_inspects_hints() {
        let mount = HarborMount {
            tenant_id: 7,
            code_space: 11,
            epoch: 3,
            lease_expires_at_us: 0,
        };
        validate_mount_profile(&mount).unwrap();
        let hints = HarborMountHintsV1 {
            harbor_profile_version_major: 1,
            harbor_profile_version_minor: 0,
            tenant_scope_ref: 7,
            code_space_ref: 11,
            lease_epoch: 3,
            dictionary_digest_ref: 0,
            catalog_digest_ref: 0,
            mount_cache_policy: 1,
            reserved: [0; 7],
            private_payload_ref: 0,
            checksum: 0,
        };
        let inspection = inspect_mount_hints(&hints.serialize()).unwrap();
        assert_eq!(inspection.lease_epoch, 3);
    }
}
