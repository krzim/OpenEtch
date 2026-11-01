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
    /// The active device is design-only: OpenEtch cannot plan for or drive it.
    #[error("{0}")]
    NotControllable(String),
    /// An upload or project file above the size limit.
    #[error("{0}")]
    TooLarge(String),
    /// A file in a format the engine cannot read (415), e.g. LightBurn's binary `.lbart`.
    #[error("{0}")]
    Unsupported(String),
    /// Run approval refused for operator-PIN reasons; `code` is `pin_not_set`, `pin_required`, `pin_invalid` or
    /// `pin_locked` (see the `pin` module).
    #[error("{message}")]
    Approval { code: &'static str, message: String },
    #[error("{0}")]
    Internal(String),
}

impl EngineError {
    /// The HTTP status for this error.
    pub fn status(&self) -> StatusCode {
        match self {
            EngineError::Command(CommandError::NotFound { .. }) | EngineError::NotFound(_) => StatusCode::NOT_FOUND,
            EngineError::Command(_) | EngineError::BadRequest(_) => StatusCode::BAD_REQUEST,
            EngineError::Plan(_) | EngineError::JobCheck(_) | EngineError::NotControllable(_) => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            EngineError::Machine(ControllerError::Busy(_)) | EngineError::Conflict(_) => StatusCode::CONFLICT,
            EngineError::Machine(ControllerError::Grbl(
                machine::grbl::GrblError::Locked(_) | machine::grbl::GrblError::DoorOpen | machine::grbl::GrblError::NotIdle(_),
            )) => StatusCode::CONFLICT,
            EngineError::Machine(
                ControllerError::Refused(_)
                | ControllerError::Field(_)
                | ControllerError::Grbl(
                    machine::grbl::GrblError::Console(_) | machine::grbl::GrblError::Program(_) | machine::grbl::GrblError::Settings(_),
                ),
            ) => StatusCode::UNPROCESSABLE_ENTITY,
            EngineError::Machine(_) => StatusCode::BAD_GATEWAY,
            EngineError::Forbidden(_) | EngineError::Approval { .. } => StatusCode::FORBIDDEN,
            EngineError::TooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            EngineError::Unsupported(_) => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            EngineError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// The JSON body: `{ok: false, error}`, plus `candidates` for a command that named a missing id and `code` for
    /// an approval refusal.
    pub fn body(&self) -> Value {
        let mut body = json!({ "ok": false, "error": self.to_string() });
        match self {
            EngineError::Command(CommandError::NotFound { candidates, .. }) => body["candidates"] = json!(candidates),
            EngineError::Approval { code, .. } => body["code"] = json!(code),
            _ => {}
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
