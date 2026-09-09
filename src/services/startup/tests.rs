use std::collections::BTreeMap;

use anyhow::Result;
use async_trait::async_trait;
use bytes::Bytes;
use serde_json::json;
use wiremock::{
    matchers::{header, method, path},
    Mock, MockServer, ResponseTemplate,
};

use super::*;
use crate::{
    repositories::{
        local_mirror::LocalMirrorRepository,
        sync::{MirrorSyncService, RemoteObject, RemoteObjectSummary, RemoteStorage},
    },
    services::edge_config_store::{EdgeConfigStore, EdgeConfigStoreError},
    services::origin_refresh::{
        load_edge_configs, refresh_origin_snapshot, sync_origin_and_refresh_edge_config,
        OriginRefreshTrigger,
    },
};

#[tokio::test]
async fn load_edge_configs_keeps_invalid_origin_in_store() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mirror = LocalMirrorRepository::new(temp.path().join("origins"));
    write_mirror_file(
        temp.path(),
        "_rendermesh/edge.yaml",
        "version: nope\nmissing:",
    )
    .await;

    let store = load_edge_configs(["web".to_string()], &mirror).await;

    assert!(matches!(
        store.get("web"),
        Err(EdgeConfigStoreError::Invalid(_))
    ));
}

#[tokio::test]
async fn sync_origin_refreshes_edge_config_store() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("origins");
    let mirror = LocalMirrorRepository::new(&root);
    let syncer = MirrorSyncService::new(&root);
    let store = EdgeConfigStore::from_configs(BTreeMap::new());
    store.set_invalid("web", "old error");
    let storage = StaticStorage::new(BTreeMap::from([(
        "_rendermesh/edge.yaml".to_string(),
        edge_object(
            r#"
version: 1
edge:
  root_object: /home.html
  auto_rewrite_index: false
missing:
  action: not_found
  page: /home.html
"#,
        ),
    )]));

    let template_store = TemplateStore::default();
    sync_origin_and_refresh_edge_config(
        "web",
        &syncer,
        &storage,
        None,
        &mirror,
        &store,
        &template_store,
    )
    .await
    .expect("sync succeeds");

    let config = store.get("web").expect("config refreshed");
    assert_eq!(config.edge.root_object, "/home.html");
    assert!(!config.edge.auto_rewrite_index);
}

#[tokio::test]
async fn sync_origin_prefers_dot_rendermesh_edge_config_over_legacy_config() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("origins");
    let mirror = LocalMirrorRepository::new(&root);
    let syncer = MirrorSyncService::new(&root);
    let store = EdgeConfigStore::from_configs(BTreeMap::new());
    let storage = StaticStorage::new(BTreeMap::from([
        (
            "_rendermesh/edge.yaml".to_string(),
            edge_object(
                r#"
version: 1
edge:
  root_object: /legacy.html
  auto_rewrite_index: false
missing:
  action: not_found
  page: /legacy.html
"#,
            ),
        ),
        (
            ".rendermesh/edge.yaml".to_string(),
            yaml_edge_object(
                ".rendermesh/edge.yaml",
                r#"
version: 1
edge:
  root_object: /dot.html
  auto_rewrite_index: false
missing:
  action: not_found
  page: /dot.html
"#,
            ),
        ),
    ]));

    let template_store = TemplateStore::default();
    sync_origin_and_refresh_edge_config(
        "web",
        &syncer,
        &storage,
        None,
        &mirror,
        &store,
        &template_store,
    )
    .await
    .expect("sync succeeds");

    let config = store.get("web").expect("config refreshed");
    assert_eq!(config.edge.root_object, "/dot.html");
}

#[tokio::test]
async fn sync_origin_refreshes_edge_config_store_from_json_object() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("origins");
    let mirror = LocalMirrorRepository::new(&root);
    let syncer = MirrorSyncService::new(&root);
    let store = EdgeConfigStore::from_configs(BTreeMap::new());
    store.set_invalid("web", "old error");
    let storage = StaticStorage::new(BTreeMap::from([(
        "_rendermesh/edge.json".to_string(),
        json_edge_object(
            "_rendermesh/edge.json",
            r#"
{
  "version": 1,
  "edge": {
    "root_object": "/json-sync.html",
    "auto_rewrite_index": false
  },
  "missing": {
    "action": "not_found",
    "page": "/json-sync.html"
  }
}
"#,
        ),
    )]));

    let template_store = TemplateStore::default();
    sync_origin_and_refresh_edge_config(
        "web",
        &syncer,
        &storage,
        None,
        &mirror,
        &store,
        &template_store,
    )
    .await
    .expect("sync succeeds");

    let config = store.get("web").expect("json config refreshed");
    assert_eq!(config.edge.root_object, "/json-sync.html");
    assert!(!config.edge.auto_rewrite_index);
}

#[tokio::test]
async fn load_edge_configs_reads_dot_rendermesh_json_when_yaml_is_missing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mirror = LocalMirrorRepository::new(temp.path().join("origins"));
    write_mirror_file(
        temp.path(),
        ".rendermesh/edge.json",
        r#"
{
  "version": 1,
  "edge": {
    "root_object": "/dot-json.html",
    "auto_rewrite_index": false
  },
  "missing": {
    "action": "not_found",
    "page": "/dot-json.html"
  }
}
"#,
    )
    .await;

    let store = load_edge_configs(["web".to_string()], &mirror).await;

    let config = store.get("web").expect("json config loaded");
    assert_eq!(config.edge.root_object, "/dot-json.html");
}

#[tokio::test]
async fn load_edge_configs_reads_json_when_yaml_is_missing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mirror = LocalMirrorRepository::new(temp.path().join("origins"));
    write_mirror_file(
        temp.path(),
        "_rendermesh/edge.json",
        r#"
{
  "version": 1,
  "edge": {
    "root_object": "/json.html",
    "auto_rewrite_index": false
  },
  "missing": {
    "action": "not_found",
    "page": "/json.html"
  }
}
"#,
    )
    .await;

    let store = load_edge_configs(["web".to_string()], &mirror).await;

    let config = store.get("web").expect("json config loaded");
    assert_eq!(config.edge.root_object, "/json.html");
    assert!(!config.edge.auto_rewrite_index);
}

#[tokio::test]
async fn load_edge_configs_reads_yml_when_yaml_is_missing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mirror = LocalMirrorRepository::new(temp.path().join("origins"));
    write_mirror_file(
        temp.path(),
        "_rendermesh/edge.yml",
        r#"
version: 1
edge:
  root_object: /yml.html
  auto_rewrite_index: false
missing:
  action: not_found
  page: /yml.html
"#,
    )
    .await;

    let store = load_edge_configs(["web".to_string()], &mirror).await;

    let config = store.get("web").expect("yml config loaded");
    assert_eq!(config.edge.root_object, "/yml.html");
    assert!(!config.edge.auto_rewrite_index);
}

#[tokio::test]
async fn load_edge_configs_prefers_yaml_over_json() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mirror = LocalMirrorRepository::new(temp.path().join("origins"));
    write_mirror_file(
        temp.path(),
        "_rendermesh/edge.yaml",
        r#"
version: 1
edge:
  root_object: /yaml.html
  auto_rewrite_index: false
missing:
  action: not_found
  page: /yaml.html
"#,
    )
    .await;
    write_mirror_file(
        temp.path(),
        "_rendermesh/edge.json",
        r#"
{
  "version": 1,
  "edge": {
    "root_object": "/json.html",
    "auto_rewrite_index": true
  },
  "missing": {
    "action": "not_found",
    "page": "/json.html"
  }
}
"#,
    )
    .await;

    let store = load_edge_configs(["web".to_string()], &mirror).await;

    let config = store.get("web").expect("config loaded");
    assert_eq!(config.edge.root_object, "/yaml.html");
    assert!(!config.edge.auto_rewrite_index);
}

#[tokio::test]
async fn load_edge_configs_prefers_dot_rendermesh_yaml_over_legacy_yaml() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mirror = LocalMirrorRepository::new(temp.path().join("origins"));
    write_mirror_file(
        temp.path(),
        "_rendermesh/edge.yaml",
        r#"
version: 1
edge:
  root_object: /legacy-yaml.html
  auto_rewrite_index: false
missing:
  action: not_found
  page: /legacy-yaml.html
"#,
    )
    .await;
    write_mirror_file(
        temp.path(),
        ".rendermesh/edge.yaml",
        r#"
version: 1
edge:
  root_object: /dot-yaml.html
  auto_rewrite_index: false
missing:
  action: not_found
  page: /dot-yaml.html
"#,
    )
    .await;

    let store = load_edge_configs(["web".to_string()], &mirror).await;

    let config = store.get("web").expect("config loaded");
    assert_eq!(config.edge.root_object, "/dot-yaml.html");
}

#[tokio::test]
async fn sync_origin_refreshes_template_store() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("origins");
    let mirror = LocalMirrorRepository::new(&root);
    let syncer = MirrorSyncService::new(&root);
    let edge_configs = EdgeConfigStore::from_configs(BTreeMap::new());
    let template_store = TemplateStore::default();
    let storage = StaticStorage::new(BTreeMap::from([(
        "index.html".to_string(),
        RemoteObject {
            key: "index.html".to_string(),
            body: Bytes::from_static(b"<h1>{{title}}</h1>"),
            etag: Some("index".to_string()),
            last_modified: None,
            content_type: Some("text/html".to_string()),
            cache_control: None,
        },
    )]));

    sync_origin_and_refresh_edge_config(
        "web",
        &syncer,
        &storage,
        None,
        &mirror,
        &edge_configs,
        &template_store,
    )
    .await
    .expect("sync succeeds");

    assert_eq!(
        template_store
            .render("web", "/index.html", &serde_json::json!({"title":"Synced"}))
            .expect("template renders"),
        "<h1>Synced</h1>"
    );
}

#[tokio::test]
async fn failed_template_refresh_keeps_previous_mirror_and_templates() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("origins");
    let mirror = LocalMirrorRepository::new(&root);
    let syncer = MirrorSyncService::new(&root);
    let edge_configs = EdgeConfigStore::from_configs(BTreeMap::new());
    let template_store = TemplateStore::default();
    let storage = StaticStorage::new(BTreeMap::from([(
        "index.html".to_string(),
        RemoteObject {
            key: "index.html".to_string(),
            body: Bytes::from_static(b"<h1>{{title}}</h1>"),
            etag: Some("index-v1".to_string()),
            last_modified: None,
            content_type: Some("text/html".to_string()),
            cache_control: None,
        },
    )]));

    sync_origin_and_refresh_edge_config(
        "web",
        &syncer,
        &storage,
        None,
        &mirror,
        &edge_configs,
        &template_store,
    )
    .await
    .expect("initial sync succeeds");

    let storage = StaticStorage::new(BTreeMap::from([(
        "index.html".to_string(),
        RemoteObject {
            key: "index.html".to_string(),
            body: Bytes::from_static(b"<h1>{{#if}}</h1>"),
            etag: Some("index-v2".to_string()),
            last_modified: None,
            content_type: Some("text/html".to_string()),
            cache_control: None,
        },
    )]));

    sync_origin_and_refresh_edge_config(
        "web",
        &syncer,
        &storage,
        None,
        &mirror,
        &edge_configs,
        &template_store,
    )
    .await
    .expect_err("invalid template prevents activation");

    assert_eq!(
        mirror
            .read_object("web", "/index.html")
            .await
            .expect("mirror read")
            .expect("object exists")
            .body,
        Bytes::from_static(b"<h1>{{title}}</h1>")
    );
    assert_eq!(
        template_store
            .render("web", "/index.html", &serde_json::json!({"title":"Stable"}))
            .expect("old template still renders"),
        "<h1>Stable</h1>"
    );
}

#[tokio::test]
async fn refresh_origin_snapshot_updates_runtime_generation() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("origins");
    let syncer = MirrorSyncService::new(&root);
    let edge_configs = EdgeConfigStore::from_configs(BTreeMap::new());
    let template_store = TemplateStore::default();
    let freshness_indexes = OriginFreshnessIndexes::default();
    let origin_runtime = crate::services::origin_runtime::OriginRuntimeStore::default();
    let storage = StaticStorage::new(BTreeMap::from([(
        "index.html".to_string(),
        RemoteObject {
            key: "index.html".to_string(),
            body: Bytes::from_static(b"<h1>{{title}}</h1>"),
            etag: Some("index-v1".to_string()),
            last_modified: None,
            content_type: Some("text/html".to_string()),
            cache_control: None,
        },
    )]));

    refresh_origin_snapshot(
        "web",
        &syncer,
        &storage,
        None,
        &edge_configs,
        &template_store,
        &freshness_indexes,
        &origin_runtime,
        None,
    )
    .await
    .expect("refresh succeeds");

    let snapshot = origin_runtime.get("web").expect("runtime snapshot");
    assert_eq!(snapshot.generation, 1);
    assert_eq!(snapshot.known_files, 1);
    assert_eq!(snapshot.added_files, 1);
    assert_eq!(snapshot.downloaded_files, 1);
    assert_eq!(snapshot.last_error, None);
}

#[tokio::test]
async fn build_render_runtime_syncs_local_origin_relative_to_manifest_path() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config_dir = temp.path().join("config");
    let source_dir = config_dir.join("site");
    let mirror_dir = temp.path().join("var/origins");
    tokio::fs::create_dir_all(source_dir.join(".rendermesh"))
        .await
        .expect("create source dir");
    tokio::fs::write(source_dir.join("index.html"), "<h1>{{title}}</h1>")
        .await
        .expect("write index");
    tokio::fs::write(
        source_dir.join(".rendermesh/edge.yaml"),
        r#"
version: 1
edge:
  root_object: /index.html
  auto_rewrite_index: true
missing:
  action: not_found
  page: /index.html
"#,
    )
    .await
    .expect("write edge config");

    let manifest_path = config_dir.join("rendermesh.yaml");
    tokio::fs::write(
        &manifest_path,
        format!(
            r#"
version: 1
runtime:
  local_store_dir: {}
  sync_interval_seconds: 60
origins:
  web:
    type: local
    path: ./site
hosts:
  web.test:
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

    let snapshot = runtime.origin_runtime.get("web").expect("origin snapshot");
    assert_eq!(snapshot.generation, 1);
    assert_eq!(snapshot.known_files, 2);
    assert_eq!(snapshot.downloaded_files, 2);
    assert_eq!(
        tokio::fs::read_to_string(mirror_dir.join("web/index.html"))
            .await
            .expect("mirror index exists"),
        "<h1>{{title}}</h1>"
    );
}

#[tokio::test]
async fn activation_barrier_keeps_the_previous_mirror_when_content_changes_alone() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config_dir = temp.path().join("config");
    let source_dir = config_dir.join("site");
    let mirror_dir = temp.path().join("var/origins");
    tokio::fs::create_dir_all(source_dir.join(".rendermesh"))
        .await
        .expect("create source dir");
    tokio::fs::write(source_dir.join("index.html"), "<h1>stable</h1>")
        .await
        .expect("write index");
    tokio::fs::write(
        source_dir.join(".rendermesh/edge.yaml"),
        "version: 1\nedge:\n  root_object: /index.html\nmissing:\n  action: not_found\n",
    )
    .await
    .expect("write barrier");
    let manifest_path = config_dir.join("rendermesh.yaml");
    tokio::fs::write(
        &manifest_path,
        format!(
            r#"
version: 1
runtime:
  local_store_dir: {}
  sync_interval_seconds: 60
origins:
  web:
    type: local
    path: ./site
    activation_barrier_path: .rendermesh/edge.yaml
hosts:
  web.test:
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

    tokio::fs::write(source_dir.join("index.html"), "<h1>partial</h1>")
        .await
        .expect("mutate index without barrier");

    let error = runtime
        .origin_refresh
        .refresh_origin("web", OriginRefreshTrigger::Manual)
        .await
        .expect_err("unchanged barrier blocks activation");

    assert!(error.to_string().contains("did not change"));
    assert_eq!(
        tokio::fs::read_to_string(mirror_dir.join("web/index.html"))
            .await
            .expect("active mirror"),
        "<h1>stable</h1>"
    );
    let mut staged_entries = tokio::fs::read_dir(mirror_dir.join(".rendermesh-sync"))
        .await
        .expect("read staging root");
    assert!(
        staged_entries
            .next_entry()
            .await
            .expect("read staging entry")
            .is_none(),
        "rejected activation must remove its staged mirror"
    );
}

#[tokio::test]
async fn build_render_runtime_submits_cloudflare_purge_after_local_origin_activation() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/zones/zone-123/purge_cache"))
        .and(header("authorization", "Bearer token-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "result": { "id": "purge-123" }
        })))
        .mount(&server)
        .await;
    let _zone = EnvVarGuard::set("TEST_CF_ZONE_ID", "zone-123");
    let _token = EnvVarGuard::set("TEST_CF_API_TOKEN", "token-123");
    let _api_base = EnvVarGuard::set("TEST_CF_API_BASE", &server.uri());
    let temp = tempfile::tempdir().expect("tempdir");
    let config_dir = temp.path().join("config");
    let source_dir = config_dir.join("site");
    let mirror_dir = temp.path().join("var/origins");
    tokio::fs::create_dir_all(&source_dir)
        .await
        .expect("create source dir");
    tokio::fs::write(source_dir.join("index.html"), "<h1>Hello</h1>")
        .await
        .expect("write index");

    let manifest_path = config_dir.join("rendermesh.yaml");
    tokio::fs::write(
        &manifest_path,
        format!(
            r#"
version: 1
runtime:
  local_store_dir: {}
  sync_interval_seconds: 60
origins:
  web:
    type: local
    path: ./site
    cdn:
      provider: cloudflare
      zone_id_env: TEST_CF_ZONE_ID
      api_token_env: TEST_CF_API_TOKEN
      api_base_env: TEST_CF_API_BASE
      strategy: changed_paths
hosts:
  " WEB.TEST ":
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

    let snapshot = runtime.origin_runtime.get("web").expect("origin snapshot");
    assert_eq!(snapshot.last_cdn_provider.as_deref(), Some("cloudflare"));
    assert_eq!(snapshot.last_cdn_status.as_deref(), Some("submitted"));
    assert_eq!(snapshot.last_cdn_request_id.as_deref(), Some("purge-123"));
    assert_eq!(snapshot.last_cdn_request_ids, vec!["purge-123".to_string()]);
    assert_eq!(snapshot.last_cdn_submitted_items, Some(1));

    let requests = server.received_requests().await.expect("requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&requests[0].body).expect("json body"),
        json!({ "files": ["https://web.test/index.html"] })
    );
}

#[tokio::test]
async fn build_render_runtime_reconciles_cloudflare_dns_domains() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/zones/zone-domains/purge_cache"))
        .and(header("authorization", "Bearer token-domains"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "result": { "id": "purge-domains" }
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/zones/zone-domains/dns_records"))
        .and(header("authorization", "Bearer token-domains"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "result": []
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/zones/zone-domains/dns_records"))
        .and(header("authorization", "Bearer token-domains"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "result": { "id": "record-1" }
        })))
        .mount(&server)
        .await;
    let _zone = EnvVarGuard::set("TEST_CF_DOMAIN_ZONE_ID", "zone-domains");
    let _token = EnvVarGuard::set("TEST_CF_DOMAIN_API_TOKEN", "token-domains");
    let _api_base = EnvVarGuard::set("TEST_CF_DOMAIN_API_BASE", &server.uri());
    let _origin = EnvVarGuard::set("TEST_CF_DOMAIN_ORIGIN", "rendermesh.example.com");
    let temp = tempfile::tempdir().expect("tempdir");
    let config_dir = temp.path().join("config");
    let source_dir = config_dir.join("site");
    let mirror_dir = temp.path().join("var/origins");
    tokio::fs::create_dir_all(&source_dir)
        .await
        .expect("create source dir");
    tokio::fs::write(source_dir.join("index.html"), "<h1>Hello</h1>")
        .await
        .expect("write index");

    let manifest_path = config_dir.join("rendermesh.yaml");
    tokio::fs::write(
        &manifest_path,
        format!(
            r#"
version: 1
runtime:
  local_store_dir: {}
  sync_interval_seconds: 60
origins:
  loja:
    type: local
    path: ./site
    cdn:
      provider: cloudflare
      zone_id_env: TEST_CF_DOMAIN_ZONE_ID
      api_token_env: TEST_CF_DOMAIN_API_TOKEN
      api_base_env: TEST_CF_DOMAIN_API_BASE
      strategy: changed_paths
      domains:
        enabled: true
        mode: dns_records
        origin_domain_env: TEST_CF_DOMAIN_ORIGIN
        proxied: true
hosts:
  megaloja.com.br:
    origin: loja
  "*.megaloja.com.br":
    origin: loja
"#,
            mirror_dir.display()
        ),
    )
    .await
    .expect("write manifest");

    let runtime = build_render_runtime(manifest_path.to_str().expect("manifest path"))
        .await
        .expect("runtime builds");

    let snapshot = runtime.origin_runtime.get("loja").expect("origin snapshot");
    assert_eq!(
        snapshot.last_cdn_domain_provider.as_deref(),
        Some("cloudflare")
    );
    assert_eq!(
        snapshot.last_cdn_domain_status.as_deref(),
        Some("submitted")
    );
    assert_eq!(snapshot.last_cdn_domain_added, Some(1));
    assert_eq!(snapshot.last_cdn_domain_unchanged, Some(0));

    let requests = server.received_requests().await.expect("requests");
    let create_request = requests
        .iter()
        .find(|request| {
            request.method.as_str() == "POST"
                && request.url.path() == "/zones/zone-domains/dns_records"
        })
        .expect("dns create request");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&create_request.body).expect("json body"),
        json!({
            "type": "CNAME",
            "name": "megaloja.com.br",
            "content": "rendermesh.example.com",
            "proxied": true,
            "ttl": 1
        })
    );
}

struct EnvVarGuard {
    key: &'static str,
    original: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let original = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(value) = self.original.as_ref() {
            std::env::set_var(self.key, value);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

async fn write_mirror_file(temp_root: &std::path::Path, key: &str, body: &str) {
    let path = temp_root.join("origins/web").join(key);
    tokio::fs::create_dir_all(path.parent().expect("parent"))
        .await
        .expect("mkdir");
    tokio::fs::write(path, body).await.expect("write file");
}

fn yaml_edge_object(key: &str, body: &str) -> RemoteObject {
    RemoteObject {
        key: key.to_string(),
        body: Bytes::from(body.to_string()),
        etag: Some("edge".to_string()),
        last_modified: None,
        content_type: Some("application/yaml".to_string()),
        cache_control: None,
    }
}

fn edge_object(body: &str) -> RemoteObject {
    yaml_edge_object("_rendermesh/edge.yaml", body)
}

fn json_edge_object(key: &str, body: &str) -> RemoteObject {
    RemoteObject {
        key: key.to_string(),
        body: Bytes::from(body.to_string()),
        etag: Some("edge-json".to_string()),
        last_modified: None,
        content_type: Some("application/json".to_string()),
        cache_control: None,
    }
}

struct StaticStorage {
    objects: BTreeMap<String, RemoteObject>,
}

impl StaticStorage {
    fn new(objects: BTreeMap<String, RemoteObject>) -> Self {
        Self { objects }
    }
}

#[async_trait]
impl RemoteStorage for StaticStorage {
    async fn list_objects(&self) -> Result<Vec<RemoteObjectSummary>> {
        Ok(self
            .objects
            .values()
            .map(|object| RemoteObjectSummary {
                key: object.key.clone(),
                created_at: None,
                etag: object.etag.clone(),
                last_modified: object.last_modified.clone(),
                size: object.body.len() as u64,
                content_type: object.content_type.clone(),
                cache_control: object.cache_control.clone(),
            })
            .collect())
    }

    async fn get_object(&self, key: &str) -> Result<RemoteObject> {
        self.objects
            .get(key)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing {key}"))
    }
}
