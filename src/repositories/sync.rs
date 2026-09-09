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

    pub async fn discard_staged_origin(&self, staged: StagedOriginSync) -> Result<()> {
        remove_dir_if_exists(&staged.staging_dir).await
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
        let summary = index
            .files
            .get(key)
            .ok_or_else(|| anyhow!("listed object {key} is missing from the freshness index"))?;
        validate_downloaded_object(key, summary, &object)?;
        write_object(origin_dir, object).await?;
        downloaded += 1;
    }

    remove_deleted_objects(origin_dir, &remote_keys).await?;
    remove_orphan_metadata_sidecars(origin_dir, &remote_keys).await?;

    Ok((index, diff, SyncReport { downloaded }))
}

fn validate_downloaded_object(
    key: &str,
    summary: &crate::services::freshness::OriginFileState,
    object: &RemoteObject,
) -> Result<()> {
    let downloaded_key = normalize_remote_key(&object.key)?;
    if downloaded_key != key {
        return Err(anyhow!(
            "downloaded object key {downloaded_key} does not match listed key {key}"
        ));
    }
    if object.body.len() as u64 != summary.size {
        return Err(anyhow!(
            "downloaded object {key} changed size after it was listed"
        ));
    }
    if summary.etag.is_some() && object.etag != summary.etag {
        return Err(anyhow!(
            "downloaded object {key} changed etag after it was listed"
        ));
    }
    if summary.last_modified.is_some() && object.last_modified != summary.last_modified {
        return Err(anyhow!(
            "downloaded object {key} changed last-modified after it was listed"
        ));
    }
    Ok(())
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
mod tests;
