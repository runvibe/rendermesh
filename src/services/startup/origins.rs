use std::{collections::BTreeMap, path::Path};

use anyhow::Result;

use crate::{
    dto::manifest::RenderMeshManifest, repositories::origin_storage::OriginStorageRepository,
};

pub(super) struct StartupOrigins {
    pub storage_by_origin: BTreeMap<String, OriginStorageRepository>,
    pub activation_barrier_by_origin: BTreeMap<String, String>,
    pub origin_buckets: BTreeMap<String, String>,
    pub origin_edge_contexts: BTreeMap<String, serde_json::Value>,
}

pub(super) async fn build_startup_origins(
    manifest: &RenderMeshManifest,
    manifest_path: &str,
) -> Result<StartupOrigins> {
    let manifest_dir = Path::new(manifest_path)
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut storage_by_origin = BTreeMap::new();
    let mut activation_barrier_by_origin = BTreeMap::new();

    for (origin_id, origin) in &manifest.origins {
        let storage = OriginStorageRepository::from_origin_config(origin, manifest_dir).await?;
        storage_by_origin.insert(origin_id.clone(), storage);
        if let Some(path) = origin.activation_barrier_path() {
            activation_barrier_by_origin.insert(origin_id.clone(), path.to_string());
        }
    }

    Ok(StartupOrigins {
        storage_by_origin,
        activation_barrier_by_origin,
        origin_buckets: origin_buckets(manifest),
        origin_edge_contexts: origin_edge_contexts(manifest),
    })
}

fn origin_buckets(manifest: &RenderMeshManifest) -> BTreeMap<String, String> {
    manifest
        .origins
        .iter()
        .map(|(origin_id, origin)| (origin_id.clone(), origin.edge_context_bucket(origin_id)))
        .collect()
}

fn origin_edge_contexts(manifest: &RenderMeshManifest) -> BTreeMap<String, serde_json::Value> {
    manifest
        .origins
        .iter()
        .filter_map(|(origin_id, origin)| {
            origin
                .edge_context()
                .cloned()
                .map(|context| (origin_id.clone(), context))
        })
        .collect()
}
