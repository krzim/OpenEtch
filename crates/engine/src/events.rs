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
    /// The document changed (edit, undo or redo). `origin` is who made the change (an undo/redo from the UI is
    /// `ui`, from MCP `agent`).
    DocumentChanged { revision: u64, origin: Origin },
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
    ws.on_upgrade(move |socket| pump(socket, rx))
}

async fn pump(mut socket: WebSocket, mut rx: broadcast::Receiver<Event>) {
    loop {
        tokio::select! {
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
        let ev = Event::DocumentChanged { revision: 3, origin: Origin::Ui };
        assert_eq!(
            serde_json::to_value(&ev).unwrap(),
            serde_json::json!({"type": "document_changed", "revision": 3, "origin": {"kind": "ui"}})
        );
        let ev = Event::RunFinished { id: "run-1".into(), ok: true, state: RunState::Finished, error: None };
        assert_eq!(
            serde_json::to_value(&ev).unwrap(),
            serde_json::json!({"type": "run_finished", "id": "run-1", "ok": true, "state": "finished", "error": null})
        );
        let ev = Event::RunPending { run_id: "run-1".into(), origin: Origin::Agent("mcp".into()), summary: "s".into() };
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "run_pending");
        assert_eq!(v["origin"]["kind"], "agent");
    }
}
