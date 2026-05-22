use anyhow::Result;
use rendermesh::{
    config::AppConfig, libs::telemetry, routes::create_router,
    services::startup::build_render_runtime, state::AppState,
};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let config = AppConfig::from_env()?;
    let _telemetry = telemetry::init_tracing(config.otel_enabled)?;

    let runtime = build_render_runtime(&config.rendermesh_manifest).await?;
    let state = AppState::new_with_runtime(runtime.render_gateway, runtime.origin_runtime);

    let router = create_router(state, &config);

    let addr = config.listen_addr()?;
    let listener = TcpListener::bind(addr).await?;
    tracing::info!("listening on {}", addr);

    axum::serve(listener, router).await?;
    Ok(())
}
