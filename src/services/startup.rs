use std::collections::BTreeMap;

use anyhow::Result;

mod background;
mod cdn;
mod initial_sync;
mod origins;

use crate::{
    repositories::{
        local_mirror::LocalMirrorRepository, manifest::ManifestRepository, sync::MirrorSyncService,
    },
    services::{
        cors::CorsPolicy,
        edge_config_store::EdgeConfigStore,
        manifest::{load_manifest, HostResolver},
        origin_refresh::{OriginFreshnessIndexes, OriginRefreshService},
        origin_runtime::OriginRuntimeStore,
        render_gateway::RenderGatewayService,
        template_store::TemplateStore,
    },
};

pub struct RenderRuntime {
    pub render_gateway: RenderGatewayService,
    pub origin_runtime: OriginRuntimeStore,
    pub origin_refresh: OriginRefreshService,
}

pub async fn build_render_gateway(manifest_path: &str) -> Result<RenderGatewayService> {
    Ok(build_render_runtime(manifest_path).await?.render_gateway)
}

pub async fn build_render_runtime(manifest_path: &str) -> Result<RenderRuntime> {
    let manifest = load_manifest(&ManifestRepository::new(), manifest_path).await?;
    let mirror = LocalMirrorRepository::new(&manifest.runtime.local_store_dir);
    let syncer = MirrorSyncService::new(&manifest.runtime.local_store_dir);
    let edge_configs = EdgeConfigStore::from_configs(BTreeMap::new());
    let template_store = TemplateStore::default();
    let freshness_indexes = OriginFreshnessIndexes::default();
    let origin_runtime = OriginRuntimeStore::default();
    let startup_origins = origins::build_startup_origins(&manifest, manifest_path).await?;
    let startup_cdn = cdn::build_startup_cdn(&manifest).await?;

    let origin_refresh = OriginRefreshService::new(
        syncer,
        edge_configs.clone(),
        template_store.clone(),
        freshness_indexes,
        origin_runtime.clone(),
        startup_origins.storage_by_origin,
        startup_origins.activation_barrier_by_origin,
        startup_cdn.refresh_by_origin,
    );

    initial_sync::sync_origins_at_startup(
        &manifest,
        &origin_refresh,
        &startup_cdn.domains_by_origin,
        &startup_cdn.tenants_by_origin,
        &origin_runtime,
    )
    .await?;

    background::spawn_background_sync(manifest.clone(), origin_refresh.clone());

    let render_gateway = RenderGatewayService::new_with_stores_origin_buckets_and_edge_contexts(
        HostResolver::new(&manifest)?,
        CorsPolicy::from_manifest(&manifest),
        mirror,
        edge_configs,
        template_store,
        startup_origins.origin_buckets,
        startup_origins.origin_edge_contexts,
    );

    Ok(RenderRuntime {
        render_gateway,
        origin_runtime,
        origin_refresh,
    })
}

#[cfg(test)]
mod tests;
