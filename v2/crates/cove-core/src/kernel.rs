//! Cove Format (COVE) v2.0 — Kernel capability descriptor (Spec §21).
//!
//! A *kernel capability* describes what an engine-specific decode kernel can
//! do beyond the canonical decode path: vectorisation hints, fused predicate
//! evaluation, etc. Capability descriptors are advisory — every COVE reader
//! MUST be able to fall back to the canonical decode path if a capability is
//! missing.

use crate::{constants::CoveEncodingKind, CoveError};

const KERNEL_CAPABILITY_ENTRY_LEN: usize = 18;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelCapabilityEntry {
    pub encoding: CoveEncodingKind,
    pub supports_eq: u8,
    pub supports_in: u8,
    pub supports_range: u8,
    pub supports_is_null: u8,
    pub supports_count: u8,
    pub supports_min_max: u8,
    pub supports_selection_decode: u8,
    pub supports_direct_executioncode_remap: u8,
    pub decode_cost_class: u8,
    pub predicate_cost_class: u8,
    pub reserved: [u8; 6],
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KernelCapabilities {
    pub entries: Vec<KernelCapabilityEntry>,
}

impl KernelCapabilities {
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < 4 {
            return Err(CoveError::BufferTooShort);
        }
        let count = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let entries_bytes = count
            .checked_mul(KERNEL_CAPABILITY_ENTRY_LEN)
            .ok_or(CoveError::ArithOverflow)?;
        let required_len = 4usize
            .checked_add(entries_bytes)
            .ok_or(CoveError::ArithOverflow)?;
        if required_len > bytes.len() {
            return Err(CoveError::BufferTooShort);
        }
        if required_len != bytes.len() {
            return Err(CoveError::BadSection(
                "kernel capabilities section has trailing bytes".into(),
            ));
        }
        let mut entries = Vec::with_capacity(count);
        let mut pos = 4;
        for _ in 0..count {
            let enc_raw = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap());
            pos += 2;
            let encoding = CoveEncodingKind::from_u16(enc_raw)
                .ok_or_else(|| CoveError::BadSection(format!("unknown encoding {enc_raw}")))?;
            let supports_eq = parse_support_byte(bytes[pos], "supports_eq")?;
            pos += 1;
            let supports_in = parse_support_byte(bytes[pos], "supports_in")?;
            pos += 1;
            let supports_range = parse_support_byte(bytes[pos], "supports_range")?;
            pos += 1;
            let supports_is_null = parse_support_byte(bytes[pos], "supports_is_null")?;
            pos += 1;
            let supports_count = parse_support_byte(bytes[pos], "supports_count")?;
            pos += 1;
            let supports_min_max = parse_support_byte(bytes[pos], "supports_min_max")?;
            pos += 1;
            let supports_selection_decode =
                parse_support_byte(bytes[pos], "supports_selection_decode")?;
            pos += 1;
            let supports_direct_executioncode_remap =
                parse_support_byte(bytes[pos], "supports_direct_executioncode_remap")?;
            pos += 1;
            let decode_cost_class = bytes[pos];
            pos += 1;
            let predicate_cost_class = bytes[pos];
            pos += 1;
            let mut reserved = [0u8; 6];
            reserved.copy_from_slice(&bytes[pos..pos + 6]);
            pos += 6;
            if reserved != [0; 6] {
                return Err(CoveError::ReservedNotZero);
            }
            entries.push(KernelCapabilityEntry {
                encoding,
                supports_eq,
                supports_in,
                supports_range,
                supports_is_null,
                supports_count,
                supports_min_max,
                supports_selection_decode,
                supports_direct_executioncode_remap,
                decode_cost_class,
                predicate_cost_class,
                reserved,
            });
        }
        Ok(Self { entries })
    }

    pub fn capability_for(&self, encoding: CoveEncodingKind) -> Option<&KernelCapabilityEntry> {
        self.entries.iter().find(|e| e.encoding == encoding)
    }

    /// Spec §21 — serialise this kernel-capabilities section into the wire
    /// format consumed by [`KernelCapabilities::parse`]. Round-trip parity is
    /// covered by the unit tests below.
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + self.entries.len() * KERNEL_CAPABILITY_ENTRY_LEN);
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for entry in &self.entries {
            out.extend_from_slice(&(entry.encoding as u16).to_le_bytes());
            out.push(entry.supports_eq);
            out.push(entry.supports_in);
            out.push(entry.supports_range);
            out.push(entry.supports_is_null);
            out.push(entry.supports_count);
            out.push(entry.supports_min_max);
            out.push(entry.supports_selection_decode);
            out.push(entry.supports_direct_executioncode_remap);
            out.push(entry.decode_cost_class);
            out.push(entry.predicate_cost_class);
            out.extend_from_slice(&entry.reserved);
        }
        out
    }
}

fn parse_support_byte(value: u8, field: &str) -> Result<u8, CoveError> {
    if value <= 1 {
        Ok(value)
    } else {
        Err(CoveError::BadSection(format!(
            "kernel capability {field} must be 0 or 1, got {value}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(encoding: CoveEncodingKind) -> KernelCapabilityEntry {
        KernelCapabilityEntry {
            encoding,
            supports_eq: 1,
            supports_in: 1,
            supports_range: 0,
            supports_is_null: 1,
            supports_count: 1,
            supports_min_max: 0,
            supports_selection_decode: 1,
            supports_direct_executioncode_remap: 0,
            decode_cost_class: 2,
            predicate_cost_class: 3,
            reserved: [0; 6],
        }
    }

    #[test]
    fn round_trip_capabilities() {
        let bytes = KernelCapabilities {
            entries: vec![entry(CoveEncodingKind::Rle)],
        }
        .serialize();
        let kc = KernelCapabilities::parse(&bytes).unwrap();
        let capability = kc.capability_for(CoveEncodingKind::Rle).unwrap();
        assert_eq!(capability.supports_eq, 1);
        assert_eq!(capability.supports_selection_decode, 1);
        assert!(kc.capability_for(CoveEncodingKind::Sparse).is_none());
    }

    #[test]
    fn rejects_unknown_encoding() {
        let mut bytes = 1u32.to_le_bytes().to_vec();
        bytes.extend_from_slice(&0xfffeu16.to_le_bytes());
        bytes.extend_from_slice(&[0; KERNEL_CAPABILITY_ENTRY_LEN - 2]);
        assert!(matches!(
            KernelCapabilities::parse(&bytes),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn rejects_oversized_entry_count_before_allocating() {
        let bytes = u32::MAX.to_le_bytes().to_vec();
        assert_eq!(
            KernelCapabilities::parse(&bytes),
            Err(CoveError::BufferTooShort)
        );
    }

    #[test]
    fn serialize_round_trip() {
        let kc = KernelCapabilities {
            entries: vec![
                entry(CoveEncodingKind::Rle),
                entry(CoveEncodingKind::PlainFixed),
            ],
        };
        let bytes = kc.serialize();
        let parsed = KernelCapabilities::parse(&bytes).unwrap();
        assert_eq!(parsed, kc);
    }

    #[test]
    fn rejects_reserved_bytes() {
        let mut bytes = KernelCapabilities {
            entries: vec![entry(CoveEncodingKind::Rle)],
        }
        .serialize();
        *bytes.last_mut().unwrap() = 1;
        assert_eq!(
            KernelCapabilities::parse(&bytes),
            Err(CoveError::ReservedNotZero)
        );
    }

    #[test]
    fn rejects_trailing_bytes() {
        let mut bytes = KernelCapabilities {
            entries: vec![entry(CoveEncodingKind::Rle)],
        }
        .serialize();
        bytes.push(0);
        assert!(matches!(
            KernelCapabilities::parse(&bytes),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn serialize_empty() {
        let kc = KernelCapabilities::default();
        let bytes = kc.serialize();
        assert_eq!(bytes, 0u32.to_le_bytes().to_vec());
        assert_eq!(KernelCapabilities::parse(&bytes).unwrap(), kc);
    }
}
