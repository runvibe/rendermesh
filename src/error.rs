use axum::{http::StatusCode, response::IntoResponse, Json};
use serde::Serialize;
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
            Unexpected(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "unexpected error".to_string(),
            ),
        };

        let payload = Json(ErrorResponse { error: message });
        (status, payload).into_response()
    }
}
