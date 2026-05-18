use std::{path::Path, sync::Arc};

use anyhow::Result;

use crate::{dto::manifest::RenderMeshManifest, services::manifest::parse_manifest_yaml};

#[derive(Clone, Default)]
pub struct ManifestRepository;

impl ManifestRepository {
    pub fn new() -> Self {
        Self
    }

    pub async fn load(&self, path: impl AsRef<Path>) -> Result<Arc<RenderMeshManifest>> {
        let content = tokio::fs::read_to_string(path).await?;
        Ok(Arc::new(parse_manifest_yaml(&content)?))
    }
}
