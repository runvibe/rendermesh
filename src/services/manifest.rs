use std::{collections::BTreeMap, path::Path, sync::Arc};

use anyhow::{anyhow, Result};

use crate::{
    dto::manifest::{CdnConfig, DomainReconcileMode, OriginConfig, RenderMeshManifest},
    repositories::manifest::ManifestRepository,
    repositories::sync::normalize_remote_key,
    services::config_format::parse_config,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedHost {
    pub normalized_host: String,
    pub matched_host: String,
    pub origin_id: String,
}

#[derive(Clone, Debug)]
pub struct HostResolver {
    exact: BTreeMap<String, String>,
    wildcards: Vec<WildcardHost>,
    fallback: Option<String>,
}

#[derive(Clone, Debug)]
struct WildcardHost {
    pattern: String,
    suffix: String,
    origin_id: String,
}

impl HostResolver {
    pub fn new(manifest: &RenderMeshManifest) -> Result<Self> {
        let mut exact = BTreeMap::new();
        let mut wildcards = Vec::new();
        let mut fallback = None;

        for (host, config) in &manifest.hosts {
            let normalized = host.trim().to_ascii_lowercase();
            if normalized == "*" {
                if fallback.replace(config.origin.clone()).is_some() {
                    return Err(anyhow!("duplicate global wildcard host"));
                }
            } else if let Some(suffix) = normalized.strip_prefix("*.") {
                let suffix = suffix.to_string();
                if normalize_host(&suffix).as_deref() != Some(suffix.as_str()) {
                    return Err(anyhow!("invalid wildcard host {host}"));
                }

                wildcards.push(WildcardHost {
                    pattern: normalized,
                    suffix: format!(".{suffix}"),
                    origin_id: config.origin.clone(),
                });
            } else {
                let normalized_host =
                    normalize_host(&normalized).ok_or_else(|| anyhow!("invalid host {host}"))?;
                exact.insert(normalized_host, config.origin.clone());
            }
        }

        wildcards.sort_by(|left, right| right.suffix.len().cmp(&left.suffix.len()));

        Ok(Self {
            exact,
            wildcards,
            fallback,
        })
    }

    pub fn resolve(&self, host_header: &str) -> Option<ResolvedHost> {
        let normalized_host = normalize_host(host_header)?;

        if let Some(origin_id) = self.exact.get(&normalized_host) {
            return Some(ResolvedHost {
                matched_host: normalized_host.clone(),
                normalized_host,
                origin_id: origin_id.clone(),
            });
        }

        for wildcard in &self.wildcards {
            if normalized_host.ends_with(&wildcard.suffix)
                && normalized_host.len() > wildcard.suffix.len()
            {
                return Some(ResolvedHost {
                    normalized_host,
                    matched_host: wildcard.pattern.clone(),
                    origin_id: wildcard.origin_id.clone(),
                });
            }
        }

        self.fallback.as_ref().map(|origin_id| ResolvedHost {
            normalized_host,
            matched_host: "*".to_string(),
            origin_id: origin_id.clone(),
        })
    }
}

pub fn normalize_host(host_header: &str) -> Option<String> {
    let value = host_header.trim();
    if value.is_empty() {
        return None;
    }

    let host = if let Some((host, port)) = value.rsplit_once(':') {
        if port.is_empty() || !port.chars().all(|ch| ch.is_ascii_digit()) {
            return None;
        }
        host
    } else {
        value
    }
    .trim()
    .to_ascii_lowercase();

    if is_valid_host(&host) {
        Some(host)
    } else {
        None
    }
}

fn is_valid_host(host: &str) -> bool {
    !host.is_empty()
        && !host.starts_with('.')
        && !host.ends_with('.')
        && host.split('.').all(is_valid_host_label)
}

fn is_valid_host_label(label: &str) -> bool {
    !label.is_empty()
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
}

pub async fn load_manifest(
    repository: &ManifestRepository,
    path: impl AsRef<Path>,
) -> Result<Arc<RenderMeshManifest>> {
    let content = repository.load_content(path).await?;
    Ok(Arc::new(parse_manifest_config(&content)?))
}

pub fn parse_manifest_yaml(input: &str) -> Result<RenderMeshManifest> {
    parse_manifest_config(input)
}

pub fn parse_manifest_config(input: &str) -> Result<RenderMeshManifest> {
    let manifest = parse_config::<RenderMeshManifest>("manifest", input)?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

pub fn validate_manifest(manifest: &RenderMeshManifest) -> Result<()> {
    if manifest.version != 1 {
        return Err(anyhow!("unsupported manifest version {}", manifest.version));
    }
    if manifest.runtime.local_store_dir.trim().is_empty() {
        return Err(anyhow!("runtime.local_store_dir is required"));
    }
    if manifest.runtime.sync_interval_seconds == 0 {
        return Err(anyhow!("runtime.sync_interval_seconds must be positive"));
    }

    for (origin_id, origin) in &manifest.origins {
        validate_origin_id(origin_id)?;
        match origin {
            OriginConfig::S3(origin) => {
                if origin.bucket.trim().is_empty() {
                    return Err(anyhow!("origin {origin_id} bucket is required"));
                }
                validate_cdn_config(origin_id, origin.cdn.as_ref())?;
            }
            OriginConfig::Local(origin) => {
                if origin.path.trim().is_empty() {
                    return Err(anyhow!("origin {origin_id} path is required"));
                }
                validate_cdn_config(origin_id, origin.cdn.as_ref())?;
            }
        }
        if origin.sync_interval_seconds() == Some(0) {
            return Err(anyhow!(
                "origin {origin_id} sync_interval_seconds must be positive"
            ));
        }
        if let Some(path) = origin.activation_barrier_path() {
            normalize_remote_key(path).map_err(|error| {
                anyhow!("origin {origin_id} activation_barrier_path is invalid: {error}")
            })?;
        }
    }

    for (host, host_config) in &manifest.hosts {
        if host.trim().is_empty() {
            return Err(anyhow!("host entry cannot be empty"));
        }
        if !manifest.origins.contains_key(&host_config.origin) {
            return Err(anyhow!(
                "host {host} references unknown origin {}",
                host_config.origin
            ));
        }
    }

    Ok(())
}

fn validate_cdn_config(origin_id: &str, cdn: Option<&CdnConfig>) -> Result<()> {
    match cdn {
        Some(CdnConfig::CloudFront(config)) => {
            if config.distribution_id_env.trim().is_empty() {
                return Err(anyhow!(
                    "origin {origin_id} cdn.distribution_id_env is required"
                ));
            }
            validate_cdn_domain_config(origin_id, config.domains.as_ref(), true)?;
        }
        Some(CdnConfig::Cloudflare(config)) => {
            if config.zone_id_env.trim().is_empty() {
                return Err(anyhow!("origin {origin_id} cdn.zone_id_env is required"));
            }
            if config.api_token_env.trim().is_empty() {
                return Err(anyhow!("origin {origin_id} cdn.api_token_env is required"));
            }
            for url_prefix in &config.url_prefixes {
                if !url_prefix.starts_with("https://") && !url_prefix.starts_with("http://") {
                    return Err(anyhow!(
                        "origin {origin_id} cdn.url_prefixes must use http or https URLs"
                    ));
                }
            }
            validate_cdn_domain_config(origin_id, config.domains.as_ref(), false)?;
        }
        Some(CdnConfig::CloudFrontSaas(config)) => {
            validate_cloudfront_saas_cdn_config(origin_id, config)?;
        }
        None => {}
    }

    Ok(())
}

fn validate_cloudfront_saas_cdn_config(
    origin_id: &str,
    config: &crate::dto::manifest::CloudFrontSaasCdnConfig,
) -> Result<()> {
    if config.distribution_id_env.trim().is_empty() {
        return Err(anyhow!(
            "origin {origin_id} cdn.distribution_id_env is required"
        ));
    }
    if let Some(connection_group_id_env) = config.connection_group_id_env.as_deref() {
        if connection_group_id_env.trim().is_empty() {
            return Err(anyhow!(
                "origin {origin_id} cdn.connection_group_id_env is required"
            ));
        }
    }
    for (parameter_name, parameter_env) in &config.parameters_env {
        if parameter_name.trim().is_empty() {
            return Err(anyhow!(
                "origin {origin_id} cdn.parameters_env contains an empty parameter name"
            ));
        }
        if parameter_env.trim().is_empty() {
            return Err(anyhow!(
                "origin {origin_id} cdn.parameters_env.{parameter_name} is required"
            ));
        }
    }
    Ok(())
}

fn validate_cdn_domain_config(
    origin_id: &str,
    domains: Option<&crate::dto::manifest::CdnDomainConfig>,
    cloudfront: bool,
) -> Result<()> {
    let Some(domains) = domains else {
        return Ok(());
    };
    if !domains.enabled {
        return Ok(());
    }
    if domains.origin_domain_env.trim().is_empty() {
        return Err(anyhow!(
            "origin {origin_id} cdn.domains.origin_domain_env is required"
        ));
    }
    if cloudfront
        && domains
            .certificate_arn_env
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
    {
        return Err(anyhow!(
            "origin {origin_id} cdn.domains.certificate_arn_env is required for CloudFront"
        ));
    }
    if !cloudfront && domains.mode == DomainReconcileMode::CustomHostnames {
        return Err(anyhow!(
            "origin {origin_id} cdn.domains.mode custom_hostnames is not implemented"
        ));
    }
    Ok(())
}

fn validate_origin_id(origin_id: &str) -> Result<()> {
    let valid = !origin_id.is_empty()
        && origin_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-');
    if !valid {
        return Err(anyhow!("invalid origin id {origin_id}"));
    }
    Ok(())
}


#[cfg(test)]
mod tests;
