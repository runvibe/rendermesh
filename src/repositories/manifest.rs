use std::path::Path;

use anyhow::{Context, Result};

#[derive(Clone, Default)]
pub struct ManifestRepository;

impl ManifestRepository {
    pub fn new() -> Self {
        Self
    }

    pub async fn load_content(&self, path: impl AsRef<Path>) -> Result<String> {
        let path = path.as_ref();
        tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("failed to read manifest file {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_manifest_file_error_includes_path() {
        let repository = ManifestRepository::new();
        let path = "./definitely-missing-rendermesh.yaml";

        let error = repository
            .load_content(path)
            .await
            .expect_err("missing file should fail");

        assert!(error.to_string().contains(path));
    }
}
