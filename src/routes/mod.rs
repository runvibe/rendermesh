use axum::Router;
use axum_tracing_opentelemetry::middleware::{OtelAxumLayer, OtelInResponseLayer};
use tower_http::limit::RequestBodyLimitLayer;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::{config::AppConfig, state::AppState};

mod cors;
pub mod mcp;
pub mod render;
pub mod system;

#[derive(OpenApi)]
#[openapi(
    info(
        title = env!("CARGO_PKG_NAME"),
        description = "Starter API with health and echo features.",
        version = env!("CARGO_PKG_VERSION"),
        license(name = "Apache-2.0", url = "https://www.apache.org/licenses/LICENSE-2.0.html")
    )
)]
struct ApiDoc;

pub fn create_router(state: AppState, config: &AppConfig) -> Router {
    let (api_router, api) = utoipa_axum::router::OpenApiRouter::with_openapi(ApiDoc::openapi())
        .merge(system::router())
        .split_for_parts();

    let api_router = api_router.merge(SwaggerUi::new("/docs").url("/openapi.json", api));

    let api_router = if config.otel_enabled {
        api_router
            .layer(OtelInResponseLayer::default())
            .layer(OtelAxumLayer::default().filter(|path| path != "/health"))
    } else {
        api_router
    };

    let api_router = api_router
        .layer(cors::build_cors_layer(&config.cors, None))
        .layer(RequestBodyLimitLayer::new(config.body_limit_bytes));

    let router = if config.mcp.enabled {
        api_router.merge(mcp::router(&config.mcp))
    } else {
        api_router
    };

    router.fallback(render::render).with_state(state)
}
