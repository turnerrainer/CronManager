//! Structured errors. Every variant maps to a specific HTTP
//! status via `IntoResponse`.
//!
//! Fixes JVM CronController's habit of throwing bare
//! `RuntimeException` on every code path — see docs/DESIGN.md §3.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use thiserror::Error;

/// Every failure surfaced to a caller. `Internal` is the escape
/// hatch for unexpected bugs; every other variant is a business
/// outcome with a matching HTTP status.
#[derive(Debug, Error)]
pub enum CronManagerError {
    #[error("job not found: {group}/{name}")]
    JobNotFound { group: String, name: String },

    #[error("job already running: {group}/{name}")]
    JobAlreadyRunning { group: String, name: String },

    #[error("invalid job definition {source_path}: {reason}")]
    InvalidJobDefinition { source_path: String, reason: String },

    #[error("invalid config file {path}: {reason}")]
    InvalidConfig { path: String, reason: String },

    #[error("invalid cron expression '{expression}': {reason}")]
    InvalidCron { expression: String, reason: String },

    #[error("request body exceeds {limit} bytes")]
    RequestTooLarge { limit: usize },

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("upstream HTTP error {status}: {body}")]
    UpstreamHttpError { status: u16, body: String },

    #[error("upstream request timed out after {seconds}s")]
    UpstreamTimeout { seconds: u64 },

    #[error("upstream response exceeds {limit} bytes")]
    UpstreamBodyTooLarge { limit: usize },

    #[error("shell command exited non-zero: {message}")]
    ShellFailed { message: String },

    #[error("shell command exceeded {seconds}s wall clock")]
    ShellTimeout { seconds: u64 },

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("yaml parse error at {path}: {source}")]
    YamlParse {
        path: String,
        #[source]
        source: serde_yaml_ng::Error,
    },

    #[error("database error: {0}")]
    Database(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl CronManagerError {
    pub fn status(&self) -> StatusCode {
        match self {
            Self::JobNotFound { .. } => StatusCode::NOT_FOUND,
            Self::JobAlreadyRunning { .. } => StatusCode::CONFLICT,
            Self::InvalidJobDefinition { .. }
            | Self::InvalidConfig { .. }
            | Self::InvalidCron { .. }
            | Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::RequestTooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
            Self::UpstreamHttpError { .. } | Self::UpstreamBodyTooLarge { .. } => {
                StatusCode::BAD_GATEWAY
            }
            Self::UpstreamTimeout { .. } => StatusCode::GATEWAY_TIMEOUT,
            Self::ShellFailed { .. }
            | Self::ShellTimeout { .. }
            | Self::Io(_)
            | Self::YamlParse { .. }
            | Self::Database(_)
            | Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::JobNotFound { .. } => "job_not_found",
            Self::JobAlreadyRunning { .. } => "job_already_running",
            Self::InvalidJobDefinition { .. } => "invalid_job_definition",
            Self::InvalidConfig { .. } => "invalid_config",
            Self::InvalidCron { .. } => "invalid_cron",
            Self::RequestTooLarge { .. } => "request_too_large",
            Self::BadRequest(_) => "bad_request",
            Self::UpstreamHttpError { .. } => "upstream_http_error",
            Self::UpstreamTimeout { .. } => "upstream_timeout",
            Self::UpstreamBodyTooLarge { .. } => "upstream_body_too_large",
            Self::ShellFailed { .. } => "shell_failed",
            Self::ShellTimeout { .. } => "shell_timeout",
            Self::Io(_) => "io_error",
            Self::YamlParse { .. } => "yaml_parse_error",
            Self::Database(_) => "database_error",
            Self::Internal(_) => "internal_error",
        }
    }
}

impl IntoResponse for CronManagerError {
    fn into_response(self) -> Response {
        // WARN for anything an operator would want to see in
        // logs; DEBUG for expected outcomes like "job not found".
        match self {
            Self::JobNotFound { .. } | Self::JobAlreadyRunning { .. } => {
                tracing::debug!("request failed: {}", self);
            }
            _ => tracing::warn!("request failed: {}", self),
        }
        let status = self.status();
        let body = match &self {
            Self::RequestTooLarge { limit } | Self::UpstreamBodyTooLarge { limit } => json!({
                "error": self.code(),
                "message": self.to_string(),
                "limit": limit,
            }),
            _ => json!({
                "error": self.code(),
                "message": self.to_string(),
            }),
        };
        (status, Json(body)).into_response()
    }
}
