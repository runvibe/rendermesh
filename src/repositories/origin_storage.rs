use std::path::Path;

use anyhow::Result;
use async_trait::async_trait;

use crate::{
    dto::manifest::OriginConfig,
    repositories::{
        local_directory_storage::LocalDirectoryStorageRepository,
        s3_storage::S3StorageRepository,
        sync::{RemoteObject, RemoteObjectSummary, RemoteStorage},
    },
};

#[derive(Clone)]
pub enum OriginStorageRepository {
    S3(S3StorageRepository),
    Local(LocalDirectoryStorageRepository),
}

impl OriginStorageRepository {
    pub async fn from_origin_config(origin: &OriginConfig, manifest_dir: &Path) -> Result<Self> {
        match origin {
            OriginConfig::S3(origin) => Ok(Self::S3(
                S3StorageRepository::from_origin_config(origin).await?,
            )),
            OriginConfig::Local(origin) => {
                let path = Path::new(&origin.path);
                let resolved = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    manifest_dir.join(path)
                };
                Ok(Self::Local(LocalDirectoryStorageRepository::new(resolved)?))
            }
        }
    }
}

#[async_trait]
impl RemoteStorage for OriginStorageRepository {
    async fn list_objects(&self) -> Result<Vec<RemoteObjectSummary>> {
        match self {
            Self::S3(storage) => storage.list_objects().await,
            Self::Local(storage) => storage.list_objects().await,
        }
    }

    async fn get_object(&self, key: &str) -> Result<RemoteObject> {
        match self {
            Self::S3(storage) => storage.get_object(key).await,
            Self::Local(storage) => storage.get_object(key).await,
        }
    }
}
