use std::{collections::BTreeMap, sync::Arc, time::Duration};

use anyhow::Result;
use rendermesh::{
    config::AppConfig,
    db::{init_pool, run_migrations},
    dto::{edge::EdgeConfig, manifest::RenderMeshManifest},
    libs::telemetry,
    repositories::{
        database::DatabaseRepository, local_mirror::LocalMirrorRepository,
        manifest::ManifestRepository, s3_storage::S3StorageRepository, sync::MirrorSyncService,
    },
    routes::create_router,
    services::{
        cors::CorsPolicy,
        edge_config::{default_edge_config, parse_edge_config_yaml},
        manifest::{load_manifest, HostResolver},
        render_gateway::RenderGatewayService,
    },
    state::AppState,
};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let config = AppConfig::from_env()?;
    let _telemetry = telemetry::init_tracing(config.otel_enabled)?;
    let pool = init_pool(&config.database_url).await?;

    if let Err(error) = run_migrations(&pool).await {
        tracing::error!("failed to run database migrations: {error}");
        return Err(error);
    }

    let render_gateway = build_render_gateway(&config.rendermesh_manifest).await?;
    let state = AppState::new(DatabaseRepository::new(pool), render_gateway);

    let router = create_router(state, &config);

    let addr = config.listen_addr()?;
    let listener = TcpListener::bind(addr).await?;
    tracing::info!("listening on {}", addr);
    if let Some(mcp_url) = config.mcp_endpoint_url() {
        tracing::info!("mcp enabled at {}", mcp_url);
    }

    axum::serve(listener, router).await?;
    Ok(())
}

async fn build_render_gateway(manifest_path: &str) -> Result<RenderGatewayService> {
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

    let edge_configs = load_edge_configs(&manifest, &mirror).await?;
    spawn_background_sync(manifest.clone(), syncer, storage_by_origin);

    Ok(RenderGatewayService::new(
        HostResolver::new(&manifest)?,
        CorsPolicy::from_manifest(&manifest),
        mirror,
        edge_configs,
    ))
}

async fn load_edge_configs(
    manifest: &Arc<RenderMeshManifest>,
    mirror: &LocalMirrorRepository,
) -> Result<BTreeMap<String, EdgeConfig>> {
    let mut configs = BTreeMap::new();

    for origin_id in manifest.origins.keys() {
        let config = match mirror
            .read_object(origin_id, "/_rendermesh/edge.yaml")
            .await?
        {
            Some(object) => {
                let content = String::from_utf8(object.body.to_vec())?;
                parse_edge_config_yaml(&content)?
            }
            None => {
                tracing::warn!(
                    origin = %origin_id,
                    "origin has no /_rendermesh/edge.yaml; using default edge config"
                );
                default_edge_config()
            }
        };
        configs.insert(origin_id.clone(), config);
    }

    Ok(configs)
}

fn spawn_background_sync(
    manifest: Arc<RenderMeshManifest>,
    syncer: MirrorSyncService,
    storage_by_origin: BTreeMap<String, S3StorageRepository>,
) {
    for (origin_id, storage) in storage_by_origin {
        let syncer = syncer.clone();
        let interval_seconds = manifest
            .origins
            .get(&origin_id)
            .and_then(|origin| origin.sync_interval_seconds)
            .unwrap_or(manifest.runtime.sync_interval_seconds);

        tokio::spawn(async move {
            let interval = Duration::from_secs(interval_seconds);
            loop {
                tokio::time::sleep(interval).await;
                match syncer.sync_origin(&origin_id, &storage).await {
                    Ok(report) => tracing::info!(
                        origin = %origin_id,
                        downloaded = report.downloaded,
                        "background origin sync completed"
                    ),
                    Err(error) => tracing::error!(
                        origin = %origin_id,
                        "background origin sync failed: {error}"
                    ),
                }
            }
        });
    }
}
