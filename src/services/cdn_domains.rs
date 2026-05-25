use std::collections::BTreeSet;

pub use crate::dto::manifest::DomainReconcileMode;
use anyhow::{anyhow, Context, Result};

use crate::{
    dto::manifest::{CdnConfig, CdnDomainConfig, RenderMeshManifest},
    repositories::{
        cdn::{CdnDomainReconcile, CdnDomainReconcileRequest, CdnPurgeRepository},
        cloudflare_cdn::CloudflareCdnRepository,
        cloudfront_cdn::CloudFrontCdnRepository,
    },
    services::manifest::normalize_host,
};

#[derive(Clone)]
pub struct OriginCdnDomains {
    repository: CdnPurgeRepository,
    config: CdnDomainConfig,
    origin_domain: String,
    certificate_arn: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CdnDomainReconcileOutcome {
    pub provider: String,
    pub status: String,
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub unchanged: usize,
}

impl OriginCdnDomains {
    pub async fn from_config(config: &CdnConfig) -> Result<Option<Self>> {
        let Some(domain_config) = config.domains() else {
            return Ok(None);
        };
        if !domain_config.enabled {
            return Ok(None);
        }
        let origin_domain = std::env::var(&domain_config.origin_domain_env).with_context(|| {
            format!(
                "read CDN origin domain env {}",
                domain_config.origin_domain_env
            )
        })?;
        let certificate_arn = domain_config
            .certificate_arn_env
            .as_ref()
            .map(|key| {
                std::env::var(key).with_context(|| format!("read CDN certificate ARN env {key}"))
            })
            .transpose()?;
        let repository = match config {
            CdnConfig::CloudFront(config) => {
                if certificate_arn.is_none() {
                    return Err(anyhow!(
                        "CloudFront domain reconciliation requires certificate_arn_env"
                    ));
                }
                CdnPurgeRepository::CloudFront(
                    CloudFrontCdnRepository::from_distribution_id_env(&config.distribution_id_env)
                        .await?,
                )
            }
            CdnConfig::Cloudflare(config) => match domain_config.mode {
                DomainReconcileMode::DnsRecords => {
                    CdnPurgeRepository::Cloudflare(CloudflareCdnRepository::from_config(config)?)
                }
                DomainReconcileMode::CustomHostnames => {
                    return Err(anyhow!(
                        "Cloudflare custom_hostnames domain reconciliation is not implemented"
                    ));
                }
            },
        };

        Ok(Some(Self {
            repository,
            config: domain_config.clone(),
            origin_domain,
            certificate_arn,
        }))
    }

    pub async fn reconcile(
        &self,
        manifest: &RenderMeshManifest,
        origin_id: &str,
    ) -> Result<CdnDomainReconcileOutcome> {
        let result = self
            .repository
            .reconcile_domains(CdnDomainReconcileRequest {
                origin_id: origin_id.to_string(),
                desired_domains: desired_domains_for_origin(manifest, origin_id, &self.config),
                origin_domain: self.origin_domain.clone(),
                certificate_arn: self.certificate_arn.clone(),
                proxied: self.config.proxied,
                remove_extra_domains: self.config.remove_extra_domains,
            })
            .await?;

        Ok(CdnDomainReconcileOutcome {
            provider: result.provider,
            status: result.status,
            added: result.added,
            updated: result.updated,
            removed: result.removed,
            unchanged: result.unchanged,
        })
    }
}

pub fn desired_domains_for_origin(
    manifest: &RenderMeshManifest,
    origin_id: &str,
    config: &CdnDomainConfig,
) -> BTreeSet<String> {
    manifest
        .hosts
        .iter()
        .filter(|(host, host_config)| {
            host_config.origin == origin_id
                && (config.include_wildcards || !host.trim().starts_with("*."))
        })
        .filter_map(|(host, _)| normalize_cdn_host(host))
        .collect()
}

fn normalize_cdn_host(host: &str) -> Option<String> {
    let host = host.trim().to_ascii_lowercase();
    if let Some(suffix) = host.strip_prefix("*.") {
        normalize_host(suffix).map(|normalized| format!("*.{normalized}"))
    } else {
        normalize_host(&host)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use crate::{
        dto::manifest::CdnDomainConfig,
        services::cdn_domains::{desired_domains_for_origin, DomainReconcileMode},
        services::manifest::parse_manifest_yaml,
    };

    #[test]
    fn desired_domains_use_exact_hosts_and_skip_wildcards_by_default() {
        let manifest = parse_manifest_yaml(
            r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  loja:
    type: local
    path: ./site
hosts:
  megaloja.com.br:
    origin: loja
  www.megaloja.com.br:
    origin: loja
  "*.megaloja.com.br":
    origin: loja
"#,
        )
        .expect("manifest parses");
        let config = CdnDomainConfig {
            enabled: true,
            mode: DomainReconcileMode::DnsRecords,
            origin_domain_env: "RENDERMESH_PUBLIC_ORIGIN".to_string(),
            certificate_arn_env: None,
            proxied: true,
            include_wildcards: false,
            remove_extra_domains: false,
        };

        let domains = desired_domains_for_origin(&manifest, "loja", &config);

        assert_eq!(
            domains,
            BTreeSet::from([
                "megaloja.com.br".to_string(),
                "www.megaloja.com.br".to_string()
            ])
        );
    }

    #[test]
    fn desired_domains_can_include_wildcards() {
        let manifest = parse_manifest_yaml(
            r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  loja:
    type: local
    path: ./site
hosts:
  "*.megaloja.com.br":
    origin: loja
"#,
        )
        .expect("manifest parses");
        let config = CdnDomainConfig {
            enabled: true,
            mode: DomainReconcileMode::DnsRecords,
            origin_domain_env: "RENDERMESH_PUBLIC_ORIGIN".to_string(),
            certificate_arn_env: None,
            proxied: true,
            include_wildcards: true,
            remove_extra_domains: false,
        };

        let domains = desired_domains_for_origin(&manifest, "loja", &config);

        assert_eq!(domains, BTreeSet::from(["*.megaloja.com.br".to_string()]));
    }
}
