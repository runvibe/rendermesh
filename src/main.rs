use anyhow::Result;
use rendermesh::{
    config::AppConfig,
    db::{init_pool, run_migrations},
    libs::telemetry,
    repositories::database::DatabaseRepository,
    routes::create_router,
    services::startup::build_render_gateway,
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
