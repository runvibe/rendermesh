use std::{
    collections::BTreeMap,
    net::{IpAddr, Ipv4Addr},
    path::Path,
};

use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
    response::Response,
    Router,
};
use http_body_util::BodyExt;
use rendermesh::{
    config::{AppConfig, CorsConfig, McpConfig, DEFAULT_BODY_LIMIT_BYTES, DEFAULT_MCP_PATH},
    libs::telemetry,
    repositories::database::DatabaseRepository,
    routes::create_router,
    state::AppState,
};
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use tokio::sync::OnceCell;
use tower::ServiceExt;
use tracing::info_span;

use rendermesh::{
    repositories::local_mirror::LocalMirrorRepository,
    services::{
        cors::CorsPolicy,
        edge_config::default_edge_config,
        manifest::{parse_manifest_yaml, HostResolver},
        render_gateway::RenderGatewayService,
    },
};

async fn setup_router() -> Router {
    setup_router_with_mcp(false).await
}

async fn setup_router_with_mcp(mcp_enabled: bool) -> Router {
    let database_url = "postgres://postgres:postgres@localhost/test".to_string();
    init_telemetry().await;
    let pool = PgPoolOptions::new()
        .connect_lazy(&database_url)
        .expect("lazy pool");

    let render_gateway = test_render_gateway(Path::new("./unused-render-mirror"));
    let state = AppState::new(DatabaseRepository::new(pool), render_gateway);
    let config = AppConfig {
        database_url,
        host: IpAddr::V4(Ipv4Addr::LOCALHOST),
        port: 0,
        cors: CorsConfig::Permissive,
        body_limit_bytes: DEFAULT_BODY_LIMIT_BYTES,
        otel_enabled: false,
        rendermesh_manifest: "./rendermesh.yaml".to_string(),
        mcp: McpConfig {
            enabled: mcp_enabled,
            path: DEFAULT_MCP_PATH.to_string(),
            cors: CorsConfig::Permissive,
        },
    };

    create_router(state, &config)
}

fn setup_render_router(temp_root: &Path) -> Router {
    let gateway = test_render_gateway(&temp_root.join("origins"));
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://postgres:postgres@localhost/test")
        .expect("lazy pool");
    let state = AppState::new(DatabaseRepository::new(pool), gateway);
    let config = AppConfig {
        database_url: "postgres://postgres:postgres@localhost/test".to_string(),
        host: IpAddr::V4(Ipv4Addr::LOCALHOST),
        port: 0,
        cors: CorsConfig::Permissive,
        body_limit_bytes: 16,
        otel_enabled: false,
        rendermesh_manifest: "./rendermesh.yaml".to_string(),
        mcp: McpConfig {
            enabled: false,
            path: DEFAULT_MCP_PATH.to_string(),
            cors: CorsConfig::Permissive,
        },
    };

    create_router(state, &config)
}

fn test_render_gateway(mirror_root: &Path) -> RenderGatewayService {
    let manifest = parse_manifest_yaml(
        r#"
version: 1
runtime:
  local_store_dir: ./unused
  sync_interval_seconds: 60
origins:
  web:
    type: s3
    bucket: web
    endpoint_env: WEB_ENDPOINT
    region_env: WEB_REGION
    access_key_id_env: WEB_ACCESS_KEY_ID
    secret_access_key_env: WEB_SECRET_ACCESS_KEY
hosts:
  app.test:
    origin: web
"#,
    )
    .expect("manifest parses");
    RenderGatewayService::new_for_tests(
        HostResolver::new(&manifest).expect("resolver builds"),
        CorsPolicy::from_manifest(&manifest),
        LocalMirrorRepository::new(mirror_root),
        BTreeMap::from([("web".to_string(), default_edge_config())]),
    )
}

static TELEMETRY_GUARD: OnceCell<telemetry::TelemetryGuard> = OnceCell::const_new();
static ENV_LOADED: OnceCell<()> = OnceCell::const_new();

async fn response_json(response: Response) -> Value {
    let body = response
        .into_body()
        .collect()
        .await
        .expect("failed to read response body")
        .to_bytes();
    serde_json::from_slice(&body).expect("failed to parse json response")
}

async fn init_telemetry() {
    load_env().await;
    TELEMETRY_GUARD
        .get_or_init(|| async { telemetry::init_tracing(false).expect("failed to init tracing") })
        .await;
}

async fn flush_telemetry() {
    if let Some(guard) = TELEMETRY_GUARD.get() {
        guard.force_flush().expect("failed to flush telemetry");
    }
}

async fn load_env() {
    ENV_LOADED
        .get_or_init(|| async {
            dotenvy::dotenv().ok();
        })
        .await;
}

async fn response_bytes(response: Response) -> bytes::Bytes {
    response
        .into_body()
        .collect()
        .await
        .expect("failed to read response body")
        .to_bytes()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn render_route_serves_host_mapped_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let object_dir = temp.path().join("origins/web/assets");
    tokio::fs::create_dir_all(&object_dir).await.expect("mkdir");
    tokio::fs::write(object_dir.join("hello.txt"), "hello from mirror")
        .await
        .expect("write object");
    let router = setup_render_router(temp.path());

    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/assets/hello.txt?cache=1")
                .header("host", "app.test")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("request failed");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_bytes(response).await;
    assert_eq!(body, bytes::Bytes::from_static(b"hello from mirror"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn render_route_rejects_unknown_host() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = setup_render_router(temp.path());

    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/")
                .header("host", "unknown.test")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("request failed");

    assert_eq!(response.status(), StatusCode::MISDIRECTED_REQUEST);
    let body = response_bytes(response).await;
    assert!(body.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn render_route_rejects_missing_host_as_misdirected_request() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = setup_render_router(temp.path());

    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("request failed");

    assert_eq!(response.status(), StatusCode::MISDIRECTED_REQUEST);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn render_route_applies_body_limit_to_fallback() {
    let temp = tempfile::tempdir().expect("tempdir");
    let object_dir = temp.path().join("origins/web/assets");
    tokio::fs::create_dir_all(&object_dir).await.expect("mkdir");
    tokio::fs::write(object_dir.join("hello.txt"), "hello from mirror")
        .await
        .expect("write object");
    let router = setup_render_router(temp.path());

    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/assets/hello.txt")
                .header("host", "app.test")
                .body(Body::from("this body is too large"))
                .expect("build request"),
        )
        .await
        .expect("request failed");

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

fn mcp_request(method: &str, id: i64, params: Value) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-06-18")
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params,
            })
            .to_string(),
        ))
        .expect("build mcp request")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn health_returns_status() {
    let _span = info_span!("integration_test", test = "health").entered();
    let router = setup_router().await;
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request failed");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["status"], "ok");
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    flush_telemetry().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn echo_routes_reflect_request() {
    let _span = info_span!("integration_test", test = "echo").entered();
    let router = setup_router().await;
    let methods = [
        Method::GET,
        Method::POST,
        Method::PUT,
        Method::PATCH,
        Method::DELETE,
        Method::OPTIONS,
    ];

    for method in methods {
        let request = Request::builder()
            .method(method.clone())
            .uri("/echo")
            .header("x-test", "value")
            .body(Body::from("payload"))
            .expect("build request");

        let response = router
            .clone()
            .oneshot(request)
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::OK);
        if method == Method::OPTIONS {
            let body = response_bytes(response).await;
            if !body.is_empty() {
                let json_body: Value =
                    serde_json::from_slice(&body).expect("failed to parse json response");
                assert_eq!(json_body["method"], method.as_str());
                assert_eq!(json_body["path"], "/echo");
                assert_eq!(json_body["body"], "payload");
                assert_eq!(json_body["headers"]["x-test"][0], "value");
            }
            continue;
        }

        let body = response_json(response).await;
        assert_eq!(body["method"], method.as_str());
        assert_eq!(body["path"], "/echo");
        assert_eq!(body["body"], "payload");
        assert_eq!(body["headers"]["x-test"][0], "value");
    }

    let head_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::HEAD)
                .uri("/echo")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("request failed");

    assert_eq!(head_response.status(), StatusCode::OK);
    flush_telemetry().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_endpoint_is_absent_when_disabled() {
    let _span = info_span!("integration_test", test = "mcp_disabled").entered();
    let router = setup_router().await;
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/mcp")
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream")
                .body(Body::from("{}"))
                .expect("build request"),
        )
        .await
        .expect("request failed");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    flush_telemetry().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_initialize_and_tools_work_over_http() {
    let _span = info_span!("integration_test", test = "mcp_http").entered();
    let router = setup_router_with_mcp(true).await;

    let initialize = router
        .clone()
        .oneshot(mcp_request(
            "initialize",
            1,
            serde_json::json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {
                    "name": "integration-test",
                    "version": "1.0"
                }
            }),
        ))
        .await
        .expect("initialize request failed");

    assert_eq!(initialize.status(), StatusCode::OK);
    let initialize_body = response_json(initialize).await;
    assert_eq!(initialize_body["jsonrpc"], "2.0");
    assert_eq!(initialize_body["id"], 1);
    assert_eq!(initialize_body["result"]["protocolVersion"], "2025-06-18");

    let tools_list = router
        .clone()
        .oneshot(mcp_request("tools/list", 2, serde_json::json!({})))
        .await
        .expect("tools/list request failed");

    assert_eq!(tools_list.status(), StatusCode::OK);
    let tools_body = response_json(tools_list).await;
    let tools = tools_body["result"]["tools"]
        .as_array()
        .expect("tools list must be an array");

    assert!(tools.iter().any(|tool| tool["name"] == "health_check"));
    assert!(tools.iter().any(|tool| tool["name"] == "echo_request"));

    let health_call = router
        .clone()
        .oneshot(mcp_request(
            "tools/call",
            3,
            serde_json::json!({
                "name": "health_check",
                "arguments": {}
            }),
        ))
        .await
        .expect("health_check request failed");

    assert_eq!(health_call.status(), StatusCode::OK);
    let health_body = response_json(health_call).await;
    assert_eq!(
        health_body["result"]["structuredContent"]["status"],
        Value::String("ok".to_string())
    );
    assert_eq!(
        health_body["result"]["structuredContent"]["version"],
        Value::String(env!("CARGO_PKG_VERSION").to_string())
    );

    let echo_call = router
        .clone()
        .oneshot(mcp_request(
            "tools/call",
            4,
            serde_json::json!({
                "name": "echo_request",
                "arguments": {
                    "method": "POST",
                    "path": "/echo",
                    "headers": {
                        "x-test": ["value"]
                    },
                    "body": "payload"
                }
            }),
        ))
        .await
        .expect("echo_request failed");

    assert_eq!(echo_call.status(), StatusCode::OK);
    let echo_body = response_json(echo_call).await;
    assert_eq!(echo_body["result"]["structuredContent"]["method"], "POST");
    assert_eq!(echo_body["result"]["structuredContent"]["path"], "/echo");
    assert_eq!(echo_body["result"]["structuredContent"]["body"], "payload");
    assert_eq!(
        echo_body["result"]["structuredContent"]["headers"]["x-test"][0],
        "value"
    );
    flush_telemetry().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_get_is_rejected_in_stateless_mode() {
    let _span = info_span!("integration_test", test = "mcp_get").entered();
    let router = setup_router_with_mcp(true).await;
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/mcp")
                .header("accept", "text/event-stream")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("request failed");

    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    flush_telemetry().await;
}
