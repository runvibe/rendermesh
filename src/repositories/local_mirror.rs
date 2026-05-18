use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use serde::{Deserialize, Serialize};

pub const METADATA_DIR_NAME: &str = ".rendermesh-meta";

#[derive(Clone)]
pub struct LocalMirrorRepository {
    root: PathBuf,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ObjectMetadata {
    pub content_type: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub cache_control: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalObject {
    pub body: Bytes,
    pub metadata: ObjectMetadata,
}

impl LocalMirrorRepository {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn origin_dir(&self, origin_id: &str) -> Result<PathBuf> {
        validate_origin_id(origin_id)?;
        Ok(self.root.join(origin_id))
    }

    pub fn object_path(&self, origin_id: &str, object_path: &str) -> Result<PathBuf> {
        let origin_dir = self.origin_dir(origin_id)?;
        let normalized = normalize_object_path(object_path)?;
        let path = origin_dir.join(normalized);

        if path.starts_with(&origin_dir) {
            Ok(path)
        } else {
            Err(anyhow!("object path escapes origin directory"))
        }
    }

    pub async fn read_object(
        &self,
        origin_id: &str,
        object_path: &str,
    ) -> Result<Option<LocalObject>> {
        let origin_dir = self.origin_dir(origin_id)?;
        let normalized = normalize_object_path(object_path)?;
        let path = self.object_path(origin_id, &normalized)?;

        match tokio::fs::read(&path).await {
            Ok(body) => {
                let metadata = self.read_metadata(&origin_dir, &normalized).await?;
                Ok(Some(LocalObject {
                    body: Bytes::from(body),
                    metadata,
                }))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => {
                Err(error).with_context(|| format!("read local object {}", path.display()))
            }
        }
    }

    async fn read_metadata(&self, origin_dir: &Path, object_key: &str) -> Result<ObjectMetadata> {
        let metadata_path = metadata_sidecar_path(origin_dir, object_key)?;

        match tokio::fs::read_to_string(&metadata_path).await {
            Ok(content) => serde_json::from_str(&content).with_context(|| {
                format!("parse local object metadata {}", metadata_path.display())
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(ObjectMetadata::default())
            }
            Err(error) => Err(error)
                .with_context(|| format!("read local object metadata {}", metadata_path.display())),
        }
    }
}

pub fn normalize_object_path(path: &str) -> Result<String> {
    let trimmed = path.trim();
    let normalized = trimmed.trim_start_matches('/');

    if normalized.is_empty() || normalized.chars().any(char::is_control) {
        return Err(anyhow!("invalid object path {path}"));
    }

    let candidate = Path::new(normalized);
    if candidate.is_absolute()
        || candidate.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::Prefix(_) | Component::RootDir
            )
        })
    {
        return Err(anyhow!("invalid object path {path}"));
    }

    if is_reserved_metadata_key(normalized) {
        return Err(anyhow!("invalid object path {path}"));
    }

    Ok(normalized.to_string())
}

pub fn metadata_sidecar_path(origin_dir: &Path, object_key: &str) -> Result<PathBuf> {
    let normalized = normalize_object_path(object_key)?;
    Ok(origin_dir
        .join(METADATA_DIR_NAME)
        .join(format!("{}.json", encode_metadata_key(&normalized))))
}

fn validate_origin_id(origin_id: &str) -> Result<()> {
    let is_valid = !origin_id.is_empty()
        && origin_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-');

    is_valid
        .then_some(())
        .ok_or_else(|| anyhow!("invalid origin id {origin_id}"))
}

fn is_reserved_metadata_key(path: &str) -> bool {
    Path::new(path)
        .components()
        .next()
        .and_then(|component| match component {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        == Some(METADATA_DIR_NAME)
}

fn encode_metadata_key(key: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(key.len() * 2);

    for byte in key.as_bytes() {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }

    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_object_and_sidecar_metadata() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("origins");
        let repository = LocalMirrorRepository::new(root.clone());
        let origin_dir = repository.origin_dir("web").expect("origin dir");
        tokio::fs::create_dir_all(origin_dir.join("docs"))
            .await
            .expect("mkdir");
        tokio::fs::write(origin_dir.join("docs/index.html"), "<h1>Docs</h1>")
            .await
            .expect("write object");
        let metadata_path =
            metadata_sidecar_path(&origin_dir, "docs/index.html").expect("metadata path");
        tokio::fs::create_dir_all(metadata_path.parent().expect("metadata parent"))
            .await
            .expect("mkdir metadata");
        tokio::fs::write(
            metadata_path,
            r#"{"content_type":"text/html","etag":"abc","last_modified":"Mon, 01 Jan 2024 00:00:00 GMT","cache_control":"max-age=60"}"#,
        )
        .await
        .expect("write metadata");

        let object = repository
            .read_object("web", "/docs/index.html")
            .await
            .expect("read")
            .expect("object");

        assert_eq!(object.body, bytes::Bytes::from_static(b"<h1>Docs</h1>"));
        assert_eq!(object.metadata.content_type.as_deref(), Some("text/html"));
        assert_eq!(object.metadata.etag.as_deref(), Some("abc"));
        assert_eq!(
            object.metadata.last_modified.as_deref(),
            Some("Mon, 01 Jan 2024 00:00:00 GMT")
        );
        assert_eq!(object.metadata.cache_control.as_deref(), Some("max-age=60"));
    }

    #[tokio::test]
    async fn reads_object_ending_meta_json_with_separate_metadata() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("origins");
        let repository = LocalMirrorRepository::new(root.clone());
        let origin_dir = repository.origin_dir("web").expect("origin dir");
        tokio::fs::create_dir_all(&origin_dir).await.expect("mkdir");
        tokio::fs::write(origin_dir.join("foo.meta.json"), "real object")
            .await
            .expect("write object");
        let metadata_path =
            metadata_sidecar_path(&origin_dir, "foo.meta.json").expect("metadata path");
        tokio::fs::create_dir_all(metadata_path.parent().expect("metadata parent"))
            .await
            .expect("mkdir metadata");
        tokio::fs::write(
            metadata_path,
            r#"{"content_type":"application/json","etag":"meta-object"}"#,
        )
        .await
        .expect("write metadata");

        let object = repository
            .read_object("web", "/foo.meta.json")
            .await
            .expect("read")
            .expect("object");

        assert_eq!(object.body, bytes::Bytes::from_static(b"real object"));
        assert_eq!(
            object.metadata.content_type.as_deref(),
            Some("application/json")
        );
        assert_eq!(object.metadata.etag.as_deref(), Some("meta-object"));
    }

    #[tokio::test]
    async fn returns_none_for_missing_object() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repository = LocalMirrorRepository::new(temp.path().join("origins"));

        let object = repository
            .read_object("web", "/missing.html")
            .await
            .expect("read");

        assert!(object.is_none());
    }

    #[test]
    fn rejects_invalid_origin_id_and_path() {
        let repository = LocalMirrorRepository::new("./var/rendermesh/origins");

        assert!(repository.origin_dir("../bad").is_err());
        assert!(repository.object_path("web", "../secret").is_err());
        assert!(repository.object_path("web", "/../secret").is_err());
        assert!(repository
            .object_path("web", ".rendermesh-meta/foo")
            .is_err());
    }
}
