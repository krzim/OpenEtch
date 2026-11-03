//! [`AppError`]: what the shared glue can refuse, with the HTTP status the engine answers it with. The engine turns it
//! into its own `EngineError` one to one (same variants, same messages, same statuses), webcore answers it directly.

use commands::CommandError;
use plan::PlanError;
use serde_json::{Value, json};

/// Why a request was refused. Rendered as `{ "ok": false, "error": "..." }` with [`AppError::status`].
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// A document command failed (unknown id, bad input, missing reference).
    #[error(transparent)]
    Command(#[from] CommandError),
    /// Planning the job failed (out of field, power above the profile limit, ...).
    #[error(transparent)]
    Plan(#[from] PlanError),
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    NotFound(String),
    /// The request conflicts with the current state.
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Forbidden(String),
    /// The active device is design-only: OpenEtch cannot plan for or drive it.
    #[error("{0}")]
    NotControllable(String),
    /// An upload or file above the size limit.
    #[error("{0}")]
    TooLarge(String),
    /// A file in a format that cannot be read (415).
    #[error("{0}")]
    Unsupported(String),
    #[error("{0}")]
    Internal(String),
}

impl AppError {
    /// The HTTP status (as the engine's `EngineError::status`).
    pub fn status(&self) -> u16 {
        match self {
            AppError::Command(CommandError::NotFound { .. }) | AppError::NotFound(_) => 404,
            AppError::Command(_) | AppError::BadRequest(_) => 400,
            AppError::Plan(_) | AppError::NotControllable(_) => 422,
            AppError::Conflict(_) => 409,
            AppError::Forbidden(_) => 403,
            AppError::TooLarge(_) => 413,
            AppError::Unsupported(_) => 415,
            AppError::Internal(_) => 500,
        }
    }

    /// The JSON body: `{ok: false, error}`, plus `candidates` for a command that named a missing id.
    pub fn body(&self) -> Value {
        let mut body = json!({ "ok": false, "error": self.to_string() });
        if let AppError::Command(CommandError::NotFound { candidates, .. }) = self {
            body["candidates"] = json!(candidates);
        }
        body
    }
}

/// Shorthand for results of the shared glue.
pub type AppResult<T> = Result<T, AppError>;
