use std::collections::BTreeMap;

use anyhow::Result;

use crate::{
    dto::manifest::RenderMeshManifest,
    services::{
        cdn_domains::OriginCdnDomains, cdn_refresh::OriginCdnRefresh,
        origin_runtime::OriginRuntimeStore,
    },
};

pub(super) struct StartupCdn {
    pub refresh_by_origin: BTreeMap<String, OriginCdnRefresh>,
    pub domains_by_origin: BTreeMap<String, OriginCdnDomains>,
}

pub(super) async fn build_startup_cdn(manifest: &RenderMeshManifest) -> Result<StartupCdn> {
    Ok(StartupCdn {
        refresh_by_origin: build_origin_cdns(manifest).await?,
        domains_by_origin: build_origin_cdn_domains(manifest).await?,
    })
}

async fn build_origin_cdns(
    manifest: &RenderMeshManifest,
) -> Result<BTreeMap<String, OriginCdnRefresh>> {
    let mut output = BTreeMap::new();

    for (origin_id, origin) in &manifest.origins {
        let Some(config) = origin.cdn() else {
            continue;
        };
        let url_prefixes = exact_url_prefixes_for_origin(manifest, origin_id);
        output.insert(
            origin_id.clone(),
            OriginCdnRefresh::from_config(config, url_prefixes).await?,
        );
    }

    Ok(output)
}

async fn build_origin_cdn_domains(
    manifest: &RenderMeshManifest,
) -> Result<BTreeMap<String, OriginCdnDomains>> {
    let mut output = BTreeMap::new();

    for (origin_id, origin) in &manifest.origins {
        let Some(config) = origin.cdn() else {
            continue;
        };
        if let Some(domains) = OriginCdnDomains::from_config(config).await? {
            output.insert(origin_id.clone(), domains);
        }
    }

    Ok(output)
}

pub(super) async fn reconcile_origin_cdn_domains(
    origin_id: &str,
    manifest: &RenderMeshManifest,
    cdn_domains: &OriginCdnDomains,
    origin_runtime: &OriginRuntimeStore,
) {
    match cdn_domains.reconcile(manifest, origin_id).await {
        Ok(outcome) => {
            tracing::info!(
                origin = %origin_id,
                provider = %outcome.provider,
                status = %outcome.status,
                added = outcome.added,
                updated = outcome.updated,
                removed = outcome.removed,
                unchanged = outcome.unchanged,
                "cdn domain reconciliation submitted"
            );
            origin_runtime.set_cdn_domain_result(
                origin_id,
                outcome.provider,
                outcome.status,
                outcome.added,
                outcome.updated,
                outcome.removed,
                outcome.unchanged,
            );
        }
        Err(error) => {
            origin_runtime.set_cdn_domain_error(origin_id, error.to_string());
            tracing::error!(origin = %origin_id, "cdn domain reconciliation failed: {error}");
        }
    }
}

fn exact_url_prefixes_for_origin(manifest: &RenderMeshManifest, origin_id: &str) -> Vec<String> {
    manifest
        .hosts
        .iter()
        .filter(|(host, config)| config.origin == origin_id && !host.starts_with("*."))
        .map(|(host, _)| format!("https://{host}"))
        .collect()
}
