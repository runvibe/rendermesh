use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use bytes::Bytes;
use sha2::{Digest, Sha256};

use crate::repositories::{
    local_mirror::METADATA_DIR_NAME,
    sync::{normalize_remote_key, RemoteObject, RemoteObjectSummary, RemoteStorage},
};

#[derive(Clone)]
pub struct LocalDirectoryStorageRepository {
    root: PathBuf,
}

impl LocalDirectoryStorageRepository {
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let root = std::fs::canonicalize(path.as_ref())
            .with_context(|| format!("canonicalize local origin {}", path.as_ref().display()))?;
        let metadata = std::fs::metadata(&root)
            .with_context(|| format!("read local origin metadata {}", root.display()))?;

        if !metadata.is_dir() {
            return Err(anyhow!(
                "local origin {} is not a directory",
                root.display()
            ));
        }

        Ok(Self { root })
    }

    fn object_path(&self, key: &str) -> Result<PathBuf> {
        let key = normalize_remote_key(key)?;
        let path = self.root.join(key);
        let canonical = std::fs::canonicalize(&path)
            .with_context(|| format!("canonicalize local origin object {}", path.display()))?;

        if !canonical.starts_with(&self.root) {
            return Err(anyhow!("object path escapes local origin directory"));
        }

        Ok(canonical)
    }
}

#[async_trait]
impl RemoteStorage for LocalDirectoryStorageRepository {
    async fn list_objects(&self) -> Result<Vec<RemoteObjectSummary>> {
        let mut output = Vec::new();
        let mut stack = vec![self.root.clone()];

        while let Some(dir) = stack.pop() {
            let mut entries = tokio::fs::read_dir(&dir)
                .await
                .with_context(|| format!("read local origin directory {}", dir.display()))?;

            while let Some(entry) = entries.next_entry().await? {
                let file_type = entry.file_type().await?;
                let path = entry.path();
                let key = relative_key(&self.root, &path)?;

                if reserved_metadata_key(&key) {
                    continue;
                }

                if file_type.is_dir() {
                    stack.push(path);
                } else if file_type.is_file() {
                    let metadata = entry.metadata().await?;
                    output.push(summary_for_path(&path, key, &metadata).await?);
                }
            }
        }

        output.sort_by(|left, right| left.key.cmp(&right.key));
        Ok(output)
    }

    async fn get_object(&self, key: &str) -> Result<RemoteObject> {
        let normalized_key = normalize_remote_key(key)?;
        let path = self.object_path(&normalized_key)?;
        let metadata = tokio::fs::metadata(&path)
            .await
            .with_context(|| format!("read local origin object metadata {}", path.display()))?;

        if !metadata.is_file() {
            return Err(anyhow!(
                "local origin object {normalized_key} is not a file"
            ));
        }

        let body = tokio::fs::read(&path)
            .await
            .with_context(|| format!("read local origin object {}", path.display()))?;

        Ok(RemoteObject {
            key: normalized_key.clone(),
            body: Bytes::from(body),
            etag: Some(content_etag(&path).await?),
            last_modified: metadata.modified().ok().map(system_time_to_rfc3339),
            content_type: mime_guess::from_path(&normalized_key)
                .first_raw()
                .map(ToString::to_string),
            cache_control: None,
        })
    }
}

async fn summary_for_path(
    path: &Path,
    key: String,
    metadata: &std::fs::Metadata,
) -> Result<RemoteObjectSummary> {
    Ok(RemoteObjectSummary {
        content_type: mime_guess::from_path(&key)
            .first_raw()
            .map(ToString::to_string),
        key,
        created_at: metadata.created().ok().map(system_time_to_rfc3339),
        etag: Some(content_etag(path).await?),
        last_modified: metadata.modified().ok().map(system_time_to_rfc3339),
        size: metadata.len(),
        cache_control: None,
    })
}

async fn content_etag(path: &Path) -> Result<String> {
    let body = tokio::fs::read(path)
        .await
        .with_context(|| format!("hash local origin object {}", path.display()))?;
    let digest = Sha256::digest(&body);
    Ok(format!("sha256:{digest:x}"))
}

fn relative_key(root: &Path, path: &Path) -> Result<String> {
    let relative = path.strip_prefix(root).with_context(|| {
        format!(
            "local origin path {} is outside root {}",
            path.display(),
            root.display()
        )
    })?;

    Ok(relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

fn reserved_metadata_key(key: &str) -> bool {
    key == METADATA_DIR_NAME
        || key
            .strip_prefix(METADATA_DIR_NAME)
            .is_some_and(|remaining| remaining.starts_with('/'))
}

fn system_time_to_rfc3339(time: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(time).to_rfc3339()
}
