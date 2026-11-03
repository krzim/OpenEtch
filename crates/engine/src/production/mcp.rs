//! The production MCP tools (listed after the core tools of `crate::mcp`). All but one only read; none can approve,
//! start, continue or switch anything: agents prepare production with document commands (`layer.update` with
//! `settings.options.operator_pause`, `var.set_serial` with `stop_at_end` / `empty_slots`, `var.slots`), watch it
//! here, and a person runs it at the machine. `serial_remark_request` creates a PENDING run like `job_request_run`.

use std::sync::Arc;
use std::time::Duration;

use commands::Origin;
use rmcp::model::{CallToolResult, JsonObject, Tool, ToolAnnotations};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::notify;
use crate::{AppState, EngineError};

/// The production tools, in listing order.
pub const TOOL_NAMES: [&str; 5] = ["production_status", "production_events", "serial_ledger", "hour_meter", "serial_remark_request"];

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct NoArgs {}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct EventsArgs {
    /// Return only events after this sequence number (the `last_seq` of the previous call; 0 or absent: the whole
    /// feed, the newest 200 at most).
    #[serde(default)]
    after: Option<u64>,
    /// Wait up to this many seconds (at most 25) for a new event when there is none yet: call again with the
    /// returned last_seq to follow the feed like a subscription.
    #[serde(default)]
    wait_s: Option<u64>,
    /// Most events returned (default 50).
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct LedgerArgs {
    /// Newest entries to read (default 50, at most 10000).
    #[serde(default)]
    limit: Option<usize>,
    /// Only this run (and its re-marks).
    #[serde(default)]
    run_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RemarkArgs {
    /// The run in the serial ledger (serial_ledger's run_id).
    run_id: String,
    /// Its entry's time_unix_s (run ids start again after an engine restart). Default: the newest entry of run_id.
    #[serde(default)]
    time_unix_s: Option<u64>,
    /// The slot number (1-based) to mark again with the same value.
    slot: usize,
}

fn tool(name: &'static str, title: &str, description: &'static str, read_only: bool, schema: Arc<JsonObject>) -> Tool {
    Tool::new(name, description, schema).with_title(title).with_annotations(ToolAnnotations::with_title(title).read_only(read_only))
}

/// The tool definitions.
pub(crate) fn tools() -> Vec<Tool> {
    use crate::mcp::schema;
    vec![
        tool(
            "production_status",
            "Production status",
            "The production cockpit's view, read-only: the job (progress, count, Stop At), the foot switch cycle, the \
             serial slots and the values the next run marks (var.slots), a run waiting at a layer pause, burn and cycle \
             times with the time left to Stop At, maintenance reminders and the hour meter, the extractor, the last \
             serial ledger entries and whether the cockpit has the design locked (edits are then refused).",
            true,
            schema::<NoArgs>(),
        ),
        tool(
            "production_events",
            "Production events",
            "The production event feed (read-only; the same events the notification targets get): job_done, \
             waiting_for_foot_switch, door, error, stop_at_reached, layer_pause, maintenance_due, each with operation \
             (frame or mark), message, device, run_id, count, stop_at. Subscribe by calling it again with after = the \
             returned last_seq and wait_s (up to 25) to wait for the next event. Nothing here can start, continue or \
             approve a burn.",
            true,
            schema::<EventsArgs>(),
        ),
        tool(
            "serial_ledger",
            "Serial ledger",
            "Which serial number or CSV row was marked on which slot (pocket) in which run: {entries: [{time_unix_s, \
             run_id, state, device, project, document, completed, counter, slots: [{number, offset, label, value, values: \
             [{object, mode, shown}], marked, skipped}], remark_of}]}, oldest first.",
            true,
            schema::<LedgerArgs>(),
        ),
        tool(
            "hour_meter",
            "Hour meter",
            "The active device's marking hours (board-measured and estimated), runs, and maintenance reminders with what \
             is due. Read-only: only a person changes or completes reminders, in the app.",
            true,
            schema::<NoArgs>(),
        ),
        tool(
            "serial_remark_request",
            "Request a slot re-mark",
            "Create a PENDING run that marks slot `slot` of ledger run `run_id` again with the same value (that slot's \
             pocket only, the current design). It does not fire the laser: a person approves it in the OpenEtch UI, and \
             the serial counter does not move after it.",
            false,
            schema::<RemarkArgs>(),
        ),
    ]
}

fn args<T: serde::de::DeserializeOwned>(tool: &str, a: JsonObject) -> Result<T, EngineError> {
    serde_json::from_value(serde_json::Value::Object(a)).map_err(|e| EngineError::BadRequest(format!("bad arguments for {tool}: {e}")))
}

/// Run production tool `name`; `None` when it is not one.
pub(crate) async fn dispatch(st: &Arc<AppState>, name: &str, a: JsonObject) -> Option<Result<CallToolResult, EngineError>> {
    let ok = crate::mcp::ok_json;
    Some(match name {
        "production_status" => args::<NoArgs>(name, a).map(|_| ok(super::status_json(st))),
        "production_events" => match args::<EventsArgs>(name, a) {
            Ok(x) => {
                let wait = Duration::from_secs(x.wait_s.unwrap_or(0).min(notify::MAX_WAIT_S));
                let limit = x.limit.unwrap_or(50).min(notify::FEED_LEN);
                let (events, last) = st.production.notify.feed_wait(x.after.unwrap_or(0), limit, wait).await;
                Ok(ok(json!({ "events": events, "last_seq": last })))
            }
            Err(e) => Err(e),
        },
        "serial_ledger" => match args::<LedgerArgs>(name, a) {
            Ok(x) => st.production.ledger.read(x.limit.unwrap_or(50)).map(|mut entries| {
                if let Some(run) = &x.run_id {
                    entries.retain(|e| &e.run_id == run || e.remark_of.as_ref().is_some_and(|r| &r.run_id == run));
                }
                ok(json!({ "entries": entries }))
            }),
            Err(e) => Err(e),
        },
        "hour_meter" => args::<NoArgs>(name, a).map(|_| ok(super::meter::status_json(st, &st.active_device().id))),
        "serial_remark_request" => match args::<RemarkArgs>(name, a) {
            Ok(x) => super::remark::request(st, &x.run_id, x.time_unix_s, x.slot, Origin::Agent(crate::mcp::MCP_AGENT.into()), None).await.map(ok),
            Err(e) => Err(e),
        },
        _ => return None,
    })
}
