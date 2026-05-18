use std::{collections::BTreeMap, sync::Arc};

use anyhow::Result;
use axum::http::{header, Method, StatusCode};
use bytes::Bytes;

use crate::{
    dto::{
        edge::{EdgeConfig, EdgeHookRequest},
        render::{RenderRequest, RenderResponse},
    },
    repositories::{
        edge_http::EdgeHttpRepository,
        local_mirror::{LocalMirrorRepository, LocalObject, ObjectMetadata},
    },
    services::{
        cors::CorsPolicy,
        edge_config_store::{EdgeConfigStore, EdgeConfigStoreError},
        edge_hooks::{
            apply_edge_payload, render_html_template, EdgeChainState, EdgePayloadOutcome,
            RenderTemplateError,
        },
        manifest::{HostResolver, ResolvedHost},
        static_rules::{auto_index_candidate, find_redirect, resolve_rewrite, resolve_root_object},
    },
};

#[derive(Clone)]
pub struct RenderGatewayService {
    resolver: Arc<HostResolver>,
    cors: Arc<CorsPolicy>,
    mirror: LocalMirrorRepository,
    edge_configs: EdgeConfigStore,
    edge_http: EdgeHttpRepository,
}

type EdgeChainResult = (Option<RenderResponse>, BTreeMap<String, String>);

impl RenderGatewayService {
    pub fn new(
        resolver: HostResolver,
        cors: CorsPolicy,
        mirror: LocalMirrorRepository,
        edge_configs: BTreeMap<String, EdgeConfig>,
    ) -> Self {
        Self::new_with_edge_config_store(
            resolver,
            cors,
            mirror,
            EdgeConfigStore::from_configs(edge_configs),
        )
    }

    pub fn new_with_edge_config_store(
        resolver: HostResolver,
        cors: CorsPolicy,
        mirror: LocalMirrorRepository,
        edge_configs: EdgeConfigStore,
    ) -> Self {
        Self {
            resolver: Arc::new(resolver),
            cors: Arc::new(cors),
            mirror,
            edge_configs,
            edge_http: EdgeHttpRepository::new(),
        }
    }

    pub fn new_for_tests(
        resolver: HostResolver,
        cors: CorsPolicy,
        mirror: LocalMirrorRepository,
        edge_configs: BTreeMap<String, EdgeConfig>,
    ) -> Self {
        Self::new(resolver, cors, mirror, edge_configs)
    }

    pub async fn handle(&self, request: RenderRequest) -> Result<RenderResponse> {
        let Some(resolved) = self.resolver.resolve(&request.host) else {
            return Ok(RenderResponse::empty(StatusCode::MISDIRECTED_REQUEST));
        };
        let cors_headers = self.cors_headers(&resolved.origin_id, &request);
        if request.method == Method::OPTIONS {
            let mut response = RenderResponse::empty(StatusCode::NO_CONTENT);
            apply_headers(&mut response.headers, cors_headers);
            insert_header(
                &mut response.headers,
                "access-control-allow-methods",
                "GET, HEAD, OPTIONS",
            );
            if let Some(headers) = request_header(&request, header::ACCESS_CONTROL_REQUEST_HEADERS)
            {
                insert_header(
                    &mut response.headers,
                    "access-control-allow-headers",
                    headers,
                );
            }
            return Ok(response);
        }
        if request.method != Method::GET && request.method != Method::HEAD {
            let mut response = RenderResponse::empty(StatusCode::METHOD_NOT_ALLOWED);
            apply_headers(&mut response.headers, cors_headers);
            return Ok(response);
        }
        let config = match self.edge_configs.get(&resolved.origin_id) {
            Ok(config) => config,
            Err(error) => {
                log_edge_config_error(&resolved.origin_id, error);
                let mut response = RenderResponse::empty(StatusCode::INTERNAL_SERVER_ERROR);
                apply_headers(&mut response.headers, cors_headers);
                return Ok(response);
            }
        };
        let edge_result = self
            .handle_edge_chain(&request, &resolved, &config, &cors_headers)
            .await?;
        if let Some(response) = edge_result.0 {
            return Ok(response);
        }
        let response_headers = combined_response_headers(&edge_result.1, &cors_headers);
        if let Some(redirect) = find_redirect(&config, &request.path, request.query.as_deref()) {
            let status = StatusCode::from_u16(redirect.status)?;
            let mut response = RenderResponse::empty(status);
            insert_header(&mut response.headers, "location", redirect.location);
            apply_headers(&mut response.headers, response_headers);
            return Ok(self.finalize_response(response, &request));
        }
        let target = self.resolve_request_target(&config, &request.path);
        if let Some(response) = self
            .serve_static_path(
                &resolved.origin_id,
                &config,
                &target,
                StatusCode::OK,
                None,
                &response_headers,
            )
            .await?
        {
            return Ok(self.finalize_response(response, &request));
        }

        Ok(self
            .handle_missing(&resolved.origin_id, &config, &response_headers)
            .await?
            .map(|response| self.finalize_response(response, &request))
            .unwrap_or_else(|| self.finalize_response(not_found_text(response_headers), &request)))
    }

    async fn handle_edge_chain(
        &self,
        request: &RenderRequest,
        resolved: &ResolvedHost,
        config: &EdgeConfig,
        cors_headers: &BTreeMap<String, String>,
    ) -> Result<EdgeChainResult> {
        let mut state = EdgeChainState::default();
        for hook in &config.edges {
            let edge_request = edge_hook_request(request);
            let edge_response = match self
                .edge_http
                .call(&hook.url, hook.timeout_ms, &edge_request)
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    tracing::error!(edge = %hook.name, "edge hook failed: {error}");
                    let mut response = RenderResponse::empty(edge_failure_status(&error));
                    apply_headers(&mut response.headers, cors_headers.clone());
                    return Ok(terminal_edge_response(
                        self.finalize_response(response, request),
                    ));
                }
            };
            let outcome =
                match apply_edge_payload(&mut state, edge_response.status, edge_response.payload) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        tracing::error!(edge = %hook.name, "edge payload failed: {error}");
                        let mut response = RenderResponse::empty(StatusCode::BAD_GATEWAY);
                        apply_headers(&mut response.headers, cors_headers.clone());
                        return Ok(terminal_edge_response(
                            self.finalize_response(response, request),
                        ));
                    }
                };
            match outcome {
                EdgePayloadOutcome::Continue => {}
                EdgePayloadOutcome::RespondDirect { status, body } => {
                    let mut response = RenderResponse {
                        status,
                        headers: BTreeMap::new(),
                        body: Bytes::from(body),
                    };
                    apply_headers(&mut response.headers, filtered_headers(&state.headers));
                    apply_headers(&mut response.headers, cors_headers.clone());
                    return Ok(terminal_edge_response(
                        self.finalize_response(response, request),
                    ));
                }
                EdgePayloadOutcome::ServeFile {
                    status,
                    file_path,
                    params,
                } => {
                    let Some(response) = self
                        .serve_static_path(
                            &resolved.origin_id,
                            config,
                            &file_path,
                            status,
                            params.as_ref(),
                            cors_headers,
                        )
                        .await?
                    else {
                        let mut response = RenderResponse::empty(StatusCode::BAD_GATEWAY);
                        apply_headers(&mut response.headers, cors_headers.clone());
                        return Ok(terminal_edge_response(
                            self.finalize_response(response, request),
                        ));
                    };
                    return Ok(terminal_edge_response(self.with_edge_headers(
                        response,
                        &state,
                        cors_headers,
                        request,
                    )));
                }
                EdgePayloadOutcome::RenderTarget { status, params } => {
                    let target = self.resolve_request_target(config, &request.path);
                    let Some(response) = self
                        .serve_static_path(
                            &resolved.origin_id,
                            config,
                            &target,
                            status,
                            Some(&params),
                            cors_headers,
                        )
                        .await?
                    else {
                        let mut response = not_found_text(cors_headers.clone());
                        response.status = StatusCode::NOT_FOUND;
                        return Ok(terminal_edge_response(
                            self.finalize_response(response, request),
                        ));
                    };
                    return Ok(terminal_edge_response(self.with_edge_headers(
                        response,
                        &state,
                        cors_headers,
                        request,
                    )));
                }
            }
        }
        Ok((None, filtered_headers(&state.headers)))
    }

    fn with_edge_headers(
        &self,
        mut response: RenderResponse,
        state: &EdgeChainState,
        cors_headers: &BTreeMap<String, String>,
        request: &RenderRequest,
    ) -> RenderResponse {
        apply_headers(&mut response.headers, filtered_headers(&state.headers));
        apply_headers(&mut response.headers, cors_headers.clone());
        self.finalize_response(response, request)
    }

    fn resolve_request_target(&self, config: &EdgeConfig, request_path: &str) -> String {
        let rewritten = resolve_rewrite(config, request_path);
        resolve_root_object(config, &rewritten)
    }

    async fn serve_static_path(
        &self,
        origin_id: &str,
        config: &EdgeConfig,
        path: &str,
        status: StatusCode,
        params: Option<&serde_json::Value>,
        cors_headers: &BTreeMap<String, String>,
    ) -> Result<Option<RenderResponse>> {
        if let Some(response) = self
            .serve_object(origin_id, path, status, params, cors_headers)
            .await?
        {
            return Ok(Some(response));
        }
        if config.edge.auto_rewrite_index {
            let candidate = auto_index_candidate(path);
            return self
                .serve_object(origin_id, &candidate, status, params, cors_headers)
                .await;
        }
        Ok(None)
    }

    async fn serve_object(
        &self,
        origin_id: &str,
        path: &str,
        status: StatusCode,
        params: Option<&serde_json::Value>,
        cors_headers: &BTreeMap<String, String>,
    ) -> Result<Option<RenderResponse>> {
        let Some(object) = self.safe_read_object(origin_id, path).await? else {
            return Ok(None);
        };
        let mut headers = headers_from_metadata(&object.metadata);
        apply_headers(&mut headers, cors_headers.clone());
        let body = match params {
            Some(params) => match render_object_template(path, &object, params) {
                Ok(body) => Bytes::from(body),
                Err(RenderTemplateError::UnsupportedMediaType) => {
                    return Ok(Some(RenderResponse {
                        status: StatusCode::UNSUPPORTED_MEDIA_TYPE,
                        headers,
                        body: Bytes::new(),
                    }));
                }
                Err(error) => {
                    tracing::error!("render template failed: {error}");
                    return Ok(Some(RenderResponse {
                        status: StatusCode::BAD_GATEWAY,
                        headers,
                        body: Bytes::new(),
                    }));
                }
            },
            None => object.body,
        };
        Ok(Some(RenderResponse {
            status,
            headers,
            body,
        }))
    }

    async fn safe_read_object(&self, origin_id: &str, path: &str) -> Result<Option<LocalObject>> {
        let object_path = match self.mirror.object_path(origin_id, path) {
            Ok(path) => path,
            Err(error) => {
                tracing::warn!(origin = %origin_id, path = %path, "invalid local object path: {error}");
                return Ok(None);
            }
        };

        if let Ok(metadata) = tokio::fs::metadata(&object_path).await {
            if metadata.is_dir() {
                return Ok(None);
            }
        }

        self.mirror.read_object(origin_id, path).await
    }

    async fn handle_missing(
        &self,
        origin_id: &str,
        config: &EdgeConfig,
        cors_headers: &BTreeMap<String, String>,
    ) -> Result<Option<RenderResponse>> {
        match config.missing.action {
            crate::dto::edge::MissingAction::NotFound => {
                if let Some(page) = &config.missing.page {
                    if let Some(mut response) = self
                        .serve_object(origin_id, page, StatusCode::NOT_FOUND, None, cors_headers)
                        .await?
                    {
                        response.status = StatusCode::NOT_FOUND;
                        return Ok(Some(response));
                    }
                }
                Ok(Some(not_found_text(cors_headers.clone())))
            }
            crate::dto::edge::MissingAction::Serve => {
                let Some(path) = &config.missing.path else {
                    return Ok(Some(not_found_text(cors_headers.clone())));
                };
                self.serve_object(origin_id, path, StatusCode::OK, None, cors_headers)
                    .await
            }
            crate::dto::edge::MissingAction::Redirect => {
                let status = config
                    .missing
                    .status
                    .map(StatusCode::from_u16)
                    .transpose()?
                    .unwrap_or(StatusCode::FOUND);
                let mut response = RenderResponse::empty(status);
                if let Some(to) = &config.missing.to {
                    insert_header(&mut response.headers, "location", to);
                }
                apply_headers(&mut response.headers, cors_headers.clone());
                Ok(Some(response))
            }
        }
    }

    fn cors_headers(&self, origin_id: &str, request: &RenderRequest) -> BTreeMap<String, String> {
        let Some(origin) = request_header(request, header::ORIGIN) else {
            return BTreeMap::new();
        };
        let Some(allowed_origin) = self.cors.allowed_origin_for(origin_id, &origin) else {
            return BTreeMap::new();
        };

        [
            ("access-control-allow-origin".to_string(), allowed_origin),
            ("vary".to_string(), "Origin".to_string()),
        ]
        .into()
    }

    fn finalize_response(
        &self,
        mut response: RenderResponse,
        request: &RenderRequest,
    ) -> RenderResponse {
        if request.method == Method::HEAD {
            response.body = Bytes::new();
        }
        response
    }
}

fn edge_hook_request(request: &RenderRequest) -> EdgeHookRequest {
    EdgeHookRequest {
        url: full_request_url(request),
        method: request.method.as_str().to_string(),
        headers: request_headers_map(request),
        body: String::new(),
    }
}

fn terminal_edge_response(response: RenderResponse) -> EdgeChainResult {
    (Some(response), BTreeMap::new())
}

fn full_request_url(request: &RenderRequest) -> String {
    let mut url = format!("{}://{}{}", request.scheme, request.host, request.path);
    if let Some(query) = request.query.as_deref().filter(|value| !value.is_empty()) {
        url.push('?');
        url.push_str(query);
    }
    url
}

fn request_headers_map(request: &RenderRequest) -> BTreeMap<String, String> {
    request
        .headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_ascii_lowercase(), value.to_string()))
        })
        .collect()
}

fn request_header(request: &RenderRequest, name: header::HeaderName) -> Option<String> {
    request
        .headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string)
}

fn headers_from_metadata(metadata: &ObjectMetadata) -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    if let Some(content_type) = &metadata.content_type {
        insert_header(&mut headers, "content-type", content_type);
    }
    if let Some(cache_control) = &metadata.cache_control {
        insert_header(&mut headers, "cache-control", cache_control);
    }
    if let Some(etag) = &metadata.etag {
        insert_header(&mut headers, "etag", etag);
    }
    if let Some(last_modified) = &metadata.last_modified {
        insert_header(&mut headers, "last-modified", last_modified);
    }
    headers
}

fn render_object_template(
    path: &str,
    object: &LocalObject,
    params: &serde_json::Value,
) -> std::result::Result<String, RenderTemplateError> {
    let body = String::from_utf8_lossy(&object.body);
    render_html_template(path, object.metadata.content_type.as_deref(), &body, params)
}

fn not_found_text(cors_headers: BTreeMap<String, String>) -> RenderResponse {
    let mut response = RenderResponse {
        status: StatusCode::NOT_FOUND,
        headers: BTreeMap::new(),
        body: Bytes::from_static(b"not found"),
    };
    apply_headers(&mut response.headers, cors_headers);
    response
}

fn edge_failure_status(error: &anyhow::Error) -> StatusCode {
    if error
        .downcast_ref::<reqwest::Error>()
        .is_some_and(reqwest::Error::is_timeout)
    {
        StatusCode::GATEWAY_TIMEOUT
    } else {
        StatusCode::BAD_GATEWAY
    }
}

fn log_edge_config_error(origin_id: &str, error: EdgeConfigStoreError) {
    match error {
        EdgeConfigStoreError::Missing => {
            tracing::error!(origin_id = %origin_id, "resolved origin is missing edge config");
        }
        EdgeConfigStoreError::Invalid(message) => {
            tracing::error!(origin_id = %origin_id, "resolved origin has invalid edge config: {message}");
        }
    }
}

fn combined_response_headers(
    edge_headers: &BTreeMap<String, String>,
    cors_headers: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut headers = edge_headers.clone();
    apply_headers(&mut headers, cors_headers.clone());
    headers
}

fn filtered_headers(headers: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    headers
        .iter()
        .filter(|(name, _)| !is_unsafe_edge_header(name))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
        .collect()
}

fn is_unsafe_edge_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "content-encoding"
            | "content-length"
            | "host"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

fn apply_headers(target: &mut BTreeMap<String, String>, headers: BTreeMap<String, String>) {
    for (name, value) in headers {
        insert_header(target, &name, value);
    }
}
fn insert_header(headers: &mut BTreeMap<String, String>, name: &str, value: impl Into<String>) {
    headers.insert(name.to_ascii_lowercase(), value.into());
}

#[cfg(test)]
mod tests;
