//! DataFusion 53.x file-format integration path.

use std::{any::Any, collections::HashMap, sync::Arc};

use arrow_schema::SchemaRef;
use async_trait::async_trait;
use cove_arrow::arrow::{
    ArrowDictionaryPolicy, ArrowStringValidationPolicy, ArrowVarBytesExportPolicy,
};
use datafusion::{
    catalog::{Session, TableProvider, TableProviderFactory},
    common::{stats::Precision, DataFusionError, GetExt, Result, Statistics},
    datasource::{
        file_format::{file_compression_type::FileCompressionType, FileFormat, FileFormatFactory},
        listing_table_factory::ListingTableFactory,
    },
    logical_expr::CreateExternalTable,
    object_store::{ObjectMeta, ObjectStore},
    physical_plan::{metrics::ExecutionPlanMetricsSet, ExecutionPlan},
};
use datafusion_datasource::{
    file::FileSource, file_scan_config::FileScanConfig, source::DataSourceExec, TableSchema,
};

use crate::{
    adapter_v53::{
        cove_to_datafusion, file_opener::ObjectStoreRangeReader, file_source::CoveFileSource,
        metrics::CoveFileMetrics, writer::CoveFileSink,
    },
    bootstrap::{bootstrap_range_reader_with_options, CoveMetadataCache},
    dataset_state::DatasetState,
    options::{
        CoveTableOptions, CoverageCacheDiscovery, CoviDiscovery, CovxDiscovery,
        ExecutionCodePolicy, FilterResidualPolicy,
    },
};

#[derive(Debug, Clone)]
pub struct CoveFileFormat {
    options: CoveTableOptions,
    cache: Arc<CoveMetadataCache>,
}

impl CoveFileFormat {
    pub fn new(options: CoveTableOptions) -> Self {
        Self {
            options,
            cache: Arc::new(CoveMetadataCache::default()),
        }
    }

    pub fn options(&self) -> CoveTableOptions {
        self.options.clone()
    }

    pub fn cache(&self) -> Arc<CoveMetadataCache> {
        Arc::clone(&self.cache)
    }

    async fn state_for_object(
        &self,
        store: &Arc<dyn ObjectStore>,
        object: &ObjectMeta,
    ) -> Result<Arc<crate::dataset_state::DatasetState>> {
        let metrics_set = ExecutionPlanMetricsSet::new();
        let metrics = CoveFileMetrics::new(&metrics_set, 0);
        let reader =
            ObjectStoreRangeReader::new(Arc::clone(store), object.location.clone(), metrics);
        bootstrap_range_reader_with_options(
            object.location.to_string(),
            object.size,
            &reader,
            self.options.clone(),
            Some(self.cache.as_ref()),
        )
        .await
        .map_err(cove_to_datafusion)
    }

    async fn states_for_config(
        &self,
        store: &Arc<dyn ObjectStore>,
        conf: &FileScanConfig,
    ) -> Result<Arc<Vec<Arc<DatasetState>>>> {
        let mut states = Vec::new();
        for group in &conf.file_groups {
            for file in group.iter() {
                states.push(self.state_for_object(store, &file.object_meta).await?);
            }
        }
        Ok(Arc::new(states))
    }
}

impl Default for CoveFileFormat {
    fn default() -> Self {
        Self::new(CoveTableOptions::default())
    }
}

#[async_trait]
impl FileFormat for CoveFileFormat {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn get_ext(&self) -> String {
        "cove".into()
    }

    fn get_ext_with_compression(
        &self,
        file_compression_type: &FileCompressionType,
    ) -> Result<String> {
        Ok(format!(
            "{}{}",
            self.get_ext(),
            file_compression_type.get_ext()
        ))
    }

    fn compression_type(&self) -> Option<FileCompressionType> {
        None
    }

    async fn infer_schema(
        &self,
        _state: &dyn Session,
        store: &Arc<dyn ObjectStore>,
        objects: &[ObjectMeta],
    ) -> Result<SchemaRef> {
        let Some(first) = objects.first() else {
            return Err(DataFusionError::Plan(
                "COVE DataFusion v2 adapter cannot infer schema from an empty listing".into(),
            ));
        };
        let first_state = self.state_for_object(store, first).await?;
        let schema = first_state.schema();
        for object in &objects[1..] {
            let state = self.state_for_object(store, object).await?;
            if state.schema().as_ref() != schema.as_ref() {
                return Err(DataFusionError::Plan(format!(
                    "COVE DataFusion v2 adapter schema mismatch between {} and {}",
                    first_state.source(),
                    state.source()
                )));
            }
        }
        Ok(schema)
    }

    async fn infer_stats(
        &self,
        _state: &dyn Session,
        store: &Arc<dyn ObjectStore>,
        table_schema: SchemaRef,
        object: &ObjectMeta,
    ) -> Result<Statistics> {
        let state = self.state_for_object(store, object).await?;
        let mut statistics = Statistics::new_unknown(table_schema.as_ref());
        statistics.num_rows = Precision::Exact(state.table().row_count as usize);
        statistics.calculate_total_byte_size(table_schema.as_ref());
        Ok(statistics)
    }

    async fn create_physical_plan(
        &self,
        state: &dyn Session,
        mut conf: FileScanConfig,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        if self.options.filter_residual_policy() == FilterResidualPolicy::ElideExactWhenProven {
            let store = state.runtime_env().object_store(&conf.object_store_url)?;
            let states = self.states_for_config(&store, &conf).await?;
            if let Some(source) = conf.file_source.as_any().downcast_ref::<CoveFileSource>() {
                conf.file_source = Arc::new(source.clone().with_exactness_states(states));
            }
        }
        Ok(DataSourceExec::from_data_source(conf) as Arc<dyn ExecutionPlan>)
    }

    async fn create_writer_physical_plan(
        &self,
        input: Arc<dyn ExecutionPlan>,
        _state: &dyn Session,
        conf: datafusion_datasource::file_sink_config::FileSinkConfig,
        order_requirements: Option<datafusion::physical_expr_common::sort_expr::LexRequirement>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        let sink = Arc::new(CoveFileSink::new(conf));
        Ok(Arc::new(datafusion_datasource::sink::DataSinkExec::new(
            input,
            sink,
            order_requirements,
        )))
    }

    fn file_source(&self, table_schema: TableSchema) -> Arc<dyn FileSource> {
        Arc::new(CoveFileSource::new(
            table_schema,
            self.options.clone(),
            Arc::clone(&self.cache),
        ))
    }
}

#[derive(Debug, Default)]
pub struct CoveFormatFactory;

impl GetExt for CoveFormatFactory {
    fn get_ext(&self) -> String {
        "cove".into()
    }
}

impl FileFormatFactory for CoveFormatFactory {
    fn create(
        &self,
        _state: &dyn Session,
        format_options: &HashMap<String, String>,
    ) -> Result<Arc<dyn FileFormat>> {
        let options = parse_format_options(format_options)?;
        Ok(Arc::new(CoveFileFormat::new(options)))
    }

    fn default(&self) -> Arc<dyn FileFormat> {
        Arc::new(CoveFileFormat::default())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug, Default)]
pub struct CoveTableFactory {
    listing: ListingTableFactory,
}

impl CoveTableFactory {
    pub fn new() -> Self {
        Self {
            listing: ListingTableFactory::new(),
        }
    }
}

#[async_trait]
impl TableProviderFactory for CoveTableFactory {
    async fn create(
        &self,
        state: &dyn Session,
        cmd: &CreateExternalTable,
    ) -> Result<Arc<dyn TableProvider>> {
        if cmd.unbounded {
            return Err(DataFusionError::Plan(
                "COVE DataFusion v2 SQL external tables are bounded only".into(),
            ));
        }
        self.listing.create(state, cmd).await
    }
}

fn parse_format_options(format_options: &HashMap<String, String>) -> Result<CoveTableOptions> {
    let mut options = CoveTableOptions::default();
    let mut range_max_gap = options.range_coalescing().max_gap;
    let mut range_max_span = options.range_coalescing().max_span;
    let mut selected_table_id = None;
    let mut selected_table_name = None;
    let mut selected_table_namespace = None;
    for (key, value) in format_options {
        let key = key.trim().to_ascii_lowercase();
        let raw_value = value.trim();
        let value = raw_value.to_ascii_lowercase();
        match key.as_str() {
            "cove.filter_residual_policy" | "filter_residual_policy" => {
                options = options.with_filter_residual_policy(match value.as_str() {
                    "preserve_all" | "preserve" => FilterResidualPolicy::PreserveAll,
                    "elide_exact_when_proven" | "elide_exact" => {
                        FilterResidualPolicy::ElideExactWhenProven
                    }
                    _ => return invalid_option_value(&key, value.as_str()),
                });
            }
            "cove.arrow_output" | "arrow_output" => {
                let mut arrow = options.arrow_export_options();
                match value.as_str() {
                    "standard" => {
                        arrow.varbytes_policy = ArrowVarBytesExportPolicy::Standard;
                        arrow.dictionary_policy = ArrowDictionaryPolicy::DecodeValues;
                    }
                    "view" => {
                        arrow.varbytes_policy = ArrowVarBytesExportPolicy::View;
                        arrow.dictionary_policy = ArrowDictionaryPolicy::DecodeValues;
                    }
                    "dictionary" | "dictionary_keys" => {
                        arrow.dictionary_policy = ArrowDictionaryPolicy::DictionaryKeys;
                    }
                    _ => return invalid_option_value(&key, value.as_str()),
                }
                options = options.with_arrow_export_options(arrow);
            }
            "cove.arrow_string_validation" | "arrow_string_validation" => {
                let mut arrow = options.arrow_export_options();
                arrow.string_validation_policy = match value.as_str() {
                    "strict" => ArrowStringValidationPolicy::Strict,
                    "strict_or_cached_proof" | "cached_proof" => {
                        ArrowStringValidationPolicy::StrictOrCachedProof
                    }
                    "trusted_page_proof" | "trusted" => {
                        ArrowStringValidationPolicy::TrustedPageProof
                    }
                    _ => return invalid_option_value(&key, value.as_str()),
                };
                options = options.with_arrow_export_options(arrow);
            }
            "cove.page_payload_validation" | "page_payload_validation" => {
                options = match value.as_str() {
                    "trusted" => options.with_trusted_page_payload_validation(),
                    "strict" => options.with_strict_page_payload_validation(),
                    _ => return invalid_option_value(&key, value.as_str()),
                };
            }
            "cove.local_file_read" | "local_file_read" => {
                options = match value.as_str() {
                    "mmap" => options.with_local_file_mmap_reads(),
                    "positioned" | "positioned_reads" => options.with_positioned_local_file_reads(),
                    _ => return invalid_option_value(&key, value.as_str()),
                };
            }
            "cove.range_coalescing_max_gap" | "range_coalescing_max_gap" => {
                range_max_gap = parse_u64_option(&key, value.as_str())?;
                options = options.with_range_coalescing(range_max_gap, range_max_span);
            }
            "cove.range_coalescing_max_span" | "range_coalescing_max_span" => {
                range_max_span = parse_u64_option(&key, value.as_str())?;
                options = options.with_range_coalescing(range_max_gap, range_max_span);
            }
            "cove.covx_discovery" | "covx_discovery" => {
                options = options.with_covx_discovery(match value.as_str() {
                    "disabled" => CovxDiscovery::Disabled,
                    "sibling" | "sibling_extension" => CovxDiscovery::SiblingExtension,
                    _ => return invalid_option_value(&key, value.as_str()),
                });
            }
            "cove.covi_discovery" | "covi_discovery" => {
                options = options.with_covi_discovery(match value.as_str() {
                    "disabled" => CoviDiscovery::Disabled,
                    "sibling" | "sibling_extension" => CoviDiscovery::SiblingExtension,
                    _ => return invalid_option_value(&key, value.as_str()),
                });
            }
            "cove.coverage_cache" | "coverage_cache" => {
                options = options.with_coverage_cache_discovery(match value.as_str() {
                    "disabled" => CoverageCacheDiscovery::Disabled,
                    "sibling-diagnostic" | "sibling_diagnostic" | "sibling" => {
                        CoverageCacheDiscovery::SiblingDiagnostic
                    }
                    _ => return invalid_option_value(&key, value.as_str()),
                });
            }
            "cove.execution_code_policy" | "execution_code_policy" => {
                options = options.with_execution_code_policy(match value.as_str() {
                    "disabled" => ExecutionCodePolicy::Disabled,
                    "opportunistic" => ExecutionCodePolicy::Opportunistic,
                    "require_supported" | "required" => ExecutionCodePolicy::RequireSupported,
                    _ => return invalid_option_value(&key, value.as_str()),
                });
            }
            "cove.target_morsels_per_partition" | "target_morsels_per_partition" => {
                options = options
                    .with_target_morsels_per_partition(parse_usize_option(&key, value.as_str())?);
            }
            "cove.table_id" | "table_id" => {
                selected_table_id = Some(parse_u32_option(&key, value.as_str())?);
            }
            "cove.table_name" | "table_name" => {
                if raw_value.is_empty() {
                    return invalid_option_value(&key, raw_value);
                }
                selected_table_name = Some(raw_value.to_string());
            }
            "cove.table_namespace" | "table_namespace" => {
                if raw_value.is_empty() {
                    return invalid_option_value(&key, raw_value);
                }
                selected_table_namespace = Some(raw_value.to_string());
            }
            _ => {
                return Err(DataFusionError::Plan(format!(
                    "COVE DataFusion v2 does not support SQL format option: {key}"
                )));
            }
        }
    }
    match (selected_table_id, selected_table_name) {
        (Some(table_id), None) => {
            options = options.with_table_id(table_id);
        }
        (None, Some(name)) => {
            options = options.with_table_name(selected_table_namespace, name);
        }
        (Some(_), Some(_)) => {
            return Err(DataFusionError::Plan(
                "COVE DataFusion SQL format options cannot set both cove.table_id and cove.table_name"
                    .into(),
            ));
        }
        (None, None) => {
            if selected_table_namespace.is_some() {
                return Err(DataFusionError::Plan(
                    "COVE DataFusion SQL format option cove.table_namespace requires cove.table_name"
                        .into(),
                ));
            }
        }
    }
    Ok(options)
}

fn parse_u32_option(key: &str, value: &str) -> Result<u32> {
    value.parse::<u32>().map_err(|_| {
        DataFusionError::Plan(format!(
            "invalid COVE SQL format option value for {key}: {value}"
        ))
    })
}

fn parse_u64_option(key: &str, value: &str) -> Result<u64> {
    value.parse::<u64>().map_err(|_| {
        DataFusionError::Plan(format!(
            "invalid COVE SQL format option value for {key}: {value}"
        ))
    })
}

fn parse_usize_option(key: &str, value: &str) -> Result<usize> {
    value.parse::<usize>().map_err(|_| {
        DataFusionError::Plan(format!(
            "invalid COVE SQL format option value for {key}: {value}"
        ))
    })
}

fn invalid_option_value<T>(key: &str, value: &str) -> Result<T> {
    Err(DataFusionError::Plan(format!(
        "invalid COVE SQL format option value for {key}: {value}"
    )))
}
