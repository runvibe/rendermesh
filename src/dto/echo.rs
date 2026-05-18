use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EchoRequestInput {
    pub headers: BTreeMap<String, Vec<String>>,
    pub path: String,
    pub method: String,
    pub body: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EchoResponse {
    pub headers: BTreeMap<String, Vec<String>>,
    pub path: String,
    pub method: String,
    pub body: Option<String>,
}
