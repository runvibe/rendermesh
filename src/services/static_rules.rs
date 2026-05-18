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
    let (rule, matched) = config
        .redirects
        .iter()
        .filter_map(|rule| match_pattern(&rule.from, path).map(|matched| (rule, matched)))
        .max_by_key(|(_, matched)| matched.rank())?;

    let mut location = apply_splat(&rule.to, matched.splat);
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
        .filter_map(|rule| match_pattern(&rule.from, path).map(|matched| (rule, matched)))
        .max_by_key(|(_, matched)| matched.rank())
        .map(|(rule, matched)| apply_splat(&rule.to, matched.splat))
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PatternMatch<'a> {
    kind: MatchKind,
    specificity: usize,
    splat: Option<&'a str>,
}

impl PatternMatch<'_> {
    fn rank(&self) -> (MatchKind, usize) {
        (self.kind, self.specificity)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum MatchKind {
    Wildcard,
    Exact,
}

fn match_pattern<'a>(pattern: &str, path: &'a str) -> Option<PatternMatch<'a>> {
    if pattern == path {
        return Some(PatternMatch {
            kind: MatchKind::Exact,
            specificity: pattern.len(),
            splat: None,
        });
    }

    let prefix = pattern.strip_suffix('*')?;
    path.strip_prefix(prefix).map(|splat| PatternMatch {
        kind: MatchKind::Wildcard,
        specificity: prefix.len(),
        splat: Some(splat),
    })
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
    fn redirect_exact_beats_wildcard_for_same_path() {
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
  - from: /docs*
    to: /wild
    status: 301
  - from: /docs
    to: /exact
    status: 302
"#,
        )
        .expect("config");

        let redirect = find_redirect(&config, "/docs", None).expect("redirect");

        assert_eq!(redirect.status, 302);
        assert_eq!(redirect.location, "/exact");
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
    fn rewrite_exact_beats_wildcard_for_same_path() {
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
  - from: /docs*
    to: /wild.html
  - from: /docs
    to: /exact.html
"#,
        )
        .expect("config");

        assert_eq!(resolve_rewrite(&config, "/docs"), "/exact.html");
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
