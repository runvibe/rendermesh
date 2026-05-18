use axum::{
    body::{to_bytes, Body},
    extract::{OriginalUri, State},
    http::{header, HeaderName, HeaderValue, Request, StatusCode},
    response::{IntoResponse, Response},
};

use crate::{
    dto::render::{RenderRequest, RenderResponse},
    state::AppState,
};

pub async fn render(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    request: Request<Body>,
) -> Response {
    let (parts, body) = request.into_parts();
    let body = match to_bytes(body, usize::MAX).await {
        Ok(body) => body,
        Err(error) => {
            tracing::warn!("failed to read render request body: {error}");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let Some(host) = parts
        .headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let request = RenderRequest {
        method: parts.method,
        host: host.to_string(),
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
