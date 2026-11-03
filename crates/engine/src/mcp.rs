//! The MCP server (rmcp, Streamable HTTP at `/mcp`). Document tools are generated from the same command registry
//! the UI uses; machine tools are separate so clients can approve them separately.
//!
//! The machine tools can frame (red pointer) and stop, and `job_request_run` creates a pending run. None of them
//! can fire the laser: this module never creates the machine crate's arming token (only the UI approval endpoint
//! does, with the operator PIN), never receives or handles a PIN, and a test checks that its source mentions neither.

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

/// Framing a gantry moves its head (with the laser off): a person starts it in the app, never an agent, as with
/// jogging and homing. A galvo's frame is only the red pointer and stays available to agents.
pub(crate) const GANTRY_FRAME: &str =
    "framing a gantry moves its head; only a person can start it in the OpenEtch app (frame_stop and machine_stop still work)";

/// The agent name MCP edits are recorded under in the history.
pub const MCP_AGENT: &str = "mcp";

/// Every tool, in listing order.
pub const TOOL_NAMES: [&str; 22] = [
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
    "project_list",
    "project_save",
    "project_open",
    "project_export_lbrn2",
    "history_list",
    "history_goto",
    "lightburn_devices",
    "documents_list",
    "document_activate",
    "devices_list",
];

/// Server instructions shown to the model.
pub const INSTRUCTIONS: &str = "OpenEtch controls a fiber galvo laser (ComMarker B6 MOPA 60W) and its live design \
document, shared with people using the web UI.

Coordinates: everything is millimetres and degrees in the FIELD FRAME: origin at the centre of the galvo field, +X to \
the operator's right, +Y away from the operator (toward the machine column). The field is a square (see \
machine_status.field_mm, about 153.75 mm); every object must lie strictly inside it.

Workflow:
1. stock.define - describe the physical piece (shape, size, thickness, where it sits in the field).
2. Import or draw art: art.import_svg, doc.import_lbrn2 (a LightBurn project), art.import_image (a photo or \
bitmap), shape.rect / shape.ellipse / shape.path.
3. art.place - fit and anchor the art on the stock (margins, alignment).
4. layer.update - set the layer's cut settings (power %, speed, frequency, MOPA pulse width, fill/hatch, passes). \
Like LightBurn's sub-layers, a layer can run more passes over the same objects with other settings: layer.sub_add \
(a copy of the main settings with changes), layer.sub_update, layer.sub_delete, layer.sub_move and \
layer.sub_duplicate (indices are 0-based positions in sub_layers). A sub-layer with cleanup {every_n_passes: n} is a \
cleanup pass that runs after every n main passes and after the last (typically fast, low power, hatch turned 45 \
degrees); the other sub-layers run after the main passes, in order. Images are engraved in LightBurn's Image mode \
with their layer's settings.image (dither: threshold, ordered, atkinson, floyd_steinberg, stucki, jarvis, \
newsprint, halftone, sketch or grayscale; interval_mm or dpi, angle_deg, negative, dot_width_mm, min_power_pct for \
grayscale) while the layer's vectors keep its mode; image.adjust sets an image's gamma, contrast and brightness. A \
fine interval on a large image can pass the job's segment limit; planning then names the image.
5. render_preview - look at the result and the toolpath; inspect_document for exact numbers and warnings.
6. frame_start - trace the job outline with the red pointer so the person can check placement (no laser), then \
frame_stop.
7. job_request_run - create a pending run.

Firing the laser always requires a person at the machine to approve the pending run: in the OpenEtch UI with their \
operator PIN or, when the engine runs without a PIN, with Start in the app (plus, if configured, a physical \
confirmation switch). The machine's foot switch never starts a run you requested: its first press frames the \
person's current document and the next burns exactly what was framed. You \
cannot and must not approve or start a run, and you \
must never ask for, guess, handle or set the operator PIN, or try to approve runs by any other route (HTTP, \
shell, browser). machine_stop is always available and stops everything.

Projects (files live in the engine's projects directory; you never pass file paths): project_list, \
project_open {name} (replaces the document; undoable), project_save {name} (saving to the current project's name \
replaces it; replacing a different existing project needs overwrite: true, which you only pass when the person asked \
for it), project_export_lbrn2 {name, overwrite?} (writes <name>.lbrn2 for LightBurn; replacing an existing one needs \
overwrite: true, same rule). File contents go through execute: doc.save \
returns the .openetch JSON, doc.load replaces the document with one, doc.export_lbrn2 returns LightBurn XML, and \
doc.import_lbrn2 {content, mode: replace|merge, field_mm} imports LightBurn XML. LightBurn files do not record the \
galvo field size they were laid out for, and a wrong field_mm shifts everything by half the difference. Leave \
field_mm out and the engine picks it: the workspace of the OpenEtch device the file names (DeviceName, written \
by LightBurn 2.x; else a LightBurn device of that name), else the active device's, else the document's field_mm, \
else the machine's; the result's field_source (and device) says which. lightburn_devices lists the known LightBurn \
devices. When none fits and the person has not said (e.g. a 150 mm or 110 mm LightBurn workspace), ask, or check \
the import warnings and bounds.

Devices (like LightBurn's Devices window): devices_list shows OpenEtch's devices and which one is active. Only a \
JCZ (LMC) galvo can be driven; other devices (GCode/DSP gantries, other galvo boards) are design-only: their \
workspace (a bed of workspace_mm with an origin corner) is used for designing and LightBurn files, and frame_start \
and job_request_run are refused. Making another device active changes the machine, so only a person can do it, in \
the OpenEtch UI (Devices); you cannot activate, import or remove devices.

Use list_commands for every command's input schema; execute runs one command, batch runs several as one undo step \
(all-or-nothing). Edits you make are tagged as agent edits in the history and can be undone.

History is a timeline like Fusion 360's: history_list shows every step (index, command, origin, summary, state \
active|rolled_back|failed) and the marker (index of the last applied step). history_goto {marker} rolls back or \
forward without losing steps; an edit made after history_goto is inserted after the marker and the later steps are \
replayed (a step that no longer applies is kept as failed). undo/redo move the marker by one step; an edit made \
after undo discards the undone steps (marker_moved_by and undone_steps in history_list say which applies). doc.new, \
doc.load, project_open and a replacing doc.import_lbrn2 start a fresh timeline (only that step can be undone).

Several documents can be open at once (like LightBurn's New Window): documents_list shows them and which one is \
active; every other tool works on the active document. document_activate {id} switches the active document (each \
has its own timeline and project); the person sees the switch in the UI, so only switch when the task needs \
another open document, and say so.";

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

/// `job_request_run` arguments: all optional (the whole job by default).
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RequestRunArgs {
    /// Cut Selected Graphics: only these object or group ids.
    #[serde(default)]
    selection: Option<Vec<String>>,
    /// Re-mark a region: only the part of the planned toolpath inside this polygon (field mm, at least 3 points);
    /// job.remark_check lists what it touches first.
    #[serde(default)]
    region: Option<Vec<[f64; 2]>>,
}

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

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ProjectArgs {
    /// Project name: 1-100 letters, digits, spaces and - _ . ( ) (no leading dot, no ".."), optionally inside up to 4
    /// folders separated by "/" ("Clients/Acme/badge"; project_list shows them). Never a file path.
    name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SaveArgs {
    /// Project name: 1-100 letters, digits, spaces and - _ . ( ) (no leading dot, no ".."), optionally inside up to 4
    /// folders separated by "/" ("Clients/Acme/badge"; project_list shows them). Never a file path.
    name: String,
    /// Replace an existing project that is not the current one. Default false; only when the person asked.
    #[serde(default)]
    overwrite: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DocumentArgs {
    /// Open document id from documents_list, e.g. "default" or "doc-2".
    id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GotoArgs {
    /// Index of the last step to apply (from history_list); 0 rolls everything back, the number of steps rolls
    /// forward to the end.
    marker: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExportArgs {
    /// File name without .lbrn2: 1-100 letters, digits, spaces and - _ . ( ) (no path, no leading dot, no "..").
    name: String,
    /// Replace an existing <name>.lbrn2. Default false; only when the person asked.
    #[serde(default)]
    overwrite: bool,
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
            tool::<NoArgs>(
                "undo",
                "Undo",
                "Undo the last edit (by anyone); the next edit discards what was undone. Returns the new revision.",
                false,
            ),
            tool::<NoArgs>("redo", "Redo", "Redo the last undone edit. Returns the new revision.", false),
            tool::<NoArgs>(
                "machine_status",
                "Machine status",
                "Laser connection (or simulator), state, field size (mm), profile, runs (pending ones wait for a \
                 person to approve them in the UI) and job (the running job's progress and the job counter).",
                true,
            ),
            tool::<FrameRequest>(
                "frame_start",
                "Frame with the red pointer",
                "Trace the job outline with the red aiming pointer so the person can check placement on the stock. \
                 Never fires the laser. ids: objects to frame (default: the whole job); mode: \"bounds\" (default), \
                 \"hull\" or \"outline\"; individually, outside_only, tool_layers_only, continuous (default true), \
                 speed_mm_s, pointer (red|focus|both), live (follow edits), max_seconds (default 120). Keeps tracing \
                 until frame_stop, machine_stop, or the time limit. Galvos only: on a gantry (machine_status driver \
                 \"grbl\" or \"ruida\") framing moves the head, so only a person can start it in the app.",
                false,
            ),
            tool::<NoArgs>("frame_stop", "Stop framing", "Stop the red-pointer frame.", false),
            tool::<RequestRunArgs>(
                "job_request_run",
                "Request a run",
                "Plan and encode the current document and create a PENDING run. This does not fire the laser: a \
                 person must review and approve it in the OpenEtch UI. Optional selection (only those objects) and \
                 region (re-mark only the toolpath inside that polygon). A job marking inside a keep-out zone (a \
                 jig's or the device's) is refused. Returns the run id, a summary and stats.",
                false,
            ),
            tool::<NoArgs>(
                "machine_stop",
                "Stop the machine",
                "Stop immediately: abort any running job or frame, close the laser and cancel pending runs. Always \
                 allowed.",
                false,
            ),
            tool::<NoArgs>(
                "project_list",
                "List projects",
                "Saved projects in the engine's projects directory, its folders and locations: {projects: [{name, folder, modified_unix_s, size}], \
                 current: {name, saved_revision, revision, dirty}}.",
                true,
            ),
            tool::<SaveArgs>(
                "project_save",
                "Save the project",
                "Save the document as <name>.openetch in the projects directory and make it the current project. \
                 Replacing an existing project other than the current one needs overwrite: true. Returns {ok, name, \
                 path, revision}.",
                false,
            ),
            tool::<ProjectArgs>(
                "project_open",
                "Open a project",
                "Replace the document with saved project <name> (one undo step, starting a fresh timeline) and make \
                 it the current project. The replaced document stays reachable with one undo. Returns {ok, name, \
                 result, revision}.",
                false,
            ),
            tool::<ExportArgs>(
                "project_export_lbrn2",
                "Export to LightBurn",
                "Write the document as a LightBurn project <name>.lbrn2 in the projects directory (laid out for the \
                 field size the last LightBurn import used, else the document's field_mm). Replacing an existing \
                 <name>.lbrn2 needs overwrite: true. Returns {ok, path, warnings, field_mm}. For the XML itself use \
                 execute doc.export_lbrn2.",
                false,
            ),
            tool::<NoArgs>(
                "history_list",
                "List the timeline",
                "The edit timeline: {marker, marker_moved_by, undone_steps, steps: [{index, command, invocations, \
                 origin, summary, state, error?}]}. marker is the index of the last applied step (0 = none); \
                 marker_moved_by is undo (the next edit discards the undone_steps steps after the marker), timeline \
                 (the next edit is inserted after the marker) or edit; state is active, rolled_back (after the \
                 marker) or failed (no longer applies after an earlier step changed; shown red). File contents in \
                 parameters are shortened.",
                true,
            ),
            tool::<GotoArgs>(
                "history_goto",
                "Roll the timeline",
                "Move the timeline marker: apply steps 1..=marker and roll back the rest, without deleting anything. \
                 An edit made while rolled back is inserted after the marker. Returns {ok, marker, revision}.",
                false,
            ),
            tool::<NoArgs>(
                "lightburn_devices",
                "LightBurn devices",
                "LightBurn's devices as the engine knows them (from LightBurn's prefs.ini on the engine's computer, or a \
                 copy a person uploaded; only the device list is read): {available, source, default_device, devices: \
                 [{name, display_name, field_mm, width_mm, height_mm}], machine: {device, field_mm, diffs} | null}. \
                 field_mm is the field size LightBurn lays files out for; doc.import_lbrn2 uses it for a file naming a \
                 LightBurn device that is not one of OpenEtch's devices (devices_list). machine.diffs compare the \
                 machine profile with the LightBurn device.",
                true,
            ),
            tool::<NoArgs>(
                "documents_list",
                "Open documents",
                "The documents open in the engine (LightBurn's New Window): {active, documents: [{id, name, project, \
                 display_name, document_name, revision, dirty, objects, layers, active}]}. Every other tool works on \
                 the active document.",
                true,
            ),
            tool::<DocumentArgs>(
                "document_activate",
                "Switch the active document",
                "Make open document <id> (from documents_list) the active one: the document tools, previews, runs and \
                 framing then use it, and the UI shows it. Each document keeps its own undo timeline. Returns {ok, \
                 active, revision}.",
                false,
            ),
            tool::<NoArgs>(
                "devices_list",
                "Devices",
                "OpenEtch's devices, like LightBurn's Devices window: {active, devices: [{id, name, display_name, \
                 source, kind (jcz_galvo|gantry|galvo_other|unknown), lightburn_type, workspace_mm, origin, \
                 controllable, reason, active}]}. Only controllable devices (JCZ galvos) can frame or run; the others \
                 are design-only workspaces. Read-only: only a person can change the active device, in the UI.",
                true,
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
                // Blocking thread: an import can take seconds.
                let inv = Invocation { id: x.id, params: Value::Object(x.params) };
                let o = st.blocking(move |st| st.execute(inv, Self::origin())).await?;
                Ok(ok_json(json!({ "ok": true, "result": o.result, "revision": o.revision })))
            }
            "batch" => {
                let x: BatchArgs = args(name, a)?;
                let invs =
                    x.commands.into_iter().map(|c| Invocation { id: c.id, params: Value::Object(c.params) }).collect();
                let outs = st.blocking(move |st| st.batch(invs, Self::origin())).await?;
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
                Ok(ok_json(json!({ "ok": true, "revision": st.undo(Self::origin())? })))
            }
            "redo" => {
                args::<NoArgs>(name, a)?;
                Ok(ok_json(json!({ "ok": true, "revision": st.redo(Self::origin())? })))
            }
            "machine_status" => {
                args::<NoArgs>(name, a)?;
                Ok(ok_json(st.machine_status()))
            }
            "frame_start" => {
                let req: FrameRequest = args(name, a)?;
                if st.machine.is_gantry() {
                    return Err(EngineError::Forbidden(GANTRY_FRAME.into()));
                }
                Ok(ok_json(st.frame_start(req).await?))
            }
            "frame_stop" => {
                args::<NoArgs>(name, a)?;
                Ok(ok_json(st.frame_stop().await?))
            }
            "job_request_run" => {
                let r = args::<RequestRunArgs>(name, a)?;
                let options = crate::runs::RunOptions { selection: r.selection, region: r.region, ..Default::default() };
                let view = st.request_run_with(Self::origin(), options).await?;
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
            "project_list" => {
                args::<NoArgs>(name, a)?;
                Ok(ok_json(json!({ "projects": st.project_list().await?, "current": st.project_status() })))
            }
            "project_save" => {
                let x: SaveArgs = args(name, a)?;
                Ok(ok_json(st.project_save(&x.name, x.overwrite).await?))
            }
            "project_open" => {
                let x: ProjectArgs = args(name, a)?;
                Ok(ok_json(st.project_open(&x.name, Self::origin()).await?))
            }
            "project_export_lbrn2" => {
                let x: ExportArgs = args(name, a)?;
                Ok(ok_json(st.project_export_lbrn2(&x.name, x.overwrite, Self::origin()).await?))
            }
            "history_list" => {
                args::<NoArgs>(name, a)?;
                Ok(ok_json(json!(st.history_view())))
            }
            "history_goto" => {
                let x: GotoArgs = args(name, a)?;
                Ok(ok_json(st.blocking(move |st| st.history_goto(x.marker, Self::origin())).await?))
            }
            "documents_list" => {
                args::<NoArgs>(name, a)?;
                Ok(ok_json(st.documents_view()))
            }
            "document_activate" => {
                let x: DocumentArgs = args(name, a)?;
                Ok(ok_json(st.activate_document(&x.id, Self::origin())?))
            }
            "devices_list" => {
                args::<NoArgs>(name, a)?;
                Ok(ok_json(st.devices_view()))
            }
            "lightburn_devices" => {
                args::<NoArgs>(name, a)?;
                let mut v = st.blocking(|st| Ok(st.lightburn.view())).await?;
                v["machine"] = st.lightburn.machine_json(&st.machine.profile());
                Ok(ok_json(v))
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
        let doc = &preview.0;
        let info = match &preview.1 {
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
            .with_server_info(Implementation::new("OpenEtch", env!("CARGO_PKG_VERSION")))
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
    fn export_tool_takes_overwrite() {
        let t = GalvoMcp::tools().into_iter().find(|t| t.name == "project_export_lbrn2").unwrap();
        assert!(t.input_schema["properties"].get("overwrite").is_some(), "{:?}", t.input_schema);
        assert!(t.description.as_deref().unwrap().contains("overwrite: true"));
        assert!(INSTRUCTIONS.contains("project_export_lbrn2 {name, overwrite?}"));
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

    #[tokio::test]
    async fn devices_list_is_read_only_and_activation_is_ui_only() {
        let mcp = GalvoMcp::new(crate::tests::test_state().await);
        let v: Value = serde_json::from_str(&text_of(&mcp.call("devices_list", JsonObject::new()).await)).unwrap();
        assert_eq!(v["devices"][0]["kind"], json!("jcz_galvo"));
        assert_eq!(v["devices"][0]["active"], json!(true));
        assert_eq!(v["active"], v["devices"][0]["id"]);
        let t = GalvoMcp::tools().into_iter().find(|t| t.name == "devices_list").unwrap();
        assert!(t.annotations.as_ref().and_then(|a| a.read_only_hint) == Some(true));
        assert!(INSTRUCTIONS.contains("only a person can do it"));
        for name in TOOL_NAMES {
            // Switching the active document is allowed; activating a device is not.
            assert!(!(name.contains("device") && name.contains("activate")) && !name.contains("device_import"), "{name}");
        }
    }

    #[test]
    fn instructions_forbid_approving_and_pins() {
        assert!(INSTRUCTIONS.contains("cannot and must not approve"));
        assert!(INSTRUCTIONS.contains("operator PIN"));
        assert!(APPROVAL_MESSAGE.contains("must not approve"));
    }

    #[test]
    fn module_never_arms_a_run() {
        let src = include_str!("mcp.rs");
        let body = src.split("#[cfg(test)]").next().unwrap_or_default();
        for forbidden in ["Armed", "approved_in_ui", "approve::", ".run(", "Controller", "pin::", "PinStore", "verify("]
        {
            assert!(!body.contains(forbidden), "mcp.rs must not reference {forbidden}");
        }
    }

    #[tokio::test]
    async fn history_tools_list_and_roll_the_timeline() {
        let st = test_state().await;
        let mcp = GalvoMcp::new(st.clone());
        for _ in 0..2 {
            let r = mcp.call("execute", obj(json!({"id": "shape.rect", "params": {"width_mm": 4, "height_mm": 4}}))).await;
            assert_eq!(r.is_error, Some(false), "{}", text_of(&r));
        }
        let r = mcp.call("history_goto", obj(json!({"marker": 1}))).await;
        assert_eq!(r.is_error, Some(false), "{}", text_of(&r));
        assert_eq!(st.document().objects.len(), 1);
        let r = mcp.call("history_list", JsonObject::new()).await;
        let v: Value = serde_json::from_str(&text_of(&r)).unwrap();
        assert_eq!(v["marker"], 1);
        assert_eq!(v["steps"][1]["state"], "rolled_back");
        assert_eq!(v["steps"][0]["origin"], json!({"kind": "agent", "name": MCP_AGENT}));
        let r = mcp.call("history_goto", obj(json!({"marker": 7}))).await;
        assert_eq!(r.is_error, Some(true));
        let r = mcp.call("history_goto", obj(json!({}))).await;
        assert_eq!(r.is_error, Some(true));
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
    async fn lightburn_devices_lists_devices_and_the_import_rule_is_documented() {
        let st = test_state().await;
        let mcp = GalvoMcp::new(st.clone());
        let r = mcp.call("lightburn_devices", JsonObject::new()).await;
        assert_eq!(r.is_error, Some(false), "{}", text_of(&r));
        let v: Value = serde_json::from_str(&text_of(&r)).unwrap();
        assert_eq!((&v["available"], &v["machine"]), (&json!(false), &json!(null)), "{v}");
        let dir = crate::pin::temp_dir("mcp-lightburn");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("prefs.ini"), include_str!("../../machine/tests/fixtures/lightburn-prefs.json")).unwrap();
        st.lightburn.configure(crate::lightburn::LightBurnConfig {
            prefs_paths: vec![dir.join("prefs.ini")],
            ..Default::default()
        });
        let v: Value = serde_json::from_str(&text_of(&mcp.call("lightburn_devices", JsonObject::new()).await)).unwrap();
        assert_eq!((&v["default_device"], &v["devices"][0]["field_mm"]), (&json!("JCZFiber"), &json!(153.8)), "{v}");
        assert_eq!(v["machine"]["device"], json!("JCZFiber"));
        assert!(!v.to_string().contains("secret"));
        assert!(mcp.call("lightburn_devices", obj(json!({"x": 1}))).await.is_error == Some(true));
        assert!(INSTRUCTIONS.contains("DeviceName") && INSTRUCTIONS.contains("field_source"));
        let spec = st.specs().into_iter().find(|c| c.id == "doc.import_lbrn2").unwrap();
        assert!(spec.description.contains("DeviceName") && spec.description.contains("field_source"));
        let _ = std::fs::remove_dir_all(&dir);
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
    async fn project_tools_save_open_list_and_export() {
        let st = test_state().await;
        let mcp = GalvoMcp::new(st.clone());
        let call = |name: &'static str, v: Value| {
            let mcp = mcp.clone();
            async move {
                let r = mcp.call(name, obj(v)).await;
                (r.is_error == Some(true), serde_json::from_str::<Value>(&text_of(&r)).unwrap_or_default())
            }
        };
        let (err, v) = call("project_list", json!({})).await;
        assert!(!err, "{v}");
        assert_eq!(v["projects"], json!([]));
        assert_eq!(v["current"]["name"], Value::Null);

        let lbrn = crate::tests::LBRN2;
        let (err, v) = call("execute", json!({"id": "doc.import_lbrn2", "params": {"content": lbrn}})).await;
        assert!(!err, "{v}");
        assert_eq!(v["result"]["field_mm"], json!(150.0), "the profile's field when none is given");
        let (err, v) = call("project_save", json!({"name": "Agent job"})).await;
        assert!(!err, "{v}");
        assert_eq!(v["name"], "Agent job");
        let (err, v) = call("project_list", json!({})).await;
        assert!(!err);
        assert_eq!(v["projects"][0]["name"], "Agent job");
        assert_eq!(v["current"], json!({"name": "Agent job", "display_name": null, "saved_revision": 1, "revision": 1, "dirty": false}));

        // Agents cannot silently replace another project.
        call("execute", json!({"id": "doc.new", "params": {}})).await;
        let (err, v) = call("project_save", json!({"name": "Agent job"})).await;
        assert!(err, "{v}");
        assert!(v["error"].as_str().unwrap().contains("overwrite"), "{v}");
        let (err, _) = call("project_save", json!({"name": "Agent job", "overwrite": true})).await;
        assert!(!err);
        call("execute", json!({"id": "doc.rename", "params": {"name": "Changed"}})).await;

        let (err, v) = call("project_open", json!({"name": "Agent job"})).await;
        assert!(!err, "{v}");
        assert_eq!(st.document().name, "Untitled");
        assert_eq!(st.history().last().unwrap().origin, Origin::Agent(MCP_AGENT.into()));

        let (err, v) = call("project_export_lbrn2", json!({"name": "Agent job"})).await;
        assert!(!err, "{v}");
        let path = std::path::PathBuf::from(v["path"].as_str().unwrap());
        assert!(path.ends_with("Agent job.lbrn2"), "{path:?}");
        assert!(std::fs::read_to_string(&path).unwrap().contains("<LightBurnProject"));
        assert_eq!(v["field_mm"], json!(150.0));
        // Like project_save, an existing export (also one differing only in case) needs overwrite: true.
        std::fs::write(&path, "old").unwrap();
        for name in ["Agent job", "AGENT JOB"] {
            let (err, v) = call("project_export_lbrn2", json!({"name": name})).await;
            assert!(err, "{v}");
            assert!(v["error"].as_str().unwrap().contains("overwrite"), "{v}");
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old", "nothing was replaced");
        let (err, v) = call("project_export_lbrn2", json!({"name": "Agent job", "overwrite": true})).await;
        assert!(!err, "{v}");
        assert!(std::fs::read_to_string(&path).unwrap().contains("<LightBurnProject"));
        let (err, v) = call("project_export_lbrn2", json!({"name": "Agent job", "overwrite": "yes"})).await;
        assert!(err, "{v}");

        for bad in [json!({"name": "../x"}), json!({"name": "a//b"}), json!({}), json!({"name": "x", "path": "/tmp"})] {
            for tool in ["project_save", "project_open", "project_export_lbrn2"] {
                let (err, v) = call(tool, bad.clone()).await;
                assert!(err, "{tool} {bad}: {v}");
            }
        }
        let _ = std::fs::remove_dir_all(st.projects.root().parent().unwrap());
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
