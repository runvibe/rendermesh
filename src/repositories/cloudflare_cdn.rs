use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use reqwest::{Client, Url};
use serde::Deserialize;
use serde_json::json;

use crate::{
    dto::manifest::CloudflareCdnConfig,
    repositories::cdn::{
        ensure_cloudflare_mode, CdnDomainReconcile, CdnDomainReconcileRequest,
        CdnDomainReconcileResult, CdnPurge, CdnPurgeRequest, CdnPurgeResult,
        CloudflarePurgePayload,
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

#[async_trait]
impl CdnDomainReconcile for CloudflareCdnRepository {
    async fn reconcile_domains(
        &self,
        request: CdnDomainReconcileRequest,
    ) -> Result<CdnDomainReconcileResult> {
        let records = self.list_dns_records().await?;
        let mut added = 0usize;
        let mut updated = 0usize;
        let mut removed = 0usize;
        let mut unchanged = 0usize;

        for domain in &request.desired_domains {
            match records
                .iter()
                .find(|record| record.name.eq_ignore_ascii_case(domain))
            {
                Some(record)
                    if record.record_type == "CNAME"
                        && record.content == request.origin_domain
                        && record.proxied == request.proxied =>
                {
                    unchanged += 1;
                }
                Some(record) => {
                    self.update_dns_record(record, &request.origin_domain, request.proxied)
                        .await?;
                    updated += 1;
                }
                None => {
                    self.create_dns_record(domain, &request.origin_domain, request.proxied)
                        .await?;
                    added += 1;
                }
            }
        }

        if request.remove_extra_domains {
            for record in records {
                if record.record_type == "CNAME"
                    && record.content == request.origin_domain
                    && !request.desired_domains.contains(&record.name)
                {
                    self.delete_dns_record(&record.id).await?;
                    removed += 1;
                }
            }
        }

        Ok(CdnDomainReconcileResult {
            provider: "cloudflare".to_string(),
            status: "submitted".to_string(),
            added,
            updated,
            removed,
            unchanged,
        })
    }
}

impl CloudflareCdnRepository {
    async fn list_dns_records(&self) -> Result<Vec<CloudflareDnsRecord>> {
        let url = format!("{}/zones/{}/dns_records", self.api_base, self.zone_id);
        let response = self
            .client
            .get(&url)
            .bearer_auth(&self.api_token)
            .query(&[("type", "CNAME")])
            .send()
            .await
            .context("list Cloudflare DNS records")?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(anyhow!(
                "Cloudflare DNS record list failed with status {status}: {body}"
            ));
        }

        let response = serde_json::from_str::<CloudflareDnsRecordListResponse>(&body)
            .with_context(|| format!("parse Cloudflare DNS record list response: {body}"))?;
        if !response.success {
            return Err(anyhow!(
                "Cloudflare DNS record list response was not successful: {body}"
            ));
        }

        Ok(response.result)
    }

    async fn create_dns_record(&self, name: &str, content: &str, proxied: bool) -> Result<()> {
        let url = format!("{}/zones/{}/dns_records", self.api_base, self.zone_id);
        self.send_dns_record_mutation(
            self.client.post(url),
            name,
            content,
            proxied,
            "create Cloudflare DNS record",
        )
        .await
    }

    async fn update_dns_record(
        &self,
        record: &CloudflareDnsRecord,
        content: &str,
        proxied: bool,
    ) -> Result<()> {
        let url = format!(
            "{}/zones/{}/dns_records/{}",
            self.api_base, self.zone_id, record.id
        );
        self.send_dns_record_mutation(
            self.client.put(url),
            &record.name,
            content,
            proxied,
            "update Cloudflare DNS record",
        )
        .await
    }

    async fn delete_dns_record(&self, record_id: &str) -> Result<()> {
        let url = format!(
            "{}/zones/{}/dns_records/{}",
            self.api_base, self.zone_id, record_id
        );
        let response = self
            .client
            .delete(url)
            .bearer_auth(&self.api_token)
            .send()
            .await
            .context("delete Cloudflare DNS record")?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!(
                "Cloudflare DNS record delete failed with status {status}: {body}"
            ));
        }
        Ok(())
    }

    async fn send_dns_record_mutation(
        &self,
        request: reqwest::RequestBuilder,
        name: &str,
        content: &str,
        proxied: bool,
        context: &str,
    ) -> Result<()> {
        let response = request
            .bearer_auth(&self.api_token)
            .json(&json!({
                "type": "CNAME",
                "name": name,
                "content": content,
                "proxied": proxied,
                "ttl": 1
            }))
            .send()
            .await
            .context(context.to_string())?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!(
                "Cloudflare DNS record mutation failed with status {status}: {body}"
            ));
        }
        Ok(())
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

#[derive(Debug, Deserialize)]
struct CloudflareDnsRecordListResponse {
    success: bool,
    result: Vec<CloudflareDnsRecord>,
}

#[derive(Clone, Debug, Deserialize)]
struct CloudflareDnsRecord {
    id: String,
    #[serde(rename = "type")]
    record_type: String,
    name: String,
    content: String,
    proxied: bool,
}

#[cfg(test)]
mod tests {
    use wiremock::{
        matchers::{header, method, path},
        Mock, MockServer, ResponseTemplate,
    };

    use super::*;
    use std::collections::BTreeSet;

    use crate::repositories::cdn::{CdnDomainReconcile, CdnDomainReconcileRequest, CdnPurgeMode};

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

    #[tokio::test]
    async fn cloudflare_domain_reconcile_creates_missing_dns_records() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/zones/zone-123/dns_records"))
            .and(header("authorization", "Bearer token-123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "success": true,
                "result": [
                    {
                        "id": "existing-1",
                        "type": "CNAME",
                        "name": "www.megaloja.com.br",
                        "content": "rendermesh.example.com",
                        "proxied": true
                    }
                ]
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/zones/zone-123/dns_records"))
            .and(header("authorization", "Bearer token-123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "success": true,
                "result": { "id": "created-1" }
            })))
            .mount(&server)
            .await;
        let repository =
            CloudflareCdnRepository::new(Client::new(), server.uri(), "zone-123", "token-123")
                .expect("repository builds");

        let result = repository
            .reconcile_domains(CdnDomainReconcileRequest {
                origin_id: "loja".to_string(),
                desired_domains: BTreeSet::from([
                    "megaloja.com.br".to_string(),
                    "www.megaloja.com.br".to_string(),
                ]),
                origin_domain: "rendermesh.example.com".to_string(),
                certificate_arn: None,
                proxied: true,
                remove_extra_domains: false,
            })
            .await
            .expect("reconcile succeeds");

        assert_eq!(result.provider, "cloudflare");
        assert_eq!(result.added, 1);
        assert_eq!(result.unchanged, 1);

        let requests = server.received_requests().await.expect("requests");
        let create_request = requests
            .iter()
            .find(|request| request.method.as_str() == "POST")
            .expect("create request");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&create_request.body).expect("json body"),
            json!({
                "type": "CNAME",
                "name": "megaloja.com.br",
                "content": "rendermesh.example.com",
                "proxied": true,
                "ttl": 1
            })
        );
    }
}
