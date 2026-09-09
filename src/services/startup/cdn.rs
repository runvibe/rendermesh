use std::{collections::BTreeMap, sync::Arc};

use anyhow::{anyhow, Result};

use crate::{
    dto::manifest::{CdnConfig, RenderMeshManifest},
    repositories::cloudfront_saas_cdn::CloudFrontSaasCdnRepository,
    services::{
        cdn_domains::OriginCdnDomains,
        cdn_refresh::OriginCdnRefresh,
        cdn_tenants::{exact_hosts_for_origin, OriginCdnTenants},
        origin_runtime::OriginRuntimeStore,
    },
};

pub(super) struct StartupCdn {
    pub refresh_by_origin: BTreeMap<String, OriginCdnRefresh>,
    pub domains_by_origin: BTreeMap<String, OriginCdnDomains>,
    pub tenants_by_origin: BTreeMap<String, OriginCdnTenants>,
}

pub(super) async fn build_startup_cdn(manifest: &RenderMeshManifest) -> Result<StartupCdn> {
    let (refresh_by_origin, tenants_by_origin) = build_origin_cdns(manifest).await?;
    Ok(StartupCdn {
        refresh_by_origin,
        domains_by_origin: build_origin_cdn_domains(manifest).await?,
        tenants_by_origin,
    })
}

async fn build_origin_cdns(
    manifest: &RenderMeshManifest,
) -> Result<(
    BTreeMap<String, OriginCdnRefresh>,
    BTreeMap<String, OriginCdnTenants>,
)> {
    let mut refresh_by_origin = BTreeMap::new();
    let mut tenants_by_origin = BTreeMap::new();

    for (origin_id, origin) in &manifest.origins {
        let Some(config) = origin.cdn() else {
            continue;
        };
        match config {
            CdnConfig::CloudFrontSaas(config) => {
                let exact_hosts = exact_hosts_for_origin(manifest, origin_id);
                let repository = CloudFrontSaasCdnRepository::from_config(config, exact_hosts)
                    .await
                    .map_err(|error| {
                        anyhow!(
                            "build CloudFront SaaS CDN services for origin {origin_id}: {error:#}"
                        )
                    })?;
                refresh_by_origin.insert(
                    origin_id.clone(),
                    OriginCdnRefresh::from_cloudfront_saas(
                        repository.clone(),
                        config.strategy.clone(),
                    ),
                );
                tenants_by_origin.insert(
                    origin_id.clone(),
                    OriginCdnTenants::new(Arc::new(repository)),
                );
            }
            _ => {
                let url_prefixes = exact_url_prefixes_for_origin(manifest, origin_id);
                refresh_by_origin.insert(
                    origin_id.clone(),
                    OriginCdnRefresh::from_config(config, url_prefixes).await?,
                );
            }
        }
    }

    Ok((refresh_by_origin, tenants_by_origin))
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

pub(super) async fn reconcile_origin_cdn_tenants(
    origin_id: &str,
    manifest: &RenderMeshManifest,
    cdn_tenants: &OriginCdnTenants,
    origin_runtime: &OriginRuntimeStore,
) {
    match cdn_tenants.reconcile(manifest, origin_id).await {
        Ok(outcome) => {
            tracing::info!(
                origin = %origin_id,
                provider = %outcome.provider,
                status = %outcome.status,
                added = outcome.added,
                updated = outcome.updated,
                removed = outcome.removed,
                unchanged = outcome.unchanged,
                "cdn tenant reconciliation submitted"
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
            tracing::error!(origin = %origin_id, "cdn tenant reconciliation failed: {error}");
        }
    }
}

fn exact_url_prefixes_for_origin(manifest: &RenderMeshManifest, origin_id: &str) -> Vec<String> {
    manifest
        .hosts
        .iter()
        .filter(|(host, config)| config.origin == origin_id && !host.trim().starts_with('*'))
        .map(|(host, _)| format!("https://{host}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::services::manifest::parse_manifest_yaml;

    use super::{build_startup_cdn, exact_url_prefixes_for_origin};

    #[tokio::test]
    async fn builds_cloudfront_saas_refresh_and_tenant_services() {
        let _distribution = EnvVarGuard::set("APP_CLOUDFRONT_DISTRIBUTION_ID", "D123");
        let _connection_group = EnvVarGuard::set("APP_CLOUDFRONT_CONNECTION_GROUP_ID", "CG123");
        let _origin_domain = EnvVarGuard::set("APP_CLOUDFRONT_ORIGIN_DOMAIN", "origin.internal");
        let manifest = parse_manifest_yaml(
            r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  app:
    type: local
    path: ./app
    cdn:
      provider: cloudfront_saas
      distribution_id_env: APP_CLOUDFRONT_DISTRIBUTION_ID
      connection_group_id_env: APP_CLOUDFRONT_CONNECTION_GROUP_ID
      strategy: changed_paths
      parameters_env:
        origin: APP_CLOUDFRONT_ORIGIN_DOMAIN
hosts:
  app.example.com:
    origin: app
  "*.example.com":
    origin: app
  "*":
    origin: app
"#,
        )
        .expect("manifest parses");

        let startup_cdn = build_startup_cdn(&manifest)
            .await
            .expect("startup CDN services build");

        assert_eq!(startup_cdn.refresh_by_origin.len(), 1);
        assert_eq!(startup_cdn.tenants_by_origin.len(), 1);
        assert!(startup_cdn.refresh_by_origin.contains_key("app"));
        assert!(startup_cdn.tenants_by_origin.contains_key("app"));
    }

    #[tokio::test]
    async fn reports_the_cloudfront_saas_parameter_field_for_missing_environment_values() {
        let _distribution = EnvVarGuard::set("APP_MISSING_TEST_CLOUDFRONT_DISTRIBUTION_ID", "D123");
        let _missing_parameter = EnvVarGuard::remove("APP_MISSING_CLOUDFRONT_ORIGIN_DOMAIN");
        let manifest = parse_manifest_yaml(
            r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  app:
    type: local
    path: ./app
    cdn:
      provider: cloudfront_saas
      distribution_id_env: APP_MISSING_TEST_CLOUDFRONT_DISTRIBUTION_ID
      parameters_env:
        origin: APP_MISSING_CLOUDFRONT_ORIGIN_DOMAIN
hosts:
  app.example.com:
    origin: app
"#,
        )
        .expect("manifest parses");

        let error = match build_startup_cdn(&manifest).await {
            Ok(_) => panic!("missing parameter environment value must fail startup"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("cdn.parameters_env.origin"));
        assert!(error
            .to_string()
            .contains("APP_MISSING_CLOUDFRONT_ORIGIN_DOMAIN"));
    }

    #[test]
    fn exact_url_prefixes_exclude_wildcard_hosts() {
        let manifest = parse_manifest_yaml(
            r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  app:
    type: local
    path: ./app
hosts:
  app.example.com:
    origin: app
  "*.example.com":
    origin: app
  "*":
    origin: app
"#,
        )
        .expect("manifest parses");

        assert_eq!(
            exact_url_prefixes_for_origin(&manifest, "app"),
            vec!["https://app.example.com".to_string()]
        );
    }

    struct EnvVarGuard {
        key: &'static str,
        original: Option<String>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let original = std::env::var(key).ok();
            std::env::set_var(key, value);
            Self { key, original }
        }

        fn remove(key: &'static str) -> Self {
            let original = std::env::var(key).ok();
            std::env::remove_var(key);
            Self { key, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(value) = self.original.as_ref() {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }
}
