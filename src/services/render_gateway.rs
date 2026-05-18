use std::{collections::BTreeMap, sync::Arc, time::Instant};

use anyhow::Result;
use axum::http::{header, Method, StatusCode};
use bytes::Bytes;
use tracing::Instrument;

use crate::{
    dto::{
        edge::{EdgeConfig, EdgeHookRequest},
        render::{RenderRequest, RenderResponse},
    },
    libs::observability::elapsed_ms,
    repositories::{
        edge_http::EdgeHttpRepository,
        local_mirror::{LocalMirrorRepository, LocalObject, ObjectMetadata},
    },
    services::{
        cors::CorsPolicy,
        edge_config_store::{EdgeConfigStore, EdgeConfigStoreError},
        edge_hooks::{apply_edge_payload, EdgeChainState, EdgePayloadOutcome},
        manifest::{HostResolver, ResolvedHost},
        static_rules::{auto_index_candidate, find_redirect, resolve_rewrite, resolve_root_object},
        template_store::{TemplateStore, TemplateStoreError},
    },
};

#[derive(Clone)]
pub struct RenderGatewayService {
    resolver: Arc<HostResolver>,
    cors: Arc<CorsPolicy>,
    mirror: LocalMirrorRepository,
    edge_configs: EdgeConfigStore,
    template_store: TemplateStore,
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
        Self::new_with_stores(
            resolver,
            cors,
            mirror,
            EdgeConfigStore::from_configs(edge_configs),
            TemplateStore::default(),
        )
    }

    pub fn new_with_stores(
        resolver: HostResolver,
        cors: CorsPolicy,
        mirror: LocalMirrorRepository,
        edge_configs: EdgeConfigStore,
        template_store: TemplateStore,
    ) -> Self {
        Self {
            resolver: Arc::new(resolver),
            cors: Arc::new(cors),
            mirror,
            edge_configs,
            template_store,
            edge_http: EdgeHttpRepository::new(),
        }
    }

    pub fn new_with_edge_config_store(
        resolver: HostResolver,
        cors: CorsPolicy,
        mirror: LocalMirrorRepository,
        edge_configs: EdgeConfigStore,
    ) -> Self {
        Self::new_with_stores(
            resolver,
            cors,
            mirror,
            edge_configs,
            TemplateStore::default(),
        )
    }

    pub fn new_for_tests(
        resolver: HostResolver,
        cors: CorsPolicy,
        mirror: LocalMirrorRepository,
        edge_configs: BTreeMap<String, EdgeConfig>,
    ) -> Self {
        Self::new(resolver, cors, mirror, edge_configs)
    }

    pub fn new_for_tests_with_template_store(
        resolver: HostResolver,
        cors: CorsPolicy,
        mirror: LocalMirrorRepository,
        edge_configs: BTreeMap<String, EdgeConfig>,
        template_store: TemplateStore,
    ) -> Self {
        Self::new_with_stores(
            resolver,
            cors,
            mirror,
            EdgeConfigStore::from_configs(edge_configs),
            template_store,
        )
    }

    #[tracing::instrument(
        name = "rendermesh.gateway",
        skip(self, request),
        fields(
            method = %request.method,
            host = %request.host,
            path = %request.path,
            status = tracing::field::Empty,
            origin_id = tracing::field::Empty,
            duration_ms = tracing::field::Empty
        )
    )]
    pub async fn handle(&self, request: RenderRequest) -> Result<RenderResponse> {
        let start = Instant::now();
        let result = self.handle_inner(request).await;
        if let Ok(response) = &result {
            tracing::Span::current().record("status", response.status.as_u16());
        }
        tracing::Span::current().record("duration_ms", elapsed_ms(start));
        result
    }

    async fn handle_inner(&self, request: RenderRequest) -> Result<RenderResponse> {
        let resolve_start = Instant::now();
        let resolve_span = tracing::info_span!(
            "rendermesh.resolve_host",
            host = %request.host,
            found = tracing::field::Empty,
            origin_id = tracing::field::Empty,
            duration_ms = tracing::field::Empty
        );
        let resolved = resolve_span.in_scope(|| self.resolver.resolve(&request.host));
        resolve_span.record("duration_ms", elapsed_ms(resolve_start));
        let Some(resolved) = resolved else {
            resolve_span.record("found", false);
            return Ok(RenderResponse::empty(StatusCode::MISDIRECTED_REQUEST));
        };
        resolve_span.record("found", true);
        resolve_span.record("origin_id", resolved.origin_id.as_str());
        tracing::Span::current().record("origin_id", resolved.origin_id.as_str());
        let cors_headers = tracing::info_span!(
            "rendermesh.cors",
            origin_id = %resolved.origin_id,
            host = %request.host
        )
        .in_scope(|| self.cors_headers(&resolved.origin_id, &request));
        if request.method == Method::OPTIONS {
            tracing::info!("handling cors preflight");
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
            tracing::info!(method = %request.method, "method not allowed");
            let mut response = RenderResponse::empty(StatusCode::METHOD_NOT_ALLOWED);
            apply_headers(&mut response.headers, cors_headers);
            return Ok(response);
        }
        let edge_config_start = Instant::now();
        let edge_config_span = tracing::info_span!(
            "rendermesh.edge_config",
            origin_id = %resolved.origin_id,
            valid = tracing::field::Empty,
            duration_ms = tracing::field::Empty
        );
        let edge_config_result =
            edge_config_span.in_scope(|| self.edge_configs.get(&resolved.origin_id));
        edge_config_span.record("duration_ms", elapsed_ms(edge_config_start));
        let config = match edge_config_result {
            Ok(config) => config,
            Err(error) => {
                edge_config_span.record("valid", false);
                log_edge_config_error(&resolved.origin_id, error);
                let mut response = RenderResponse::empty(StatusCode::INTERNAL_SERVER_ERROR);
                apply_headers(&mut response.headers, cors_headers);
                return Ok(response);
            }
        };
        edge_config_span.record("valid", true);
        let edge_result = self
            .handle_edge_chain(&request, &resolved, &config, &cors_headers)
            .await?;
        if let Some(response) = edge_result.0 {
            return Ok(response);
        }
        let response_headers = combined_response_headers(&edge_result.1, &cors_headers);
        let redirect_start = Instant::now();
        let redirect_span = tracing::info_span!(
            "rendermesh.redirect",
            path = %request.path,
            matched = tracing::field::Empty,
            duration_ms = tracing::field::Empty
        );
        let redirect = redirect_span
            .in_scope(|| find_redirect(&config, &request.path, request.query.as_deref()));
        redirect_span.record("duration_ms", elapsed_ms(redirect_start));
        if let Some(redirect) = redirect {
            redirect_span.record("matched", true);
            let status = StatusCode::from_u16(redirect.status)?;
            let mut response = RenderResponse::empty(status);
            insert_header(&mut response.headers, "location", redirect.location);
            apply_headers(&mut response.headers, response_headers);
            return Ok(self.finalize_response(response, &request));
        }
        redirect_span.record("matched", false);
        let target = tracing::info_span!("rendermesh.resolve_target", path = %request.path)
            .in_scope(|| self.resolve_request_target(&config, &request.path));
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
        let start = Instant::now();
        let span = tracing::info_span!(
            "rendermesh.edge_chain",
            origin_id = %resolved.origin_id,
            host = %request.host,
            path = %request.path,
            edge_count = config.edges.len(),
            terminal = tracing::field::Empty,
            duration_ms = tracing::field::Empty
        );
        async move {
            let mut state = EdgeChainState::default();
            for hook in &config.edges {
                let edge_request = edge_hook_request(request);
                let edge_span = tracing::info_span!(
                    "rendermesh.edge_hook",
                    edge = %hook.name,
                    edge_url = %hook.url,
                    timeout_ms = hook.timeout_ms,
                    status = tracing::field::Empty,
                    outcome = tracing::field::Empty,
                    duration_ms = tracing::field::Empty
                );
                let edge_start = Instant::now();
                edge_span.in_scope(|| {
                    tracing::info!(edge = %hook.name, edge_url = %hook.url, "edge_hook_request_start");
                });
                let edge_response = match self
                    .edge_http
                    .call(&hook.url, hook.timeout_ms, &edge_request)
                    .instrument(edge_span.clone())
                    .await
                {
                    Ok(response) => {
                        edge_span.record("status", response.status.as_u16());
                        edge_span.record("duration_ms", elapsed_ms(edge_start));
                        edge_span.in_scope(|| {
                            tracing::info!(
                                edge = %hook.name,
                                status = response.status.as_u16(),
                                duration_ms = elapsed_ms(edge_start),
                                "edge_hook_request_finish"
                            );
                        });
                        response
                    }
                    Err(error) => {
                        edge_span.record("outcome", "error");
                        edge_span.record("duration_ms", elapsed_ms(edge_start));
                        tracing::error!(
                            edge = %hook.name,
                            duration_ms = elapsed_ms(edge_start),
                            "edge hook failed: {error}"
                        );
                        let mut response = RenderResponse::empty(edge_failure_status(&error));
                        apply_headers(&mut response.headers, cors_headers.clone());
                        tracing::Span::current().record("terminal", true);
                        tracing::Span::current().record("duration_ms", elapsed_ms(start));
                        return Ok(terminal_edge_response(
                            self.finalize_response(response, request),
                        ));
                    }
                };
                let outcome = match apply_edge_payload(
                    &mut state,
                    edge_response.status,
                    edge_response.payload,
                ) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        edge_span.record("outcome", "invalid_payload");
                        tracing::error!(edge = %hook.name, "edge payload failed: {error}");
                        let mut response = RenderResponse::empty(StatusCode::BAD_GATEWAY);
                        apply_headers(&mut response.headers, cors_headers.clone());
                        tracing::Span::current().record("terminal", true);
                        tracing::Span::current().record("duration_ms", elapsed_ms(start));
                        return Ok(terminal_edge_response(
                            self.finalize_response(response, request),
                        ));
                    }
                };
                match outcome {
                    EdgePayloadOutcome::Continue => {
                        edge_span.record("outcome", "continue");
                    }
                    EdgePayloadOutcome::RespondDirect { status, body } => {
                        edge_span.record("outcome", "respond_direct");
                        let mut response = RenderResponse {
                            status,
                            headers: BTreeMap::new(),
                            body: Bytes::from(body),
                        };
                        apply_headers(&mut response.headers, filtered_headers(&state.headers));
                        apply_headers(&mut response.headers, cors_headers.clone());
                        tracing::Span::current().record("terminal", true);
                        tracing::Span::current().record("duration_ms", elapsed_ms(start));
                        return Ok(terminal_edge_response(
                            self.finalize_response(response, request),
                        ));
                    }
                    EdgePayloadOutcome::ServeFile {
                        status,
                        file_path,
                        params,
                    } => {
                        edge_span.record("outcome", "serve_file");
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
                            tracing::Span::current().record("terminal", true);
                            tracing::Span::current().record("duration_ms", elapsed_ms(start));
                            return Ok(terminal_edge_response(
                                self.finalize_response(response, request),
                            ));
                        };
                        tracing::Span::current().record("terminal", true);
                        tracing::Span::current().record("duration_ms", elapsed_ms(start));
                        return Ok(terminal_edge_response(self.with_edge_headers(
                            response,
                            &state,
                            cors_headers,
                            request,
                        )));
                    }
                    EdgePayloadOutcome::RenderTarget { status, params } => {
                        edge_span.record("outcome", "render_target");
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
                            tracing::Span::current().record("terminal", true);
                            tracing::Span::current().record("duration_ms", elapsed_ms(start));
                            return Ok(terminal_edge_response(
                                self.finalize_response(response, request),
                            ));
                        };
                        tracing::Span::current().record("terminal", true);
                        tracing::Span::current().record("duration_ms", elapsed_ms(start));
                        return Ok(terminal_edge_response(self.with_edge_headers(
                            response,
                            &state,
                            cors_headers,
                            request,
                        )));
                    }
                }
            }
            tracing::Span::current().record("terminal", false);
            tracing::Span::current().record("duration_ms", elapsed_ms(start));
            Ok((None, filtered_headers(&state.headers)))
        }
        .instrument(span)
        .await
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
        let start = Instant::now();
        let span = tracing::info_span!(
            "rendermesh.static",
            origin_id,
            path,
            has_params = params.is_some(),
            status = status.as_u16(),
            hit = tracing::field::Empty,
            auto_index = tracing::field::Empty,
            duration_ms = tracing::field::Empty
        );
        async move {
            if let Some(response) = self
                .serve_object(origin_id, path, status, params, cors_headers)
                .await?
            {
                tracing::Span::current().record("hit", true);
                tracing::Span::current().record("auto_index", false);
                tracing::Span::current().record("duration_ms", elapsed_ms(start));
                return Ok(Some(response));
            }
            if config.edge.auto_rewrite_index {
                let candidate = auto_index_candidate(path);
                let response = self
                    .serve_object(origin_id, &candidate, status, params, cors_headers)
                    .await?;
                tracing::Span::current().record("hit", response.is_some());
                tracing::Span::current().record("auto_index", true);
                tracing::Span::current().record("duration_ms", elapsed_ms(start));
                return Ok(response);
            }
            tracing::Span::current().record("hit", false);
            tracing::Span::current().record("auto_index", false);
            tracing::Span::current().record("duration_ms", elapsed_ms(start));
            Ok(None)
        }
        .instrument(span)
        .await
    }

    async fn serve_object(
        &self,
        origin_id: &str,
        path: &str,
        status: StatusCode,
        params: Option<&serde_json::Value>,
        cors_headers: &BTreeMap<String, String>,
    ) -> Result<Option<RenderResponse>> {
        let start = Instant::now();
        let span = tracing::info_span!(
            "rendermesh.object",
            origin_id,
            path,
            has_params = params.is_some(),
            hit = tracing::field::Empty,
            rendered = tracing::field::Empty,
            duration_ms = tracing::field::Empty
        );
        async move {
            let Some(object) = self.safe_read_object(origin_id, path).await? else {
                tracing::Span::current().record("hit", false);
                tracing::Span::current().record("duration_ms", elapsed_ms(start));
                return Ok(None);
            };
            tracing::Span::current().record("hit", true);
            let mut headers = headers_from_metadata(&object.metadata);
            apply_headers(&mut headers, cors_headers.clone());
            let body = match params {
                Some(params) => match tracing::info_span!(
                    "rendermesh.template_render",
                    origin_id,
                    path,
                    duration_ms = tracing::field::Empty
                )
                .in_scope(|| {
                    let render_start = Instant::now();
                    let result = self.template_store.render(origin_id, path, params);
                    tracing::Span::current().record("duration_ms", elapsed_ms(render_start));
                    result
                }) {
                    Ok(body) => Bytes::from(body),
                    Err(TemplateStoreError::NotHtml) => {
                        tracing::Span::current().record("rendered", false);
                        return Ok(Some(RenderResponse {
                            status: StatusCode::UNSUPPORTED_MEDIA_TYPE,
                            headers,
                            body: Bytes::new(),
                        }));
                    }
                    Err(error) => {
                        tracing::Span::current().record("rendered", false);
                        tracing::error!("render template failed: {error}");
                        return Ok(Some(RenderResponse {
                            status: StatusCode::BAD_GATEWAY,
                            headers,
                            body: Bytes::new(),
                        }));
                    }
                },
                None => {
                    tracing::Span::current().record("rendered", false);
                    object.body
                }
            };
            if params.is_some() {
                tracing::Span::current().record("rendered", true);
            }
            tracing::Span::current().record("duration_ms", elapsed_ms(start));
            Ok(Some(RenderResponse {
                status,
                headers,
                body,
            }))
        }
        .instrument(span)
        .await
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
        let start = Instant::now();
        let span = tracing::info_span!(
            "rendermesh.missing",
            origin_id,
            action = ?config.missing.action,
            duration_ms = tracing::field::Empty
        );
        async move {
            match config.missing.action {
                crate::dto::edge::MissingAction::NotFound => {
                    if let Some(page) = &config.missing.page {
                        if let Some(mut response) = self
                            .serve_object(
                                origin_id,
                                page,
                                StatusCode::NOT_FOUND,
                                None,
                                cors_headers,
                            )
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
            .inspect(|_| {
                tracing::Span::current().record("duration_ms", elapsed_ms(start));
            })
        }
        .instrument(span)
        .await
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
