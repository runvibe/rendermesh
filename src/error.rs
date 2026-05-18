use axum::{http::StatusCode, response::IntoResponse, Json};
use serde::Serialize;
use sqlx::Error as SqlxError;
use thiserror::Error;

pub type ApiResult<T> = Result<T, ApiError>;

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("misdirected request")]
    MisdirectedRequest,
    #[error("method not allowed")]
    MethodNotAllowed,
    #[error("bad gateway: {0}")]
    BadGateway(String),
    #[error("gateway timeout: {0}")]
    GatewayTimeout(String),
    #[error("unsupported media type: {0}")]
    UnsupportedMediaType(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error(transparent)]
    Database(#[from] SqlxError),
    #[error(transparent)]
    Unexpected(#[from] anyhow::Error),
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        use ApiError::*;

        let (status, message) = match &self {
            BadRequest(message) => (StatusCode::BAD_REQUEST, message.clone()),
            MisdirectedRequest => (StatusCode::MISDIRECTED_REQUEST, "unknown host".to_string()),
            MethodNotAllowed => (
                StatusCode::METHOD_NOT_ALLOWED,
                "method not allowed".to_string(),
            ),
            BadGateway(message) => (StatusCode::BAD_GATEWAY, message.clone()),
            GatewayTimeout(message) => (StatusCode::GATEWAY_TIMEOUT, message.clone()),
            UnsupportedMediaType(message) => (StatusCode::UNSUPPORTED_MEDIA_TYPE, message.clone()),
            NotFound(message) => (StatusCode::NOT_FOUND, message.clone()),
            Database(error) => match error {
                SqlxError::RowNotFound => (StatusCode::NOT_FOUND, "resource not found".to_string()),
                SqlxError::PoolTimedOut | SqlxError::PoolClosed => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "database connection error".to_string(),
                ),
                SqlxError::Database(db_error) => match db_error.code().as_deref() {
                    Some("23505") => (
                        StatusCode::CONFLICT,
                        "unique constraint violated".to_string(),
                    ),
                    Some("23514") => (StatusCode::BAD_REQUEST, db_error.message().to_string()),
                    Some("23503") => (
                        StatusCode::BAD_REQUEST,
                        "foreign key constraint violated".to_string(),
                    ),
                    Some("02000") => (StatusCode::NOT_FOUND, "resource not found".to_string()),
                    _ => (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "database error".to_string(),
                    ),
                },
                _ => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "database error".to_string(),
                ),
            },
            Unexpected(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "unexpected error".to_string(),
            ),
        };

        let payload = Json(ErrorResponse { error: message });
        (status, payload).into_response()
    }
}
