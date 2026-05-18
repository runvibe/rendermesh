use crate::dto::edge::EdgeConfig;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RedirectDecision {
    pub status: u16,
    pub location: String,
}

pub fn find_redirect(
    config: &EdgeConfig,
    path: &str,
    query: Option<&str>,
) -> Option<RedirectDecision> {
    let (rule, splat) = config
        .redirects
        .iter()
        .filter_map(|rule| match_pattern(&rule.from, path).map(|splat| (rule, splat)))
        .max_by_key(|(rule, _)| rule.from.len())?;

    let mut location = apply_splat(&rule.to, splat);
    if !location.contains('?') {
        if let Some(query) = query.filter(|value| !value.is_empty()) {
            location.push('?');
            location.push_str(query);
        }
    }

    Some(RedirectDecision {
        status: rule.status,
        location,
    })
}

pub fn resolve_rewrite(config: &EdgeConfig, path: &str) -> String {
    config
        .rewrites
        .iter()
        .filter_map(|rule| match_pattern(&rule.from, path).map(|splat| (rule, splat)))
        .max_by_key(|(rule, _)| rule.from.len())
        .map(|(rule, splat)| apply_splat(&rule.to, splat))
        .unwrap_or_else(|| path.to_string())
}

pub fn resolve_root_object(config: &EdgeConfig, path: &str) -> String {
    if !path.ends_with('/') {
        return path.to_string();
    }

    let prefix = path.trim_end_matches('/');
    format!("{prefix}{}", config.edge.root_object)
}

pub fn auto_index_candidate(path: &str) -> String {
    format!("{}/index.html", path.trim_end_matches('/'))
}

fn apply_splat(target: &str, splat: Option<&str>) -> String {
    target.replace(":splat", splat.unwrap_or_default())
}

fn match_pattern<'a>(pattern: &str, path: &'a str) -> Option<Option<&'a str>> {
    if pattern == path {
        return Some(None);
    }

    let prefix = pattern.strip_suffix('*')?;
    path.strip_prefix(prefix).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::edge_config::parse_edge_config_yaml;

    #[test]
    fn redirects_exact_and_preserves_query() {
        let config = parse_edge_config_yaml(
            r#"
version: 1
edge:
  root_object: /index.html
  auto_rewrite_index: true
missing:
  action: not_found
  page: /index.html
redirects:
  - from: /docs
    to: /docs/
    status: 308
"#,
        )
        .expect("config");

        let redirect = find_redirect(&config, "/docs", Some("v=1")).expect("redirect");

        assert_eq!(redirect.status, 308);
        assert_eq!(redirect.location, "/docs/?v=1");
    }

    #[test]
    fn wildcard_redirect_replaces_splat() {
        let config = parse_edge_config_yaml(
            r#"
version: 1
edge:
  root_object: /index.html
  auto_rewrite_index: true
missing:
  action: not_found
  page: /index.html
redirects:
  - from: /old/*
    to: /new/:splat
    status: 301
"#,
        )
        .expect("config");

        let redirect = find_redirect(&config, "/old/a/b", None).expect("redirect");

        assert_eq!(redirect.location, "/new/a/b");
    }

    #[test]
    fn rewrite_and_root_object_resolution_work() {
        let config = parse_edge_config_yaml(
            r#"
version: 1
edge:
  root_object: /index.html
  auto_rewrite_index: true
missing:
  action: not_found
  page: /index.html
rewrites:
  - from: /docs
    to: /docs/index.html
"#,
        )
        .expect("config");

        assert_eq!(resolve_rewrite(&config, "/docs"), "/docs/index.html");
        assert_eq!(resolve_root_object(&config, "/guide/"), "/guide/index.html");
    }

    #[test]
    fn auto_index_candidate_uses_path_index_html() {
        assert_eq!(auto_index_candidate("/docs"), "/docs/index.html");
        assert_eq!(
            auto_index_candidate("/blog/post-1"),
            "/blog/post-1/index.html"
        );
    }
}
