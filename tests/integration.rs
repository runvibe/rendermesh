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
    config::{AppConfig, CorsConfig, DEFAULT_BODY_LIMIT_BYTES},
    libs::telemetry,
    routes::create_router,
    state::AppState,
};
use serde_json::Value;
use tokio::sync::OnceCell;
use tower::ServiceExt;
use tracing::info_span;

use rendermesh::{
    repositories::local_mirror::LocalMirrorRepository,
    services::{
        cors::CorsPolicy,
        edge_config::default_edge_config,
        manifest::{parse_manifest_yaml, HostResolver},
        origin_runtime::{OriginRuntimeStore, OriginSnapshotDebug},
        render_gateway::RenderGatewayService,
        startup::build_render_runtime,
    },
};

async fn setup_router() -> Router {
    init_telemetry().await;

    let render_gateway = test_render_gateway(Path::new("./unused-render-mirror"));
    let state = AppState::new(render_gateway);
    let config = AppConfig {
        host: IpAddr::V4(Ipv4Addr::LOCALHOST),
        port: 0,
        cors: CorsConfig::Permissive,
        body_limit_bytes: DEFAULT_BODY_LIMIT_BYTES,
        otel_enabled: false,
        rendermesh_manifest: "./rendermesh.yaml".to_string(),
        admin_token: None,
    };

    create_router(state, &config)
}

fn setup_render_router(temp_root: &Path) -> Router {
    setup_render_router_with_admin_token(temp_root, None)
}

fn setup_render_router_with_admin_token(temp_root: &Path, admin_token: Option<String>) -> Router {
    let gateway = test_render_gateway(&temp_root.join("origins"));
    let runtime = OriginRuntimeStore::default();
    runtime.set_snapshot(OriginSnapshotDebug {
        origin_id: "web".to_string(),
        generation: 7,
        activated_at: "2026-05-22T14:00:00Z".to_string(),
        captured_at: "2026-05-22T13:59:59Z".to_string(),
        known_files: 2,
        added_files: 1,
        modified_files: 0,
        removed_files: 0,
        unchanged_files: 1,
        downloaded_files: 1,
        last_error: None,
        last_cdn_provider: None,
        last_cdn_status: None,
        last_cdn_request_id: None,
        last_cdn_refreshed_at: None,
        last_cdn_submitted_items: None,
        last_cdn_error: None,
        last_cdn_domain_provider: None,
        last_cdn_domain_status: None,
        last_cdn_domain_reconciled_at: None,
        last_cdn_domain_added: None,
        last_cdn_domain_updated: None,
        last_cdn_domain_removed: None,
        last_cdn_domain_unchanged: None,
        last_cdn_domain_error: None,
    });
    let state = AppState::new_with_runtime(gateway, runtime);
    let config = AppConfig {
        host: IpAddr::V4(Ipv4Addr::LOCALHOST),
        port: 0,
        cors: CorsConfig::Permissive,
        body_limit_bytes: 16,
        otel_enabled: false,
        rendermesh_manifest: "./rendermesh.yaml".to_string(),
        admin_token,
    };

    create_router(state, &config)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn debug_origin_routes_expose_runtime_snapshot() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = setup_render_router(temp.path());

    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/_rendermesh/origins")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("request failed");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["origins"][0]["origin_id"], "web");
    assert_eq!(body["origins"][0]["generation"], 7);
    assert_eq!(body["origins"][0]["known_files"], 2);

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/_rendermesh/origins/web/freshness")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("request failed");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["origin_id"], "web");
    assert_eq!(body["added_files"], 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_origin_sync_is_disabled_without_admin_token() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = setup_render_router(temp.path());

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_rendermesh/origins/web/sync")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("request failed");

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], "manual_sync_disabled");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_origin_sync_requires_authorization_header() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = setup_render_router_with_admin_token(temp.path(), Some("secret".to_string()));

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_rendermesh/origins/web/sync")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("request failed");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], "missing_authorization");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_origin_sync_rejects_invalid_admin_token() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = setup_render_router_with_admin_token(temp.path(), Some("secret".to_string()));

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_rendermesh/origins/web/sync")
                .header("authorization", "Bearer wrong")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("request failed");

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], "invalid_token");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_origin_sync_refreshes_local_origin_without_restart() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source_dir = temp.path().join("site");
    let mirror_dir = temp.path().join("mirror");
    tokio::fs::create_dir_all(source_dir.join(".rendermesh"))
        .await
        .expect("create source");
    write_local_edge_config(&source_dir, "serve")
        .await
        .expect("write edge config");
    tokio::fs::write(source_dir.join("index.html"), "version one")
        .await
        .expect("write index");
    let manifest_path = temp.path().join("rendermesh.yaml");
    tokio::fs::write(
        &manifest_path,
        format!(
            r#"
version: 1
runtime:
  local_store_dir: {}
  sync_interval_seconds: 3600
origins:
  web:
    type: local
    path: ./site
hosts:
  app.test:
    origin: web
"#,
            mirror_dir.display()
        ),
    )
    .await
    .expect("write manifest");

    let runtime = build_render_runtime(manifest_path.to_str().expect("manifest path"))
        .await
        .expect("runtime builds");
    let state = AppState::new_with_refresh(
        runtime.render_gateway,
        runtime.origin_runtime,
        runtime.origin_refresh,
    );
    let router = create_router(
        state,
        &AppConfig {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 0,
            cors: CorsConfig::Permissive,
            body_limit_bytes: DEFAULT_BODY_LIMIT_BYTES,
            otel_enabled: false,
            rendermesh_manifest: manifest_path.to_string_lossy().to_string(),
            admin_token: Some("secret".to_string()),
        },
    );

    tokio::fs::write(source_dir.join("index.html"), "version two")
        .await
        .expect("update index");

    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_rendermesh/origins/web/sync")
                .header("authorization", "Bearer secret")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("request failed");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["origin_id"], "web");
    assert_eq!(body["generation"], 2);
    assert_eq!(body["modified_files"], 1);

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/")
                .header("host", "app.test")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("request failed");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_bytes(response).await;
    assert_eq!(body, bytes::Bytes::from_static(b"version two"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_origin_sync_returns_not_found_for_unknown_origin() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source_dir = temp.path().join("site");
    let mirror_dir = temp.path().join("mirror");
    tokio::fs::create_dir_all(source_dir.join(".rendermesh"))
        .await
        .expect("create source");
    write_local_edge_config(&source_dir, "serve")
        .await
        .expect("write edge config");
    tokio::fs::write(source_dir.join("index.html"), "hello")
        .await
        .expect("write index");
    let manifest_path = temp.path().join("rendermesh.yaml");
    tokio::fs::write(
        &manifest_path,
        format!(
            r#"
version: 1
runtime:
  local_store_dir: {}
  sync_interval_seconds: 3600
origins:
  web:
    type: local
    path: ./site
hosts:
  app.test:
    origin: web
"#,
            mirror_dir.display()
        ),
    )
    .await
    .expect("write manifest");

    let runtime = build_render_runtime(manifest_path.to_str().expect("manifest path"))
        .await
        .expect("runtime builds");
    let state = AppState::new_with_refresh(
        runtime.render_gateway,
        runtime.origin_runtime,
        runtime.origin_refresh,
    );
    let router = create_router(
        state,
        &AppConfig {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 0,
            cors: CorsConfig::Permissive,
            body_limit_bytes: DEFAULT_BODY_LIMIT_BYTES,
            otel_enabled: false,
            rendermesh_manifest: manifest_path.to_string_lossy().to_string(),
            admin_token: Some("secret".to_string()),
        },
    );

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_rendermesh/origins/missing/sync")
                .header("authorization", "Bearer secret")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("request failed");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], "origin_not_found");
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

async fn write_local_edge_config(source_dir: &Path, missing_action: &str) -> std::io::Result<()> {
    let (field_name, field_value) = match missing_action {
        "serve" => ("path", "/index.html"),
        _ => ("page", "/index.html"),
    };
    tokio::fs::write(
        source_dir.join(".rendermesh/edge.yaml"),
        format!(
            r#"
version: 1
edge:
  root_object: /index.html
  auto_rewrite_index: true
missing:
  action: {missing_action}
  {field_name}: {field_value}
"#
        ),
    )
    .await
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
async fn openapi_routes_are_not_mounted() {
    let router = setup_router().await;

    for uri in ["/openapi.json", "/docs"] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(uri)
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::MISDIRECTED_REQUEST);
    }
}
