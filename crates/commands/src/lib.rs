//! The command registry. Every change to a [`Document`] goes through a registered command, whoever issues it: the
//! web UI, a keyboard shortcut, a test, or an AI agent over MCP. Each command has a stable dotted id, a JSON Schema for
//! its input (from `schemars`), a risk class, and a description that doubles as model-facing documentation.
//!
//! [`Session`] owns the document, runs commands, keeps undo/redo (whole-document snapshots; documents are small), and
//! records an origin-tagged history so the UI can show what the AI did.

use doc::Document;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod builtin;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub revision: u64,
    pub command: String,
    pub origin: Origin,
    pub summary: String,
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

/// A live editing session: the document, undo/redo, and history.
pub struct Session {
    registry: Registry,
    doc: Document,
    undo: Vec<Document>,
    redo: Vec<Document>,
    history: Vec<HistoryEntry>,
}

impl Session {
    pub fn new(doc: Document) -> Self {
        Session { registry: Registry::new(), doc, undo: Vec::new(), redo: Vec::new(), history: Vec::new() }
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    pub fn document(&self) -> &Document {
        &self.doc
    }

    pub fn history(&self) -> &[HistoryEntry] {
        &self.history
    }

    /// Run one command. `Query` commands do not create an undo step or bump the revision.
    pub fn execute(&mut self, inv: Invocation, origin: Origin) -> Result<Outcome> {
        self.batch(vec![inv], origin).map(|mut v| v.remove(0))
    }

    /// Run several commands as one undo step. All-or-nothing: on the first error the document is restored.
    pub fn batch(&mut self, invs: Vec<Invocation>, origin: Origin) -> Result<Vec<Outcome>> {
        let before = self.doc.clone();
        let mut outcomes = Vec::with_capacity(invs.len());
        let mut summaries = Vec::new();
        let mut edited = false;
        for inv in invs {
            let entry = match self.registry.entry(&inv.id) {
                Ok(e) => e,
                Err(e) => {
                    self.doc = before;
                    return Err(e);
                }
            };
            let params = if inv.params.is_null() { Value::Object(Default::default()) } else { inv.params };
            match (entry.apply)(&mut self.doc, params) {
                Ok((result, summary)) => {
                    if entry.spec.risk != Risk::Query {
                        edited = true;
                        summaries.push((inv.id.clone(), summary));
                    }
                    outcomes.push(Outcome { result, revision: self.doc.revision });
                }
                Err(e) => {
                    self.doc = before;
                    return Err(e);
                }
            }
        }
        if edited {
            // Backstop for every command: JSON cannot store NaN or infinity and planning cannot use them, so a
            // document holding one (e.g. from transforms that overflow) is never committed.
            if let Some(problem) = self.doc.numeric_problem() {
                self.doc = before;
                return Err(CommandError::Failed(format!(
                    "refused: the result would contain unusable geometry ({problem}); nothing was changed"
                )));
            }
            self.doc.revision = before.revision + 1;
            self.undo.push(before);
            self.redo.clear();
            let command = summaries.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>().join(", ");
            let summary = summaries.iter().map(|(_, s)| s.as_str()).collect::<Vec<_>>().join("; ");
            self.history.push(HistoryEntry { revision: self.doc.revision, command, origin, summary });
            for o in &mut outcomes {
                o.revision = self.doc.revision;
            }
        }
        Ok(outcomes)
    }

    pub fn undo(&mut self) -> Result<u64> {
        let prev = self.undo.pop().ok_or(CommandError::NothingTo("undo"))?;
        let rev = self.doc.revision + 1;
        self.redo.push(std::mem::replace(&mut self.doc, prev));
        self.doc.revision = rev;
        Ok(rev)
    }

    pub fn redo(&mut self) -> Result<u64> {
        let next = self.redo.pop().ok_or(CommandError::NothingTo("redo"))?;
        let rev = self.doc.revision + 1;
        self.undo.push(std::mem::replace(&mut self.doc, next));
        self.doc.revision = rev;
        Ok(rev)
    }
}
