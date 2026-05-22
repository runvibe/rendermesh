use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};

use crate::{
    services::origin_runtime::{OriginListDebug, OriginSnapshotDebug},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/_rendermesh/origins", get(list_origins))
        .route(
            "/_rendermesh/origins/{origin_id}/snapshot",
            get(get_origin_snapshot),
        )
        .route(
            "/_rendermesh/origins/{origin_id}/freshness",
            get(get_origin_snapshot),
        )
}

async fn list_origins(State(state): State<AppState>) -> Json<OriginListDebug> {
    Json(state.origin_runtime().list())
}

async fn get_origin_snapshot(
    State(state): State<AppState>,
    Path(origin_id): Path<String>,
) -> Result<Json<OriginSnapshotDebug>, StatusCode> {
    state
        .origin_runtime()
        .get(&origin_id)
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}
