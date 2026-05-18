use std::collections::BTreeMap;

use crate::{dto::manifest::RenderMeshManifest, services::manifest::normalize_host};

#[derive(Clone, Debug, Default)]
pub struct CorsPolicy {
    rules_by_origin: BTreeMap<String, Vec<CorsHostRule>>,
}

#[derive(Clone, Debug)]
enum CorsHostRule {
    Exact(String),
    WildcardSuffix(String),
}

impl CorsPolicy {
    pub fn from_manifest(manifest: &RenderMeshManifest) -> Self {
        let mut rules_by_origin: BTreeMap<String, Vec<CorsHostRule>> = BTreeMap::new();

        for (host, host_config) in &manifest.hosts {
            let normalized = host.trim().to_ascii_lowercase();
            let rule = if let Some(suffix) = normalized.strip_prefix("*.") {
                CorsHostRule::WildcardSuffix(format!(".{suffix}"))
            } else {
                CorsHostRule::Exact(normalized)
            };

            rules_by_origin
                .entry(host_config.origin.clone())
                .or_default()
                .push(rule);
        }

        Self { rules_by_origin }
    }

    pub fn allowed_origin_for(&self, origin_id: &str, request_origin: &str) -> Option<String> {
        let parsed = url::Url::parse(request_origin).ok()?;
        if parsed.scheme() != "https" {
            return None;
        }

        let host = normalize_host(parsed.host_str()?)?;
        let rules = self.rules_by_origin.get(origin_id)?;

        for rule in rules {
            match rule {
                CorsHostRule::Exact(exact) if exact == &host => {
                    return Some(request_origin.to_string());
                }
                CorsHostRule::WildcardSuffix(suffix)
                    if host.ends_with(suffix) && host.len() > suffix.len() =>
                {
                    return Some(request_origin.to_string());
                }
                _ => {}
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::manifest::parse_manifest_yaml;

    #[test]
    fn allows_exact_host_origin() {
        let manifest = parse_manifest_yaml(
            r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  web:
    type: s3
    bucket: web
    endpoint_env: WEB_ENDPOINT
    region_env: WEB_REGION
    access_key_id_env: WEB_KEY
    secret_access_key_env: WEB_SECRET
hosts:
  megaloja.com.br:
    origin: web
"#,
        )
        .expect("manifest parses");

        let policy = CorsPolicy::from_manifest(&manifest);

        assert_eq!(
            policy.allowed_origin_for("web", "https://megaloja.com.br"),
            Some("https://megaloja.com.br".to_string())
        );
    }

    #[test]
    fn reflects_matching_wildcard_origin() {
        let manifest = parse_manifest_yaml(
            r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  web:
    type: s3
    bucket: web
    endpoint_env: WEB_ENDPOINT
    region_env: WEB_REGION
    access_key_id_env: WEB_KEY
    secret_access_key_env: WEB_SECRET
hosts:
  "*.megaloja.com.br":
    origin: web
"#,
        )
        .expect("manifest parses");

        let policy = CorsPolicy::from_manifest(&manifest);

        assert_eq!(
            policy.allowed_origin_for("web", "https://admin.megaloja.com.br"),
            Some("https://admin.megaloja.com.br".to_string())
        );
        assert_eq!(
            policy.allowed_origin_for("web", "https://megaloja.com.br"),
            None
        );
    }
}
