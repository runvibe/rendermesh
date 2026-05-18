use axum::{Extension, Router};
use axum_tracing_opentelemetry::middleware::{OtelAxumLayer, OtelInResponseLayer};
use tower_http::limit::RequestBodyLimitLayer;

use crate::{config::AppConfig, state::AppState};

mod cors;
pub mod render;
pub mod system;

pub fn create_router(state: AppState, config: &AppConfig) -> Router {
    let api_router = Router::new().merge(system::router());

    let api_router = if config.otel_enabled {
        api_router
            .layer(OtelInResponseLayer::default())
            .layer(OtelAxumLayer::default().filter(|path| path != "/health"))
    } else {
        api_router
    };

    let api_router = api_router.layer(cors::build_cors_layer(&config.cors, None));

    api_router
        .fallback(render::render)
        .layer(RequestBodyLimitLayer::new(config.body_limit_bytes))
        .layer(Extension(render::RenderRouteConfig {
            body_limit_bytes: config.body_limit_bytes,
        }))
        .with_state(state)
}
