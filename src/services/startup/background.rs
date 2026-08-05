use std::{sync::Arc, time::Duration};

use crate::{
    dto::manifest::RenderMeshManifest,
    services::origin_refresh::{OriginRefreshService, OriginRefreshTrigger},
};

pub(super) fn spawn_background_sync(
    manifest: Arc<RenderMeshManifest>,
    origin_refresh: OriginRefreshService,
) {
    for origin_id in manifest.origins.keys().cloned().collect::<Vec<_>>() {
        let origin_refresh = origin_refresh.clone();
        let interval_seconds = manifest
            .origins
            .get(&origin_id)
            .and_then(|origin| origin.sync_interval_seconds())
            .unwrap_or(manifest.runtime.sync_interval_seconds);

        tokio::spawn(async move {
            let interval = Duration::from_secs(interval_seconds);
            loop {
                tokio::time::sleep(interval).await;
                if let Err(error) = origin_refresh
                    .refresh_origin(&origin_id, OriginRefreshTrigger::Background)
                    .await
                {
                    tracing::error!(origin = %origin_id, "background origin sync failed: {error}");
                }
            }
        });
    }
}
