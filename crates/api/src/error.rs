use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// Every error leaves the API as `{ "error": { "code", "message" } }`.
/// `code` is stable for clients to branch on; `message` is Indonesian and may
/// be shown to the person at the door as-is.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{message}")]
    Client { status: StatusCode, code: &'static str, message: String },
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self::Client { status, code, message: message.into() }
    }
    pub fn bad_request(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", message)
    }
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", message)
    }
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message)
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        Self::Internal(e.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            Self::Client { status, code, message } => {
                (status, Json(json!({ "error": { "code": code, "message": message } }))).into_response()
            }
            Self::Internal(e) => {
                tracing::error!(error = ?e, "internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(
                        json!({ "error": { "code": "internal", "message": "Terjadi kesalahan di server." } }),
                    ),
                )
                    .into_response()
            }
        }
    }
}
