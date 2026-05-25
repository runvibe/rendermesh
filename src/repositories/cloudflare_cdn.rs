use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use reqwest::{Client, Url};
use serde::Deserialize;
use serde_json::json;

use crate::{
    dto::manifest::CloudflareCdnConfig,
    repositories::cdn::{
        ensure_cloudflare_mode, CdnPurge, CdnPurgeRequest, CdnPurgeResult, CloudflarePurgePayload,
    },
};

const DEFAULT_CLOUDFLARE_API_BASE: &str = "https://api.cloudflare.com/client/v4";

#[derive(Clone)]
pub struct CloudflareCdnRepository {
    client: Client,
    api_base: String,
    zone_id: String,
    api_token: String,
}

impl CloudflareCdnRepository {
    pub fn from_config(config: &CloudflareCdnConfig) -> Result<Self> {
        let zone_id = std::env::var(&config.zone_id_env)
            .with_context(|| format!("read Cloudflare zone id env {}", config.zone_id_env))?;
        let api_token = std::env::var(&config.api_token_env)
            .with_context(|| format!("read Cloudflare API token env {}", config.api_token_env))?;
        let api_base = config
            .api_base_env
            .as_ref()
            .map(|key| {
                std::env::var(key).with_context(|| format!("read Cloudflare API base env {key}"))
            })
            .transpose()?
            .unwrap_or_else(|| DEFAULT_CLOUDFLARE_API_BASE.to_string());

        Self::new(Client::new(), api_base, zone_id, api_token)
    }

    pub fn new(
        client: Client,
        api_base: impl Into<String>,
        zone_id: impl Into<String>,
        api_token: impl Into<String>,
    ) -> Result<Self> {
        let api_base = api_base.into().trim_end_matches('/').to_string();
        Url::parse(&api_base).with_context(|| format!("parse Cloudflare API base {api_base}"))?;

        Ok(Self {
            client,
            api_base,
            zone_id: zone_id.into(),
            api_token: api_token.into(),
        })
    }
}

#[async_trait]
impl CdnPurge for CloudflareCdnRepository {
    async fn purge(&self, request: CdnPurgeRequest) -> Result<CdnPurgeResult> {
        let payload = ensure_cloudflare_mode(request.mode)?;
        let submitted_items = match &payload {
            CloudflarePurgePayload::Everything => 1,
            CloudflarePurgePayload::Files(urls) => urls.len(),
        };
        let body = match payload {
            CloudflarePurgePayload::Everything => json!({ "purge_everything": true }),
            CloudflarePurgePayload::Files(urls) => json!({ "files": urls }),
        };
        let url = format!("{}/zones/{}/purge_cache", self.api_base, self.zone_id);
        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.api_token)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("send Cloudflare cache purge for {}", request.origin_id))?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(anyhow!(
                "Cloudflare cache purge failed with status {status}: {body}"
            ));
        }

        let parsed = serde_json::from_str::<CloudflarePurgeResponse>(&body).ok();
        if matches!(
            parsed.as_ref().map(|response| response.success),
            Some(false)
        ) {
            return Err(anyhow!(
                "Cloudflare cache purge response was not successful: {body}"
            ));
        }

        Ok(CdnPurgeResult {
            provider: "cloudflare".to_string(),
            request_id: parsed
                .and_then(|response| response.result)
                .and_then(|result| result.id),
            status: "submitted".to_string(),
            submitted_items,
        })
    }
}

#[derive(Debug, Deserialize)]
struct CloudflarePurgeResponse {
    success: bool,
    result: Option<CloudflarePurgeResultBody>,
}

#[derive(Debug, Deserialize)]
struct CloudflarePurgeResultBody {
    id: Option<String>,
}

#[cfg(test)]
mod tests {
    use wiremock::{
        matchers::{header, method, path},
        Mock, MockServer, ResponseTemplate,
    };

    use super::*;
    use crate::repositories::cdn::CdnPurgeMode;

    #[tokio::test]
    async fn cloudflare_purge_posts_files_payload() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/zones/zone-123/purge_cache"))
            .and(header("authorization", "Bearer token-123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "success": true,
                "result": { "id": "purge-123" }
            })))
            .mount(&server)
            .await;
        let repository =
            CloudflareCdnRepository::new(Client::new(), server.uri(), "zone-123", "token-123")
                .expect("repository builds");

        let result = repository
            .purge(CdnPurgeRequest {
                origin_id: "web".to_string(),
                generation: 7,
                mode: CdnPurgeMode::Urls(vec!["https://web.test/index.html".to_string()]),
            })
            .await
            .expect("purge succeeds");

        assert_eq!(result.provider, "cloudflare");
        assert_eq!(result.request_id.as_deref(), Some("purge-123"));
        assert_eq!(result.submitted_items, 1);

        let requests = server.received_requests().await.expect("requests");
        let body = requests[0].body.clone();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).expect("json body"),
            json!({ "files": ["https://web.test/index.html"] })
        );
    }
}
