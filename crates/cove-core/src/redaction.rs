//! Cove Format (COVE) v1.0 — Redaction manifest (Spec §64).
//!
//! Redactions are first-class structural metadata. A redacted value is present
//! but inaccessible, and the redaction manifest records the target section,
//! target-local reference, policy, and audit fields needed to prove the
//! redaction happened deliberately. Redacted values are *not* null values.

use crate::CoveError;

/// One redacted value's audit record (Spec §64 `RedactionManifestEntryV1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactionEntry {
    pub redaction_id: u64,
    pub section_id: u32,
    pub local_ref: u64,
    pub reason_code: u16,
    pub policy_id: Vec<u8>,
    pub audit_ref: Vec<u8>,
    pub created_at_us: i64,
}

/// A parsed redaction manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RedactionManifest {
    pub entries: Vec<RedactionEntry>,
}

impl RedactionManifest {
    const MIN_ENTRY_LEN: usize = 8 + 4 + 8 + 2 + 2 + 2 + 8;

    /// Parse a redaction manifest section.
    ///
    /// Wire format (LE throughout):
    ///   `u32` entry_count
    ///   For each entry, Spec §64 `RedactionManifestEntryV1`:
    ///     `u64` redaction_id
    ///     `u32` section_id
    ///     `u64` local_ref
    ///     `u16` reason_code
    ///     `u16` policy_id_len, policy_id bytes
    ///     `u16` audit_ref_len, audit_ref bytes
    ///     `i64` created_at_us
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < 4 {
            return Err(CoveError::BufferTooShort);
        }
        let count = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let mut pos = 4usize;
        let max_entries_by_min_size = bytes
            .len()
            .saturating_sub(4)
            / Self::MIN_ENTRY_LEN;
        let mut entries = Vec::with_capacity(count.min(max_entries_by_min_size));
        for _ in 0..count {
            if pos + 8 + 4 + 8 + 2 > bytes.len() {
                return Err(CoveError::BufferTooShort);
            }
            let redaction_id = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
            pos += 8;
            let section_id = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
            pos += 4;
            let local_ref = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
            pos += 8;
            let reason_code = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap());
            pos += 2;
            let policy_id = read_bytes(bytes, &mut pos, "redaction policy_id")?;
            let audit_ref = read_bytes(bytes, &mut pos, "redaction audit_ref")?;
            if pos + 8 > bytes.len() {
                return Err(CoveError::BufferTooShort);
            }
            let created_at_us = i64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
            pos += 8;
            entries.push(RedactionEntry {
                redaction_id,
                section_id,
                local_ref,
                reason_code,
                policy_id,
                audit_ref,
                created_at_us,
            });
        }
        Ok(Self { entries })
    }

    /// Inverse of [`Self::parse`]; produces canonical bytes that round-trip.
    pub fn serialize(&self) -> Result<Vec<u8>, CoveError> {
        let mut out = Vec::with_capacity(4 + self.entries.len() * 40);
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for entry in &self.entries {
            out.extend_from_slice(&entry.redaction_id.to_le_bytes());
            out.extend_from_slice(&entry.section_id.to_le_bytes());
            out.extend_from_slice(&entry.local_ref.to_le_bytes());
            out.extend_from_slice(&entry.reason_code.to_le_bytes());
            write_len_prefixed(&mut out, &entry.policy_id, "redaction policy_id")?;
            write_len_prefixed(&mut out, &entry.audit_ref, "redaction audit_ref")?;
            out.extend_from_slice(&entry.created_at_us.to_le_bytes());
        }
        Ok(out)
    }

    /// Look up a redaction record by target section and target-local reference.
    pub fn entry(&self, section_id: u32, local_ref: u64) -> Option<&RedactionEntry> {
        self.entries
            .iter()
            .find(|e| e.section_id == section_id && e.local_ref == local_ref)
    }
}

fn read_bytes(bytes: &[u8], pos: &mut usize, _what: &str) -> Result<Vec<u8>, CoveError> {
    if *pos + 2 > bytes.len() {
        return Err(CoveError::BufferTooShort);
    }
    let len = u16::from_le_bytes(bytes[*pos..*pos + 2].try_into().unwrap()) as usize;
    *pos += 2;
    let end = pos.checked_add(len).ok_or(CoveError::ArithOverflow)?;
    if end > bytes.len() {
        return Err(CoveError::BufferTooShort);
    }
    let bytes = bytes[*pos..end].to_vec();
    *pos = end;
    Ok(bytes)
}

fn write_len_prefixed(out: &mut Vec<u8>, bytes: &[u8], what: &str) -> Result<(), CoveError> {
    let len = u16::try_from(bytes.len())
        .map_err(|_| CoveError::BadSection(format!("{what} exceeds u16 length limit")))?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}

/// Reader policy that decides how a redacted value is surfaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedactionPolicy {
    /// Treat redacted values as missing (return `None`).
    Hide,
    /// Surface a synthetic placeholder string.
    Placeholder,
    /// Refuse to read the column at all (return [`CoveError::RedactionPolicy`]).
    Refuse,
}

/// Apply a [`RedactionPolicy`] to the lookup of a redacted value.
pub fn apply_policy(
    policy: RedactionPolicy,
    placeholder: &str,
) -> Result<Option<String>, CoveError> {
    match policy {
        RedactionPolicy::Hide => Ok(None),
        RedactionPolicy::Placeholder => Ok(Some(placeholder.to_string())),
        RedactionPolicy::Refuse => Err(CoveError::RedactionPolicy),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_bytes(entries: &[(u64, u32, u64, u16, &[u8], &[u8], i64)]) -> Vec<u8> {
        let mut out = (entries.len() as u32).to_le_bytes().to_vec();
        for (redaction_id, section_id, local_ref, reason, policy, audit, ts) in entries {
            out.extend_from_slice(&redaction_id.to_le_bytes());
            out.extend_from_slice(&section_id.to_le_bytes());
            out.extend_from_slice(&local_ref.to_le_bytes());
            out.extend_from_slice(&reason.to_le_bytes());
            out.extend_from_slice(&(policy.len() as u16).to_le_bytes());
            out.extend_from_slice(policy);
            out.extend_from_slice(&(audit.len() as u16).to_le_bytes());
            out.extend_from_slice(audit);
            out.extend_from_slice(&ts.to_le_bytes());
        }
        out
    }

    #[test]
    fn empty_manifest_parses() {
        let m = RedactionManifest::parse(&make_bytes(&[])).unwrap();
        assert!(m.entries.is_empty());
    }

    #[test]
    fn round_trip_single_entry() {
        let bytes = make_bytes(&[(
            1,
            7,
            42,
            17,
            b"policy/gdpr",
            b"ticket #42",
            1_700_000_000_000_000,
        )]);
        let m = RedactionManifest::parse(&bytes).unwrap();
        let e = m.entry(7, 42).unwrap();
        assert_eq!(e.reason_code, 17);
        assert_eq!(e.policy_id, b"policy/gdpr");
        assert_eq!(e.audit_ref, b"ticket #42");
        assert_eq!(e.created_at_us, 1_700_000_000_000_000);
    }

    #[test]
    fn truncated_rejected() {
        let mut bytes = 1u32.to_le_bytes().to_vec();
        bytes.extend_from_slice(&0u64.to_le_bytes()); // missing rest
        assert!(matches!(
            RedactionManifest::parse(&bytes),
            Err(CoveError::BufferTooShort)
        ));
    }

    #[test]
    fn spec_64_policy_hide_returns_none() {
        assert_eq!(apply_policy(RedactionPolicy::Hide, "***"), Ok(None));
    }

    #[test]
    fn spec_64_policy_placeholder_returns_string() {
        assert_eq!(
            apply_policy(RedactionPolicy::Placeholder, "***"),
            Ok(Some("***".into()))
        );
    }

    #[test]
    fn spec_64_policy_refuse_errors() {
        assert_eq!(
            apply_policy(RedactionPolicy::Refuse, "***"),
            Err(CoveError::RedactionPolicy)
        );
    }
}

#[cfg(test)]
mod serialize_tests {
    use super::*;

    #[test]
    fn serialize_round_trip() {
        let m = RedactionManifest {
            entries: vec![
                RedactionEntry {
                    redaction_id: 1,
                    section_id: 7,
                    local_ref: 11,
                    reason_code: 17,
                    policy_id: b"GDPR.ART17".to_vec(),
                    audit_ref: b"ticket=42".to_vec(),
                    created_at_us: 1_700_000_000_000_000,
                },
                RedactionEntry {
                    redaction_id: 2,
                    section_id: 7,
                    local_ref: 12,
                    reason_code: 0,
                    policy_id: Vec::new(),
                    audit_ref: Vec::new(),
                    created_at_us: -1,
                },
            ],
        };
        let bytes = m.serialize().unwrap();
        assert_eq!(RedactionManifest::parse(&bytes).unwrap(), m);
    }
}
