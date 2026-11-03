//! The job ledger's MCP tools (module `ledger`), listed after the core tools of `mcp`: search and read what was
//! burned, mark a run OK or scrap with a note, and open a burned job as a new document. None of them plans, requests
//! or approves a run: an agent that wants the job burned again calls `job_request_run` after `job_ledger_open`, and a
//! person approves it in the app.

use std::sync::Arc;

use base64::Engine as _;
use commands::Origin;
use rmcp::model::{CallToolResult, ContentBlock, JsonObject, Tool};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::ledger::{self, EntryView, LedgerQuery, Verdict};
use crate::{AppState, EngineError};

/// The ledger tools, in listing order (after `mcp::TOOL_NAMES`).
pub const TOOL_NAMES: [&str; 4] = ["job_ledger_search", "job_ledger_get", "job_ledger_mark", "job_ledger_open"];

/// Added to the server instructions.
pub const INSTRUCTIONS: &str = "Job ledger (what was burned): every run that ended is kept with each used layer's \
settings (power, speed, frequency, pulse width, passes, sub-layers, image mode), its share of the time, the serial \
number / CSV row and each variable text's value, the run options, the outcome and a person's verdict (ok or scrap) \
and note. job_ledger_search {q?, verdict?, state?, device?, project?, since_unix_s?, until_unix_s?, limit?} finds \
runs (newest first; q words must all match, e.g. \"blue coin\"; now_unix_s in the answer helps with \"last week\"); \
job_ledger_get {id, thumbnail?} gives one with full settings; job_ledger_mark {id, verdict, note?} records a verdict \
only when the person tells you how the part came out; job_ledger_open {id} opens the burned document as a new \
document (the person sees it). To burn it again, call job_request_run after opening it: the person approves the run \
in the app, as always.";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct IdArgs {
    /// Ledger entry id from job_ledger_search, e.g. "1730000000-run-3".
    id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GetArgs {
    /// Ledger entry id from job_ledger_search, e.g. "1730000000-run-3".
    id: String,
    /// Also return the run's thumbnail (PNG) when one is kept. Default false.
    #[serde(default)]
    thumbnail: bool,
}

/// `none` clears the verdict.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum VerdictArg {
    Ok,
    Scrap,
    None,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MarkArgs {
    /// Ledger entry id from job_ledger_search.
    id: String,
    /// How the part came out, as the person said: ok, scrap, or none to clear it.
    verdict: VerdictArg,
    /// A short note (at most 2000 characters), e.g. "colour too dark at the rim". Replaces the previous note; omit or
    /// leave empty for none.
    #[serde(default)]
    note: Option<String>,
}

fn tool<T: JsonSchema>(name: &'static str, title: &str, description: &'static str, read_only: bool) -> Tool {
    let mut v = serde_json::to_value(schemars::schema_for!(T)).unwrap_or_else(|_| json!({"type": "object"}));
    let obj = v.as_object_mut().map(|o| {
        o.remove("$schema");
        o.remove("title");
        o.entry("properties").or_insert_with(|| json!({}));
        o.clone()
    });
    Tool::new(name, description, Arc::new(obj.unwrap_or_default()))
        .with_title(title)
        .with_annotations(rmcp::model::ToolAnnotations::with_title(title).read_only(read_only))
}

pub fn tools() -> Vec<Tool> {
    vec![
        tool::<LedgerQuery>(
            "job_ledger_search",
            "Search the job ledger",
            "Runs that ended, newest first, with what was burned: {now_unix_s, count, entries: [{id, time_unix_s, \
             state, origin, device, project, document, summary, duration_s, completed, error, verdict, note, \
             layers: [{name, color, output, settings: {power_pct, speed_mm_s, frequency_khz, pulse_width_ns, passes, \
             mode, hatch, image}, sub_layers, passes, estimated_time_s}], variables: {current, csv_file, texts: \
             [{name, mode, value}]}, options, estimated_time_s, thumbnail, rerun}]}. Filters: q (words that must all \
             appear in project, document, device, summary, layer names and settings, variable values, note or error), \
             verdict (ok, scrap, unmarked), state (finished, failed, aborted, cancelled), device, project, \
             since_unix_s / until_unix_s, limit (default 20 here, at most 200).",
            true,
        ),
        tool::<GetArgs>(
            "job_ledger_get",
            "Read a ledger entry",
            "One run of the job ledger with every layer's full settings as burned (doc CutSettings JSON), the \
             variable text values, run options, verdict and note; with thumbnail: true also its PNG thumbnail.",
            true,
        ),
        tool::<MarkArgs>(
            "job_ledger_mark",
            "Mark a run ok or scrap",
            "Record how a burned part came out: verdict ok, scrap or none (clears), with an optional note. Only when \
             the person told you; recorded as an agent's note. Returns the entry.",
            false,
        ),
        tool::<IdArgs>(
            "job_ledger_open",
            "Open a burned job",
            "Open the document a run burned (kept for recent runs: rerun true in the entry) as a NEW open document and \
             make it active, exactly as burned (same layers, settings and serial number / CSV row). Nothing is \
             planned or started; call job_request_run next to prepare a pending run, which a person approves in the \
             app. device_mismatch in the answer warns when another device is active than the one it burned on. \
             Returns {ok, id, document_id, revision, device_mismatch?}.",
            false,
        ),
    ]
}

fn ok_json(v: Value) -> CallToolResult {
    let text = serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string());
    CallToolResult::success(vec![ContentBlock::text(text)])
}

fn args<T: serde::de::DeserializeOwned>(tool: &str, a: JsonObject) -> Result<T, EngineError> {
    serde_json::from_value(Value::Object(a)).map_err(|e| EngineError::BadRequest(format!("bad arguments for {tool}: {e}")))
}

fn origin() -> Origin {
    Origin::Agent(crate::mcp::MCP_AGENT.into())
}

/// An entry for an agent: the stored note's author and the thumbnail flag kept, run ids and settings as stored.
fn entry_json(v: &EntryView) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

/// Run ledger tool `name` (one of [`TOOL_NAMES`]).
pub async fn dispatch(st: &Arc<AppState>, name: &str, a: JsonObject) -> Result<CallToolResult, EngineError> {
    match name {
        "job_ledger_search" => {
            let mut q: LedgerQuery = args(name, a)?;
            q.limit = Some(q.limit.unwrap_or(20).clamp(1, 200));
            let st2 = st.clone();
            let entries = tokio::task::spawn_blocking(move || ledger::search(&st2, &q)).await??;
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            Ok(ok_json(json!({
                "now_unix_s": now,
                "count": entries.len(),
                "entries": entries.iter().map(entry_json).collect::<Vec<_>>(),
            })))
        }
        "job_ledger_get" => {
            let x: GetArgs = args(name, a)?;
            let st2 = st.clone();
            let id = x.id.clone();
            let v = tokio::task::spawn_blocking(move || ledger::get(&st2, &id)).await??;
            let mut content = vec![ContentBlock::text(serde_json::to_string_pretty(&entry_json(&v)).unwrap_or_default())];
            if x.thumbnail
                && let Some(png) = st.libs.job_ledger(|l| l.thumbnail(&x.id))
            {
                content.push(ContentBlock::image(base64::engine::general_purpose::STANDARD.encode(png), "image/png"));
            }
            Ok(CallToolResult::success(content))
        }
        "job_ledger_mark" => {
            let x: MarkArgs = args(name, a)?;
            let verdict = match x.verdict {
                VerdictArg::Ok => Some(Verdict::Ok),
                VerdictArg::Scrap => Some(Verdict::Scrap),
                VerdictArg::None => None,
            };
            let st2 = st.clone();
            let v = tokio::task::spawn_blocking(move || {
                ledger::mark(&st2, &x.id, verdict, x.note.as_deref().unwrap_or_default(), &origin())
            })
            .await??;
            Ok(ok_json(entry_json(&v)))
        }
        "job_ledger_open" => {
            let x: IdArgs = args(name, a)?;
            Ok(ok_json(ledger::open_document(st, &x.id, origin()).await?))
        }
        other => Err(EngineError::NotFound(format!("unknown tool '{other}'"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_tools_cannot_approve_and_never_arm() {
        let names: Vec<String> = tools().iter().map(|t| t.name.to_string()).collect();
        assert_eq!(names, TOOL_NAMES.map(String::from).to_vec());
        for name in TOOL_NAMES {
            for word in ["approve", "pin", "arm", "input", "fire", "start", "run"] {
                assert!(!name.contains(word), "{name}");
            }
        }
        let src = include_str!("ledger_mcp.rs");
        let body = src.split("#[cfg(test)]").next().unwrap_or_default();
        for forbidden in ["Armed", "approved_in_ui", "approve::", "Controller", "pin::", "request_run_with", "ledger::rerun"] {
            assert!(!body.contains(forbidden), "ledger_mcp.rs must not reference {forbidden}");
        }
        for t in tools() {
            assert_eq!(t.input_schema.get("type"), Some(&json!("object")), "{}", t.name);
        }
    }
}
