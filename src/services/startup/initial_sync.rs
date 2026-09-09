use std::collections::BTreeMap;

use anyhow::Result;

use crate::{
    dto::manifest::RenderMeshManifest,
    services::{
        cdn_domains::OriginCdnDomains,
        cdn_tenants::OriginCdnTenants,
        origin_refresh::{OriginRefreshError, OriginRefreshService, OriginRefreshTrigger},
        origin_runtime::OriginRuntimeStore,
    },
};

use super::cdn::{reconcile_origin_cdn_domains, reconcile_origin_cdn_tenants};

pub(super) async fn sync_origins_at_startup(
    manifest: &RenderMeshManifest,
    origin_refresh: &OriginRefreshService,
    cdn_domains_by_origin: &BTreeMap<String, OriginCdnDomains>,
    cdn_tenants_by_origin: &BTreeMap<String, OriginCdnTenants>,
    origin_runtime: &OriginRuntimeStore,
) -> Result<()> {
    for origin_id in manifest.origins.keys() {
        let report = origin_refresh
            .refresh_origin(origin_id, OriginRefreshTrigger::Startup)
            .await
            .map_err(origin_refresh_error_to_anyhow)?;
        tracing::info!(
            origin = %origin_id,
            downloaded = report.downloaded_files,
            "initial origin sync completed"
        );
        if let Some(cdn_domains) = cdn_domains_by_origin.get(origin_id) {
            reconcile_origin_cdn_domains(origin_id, manifest, cdn_domains, origin_runtime).await;
        }
        if let Some(cdn_tenants) = cdn_tenants_by_origin.get(origin_id) {
            reconcile_origin_cdn_tenants(origin_id, manifest, cdn_tenants, origin_runtime).await;
        }
    }

    Ok(())
}

fn origin_refresh_error_to_anyhow(error: OriginRefreshError) -> anyhow::Error {
    match error {
        OriginRefreshError::NotFound => anyhow::anyhow!("origin not found during startup sync"),
        OriginRefreshError::AlreadyRunning => {
            anyhow::anyhow!("origin sync already running during startup")
        }
        OriginRefreshError::Failed(error) => error,
    }
}
