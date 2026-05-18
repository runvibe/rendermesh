use std::time::Duration;
use std::time::Instant;

use anyhow::Result;
use axum::http::StatusCode;

use crate::{
    dto::edge::{EdgeHookPayload, EdgeHookRequest},
    libs::observability::elapsed_ms,
};

#[derive(Clone)]
pub struct EdgeHttpRepository {
    client: reqwest::Client,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EdgeHttpResponse {
    pub status: StatusCode,
    pub payload: EdgeHookPayload,
}

impl EdgeHttpRepository {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    pub async fn call(
        &self,
        url: &str,
        timeout_ms: u64,
        request: &EdgeHookRequest,
    ) -> Result<EdgeHttpResponse> {
        let start = Instant::now();
        tracing::info!(
            edge_url = %url,
            timeout_ms,
            method = %request.request.method,
            request_url = %request.request.url,
            origin = %request.context.origin,
            bucket = %request.context.bucket,
            "edge_http_request_start"
        );
        let response = self
            .client
            .post(url)
            .timeout(Duration::from_millis(timeout_ms))
            .json(request)
            .send()
            .await
            .inspect_err(|error| {
                tracing::error!(
                    edge_url = %url,
                    duration_ms = elapsed_ms(start),
                    "edge_http_request_error: {error}"
                );
            })?;
        let status = StatusCode::from_u16(response.status().as_u16())?;
        tracing::info!(
            edge_url = %url,
            status = status.as_u16(),
            duration_ms = elapsed_ms(start),
            "edge_http_response_received"
        );
        let payload = response.json::<EdgeHookPayload>().await?;
        tracing::info!(
            edge_url = %url,
            status = status.as_u16(),
            duration_ms = elapsed_ms(start),
            "edge_http_payload_decoded"
        );

        Ok(EdgeHttpResponse { status, payload })
    }
}

impl Default for EdgeHttpRepository {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::edge::{EdgeHookContext, EdgeHookHttpRequest, EdgeHookRequest};
    use std::collections::BTreeMap;
    use wiremock::{
        matchers::{body_json, method, path},
        Mock, MockServer, ResponseTemplate,
    };

    #[tokio::test]
    async fn posts_edge_request_and_parses_payload() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/edge"))
            .and(body_json(serde_json::json!({
                "context": {
                    "bucket": "app-bucket",
                    "ip": "203.0.113.10",
                    "origin": "app"
                },
                "request": {
                    "url": "https://app.test/",
                    "method": "GET",
                    "headers": {},
                    "body": ""
                }
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "body": "ok",
                "headers": {"x-edge": "yes"}
            })))
            .mount(&server)
            .await;

        let client = EdgeHttpRepository::new();
        let response = client
            .call(
                &format!("{}/edge", server.uri()),
                500,
                &EdgeHookRequest {
                    context: EdgeHookContext {
                        bucket: "app-bucket".to_string(),
                        ip: Some("203.0.113.10".to_string()),
                        origin: "app".to_string(),
                    },
                    request: EdgeHookHttpRequest {
                        url: "https://app.test/".to_string(),
                        method: "GET".to_string(),
                        headers: BTreeMap::new(),
                        body: String::new(),
                    },
                },
            )
            .await
            .expect("edge call succeeds");

        assert_eq!(response.status, axum::http::StatusCode::CREATED);
        assert_eq!(response.payload.body.as_deref(), Some("ok"));
        assert_eq!(response.payload.headers["x-edge"], "yes");
    }
}
