use std::collections::BTreeMap;

use axum::{
    extract::OriginalUri,
    http::{HeaderMap, Method},
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose, Engine as _};

use crate::{
    dto::echo::{EchoRequestInput, EchoResponse},
    services::echo,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/echo",
        get(get_echo)
            .post(post_echo)
            .put(put_echo)
            .patch(patch_echo)
            .delete(delete_echo)
            .head(head_echo)
            .options(options_echo),
    )
}

#[tracing::instrument(name = "echo.get", skip(headers, body))]
async fn get_echo(
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: String,
) -> Json<EchoResponse> {
    Json(echo::echo(build_request_input(method, uri, headers, body)))
}

#[tracing::instrument(name = "echo.post", skip(headers, body))]
async fn post_echo(
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: String,
) -> Json<EchoResponse> {
    Json(echo::echo(build_request_input(method, uri, headers, body)))
}

#[tracing::instrument(name = "echo.put", skip(headers, body))]
async fn put_echo(
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: String,
) -> Json<EchoResponse> {
    Json(echo::echo(build_request_input(method, uri, headers, body)))
}

#[tracing::instrument(name = "echo.patch", skip(headers, body))]
async fn patch_echo(
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: String,
) -> Json<EchoResponse> {
    Json(echo::echo(build_request_input(method, uri, headers, body)))
}

#[tracing::instrument(name = "echo.delete", skip(headers, body))]
async fn delete_echo(
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: String,
) -> Json<EchoResponse> {
    Json(echo::echo(build_request_input(method, uri, headers, body)))
}

#[tracing::instrument(name = "echo.head", skip(headers, body))]
async fn head_echo(
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: String,
) -> Json<EchoResponse> {
    Json(echo::echo(build_request_input(method, uri, headers, body)))
}

#[tracing::instrument(name = "echo.options", skip(headers, body))]
async fn options_echo(
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: String,
) -> Json<EchoResponse> {
    Json(echo::echo(build_request_input(method, uri, headers, body)))
}

fn build_request_input(
    method: Method,
    uri: axum::http::Uri,
    headers: HeaderMap,
    body: String,
) -> EchoRequestInput {
    EchoRequestInput {
        headers: headers_to_map(&headers),
        path: uri.path().to_string(),
        method: method.to_string(),
        body: (!body.is_empty()).then_some(body),
    }
}

fn headers_to_map(headers: &HeaderMap) -> BTreeMap<String, Vec<String>> {
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, value) in headers.iter() {
        let entry = map.entry(name.to_string()).or_default();
        match value.to_str() {
            Ok(as_str) => entry.push(as_str.to_string()),
            Err(_) => entry.push(general_purpose::STANDARD.encode(value.as_bytes())),
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{header::HeaderName, HeaderValue, Method, Uri};

    #[tokio::test]
    async fn build_request_input_mirrors_request() {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("x-test"),
            HeaderValue::from_static("value"),
        );
        let body = "payload".to_string();

        let response = build_request_input(Method::POST, Uri::from_static("/echo"), headers, body);

        assert_eq!(response.method, "POST");
        assert_eq!(response.path, "/echo");
        assert_eq!(response.body.as_deref(), Some("payload"));
        assert_eq!(
            response
                .headers
                .get("x-test")
                .and_then(|values| values.first())
                .map(String::as_str),
            Some("value")
        );
    }
}
