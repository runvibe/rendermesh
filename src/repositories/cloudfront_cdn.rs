use anyhow::{Context, Result};
use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_sdk_cloudfront::{
    config::Region,
    types::{InvalidationBatch, Paths},
    Client,
};
use uuid::Uuid;

use crate::repositories::cdn::{ensure_paths_mode, CdnPurge, CdnPurgeRequest, CdnPurgeResult};

#[derive(Clone)]
pub struct CloudFrontCdnRepository {
    client: Client,
    distribution_id: String,
}

impl CloudFrontCdnRepository {
    pub async fn from_distribution_id_env(distribution_id_env: &str) -> Result<Self> {
        let distribution_id = std::env::var(distribution_id_env).with_context(|| {
            format!("read CloudFront distribution id env {distribution_id_env}")
        })?;
        let shared_config = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .load()
            .await;

        Ok(Self {
            client: Client::new(&shared_config),
            distribution_id,
        })
    }
}

#[async_trait]
impl CdnPurge for CloudFrontCdnRepository {
    async fn purge(&self, request: CdnPurgeRequest) -> Result<CdnPurgeResult> {
        let paths = ensure_paths_mode(request.mode, "CloudFront")?;
        let quantity = paths.len() as i32;
        let caller_reference = format!(
            "rendermesh-{}-{}-{}",
            request.origin_id,
            request.generation,
            Uuid::new_v4()
        );
        let paths = Paths::builder()
            .quantity(quantity)
            .set_items(Some(paths))
            .build()?;
        let batch = InvalidationBatch::builder()
            .caller_reference(caller_reference)
            .paths(paths)
            .build()?;
        let response = self
            .client
            .create_invalidation()
            .distribution_id(&self.distribution_id)
            .invalidation_batch(batch)
            .send()
            .await?;
        let invalidation = response.invalidation();

        Ok(CdnPurgeResult {
            provider: "cloudfront".to_string(),
            request_id: invalidation.map(|invalidation| invalidation.id().to_string()),
            status: invalidation
                .map(|invalidation| invalidation.status())
                .unwrap_or("submitted")
                .to_string(),
            submitted_items: quantity as usize,
        })
    }
}
