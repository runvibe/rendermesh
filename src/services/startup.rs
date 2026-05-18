use std::{collections::BTreeMap, sync::Arc, time::Duration};

use anyhow::Result;

use crate::{
    dto::manifest::RenderMeshManifest,
    repositories::{
        local_mirror::LocalMirrorRepository, manifest::ManifestRepository,
        s3_storage::S3StorageRepository, sync::MirrorSyncService, sync::RemoteStorage,
    },
    services::{
        cors::CorsPolicy,
        edge_config::{default_edge_config, parse_edge_config},
        edge_config_store::EdgeConfigStore,
        manifest::{load_manifest, HostResolver},
        render_gateway::RenderGatewayService,
        template_store::TemplateStore,
    },
};

const EDGE_CONFIG_PATHS: [&str; 3] = [
    "/_rendermesh/edge.yaml",
    "/_rendermesh/edge.yml",
    "/_rendermesh/edge.json",
];

pub async fn build_render_gateway(manifest_path: &str) -> Result<RenderGatewayService> {
    let manifest = load_manifest(&ManifestRepository::new(), manifest_path).await?;
    let mirror = LocalMirrorRepository::new(&manifest.runtime.local_store_dir);
    let syncer = MirrorSyncService::new(&manifest.runtime.local_store_dir);

    let mut storage_by_origin = BTreeMap::new();
    for (origin_id, origin) in &manifest.origins {
        let storage = S3StorageRepository::from_origin_config(origin).await?;
        let report = syncer.sync_origin(origin_id, &storage).await?;
        tracing::info!(
            origin = %origin_id,
            downloaded = report.downloaded,
            "initial origin sync completed"
        );
        storage_by_origin.insert(origin_id.clone(), storage);
    }

    let edge_configs = load_edge_configs(manifest.origins.keys().cloned(), &mirror).await;
    let template_store = load_templates(manifest.origins.keys().cloned(), &mirror).await?;
    spawn_background_sync(
        manifest.clone(),
        syncer,
        mirror.clone(),
        edge_configs.clone(),
        template_store.clone(),
        storage_by_origin,
    );

    Ok(RenderGatewayService::new_with_stores_and_origin_buckets(
        HostResolver::new(&manifest)?,
        CorsPolicy::from_manifest(&manifest),
        mirror,
        edge_configs,
        template_store,
        origin_buckets(&manifest),
    ))
}

fn origin_buckets(manifest: &RenderMeshManifest) -> BTreeMap<String, String> {
    manifest
        .origins
        .iter()
        .map(|(origin_id, origin)| (origin_id.clone(), origin.bucket.clone()))
        .collect()
}

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

pub(crate) async fn sync_origin_and_refresh_edge_config<S>(
    origin_id: &str,
    syncer: &MirrorSyncService,
    storage: &S,
    mirror: &LocalMirrorRepository,
    edge_configs: &EdgeConfigStore,
    template_store: &TemplateStore,
) -> Result<()>
where
    S: RemoteStorage,
{
    let report = syncer.sync_origin(origin_id, storage).await?;
    tracing::info!(
        origin = %origin_id,
        downloaded = report.downloaded,
        "origin sync completed"
    );
    refresh_edge_config(origin_id, mirror, edge_configs).await;
    template_store
        .load_origin_templates(origin_id, mirror)
        .await?;
    Ok(())
}

pub(crate) async fn load_templates<I>(
    origin_ids: I,
    mirror: &LocalMirrorRepository,
) -> Result<TemplateStore>
where
    I: IntoIterator<Item = String>,
{
    let store = TemplateStore::default();
    for origin_id in origin_ids {
        store.load_origin_templates(&origin_id, mirror).await?;
    }
    Ok(store)
}

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
    mirror: LocalMirrorRepository,
    edge_configs: EdgeConfigStore,
    template_store: TemplateStore,
    storage_by_origin: BTreeMap<String, S3StorageRepository>,
) {
    for (origin_id, storage) in storage_by_origin {
        let syncer = syncer.clone();
        let mirror = mirror.clone();
        let edge_configs = edge_configs.clone();
        let template_store = template_store.clone();
        let interval_seconds = manifest
            .origins
            .get(&origin_id)
            .and_then(|origin| origin.sync_interval_seconds)
            .unwrap_or(manifest.runtime.sync_interval_seconds);

        tokio::spawn(async move {
            let interval = Duration::from_secs(interval_seconds);
            loop {
                tokio::time::sleep(interval).await;
                if let Err(error) = sync_origin_and_refresh_edge_config(
                    &origin_id,
                    &syncer,
                    &storage,
                    &mirror,
                    &edge_configs,
                    &template_store,
                )
                .await
                {
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
