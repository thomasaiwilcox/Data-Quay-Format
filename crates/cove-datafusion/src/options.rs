//! Public options for COVE-backed DataFusion table registration.

use cove_arrow::arrow::{ArrowDictionaryPolicy, ArrowExportOptions};

use crate::range_reader::RangeCoalescingOptions;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoveTableOptions {
    arrow_export_options: ArrowExportOptions,
    covm_trust_policy: CovmTrustPolicy,
    sidecar_digest_policy: SidecarDigestPolicy,
    covx_discovery: CovxDiscovery,
    execution_code_policy: ExecutionCodePolicy,
    target_morsels_per_partition: usize,
    range_coalescing: RangeCoalescingOptions,
    #[cfg(feature = "dynamic-filters")]
    dynamic_filters_enabled: bool,
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
pub enum ExecutionCodePolicy {
    Disabled,
    Opportunistic,
    RequireSupported,
}

impl Default for CoveTableOptions {
    fn default() -> Self {
        Self {
            arrow_export_options: ArrowExportOptions::default(),
            covm_trust_policy: CovmTrustPolicy::Conservative,
            sidecar_digest_policy: SidecarDigestPolicy::RequireFreshFingerprint,
            covx_discovery: default_covx_discovery(),
            execution_code_policy: ExecutionCodePolicy::Opportunistic,
            target_morsels_per_partition: 128,
            range_coalescing: RangeCoalescingOptions::default(),
            #[cfg(feature = "dynamic-filters")]
            dynamic_filters_enabled: false,
        }
    }
}

impl CoveTableOptions {
    pub fn arrow_export_options(self) -> ArrowExportOptions {
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

    pub fn covm_trust_policy(self) -> CovmTrustPolicy {
        self.covm_trust_policy
    }

    pub fn with_covm_trust_policy(mut self, policy: CovmTrustPolicy) -> Self {
        self.covm_trust_policy = policy;
        self
    }

    pub fn sidecar_digest_policy(self) -> SidecarDigestPolicy {
        self.sidecar_digest_policy
    }

    pub fn with_sidecar_digest_policy(mut self, policy: SidecarDigestPolicy) -> Self {
        self.sidecar_digest_policy = policy;
        self
    }

    pub fn covx_discovery(self) -> CovxDiscovery {
        self.covx_discovery
    }

    pub fn with_covx_discovery(mut self, discovery: CovxDiscovery) -> Self {
        self.covx_discovery = discovery;
        self
    }

    pub fn execution_code_policy(self) -> ExecutionCodePolicy {
        self.execution_code_policy
    }

    pub fn with_execution_code_policy(mut self, policy: ExecutionCodePolicy) -> Self {
        self.execution_code_policy = policy;
        self
    }

    pub fn target_morsels_per_partition(self) -> usize {
        self.target_morsels_per_partition
    }

    pub fn with_target_morsels_per_partition(mut self, target: usize) -> Self {
        self.target_morsels_per_partition = target.max(1);
        self
    }

    pub fn range_coalescing(self) -> RangeCoalescingOptions {
        self.range_coalescing
    }

    pub fn with_range_coalescing(mut self, max_gap: u64, max_span: u64) -> Self {
        self.range_coalescing = RangeCoalescingOptions { max_gap, max_span };
        self
    }

    #[cfg(feature = "dynamic-filters")]
    pub fn dynamic_filters_enabled(self) -> bool {
        self.dynamic_filters_enabled
    }

    #[cfg(not(feature = "dynamic-filters"))]
    pub fn dynamic_filters_enabled(self) -> bool {
        false
    }

    #[cfg(feature = "dynamic-filters")]
    pub fn with_dynamic_filters_enabled(mut self, enabled: bool) -> Self {
        self.dynamic_filters_enabled = enabled;
        self
    }
}

fn default_covx_discovery() -> CovxDiscovery {
    if cfg!(feature = "covx") {
        CovxDiscovery::SiblingExtension
    } else {
        CovxDiscovery::Disabled
    }
}
