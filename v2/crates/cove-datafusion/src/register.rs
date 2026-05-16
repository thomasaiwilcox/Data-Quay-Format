//! Public registration helpers and thin session glue.

use std::{path::Path, sync::Arc};

use datafusion::{
    common::Result, datasource::listing::ListingOptions, execution::context::SessionContext,
};

use crate::{
    adapter_v53::{
        cove_to_datafusion,
        file_format::{CoveFileFormat, CoveFormatFactory, CoveTableFactory},
        optimizer::install_cove_optimizer,
        table_provider::CoveTableProvider,
    },
    bootstrap::{
        bootstrap_local_file, bootstrap_local_file_async, bootstrap_local_file_with_options,
        bootstrap_local_file_with_options_async, bootstrap_overlay_snapshot_with_options,
        bootstrap_overlay_snapshot_with_options_async,
    },
    overlay::CoveOverlaySnapshot,
};

#[cfg(feature = "covm")]
use crate::bootstrap::{
    bootstrap_covm_local_file_with_options, bootstrap_covm_local_file_with_options_async,
};

pub use crate::options::{
    CoveTableOptions, CoviDiscovery, CovmTrustPolicy, CovxDiscovery, ExecutionCodePolicy,
    FilterResidualPolicy, SidecarDigestPolicy,
};

/// Build a DataFusion table provider for a local `.cove` file.
///
/// This synchronous convenience wrapper blocks the current thread.
pub fn cove_table_from_path(path: impl AsRef<Path>) -> Result<Arc<CoveTableProvider>> {
    let state = bootstrap_local_file(path).map_err(cove_to_datafusion)?;
    Ok(Arc::new(CoveTableProvider::new(state)))
}

/// Build a DataFusion table provider for a local `.cove` file.
pub async fn cove_table_from_path_async(path: impl AsRef<Path>) -> Result<Arc<CoveTableProvider>> {
    let state = bootstrap_local_file_async(path)
        .await
        .map_err(cove_to_datafusion)?;
    Ok(Arc::new(CoveTableProvider::new(state)))
}

/// Build a DataFusion table provider for a local `.cove` file with explicit
/// COVE table options.
///
/// This synchronous convenience wrapper blocks the current thread.
pub fn cove_table_from_path_with_options(
    path: impl AsRef<Path>,
    options: CoveTableOptions,
) -> Result<Arc<CoveTableProvider>> {
    let state = bootstrap_local_file_with_options(path, options).map_err(cove_to_datafusion)?;
    Ok(Arc::new(CoveTableProvider::new(state)))
}

/// Build a DataFusion table provider for a local `.cove` file with explicit
/// COVE table options.
pub async fn cove_table_from_path_with_options_async(
    path: impl AsRef<Path>,
    options: CoveTableOptions,
) -> Result<Arc<CoveTableProvider>> {
    let state = bootstrap_local_file_with_options_async(path, options)
        .await
        .map_err(cove_to_datafusion)?;
    Ok(Arc::new(CoveTableProvider::new(state)))
}

/// Build a DataFusion table provider for an overlay snapshot.
pub fn cove_table_from_overlay_snapshot(
    snapshot: CoveOverlaySnapshot,
    options: CoveTableOptions,
) -> Result<Arc<CoveTableProvider>> {
    let state =
        bootstrap_overlay_snapshot_with_options(snapshot, options).map_err(cove_to_datafusion)?;
    Ok(Arc::new(CoveTableProvider::new(state)))
}

/// Build a DataFusion table provider for an overlay snapshot.
pub async fn cove_table_from_overlay_snapshot_async(
    snapshot: CoveOverlaySnapshot,
    options: CoveTableOptions,
) -> Result<Arc<CoveTableProvider>> {
    let state = bootstrap_overlay_snapshot_with_options_async(snapshot, options)
        .await
        .map_err(cove_to_datafusion)?;
    Ok(Arc::new(CoveTableProvider::new(state)))
}

#[cfg(feature = "covm")]
pub fn cove_table_from_covm_path(path: impl AsRef<Path>) -> Result<Arc<CoveTableProvider>> {
    cove_table_from_covm_path_with_options(path, CoveTableOptions::default())
}

#[cfg(feature = "covm")]
pub async fn cove_table_from_covm_path_async(
    path: impl AsRef<Path>,
) -> Result<Arc<CoveTableProvider>> {
    cove_table_from_covm_path_with_options_async(path, CoveTableOptions::default()).await
}

#[cfg(feature = "covm")]
pub fn cove_table_from_covm_path_with_options(
    path: impl AsRef<Path>,
    options: CoveTableOptions,
) -> Result<Arc<CoveTableProvider>> {
    let state =
        bootstrap_covm_local_file_with_options(path, options).map_err(cove_to_datafusion)?;
    Ok(Arc::new(CoveTableProvider::new(state)))
}

#[cfg(feature = "covm")]
pub async fn cove_table_from_covm_path_with_options_async(
    path: impl AsRef<Path>,
    options: CoveTableOptions,
) -> Result<Arc<CoveTableProvider>> {
    let state = bootstrap_covm_local_file_with_options_async(path, options)
        .await
        .map_err(cove_to_datafusion)?;
    Ok(Arc::new(CoveTableProvider::new(state)))
}

/// Register a local `.cove` file as a DataFusion table.
///
/// This synchronous convenience wrapper blocks the current thread while it
/// builds the table provider.
pub fn register_cove_file(
    ctx: &SessionContext,
    table_name: &str,
    path: impl AsRef<Path>,
) -> Result<Arc<CoveTableProvider>> {
    install_cove_optimizer(ctx);
    let provider = cove_table_from_path(path)?;
    ctx.register_table(table_name, provider.clone())?;
    Ok(provider)
}

/// Register a local `.cove` file as a DataFusion table.
pub async fn register_cove_file_async(
    ctx: &SessionContext,
    table_name: &str,
    path: impl AsRef<Path>,
) -> Result<Arc<CoveTableProvider>> {
    install_cove_optimizer(ctx);
    let provider = cove_table_from_path_async(path).await?;
    ctx.register_table(table_name, provider.clone())?;
    Ok(provider)
}

/// Register a local `.cove` file as a DataFusion table with explicit COVE
/// table options.
///
/// This synchronous convenience wrapper blocks the current thread while it
/// builds the table provider.
pub fn register_cove_file_with_options(
    ctx: &SessionContext,
    table_name: &str,
    path: impl AsRef<Path>,
    options: CoveTableOptions,
) -> Result<Arc<CoveTableProvider>> {
    install_cove_optimizer(ctx);
    let provider = cove_table_from_path_with_options(path, options)?;
    ctx.register_table(table_name, provider.clone())?;
    Ok(provider)
}

/// Register a local `.cove` file as a DataFusion table with explicit COVE
/// table options.
pub async fn register_cove_file_with_options_async(
    ctx: &SessionContext,
    table_name: &str,
    path: impl AsRef<Path>,
    options: CoveTableOptions,
) -> Result<Arc<CoveTableProvider>> {
    install_cove_optimizer(ctx);
    let provider = cove_table_from_path_with_options_async(path, options).await?;
    ctx.register_table(table_name, provider.clone())?;
    Ok(provider)
}

pub fn register_cove_overlay_snapshot(
    ctx: &SessionContext,
    table_name: &str,
    snapshot: CoveOverlaySnapshot,
    options: CoveTableOptions,
) -> Result<Arc<CoveTableProvider>> {
    install_cove_optimizer(ctx);
    let provider = cove_table_from_overlay_snapshot(snapshot, options)?;
    ctx.register_table(table_name, provider.clone())?;
    Ok(provider)
}

pub async fn register_cove_overlay_snapshot_async(
    ctx: &SessionContext,
    table_name: &str,
    snapshot: CoveOverlaySnapshot,
    options: CoveTableOptions,
) -> Result<Arc<CoveTableProvider>> {
    install_cove_optimizer(ctx);
    let provider = cove_table_from_overlay_snapshot_async(snapshot, options).await?;
    ctx.register_table(table_name, provider.clone())?;
    Ok(provider)
}

#[cfg(feature = "covm")]
pub fn register_cove_covm(
    ctx: &SessionContext,
    table_name: &str,
    path: impl AsRef<Path>,
) -> Result<Arc<CoveTableProvider>> {
    install_cove_optimizer(ctx);
    let provider = cove_table_from_covm_path(path)?;
    ctx.register_table(table_name, provider.clone())?;
    Ok(provider)
}

#[cfg(feature = "covm")]
pub async fn register_cove_covm_async(
    ctx: &SessionContext,
    table_name: &str,
    path: impl AsRef<Path>,
) -> Result<Arc<CoveTableProvider>> {
    install_cove_optimizer(ctx);
    let provider = cove_table_from_covm_path_async(path).await?;
    ctx.register_table(table_name, provider.clone())?;
    Ok(provider)
}

#[cfg(feature = "covm")]
pub fn register_cove_covm_with_options(
    ctx: &SessionContext,
    table_name: &str,
    path: impl AsRef<Path>,
    options: CoveTableOptions,
) -> Result<Arc<CoveTableProvider>> {
    install_cove_optimizer(ctx);
    let provider = cove_table_from_covm_path_with_options(path, options)?;
    ctx.register_table(table_name, provider.clone())?;
    Ok(provider)
}

#[cfg(feature = "covm")]
pub async fn register_cove_covm_with_options_async(
    ctx: &SessionContext,
    table_name: &str,
    path: impl AsRef<Path>,
    options: CoveTableOptions,
) -> Result<Arc<CoveTableProvider>> {
    install_cove_optimizer(ctx);
    let provider = cove_table_from_covm_path_with_options_async(path, options).await?;
    ctx.register_table(table_name, provider.clone())?;
    Ok(provider)
}

/// Build DataFusion listing options for `.cove` compatibility-mode datasets.
pub fn cove_listing_options(options: CoveTableOptions) -> ListingOptions {
    ListingOptions::new(Arc::new(CoveFileFormat::new(options))).with_file_extension("cove")
}

/// Register a directory, file, or object-store listing of `.cove` files through
/// DataFusion's file-format compatibility path.
pub async fn register_cove_listing_table(
    ctx: &SessionContext,
    table_name: &str,
    table_path: impl AsRef<str>,
) -> Result<()> {
    register_cove_listing_table_with_options(
        ctx,
        table_name,
        table_path,
        CoveTableOptions::default(),
    )
    .await
}

/// Register a `.cove` listing table with explicit COVE table options.
pub async fn register_cove_listing_table_with_options(
    ctx: &SessionContext,
    table_name: &str,
    table_path: impl AsRef<str>,
    options: CoveTableOptions,
) -> Result<()> {
    install_cove_optimizer(ctx);
    ctx.register_listing_table(
        table_name,
        table_path.as_ref(),
        cove_listing_options(options),
        None,
        None,
    )
    .await
}

/// Register COVE as a SQL external-table file format for this context.
///
/// After this call, DataFusion SQL can use `STORED AS COVE`.
pub fn register_cove_file_format(ctx: &SessionContext) -> Result<()> {
    install_cove_optimizer(ctx);
    let state_ref = ctx.state_ref();
    let mut state = state_ref.write();
    state.register_file_format(Arc::new(CoveFormatFactory), true)?;
    state
        .table_factories_mut()
        .insert("COVE".into(), Arc::new(CoveTableFactory::new()));
    Ok(())
}
