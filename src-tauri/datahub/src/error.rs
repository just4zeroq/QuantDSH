//! Unified error type. Every fallible path in the daemon funnels through this so
//! the HTTP layer has exactly one place to decide status codes.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("unauthorized")]
    Unauthorized,

    #[error("not found: {0}")]
    NotFound(String),

    /// Rejected by the read-only SQL guard. Not a server fault — the caller
    /// asked for something we refuse to run.
    #[error("sql blocked [{code}]: {reason}")]
    SqlBlocked { code: &'static str, reason: String },

    #[error("duckdb: {0}")]
    DuckDb(String),

    #[error("sqlite: {0}")]
    Sqlite(String),

    #[error("source: {0}")]
    Source(String),

    #[error("internal: {0}")]
    Internal(String),
}

impl AppError {
    pub fn status(&self) -> StatusCode {
        match self {
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Unauthorized => StatusCode::UNAUTHORIZED,
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::SqlBlocked { .. } => StatusCode::BAD_REQUEST,
            AppError::DuckDb(_) | AppError::Sqlite(_) | AppError::Internal(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
            AppError::Source(_) => StatusCode::BAD_GATEWAY,
        }
    }

    /// Machine-readable tag so the UI can branch without parsing prose.
    pub fn code(&self) -> &'static str {
        match self {
            AppError::BadRequest(_) => "bad_request",
            AppError::Unauthorized => "unauthorized",
            AppError::NotFound(_) => "not_found",
            AppError::SqlBlocked { code, .. } => code,
            AppError::DuckDb(_) => "duckdb_error",
            AppError::Sqlite(_) => "sqlite_error",
            AppError::Source(_) => "source_error",
            AppError::Internal(_) => "internal_error",
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();
        let body = json!({
            "error": { "code": self.code(), "message": self.to_string() }
        });
        (status, Json(body)).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;

// ── Convenience conversions from the two drivers ──

impl From<duckdb::Error> for AppError {
    fn from(e: duckdb::Error) -> Self {
        AppError::DuckDb(e.to_string())
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        AppError::Sqlite(e.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::BadRequest(format!("invalid json: {e}"))
    }
}
