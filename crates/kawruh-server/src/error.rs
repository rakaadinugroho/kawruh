//! API errors: HTTP status + `{"error", "code"}` JSON.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    /// Seconds for a `Retry-After` header.
    pub retry_after: Option<u64>,
}

pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> ApiError {
        ApiError {
            status,
            code,
            message: message.into(),
            retry_after: None,
        }
    }
    pub fn bad_request(code: &'static str, message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::BAD_REQUEST, code, message)
    }
    pub fn unauthorized() -> ApiError {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "missing or invalid token",
        )
    }
    pub fn forbidden(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::FORBIDDEN, "forbidden", message)
    }
    pub fn not_found(code: &'static str, message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::NOT_FOUND, code, message)
    }
    pub fn conflict(code: &'static str, message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::CONFLICT, code, message)
    }
    pub fn too_many_attempts(wait: std::time::Duration) -> ApiError {
        let secs = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
        let secs = secs.max(1);
        let mut e = ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_attempts",
            format!("too many failed login attempts; try again in {secs} seconds"),
        );
        e.retry_after = Some(secs);
        e
    }
    pub fn internal(e: impl std::fmt::Display) -> ApiError {
        tracing::error!("internal error: {e}");
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "internal server error",
        )
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> ApiError {
        ApiError::internal(e)
    }
}

impl From<kawruh_core::store::StoreError> for ApiError {
    fn from(e: kawruh_core::store::StoreError) -> ApiError {
        ApiError::internal(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(serde_json::json!({ "error": self.message, "code": self.code }));
        let mut res = (self.status, body).into_response();
        if self.status == StatusCode::UNAUTHORIZED {
            res.headers_mut()
                .insert("WWW-Authenticate", http::HeaderValue::from_static("Bearer"));
        }
        if let Some(secs) = self.retry_after {
            res.headers_mut()
                .insert(http::header::RETRY_AFTER, http::HeaderValue::from(secs));
        }
        res
    }
}
