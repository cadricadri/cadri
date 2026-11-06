//! JSON API errors shared by routes and memory publish.

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// Rendered as `{"error": message}` with `status`, plus `"paths"` when set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
    /// Paths a memory edit touched that changed since the caller's base.
    pub paths: Option<Vec<String>>,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        ApiError {
            status,
            message: message.into(),
            paths: None,
        }
    }

    /// 409 `{"error": "conflict", "paths": [...]}`.
    pub fn conflict(paths: Vec<String>) -> Self {
        ApiError {
            paths: Some(paths),
            ..ApiError::new(StatusCode::CONFLICT, "conflict")
        }
    }

    pub fn unauthorized() -> Self {
        ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = match self.paths {
            Some(paths) => json!({ "error": self.message, "paths": paths }),
            None => json!({ "error": self.message }),
        };
        (self.status, Json(body)).into_response()
    }
}

impl From<JsonRejection> for ApiError {
    fn from(rejection: JsonRejection) -> Self {
        ApiError::new(rejection.status(), rejection.body_text())
    }
}

impl From<QueryRejection> for ApiError {
    fn from(rejection: QueryRejection) -> Self {
        ApiError::new(rejection.status(), rejection.body_text())
    }
}
