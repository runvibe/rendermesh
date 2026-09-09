use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct RenderMeshManifest {
    pub version: u16,
    pub runtime: RuntimeConfig,
    pub origins: BTreeMap<String, OriginConfig>,
    pub hosts: BTreeMap<String, HostConfig>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct RuntimeConfig {
    pub local_store_dir: String,
    pub sync_interval_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OriginConfig {
    S3(S3OriginConfig),
    Local(LocalOriginConfig),
}

impl OriginConfig {
    pub fn sync_interval_seconds(&self) -> Option<u64> {
        match self {
            Self::S3(origin) => origin.sync_interval_seconds,
            Self::Local(origin) => origin.sync_interval_seconds,
        }
    }

    pub fn edge_context_bucket(&self, origin_id: &str) -> String {
        match self {
            Self::S3(origin) => origin.bucket.clone(),
            Self::Local(_) => origin_id.to_string(),
        }
    }

    pub fn cdn(&self) -> Option<&CdnConfig> {
        match self {
            Self::S3(origin) => origin.cdn.as_ref(),
            Self::Local(origin) => origin.cdn.as_ref(),
        }
    }

    pub fn activation_barrier_path(&self) -> Option<&str> {
        match self {
            Self::S3(origin) => origin.activation_barrier_path.as_deref(),
            Self::Local(origin) => origin.activation_barrier_path.as_deref(),
        }
    }

    pub fn edge_context(&self) -> Option<&Value> {
        match self {
            Self::S3(origin) => origin.edge_context.as_ref(),
            Self::Local(origin) => origin.edge_context.as_ref(),
        }
    }
}

impl CdnConfig {
    pub fn domains(&self) -> Option<&CdnDomainConfig> {
        match self {
            Self::CloudFront(config) => config.domains.as_ref(),
            Self::Cloudflare(config) => config.domains.as_ref(),
            Self::CloudFrontSaas(_) => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct S3OriginConfig {
    pub bucket: String,
    pub endpoint_env: String,
    pub region_env: String,
    pub access_key_id_env: Option<String>,
    pub secret_access_key_env: Option<String>,
    pub force_path_style_env: Option<String>,
    pub sync_interval_seconds: Option<u64>,
    pub activation_barrier_path: Option<String>,
    pub cdn: Option<CdnConfig>,
    pub edge_context: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LocalOriginConfig {
    pub path: String,
    pub sync_interval_seconds: Option<u64>,
    pub activation_barrier_path: Option<String>,
    pub cdn: Option<CdnConfig>,
    pub edge_context: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct HostConfig {
    pub origin: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum CdnConfig {
    #[serde(rename = "cloudfront")]
    CloudFront(CloudFrontCdnConfig),
    #[serde(rename = "cloudfront_saas")]
    CloudFrontSaas(CloudFrontSaasCdnConfig),
    Cloudflare(CloudflareCdnConfig),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CloudFrontCdnConfig {
    pub distribution_id_env: String,
    #[serde(default)]
    pub strategy: CdnRefreshStrategy,
    pub domains: Option<CdnDomainConfig>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CloudflareCdnConfig {
    pub zone_id_env: String,
    pub api_token_env: String,
    pub api_base_env: Option<String>,
    #[serde(default)]
    pub strategy: CdnRefreshStrategy,
    #[serde(default)]
    pub url_prefixes: Vec<String>,
    pub domains: Option<CdnDomainConfig>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CloudFrontSaasCdnConfig {
    pub distribution_id_env: String,
    pub connection_group_id_env: Option<String>,
    #[serde(default)]
    pub strategy: CdnRefreshStrategy,
    #[serde(default)]
    pub parameters_env: BTreeMap<String, String>,
    pub certificate: Option<CloudFrontSaasCertificateConfig>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum CloudFrontSaasCertificateConfig {
    Managed {
        #[serde(default)]
        validation_token_host: CloudFrontSaasValidationTokenHost,
    },
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CloudFrontSaasValidationTokenHost {
    #[default]
    #[serde(rename = "cloudfront")]
    CloudFront,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CdnRefreshStrategy {
    #[default]
    ChangedPaths,
    All,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CdnDomainConfig {
    pub enabled: bool,
    #[serde(default)]
    pub mode: DomainReconcileMode,
    pub origin_domain_env: String,
    pub certificate_arn_env: Option<String>,
    #[serde(default = "default_true")]
    pub proxied: bool,
    #[serde(default)]
    pub include_wildcards: bool,
    #[serde(default)]
    pub remove_extra_domains: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DomainReconcileMode {
    #[default]
    DnsRecords,
    CustomHostnames,
}

fn default_true() -> bool {
    true
}
