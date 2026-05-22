use std::collections::BTreeMap;

use serde::Deserialize;

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
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LocalOriginConfig {
    pub path: String,
    pub sync_interval_seconds: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct HostConfig {
    pub origin: String,
}
