use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EchoRequestInput {
    pub headers: BTreeMap<String, Vec<String>>,
    pub path: String,
    pub method: String,
    pub body: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EchoResponse {
    pub headers: BTreeMap<String, Vec<String>>,
    pub path: String,
    pub method: String,
    pub body: Option<String>,
}
