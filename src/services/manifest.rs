use std::{collections::BTreeMap, path::Path, sync::Arc};

use anyhow::{anyhow, Result};

use crate::{
    dto::manifest::RenderMeshManifest, repositories::manifest::ManifestRepository,
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

        for (host, config) in &manifest.hosts {
            let normalized = host.trim().to_ascii_lowercase();
            if let Some(suffix) = normalized.strip_prefix("*.") {
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

        Ok(Self { exact, wildcards })
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

        None
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
        if origin.bucket.trim().is_empty() {
            return Err(anyhow!("origin {origin_id} bucket is required"));
        }
        if origin.sync_interval_seconds == Some(0) {
            return Err(anyhow!(
                "origin {origin_id} sync_interval_seconds must be positive"
            ));
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
mod tests {
    use super::*;

    fn sample_manifest() -> &'static str {
        r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  my_app:
    type: s3
    bucket: bucket_my_app_123
    endpoint_env: MY_APP_STORAGE_ENDPOINT
    region_env: MY_APP_STORAGE_REGION
    access_key_id_env: MY_APP_ACCESS_KEY_ID
    secret_access_key_env: MY_APP_SECRET_ACCESS_KEY
    force_path_style_env: MY_APP_FORCE_PATH_STYLE
    sync_interval_seconds: 30
hosts:
  myapp.com:
    origin: my_app
  "*.myapp.com":
    origin: my_app
"#
    }

    #[test]
    fn parses_manifest_runtime_origins_and_hosts() {
        let manifest = parse_manifest_yaml(sample_manifest()).expect("manifest parses");

        assert_eq!(manifest.version, 1);
        assert_eq!(manifest.runtime.local_store_dir, "./var/rendermesh/origins");
        assert_eq!(manifest.runtime.sync_interval_seconds, 60);
        assert_eq!(manifest.origins["my_app"].bucket, "bucket_my_app_123");
        assert_eq!(manifest.origins["my_app"].sync_interval_seconds, Some(30));
        assert_eq!(manifest.hosts["myapp.com"].origin, "my_app");
    }

    #[test]
    fn parses_manifest_json() {
        let manifest = parse_manifest_config(
            r#"
{
  "version": 1,
  "runtime": {
    "local_store_dir": "./var/rendermesh/origins",
    "sync_interval_seconds": 60
  },
  "origins": {
    "my_app": {
      "type": "s3",
      "bucket": "bucket_my_app_123",
      "endpoint_env": "MY_APP_STORAGE_ENDPOINT",
      "region_env": "MY_APP_STORAGE_REGION",
      "access_key_id_env": "MY_APP_ACCESS_KEY_ID",
      "secret_access_key_env": "MY_APP_SECRET_ACCESS_KEY",
      "force_path_style_env": "MY_APP_FORCE_PATH_STYLE",
      "sync_interval_seconds": 30
    }
  },
  "hosts": {
    "myapp.com": {
      "origin": "my_app"
    },
    "*.myapp.com": {
      "origin": "my_app"
    }
  }
}
"#,
        )
        .expect("json manifest parses");

        assert_eq!(manifest.version, 1);
        assert_eq!(manifest.origins["my_app"].bucket, "bucket_my_app_123");
        assert_eq!(manifest.hosts["*.myapp.com"].origin, "my_app");
    }

    #[test]
    fn rejects_host_that_references_missing_origin() {
        let yaml = r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins: {}
hosts:
  myapp.com:
    origin: missing
"#;

        let manifest = serde_yaml::from_str::<crate::dto::manifest::RenderMeshManifest>(yaml)
            .expect("yaml parses");
        let error = validate_manifest(&manifest).expect_err("validation fails");

        assert!(error.to_string().contains("unknown origin missing"));
    }

    #[test]
    fn rejects_non_positive_sync_intervals() {
        let yaml = r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 0
origins:
  web:
    type: s3
    bucket: web
    endpoint_env: WEB_ENDPOINT
    region_env: WEB_REGION
    access_key_id_env: WEB_KEY
    secret_access_key_env: WEB_SECRET
hosts:
  web.test:
    origin: web
"#;

        let manifest = serde_yaml::from_str::<crate::dto::manifest::RenderMeshManifest>(yaml)
            .expect("yaml parses");
        let error = validate_manifest(&manifest).expect_err("validation fails");

        assert!(error.to_string().contains("sync_interval_seconds"));
    }

    #[test]
    fn exact_host_wins_over_wildcard() {
        let manifest = parse_manifest_yaml(
            r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  admin:
    type: s3
    bucket: admin
    endpoint_env: ADMIN_ENDPOINT
    region_env: ADMIN_REGION
    access_key_id_env: ADMIN_KEY
    secret_access_key_env: ADMIN_SECRET
  web:
    type: s3
    bucket: web
    endpoint_env: WEB_ENDPOINT
    region_env: WEB_REGION
    access_key_id_env: WEB_KEY
    secret_access_key_env: WEB_SECRET
hosts:
  admin.megaloja.com.br:
    origin: admin
  "*.megaloja.com.br":
    origin: web
"#,
        )
        .expect("manifest parses");

        let resolver = HostResolver::new(&manifest).expect("resolver builds");
        let resolved = resolver
            .resolve("ADMIN.megaloja.com.br:443")
            .expect("host resolves");

        assert_eq!(resolved.origin_id, "admin");
        assert_eq!(resolved.matched_host, "admin.megaloja.com.br");
    }

    #[test]
    fn most_specific_wildcard_wins() {
        let manifest = parse_manifest_yaml(
            r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  broad:
    type: s3
    bucket: broad
    endpoint_env: BROAD_ENDPOINT
    region_env: BROAD_REGION
    access_key_id_env: BROAD_KEY
    secret_access_key_env: BROAD_SECRET
  narrow:
    type: s3
    bucket: narrow
    endpoint_env: NARROW_ENDPOINT
    region_env: NARROW_REGION
    access_key_id_env: NARROW_KEY
    secret_access_key_env: NARROW_SECRET
hosts:
  "*.megaloja.com.br":
    origin: broad
  "*.admin.megaloja.com.br":
    origin: narrow
"#,
        )
        .expect("manifest parses");

        let resolver = HostResolver::new(&manifest).expect("resolver builds");
        let resolved = resolver
            .resolve("x.admin.megaloja.com.br")
            .expect("host resolves");

        assert_eq!(resolved.origin_id, "narrow");
    }

    #[test]
    fn unknown_host_is_none() {
        let manifest = parse_manifest_yaml(sample_manifest()).expect("manifest parses");
        let resolver = HostResolver::new(&manifest).expect("resolver builds");

        assert!(resolver.resolve("unknown.test").is_none());
    }
}
