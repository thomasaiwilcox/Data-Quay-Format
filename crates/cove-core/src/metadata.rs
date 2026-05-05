//! Cove Format (COVE) v1.0 — descriptive footer metadata JSON.
//!
//! Spec §15: footer metadata JSON is optional, descriptive, and non-authoritative.
//! Binary metadata remains the source of truth for offsets, schema, checksums,
//! section layout, and feature requirements.

use crate::{constants::METADATA_LEN_MAX, CoveError};

/// Parsed optional footer metadata JSON.
///
/// Spec §15: metadata bytes must be valid UTF-8 JSON when present and must not
/// exceed 1 MiB. The parsed value is intentionally not used by validation logic
/// that depends on authoritative binary structures.
#[derive(Debug, Clone, PartialEq)]
pub struct MetadataJson {
    raw: Vec<u8>,
    value: Option<serde_json::Value>,
}

impl MetadataJson {
    /// Parse and validate footer metadata bytes.
    ///
    /// Empty metadata is valid and has no parsed JSON value.
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        validate(bytes)?;
        let value = if bytes.is_empty() {
            None
        } else {
            Some(serde_json::from_slice(bytes).map_err(|_| {
                CoveError::BadSection(
                    "Spec §15: metadata_json must be syntactically valid JSON".to_string(),
                )
            })?)
        };
        Ok(Self {
            raw: bytes.to_vec(),
            value,
        })
    }

    /// Return the original metadata bytes exactly as stored in the footer.
    pub fn raw(&self) -> &[u8] {
        &self.raw
    }

    /// Return metadata as UTF-8 text.
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.raw).expect("MetadataJson validated UTF-8")
    }

    /// Return the parsed JSON value when metadata is present.
    pub fn value(&self) -> Option<&serde_json::Value> {
        self.value.as_ref()
    }

    /// Return true when the footer metadata payload is empty.
    pub fn is_empty(&self) -> bool {
        self.raw.is_empty()
    }
}

/// Validate metadata bytes without retaining a parsed value.
///
/// Spec §15: readers must ignore unknown metadata keys; this function only
/// verifies the generic wire constraints.
pub fn validate(bytes: &[u8]) -> Result<(), CoveError> {
    if bytes.len() > METADATA_LEN_MAX as usize {
        return Err(CoveError::BadSection(format!(
            "Spec §15: metadata_json length {} exceeds 1 MiB limit",
            bytes.len()
        )));
    }
    std::str::from_utf8(bytes).map_err(|_| {
        CoveError::BadSection("Spec §15: metadata_json must be valid UTF-8".to_string())
    })?;
    if !bytes.is_empty() && serde_json::from_slice::<serde_json::Value>(bytes).is_err() {
        return Err(CoveError::BadSection(
            "Spec §15: metadata_json must be syntactically valid JSON".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_15_empty_metadata_is_valid() {
        let metadata = MetadataJson::parse(&[]).unwrap();
        assert!(metadata.is_empty());
        assert!(metadata.value().is_none());
    }

    #[test]
    fn spec_15_metadata_json_is_descriptive_and_parsed_when_present() {
        let metadata = MetadataJson::parse(br#"{"format_version":"1.0","notes":{}}"#).unwrap();
        assert_eq!(metadata.as_str(), r#"{"format_version":"1.0","notes":{}}"#);
        assert!(metadata.value().unwrap().get("notes").is_some());
    }

    #[test]
    fn spec_15_metadata_rejects_non_utf8() {
        assert!(matches!(
            MetadataJson::parse(&[0xff]),
            Err(CoveError::BadSection(_))
        ));
    }

    #[test]
    fn spec_15_metadata_rejects_invalid_json() {
        assert!(matches!(
            MetadataJson::parse(b"{not-json"),
            Err(CoveError::BadSection(_))
        ));
    }
}
