use std::collections::BTreeSet;

use anyhow::{anyhow, Result};
use async_trait::async_trait;

use crate::repositories::{
    cloudflare_cdn::CloudflareCdnRepository, cloudfront_cdn::CloudFrontCdnRepository,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CdnPurgeRequest {
    pub origin_id: String,
    pub generation: u64,
    pub mode: CdnPurgeMode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CdnPurgeMode {
    All,
    Paths(Vec<String>),
    Urls(Vec<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CdnPurgeResult {
    pub provider: String,
    pub request_id: Option<String>,
    pub status: String,
    pub submitted_items: usize,
}

#[async_trait]
pub trait CdnPurge: Send + Sync {
    async fn purge(&self, request: CdnPurgeRequest) -> Result<CdnPurgeResult>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CdnDomainReconcileRequest {
    pub origin_id: String,
    pub desired_domains: BTreeSet<String>,
    pub origin_domain: String,
    pub certificate_arn: Option<String>,
    pub proxied: bool,
    pub remove_extra_domains: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CdnDomainReconcileResult {
    pub provider: String,
    pub status: String,
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub unchanged: usize,
}

#[async_trait]
pub trait CdnDomainReconcile: Send + Sync {
    async fn reconcile_domains(
        &self,
        request: CdnDomainReconcileRequest,
    ) -> Result<CdnDomainReconcileResult>;
}

#[derive(Clone)]
pub enum CdnPurgeRepository {
    CloudFront(CloudFrontCdnRepository),
    Cloudflare(CloudflareCdnRepository),
}

#[async_trait]
impl CdnPurge for CdnPurgeRepository {
    async fn purge(&self, request: CdnPurgeRequest) -> Result<CdnPurgeResult> {
        match self {
            Self::CloudFront(repository) => repository.purge(request).await,
            Self::Cloudflare(repository) => repository.purge(request).await,
        }
    }
}

#[async_trait]
impl CdnDomainReconcile for CdnPurgeRepository {
    async fn reconcile_domains(
        &self,
        request: CdnDomainReconcileRequest,
    ) -> Result<CdnDomainReconcileResult> {
        match self {
            Self::CloudFront(repository) => repository.reconcile_domains(request).await,
            Self::Cloudflare(repository) => repository.reconcile_domains(request).await,
        }
    }
}

pub fn ensure_paths_mode(mode: CdnPurgeMode, provider: &str) -> Result<Vec<String>> {
    match mode {
        CdnPurgeMode::All => Ok(vec!["/*".to_string()]),
        CdnPurgeMode::Paths(paths) => Ok(paths),
        CdnPurgeMode::Urls(_) => Err(anyhow!("{provider} CDN purge requires paths")),
    }
}

pub fn ensure_cloudflare_mode(mode: CdnPurgeMode) -> Result<CloudflarePurgePayload> {
    match mode {
        CdnPurgeMode::All => Ok(CloudflarePurgePayload::Everything),
        CdnPurgeMode::Urls(urls) => Ok(CloudflarePurgePayload::Files(urls)),
        CdnPurgeMode::Paths(_) => Err(anyhow!("Cloudflare CDN purge requires full URLs")),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloudflarePurgePayload {
    Everything,
    Files(Vec<String>),
}
