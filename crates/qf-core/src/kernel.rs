//! Quay Format (QF) v1.0 — Kernel capability descriptor (Spec §21).
//!
//! A *kernel capability* describes what an engine-specific decode kernel can
//! do beyond the canonical decode path: vectorisation hints, fused predicate
//! evaluation, etc. Capability descriptors are advisory — every QF reader
//! MUST be able to fall back to the canonical decode path if a capability is
//! missing.

use crate::{constants::QfEncodingKind, QfError};

/// Capability flags (Spec §21.2). Stored as a 32-bit bitset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KernelCapabilityFlags(pub u32);

impl KernelCapabilityFlags {
    pub const CANONICAL_DECODE: Self = Self(1 << 0);
    pub const FAST_DECODE: Self = Self(1 << 1);
    pub const PREDICATE_PUSHDOWN: Self = Self(1 << 2);
    pub const ENGINE_NATIVE: Self = Self(1 << 3);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub const fn bits(self) -> u32 {
        self.0
    }
}

impl std::ops::BitOr for KernelCapabilityFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelCapabilityEntry {
    pub encoding: QfEncodingKind,
    pub flags: KernelCapabilityFlags,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KernelCapabilities {
    pub entries: Vec<KernelCapabilityEntry>,
}

impl KernelCapabilities {
    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < 4 {
            return Err(QfError::BufferTooShort);
        }
        let count = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let entry_size = 2usize + 4;
        let entries_bytes = count
            .checked_mul(entry_size)
            .ok_or(QfError::ArithOverflow)?;
        let required_len = 4usize
            .checked_add(entries_bytes)
            .ok_or(QfError::ArithOverflow)?;
        if required_len > bytes.len() {
            return Err(QfError::BufferTooShort);
        }
        let mut entries = Vec::with_capacity(count);
        let mut pos = 4;
        for _ in 0..count {
            let enc_raw = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap());
            pos += 2;
            let flag_raw = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
            pos += 4;
            let encoding = QfEncodingKind::from_u16(enc_raw)
                .ok_or_else(|| QfError::BadSection(format!("unknown encoding {enc_raw}")))?;
            entries.push(KernelCapabilityEntry {
                encoding,
                flags: KernelCapabilityFlags(flag_raw),
            });
        }
        Ok(Self { entries })
    }

    pub fn capability_for(&self, encoding: QfEncodingKind) -> Option<KernelCapabilityFlags> {
        self.entries
            .iter()
            .find(|e| e.encoding == encoding)
            .map(|e| e.flags)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_capabilities() {
        let mut bytes = 1u32.to_le_bytes().to_vec();
        bytes.extend_from_slice(&(QfEncodingKind::Rle as u16).to_le_bytes());
        bytes.extend_from_slice(
            &(KernelCapabilityFlags::CANONICAL_DECODE | KernelCapabilityFlags::FAST_DECODE)
                .bits()
                .to_le_bytes(),
        );
        let kc = KernelCapabilities::parse(&bytes).unwrap();
        let f = kc.capability_for(QfEncodingKind::Rle).unwrap();
        assert!(f.contains(KernelCapabilityFlags::FAST_DECODE));
        assert!(kc.capability_for(QfEncodingKind::Sparse).is_none());
    }

    #[test]
    fn rejects_unknown_encoding() {
        let mut bytes = 1u32.to_le_bytes().to_vec();
        bytes.extend_from_slice(&0xfffeu16.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        assert!(matches!(
            KernelCapabilities::parse(&bytes),
            Err(QfError::BadSection(_))
        ));
    }

    #[test]
    fn rejects_oversized_entry_count_before_allocating() {
        let bytes = u32::MAX.to_le_bytes().to_vec();
        assert_eq!(KernelCapabilities::parse(&bytes), Err(QfError::BufferTooShort));
    }
}
