use std::sync::Arc;

use crate::services::{
    origin_refresh::OriginRefreshService, origin_runtime::OriginRuntimeStore,
    render_gateway::RenderGatewayService,
};

#[derive(Clone)]
pub struct AppState {
    inner: Arc<SharedState>,
}

struct SharedState {
    pub render_gateway: RenderGatewayService,
    pub origin_runtime: OriginRuntimeStore,
    pub origin_refresh: Option<OriginRefreshService>,
}

impl AppState {
    pub fn new(render_gateway: RenderGatewayService) -> Self {
        Self::new_with_runtime(render_gateway, OriginRuntimeStore::default())
    }

    pub fn new_with_runtime(
        render_gateway: RenderGatewayService,
        origin_runtime: OriginRuntimeStore,
    ) -> Self {
        let inner = SharedState {
            render_gateway,
            origin_runtime,
            origin_refresh: None,
        };
        Self {
            inner: Arc::new(inner),
        }
    }

    pub fn new_with_refresh(
        render_gateway: RenderGatewayService,
        origin_runtime: OriginRuntimeStore,
        origin_refresh: OriginRefreshService,
    ) -> Self {
        let inner = SharedState {
            render_gateway,
            origin_runtime,
            origin_refresh: Some(origin_refresh),
        };
        Self {
            inner: Arc::new(inner),
        }
    }

    pub fn render_gateway(&self) -> RenderGatewayService {
        self.inner.render_gateway.clone()
    }

    pub fn origin_runtime(&self) -> OriginRuntimeStore {
        self.inner.origin_runtime.clone()
    }

    pub fn origin_refresh(&self) -> Option<OriginRefreshService> {
        self.inner.origin_refresh.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::AppState;
    use crate::repositories::local_mirror::LocalMirrorRepository;
    use crate::services::{
        cors::CorsPolicy,
        edge_config::default_edge_config,
        manifest::{parse_manifest_yaml, HostResolver},
        render_gateway::RenderGatewayService,
    };

    #[tokio::test]
    async fn state_exposes_render_gateway() {
        let gateway = test_gateway();
        let state = AppState::new(gateway);

        let _ = state.render_gateway();
    }

    fn test_gateway() -> RenderGatewayService {
        let manifest = parse_manifest_yaml(
            r#"
version: 1
runtime:
  local_store_dir: ./unused
  sync_interval_seconds: 60
origins:
  web:
    type: s3
    bucket: web
    endpoint_env: WEB_ENDPOINT
    region_env: WEB_REGION
    access_key_id_env: WEB_ACCESS_KEY_ID
    secret_access_key_env: WEB_SECRET_ACCESS_KEY
hosts:
  app.test:
    origin: web
"#,
        )
        .expect("manifest parses");

        RenderGatewayService::new_for_tests(
            HostResolver::new(&manifest).expect("resolver"),
            CorsPolicy::from_manifest(&manifest),
            LocalMirrorRepository::new("./unused"),
            BTreeMap::from([("web".to_string(), default_edge_config())]),
        )
    }
}
