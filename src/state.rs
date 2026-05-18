use std::sync::Arc;

use crate::repositories::database::DatabaseRepository;
use crate::services::render_gateway::RenderGatewayService;

#[derive(Clone)]
pub struct AppState {
    inner: Arc<SharedState>,
}

struct SharedState {
    pub database: DatabaseRepository,
    pub render_gateway: RenderGatewayService,
}

impl AppState {
    pub fn new(database: DatabaseRepository, render_gateway: RenderGatewayService) -> Self {
        let inner = SharedState {
            database,
            render_gateway,
        };
        Self {
            inner: Arc::new(inner),
        }
    }

    pub fn database(&self) -> DatabaseRepository {
        self.inner.database.clone()
    }

    pub fn render_gateway(&self) -> RenderGatewayService {
        self.inner.render_gateway.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use sqlx::postgres::PgPoolOptions;

    use super::AppState;
    use crate::repositories::{database::DatabaseRepository, local_mirror::LocalMirrorRepository};
    use crate::services::{
        cors::CorsPolicy,
        edge_config::default_edge_config,
        manifest::{parse_manifest_yaml, HostResolver},
        render_gateway::RenderGatewayService,
    };

    #[tokio::test]
    async fn state_exposes_database_repository() {
        let pool = PgPoolOptions::new().connect_lazy("postgres://postgres:postgres@localhost/test");
        let repository = DatabaseRepository::new(pool.expect("lazy pool"));
        let gateway = test_gateway();
        let state = AppState::new(repository.clone(), gateway);

        let _ = state.database();
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
