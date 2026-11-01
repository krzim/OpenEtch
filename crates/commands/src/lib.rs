//! The command registry. Every change to a [`Document`] goes through a registered command, whoever issues it: the
//! web UI, a keyboard shortcut, a test, or an AI agent over MCP. Each command has a stable dotted id, a JSON Schema for
//! its input (from `schemars`), a risk class, and a description that doubles as model-facing documentation.
//!
//! [`Session`] owns the document, runs commands and keeps a Fusion-style timeline (see the `session` module): every
//! recorded step with its invocations, origin and summary, a marker that can roll back and forward, insert-and-replay
//! when editing after moving the marker on the timeline, and undo/redo as moving the marker by one step (an edit after
//! undo discards the undone steps, LightBurn-style). Replacing the document starts a fresh timeline.

use doc::Document;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod builtin;
mod session;

pub use builtin::{ORIGIN_KEY, WorkspaceOrigin, lbrn2_device_name};
pub use session::{HistoryEntry, HistoryStep, HistoryView, MAX_STEPS, MarkerMove, Session, StepState};

/// What a command can affect. Machine actions (`Pointer`, `Emit`) are not document commands; they are listed here so
/// the engine can expose one catalogue, but the engine gates them separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// Reads only.
    Query,
    /// Changes the document.
    Edit,
    /// Moves the galvos with the red pointer; never fires.
    Pointer,
    /// Fires the laser.
    Emit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandSpec {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub risk: Risk,
    pub input_schema: Value,
}

/// Who issued a command. Shown in the history; `Agent` entries can be filtered or reverted as a group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "kind", content = "name")]
pub enum Origin {
    Ui,
    Agent(String),
    Script,
}

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("unknown command '{0}'")]
    Unknown(String),
    #[error("invalid input for '{id}': {message}")]
    InvalidInput { id: String, message: String },
    /// A reference (object/layer id) that does not exist. `candidates` lists close matches so the caller can re-issue
    /// with an exact id; commands never guess.
    #[error("{message}")]
    NotFound { message: String, candidates: Vec<String> },
    #[error("{0}")]
    Failed(String),
    #[error("nothing to {0}")]
    NothingTo(&'static str),
}

impl From<doc::IdsExhausted> for CommandError {
    fn from(e: doc::IdsExhausted) -> Self {
        CommandError::Failed(e.to_string())
    }
}

pub type Result<T, E = CommandError> = std::result::Result<T, E>;

/// A document command. Implementations live in `builtin`.
pub trait Command: Send + Sync + 'static {
    type Input: DeserializeOwned + JsonSchema;
    const ID: &'static str;
    const TITLE: &'static str;
    /// Model-facing documentation: what it does, units, frames, and what it returns.
    const DESCRIPTION: &'static str;
    const RISK: Risk;

    /// Apply to `doc`. Must leave `doc` unchanged on error. Returns a JSON result for the caller (ids created, etc.)
    /// and a one-line summary for the history.
    fn apply(doc: &mut Document, input: Self::Input) -> Result<(Value, String)>;
}

type ApplyFn = fn(&mut Document, Value) -> Result<(Value, String)>;

struct Entry {
    spec: CommandSpec,
    apply: ApplyFn,
}

pub struct Registry {
    entries: Vec<Entry>,
}

impl Registry {
    /// The registry with every built-in command.
    pub fn new() -> Self {
        let mut r = Registry { entries: Vec::new() };
        builtin::register_all(&mut r);
        r
    }

    pub fn register<C: Command>(&mut self) {
        fn apply<C: Command>(doc: &mut Document, params: Value) -> Result<(Value, String)> {
            let input: C::Input = serde_json::from_value(params)
                .map_err(|e| CommandError::InvalidInput { id: C::ID.to_string(), message: e.to_string() })?;
            C::apply(doc, input)
        }
        assert!(self.entries.iter().all(|e| e.spec.id != C::ID), "duplicate command id {}", C::ID);
        let schema = serde_json::to_value(schemars::schema_for!(C::Input)).expect("schema serializes");
        self.entries.push(Entry {
            spec: CommandSpec { id: C::ID, title: C::TITLE, description: C::DESCRIPTION, risk: C::RISK, input_schema: schema },
            apply: apply::<C>,
        });
    }

    pub fn specs(&self) -> impl Iterator<Item = &CommandSpec> {
        self.entries.iter().map(|e| &e.spec)
    }

    pub fn spec(&self, id: &str) -> Option<&CommandSpec> {
        self.specs().find(|s| s.id == id)
    }

    fn entry(&self, id: &str) -> Result<&Entry> {
        self.entries.iter().find(|e| e.spec.id == id).ok_or_else(|| CommandError::Unknown(id.to_string()))
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invocation {
    pub id: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    pub result: Value,
    pub revision: u64,
}
