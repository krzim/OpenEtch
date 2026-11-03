//! Server -> client events (`WS /api/events`), fanned out over a tokio broadcast channel.

use std::sync::Arc;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use tokio::sync::broadcast;

use crate::AppState;

/// Capacity of the event channel. A slow client that falls further behind skips events (and refetches).
pub const EVENT_CAPACITY: usize = 256;

pub use appcore::event::Event;

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
    use crate::runs::RunState;
    use commands::Origin;

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
