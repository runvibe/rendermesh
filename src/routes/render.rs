use axum::{
    body::{to_bytes, Body},
    extract::{Extension, OriginalUri, State},
    http::{header, HeaderName, HeaderValue, Method, Request, StatusCode},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use std::time::Instant;
use tracing::Instrument;

use crate::{
    dto::render::{RenderRequest, RenderResponse},
    libs::observability::elapsed_ms,
    state::AppState,
};

#[derive(Clone, Debug)]
pub struct RenderRouteConfig {
    pub body_limit_bytes: usize,
}

pub async fn render(
    State(state): State<AppState>,
    Extension(config): Extension<RenderRouteConfig>,
    OriginalUri(uri): OriginalUri,
    request: Request<Body>,
) -> Response {
    let (parts, body) = request.into_parts();
    let method = parts.method.clone();
    let host = parts
        .headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let path = uri.path().to_string();
    let has_query = uri.query().is_some();
    let span = tracing::info_span!(
        "rendermesh.request",
        method = %method,
        host = %host,
        path = %path,
        has_query,
        body_limit_bytes = config.body_limit_bytes,
        status = tracing::field::Empty,
        duration_ms = tracing::field::Empty
    );
    let span_for_record = span.clone();

    async move {
        let start = Instant::now();
        let read_body_span = tracing::info_span!(
            "rendermesh.read_body",
            method = %parts.method,
            body_limit_bytes = config.body_limit_bytes,
            body_bytes = tracing::field::Empty,
            duration_ms = tracing::field::Empty
        );
        let body = async {
            let read_start = Instant::now();
            let body = if matches!(parts.method, Method::GET | Method::HEAD | Method::OPTIONS) {
                match to_bytes(body, config.body_limit_bytes).await {
                    Ok(body) => body,
                    Err(error) => {
                        tracing::warn!("failed to read render request body: {error}");
                        let response = StatusCode::PAYLOAD_TOO_LARGE.into_response();
                        span_for_record.record("status", response.status().as_u16());
                        span_for_record.record("duration_ms", elapsed_ms(start));
                        return Err(response);
                    }
                }
            } else {
                Bytes::new()
            };
            tracing::Span::current().record("body_bytes", body.len());
            tracing::Span::current().record("duration_ms", elapsed_ms(read_start));
            Ok(body)
        }
        .instrument(read_body_span)
        .await;
        let body = match body {
            Ok(body) => body,
            Err(response) => return response,
        };

        let request = RenderRequest {
            method: parts.method,
            host,
            path,
            query: uri.query().map(ToString::to_string),
            scheme: "https".to_string(),
            headers: parts.headers,
            body,
        };

        match state.render_gateway().handle(request).await {
            Ok(response) => {
                let response = render_response(response);
                span_for_record.record("status", response.status().as_u16());
                span_for_record.record("duration_ms", elapsed_ms(start));
                response
            }
            Err(error) => {
                tracing::error!("render gateway failed: {error}");
                let response = StatusCode::INTERNAL_SERVER_ERROR.into_response();
                span_for_record.record("status", response.status().as_u16());
                span_for_record.record("duration_ms", elapsed_ms(start));
                response
            }
        }
    }
    .instrument(span)
    .await
}

fn render_response(response: RenderResponse) -> Response {
    let start = Instant::now();
    let span = tracing::info_span!(
        "rendermesh.response_build",
        status = response.status.as_u16(),
        header_count = response.headers.len(),
        body_bytes = response.body.len(),
        duration_ms = tracing::field::Empty
    );
    let _enter = span.enter();
    let mut builder = Response::builder().status(response.status);
    if let Some(headers) = builder.headers_mut() {
        for (name, value) in response.headers {
            let Ok(name) = HeaderName::from_bytes(name.as_bytes()) else {
                tracing::warn!(header = %name, "skipping invalid render response header name");
                continue;
            };
            let Ok(value) = HeaderValue::from_str(&value) else {
                tracing::warn!(header = %name, "skipping invalid render response header value");
                continue;
            };
            headers.insert(name, value);
        }
    }

    builder
        .body(Body::from(response.body))
        .inspect(|_| {
            span.record("duration_ms", elapsed_ms(start));
        })
        .unwrap_or_else(|error| {
            tracing::error!("failed to build render response: {error}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        })
}
