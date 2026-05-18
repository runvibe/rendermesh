use super::*;
use crate::repositories::local_mirror::{metadata_sidecar_path, LocalMirrorRepository};
use crate::services::cors::CorsPolicy;
use crate::services::edge_config::{default_edge_config, parse_edge_config_yaml};
use crate::services::edge_config_store::EdgeConfigStore;
use crate::services::manifest::{parse_manifest_yaml, HostResolver};
use crate::services::template_store::TemplateStore;
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
async fn resolved_host_invalid_edge_config_returns_500() {
    let temp = tempfile::tempdir().expect("tempdir");
    let manifest = test_manifest();
    let edge_configs = EdgeConfigStore::from_configs(BTreeMap::new());
    edge_configs.set_invalid("web", "invalid edge config");
    let service = RenderGatewayService::new_with_edge_config_store(
        HostResolver::new(&manifest).expect("resolver"),
        CorsPolicy::from_manifest(&manifest),
        LocalMirrorRepository::new(temp.path().join("origins")),
        edge_configs,
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
async fn malformed_object_path_is_treated_as_not_found() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_object(temp.path(), "index.html", "<h1>Shell</h1>", None).await;
    let service = test_gateway(temp.path().join("origins"));

    let response = service
        .handle(test_request(Method::GET, "/../secret"))
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
async fn edge_hook_request_body_is_empty_for_mvp() {
    let request = RenderRequest {
        body: bytes::Bytes::from_static(b"ignored"),
        ..test_request(Method::GET, "/")
    };

    let edge_request = edge_hook_request(&request);

    assert_eq!(edge_request.body, "");
}

#[tokio::test]
async fn edge_file_path_outcome_serves_specified_file_with_edge_status() {
    let server = edge_server(
        203,
        serde_json::json!({
            "file_path": "/edge.html",
            "headers": {"x-edge": "file", "access-control-allow-origin": "*"}
        }),
    )
    .await;
    let temp = tempfile::tempdir().expect("tempdir");
    write_object(temp.path(), "edge.html", "<h1>Edge File</h1>", None).await;
    let service = test_gateway_with_edge_url(temp.path().join("origins"), &server.uri());
    let response = service
        .handle(test_request_with_origin(Method::GET, "/"))
        .await
        .expect("response");
    assert_eq!(response.status, StatusCode::NON_AUTHORITATIVE_INFORMATION);
    assert_eq!(
        response.body,
        bytes::Bytes::from_static(b"<h1>Edge File</h1>")
    );
    assert_header(&response, "x-edge", "file");
    assert_header(&response, "access-control-allow-origin", "https://web.test");
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
    let template_store = TemplateStore::default();
    template_store
        .load_origin_templates(
            "web",
            &LocalMirrorRepository::new(temp.path().join("origins")),
        )
        .await
        .expect("templates load");
    let service = test_gateway_with_edge_url_and_templates(
        temp.path().join("origins"),
        &server.uri(),
        template_store,
    );
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
async fn edge_params_render_uses_loaded_template_store() {
    let server = edge_server(
        200,
        serde_json::json!({
            "params": {"title": "Loaded"}
        }),
    )
    .await;
    let temp = tempfile::tempdir().expect("tempdir");
    write_object(
        temp.path(),
        "index.html",
        "<h1>{{title}} from memory</h1>",
        Some(r#"{"content_type":"text/html"}"#),
    )
    .await;
    let template_store = TemplateStore::default();
    template_store
        .load_origin_templates(
            "web",
            &LocalMirrorRepository::new(temp.path().join("origins")),
        )
        .await
        .expect("templates load");
    write_object(
        temp.path(),
        "index.html",
        "<h1>{{title}} from disk</h1>",
        Some(r#"{"content_type":"text/html"}"#),
    )
    .await;
    let service = test_gateway_with_edge_url_and_templates(
        temp.path().join("origins"),
        &server.uri(),
        template_store,
    );

    let response = service
        .handle(test_request(Method::GET, "/"))
        .await
        .expect("response");

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        response.body,
        bytes::Bytes::from_static(b"<h1>Loaded from memory</h1>")
    );
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
    test_gateway_with_edge_url_and_templates(root, edge_base_url, TemplateStore::default())
}

fn test_gateway_with_edge_url_and_templates(
    root: std::path::PathBuf,
    edge_base_url: &str,
    template_store: TemplateStore,
) -> RenderGatewayService {
    test_gateway_with_config_and_templates(
        root,
        edge_config(&format!(
            r#"edges:
  - name: test
    url: {edge_base_url}/edge
    timeout_ms: 500
"#
        )),
        template_store,
    )
}
fn test_gateway_with_config(root: std::path::PathBuf, config: EdgeConfig) -> RenderGatewayService {
    test_gateway_with_config_and_templates(root, config, TemplateStore::default())
}

fn test_gateway_with_config_and_templates(
    root: std::path::PathBuf,
    config: EdgeConfig,
    template_store: TemplateStore,
) -> RenderGatewayService {
    let manifest = test_manifest();

    RenderGatewayService::new_for_tests_with_template_store(
        HostResolver::new(&manifest).expect("resolver"),
        CorsPolicy::from_manifest(&manifest),
        LocalMirrorRepository::new(root),
        [("web".to_string(), config)].into(),
        template_store,
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
async fn write_object(temp_path: &std::path::Path, key: &str, body: &str, metadata: Option<&str>) {
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
