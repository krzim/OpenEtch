//! Server -> client events (`WS /api/events`), fanned out over a tokio broadcast channel.

use std::sync::Arc;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use commands::Origin;
use serde::Serialize;
use tokio::sync::broadcast;

use crate::AppState;
use crate::runs::RunState;

/// Capacity of the event channel. A slow client that falls further behind skips events (and refetches).
pub const EVENT_CAPACITY: usize = 256;

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
    /// is twice as long (see the `pin` module).
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

/// A new event channel.
pub fn channel() -> broadcast::Sender<Event> {
    broadcast::channel(EVENT_CAPACITY).0
}

/// `GET /api/events`: upgrade to a WebSocket and stream every event as JSON text.
pub async fn events_ws(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    let rx = state.subscribe();
    let closing = state.shutdown.closing();
    ws.on_upgrade(move |socket| pump(socket, rx, closing))
}

/// Resolves once the engine has shut down (never, should the state be gone).
async fn shut_down(closing: &mut tokio::sync::watch::Receiver<bool>) {
    if closing.wait_for(|c| *c).await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// Stream events until the client goes away or the engine has shut down (module `shutdown`).
async fn pump(mut socket: WebSocket, mut rx: broadcast::Receiver<Event>, mut closing: tokio::sync::watch::Receiver<bool>) {
    loop {
        tokio::select! {
            () = shut_down(&mut closing) => {
                // Send what is still queued (the `shutting_down` event), then close.
                while let Ok(ev) = rx.try_recv() {
                    let Ok(text) = serde_json::to_string(&ev) else { continue };
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                let _ = socket.send(Message::Close(None)).await;
                break;
            }
            ev = rx.recv() => match ev {
                Ok(ev) => {
                    let Ok(text) = serde_json::to_string(&ev) else { continue };
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                // Missed events: tell the client to refetch everything.
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let text =
                        serde_json::json!({ "type": "document_changed", "revision": -1, "origin": null }).to_string();
                    let _ = socket.send(Message::Text(text.into())).await;
                    let _ = socket.send(Message::Text(r#"{"type":"machine","event":"lagged"}"#.into())).await;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            msg = socket.recv() => match msg {
                // Clients do not send anything meaningful; keep reading so pings and close frames are handled.
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_serialise_with_type_tag() {
        let ev = Event::DocumentChanged { revision: 3, origin: Origin::Ui, document_id: "default".into() };
        assert_eq!(
            serde_json::to_value(&ev).unwrap(),
            serde_json::json!({"type": "document_changed", "revision": 3, "origin": {"kind": "ui"}, "document_id": "default"})
        );
        assert_eq!(
            serde_json::to_value(Event::DocumentsChanged { active: "doc-2".into() }).unwrap(),
            serde_json::json!({"type": "documents_changed", "active": "doc-2"})
        );
        let ev = Event::RunFinished { id: "run-1".into(), ok: true, state: RunState::Finished, error: None };
        assert_eq!(
            serde_json::to_value(&ev).unwrap(),
            serde_json::json!({"type": "run_finished", "id": "run-1", "ok": true, "state": "finished", "error": null})
        );
        assert_eq!(
            serde_json::to_value(Event::ApprovalLocked { until_unix_s: 1_800_000_000 }).unwrap(),
            serde_json::json!({"type": "approval_locked", "until_unix_s": 1_800_000_000u64})
        );
        assert_eq!(
            serde_json::to_value(Event::ProjectSaved { name: "Coaster".into(), revision: 7, document_id: "default".into() }).unwrap(),
            serde_json::json!({"type": "project_saved", "name": "Coaster", "revision": 7, "document_id": "default"})
        );
        assert_eq!(
            serde_json::to_value(Event::HistoryChanged { marker: 2, steps: 5, revision: 9, document_id: "default".into() }).unwrap(),
            serde_json::json!({"type": "history_changed", "marker": 2, "steps": 5, "revision": 9, "document_id": "default"})
        );
        assert_eq!(
            serde_json::to_value(Event::DeviceChanged { id: "jczfiber".into() }).unwrap(),
            serde_json::json!({"type": "device_changed", "id": "jczfiber"})
        );
        assert_eq!(serde_json::to_value(Event::DevicesChanged).unwrap(), serde_json::json!({"type": "devices_changed"}));
        let ev = Event::RunPending { run_id: "run-1".into(), origin: Origin::Agent("mcp".into()), summary: "s".into() };
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "run_pending");
        assert_eq!(v["origin"]["kind"], "agent");
    }
}
