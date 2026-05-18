use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use axum::http::StatusCode;
use serde_json::Value;

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

pub fn apply_edge_payload(
    state: &mut EdgeChainState,
    status: StatusCode,
    payload: EdgeHookPayload,
) -> Result<EdgePayloadOutcome> {
    state.headers.extend(payload.headers);

    if let Some(body) = payload.body {
        return Ok(EdgePayloadOutcome::RespondDirect { status, body });
    }

    if let Some(file_path) = payload.file_path {
        validate_edge_file_path(&file_path)?;
        return Ok(EdgePayloadOutcome::ServeFile {
            status,
            file_path,
            params: payload.params,
        });
    }

    if let Some(params) = payload.params {
        return Ok(EdgePayloadOutcome::RenderTarget { status, params });
    }

    Ok(EdgePayloadOutcome::Continue)
}

pub fn validate_edge_file_path(path: &str) -> Result<()> {
    if !path.starts_with('/')
        || path.chars().any(char::is_control)
        || path.split('/').any(|segment| segment == "..")
    {
        return Err(anyhow!("invalid file_path {path}"));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::edge::EdgeHookPayload;
    use axum::http::StatusCode;

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
}
