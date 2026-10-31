//! The MCP server (rmcp, Streamable HTTP at `/mcp`). Document tools are generated from the same command registry
//! the UI uses; machine tools are separate so clients can approve them separately.
//!
//! The machine tools can frame (red pointer) and stop, and `job_request_run` creates a pending run. None of them
//! can fire the laser: this module never creates the machine crate's arming token (only the UI approval endpoint
//! does), and a test checks that its source does not mention it.

use std::sync::Arc;

use base64::Engine as _;
use commands::{Invocation, Origin};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation, JsonObject, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use schemars::JsonSchema;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

use crate::frame::FrameRequest;
use crate::render::{self, RenderOptions};
use crate::state::APPROVAL_MESSAGE;
use crate::{AppState, EngineError};

/// The agent name MCP edits are recorded under in the history.
pub const MCP_AGENT: &str = "mcp";

/// Every tool, in listing order.
pub const TOOL_NAMES: [&str; 12] = [
    "list_commands",
    "execute",
    "batch",
    "inspect_document",
    "render_preview",
    "undo",
    "redo",
    "machine_status",
    "frame_start",
    "frame_stop",
    "job_request_run",
    "machine_stop",
];

/// Server instructions shown to the model.
pub const INSTRUCTIONS: &str = "galvo-studio controls a fiber galvo laser (ComMarker B6 MOPA 60W) and its live design \
document, shared with people using the web UI.

Coordinates: everything is millimetres and degrees in the FIELD FRAME: origin at the centre of the galvo field, +X to \
the operator's right, +Y away from the operator (toward the machine column). The field is a square (see \
machine_status.field_mm, about 153.75 mm); every object must lie strictly inside it.

Workflow:
1. stock.define - describe the physical piece (shape, size, thickness, where it sits in the field).
2. Import or draw art: art.import_svg, shape.rect / shape.ellipse / shape.path.
3. art.place - fit and anchor the art on the stock (margins, alignment).
4. layer.update - set the layer's cut settings (power %, speed, frequency, MOPA pulse width, fill/hatch, passes).
5. render_preview - look at the result and the toolpath; inspect_document for exact numbers and warnings.
6. frame_start - trace the job outline with the red pointer so the person can check placement (no laser), then \
frame_stop.
7. job_request_run - create a pending run.

Firing the laser always requires a person to approve the pending run in the galvo-studio UI. You cannot approve or \
start a run. machine_stop is always available and stops everything.

Use list_commands for every command's input schema; execute runs one command, batch runs several as one undo step \
(all-or-nothing). Edits you make are tagged as agent edits in the history and can be undone.";

/// The MCP server handler. Cheap to clone; one per MCP session.
#[derive(Clone)]
pub struct GalvoMcp {
    state: Arc<AppState>,
}

/// The `/mcp` tower service.
pub fn service(state: Arc<AppState>) -> StreamableHttpService<GalvoMcp, LocalSessionManager> {
    // Host/Origin checks happen in `security::guard` for the whole router (it also accepts LAN IP literals and
    // `--allow-host` names, which rmcp's list cannot express), so rmcp's own Host check is turned off.
    let config = StreamableHttpServerConfig::default().disable_allowed_hosts();
    StreamableHttpService::new(move || Ok(GalvoMcp::new(state.clone())), Default::default(), config)
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExecuteArgs {
    /// Command id from list_commands, e.g. "shape.rect".
    id: String,
    /// The command's input, matching its input_schema from list_commands.
    #[serde(default)]
    params: Map<String, Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BatchArgs {
    /// Commands to run in order as one undo step. If any fails, none is applied.
    commands: Vec<ExecuteArgs>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RenderArgs {
    /// Image width in pixels (default 768, max 2048).
    #[serde(default)]
    width_px: Option<u32>,
    /// Draw travel (laser-off) moves as dashed grey lines. Default true.
    #[serde(default)]
    show_jumps: Option<bool>,
}

fn schema<T: JsonSchema>() -> Arc<JsonObject> {
    let mut v = serde_json::to_value(schemars::schema_for!(T)).unwrap_or_else(|_| json!({"type": "object"}));
    if let Some(obj) = v.as_object_mut() {
        obj.remove("$schema");
        obj.remove("title");
        if !obj.contains_key("properties") {
            obj.insert("properties".into(), json!({}));
        }
        return Arc::new(obj.clone());
    }
    Arc::new(Map::new())
}

fn tool<T: JsonSchema>(name: &'static str, title: &str, description: &'static str, read_only: bool) -> Tool {
    Tool::new(name, description, schema::<T>())
        .with_title(title)
        .with_annotations(ToolAnnotations::with_title(title).read_only(read_only))
}

fn ok_json(v: Value) -> CallToolResult {
    let text = serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string());
    CallToolResult::success(vec![ContentBlock::text(text)])
}

fn err_result(e: EngineError) -> CallToolResult {
    let body = e.body();
    let text = serde_json::to_string_pretty(&body).unwrap_or_else(|_| e.to_string());
    CallToolResult::error(vec![ContentBlock::text(text)])
}

fn args<T: DeserializeOwned>(tool: &str, a: JsonObject) -> Result<T, EngineError> {
    serde_json::from_value(Value::Object(a))
        .map_err(|e| EngineError::BadRequest(format!("bad arguments for {tool}: {e}")))
}

impl GalvoMcp {
    pub fn new(state: Arc<AppState>) -> Self {
        GalvoMcp { state }
    }

    fn origin() -> Origin {
        Origin::Agent(MCP_AGENT.into())
    }

    /// Every tool definition, in [`TOOL_NAMES`] order.
    pub fn tools() -> Vec<Tool> {
        vec![
            tool::<NoArgs>(
                "list_commands",
                "List document commands",
                "Every document command: id, title, description (units, frames, result), risk and JSON input_schema. \
                 Run them with execute or batch.",
                true,
            ),
            tool::<ExecuteArgs>(
                "execute",
                "Execute a command",
                "Run one document command by id with its params. Returns {ok, result, revision}; on failure the error \
                 and, for unknown ids, close matches (candidates). One undo step.",
                false,
            ),
            tool::<BatchArgs>(
                "batch",
                "Execute commands as one step",
                "Run several document commands in order as ONE undo step, all-or-nothing: if any fails the document \
                 is left unchanged. Returns {ok, results, revision}.",
                false,
            ),
            tool::<NoArgs>(
                "inspect_document",
                "Inspect the document",
                "Structured summary (doc.inspect): stock, layers with cut settings, objects with field-frame bounds in \
                 mm, and warnings (objects off the stock, empty layers, objects that will not run).",
                true,
            ),
            tool::<RenderArgs>(
                "render_preview",
                "Render a preview",
                "PNG of the whole galvo field seen from above (operator at the bottom, +Y up): stock (tan), objects \
                 (layer colours), planned marks (solid) and travel (dashed). Also returns job stats as JSON text, or \
                 the planning error (e.g. out of field, power above the machine limit).",
                true,
            ),
            tool::<NoArgs>("undo", "Undo", "Undo the last edit (by anyone). Returns the new revision.", false),
            tool::<NoArgs>("redo", "Redo", "Redo the last undone edit. Returns the new revision.", false),
            tool::<NoArgs>(
                "machine_status",
                "Machine status",
                "Laser connection (or simulator), state, field size (mm), profile, and runs (pending ones wait for a \
                 person to approve them in the UI).",
                true,
            ),
            tool::<FrameRequest>(
                "frame_start",
                "Frame with the red pointer",
                "Trace the job outline with the red aiming pointer so the person can check placement on the stock. \
                 Never fires the laser. ids: objects to frame (default: the whole job); mode: \"bounds\" (default) or \
                 \"outline\". Keeps tracing until frame_stop, machine_stop, or 2 minutes.",
                false,
            ),
            tool::<NoArgs>("frame_stop", "Stop framing", "Stop the red-pointer frame.", false),
            tool::<NoArgs>(
                "job_request_run",
                "Request a run",
                "Plan and encode the current document and create a PENDING run. This does not fire the laser: a \
                 person must review and approve it in the galvo-studio UI. Returns the run id, a summary and stats.",
                false,
            ),
            tool::<NoArgs>(
                "machine_stop",
                "Stop the machine",
                "Stop immediately: abort any running job or frame, close the laser and cancel pending runs. Always \
                 allowed.",
                false,
            ),
        ]
    }

    /// Run tool `name` with `arguments`. Failures come back as tool errors (`is_error`), never protocol errors.
    pub async fn call(&self, name: &str, arguments: JsonObject) -> CallToolResult {
        match self.dispatch(name, arguments).await {
            Ok(r) => r,
            Err(e) => err_result(e),
        }
    }

    async fn dispatch(&self, name: &str, a: JsonObject) -> Result<CallToolResult, EngineError> {
        let st = &self.state;
        match name {
            "list_commands" => {
                args::<NoArgs>(name, a)?;
                Ok(ok_json(json!(st.specs())))
            }
            "execute" => {
                let x: ExecuteArgs = args(name, a)?;
                let o = st.execute(Invocation { id: x.id, params: Value::Object(x.params) }, Self::origin())?;
                Ok(ok_json(json!({ "ok": true, "result": o.result, "revision": o.revision })))
            }
            "batch" => {
                let x: BatchArgs = args(name, a)?;
                let invs =
                    x.commands.into_iter().map(|c| Invocation { id: c.id, params: Value::Object(c.params) }).collect();
                let outs = st.batch(invs, Self::origin())?;
                let revision = outs.last().map(|o| o.revision).unwrap_or_else(|| st.revision());
                let results: Vec<&Value> = outs.iter().map(|o| &o.result).collect();
                Ok(ok_json(json!({ "ok": true, "results": results, "revision": revision })))
            }
            "inspect_document" => {
                args::<NoArgs>(name, a)?;
                let o = st.execute(Invocation { id: "doc.inspect".into(), params: json!({}) }, Self::origin())?;
                Ok(ok_json(o.result))
            }
            "render_preview" => {
                let x: RenderArgs = args(name, a)?;
                self.render_preview(x).await
            }
            "undo" => {
                args::<NoArgs>(name, a)?;
                Ok(ok_json(json!({ "ok": true, "revision": st.undo()? })))
            }
            "redo" => {
                args::<NoArgs>(name, a)?;
                Ok(ok_json(json!({ "ok": true, "revision": st.redo()? })))
            }
            "machine_status" => {
                args::<NoArgs>(name, a)?;
                Ok(ok_json(st.machine_status()))
            }
            "frame_start" => {
                let req: FrameRequest = args(name, a)?;
                Ok(ok_json(st.frame_start(req).await?))
            }
            "frame_stop" => {
                args::<NoArgs>(name, a)?;
                st.frame_stop().await?;
                Ok(ok_json(json!({ "ok": true })))
            }
            "job_request_run" => {
                args::<NoArgs>(name, a)?;
                let view = st.request_run(Self::origin()).await?;
                Ok(ok_json(json!({
                    "ok": true,
                    "run_id": view.id,
                    "state": view.state,
                    "summary": view.summary,
                    "stats": view.stats,
                    "message": APPROVAL_MESSAGE,
                })))
            }
            "machine_stop" => {
                args::<NoArgs>(name, a)?;
                Ok(ok_json(st.stop().await?))
            }
            other => Err(EngineError::NotFound(format!("unknown tool '{other}' (tools: {})", TOOL_NAMES.join(", ")))),
        }
    }

    async fn render_preview(&self, x: RenderArgs) -> Result<CallToolResult, EngineError> {
        let opts = RenderOptions { show_jumps: x.show_jumps.unwrap_or(true) };
        let width = x.width_px.unwrap_or(768).min(render::MAX_PNG_PX);
        let (svg, preview) = self.state.preview_svg(opts).await?;
        let png = tokio::task::spawn_blocking(move || render::svg_to_png(&svg, width)).await??;
        let b64 = base64::engine::general_purpose::STANDARD.encode(png);
        let doc = self.state.document();
        let info = match &preview {
            Ok(p) => json!({
                "ok": true,
                "revision": doc.revision,
                "field_mm": self.state.machine.profile().field_mm,
                "objects": doc.objects.len(),
                "stats": p.stats,
                "mark_segments": p.segments.iter().filter(|s| s.kind == plan::SegmentKind::Mark).count(),
                "view": "whole field from above; operator at the bottom; +X right, +Y up (away from the operator)",
            }),
            Err(e) => json!({
                "ok": false,
                "revision": doc.revision,
                "plan_error": e.to_string(),
                "note": "the image shows the document without a toolpath because planning failed",
            }),
        };
        let text = serde_json::to_string_pretty(&info).unwrap_or_default();
        Ok(CallToolResult::success(vec![ContentBlock::image(b64, "image/png"), ContentBlock::text(text)]))
    }
}

impl ServerHandler for GalvoMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("galvo-studio", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(Self::tools()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        Ok(self.call(&request.name, request.arguments.unwrap_or_default()).await.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::RunState;
    use crate::tests::test_state;

    fn obj(v: Value) -> JsonObject {
        v.as_object().cloned().unwrap_or_default()
    }

    fn text_of(r: &CallToolResult) -> String {
        r.content
            .iter()
            .find_map(|c| match c {
                ContentBlock::Text(t) => Some(t.text.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }

    #[test]
    fn tool_list_is_complete() {
        let names: Vec<String> = GalvoMcp::tools().iter().map(|t| t.name.to_string()).collect();
        assert_eq!(names, TOOL_NAMES.map(String::from).to_vec());
        for t in GalvoMcp::tools() {
            assert_eq!(t.input_schema.get("type"), Some(&json!("object")), "{}", t.name);
            assert!(t.description.as_deref().is_some_and(|d| !d.is_empty()));
        }
    }

    #[test]
    fn module_never_arms_a_run() {
        let src = include_str!("mcp.rs");
        let body = src.split("#[cfg(test)]").next().unwrap_or_default();
        for forbidden in ["Armed", "approved_in_ui", "approve::", ".run(", "Controller"] {
            assert!(!body.contains(forbidden), "mcp.rs must not reference {forbidden}");
        }
    }

    #[tokio::test]
    async fn job_request_run_creates_a_pending_run_and_sends_nothing() {
        let st = test_state().await;
        let mcp = GalvoMcp::new(st.clone());
        let r =
            mcp.call("execute", obj(json!({"id": "shape.rect", "params": {"width_mm": 10, "height_mm": 10}}))).await;
        assert_eq!(r.is_error, Some(false), "{}", text_of(&r));
        let counters = st.machine.sim_counters().unwrap().clone();
        let before = (counters.singles(), counters.packets());

        let r = mcp.call("job_request_run", JsonObject::new()).await;
        assert_eq!(r.is_error, Some(false), "{}", text_of(&r));
        let v: Value = serde_json::from_str(&text_of(&r)).unwrap();
        let id = v["run_id"].as_str().unwrap().to_owned();
        assert!(v["message"].as_str().unwrap().contains("approve"));

        // Give any (wrongly) spawned work a chance to run.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_eq!((counters.singles(), counters.packets()), before, "requesting a run must not talk to the board");
        assert_eq!(counters.laser_singles(), 0);
        assert!(!st.machine.is_running());
        assert_eq!(st.runs().get(&id).unwrap().state, RunState::Pending);
        let status = st.machine_status();
        assert_eq!(status["pending_runs"][0]["id"], id.as_str());
    }

    #[tokio::test]
    async fn edits_are_tagged_as_agent_and_errors_are_tool_errors() {
        let st = test_state().await;
        let mcp = GalvoMcp::new(st.clone());
        let r = mcp.call("execute", obj(json!({"id": "shape.rect", "params": {"width_mm": 5, "height_mm": 5}}))).await;
        assert_eq!(r.is_error, Some(false));
        assert_eq!(st.history()[0].origin, Origin::Agent(MCP_AGENT.into()));

        let r = mcp.call("execute", obj(json!({"id": "no.such", "params": {}}))).await;
        assert_eq!(r.is_error, Some(true));
        let r = mcp.call("nope", JsonObject::new()).await;
        assert_eq!(r.is_error, Some(true));
        let r = mcp.call("undo", obj(json!({"extra": 1}))).await;
        assert_eq!(r.is_error, Some(true));
    }

    #[tokio::test]
    async fn render_preview_returns_png_and_stats() {
        let st = test_state().await;
        let mcp = GalvoMcp::new(st.clone());
        mcp.call("execute", obj(json!({"id": "shape.rect", "params": {"width_mm": 20, "height_mm": 10}}))).await;
        let r = mcp.call("render_preview", obj(json!({"width_px": 200}))).await;
        assert_eq!(r.is_error, Some(false), "{}", text_of(&r));
        assert!(r.content.iter().any(|c| matches!(c, ContentBlock::Image(i) if i.mime_type == "image/png")));
        let v: Value = serde_json::from_str(&text_of(&r)).unwrap();
        assert!(v["stats"]["mark_length_mm"].as_f64().unwrap() > 59.0);
    }
}
