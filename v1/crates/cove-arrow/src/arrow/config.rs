use std::{panic::RefUnwindSafe, sync::Arc};

use arrow_buffer::alloc::Allocation;

use crate::constants::CoveLogicalType;

/// Policy for exporting FileCode-backed scalar columns to Arrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ArrowDictionaryPolicy {
    /// Decode FileCodes to their logical values before building the Arrow array.
    DecodeValues,
    /// Export FileCodes as Arrow dictionary keys when values are representable.
    DictionaryKeys,
}

impl Default for ArrowDictionaryPolicy {
    fn default() -> Self {
        Self::DictionaryKeys
    }
}

/// Policy for exporting COVE variable byte payloads to Arrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ArrowVarBytesExportPolicy {
    /// Materialise COVE length-prefixed bytes into standard Arrow Utf8/Binary
    /// offset/value buffers.
    Standard,
    /// Export COVE length-prefixed bytes as legal Arrow Utf8View/BinaryView
    /// arrays. The backing buffer must own or retain the COVE values bytes.
    View,
}

impl Default for ArrowVarBytesExportPolicy {
    fn default() -> Self {
        Self::Standard
    }
}

/// Policy for validating COVE byte payloads before constructing Arrow Utf8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ArrowStringValidationPolicy {
    /// Validate all materialized non-null rows while exporting.
    Strict,
    /// Validate on the first export, but allow an outer caller to replace this
    /// with [`ArrowStringValidationPolicy::TrustedPageProof`] once it has
    /// recorded an exact page-level proof. Inside `cove-arrow` this behaves the
    /// same as [`ArrowStringValidationPolicy::Strict`].
    StrictOrCachedProof,
    /// Trust a caller-supplied page-level proof that every non-null row slice is
    /// valid UTF-8.
    TrustedPageProof,
}

impl Default for ArrowStringValidationPolicy {
    fn default() -> Self {
        Self::Strict
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ArrowDecimalContext {
    pub precision: u8,
    pub scale: i8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ArrowFidelitySeverity {
    Informational,
    Lossy,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArrowFidelityIssue {
    pub field: Option<String>,
    pub logical_type: CoveLogicalType,
    pub severity: ArrowFidelitySeverity,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArrowExportReport {
    pub issues: Vec<ArrowFidelityIssue>,
}

impl ArrowExportReport {
    pub fn has_lossy_or_unsupported(&self) -> bool {
        self.issues.iter().any(|issue| {
            matches!(
                issue.severity,
                ArrowFidelitySeverity::Lossy | ArrowFidelitySeverity::Unsupported
            )
        })
    }

    pub(crate) fn push(
        &mut self,
        field: Option<&str>,
        logical_type: CoveLogicalType,
        severity: ArrowFidelitySeverity,
        message: impl Into<String>,
    ) {
        self.issues.push(ArrowFidelityIssue {
            field: field.map(ToOwned::to_owned),
            logical_type,
            severity,
            message: message.into(),
        });
    }

    pub(crate) fn extend_with_field(&mut self, field: &str, mut other: ArrowExportReport) {
        for issue in &mut other.issues {
            if issue.field.is_none() {
                issue.field = Some(field.to_string());
            }
        }
        self.issues.extend(other.issues);
    }
}

pub struct ArrowExportResult<T> {
    pub value: T,
    pub report: ArrowExportReport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ArrowExportOptions {
    pub dictionary_policy: ArrowDictionaryPolicy,
    pub varbytes_policy: ArrowVarBytesExportPolicy,
    pub string_validation_policy: ArrowStringValidationPolicy,
    pub decimal: Option<ArrowDecimalContext>,
    pub emit_uuid_extension_metadata: bool,
    pub emit_json_extension_metadata: bool,
}

impl Default for ArrowExportOptions {
    fn default() -> Self {
        Self {
            dictionary_policy: ArrowDictionaryPolicy::DecodeValues,
            varbytes_policy: ArrowVarBytesExportPolicy::Standard,
            string_validation_policy: ArrowStringValidationPolicy::Strict,
            decimal: None,
            emit_uuid_extension_metadata: false,
            emit_json_extension_metadata: false,
        }
    }
}

/// Owner for an Arrow buffer that points into an externally retained COVE byte
/// allocation.
pub type ArrowBufferOwner = Arc<dyn Allocation>;

/// Convert an owned COVE allocation into an Arrow buffer owner.
pub fn arrow_buffer_owner<T>(owner: Arc<T>) -> ArrowBufferOwner
where
    T: RefUnwindSafe + Send + Sync + 'static,
{
    owner
}
