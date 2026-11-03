//! Server -> client events (`WS /api/events` on the engine, `{kind: "event", data}` worker messages in the browser
//! build). The engine fans them out over its broadcast channel; webcore hands them to the worker shell after each
//! request.

use commands::Origin;
use serde::Serialize;

/// Where a run is in its life: `pending` -> `approved` -> (`awaiting_confirm` ->) `running` -> `finished` |
/// `failed` | `aborted` (stopped while streaming), or `cancelled` (stopped or refused before streaming).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// Waiting for a person to approve it (the operator PIN in the UI, or Start in the app without a PIN).
    Pending,
    /// Approved; about to start (or about to wait for the physical input).
    Approved,
    /// Approved; waiting for the physical confirmation input (see the engine's `pin::PhysicalConfirm`).
    AwaitingConfirm,
    /// Being streamed to the board.
    Running,
    /// Ran to the end.
    Finished,
    /// The board or link failed; the abort sequence was sent.
    Failed,
    /// Stopped by a stop request while `running` (streaming); the abort sequence was sent.
    Aborted,
    /// Never streamed: stop pressed while pending, approved or awaiting confirmation, the document changed, or the
    /// physical input was not pressed in time.
    Cancelled,
}

impl RunState {
    /// True once the run can no longer change.
    pub fn is_final(self) -> bool {
        matches!(self, RunState::Finished | RunState::Failed | RunState::Aborted | RunState::Cancelled)
    }
}

/// One event, serialised as `{"type": "...", ...}`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// The document changed (edit, undo or redo, or another open document became the active one). `origin` is who
    /// made the change (an undo/redo from the UI is `ui`, from MCP `agent`); `document_id` the open document (see
    /// `GET /api/documents`; `default` for the first one).
    DocumentChanged { revision: u64, origin: Origin, document_id: String },
    /// Machine status changed: `event` is "framing", "frame_stopped", "run_approved", "awaiting_confirm",
    /// "run_started", "run_finished", "stopped", "error" (or "lagged"); clients refetch `GET /api/machine`.
    Machine {
        event: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// A run is waiting for approval in the UI.
    RunPending { run_id: String, origin: Origin, summary: String },
    /// An approved run ended: `state` is `finished`, `failed`, `aborted` or `cancelled`; `error` is `null` when
    /// `ok`.
    RunFinished { id: String, ok: bool, state: RunState, error: Option<String> },
    /// Too many wrong operator PINs: approvals are locked until `until_unix_s` (Unix seconds). Each further lockout
    /// is twice as long (see the engine's `pin` module).
    ApprovalLocked { until_unix_s: u64 },
    /// The document `document_id` was saved as project `name` at `revision` (clients refetch `GET /api/project`).
    ProjectSaved { name: String, revision: u64, document_id: String },
    /// The timeline changed (a step was recorded, deleted or replayed, or the marker moved by undo, redo or goto):
    /// `marker` is the index of the last applied step (0 = none), `steps` how many there are. Clients refetch
    /// `GET /api/history`.
    HistoryChanged { marker: usize, steps: usize, revision: u64, document_id: String },
    /// A document was opened, closed or made active (`POST /api/documents`, `.../{id}/activate`, `.../{id}/close`):
    /// `active` is the active document's id. Clients refetch `GET /api/documents`; when `active` changed, everything
    /// about the document (the engine also sends `document_changed` and `history_changed` for it).
    DocumentsChanged { active: String },
    /// Device `id` was made active (`POST /api/devices/{id}/activate`): the machine profile and workspace changed.
    /// Clients refetch `GET /api/devices` and `GET /api/machine`.
    DeviceChanged { id: String },
    /// Devices were imported or removed; clients refetch `GET /api/devices`.
    DevicesChanged,
    /// An Art Library changed (items added, renamed, removed) or, with `library: null`, the list of libraries did;
    /// clients refetch `GET /api/art/libraries` (and the library).
    ArtLibraryChanged { library: Option<String> },
    /// While a job holds the machine, every 500 ms: `status` is `GET /api/job`'s answer (iteration, packets sent,
    /// paused, time remaining, count).
    JobProgress { status: serde_json::Value },
}

impl Event {
    /// A [`Event::Machine`] event.
    pub fn machine(event: &str, detail: Option<String>) -> Self {
        Event::Machine { event: event.to_owned(), detail }
    }
}

/// The two events every change of the active document's timeline sends: `document_changed` when the revision moved,
/// then `history_changed` (always).
pub fn announce(before: u64, after: u64, origin: Origin, (marker, steps): (usize, usize), document_id: String) -> Vec<Event> {
    let mut out = Vec::with_capacity(2);
    if after != before {
        out.push(Event::DocumentChanged { revision: after, origin, document_id: document_id.clone() });
    }
    out.push(Event::HistoryChanged { marker, steps, revision: after, document_id });
    out
}
