use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use bytes::Bytes;
use chrono::Utc;

use crate::{
    repositories::local_mirror::{
        metadata_sidecar_path, LocalMirrorRepository, ObjectMetadata, METADATA_DIR_NAME,
    },
    services::freshness::{
        build_origin_index, diff_origin_indexes, OriginFreshnessDiff, OriginFreshnessIndex,
    },
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteObjectSummary {
    pub key: String,
    pub created_at: Option<String>,
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

#[derive(Clone, Debug)]
pub struct StagedOriginSync {
    pub origin_id: String,
    pub staging_dir: PathBuf,
    pub index: OriginFreshnessIndex,
    pub diff: OriginFreshnessDiff,
    pub report: SyncReport,
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
        let staging_dir = self.staging_dir(origin_id)?;
        prepare_staging_dir(&origin_dir, &staging_dir).await?;

        let result = sync_origin_dir(&staging_dir, storage).await;
        let report = match result {
            Ok(report) => report,
            Err(error) => {
                remove_dir_if_exists(&staging_dir).await?;
                return Err(error);
            }
        };

        if let Err(error) = swap_origin_dir(&origin_dir, &staging_dir).await {
            remove_dir_if_exists(&staging_dir).await?;
            return Err(error);
        }

        Ok(report)
    }

    pub async fn stage_origin_sync<S>(
        &self,
        origin_id: &str,
        storage: &S,
        previous_index: Option<&OriginFreshnessIndex>,
    ) -> Result<StagedOriginSync>
    where
        S: RemoteStorage,
    {
        let origin_dir = LocalMirrorRepository::new(self.root.clone()).origin_dir(origin_id)?;
        let staging_dir = self.staging_dir(origin_id)?;
        prepare_staging_dir(&origin_dir, &staging_dir).await?;

        let result = stage_origin_dir(origin_id, &staging_dir, storage, previous_index).await;
        match result {
            Ok((index, diff, report)) => Ok(StagedOriginSync {
                origin_id: origin_id.to_string(),
                staging_dir,
                index,
                diff,
                report,
            }),
            Err(error) => {
                remove_dir_if_exists(&staging_dir).await?;
                Err(error)
            }
        }
    }

    pub async fn activate_staged_origin(&self, staged: StagedOriginSync) -> Result<()> {
        let origin_dir =
            LocalMirrorRepository::new(self.root.clone()).origin_dir(&staged.origin_id)?;
        if let Err(error) = swap_origin_dir(&origin_dir, &staged.staging_dir).await {
            remove_dir_if_exists(&staged.staging_dir).await?;
            return Err(error);
        }
        Ok(())
    }

    fn staging_dir(&self, origin_id: &str) -> Result<PathBuf> {
        LocalMirrorRepository::new(self.root.clone()).origin_dir(origin_id)?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        Ok(self
            .root
            .join(".rendermesh-sync")
            .join(format!("{origin_id}-{}-{timestamp}", std::process::id())))
    }
}

async fn stage_origin_dir<S>(
    origin_id: &str,
    origin_dir: &Path,
    storage: &S,
    previous_index: Option<&OriginFreshnessIndex>,
) -> Result<(OriginFreshnessIndex, OriginFreshnessDiff, SyncReport)>
where
    S: RemoteStorage,
{
    tokio::fs::create_dir_all(origin_dir)
        .await
        .with_context(|| format!("create origin mirror {}", origin_dir.display()))?;

    let summaries = storage.list_objects().await?;
    let index = build_origin_index(origin_id, summaries, Utc::now())?;
    let diff = diff_origin_indexes(previous_index, &index);
    let remote_keys = index.files.keys().cloned().collect::<BTreeSet<_>>();
    let mut downloaded = 0usize;

    for key in diff.changed_paths() {
        let object = storage.get_object(key).await?;
        write_object(origin_dir, object).await?;
        downloaded += 1;
    }

    remove_deleted_objects(origin_dir, &remote_keys).await?;
    remove_orphan_metadata_sidecars(origin_dir, &remote_keys).await?;

    Ok((index, diff, SyncReport { downloaded }))
}

async fn sync_origin_dir<S>(origin_dir: &Path, storage: &S) -> Result<SyncReport>
where
    S: RemoteStorage,
{
    tokio::fs::create_dir_all(origin_dir)
        .await
        .with_context(|| format!("create origin mirror {}", origin_dir.display()))?;

    let summaries = storage.list_objects().await?;
    let mut remote_keys = BTreeSet::new();
    let mut downloaded = 0usize;

    for summary in summaries {
        let normalized_key = normalize_remote_key(&summary.key)?;
        remote_keys.insert(normalized_key.clone());

        if local_object_matches_summary(origin_dir, &normalized_key, &summary).await? {
            continue;
        }

        let object = storage.get_object(&summary.key).await?;
        write_object(origin_dir, object).await?;
        downloaded += 1;
    }

    remove_deleted_objects(origin_dir, &remote_keys).await?;
    remove_orphan_metadata_sidecars(origin_dir, &remote_keys).await?;
    Ok(SyncReport { downloaded })
}

async fn prepare_staging_dir(origin_dir: &Path, staging_dir: &Path) -> Result<()> {
    remove_dir_if_exists(staging_dir).await?;
    if tokio::fs::metadata(origin_dir).await.is_ok() {
        copy_dir_contents(origin_dir, staging_dir).await
    } else {
        tokio::fs::create_dir_all(staging_dir)
            .await
            .with_context(|| format!("create staging mirror {}", staging_dir.display()))
    }
}

async fn copy_dir_contents(from: &Path, to: &Path) -> Result<()> {
    tokio::fs::create_dir_all(to)
        .await
        .with_context(|| format!("create staging mirror {}", to.display()))?;

    let mut stack = vec![(from.to_path_buf(), to.to_path_buf())];
    while let Some((source_dir, target_dir)) = stack.pop() {
        let mut entries = tokio::fs::read_dir(&source_dir)
            .await
            .with_context(|| format!("read mirror dir {}", source_dir.display()))?;

        while let Some(entry) = entries.next_entry().await? {
            let file_type = entry.file_type().await?;
            let target_path = target_dir.join(entry.file_name());

            if file_type.is_dir() {
                tokio::fs::create_dir_all(&target_path)
                    .await
                    .with_context(|| format!("create staging dir {}", target_path.display()))?;
                stack.push((entry.path(), target_path));
            } else if file_type.is_file() {
                tokio::fs::copy(entry.path(), &target_path)
                    .await
                    .with_context(|| format!("copy staging file {}", target_path.display()))?;
            }
        }
    }

    Ok(())
}

async fn swap_origin_dir(origin_dir: &Path, staging_dir: &Path) -> Result<()> {
    if let Some(parent) = origin_dir.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create mirror root {}", parent.display()))?;
    }

    let backup_dir = origin_dir.with_extension(format!(
        "rendermesh-backup-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));

    let had_origin = tokio::fs::metadata(origin_dir).await.is_ok();
    if had_origin {
        tokio::fs::rename(origin_dir, &backup_dir)
            .await
            .with_context(|| format!("move old mirror to {}", backup_dir.display()))?;
    }

    if let Err(error) = tokio::fs::rename(staging_dir, origin_dir).await {
        if had_origin {
            tokio::fs::rename(&backup_dir, origin_dir)
                .await
                .with_context(|| format!("restore old mirror {}", origin_dir.display()))?;
        }
        return Err(error).with_context(|| format!("activate mirror {}", origin_dir.display()));
    }

    remove_dir_if_exists(&backup_dir).await?;
    Ok(())
}

async fn remove_dir_if_exists(path: &Path) -> Result<()> {
    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove dir {}", path.display())),
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

    let metadata = read_sidecar_metadata(origin_dir, key).await?;
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
    let sidecar_path = metadata_sidecar_path(origin_dir, &key)?;
    if let Some(parent) = sidecar_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create metadata parent {}", parent.display()))?;
    }
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
                if path == origin_dir.join(METADATA_DIR_NAME) {
                    continue;
                }
                stack.push(path);
                continue;
            }

            if !file_type.is_file() {
                continue;
            }

            let key = relative_key(origin_dir, &path)?;
            if remote_keys.contains(&key) {
                continue;
            }

            tokio::fs::remove_file(&path)
                .await
                .with_context(|| format!("remove deleted local object {}", path.display()))?;

            let sidecar_path = metadata_sidecar_path(origin_dir, &key)?;
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

async fn remove_orphan_metadata_sidecars(
    origin_dir: &Path,
    remote_keys: &BTreeSet<String>,
) -> Result<()> {
    let metadata_dir = origin_dir.join(METADATA_DIR_NAME);
    let mut expected_paths = BTreeSet::new();

    for key in remote_keys {
        expected_paths.insert(metadata_sidecar_path(origin_dir, key)?);
    }

    let mut stack = vec![metadata_dir.clone()];
    while let Some(dir) = stack.pop() {
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("read metadata mirror dir {}", dir.display()))
            }
        };

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let file_type = entry.file_type().await?;

            if file_type.is_dir() {
                stack.push(path);
                continue;
            }

            if !file_type.is_file() || expected_paths.contains(&path) {
                continue;
            }

            tokio::fs::remove_file(&path)
                .await
                .with_context(|| format!("remove orphan metadata sidecar {}", path.display()))?;
        }
    }

    Ok(())
}

async fn read_sidecar_metadata(origin_dir: &Path, key: &str) -> Result<ObjectMetadata> {
    let sidecar_path = metadata_sidecar_path(origin_dir, key)?;

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

pub(crate) fn normalize_remote_key(key: &str) -> Result<String> {
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

    if key.starts_with(METADATA_DIR_NAME)
        && (key.len() == METADATA_DIR_NAME.len()
            || key.as_bytes().get(METADATA_DIR_NAME.len()) == Some(&b'/'))
    {
        return Err(anyhow!("invalid object path {key}"));
    }

    Ok(key.to_string())
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
                    created_at: None,
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

    fn remote_object(
        key: &str,
        body: &str,
        etag: Option<&str>,
        content_type: Option<&str>,
    ) -> RemoteObject {
        RemoteObject {
            key: key.to_string(),
            body: Bytes::from(body.to_string()),
            etag: etag.map(str::to_string),
            last_modified: None,
            content_type: content_type.map(str::to_string),
            cache_control: None,
        }
    }

    async fn requested_keys(storage: &FakeStorage) -> Vec<String> {
        let mut keys = storage.requested_keys.lock().await.clone();
        keys.sort();
        keys
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
        let metadata_path = metadata_sidecar_path(&temp.path().join("origins/web"), "index.html")
            .expect("metadata path");
        assert!(metadata_path.exists());
    }

    #[tokio::test]
    async fn staged_sync_fetches_only_changed_files_and_waits_for_activation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let syncer = MirrorSyncService::new(temp.path().join("origins"));
        let storage = FakeStorage::default();
        storage.objects.lock().await.insert(
            "index.html".to_string(),
            remote_object(
                "index.html",
                "old index",
                Some("index-v1"),
                Some("text/html"),
            ),
        );
        storage.objects.lock().await.insert(
            "same.css".to_string(),
            remote_object("same.css", "body{}", Some("same-v1"), Some("text/css")),
        );
        storage.objects.lock().await.insert(
            "removed.html".to_string(),
            remote_object(
                "removed.html",
                "removed",
                Some("removed-v1"),
                Some("text/html"),
            ),
        );
        syncer
            .sync_origin("web", &storage)
            .await
            .expect("initial sync");
        let previous_index = crate::services::freshness::build_origin_index(
            "web",
            storage.list_objects().await.expect("list previous"),
            chrono::Utc::now(),
        )
        .expect("previous index");
        storage.requested_keys.lock().await.clear();

        storage.objects.lock().await.insert(
            "index.html".to_string(),
            remote_object(
                "index.html",
                "new index",
                Some("index-v2"),
                Some("text/html"),
            ),
        );
        storage.objects.lock().await.remove("removed.html");
        storage.objects.lock().await.insert(
            "new.html".to_string(),
            remote_object("new.html", "new", Some("new-v1"), Some("text/html")),
        );

        let staged = syncer
            .stage_origin_sync("web", &storage, Some(&previous_index))
            .await
            .expect("stage sync");

        assert_eq!(staged.report.downloaded, 2);
        assert!(staged.diff.modified.contains("index.html"));
        assert!(staged.diff.added.contains("new.html"));
        assert!(staged.diff.removed.contains("removed.html"));
        assert!(staged.diff.unchanged.contains("same.css"));
        assert_eq!(
            tokio::fs::read_to_string(temp.path().join("origins/web/index.html"))
                .await
                .expect("read active index"),
            "old index"
        );
        assert_eq!(
            requested_keys(&storage).await,
            vec!["index.html".to_string(), "new.html".to_string()]
        );

        syncer
            .activate_staged_origin(staged)
            .await
            .expect("activate");

        assert_eq!(
            tokio::fs::read_to_string(temp.path().join("origins/web/index.html"))
                .await
                .expect("read activated index"),
            "new index"
        );
        assert!(temp.path().join("origins/web/new.html").exists());
        assert!(!temp.path().join("origins/web/removed.html").exists());
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
        let stale_metadata_path =
            metadata_sidecar_path(&origin_dir, "stale.html").expect("stale metadata path");
        tokio::fs::create_dir_all(stale_metadata_path.parent().expect("metadata parent"))
            .await
            .expect("mkdir metadata");
        tokio::fs::write(&stale_metadata_path, "{}")
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
        assert!(!stale_metadata_path.exists());
        assert!(origin_dir.join("assets/keep.css").exists());
    }

    #[tokio::test]
    async fn removes_stale_real_object_ending_meta_json_and_its_metadata() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("origins");
        let origin_dir = root.join("web");
        tokio::fs::create_dir_all(&origin_dir).await.expect("mkdir");
        tokio::fs::write(origin_dir.join("foo.meta.json"), "real object")
            .await
            .expect("write stale object");
        let metadata_path =
            metadata_sidecar_path(&origin_dir, "foo.meta.json").expect("metadata path");
        tokio::fs::create_dir_all(metadata_path.parent().expect("metadata parent"))
            .await
            .expect("mkdir metadata");
        tokio::fs::write(&metadata_path, r#"{"etag":"stale"}"#)
            .await
            .expect("write stale metadata");

        let storage = FakeStorage::default();

        MirrorSyncService::new(root)
            .sync_origin("web", &storage)
            .await
            .expect("sync succeeds");

        assert!(!origin_dir.join("foo.meta.json").exists());
        assert!(!metadata_path.exists());
    }

    #[tokio::test]
    async fn removes_orphan_metadata_sidecars() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("origins");
        let origin_dir = root.join("web");
        let orphan_path = origin_dir
            .join(METADATA_DIR_NAME)
            .join("aa")
            .join("orphan.json");
        tokio::fs::create_dir_all(orphan_path.parent().expect("orphan parent"))
            .await
            .expect("mkdir orphan metadata");
        tokio::fs::write(&orphan_path, "{}")
            .await
            .expect("write orphan metadata");

        let storage = FakeStorage::default();
        storage.objects.lock().await.insert(
            "index.html".to_string(),
            RemoteObject {
                key: "index.html".to_string(),
                body: Bytes::from_static(b"<h1>Hello</h1>"),
                etag: Some("abc".to_string()),
                last_modified: None,
                content_type: Some("text/html".to_string()),
                cache_control: None,
            },
        );

        MirrorSyncService::new(root)
            .sync_origin("web", &storage)
            .await
            .expect("sync succeeds");

        assert!(!orphan_path.exists());
        assert!(metadata_sidecar_path(&origin_dir, "index.html")
            .expect("metadata path")
            .exists());
    }

    #[tokio::test]
    async fn syncs_and_reads_metadata_for_long_key_with_bounded_sidecar_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("origins");
        let origin_dir = root.join("web");
        let long_key = (0..8)
            .map(|index| format!("segment-{index:02}-{}", "a".repeat(40)))
            .collect::<Vec<_>>()
            .join("/");
        let storage = FakeStorage::default();
        storage.objects.lock().await.insert(
            long_key.clone(),
            RemoteObject {
                key: long_key.clone(),
                body: Bytes::from_static(b"long key body"),
                etag: Some("long-etag".to_string()),
                last_modified: None,
                content_type: Some("text/plain".to_string()),
                cache_control: None,
            },
        );

        MirrorSyncService::new(root.clone())
            .sync_origin("web", &storage)
            .await
            .expect("sync succeeds");

        let metadata_path = metadata_sidecar_path(&origin_dir, &long_key).expect("metadata path");
        for component in metadata_path.components() {
            if let Component::Normal(part) = component {
                assert!(
                    part.to_string_lossy().len() <= 80,
                    "component is too long: {}",
                    part.to_string_lossy().len()
                );
            }
        }

        let object = LocalMirrorRepository::new(root)
            .read_object("web", &long_key)
            .await
            .expect("read")
            .expect("object");

        assert_eq!(object.body, Bytes::from_static(b"long key body"));
        assert_eq!(object.metadata.etag.as_deref(), Some("long-etag"));
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
        let metadata_path =
            metadata_sidecar_path(&origin_dir, "index.html").expect("metadata path");
        tokio::fs::create_dir_all(metadata_path.parent().expect("metadata parent"))
            .await
            .expect("mkdir metadata");
        tokio::fs::write(
            metadata_path,
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

    #[tokio::test]
    async fn failed_sync_keeps_previous_mirror_untouched() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("origins");
        let origin_dir = root.join("web");
        tokio::fs::create_dir_all(&origin_dir).await.expect("mkdir");
        tokio::fs::write(origin_dir.join("index.html"), "old")
            .await
            .expect("write old index");
        tokio::fs::write(origin_dir.join("keep.html"), "keep")
            .await
            .expect("write old keep");

        let storage = FailingStorage {
            fail_key: "keep.html".to_string(),
            objects: BTreeMap::from([
                (
                    "index.html".to_string(),
                    RemoteObject {
                        key: "index.html".to_string(),
                        body: Bytes::from_static(b"new"),
                        etag: Some("new-index".to_string()),
                        last_modified: None,
                        content_type: Some("text/html".to_string()),
                        cache_control: None,
                    },
                ),
                (
                    "keep.html".to_string(),
                    RemoteObject {
                        key: "keep.html".to_string(),
                        body: Bytes::from_static(b"new keep"),
                        etag: Some("new-keep".to_string()),
                        last_modified: None,
                        content_type: Some("text/html".to_string()),
                        cache_control: None,
                    },
                ),
            ]),
        };

        let error = MirrorSyncService::new(root)
            .sync_origin("web", &storage)
            .await
            .expect_err("sync fails");

        assert!(error.to_string().contains("forced failure"));
        assert_eq!(
            tokio::fs::read_to_string(origin_dir.join("index.html"))
                .await
                .expect("read old index"),
            "old"
        );
        assert_eq!(
            tokio::fs::read_to_string(origin_dir.join("keep.html"))
                .await
                .expect("read old keep"),
            "keep"
        );
    }

    struct FailingStorage {
        fail_key: String,
        objects: BTreeMap<String, RemoteObject>,
    }

    #[async_trait]
    impl RemoteStorage for FailingStorage {
        async fn list_objects(&self) -> anyhow::Result<Vec<RemoteObjectSummary>> {
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

        async fn get_object(&self, key: &str) -> anyhow::Result<RemoteObject> {
            if key == self.fail_key {
                anyhow::bail!("forced failure for {key}");
            }

            self.objects
                .get(key)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("missing {key}"))
        }
    }
}
