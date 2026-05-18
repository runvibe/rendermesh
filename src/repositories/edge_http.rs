use std::time::Duration;

use anyhow::Result;
use axum::http::StatusCode;

use crate::dto::edge::{EdgeHookPayload, EdgeHookRequest};

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
        let response = self
            .client
            .post(url)
            .timeout(Duration::from_millis(timeout_ms))
            .json(request)
            .send()
            .await?;
        let status = StatusCode::from_u16(response.status().as_u16())?;
        let payload = response.json::<EdgeHookPayload>().await?;

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
    use crate::dto::edge::EdgeHookRequest;
    use std::collections::BTreeMap;
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, ResponseTemplate,
    };

    #[tokio::test]
    async fn posts_edge_request_and_parses_payload() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/edge"))
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
                    url: "https://app.test/".to_string(),
                    method: "GET".to_string(),
                    headers: BTreeMap::new(),
                    body: String::new(),
                },
            )
            .await
            .expect("edge call succeeds");

        assert_eq!(response.status, axum::http::StatusCode::CREATED);
        assert_eq!(response.payload.body.as_deref(), Some("ok"));
        assert_eq!(response.payload.headers["x-edge"], "yes");
    }
}
