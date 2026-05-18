use anyhow::{anyhow, Result};

use crate::dto::manifest::RenderMeshManifest;

pub fn parse_manifest_yaml(input: &str) -> Result<RenderMeshManifest> {
    let manifest = serde_yaml::from_str::<RenderMeshManifest>(input)?;
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
}
