use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use axum::http::StatusCode;
use handlebars::Handlebars;
use serde_json::Value;
use thiserror::Error;

use crate::dto::edge::EdgeHookPayload;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EdgeChainState {
    pub headers: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EdgePayloadOutcome {
    Continue,
    RespondDirect {
        status: StatusCode,
        body: String,
    },
    ServeFile {
        status: StatusCode,
        file_path: String,
        params: Option<Value>,
    },
    RenderTarget {
        status: StatusCode,
        params: Value,
    },
}

#[derive(Debug, Error)]
pub enum RenderTemplateError {
    #[error("unsupported media type")]
    UnsupportedMediaType,
    #[error(transparent)]
    Render(#[from] handlebars::RenderError),
    #[error(transparent)]
    Template(#[from] handlebars::TemplateError),
}

impl PartialEq for RenderTemplateError {
    fn eq(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (
                RenderTemplateError::UnsupportedMediaType,
                RenderTemplateError::UnsupportedMediaType
            )
        )
    }
}

pub fn apply_edge_payload(
    state: &mut EdgeChainState,
    status: StatusCode,
    payload: EdgeHookPayload,
) -> Result<EdgePayloadOutcome> {
    let outcome = if let Some(body) = payload.body {
        EdgePayloadOutcome::RespondDirect { status, body }
    } else if let Some(file_path) = payload.file_path {
        validate_edge_file_path(&file_path)?;
        EdgePayloadOutcome::ServeFile {
            status,
            file_path,
            params: payload.params,
        }
    } else if let Some(params) = payload.params {
        EdgePayloadOutcome::RenderTarget { status, params }
    } else {
        EdgePayloadOutcome::Continue
    };

    state.headers.extend(payload.headers);
    Ok(outcome)
}

pub fn validate_edge_file_path(path: &str) -> Result<()> {
    if !path.starts_with('/') || path.chars().any(char::is_control) || path.contains("..") {
        return Err(anyhow!("invalid file_path {path}"));
    }

    Ok(())
}

pub fn render_html_template(
    path: &str,
    content_type: Option<&str>,
    body: &str,
    params: &Value,
) -> std::result::Result<String, RenderTemplateError> {
    if !is_html(path, content_type) {
        return Err(RenderTemplateError::UnsupportedMediaType);
    }

    let handlebars = Handlebars::new();
    handlebars.render_template(body, params).map_err(Into::into)
}

pub fn is_html(path: &str, content_type: Option<&str>) -> bool {
    if let Some(content_type) = content_type {
        let media_type = content_type.split(';').next().unwrap_or_default().trim();
        return media_type.eq_ignore_ascii_case("text/html");
    }

    let path = path.to_ascii_lowercase();
    path.ends_with(".html") || path.ends_with(".htm")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::edge::EdgeHookPayload;
    use axum::http::StatusCode;
    use serde_json::json;

    #[test]
    fn headers_only_payload_continues_and_accumulates() {
        let mut state = EdgeChainState::default();
        let outcome = apply_edge_payload(
            &mut state,
            StatusCode::OK,
            EdgeHookPayload {
                headers: [("x-a".to_string(), "1".to_string())].into(),
                body: None,
                file_path: None,
                params: None,
            },
        )
        .expect("payload applies");

        assert_eq!(outcome, EdgePayloadOutcome::Continue);
        assert_eq!(state.headers["x-a"], "1");
    }

    #[test]
    fn body_payload_stops_chain() {
        let mut state = EdgeChainState::default();
        let outcome = apply_edge_payload(
            &mut state,
            StatusCode::ACCEPTED,
            EdgeHookPayload {
                headers: Default::default(),
                body: Some("ready".to_string()),
                file_path: None,
                params: None,
            },
        )
        .expect("payload applies");

        assert_eq!(
            outcome,
            EdgePayloadOutcome::RespondDirect {
                status: StatusCode::ACCEPTED,
                body: "ready".to_string()
            }
        );
    }

    #[test]
    fn invalid_file_path_is_rejected() {
        let error = validate_edge_file_path("../secret").expect_err("invalid path");

        assert!(error.to_string().contains("invalid file_path"));
    }

    #[test]
    fn invalid_file_path_with_headers_does_not_mutate_state_headers() {
        let mut state = EdgeChainState {
            headers: [("x-existing".to_string(), "kept".to_string())].into(),
        };

        let error = apply_edge_payload(
            &mut state,
            StatusCode::OK,
            EdgeHookPayload {
                headers: [("x-new".to_string(), "rejected".to_string())].into(),
                body: None,
                file_path: Some("/safe/../secret".to_string()),
                params: None,
            },
        )
        .expect_err("invalid path is rejected");

        assert!(error.to_string().contains("invalid file_path"));
        assert_eq!(state.headers.len(), 1);
        assert_eq!(state.headers["x-existing"], "kept");
        assert!(!state.headers.contains_key("x-new"));
    }

    #[test]
    fn validate_edge_file_path_rejects_dotdot_and_control_chars() {
        for path in [
            "/../secret",
            "/safe/../secret",
            "/safe/\nsecret",
            "/safe/v1..2/index.html",
        ] {
            let error = validate_edge_file_path(path).expect_err("path is invalid");

            assert!(error.to_string().contains("invalid file_path"));
        }
    }

    #[test]
    fn validate_edge_file_path_accepts_valid_path_without_dotdot() {
        validate_edge_file_path("/safe/v1-2/index.html").expect("path is valid");
    }

    #[test]
    fn renders_html_with_params() {
        let rendered = render_html_template(
            "/index.html",
            Some("text/html"),
            "<h1>{{title}}</h1>",
            &json!({ "title": "Hello" }),
        )
        .expect("html template renders");

        assert_eq!(rendered, "<h1>Hello</h1>");
    }

    #[test]
    fn rejects_params_for_non_html() {
        let error = render_html_template(
            "/data.json",
            Some("application/json"),
            "{\"title\":\"{{title}}\"}",
            &json!({ "title": "Hello" }),
        )
        .expect_err("non-html templates are rejected");

        assert_eq!(error, RenderTemplateError::UnsupportedMediaType);
    }
}
