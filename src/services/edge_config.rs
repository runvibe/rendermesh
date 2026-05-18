use anyhow::{anyhow, Result};

use crate::dto::edge::{EdgeConfig, EdgeDefaults, MissingAction, MissingConfig};

pub fn default_edge_config() -> EdgeConfig {
    EdgeConfig {
        version: 1,
        edge: EdgeDefaults {
            root_object: "/index.html".to_string(),
            auto_rewrite_index: true,
        },
        missing: MissingConfig {
            action: MissingAction::NotFound,
            page: Some("/index.html".to_string()),
            path: None,
            to: None,
            status: None,
        },
        redirects: Vec::new(),
        rewrites: Vec::new(),
        edges: Vec::new(),
    }
}

pub fn parse_edge_config_yaml(input: &str) -> Result<EdgeConfig> {
    let config = serde_yaml::from_str::<EdgeConfig>(input)?;
    validate_edge_config(&config)?;
    Ok(config)
}

pub fn validate_edge_config(config: &EdgeConfig) -> Result<()> {
    if config.version != 1 {
        return Err(anyhow!(
            "unsupported edge config version {}",
            config.version
        ));
    }

    validate_absolute_path("edge.root_object", &config.edge.root_object)?;
    validate_missing_config(&config.missing)?;

    for redirect in &config.redirects {
        validate_absolute_path("redirect.from", &redirect.from)?;
        validate_absolute_path("redirect.to", &redirect.to)?;
        validate_redirect_status(redirect.status)?;
    }

    for rewrite in &config.rewrites {
        validate_absolute_path("rewrite.from", &rewrite.from)?;
        validate_absolute_path("rewrite.to", &rewrite.to)?;
    }

    for edge in &config.edges {
        if edge.name.trim().is_empty() {
            return Err(anyhow!("edge hook name is required"));
        }
        if edge.timeout_ms == 0 {
            return Err(anyhow!(
                "edge hook {} timeout_ms must be positive",
                edge.name
            ));
        }
        url::Url::parse(&edge.url)?;
    }

    Ok(())
}

fn validate_missing_config(missing: &MissingConfig) -> Result<()> {
    match missing.action {
        MissingAction::NotFound => {
            validate_optional_path("missing.page", missing.page.as_deref())?;
            reject_present("missing.path", missing.path.as_deref())?;
            reject_present("missing.to", missing.to.as_deref())?;
            reject_present("missing.status", missing.status)?;
        }
        MissingAction::Serve => {
            let path = missing
                .path
                .as_deref()
                .ok_or_else(|| anyhow!("missing serve requires path"))?;
            validate_absolute_path("missing.path", path)?;
            reject_present("missing.page", missing.page.as_deref())?;
            reject_present("missing.to", missing.to.as_deref())?;
            reject_present("missing.status", missing.status)?;
        }
        MissingAction::Redirect => {
            let to = missing
                .to
                .as_deref()
                .ok_or_else(|| anyhow!("missing redirect requires to"))?;
            validate_absolute_path("missing.to", to)?;
            let status = missing
                .status
                .ok_or_else(|| anyhow!("missing redirect requires status"))?;
            validate_redirect_status(status)?;
            reject_present("missing.page", missing.page.as_deref())?;
            reject_present("missing.path", missing.path.as_deref())?;
        }
    }

    Ok(())
}

fn reject_present<T>(field: &str, value: Option<T>) -> Result<()> {
    if value.is_some() {
        return Err(anyhow!("{field} is not valid for this missing action"));
    }
    Ok(())
}

fn validate_optional_path(field: &str, value: Option<&str>) -> Result<()> {
    if let Some(path) = value {
        validate_absolute_path(field, path)?;
    }
    Ok(())
}

fn validate_absolute_path(field: &str, path: &str) -> Result<()> {
    let has_control_char = path.chars().any(char::is_control);
    if path.is_empty() || !path.starts_with('/') || path.contains("..") || has_control_char {
        return Err(anyhow!("{field} must be an absolute path without .."));
    }
    Ok(())
}

fn validate_redirect_status(status: u16) -> Result<()> {
    if !matches!(status, 301 | 302 | 307 | 308) {
        return Err(anyhow!("invalid redirect status {status}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_edge_config_matches_mvp_defaults() {
        let config = default_edge_config();

        assert_eq!(config.version, 1);
        assert_eq!(config.edge.root_object, "/index.html");
        assert!(config.edge.auto_rewrite_index);
        assert_eq!(config.missing.action, MissingAction::NotFound);
        assert_eq!(config.missing.page.as_deref(), Some("/index.html"));
    }

    #[test]
    fn parses_redirects_rewrites_and_edges() {
        let yaml = r#"
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
rewrites:
  - from: /docs
    to: /docs/index.html
edges:
  - name: auth
    url: https://api.example.com/edge
    timeout_ms: 800
"#;

        let config = parse_edge_config_yaml(yaml).expect("edge config parses");

        assert_eq!(config.redirects[0].from, "/old/*");
        assert_eq!(config.rewrites[0].to, "/docs/index.html");
        assert_eq!(config.edges[0].timeout_ms, 800);
    }
}
