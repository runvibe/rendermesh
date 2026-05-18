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
pub struct OriginConfig {
    #[serde(rename = "type")]
    pub origin_type: OriginType,
    pub bucket: String,
    pub endpoint_env: String,
    pub region_env: String,
    pub access_key_id_env: String,
    pub secret_access_key_env: String,
    pub force_path_style_env: Option<String>,
    pub sync_interval_seconds: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OriginType {
    S3,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct HostConfig {
    pub origin: String,
}
