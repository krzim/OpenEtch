//! [`EngineError`]: everything an API call or MCP tool can refuse, with its HTTP status.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use commands::CommandError;
use machine::ControllerError;
use plan::PlanError;
use serde_json::{Value, json};

/// Why a request was refused. Rendered as `{ "ok": false, "error": "..." }` with a matching status.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// A document command failed (unknown id, bad input, missing reference).
    #[error(transparent)]
    Command(#[from] CommandError),
    /// Planning or encoding the job failed (out of field, power above the profile limit, ...).
    #[error(transparent)]
    Plan(#[from] PlanError),
    /// The controller refused or the link failed.
    #[error(transparent)]
    Machine(#[from] ControllerError),
    /// The job failed the machine profile's hard limits.
    #[error(transparent)]
    JobCheck(#[from] machine::JobCheckError),
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    NotFound(String),
    /// The request conflicts with the current state (busy, stale run, ...).
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    Internal(String),
}

impl EngineError {
    /// The HTTP status for this error.
    pub fn status(&self) -> StatusCode {
        match self {
            EngineError::Command(CommandError::NotFound { .. }) | EngineError::NotFound(_) => StatusCode::NOT_FOUND,
            EngineError::Command(_) | EngineError::BadRequest(_) => StatusCode::BAD_REQUEST,
            EngineError::Plan(_) | EngineError::JobCheck(_) => StatusCode::UNPROCESSABLE_ENTITY,
            EngineError::Machine(ControllerError::Busy(_)) | EngineError::Conflict(_) => StatusCode::CONFLICT,
            EngineError::Machine(ControllerError::Refused(_) | ControllerError::Field(_)) => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            EngineError::Machine(_) => StatusCode::BAD_GATEWAY,
            EngineError::Forbidden(_) => StatusCode::FORBIDDEN,
            EngineError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// The JSON body: `{ok: false, error}`, plus `candidates` for a command that named a missing id.
    pub fn body(&self) -> Value {
        let mut body = json!({ "ok": false, "error": self.to_string() });
        if let EngineError::Command(CommandError::NotFound { candidates, .. }) = self {
            body["candidates"] = json!(candidates);
        }
        body
    }
}

impl IntoResponse for EngineError {
    fn into_response(self) -> Response {
        (self.status(), Json(self.body())).into_response()
    }
}

impl From<tokio::task::JoinError> for EngineError {
    fn from(e: tokio::task::JoinError) -> Self {
        EngineError::Internal(format!("background task failed: {e}"))
    }
}
