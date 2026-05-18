use axum::{routing::get, Json, Router};

use crate::{dto::health::HealthStatus, services::health, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new().route("/health", get(get_health))
}

async fn get_health() -> Json<HealthStatus> {
    Json(health::health_status())
}
