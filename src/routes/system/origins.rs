use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Extension, Json, Router,
};

use crate::{
    dto::origin_sync::{OriginSyncErrorBody, OriginSyncErrorResponse},
    routes::SystemRouteConfig,
    services::origin_refresh::{OriginRefreshError, OriginRefreshTrigger},
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
        .route("/_rendermesh/origins/{origin_id}/sync", post(sync_origin))
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

async fn sync_origin(
    State(state): State<AppState>,
    Extension(config): Extension<SystemRouteConfig>,
    Path(origin_id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(response) = authorize_manual_sync(&config, &headers) {
        return response;
    }

    let Some(origin_refresh) = state.origin_refresh() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "origin_sync_unavailable",
            "origin refresh service is not available",
        );
    };

    match origin_refresh
        .refresh_origin(&origin_id, OriginRefreshTrigger::Manual)
        .await
    {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(OriginRefreshError::NotFound) => error_response(
            StatusCode::NOT_FOUND,
            "origin_not_found",
            "origin not found",
        ),
        Err(OriginRefreshError::AlreadyRunning) => error_response(
            StatusCode::CONFLICT,
            "origin_sync_in_progress",
            "origin sync already running",
        ),
        Err(OriginRefreshError::Failed(error)) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "origin_sync_failed",
            &error.to_string(),
        ),
    }
}

fn authorize_manual_sync(
    config: &SystemRouteConfig,
    headers: &HeaderMap,
) -> Result<(), axum::response::Response> {
    let Some(expected_token) = config.admin_token.as_deref() else {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "manual_sync_disabled",
            "manual sync is disabled",
        ));
    };

    let Some(header) = headers.get(axum::http::header::AUTHORIZATION) else {
        return Err(error_response(
            StatusCode::UNAUTHORIZED,
            "missing_authorization",
            "authorization header is required",
        ));
    };

    let Ok(value) = header.to_str() else {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "invalid_authorization",
            "authorization header is invalid",
        ));
    };

    let Some(token) = value.strip_prefix("Bearer ") else {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "invalid_authorization",
            "authorization header must use bearer token",
        ));
    };

    if token != expected_token {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "invalid_token",
            "admin token is invalid",
        ));
    }

    Ok(())
}

fn error_response(
    status: StatusCode,
    code: impl Into<String>,
    message: impl Into<String>,
) -> axum::response::Response {
    (
        status,
        Json(OriginSyncErrorResponse {
            error: OriginSyncErrorBody {
                code: code.into(),
                message: message.into(),
            },
        }),
    )
        .into_response()
}
