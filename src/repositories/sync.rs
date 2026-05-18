use std::{
    collections::BTreeSet,
    ffi::OsString,
    path::{Component, Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use bytes::Bytes;

use crate::repositories::local_mirror::{LocalMirrorRepository, ObjectMetadata};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteObjectSummary {
    pub key: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub size: u64,
    pub content_type: Option<String>,
    pub cache_control: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteObject {
    pub key: String,
    pub body: Bytes,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub content_type: Option<String>,
    pub cache_control: Option<String>,
}

#[async_trait]
pub trait RemoteStorage: Send + Sync {
    async fn list_objects(&self) -> Result<Vec<RemoteObjectSummary>>;
    async fn get_object(&self, key: &str) -> Result<RemoteObject>;
}

#[derive(Clone)]
pub struct MirrorSyncService {
    root: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncReport {
    pub downloaded: usize,
}

impl MirrorSyncService {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub async fn sync_origin<S>(&self, origin_id: &str, storage: &S) -> Result<SyncReport>
    where
        S: RemoteStorage,
    {
        let origin_dir = LocalMirrorRepository::new(self.root.clone()).origin_dir(origin_id)?;
        tokio::fs::create_dir_all(&origin_dir)
            .await
            .with_context(|| format!("create origin mirror {}", origin_dir.display()))?;

        let summaries = storage.list_objects().await?;
        let mut remote_keys = BTreeSet::new();
        let mut downloaded = 0usize;

        for summary in summaries {
            let normalized_key = normalize_remote_key(&summary.key)?;
            remote_keys.insert(normalized_key.clone());

            if local_object_matches_summary(&origin_dir, &normalized_key, &summary).await? {
                continue;
            }

            let object = storage.get_object(&summary.key).await?;
            write_object(&origin_dir, object).await?;
            downloaded += 1;
        }

        remove_deleted_objects(&origin_dir, &remote_keys).await?;
        Ok(SyncReport { downloaded })
    }
}

async fn local_object_matches_summary(
    origin_dir: &Path,
    key: &str,
    summary: &RemoteObjectSummary,
) -> Result<bool> {
    let object_path = object_path(origin_dir, key)?;
    let file_metadata = match tokio::fs::metadata(&object_path).await {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("read local object metadata {}", object_path.display()))
        }
    };

    if file_metadata.len() != summary.size {
        return Ok(false);
    }

    let metadata = read_sidecar_metadata(&object_path).await?;
    Ok(optional_field_matches(&metadata.etag, &summary.etag)
        && optional_field_matches(&metadata.last_modified, &summary.last_modified)
        && optional_field_matches(&metadata.content_type, &summary.content_type)
        && optional_field_matches(&metadata.cache_control, &summary.cache_control))
}

fn optional_field_matches(local: &Option<String>, remote: &Option<String>) -> bool {
    match remote {
        Some(remote_value) => local.as_deref() == Some(remote_value.as_str()),
        None => true,
    }
}

async fn write_object(origin_dir: &Path, object: RemoteObject) -> Result<()> {
    let key = normalize_remote_key(&object.key)?;
    let object_path = object_path(origin_dir, &key)?;

    if let Some(parent) = object_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create object parent {}", parent.display()))?;
    }

    tokio::fs::write(&object_path, object.body)
        .await
        .with_context(|| format!("write local object {}", object_path.display()))?;

    let metadata = ObjectMetadata {
        content_type: object.content_type,
        etag: object.etag,
        last_modified: object.last_modified,
        cache_control: object.cache_control,
    };
    let sidecar_path = metadata_sidecar_path(&object_path);
    tokio::fs::write(&sidecar_path, serde_json::to_vec(&metadata)?)
        .await
        .with_context(|| format!("write local object metadata {}", sidecar_path.display()))?;

    Ok(())
}

async fn remove_deleted_objects(origin_dir: &Path, remote_keys: &BTreeSet<String>) -> Result<()> {
    let mut stack = vec![origin_dir.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("read mirror dir {}", dir.display()))
            }
        };

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let file_type = entry.file_type().await?;

            if file_type.is_dir() {
                stack.push(path);
                continue;
            }

            if !file_type.is_file() || is_metadata_sidecar(&path) {
                continue;
            }

            let key = relative_key(origin_dir, &path)?;
            if remote_keys.contains(&key) {
                continue;
            }

            tokio::fs::remove_file(&path)
                .await
                .with_context(|| format!("remove deleted local object {}", path.display()))?;

            let sidecar_path = metadata_sidecar_path(&path);
            match tokio::fs::remove_file(&sidecar_path).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "remove deleted local object metadata {}",
                            sidecar_path.display()
                        )
                    })
                }
            }
        }
    }

    Ok(())
}

async fn read_sidecar_metadata(object_path: &Path) -> Result<ObjectMetadata> {
    let sidecar_path = metadata_sidecar_path(object_path);

    match tokio::fs::read_to_string(&sidecar_path).await {
        Ok(content) => serde_json::from_str(&content)
            .with_context(|| format!("parse local object metadata {}", sidecar_path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ObjectMetadata::default()),
        Err(error) => Err(error)
            .with_context(|| format!("read local object metadata {}", sidecar_path.display())),
    }
}

fn object_path(origin_dir: &Path, key: &str) -> Result<PathBuf> {
    let key = normalize_remote_key(key)?;
    let path = origin_dir.join(key);

    if path.starts_with(origin_dir) {
        Ok(path)
    } else {
        Err(anyhow!("object path escapes origin directory"))
    }
}

fn normalize_remote_key(key: &str) -> Result<String> {
    if key.is_empty() || key.starts_with('/') || key.chars().any(char::is_control) {
        return Err(anyhow!("invalid object path {key}"));
    }

    let path = Path::new(key);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::Prefix(_) | Component::RootDir
            )
        })
    {
        return Err(anyhow!("invalid object path {key}"));
    }

    Ok(key.to_string())
}

fn metadata_sidecar_path(object_path: &Path) -> PathBuf {
    let mut sidecar = OsString::from(object_path.as_os_str());
    sidecar.push(".meta.json");
    PathBuf::from(sidecar)
}

fn is_metadata_sidecar(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".meta.json"))
}

fn relative_key(origin_dir: &Path, object_path: &Path) -> Result<String> {
    let relative = object_path.strip_prefix(origin_dir).with_context(|| {
        format!(
            "local object {} is outside origin dir {}",
            object_path.display(),
            origin_dir.display()
        )
    })?;

    let key = relative
        .components()
        .map(|component| match component {
            Component::Normal(part) => part.to_string_lossy().into_owned(),
            _ => String::new(),
        })
        .collect::<Vec<_>>()
        .join("/");

    normalize_remote_key(&key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use bytes::Bytes;
    use std::{collections::BTreeMap, sync::Arc};
    use tokio::sync::Mutex;

    #[derive(Clone, Default)]
    struct FakeStorage {
        objects: Arc<Mutex<BTreeMap<String, RemoteObject>>>,
        requested_keys: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl RemoteStorage for FakeStorage {
        async fn list_objects(&self) -> anyhow::Result<Vec<RemoteObjectSummary>> {
            let objects = self.objects.lock().await;
            Ok(objects
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

        async fn get_object(&self, key: &str) -> anyhow::Result<RemoteObject> {
            self.requested_keys.lock().await.push(key.to_string());
            self.objects
                .lock()
                .await
                .get(key)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("missing"))
        }
    }

    #[tokio::test]
    async fn initial_sync_downloads_objects_and_metadata() {
        let temp = tempfile::tempdir().expect("tempdir");
        let storage = FakeStorage::default();
        storage.objects.lock().await.insert(
            "index.html".to_string(),
            RemoteObject {
                key: "index.html".to_string(),
                body: Bytes::from_static(b"<h1>Hello</h1>"),
                etag: Some("abc".to_string()),
                last_modified: Some("Mon, 01 Jan 2024 00:00:00 GMT".to_string()),
                content_type: Some("text/html".to_string()),
                cache_control: Some("max-age=60".to_string()),
            },
        );
        let syncer = MirrorSyncService::new(temp.path().join("origins"));

        let report = syncer
            .sync_origin("web", &storage)
            .await
            .expect("sync succeeds");

        assert_eq!(report, SyncReport { downloaded: 1 });
        assert_eq!(
            tokio::fs::read_to_string(temp.path().join("origins/web/index.html"))
                .await
                .expect("read"),
            "<h1>Hello</h1>"
        );
        assert!(temp
            .path()
            .join("origins/web/index.html.meta.json")
            .exists());
    }

    #[tokio::test]
    async fn removes_objects_missing_from_remote_listing() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("origins");
        let origin_dir = root.join("web");
        tokio::fs::create_dir_all(origin_dir.join("assets"))
            .await
            .expect("mkdir");
        tokio::fs::write(origin_dir.join("stale.html"), "old")
            .await
            .expect("write stale");
        tokio::fs::write(origin_dir.join("stale.html.meta.json"), "{}")
            .await
            .expect("write stale metadata");
        tokio::fs::write(origin_dir.join("assets/keep.css"), "body{}")
            .await
            .expect("write kept object");

        let storage = FakeStorage::default();
        storage.objects.lock().await.insert(
            "assets/keep.css".to_string(),
            RemoteObject {
                key: "assets/keep.css".to_string(),
                body: Bytes::from_static(b"body{}"),
                etag: Some("keep".to_string()),
                last_modified: None,
                content_type: Some("text/css".to_string()),
                cache_control: None,
            },
        );

        MirrorSyncService::new(root)
            .sync_origin("web", &storage)
            .await
            .expect("sync succeeds");

        assert!(!origin_dir.join("stale.html").exists());
        assert!(!origin_dir.join("stale.html.meta.json").exists());
        assert!(origin_dir.join("assets/keep.css").exists());
    }

    #[tokio::test]
    async fn skips_download_when_local_object_metadata_matches_summary() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("origins");
        let origin_dir = root.join("web");
        tokio::fs::create_dir_all(&origin_dir).await.expect("mkdir");
        tokio::fs::write(origin_dir.join("index.html"), "<h1>Hello</h1>")
            .await
            .expect("write object");
        tokio::fs::write(
            origin_dir.join("index.html.meta.json"),
            r#"{"content_type":"text/html","etag":"abc","last_modified":"Mon, 01 Jan 2024 00:00:00 GMT","cache_control":"max-age=60","size":14}"#,
        )
        .await
        .expect("write metadata");

        let storage = FakeStorage::default();
        storage.objects.lock().await.insert(
            "index.html".to_string(),
            RemoteObject {
                key: "index.html".to_string(),
                body: Bytes::from_static(b"<h1>Hello</h1>"),
                etag: Some("abc".to_string()),
                last_modified: Some("Mon, 01 Jan 2024 00:00:00 GMT".to_string()),
                content_type: Some("text/html".to_string()),
                cache_control: Some("max-age=60".to_string()),
            },
        );

        let report = MirrorSyncService::new(root)
            .sync_origin("web", &storage)
            .await
            .expect("sync succeeds");

        assert_eq!(report, SyncReport { downloaded: 0 });
        assert!(storage.requested_keys.lock().await.is_empty());
    }

    #[tokio::test]
    async fn rejects_remote_keys_that_escape_origin_dir() {
        let temp = tempfile::tempdir().expect("tempdir");
        let storage = FakeStorage::default();
        storage.objects.lock().await.insert(
            "../secret.txt".to_string(),
            RemoteObject {
                key: "../secret.txt".to_string(),
                body: Bytes::from_static(b"secret"),
                etag: None,
                last_modified: None,
                content_type: None,
                cache_control: None,
            },
        );

        let error = MirrorSyncService::new(temp.path().join("origins"))
            .sync_origin("web", &storage)
            .await
            .expect_err("invalid key is rejected");

        assert!(error.to_string().contains("invalid object path"));
        assert!(!temp.path().join("origins/secret.txt").exists());
    }
}
