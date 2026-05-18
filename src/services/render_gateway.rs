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
    edge_configs: Arc<BTreeMap<String, EdgeConfig>>,
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
        Self {
            resolver: Arc::new(resolver),
            cors: Arc::new(cors),
            mirror,
            edge_configs: Arc::new(edge_configs),
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
        let Some(config) = self.edge_configs.get(&resolved.origin_id) else {
            tracing::error!(
                origin_id = %resolved.origin_id,
                "resolved origin is missing edge config"
            );
            let mut response = RenderResponse::empty(StatusCode::INTERNAL_SERVER_ERROR);
            apply_headers(&mut response.headers, cors_headers);
            return Ok(response);
        };
        let edge_result = self
            .handle_edge_chain(&request, &resolved, config, &cors_headers)
            .await?;
        if let Some(response) = edge_result.0 {
            return Ok(response);
        }
        let response_headers = combined_response_headers(&edge_result.1, &cors_headers);
        if let Some(redirect) = find_redirect(config, &request.path, request.query.as_deref()) {
            let status = StatusCode::from_u16(redirect.status)?;
            let mut response = RenderResponse::empty(status);
            insert_header(&mut response.headers, "location", redirect.location);
            apply_headers(&mut response.headers, response_headers);
            return Ok(self.finalize_response(response, &request));
        }
        let target = self.resolve_request_target(config, &request.path);
        if let Some(response) = self
            .serve_static_path(
                &resolved.origin_id,
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
            .handle_missing(&resolved.origin_id, config, &response_headers)
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
                    return Ok(terminal_edge_response(
                        self.with_edge_headers(response, &state, request),
                    ));
                }
                EdgePayloadOutcome::RenderTarget { status, params } => {
                    let target = self.resolve_request_target(config, &request.path);
                    let Some(response) = self
                        .serve_static_path(
                            &resolved.origin_id,
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
                    return Ok(terminal_edge_response(
                        self.with_edge_headers(response, &state, request),
                    ));
                }
            }
        }
        Ok((None, filtered_headers(&state.headers)))
    }

    fn with_edge_headers(
        &self,
        mut response: RenderResponse,
        state: &EdgeChainState,
        request: &RenderRequest,
    ) -> RenderResponse {
        apply_headers(&mut response.headers, filtered_headers(&state.headers));
        self.finalize_response(response, request)
    }

    fn resolve_request_target(&self, config: &EdgeConfig, request_path: &str) -> String {
        let rewritten = resolve_rewrite(config, request_path);
        resolve_root_object(config, &rewritten)
    }

    async fn serve_static_path(
        &self,
        origin_id: &str,
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
        let Some(config) = self.edge_configs.get(origin_id) else {
            return Ok(None);
        };
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
        if let Ok(object_path) = self.mirror.object_path(origin_id, path) {
            if let Ok(metadata) = tokio::fs::metadata(&object_path).await {
                if metadata.is_dir() {
                    return Ok(None);
                }
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
        body: String::from_utf8_lossy(&request.body).into_owned(),
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
mod tests {
    use super::*;
    use crate::repositories::local_mirror::{metadata_sidecar_path, LocalMirrorRepository};
    use crate::services::cors::CorsPolicy;
    use crate::services::edge_config::{default_edge_config, parse_edge_config_yaml};
    use crate::services::manifest::{parse_manifest_yaml, HostResolver};
    use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, ResponseTemplate,
    };

    #[tokio::test]
    async fn serves_get_from_local_mirror() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_object(
            temp.path(),
            "index.html",
            "<h1>Hello</h1>",
            Some(r#"{"content_type":"text/html"}"#),
        )
        .await;
        let service = test_gateway(temp.path().join("origins"));
        let response = service
            .handle(test_request(Method::GET, "/"))
            .await
            .expect("response");
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body, bytes::Bytes::from_static(b"<h1>Hello</h1>"));
        assert_header(&response, "content-type", "text/html");
    }

    #[tokio::test]
    async fn unknown_host_returns_421() {
        let temp = tempfile::tempdir().expect("tempdir");
        let service = test_gateway(temp.path().join("origins"));
        let response = service
            .handle(RenderRequest {
                host: "unknown.test".to_string(),
                ..test_request(Method::GET, "/")
            })
            .await
            .expect("response");
        assert_eq!(response.status, StatusCode::MISDIRECTED_REQUEST);
    }

    #[tokio::test]
    async fn resolved_host_missing_edge_config_returns_500() {
        let temp = tempfile::tempdir().expect("tempdir");
        let manifest = test_manifest();
        let service = RenderGatewayService::new_for_tests(
            HostResolver::new(&manifest).expect("resolver"),
            CorsPolicy::from_manifest(&manifest),
            LocalMirrorRepository::new(temp.path().join("origins")),
            BTreeMap::new(),
        );
        let response = service
            .handle(test_request_with_origin(Method::GET, "/"))
            .await
            .expect("response");
        assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_header(&response, "access-control-allow-origin", "https://web.test");
        assert!(response.body.is_empty());
    }

    #[tokio::test]
    async fn missing_not_found_uses_index_body_with_404() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_object(temp.path(), "index.html", "<h1>Shell</h1>", None).await;
        let service = test_gateway(temp.path().join("origins"));
        let response = service
            .handle(test_request(Method::GET, "/missing"))
            .await
            .expect("response");
        assert_eq!(response.status, StatusCode::NOT_FOUND);
        assert_eq!(response.body, bytes::Bytes::from_static(b"<h1>Shell</h1>"));
    }

    #[tokio::test]
    async fn auto_index_serves_directory_index() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_object(temp.path(), "docs/index.html", "<h1>Docs</h1>", None).await;
        let service = test_gateway(temp.path().join("origins"));
        let response = service
            .handle(test_request(Method::GET, "/docs"))
            .await
            .expect("response");
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body, bytes::Bytes::from_static(b"<h1>Docs</h1>"));
    }

    #[tokio::test]
    async fn redirect_returns_configured_status_and_location() {
        let temp = tempfile::tempdir().expect("tempdir");
        let service = test_gateway_with_config(
            temp.path().join("origins"),
            edge_config(
                r#"redirects:
  - from: /old
    to: /new
    status: 308
"#,
            ),
        );

        let response = service
            .handle(RenderRequest {
                query: Some("a=1".to_string()),
                ..test_request(Method::GET, "/old")
            })
            .await
            .expect("response");
        assert_eq!(response.status, StatusCode::PERMANENT_REDIRECT);
        assert_header(&response, "location", "/new?a=1");
        assert!(response.body.is_empty());
    }

    #[tokio::test]
    async fn explicit_rewrite_serves_rewritten_object() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_object(temp.path(), "app/index.html", "<h1>App</h1>", None).await;
        let service = test_gateway_with_config(
            temp.path().join("origins"),
            edge_config(
                r#"rewrites:
  - from: /app
    to: /app/index.html
"#,
            ),
        );

        let response = service
            .handle(test_request(Method::GET, "/app"))
            .await
            .expect("response");
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body, bytes::Bytes::from_static(b"<h1>App</h1>"));
    }

    #[tokio::test]
    async fn head_returns_headers_status_but_empty_body() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_object(
            temp.path(),
            "index.html",
            "<h1>Hello</h1>",
            Some(r#"{"content_type":"text/html","etag":"abc"}"#),
        )
        .await;
        let service = test_gateway(temp.path().join("origins"));

        let response = service
            .handle(test_request(Method::HEAD, "/"))
            .await
            .expect("response");
        assert_eq!(response.status, StatusCode::OK);
        assert!(response.body.is_empty());
        assert_header(&response, "content-type", "text/html");
        assert_header(&response, "etag", "abc");
    }

    #[tokio::test]
    async fn options_returns_cors_preflight_headers_for_allowed_origin() {
        let temp = tempfile::tempdir().expect("tempdir");
        let service = test_gateway(temp.path().join("origins"));
        let request = test_request_with_origin(Method::OPTIONS, "/");
        let response = service.handle(request).await.expect("response");
        assert_eq!(response.status, StatusCode::NO_CONTENT);
        assert_header(&response, "access-control-allow-origin", "https://web.test");
        assert_header(
            &response,
            "access-control-allow-methods",
            "GET, HEAD, OPTIONS",
        );
        assert!(response.body.is_empty());
    }

    #[tokio::test]
    async fn get_response_includes_cors_header_for_allowed_origin() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_object(temp.path(), "index.html", "<h1>Hello</h1>", None).await;
        let service = test_gateway(temp.path().join("origins"));
        let request = test_request_with_origin(Method::GET, "/");
        let response = service.handle(request).await.expect("response");
        assert_eq!(response.status, StatusCode::OK);
        assert_header(&response, "access-control-allow-origin", "https://web.test");
    }

    #[tokio::test]
    async fn unsupported_method_includes_cors_header_for_allowed_origin() {
        let temp = tempfile::tempdir().expect("tempdir");
        let service = test_gateway(temp.path().join("origins"));
        let response = service
            .handle(test_request_with_origin(Method::POST, "/"))
            .await
            .expect("response");
        assert_eq!(response.status, StatusCode::METHOD_NOT_ALLOWED);
        assert_header(&response, "access-control-allow-origin", "https://web.test");
    }

    #[tokio::test]
    async fn edge_headers_only_payload_applies_to_normal_static_response() {
        let server = edge_server(
            200,
            serde_json::json!({
                "headers": {
                    "x-edge": "yes",
                    "content-length": "999",
                    "content-encoding": "gzip",
                    "connection": "close"
                }
            }),
        )
        .await;
        let temp = tempfile::tempdir().expect("tempdir");
        write_object(temp.path(), "index.html", "<h1>Hello</h1>", None).await;
        let service = test_gateway_with_edge_url(temp.path().join("origins"), &server.uri());

        let response = service
            .handle(test_request(Method::GET, "/"))
            .await
            .expect("response");
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body, bytes::Bytes::from_static(b"<h1>Hello</h1>"));
        assert_header(&response, "x-edge", "yes");
        assert!(!response.headers.contains_key("content-length"));
        assert!(!response.headers.contains_key("content-encoding"));
        assert!(!response.headers.contains_key("connection"));
    }

    #[tokio::test]
    async fn edge_body_outcome_returns_edge_body_status_and_headers() {
        let server = edge_server(
            202,
            serde_json::json!({
                "body": "edge body",
                "headers": {"x-edge": "yes"}
            }),
        )
        .await;
        let temp = tempfile::tempdir().expect("tempdir");
        let service = test_gateway_with_edge_url(temp.path().join("origins"), &server.uri());

        let response = service
            .handle(test_request(Method::GET, "/"))
            .await
            .expect("response");
        assert_eq!(response.status, StatusCode::ACCEPTED);
        assert_eq!(response.body, bytes::Bytes::from_static(b"edge body"));
        assert_header(&response, "x-edge", "yes");
    }

    #[tokio::test]
    async fn edge_file_path_outcome_serves_specified_file_with_edge_status() {
        let server = edge_server(
            203,
            serde_json::json!({
                "file_path": "/edge.html",
                "headers": {"x-edge": "file"}
            }),
        )
        .await;
        let temp = tempfile::tempdir().expect("tempdir");
        write_object(temp.path(), "edge.html", "<h1>Edge File</h1>", None).await;
        let service = test_gateway_with_edge_url(temp.path().join("origins"), &server.uri());

        let response = service
            .handle(test_request(Method::GET, "/"))
            .await
            .expect("response");
        assert_eq!(response.status, StatusCode::NON_AUTHORITATIVE_INFORMATION);
        assert_eq!(
            response.body,
            bytes::Bytes::from_static(b"<h1>Edge File</h1>")
        );
        assert_header(&response, "x-edge", "file");
    }

    #[tokio::test]
    async fn edge_params_renders_target_html() {
        let server = edge_server(
            200,
            serde_json::json!({
                "params": {"title": "Edge Title"},
                "headers": {"x-edge": "params"}
            }),
        )
        .await;
        let temp = tempfile::tempdir().expect("tempdir");
        write_object(
            temp.path(),
            "index.html",
            "<h1>{{title}}</h1>",
            Some(r#"{"content_type":"text/html"}"#),
        )
        .await;
        let service = test_gateway_with_edge_url(temp.path().join("origins"), &server.uri());

        let response = service
            .handle(test_request(Method::GET, "/"))
            .await
            .expect("response");
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(
            response.body,
            bytes::Bytes::from_static(b"<h1>Edge Title</h1>")
        );
        assert_header(&response, "x-edge", "params");
    }

    #[tokio::test]
    async fn edge_params_on_non_html_returns_415() {
        let server = edge_server(
            200,
            serde_json::json!({
                "params": {"title": "Edge Title"}
            }),
        )
        .await;
        let temp = tempfile::tempdir().expect("tempdir");
        write_object(
            temp.path(),
            "data.json",
            r#"{"title":"{{title}}"}"#,
            Some(r#"{"content_type":"application/json"}"#),
        )
        .await;
        let service = test_gateway_with_edge_url(temp.path().join("origins"), &server.uri());

        let response = service
            .handle(test_request(Method::GET, "/data.json"))
            .await
            .expect("response");
        assert_eq!(response.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert!(response.body.is_empty());
    }

    fn test_gateway(root: std::path::PathBuf) -> RenderGatewayService {
        test_gateway_with_config(root, default_edge_config())
    }
    fn assert_header(response: &RenderResponse, name: &str, value: &str) {
        assert_eq!(response.headers.get(name).map(String::as_str), Some(value));
    }
    async fn edge_server(status: u16, body: serde_json::Value) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/edge"))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(&server)
            .await;
        server
    }
    fn test_gateway_with_edge_url(
        root: std::path::PathBuf,
        edge_base_url: &str,
    ) -> RenderGatewayService {
        test_gateway_with_config(
            root,
            edge_config(&format!(
                r#"edges:
  - name: test
    url: {edge_base_url}/edge
    timeout_ms: 500
"#
            )),
        )
    }
    fn test_gateway_with_config(
        root: std::path::PathBuf,
        config: EdgeConfig,
    ) -> RenderGatewayService {
        let manifest = test_manifest();

        RenderGatewayService::new_for_tests(
            HostResolver::new(&manifest).expect("resolver"),
            CorsPolicy::from_manifest(&manifest),
            LocalMirrorRepository::new(root),
            [("web".to_string(), config)].into(),
        )
    }
    fn test_request(method: Method, path: &str) -> RenderRequest {
        RenderRequest {
            method,
            host: "web.test".to_string(),
            path: path.to_string(),
            query: None,
            scheme: "https".to_string(),
            headers: HeaderMap::new(),
            body: bytes::Bytes::new(),
        }
    }

    fn test_request_with_origin(method: Method, path: &str) -> RenderRequest {
        let mut request = test_request(method, path);
        request
            .headers
            .insert(header::ORIGIN, HeaderValue::from_static("https://web.test"));
        request
    }

    fn edge_config(extra: &str) -> EdgeConfig {
        parse_edge_config_yaml(&format!(
            r#"version: 1
edge:
  root_object: /index.html
  auto_rewrite_index: true
missing:
  action: not_found
  page: /index.html
{extra}"#
        ))
        .expect("config")
    }

    async fn write_object(
        temp_path: &std::path::Path,
        key: &str,
        body: &str,
        metadata: Option<&str>,
    ) {
        let origin_dir = temp_path.join("origins/web");
        let object_path = origin_dir.join(key);
        tokio::fs::create_dir_all(object_path.parent().expect("object parent"))
            .await
            .expect("mkdir object parent");
        tokio::fs::write(&object_path, body)
            .await
            .expect("write object");

        if let Some(metadata) = metadata {
            let metadata_path = metadata_sidecar_path(&origin_dir, key).expect("metadata path");
            tokio::fs::create_dir_all(metadata_path.parent().expect("metadata parent"))
                .await
                .expect("mkdir metadata parent");
            tokio::fs::write(metadata_path, metadata)
                .await
                .expect("write metadata");
        }
    }

    fn test_manifest() -> crate::dto::manifest::RenderMeshManifest {
        parse_manifest_yaml(
            r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  web:
    type: s3
    bucket: web
    endpoint_env: WEB_ENDPOINT
    region_env: WEB_REGION
    access_key_id_env: WEB_KEY
    secret_access_key_env: WEB_SECRET
hosts:
  web.test:
    origin: web
"#,
        )
        .expect("manifest")
    }
}
