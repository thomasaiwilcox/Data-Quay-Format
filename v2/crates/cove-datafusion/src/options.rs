//! Public options for COVE-backed DataFusion table registration.

use cove_arrow::arrow::{
    ArrowDictionaryPolicy, ArrowExportOptions, ArrowStringValidationPolicy,
    ArrowVarBytesExportPolicy,
};
use cove_core::{
    table::{TableCatalog, TableEntry},
    CoveError,
};

use crate::range_reader::RangeCoalescingOptions;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoveTableOptions {
    arrow_export_options: ArrowExportOptions,
    table_selection: Option<CoveTableSelection>,
    covm_trust_policy: CovmTrustPolicy,
    sidecar_digest_policy: SidecarDigestPolicy,
    covx_discovery: CovxDiscovery,
    covi_discovery: CoviDiscovery,
    coverage_cache_discovery: CoverageCacheDiscovery,
    execution_code_policy: ExecutionCodePolicy,
    page_payload_validation_policy: PagePayloadValidationPolicy,
    local_file_read_policy: LocalFileReadPolicy,
    filter_residual_policy: FilterResidualPolicy,
    target_morsels_per_partition: usize,
    range_coalescing: RangeCoalescingOptions,
    #[cfg(feature = "dynamic-filters")]
    dynamic_filters_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CoveTableSelection {
    Id(u32),
    Name {
        namespace: Option<String>,
        name: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CovmTrustPolicy {
    Conservative,
    CachedFreshness,
    ExternalCatalogTrusted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidecarDigestPolicy {
    RequireFreshFingerprint,
    FullFileDigestOnDemand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CovxDiscovery {
    Disabled,
    SiblingExtension,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoviDiscovery {
    Disabled,
    SiblingExtension,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverageCacheDiscovery {
    Disabled,
    SiblingDiagnostic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionCodePolicy {
    Disabled,
    Opportunistic,
    RequireSupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PagePayloadValidationPolicy {
    Trusted,
    Strict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LocalFileReadPolicy {
    PositionedReads,
    Mmap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FilterResidualPolicy {
    PreserveAll,
    ElideExactWhenProven,
}

impl Default for CoveTableOptions {
    fn default() -> Self {
        Self {
            arrow_export_options: ArrowExportOptions {
                string_validation_policy: ArrowStringValidationPolicy::StrictOrCachedProof,
                ..ArrowExportOptions::default()
            },
            table_selection: None,
            covm_trust_policy: CovmTrustPolicy::Conservative,
            sidecar_digest_policy: SidecarDigestPolicy::RequireFreshFingerprint,
            covx_discovery: default_covx_discovery(),
            covi_discovery: default_covi_discovery(),
            coverage_cache_discovery: CoverageCacheDiscovery::Disabled,
            execution_code_policy: ExecutionCodePolicy::Opportunistic,
            page_payload_validation_policy: PagePayloadValidationPolicy::Trusted,
            local_file_read_policy: LocalFileReadPolicy::Mmap,
            filter_residual_policy: FilterResidualPolicy::PreserveAll,
            target_morsels_per_partition: 128,
            range_coalescing: RangeCoalescingOptions::default(),
            #[cfg(feature = "dynamic-filters")]
            dynamic_filters_enabled: false,
        }
    }
}

impl CoveTableOptions {
    pub fn arrow_export_options(&self) -> ArrowExportOptions {
        self.arrow_export_options
    }

    pub fn with_arrow_export_options(mut self, options: ArrowExportOptions) -> Self {
        self.arrow_export_options = options;
        self
    }

    pub fn with_arrow_dictionary_output(mut self) -> Self {
        self.arrow_export_options.dictionary_policy = ArrowDictionaryPolicy::DictionaryKeys;
        self
    }

    pub fn with_arrow_view_output(mut self) -> Self {
        self.arrow_export_options.varbytes_policy = ArrowVarBytesExportPolicy::View;
        self
    }

    pub fn with_standard_arrow_varbytes_output(mut self) -> Self {
        self.arrow_export_options.varbytes_policy = ArrowVarBytesExportPolicy::Standard;
        self
    }

    pub fn with_trusted_arrow_string_validation(mut self) -> Self {
        self.arrow_export_options.string_validation_policy =
            ArrowStringValidationPolicy::TrustedPageProof;
        self
    }

    pub fn with_strict_arrow_string_validation(mut self) -> Self {
        self.arrow_export_options.string_validation_policy = ArrowStringValidationPolicy::Strict;
        self
    }

    pub fn with_cached_proof_arrow_string_validation(mut self) -> Self {
        self.arrow_export_options.string_validation_policy =
            ArrowStringValidationPolicy::StrictOrCachedProof;
        self
    }

    pub fn table_selection(&self) -> Option<&CoveTableSelection> {
        self.table_selection.as_ref()
    }

    pub fn with_table_id(mut self, table_id: u32) -> Self {
        self.table_selection = Some(CoveTableSelection::Id(table_id));
        self
    }

    pub fn with_table_name(mut self, namespace: Option<String>, name: String) -> Self {
        self.table_selection = Some(CoveTableSelection::Name { namespace, name });
        self
    }

    pub fn covm_trust_policy(&self) -> CovmTrustPolicy {
        self.covm_trust_policy
    }

    pub fn with_covm_trust_policy(mut self, policy: CovmTrustPolicy) -> Self {
        self.covm_trust_policy = policy;
        self
    }

    pub fn sidecar_digest_policy(&self) -> SidecarDigestPolicy {
        self.sidecar_digest_policy
    }

    pub fn with_sidecar_digest_policy(mut self, policy: SidecarDigestPolicy) -> Self {
        self.sidecar_digest_policy = policy;
        self
    }

    pub fn covx_discovery(&self) -> CovxDiscovery {
        self.covx_discovery
    }

    pub fn with_covx_discovery(mut self, discovery: CovxDiscovery) -> Self {
        self.covx_discovery = discovery;
        self
    }

    pub fn covi_discovery(&self) -> CoviDiscovery {
        self.covi_discovery
    }

    pub fn with_covi_discovery(mut self, discovery: CoviDiscovery) -> Self {
        self.covi_discovery = discovery;
        self
    }

    pub fn coverage_cache_discovery(&self) -> CoverageCacheDiscovery {
        self.coverage_cache_discovery
    }

    pub fn with_coverage_cache_discovery(mut self, discovery: CoverageCacheDiscovery) -> Self {
        self.coverage_cache_discovery = discovery;
        self
    }

    pub fn with_sibling_coverage_cache(self) -> Self {
        self.with_coverage_cache_discovery(CoverageCacheDiscovery::SiblingDiagnostic)
    }

    pub fn execution_code_policy(&self) -> ExecutionCodePolicy {
        self.execution_code_policy
    }

    pub fn with_execution_code_policy(mut self, policy: ExecutionCodePolicy) -> Self {
        self.execution_code_policy = policy;
        self
    }

    pub fn page_payload_validation_policy(&self) -> PagePayloadValidationPolicy {
        self.page_payload_validation_policy
    }

    pub fn with_trusted_page_payload_validation(mut self) -> Self {
        self.page_payload_validation_policy = PagePayloadValidationPolicy::Trusted;
        self
    }

    pub fn with_strict_page_payload_validation(mut self) -> Self {
        self.page_payload_validation_policy = PagePayloadValidationPolicy::Strict;
        self
    }

    pub fn local_file_read_policy(&self) -> LocalFileReadPolicy {
        self.local_file_read_policy
    }

    pub fn with_local_file_mmap_reads(mut self) -> Self {
        self.local_file_read_policy = LocalFileReadPolicy::Mmap;
        self
    }

    /// Enable the two fastest knobs measured for trusted immutable local files:
    /// skip Arrow UTF-8 revalidation and use mmap-backed local reads.
    ///
    /// This is only appropriate when every non-null Utf8 row slice is already
    /// known to be valid UTF-8 and the local file will not be concurrently
    /// replaced, truncated, or modified while it is being scanned.
    pub fn with_trusted_arrow_string_validation_and_local_file_mmap_reads(self) -> Self {
        self.with_trusted_arrow_string_validation()
            .with_local_file_mmap_reads()
    }

    pub fn with_positioned_local_file_reads(mut self) -> Self {
        self.local_file_read_policy = LocalFileReadPolicy::PositionedReads;
        self
    }

    pub fn filter_residual_policy(&self) -> FilterResidualPolicy {
        self.filter_residual_policy
    }

    pub fn with_filter_residual_policy(mut self, policy: FilterResidualPolicy) -> Self {
        self.filter_residual_policy = policy;
        self
    }

    pub fn target_morsels_per_partition(&self) -> usize {
        self.target_morsels_per_partition
    }

    pub fn with_target_morsels_per_partition(mut self, target: usize) -> Self {
        self.target_morsels_per_partition = target.max(1);
        self
    }

    pub fn range_coalescing(&self) -> RangeCoalescingOptions {
        self.range_coalescing
    }

    pub fn with_range_coalescing(mut self, max_gap: u64, max_span: u64) -> Self {
        self.range_coalescing = RangeCoalescingOptions { max_gap, max_span };
        self
    }

    #[cfg(feature = "dynamic-filters")]
    pub fn dynamic_filters_enabled(&self) -> bool {
        self.dynamic_filters_enabled
    }

    #[cfg(not(feature = "dynamic-filters"))]
    pub fn dynamic_filters_enabled(&self) -> bool {
        false
    }

    #[cfg(feature = "dynamic-filters")]
    pub fn with_dynamic_filters_enabled(mut self, enabled: bool) -> Self {
        self.dynamic_filters_enabled = enabled;
        self
    }
}

pub(crate) fn select_table(
    catalog: &TableCatalog,
    selection: Option<&CoveTableSelection>,
) -> Result<TableEntry, CoveError> {
    match selection {
        Some(CoveTableSelection::Id(table_id)) => catalog
            .tables
            .iter()
            .find(|table| table.table_id == *table_id)
            .cloned()
            .ok_or_else(|| {
                CoveError::BadSchema(format!(
                    "COVE DataFusion selected table_id {table_id} not found"
                ))
            }),
        Some(CoveTableSelection::Name { namespace, name }) => {
            let matches = catalog
                .tables
                .iter()
                .filter(|table| {
                    table.name == *name
                        && namespace
                            .as_ref()
                            .map(|expected| table.namespace == *expected)
                            .unwrap_or(true)
                })
                .cloned()
                .collect::<Vec<_>>();
            match matches.len() {
                1 => Ok(matches[0].clone()),
                0 => Err(CoveError::BadSchema(match namespace {
                    Some(namespace) => format!(
                        "COVE DataFusion selected table {namespace}.{name} not found"
                    ),
                    None => format!("COVE DataFusion selected table {name} not found"),
                })),
                _ => Err(CoveError::BadSchema(format!(
                    "COVE DataFusion selected table name {name} is ambiguous; set cove.table_namespace or cove.table_id"
                ))),
            }
        }
        None => match catalog.tables.len() {
            1 => Ok(catalog.tables[0].clone()),
            count => Err(CoveError::BadSchema(format!(
                "COVE DataFusion requires cove.table_id or cove.table_name for multi-table files, found {count}"
            ))),
        },
    }
}

fn default_covx_discovery() -> CovxDiscovery {
    if cfg!(feature = "covx") {
        CovxDiscovery::SiblingExtension
    } else {
        CovxDiscovery::Disabled
    }
}

fn default_covi_discovery() -> CoviDiscovery {
    if cfg!(feature = "covi") {
        CoviDiscovery::SiblingExtension
    } else {
        CoviDiscovery::Disabled
    }
}
