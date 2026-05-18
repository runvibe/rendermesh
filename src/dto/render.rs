use std::collections::BTreeMap;

use axum::http::{HeaderMap, Method, StatusCode};
use bytes::Bytes;

#[derive(Clone, Debug)]
pub struct RenderRequest {
    pub method: Method,
    pub host: String,
    pub path: String,
    pub query: Option<String>,
    pub scheme: String,
    pub headers: HeaderMap,
    pub body: Bytes,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderResponse {
    pub status: StatusCode,
    pub headers: BTreeMap<String, String>,
    pub body: Bytes,
}

impl RenderResponse {
    pub fn empty(status: StatusCode) -> Self {
        Self {
            status,
            headers: BTreeMap::new(),
            body: Bytes::new(),
        }
    }
}
