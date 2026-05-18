use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct EdgeConfig {
    pub version: u16,
    pub edge: EdgeDefaults,
    pub missing: MissingConfig,
    #[serde(default)]
    pub redirects: Vec<RedirectRule>,
    #[serde(default)]
    pub rewrites: Vec<RewriteRule>,
    #[serde(default)]
    pub edges: Vec<EdgeHookConfig>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct EdgeDefaults {
    pub root_object: String,
    #[serde(default = "default_true")]
    pub auto_rewrite_index: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct MissingConfig {
    pub action: MissingAction,
    pub page: Option<String>,
    pub path: Option<String>,
    pub to: Option<String>,
    pub status: Option<u16>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MissingAction {
    NotFound,
    Serve,
    Redirect,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct RedirectRule {
    pub from: String,
    pub to: String,
    pub status: u16,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct RewriteRule {
    pub from: String,
    pub to: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct EdgeHookConfig {
    pub name: String,
    pub url: String,
    pub timeout_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct EdgeHookRequest {
    pub context: EdgeHookContext,
    pub request: EdgeHookHttpRequest,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct EdgeHookContext {
    pub bucket: String,
    pub ip: Option<String>,
    pub origin: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct EdgeHookHttpRequest {
    pub url: String,
    pub method: String,
    pub headers: BTreeMap<String, String>,
    pub body: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Default, PartialEq)]
pub struct EdgeHookPayload {
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    pub body: Option<String>,
    pub file_path: Option<String>,
    pub params: Option<Value>,
}

fn default_true() -> bool {
    true
}
