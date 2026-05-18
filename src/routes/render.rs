use axum::{
    body::{to_bytes, Body},
    extract::{Extension, OriginalUri, State},
    http::{header, HeaderName, HeaderValue, Method, Request, StatusCode},
    response::{IntoResponse, Response},
};
use bytes::Bytes;

use crate::{
    dto::render::{RenderRequest, RenderResponse},
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
    let host = parts
        .headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();

    let body = if matches!(parts.method, Method::GET | Method::HEAD | Method::OPTIONS) {
        match to_bytes(body, config.body_limit_bytes).await {
            Ok(body) => body,
            Err(error) => {
                tracing::warn!("failed to read render request body: {error}");
                return StatusCode::PAYLOAD_TOO_LARGE.into_response();
            }
        }
    } else {
        Bytes::new()
    };

    let request = RenderRequest {
        method: parts.method,
        host,
        path: uri.path().to_string(),
        query: uri.query().map(ToString::to_string),
        scheme: "https".to_string(),
        headers: parts.headers,
        body,
    };

    match state.render_gateway().handle(request).await {
        Ok(response) => render_response(response),
        Err(error) => {
            tracing::error!("render gateway failed: {error}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

fn render_response(response: RenderResponse) -> Response {
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
        .unwrap_or_else(|error| {
            tracing::error!("failed to build render response: {error}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        })
}
