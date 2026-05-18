use std::{
    net::{IpAddr, Ipv4Addr},
    time::Duration,
};

use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
    response::Response,
    Router,
};
use http_body_util::BodyExt;
use rust_api_template::{
    config::{
        otel_enabled_from_env, AppConfig, CorsConfig, McpConfig, DEFAULT_BODY_LIMIT_BYTES,
        DEFAULT_MCP_PATH,
    },
    db::{init_pool, run_migrations},
    libs::telemetry,
    repositories::database::DatabaseRepository,
    routes::create_router,
    state::AppState,
};
use serde_json::Value;
use sqlx::PgPool;
use testcontainers::{
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
    GenericImage, ImageExt,
};
use tokio::sync::OnceCell;
use tower::ServiceExt;
use tracing::info_span;

async fn setup_router() -> Router {
    setup_router_with_mcp(false).await
}

async fn setup_router_with_mcp(mcp_enabled: bool) -> Router {
    let database_url = database_url().await;
    init_telemetry().await;
    let pool = init_pool_with_retry(&database_url).await;
    let pool_for_migrations = pool.clone();
    MIGRATIONS
        .get_or_init(|| async move {
            run_migrations(&pool_for_migrations)
                .await
                .expect("failed to run database migrations");
        })
        .await;

    let state = AppState::new(DatabaseRepository::new(pool));
    let config = AppConfig {
        database_url,
        host: IpAddr::V4(Ipv4Addr::LOCALHOST),
        port: 0,
        cors: CorsConfig::Permissive,
        body_limit_bytes: DEFAULT_BODY_LIMIT_BYTES,
        otel_enabled: otel_enabled_from_env(),
        mcp: McpConfig {
            enabled: mcp_enabled,
            path: DEFAULT_MCP_PATH.to_string(),
            cors: CorsConfig::Permissive,
        },
    };

    create_router(state, &config)
}

static MIGRATIONS: OnceCell<()> = OnceCell::const_new();
static TEST_DB_URL: OnceCell<String> = OnceCell::const_new();
static TELEMETRY_GUARD: OnceCell<telemetry::TelemetryGuard> = OnceCell::const_new();
static OTEL_ENDPOINT: OnceCell<String> = OnceCell::const_new();
static ENV_LOADED: OnceCell<()> = OnceCell::const_new();

async fn database_url() -> String {
    TEST_DB_URL
        .get_or_init(|| async {
            let image = GenericImage::new("pgvector/pgvector", "pg18")
                .with_exposed_port(5432.tcp())
                .with_wait_for(WaitFor::message_on_stdout(
                    "database system is ready to accept connections",
                ))
                .with_env_var("POSTGRES_PASSWORD", "postgres")
                .with_env_var("POSTGRES_USER", "postgres")
                .with_env_var("POSTGRES_DB", "postgres");
            let container = image
                .start()
                .await
                .expect("failed to start postgres container");
            let container = Box::leak(Box::new(container));
            let port = container
                .get_host_port_ipv4(5432)
                .await
                .expect("failed to resolve postgres mapped port");
            format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres")
        })
        .await
        .clone()
}

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
    if !otel_enabled_from_env() {
        TELEMETRY_GUARD
            .get_or_init(|| async {
                telemetry::init_tracing(false).expect("failed to init tracing")
            })
            .await;
        return;
    }

    let endpoint = otel_endpoint().await;
    if let Some(endpoint) = endpoint {
        set_env_if_missing("OTEL_EXPORTER_OTLP_PROTOCOL", "grpc");
        set_env_if_missing("OTEL_EXPORTER_OTLP_ENDPOINT", &endpoint);
    }
    set_env_if_missing("OTEL_EXPORTER_OTLP_TIMEOUT", "2000");
    set_env_if_missing("OTEL_EXPORTER_OTLP_TRACES_TIMEOUT", "2000");
    set_env_if_missing("OTEL_TRACES_SAMPLER", "always_on");
    set_env_if_missing("OTEL_USE_SIMPLE_EXPORTER", "true");
    set_env_if_missing("OTEL_BSP_SCHEDULE_DELAY", "200");
    set_env_if_missing(
        "OTEL_SERVICE_NAME",
        concat!(env!("CARGO_PKG_NAME"), "-tests"),
    );

    TELEMETRY_GUARD
        .get_or_init(|| async { telemetry::init_tracing(true).expect("failed to init tracing") })
        .await;
}

async fn otel_endpoint() -> Option<String> {
    if std::env::var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT").is_ok()
        || std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").is_ok()
    {
        return None;
    }

    Some(
        OTEL_ENDPOINT
            .get_or_init(|| async {
                let image = GenericImage::new("jaegertracing/jaeger", "latest")
                    .with_exposed_port(4317.tcp())
                    .with_wait_for(WaitFor::seconds(3))
                    .with_env_var("COLLECTOR_OTLP_ENABLED", "true")
                    .with_env_var("COLLECTOR_OTLP_GRPC_HOST_PORT", "0.0.0.0:4317");
                let container = image
                    .start()
                    .await
                    .expect("failed to start jaeger container");
                let container = Box::leak(Box::new(container));
                let port = container
                    .get_host_port_ipv4(4317)
                    .await
                    .expect("failed to resolve jaeger mapped port");
                format!("http://127.0.0.1:{port}")
            })
            .await
            .clone(),
    )
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

fn set_env_if_missing(key: &str, value: &str) {
    if std::env::var(key).is_err() {
        std::env::set_var(key, value);
    }
}

async fn init_pool_with_retry(database_url: &str) -> PgPool {
    let mut attempts = 0;
    loop {
        match init_pool(database_url).await {
            Ok(pool) => return pool,
            Err(err) => {
                attempts += 1;
                if attempts >= 10 {
                    panic!("failed to initialize database pool after {attempts} attempts: {err}");
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
}

async fn response_bytes(response: Response) -> bytes::Bytes {
    response
        .into_body()
        .collect()
        .await
        .expect("failed to read response body")
        .to_bytes()
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
