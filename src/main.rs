use anyhow::Result;
use rendermesh::{
    config::AppConfig, libs::telemetry, routes::create_router,
    services::startup::build_render_gateway, state::AppState,
};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let config = AppConfig::from_env()?;
    let _telemetry = telemetry::init_tracing(config.otel_enabled)?;

    let render_gateway = build_render_gateway(&config.rendermesh_manifest).await?;
    let state = AppState::new(render_gateway);

    let router = create_router(state, &config);

    let addr = config.listen_addr()?;
    let listener = TcpListener::bind(addr).await?;
    tracing::info!("listening on {}", addr);

    axum::serve(listener, router).await?;
    Ok(())
}
