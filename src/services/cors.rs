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
            let Some(rule) = (if let Some(suffix) = normalized.strip_prefix("*.") {
                normalize_host(suffix)
                    .filter(|host| host == suffix)
                    .map(|suffix| CorsHostRule::WildcardSuffix(format!(".{suffix}")))
            } else {
                normalize_host(&normalized).map(CorsHostRule::Exact)
            }) else {
                continue;
            };

            rules_by_origin
                .entry(host_config.origin.clone())
                .or_default()
                .push(rule);
        }

        Self { rules_by_origin }
    }

    pub fn allowed_origin_for(&self, origin_id: &str, request_origin: &str) -> Option<String> {
        let host = parse_https_origin_host(request_origin)?;
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

fn parse_https_origin_host(request_origin: &str) -> Option<String> {
    let parsed = url::Url::parse(request_origin).ok()?;
    if parsed.scheme() != "https"
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || !matches!(parsed.port(), None | Some(443))
    {
        return None;
    }

    normalize_host(parsed.host_str()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::manifest::parse_manifest_yaml;

    fn exact_host_manifest() -> crate::dto::manifest::RenderMeshManifest {
        parse_manifest_yaml(
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
        .expect("manifest parses")
    }

    #[test]
    fn allows_exact_host_origin() {
        let policy = CorsPolicy::from_manifest(&exact_host_manifest());

        assert_eq!(
            policy.allowed_origin_for("web", "https://megaloja.com.br"),
            Some("https://megaloja.com.br".to_string())
        );
    }

    #[test]
    fn allows_exact_host_origin_with_uppercase_host() {
        let policy = CorsPolicy::from_manifest(&exact_host_manifest());

        assert_eq!(
            policy.allowed_origin_for("web", "https://MEGALOJA.com.br"),
            Some("https://MEGALOJA.com.br".to_string())
        );
    }

    #[test]
    fn rejects_http_origin() {
        let policy = CorsPolicy::from_manifest(&exact_host_manifest());

        assert_eq!(
            policy.allowed_origin_for("web", "http://megaloja.com.br"),
            None
        );
    }

    #[test]
    fn rejects_non_default_port() {
        let policy = CorsPolicy::from_manifest(&exact_host_manifest());

        assert_eq!(
            policy.allowed_origin_for("web", "https://megaloja.com.br:444"),
            None
        );
    }

    #[test]
    fn rejects_origin_with_path_query_fragment_or_userinfo() {
        let policy = CorsPolicy::from_manifest(&exact_host_manifest());

        for request_origin in [
            "https://megaloja.com.br/path",
            "https://megaloja.com.br?debug=true",
            "https://megaloja.com.br#section",
            "https://user@megaloja.com.br",
            "https://user:password@megaloja.com.br",
        ] {
            assert_eq!(
                policy.allowed_origin_for("web", request_origin),
                None,
                "{request_origin} must be rejected"
            );
        }
    }

    #[test]
    fn reflects_matching_wildcard_origin() {
        let manifest = wildcard_host_manifest();
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

    fn wildcard_host_manifest() -> crate::dto::manifest::RenderMeshManifest {
        parse_manifest_yaml(
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
        .expect("manifest parses")
    }
}
