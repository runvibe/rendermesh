use axum::Json;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{dto::health::HealthStatus, services::health, state::AppState};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(get_health))
}

#[utoipa::path(
    get,
    path = "/health",
    summary = "Health check",
    description = "Returns the current health status payload.",
    responses(
        (status = 200, description = "Service is healthy", body = HealthStatus)
    )
)]
async fn get_health() -> Json<HealthStatus> {
    Json(health::health_status())
}
