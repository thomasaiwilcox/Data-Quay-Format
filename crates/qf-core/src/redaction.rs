//! Quay Format (QF) v1.0 — Redaction manifest (Spec §64).
//!
//! Redactions are first-class structural metadata. When a value is removed
//! from a column it is replaced with a [`StorageClass::Redacted`] dictionary
//! entry, and the redaction manifest records the audit fields needed to prove
//! the removal happened deliberately. Redacted values are *not* the same as
//! null values: nulls remain in the validity bitmap (Spec §6.1).

use crate::QfError;

/// One redacted value's audit record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactionEntry {
    /// FileCode of the redacted dictionary entry.
    pub file_code: u64,
    /// Stable reason code (vendor- or policy-defined) per Spec §64.2.
    pub reason_code: String,
    /// Free-form audit context (operator, ticket, system, etc.).
    pub audit_context: String,
    /// Wall-clock timestamp when redaction was performed (microseconds since
    /// the Unix epoch). Reused to pin redactions in audit logs.
    pub created_at_us: i64,
}

/// A parsed redaction manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RedactionManifest {
    pub entries: Vec<RedactionEntry>,
}

impl RedactionManifest {
    /// Parse a redaction manifest section.
    ///
    /// Wire format (LE throughout):
    ///   `u32` entry_count
    ///   For each entry:
    ///     `u64` file_code
    ///     `i64` created_at_us
    ///     `u16` reason_len, reason_len bytes (UTF-8)
    ///     `u16` ctx_len,    ctx_len bytes (UTF-8)
    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < 4 {
            return Err(QfError::BufferTooShort);
        }
        let count = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let mut pos = 4usize;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            if pos + 16 > bytes.len() {
                return Err(QfError::BufferTooShort);
            }
            let file_code = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
            pos += 8;
            let created_at_us = i64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
            pos += 8;
            let reason = read_str(bytes, &mut pos, "redaction reason")?;
            let context = read_str(bytes, &mut pos, "redaction context")?;
            entries.push(RedactionEntry {
                file_code,
                reason_code: reason,
                audit_context: context,
                created_at_us,
            });
        }
        Ok(Self { entries })
    }

    /// Look up a redaction record by FileCode.
    pub fn entry(&self, file_code: u64) -> Option<&RedactionEntry> {
        self.entries.iter().find(|e| e.file_code == file_code)
    }
}

fn read_str(bytes: &[u8], pos: &mut usize, what: &str) -> Result<String, QfError> {
    if *pos + 2 > bytes.len() {
        return Err(QfError::BufferTooShort);
    }
    let len = u16::from_le_bytes(bytes[*pos..*pos + 2].try_into().unwrap()) as usize;
    *pos += 2;
    let end = pos.checked_add(len).ok_or(QfError::ArithOverflow)?;
    if end > bytes.len() {
        return Err(QfError::BufferTooShort);
    }
    let s = std::str::from_utf8(&bytes[*pos..end])
        .map_err(|_| QfError::BadSection(format!("{what} is not valid UTF-8")))?
        .to_string();
    *pos = end;
    Ok(s)
}

/// Reader policy that decides how a redacted value is surfaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedactionPolicy {
    /// Treat redacted values as missing (return `None`).
    Hide,
    /// Surface a synthetic placeholder string.
    Placeholder,
    /// Refuse to read the column at all (return [`QfError::RedactionPolicy`]).
    Refuse,
}

/// Apply a [`RedactionPolicy`] to the lookup of a redacted value.
pub fn apply_policy(policy: RedactionPolicy, placeholder: &str) -> Result<Option<String>, QfError> {
    match policy {
        RedactionPolicy::Hide => Ok(None),
        RedactionPolicy::Placeholder => Ok(Some(placeholder.to_string())),
        RedactionPolicy::Refuse => Err(QfError::RedactionPolicy),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_bytes(entries: &[(u64, i64, &str, &str)]) -> Vec<u8> {
        let mut out = (entries.len() as u32).to_le_bytes().to_vec();
        for (fc, ts, reason, ctx) in entries {
            out.extend_from_slice(&fc.to_le_bytes());
            out.extend_from_slice(&ts.to_le_bytes());
            out.extend_from_slice(&(reason.len() as u16).to_le_bytes());
            out.extend_from_slice(reason.as_bytes());
            out.extend_from_slice(&(ctx.len() as u16).to_le_bytes());
            out.extend_from_slice(ctx.as_bytes());
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
        let bytes = make_bytes(&[(7, 1_700_000_000_000_000, "GDPR-erasure", "ticket #42")]);
        let m = RedactionManifest::parse(&bytes).unwrap();
        let e = m.entry(7).unwrap();
        assert_eq!(e.reason_code, "GDPR-erasure");
        assert_eq!(e.audit_context, "ticket #42");
        assert_eq!(e.created_at_us, 1_700_000_000_000_000);
    }

    #[test]
    fn truncated_rejected() {
        let mut bytes = 1u32.to_le_bytes().to_vec();
        bytes.extend_from_slice(&0u64.to_le_bytes()); // missing rest
        assert!(matches!(
            RedactionManifest::parse(&bytes),
            Err(QfError::BufferTooShort)
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
            Err(QfError::RedactionPolicy)
        );
    }
}
