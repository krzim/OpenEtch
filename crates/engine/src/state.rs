//! [`AppState`]: the live session, the machine, pending runs and the event channel, with the operations both the
//! HTTP API and the MCP server call. Locks are std mutexes held only for short, non-awaiting sections; planning
//! and rendering run on blocking threads.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use commands::{CommandSpec, HistoryEntry, Invocation, Origin, Outcome, Risk, Session};
use doc::Document;
use plan::{PlanError, Preview};
use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::frame::{self, FrameRequest};
use crate::hw::Machine;
use crate::render::{self, RenderOptions};
use crate::runs::{self, RunStore, RunView};
use crate::security::SecurityConfig;
use crate::{EngineError, Event};

/// The message every pending-run answer carries.
pub const APPROVAL_MESSAGE: &str = "The run is pending. Nothing fires until a person reviews and approves it in the \
    galvo-studio UI (Laser workspace). Agents cannot approve runs.";

/// Shared engine state. See the module docs.
pub struct AppState {
    session: Mutex<Session>,
    pub machine: Arc<Machine>,
    runs: Mutex<RunStore>,
    events: broadcast::Sender<Event>,
    pub security: SecurityConfig,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Parse a request's `origin`: `"ui"`, `"script"`, `"agent"` / `"agent:<name>"`, or the serde form of
/// [`Origin`] (`{"kind": "agent", "name": "..."}`). Missing means [`Origin::Ui`].
pub fn parse_origin(v: Option<&Value>) -> Result<Origin, EngineError> {
    match v {
        None | Some(Value::Null) => Ok(Origin::Ui),
        Some(Value::String(s)) => match s.as_str() {
            "ui" => Ok(Origin::Ui),
            "script" => Ok(Origin::Script),
            "agent" => Ok(Origin::Agent("agent".into())),
            other => match other.strip_prefix("agent:") {
                Some(name) if !name.is_empty() => Ok(Origin::Agent(name.to_owned())),
                _ => Err(EngineError::BadRequest(format!(
                    "unknown origin '{other}' (expected \"ui\", \"script\" or \"agent:<name>\")"
                ))),
            },
        },
        Some(obj) => {
            serde_json::from_value(obj.clone()).map_err(|e| EngineError::BadRequest(format!("bad origin: {e}")))
        }
    }
}

impl AppState {
    pub fn new(session: Session, machine: Arc<Machine>, security: SecurityConfig) -> Arc<Self> {
        Arc::new(AppState {
            session: Mutex::new(session),
            machine,
            runs: Mutex::new(RunStore::default()),
            events: crate::events::channel(),
            security,
        })
    }

    /// A receiver for every future event.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    /// Send an event (dropped when nobody listens).
    pub fn emit(&self, ev: Event) {
        let _ = self.events.send(ev);
    }

    pub(crate) fn runs(&self) -> MutexGuard<'_, RunStore> {
        lock(&self.runs)
    }

    /// Every registered document command.
    pub fn specs(&self) -> Vec<CommandSpec> {
        lock(&self.session).registry().specs().cloned().collect()
    }

    /// A copy of the current document.
    pub fn document(&self) -> Document {
        lock(&self.session).document().clone()
    }

    pub fn revision(&self) -> u64 {
        lock(&self.session).document().revision
    }

    pub fn history(&self) -> Vec<HistoryEntry> {
        lock(&self.session).history().to_vec()
    }

    /// Run one command (see [`AppState::batch`]).
    pub fn execute(&self, inv: Invocation, origin: Origin) -> Result<Outcome, EngineError> {
        self.batch(vec![inv], origin).map(|mut v| v.remove(0))
    }

    /// Run commands as one undo step, all-or-nothing. Only `Query` and `Edit` commands are accepted here: machine
    /// actions have their own endpoints and tools.
    pub fn batch(&self, invs: Vec<Invocation>, origin: Origin) -> Result<Vec<Outcome>, EngineError> {
        if invs.is_empty() {
            return Err(EngineError::BadRequest("batch has no commands".into()));
        }
        let (outcomes, before, after) = {
            let mut s = lock(&self.session);
            for inv in &invs {
                if let Some(spec) = s.registry().spec(&inv.id)
                    && !matches!(spec.risk, Risk::Query | Risk::Edit)
                {
                    return Err(EngineError::Forbidden(format!(
                        "'{}' is a machine action; use the machine endpoints/tools instead",
                        inv.id
                    )));
                }
            }
            let before = s.document().revision;
            let outcomes = s.batch(invs, origin.clone())?;
            (outcomes, before, s.document().revision)
        };
        if after != before {
            self.emit(Event::DocumentChanged { revision: after, origin: Some(origin) });
        }
        Ok(outcomes)
    }

    /// Undo the last edit; returns the new revision.
    pub fn undo(&self) -> Result<u64, EngineError> {
        let rev = lock(&self.session).undo()?;
        self.emit(Event::DocumentChanged { revision: rev, origin: None });
        Ok(rev)
    }

    /// Redo the last undone edit; returns the new revision.
    pub fn redo(&self) -> Result<u64, EngineError> {
        let rev = lock(&self.session).redo()?;
        self.emit(Event::DocumentChanged { revision: rev, origin: None });
        Ok(rev)
    }

    /// Plan the current document (no encoding). An empty document previews as nothing rather than an error.
    pub async fn preview(&self) -> Result<(Document, Result<Preview, PlanError>), EngineError> {
        let doc = self.document();
        let target = self.machine.target().clone();
        Ok(tokio::task::spawn_blocking(move || {
            let preview = match plan::plan_job(&doc, &target) {
                Ok(job) => Ok(plan::preview_plan(&job)),
                Err(PlanError::Empty) => Ok(Preview::default()),
                Err(e) => Err(e),
            };
            (doc, preview)
        })
        .await?)
    }

    /// The preview as SVG. A plan error still renders the document (without toolpath); the error is returned
    /// alongside.
    pub async fn preview_svg(&self, opts: RenderOptions) -> Result<(String, Result<Preview, PlanError>), EngineError> {
        let (doc, preview) = self.preview().await?;
        let field = self.machine.profile().field_mm;
        let svg = render::preview_svg(&doc, field, preview.as_ref().ok(), opts);
        Ok((svg, preview))
    }

    /// Plan, encode and check the current document and store it as a pending run. Never touches the machine.
    pub async fn request_run(&self, origin: Origin) -> Result<RunView, EngineError> {
        let doc = self.document();
        let revision = doc.revision;
        let target = self.machine.target().clone();
        let profile = self.machine.profile().clone();
        let prepared = tokio::task::spawn_blocking(move || runs::prepare_job(&doc, &target, &profile)).await??;
        let view = {
            let mut store = self.runs();
            let run = store.add(origin.clone(), revision, prepared)?;
            run.view(revision)
        };
        tracing::info!(run_id = %view.id, ?origin, "run requested; waiting for approval in the UI");
        self.emit(Event::RunPending { run_id: view.id.clone(), origin, summary: view.summary.clone() });
        Ok(view)
    }

    /// `GET /api/machine`: machine status plus pending and recent runs.
    pub fn machine_status(&self) -> Value {
        let rev = self.revision();
        let mut v = self.machine.status_json();
        let store = self.runs();
        v["pending_runs"] = json!(store.pending(rev));
        v["runs"] = json!(store.views(rev));
        v
    }

    /// Start red-pointer framing of the selection (or the whole job). Light only.
    pub async fn frame_start(&self, req: FrameRequest) -> Result<Value, EngineError> {
        let outline = frame::outline_for(&self.document(), &req)?;
        frame::start(&self.machine, &self.events, outline).await
    }

    /// Stop framing.
    pub async fn frame_stop(&self) -> Result<(), EngineError> {
        frame::stop(&self.machine, &self.events).await
    }

    /// Stop everything: abort any run or frame and cancel pending runs. Never gated.
    pub async fn stop(&self) -> Result<Value, EngineError> {
        let cancelled = self.runs().cancel_pending("cancelled by stop");
        let result = self.machine.abort().await;
        self.emit(Event::machine("stopped", result.as_ref().err().map(ToString::to_string)));
        match result {
            Ok(()) => Ok(json!({ "ok": true, "cancelled_runs": cancelled })),
            Err(e) => {
                self.machine.set_error(Some(e.to_string()));
                Err(EngineError::Internal(format!(
                    "the abort sequence failed ({e}); use the hardware E-stop or key switch"
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_parse() {
        assert_eq!(parse_origin(None).unwrap(), Origin::Ui);
        assert_eq!(parse_origin(Some(&json!("ui"))).unwrap(), Origin::Ui);
        assert_eq!(parse_origin(Some(&json!("agent:claude"))).unwrap(), Origin::Agent("claude".into()));
        assert_eq!(parse_origin(Some(&json!({"kind": "script"}))).unwrap(), Origin::Script);
        assert!(parse_origin(Some(&json!("root"))).is_err());
    }
}
