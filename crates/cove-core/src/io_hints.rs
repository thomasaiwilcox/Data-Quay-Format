//! Cove Format (COVE) v1.0 — I/O hints (Spec §8.8, §67).
//!
//! Optional descriptive metadata that nudges the reader toward efficient
//! object-store / NVMe access patterns. The reader is free to ignore hints.

use crate::CoveError;

/// Suggested coalescing window for adjacent reads, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoHints {
    /// Preferred minimum read size (e.g. 1 MiB for object stores).
    pub min_read_bytes: u32,
    /// Suggested prefetch distance ahead of the cursor, in bytes.
    pub prefetch_bytes: u32,
    /// Recommended page alignment for direct I/O.
    pub alignment_bytes: u32,
}

impl IoHints {
    pub const ENCODED_LEN: usize = 12;

    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < Self::ENCODED_LEN {
            return Err(CoveError::BufferTooShort);
        }
        Ok(Self {
            min_read_bytes: u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
            prefetch_bytes: u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            alignment_bytes: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
        })
    }

    pub fn encode(&self) -> [u8; Self::ENCODED_LEN] {
        let mut out = [0u8; Self::ENCODED_LEN];
        out[0..4].copy_from_slice(&self.min_read_bytes.to_le_bytes());
        out[4..8].copy_from_slice(&self.prefetch_bytes.to_le_bytes());
        out[8..12].copy_from_slice(&self.alignment_bytes.to_le_bytes());
        out
    }
}

/// Default Spec §67 hints suitable for cloud object stores.
pub const fn defaults_object_store() -> IoHints {
    IoHints {
        min_read_bytes: 1 << 20,       // 1 MiB
        prefetch_bytes: 4 * (1 << 20), // 4 MiB
        alignment_bytes: 1 << 20,
    }
}

/// Default hints suitable for local NVMe.
pub const fn defaults_nvme() -> IoHints {
    IoHints {
        min_read_bytes: 1 << 16,  // 64 KiB
        prefetch_bytes: 1 << 20,  // 1 MiB
        alignment_bytes: 1 << 12, // 4 KiB
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let h = defaults_object_store();
        let bytes = h.encode();
        assert_eq!(IoHints::parse(&bytes).unwrap(), h);
    }
}
