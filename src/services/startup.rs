use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, RwLock},
    time::Duration,
};

use anyhow::Result;

use crate::{
    dto::manifest::RenderMeshManifest,
    repositories::{
        local_mirror::LocalMirrorRepository, manifest::ManifestRepository,
        origin_storage::OriginStorageRepository, sync::MirrorSyncService, sync::RemoteStorage,
    },
    services::{
        cors::CorsPolicy,
        edge_config::{default_edge_config, parse_edge_config},
        edge_config_store::EdgeConfigStore,
        freshness::OriginFreshnessIndex,
        manifest::{load_manifest, HostResolver},
        origin_runtime::{OriginRuntimeStore, OriginSnapshotDebug},
        render_gateway::RenderGatewayService,
        template_store::TemplateStore,
    },
};

const EDGE_CONFIG_PATHS: [&str; 3] = [
    "/_rendermesh/edge.yaml",
    "/_rendermesh/edge.yml",
    "/_rendermesh/edge.json",
];

type OriginFreshnessIndexes = Arc<RwLock<BTreeMap<String, OriginFreshnessIndex>>>;

pub struct RenderRuntime {
    pub render_gateway: RenderGatewayService,
    pub origin_runtime: OriginRuntimeStore,
}

pub async fn build_render_gateway(manifest_path: &str) -> Result<RenderGatewayService> {
    Ok(build_render_runtime(manifest_path).await?.render_gateway)
}

pub async fn build_render_runtime(manifest_path: &str) -> Result<RenderRuntime> {
    let manifest = load_manifest(&ManifestRepository::new(), manifest_path).await?;
    let manifest_dir = Path::new(manifest_path)
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mirror = LocalMirrorRepository::new(&manifest.runtime.local_store_dir);
    let syncer = MirrorSyncService::new(&manifest.runtime.local_store_dir);
    let edge_configs = EdgeConfigStore::from_configs(BTreeMap::new());
    let template_store = TemplateStore::default();
    let freshness_indexes = OriginFreshnessIndexes::default();
    let origin_runtime = OriginRuntimeStore::default();

    let mut storage_by_origin = BTreeMap::new();
    for (origin_id, origin) in &manifest.origins {
        let storage = OriginStorageRepository::from_origin_config(origin, manifest_dir).await?;
        let report = refresh_origin_snapshot(
            origin_id,
            &syncer,
            &storage,
            &edge_configs,
            &template_store,
            &freshness_indexes,
            &origin_runtime,
        )
        .await?;
        tracing::info!(
            origin = %origin_id,
            downloaded = report.downloaded,
            "initial origin sync completed"
        );
        storage_by_origin.insert(origin_id.clone(), storage);
    }

    spawn_background_sync(
        manifest.clone(),
        syncer,
        edge_configs.clone(),
        template_store.clone(),
        freshness_indexes,
        origin_runtime.clone(),
        storage_by_origin,
    );

    let render_gateway = RenderGatewayService::new_with_stores_and_origin_buckets(
        HostResolver::new(&manifest)?,
        CorsPolicy::from_manifest(&manifest),
        mirror,
        edge_configs,
        template_store,
        origin_buckets(&manifest),
    );

    Ok(RenderRuntime {
        render_gateway,
        origin_runtime,
    })
}

fn origin_buckets(manifest: &RenderMeshManifest) -> BTreeMap<String, String> {
    manifest
        .origins
        .iter()
        .map(|(origin_id, origin)| (origin_id.clone(), origin.edge_context_bucket(origin_id)))
        .collect()
}

#[cfg(test)]
pub(crate) async fn load_edge_configs<I>(
    origin_ids: I,
    mirror: &LocalMirrorRepository,
) -> EdgeConfigStore
where
    I: IntoIterator<Item = String>,
{
    let store = EdgeConfigStore::from_configs(BTreeMap::new());
    for origin_id in origin_ids {
        refresh_edge_config(&origin_id, mirror, &store).await;
    }
    store
}

#[cfg(test)]
pub(crate) async fn sync_origin_and_refresh_edge_config<S>(
    origin_id: &str,
    syncer: &MirrorSyncService,
    storage: &S,
    _mirror: &LocalMirrorRepository,
    edge_configs: &EdgeConfigStore,
    template_store: &TemplateStore,
) -> Result<()>
where
    S: RemoteStorage,
{
    let freshness_indexes = OriginFreshnessIndexes::default();
    let origin_runtime = OriginRuntimeStore::default();
    let report = refresh_origin_snapshot(
        origin_id,
        syncer,
        storage,
        edge_configs,
        template_store,
        &freshness_indexes,
        &origin_runtime,
    )
    .await?;
    tracing::info!(
        origin = %origin_id,
        downloaded = report.downloaded,
        "origin sync completed"
    );
    Ok(())
}

pub(crate) async fn refresh_origin_snapshot<S>(
    origin_id: &str,
    syncer: &MirrorSyncService,
    storage: &S,
    edge_configs: &EdgeConfigStore,
    template_store: &TemplateStore,
    freshness_indexes: &OriginFreshnessIndexes,
    origin_runtime: &OriginRuntimeStore,
) -> Result<crate::repositories::sync::SyncReport>
where
    S: RemoteStorage,
{
    let previous_index = freshness_indexes
        .read()
        .expect("freshness index lock")
        .get(origin_id)
        .cloned();
    let staged = syncer
        .stage_origin_sync(origin_id, storage, previous_index.as_ref())
        .await?;
    let (stage_mirror, stage_origin_id) = staged_origin_mirror(&staged.staging_dir)?;
    let edge_config = load_origin_edge_config(&stage_origin_id, &stage_mirror).await?;
    let template_registry = template_store
        .compile_template_update_from_mirror(
            origin_id,
            &stage_origin_id,
            &stage_mirror,
            &staged.diff,
        )
        .await?;
    let next_index = staged.index.clone();
    let report = staged.report.clone();
    let next_generation = origin_runtime
        .get(origin_id)
        .map(|snapshot| snapshot.generation + 1)
        .unwrap_or(1);
    let activated_at = chrono::Utc::now().to_rfc3339();
    let snapshot = OriginSnapshotDebug {
        origin_id: origin_id.to_string(),
        generation: next_generation,
        activated_at,
        captured_at: next_index.captured_at.to_rfc3339(),
        known_files: next_index.files.len(),
        added_files: staged.diff.added.len(),
        modified_files: staged.diff.modified.len(),
        removed_files: staged.diff.removed.len(),
        unchanged_files: staged.diff.unchanged.len(),
        downloaded_files: report.downloaded,
        last_error: None,
    };
    tracing::info!(
        origin = %origin_id,
        generation = next_generation,
        listed_files = staged.index.files.len(),
        added_files = staged.diff.added.len(),
        modified_files = staged.diff.modified.len(),
        removed_files = staged.diff.removed.len(),
        unchanged_files = staged.diff.unchanged.len(),
        downloaded = report.downloaded,
        "origin freshness refresh staged"
    );

    syncer.activate_staged_origin(staged).await?;
    edge_configs.set_valid(origin_id, edge_config);
    template_store.set_origin_registry(origin_id, template_registry);
    freshness_indexes
        .write()
        .expect("freshness index lock")
        .insert(origin_id.to_string(), next_index);
    origin_runtime.set_snapshot(snapshot);
    tracing::info!(origin = %origin_id, generation = next_generation, "origin freshness refresh activated");

    Ok(report)
}

fn staged_origin_mirror(staging_dir: &Path) -> Result<(LocalMirrorRepository, String)> {
    let root = staging_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("staging dir has no parent"))?;
    let origin_id = staging_dir
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| anyhow::anyhow!("staging dir has invalid origin id"))?;
    Ok((LocalMirrorRepository::new(root), origin_id.to_string()))
}

#[cfg(test)]
async fn refresh_edge_config(
    origin_id: &str,
    mirror: &LocalMirrorRepository,
    edge_configs: &EdgeConfigStore,
) {
    match load_origin_edge_config(origin_id, mirror).await {
        Ok(config) => edge_configs.set_valid(origin_id, config),
        Err(error) => {
            tracing::error!(origin = %origin_id, "failed to load edge config: {error}");
            edge_configs.set_invalid(origin_id, error.to_string());
        }
    }
}

async fn load_origin_edge_config(
    origin_id: &str,
    mirror: &LocalMirrorRepository,
) -> Result<crate::dto::edge::EdgeConfig> {
    for path in EDGE_CONFIG_PATHS {
        if let Some(object) = mirror.read_object(origin_id, path).await? {
            let content = String::from_utf8(object.body.to_vec())?;
            return Ok(parse_edge_config(&content)?);
        }
    }

    tracing::warn!(
        origin = %origin_id,
        "origin has no edge config file; using default edge config"
    );
    Ok(default_edge_config())
}

fn spawn_background_sync(
    manifest: Arc<RenderMeshManifest>,
    syncer: MirrorSyncService,
    edge_configs: EdgeConfigStore,
    template_store: TemplateStore,
    freshness_indexes: OriginFreshnessIndexes,
    origin_runtime: OriginRuntimeStore,
    storage_by_origin: BTreeMap<String, OriginStorageRepository>,
) {
    for (origin_id, storage) in storage_by_origin {
        let syncer = syncer.clone();
        let edge_configs = edge_configs.clone();
        let template_store = template_store.clone();
        let freshness_indexes = freshness_indexes.clone();
        let origin_runtime = origin_runtime.clone();
        let interval_seconds = manifest
            .origins
            .get(&origin_id)
            .and_then(|origin| origin.sync_interval_seconds())
            .unwrap_or(manifest.runtime.sync_interval_seconds);

        tokio::spawn(async move {
            let interval = Duration::from_secs(interval_seconds);
            loop {
                tokio::time::sleep(interval).await;
                if let Err(error) = refresh_origin_snapshot(
                    &origin_id,
                    &syncer,
                    &storage,
                    &edge_configs,
                    &template_store,
                    &freshness_indexes,
                    &origin_runtime,
                )
                .await
                {
                    origin_runtime.set_error(&origin_id, error.to_string());
                    tracing::error!(origin = %origin_id, "background origin sync failed: {error}");
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use anyhow::Result;
    use async_trait::async_trait;
    use bytes::Bytes;

    use super::*;
    use crate::{
        repositories::{
            local_mirror::LocalMirrorRepository,
            sync::{MirrorSyncService, RemoteObject, RemoteObjectSummary, RemoteStorage},
        },
        services::edge_config_store::{EdgeConfigStore, EdgeConfigStoreError},
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
            &edge_configs,
            &template_store,
            &freshness_indexes,
            &origin_runtime,
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
        tokio::fs::create_dir_all(source_dir.join("_rendermesh"))
            .await
            .expect("create source dir");
        tokio::fs::write(source_dir.join("index.html"), "<h1>{{title}}</h1>")
            .await
            .expect("write index");
        tokio::fs::write(
            source_dir.join("_rendermesh/edge.yaml"),
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

    async fn write_mirror_file(temp_root: &std::path::Path, key: &str, body: &str) {
        let path = temp_root.join("origins/web").join(key);
        tokio::fs::create_dir_all(path.parent().expect("parent"))
            .await
            .expect("mkdir");
        tokio::fs::write(path, body).await.expect("write file");
    }

    fn edge_object(body: &str) -> RemoteObject {
        RemoteObject {
            key: "_rendermesh/edge.yaml".to_string(),
            body: Bytes::from(body.to_string()),
            etag: Some("edge".to_string()),
            last_modified: None,
            content_type: Some("application/yaml".to_string()),
            cache_control: None,
        }
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
}
