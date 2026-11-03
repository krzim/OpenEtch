//! The document routes, as the engine's `api`, `documents` and `userfiles` modules answer them: commands, execute,
//! batch, undo/redo/history, the document and its change sets, open documents, previews, project status,
//! downloads, uploads, imports, exports, print and image files.

use std::collections::HashMap;

use appcore::AppError;
use appcore::http::{body_or_default, body_required, check_query, parse_origin};
use appcore::upload::{FileFormat, IMPORT_QUERY, ImportRequest, UPLOAD_QUERY, UploadOptions};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use commands::{Invocation, Origin};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Backend;
use super::reply::{Answer, Routed, command_answer};

#[derive(Deserialize)]
struct ExecuteBody {
    id: String,
    #[serde(default)]
    params: Value,
    #[serde(default)]
    origin: Option<Value>,
}

#[derive(Deserialize)]
struct BatchBody {
    commands: Vec<Invocation>,
    #[serde(default)]
    origin: Option<Value>,
}

#[derive(Default, Deserialize)]
struct OriginBody {
    #[serde(default)]
    origin: Option<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GotoBody {
    marker: usize,
    #[serde(default)]
    origin: Option<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteStepBody {
    index: usize,
    #[serde(default)]
    origin: Option<Value>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBody {
    project: Option<String>,
    activate: Option<bool>,
    origin: Option<Value>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActivateBody {
    origin: Option<Value>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CloseBody {
    discard: Option<bool>,
}

/// An optional JSON body of the document routes (`{}` when empty; "bad body" as the engine's `documents` module).
fn doc_body<T: for<'de> Deserialize<'de> + Default>(bytes: &[u8]) -> Result<T, AppError> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(bytes).map_err(|e| AppError::BadRequest(format!("bad body: {e}")))
}

/// Refuse query parameters a route does not take (`deny_unknown_fields` on the engine's query structs).
fn only(q: &HashMap<String, String>, allowed: &[&str]) -> Result<(), AppError> {
    match q.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) => Err(AppError::BadRequest(format!("bad query: unknown field `{k}`, expected one of {}", allowed.iter().map(|a| format!("`{a}`")).collect::<Vec<_>>().join(", ")))),
        None => Ok(()),
    }
}

/// A route of this module, or `None` when the path is not one.
pub fn route(b: &mut Backend, method: &str, segs: &[&str], q: &HashMap<String, String>, body: &[u8]) -> Result<Option<Answer>, AppError> {
    let a = match (method, segs) {
        ("GET", ["commands"]) => Answer::json(&json!(b.session.registry().specs().collect::<Vec<_>>())),
        ("POST", ["execute"]) => execute(b, body)?,
        ("POST", ["batch"]) => batch(b, body)?,
        ("POST", ["undo"]) => undo_redo(b, body, true)?,
        ("POST", ["redo"]) => undo_redo(b, body, false)?,
        ("GET", ["document"]) => {
            let id = b.docs.active.clone();
            b.doc_log.observe(b.session.document(), &id);
            Answer::json(&json!(b.session.document()))
        }
        ("GET", ["document", "sync"]) => {
            only(q, &["since", "epoch"])?;
            let since = match (q.get("since"), q.get("epoch")) {
                (Some(r), Some(e)) => Some((e.clone(), r.parse::<u64>().map_err(|e| AppError::BadRequest(format!("bad query: {e}")))?)),
                _ => None,
            };
            let payload = b.doc_sync(since);
            Answer::bytes("application/json", serde_json::to_vec(&payload).map_err(|e| AppError::Internal(e.to_string()))?).header("cache-control", "no-store")
        }
        ("GET", ["history"]) => Answer::json(&json!(b.session.history_view())),
        ("POST", ["history", "goto"]) => {
            let g: GotoBody = body_required(body)?;
            let origin = parse_origin(g.origin.as_ref())?;
            command_answer(b.timeline_op(origin, |s| s.goto(g.marker)))?
        }
        ("POST", ["history", "delete"]) => {
            let d: DeleteStepBody = body_required(body)?;
            let origin = parse_origin(d.origin.as_ref())?;
            command_answer(b.timeline_op(origin, |s| s.delete_step(d.index)))?
        }
        ("GET", ["preview"]) => {
            only(q, &["layers"])?;
            let layers = q.get("layers").cloned();
            let c = b.planned();
            let p = c.result.as_ref().map_err(|e| AppError::Plan(e.clone()))?;
            let v = appcore::preview::preview_json(&c.doc, p, layers.as_deref()).map_err(|e| AppError::Internal(e.to_string()))?;
            Answer::json(&v)
        }
        ("GET", ["preview", "stats"]) => {
            let c = b.planned();
            let p = c.result.as_ref().map_err(|e| AppError::Plan(e.clone()))?;
            Answer::json(&appcore::preview::stats_json(&c.doc, p, c.revision))
        }
        ("GET", ["preview", "context"]) => Answer::json(&json!(b.preview_context())),
        ("GET", ["preview.svg"]) => {
            let opts = appcore::render::RenderOptions { show_jumps: q.get("jumps").is_none_or(|v| v != "false") };
            let field = b.target().field_mm;
            let c = b.planned();
            let svg = appcore::render::preview_svg(&c.doc, field, c.result.as_ref().ok(), opts);
            Answer::bytes("image/svg+xml", svg.into_bytes()).header("cache-control", "no-store")
        }
        ("GET", ["fill_regions"]) => Answer::json(&json!(plan::fill_regions(b.session.document()))),
        ("GET", ["project"]) => Answer::json(&b.project.status(b.session.document().revision)),
        ("GET", ["document", "download"]) => download(b, q)?,
        ("POST", ["document", "upload"]) => upload(b, q, body)?,
        ("POST", ["document", "import"]) => import(b, q, body)?,
        ("GET", ["export"]) => export(b, q)?,
        ("GET", ["print"]) => print(b, q)?,
        ("GET", ["images", id, "file"]) => image_file(b, id)?,
        ("GET", ["images", id, "processed.png"]) => image_processed(b, id, q)?,
        ("GET", ["documents"]) => Answer::json(&b.docs.view(&b.session, &b.project)),
        ("POST", ["documents"]) => create_document(b, body)?,
        ("POST", ["documents", id, "activate"]) => {
            let a: ActivateBody = doc_body(body)?;
            let origin = parse_origin(a.origin.as_ref())?;
            Answer::json(&activate_document(b, id, origin)?)
        }
        ("POST", ["documents", id, "close"]) => {
            let c: CloseBody = doc_body(body)?;
            Answer::json(&close_document(b, id, c.discard.unwrap_or(false))?)
        }
        ("GET", ["layers", "unsupported"]) => Answer::json(&appcore::docset::unsupported_settings(b.session.document())),
        _ => return Ok(None),
    };
    Ok(Some(a))
}

fn execute(b: &mut Backend, body: &[u8]) -> Routed {
    let e: ExecuteBody = body_required(body)?;
    let origin = parse_origin(e.origin.as_ref())?;
    command_answer(b.execute(Invocation { id: e.id, params: e.params }, origin).map(|o| json!({ "ok": true, "result": o.result, "revision": o.revision })))
}

fn batch(b: &mut Backend, body: &[u8]) -> Routed {
    let bb: BatchBody = body_required(body)?;
    let origin = parse_origin(bb.origin.as_ref())?;
    let r = b.batch_opening(bb.commands, origin, None, None).map(|outs| {
        let revision = outs.last().map(|o| o.revision).unwrap_or_else(|| b.session.document().revision);
        let results: Vec<&Value> = outs.iter().map(|o| &o.result).collect();
        json!({ "ok": true, "results": results, "revision": revision })
    });
    command_answer(r)
}

fn undo_redo(b: &mut Backend, body: &[u8], undo: bool) -> Routed {
    let o: OriginBody = body_or_default(body)?;
    let origin = parse_origin(o.origin.as_ref())?;
    let r = if undo { b.timeline_op(origin, commands::Session::undo) } else { b.timeline_op(origin, commands::Session::redo) };
    command_answer(r.map(|v| json!({ "ok": true, "revision": v["revision"].as_u64().unwrap_or_default() })))
}

/// `GET /api/document/download?format=openetch|lbrn2`. Also names the revision and open document it is of
/// (`X-OpenEtch-Revision`, `X-OpenEtch-Document`), so the shell can record a save (`/core/saved`).
fn download(b: &mut Backend, q: &HashMap<String, String>) -> Routed {
    check_query(q, &["format"])?;
    let format = FileFormat::parse(q.get("format").map_or("openetch", String::as_str))?;
    let (content, warnings) = match format {
        FileFormat::OpenEtch => (appcore::upload::saveable(b.session.document())?, 0),
        FileFormat::Lbrn2 => {
            let o = b.execute(Invocation { id: "doc.export_lbrn2".into(), params: json!({}) }, Origin::Ui)?;
            let n = o.result["warnings"].as_array().map_or(0, Vec::len);
            (o.result["content"].as_str().unwrap_or_default().to_owned(), n)
        }
    };
    let revision = b.session.document().revision;
    Ok(Answer::bytes(format.content_type(), content.into_bytes())
        .header("content-disposition", appcore::http::disposition("attachment", &b.stem(), format.extension()))
        .header("cache-control", "no-store")
        .header("x-openetch-warnings", warnings.to_string())
        .header("x-openetch-revision", revision.to_string())
        .header("x-openetch-document", b.docs.active.clone()))
}

fn upload(b: &mut Backend, q: &HashMap<String, String>, body: &[u8]) -> Routed {
    check_query(q, &UPLOAD_QUERY)?;
    let (format, opts) = UploadOptions::from_query(q)?;
    if body.is_empty() {
        return Err(AppError::BadRequest("the upload is empty".into()));
    }
    let r = appcore::upload::upload_invocation(format, opts, body.to_vec())
        .and_then(|(inv, display)| b.batch_opening(vec![inv], Origin::Ui, None, display))
        .map(|mut v| {
            let o = v.remove(0);
            json!({ "ok": true, "result": o.result, "revision": o.revision })
        });
    command_answer(r)
}

fn import(b: &mut Backend, q: &HashMap<String, String>, body: &[u8]) -> Routed {
    check_query(q, &IMPORT_QUERY)?;
    let req = ImportRequest::parse(q.clone(), body.is_empty())?;
    let lightburn = req.lightburn();
    let display = req.display.clone();
    let r = req.invocations(b.session.document(), body.to_vec()).and_then(|invs| {
        let replacing = lightburn || invs.first().is_some_and(|i| i.id == "doc.new");
        let display = if replacing { display } else { None };
        b.batch_opening(invs, Origin::Ui, None, display)
    });
    command_answer(r.map(|mut v| {
        let o = v.pop().expect("the import ran");
        json!({ "ok": true, "result": o.result, "revision": o.revision })
    }))
}

/// Run a query command; a failure is the command's error.
fn query_cmd(b: &mut Backend, id: &str, params: Value) -> Result<Value, AppError> {
    Ok(b.execute(Invocation { id: id.into(), params }, Origin::Ui)?.result)
}

fn export(b: &mut Backend, q: &HashMap<String, String>) -> Routed {
    check_query(q, &["format", "ids", "colors", "stroke_mm", "images", "tolerance_mm", "keep_circles", "area"])?;
    let mut params = serde_json::Map::new();
    for (k, v) in q {
        params.insert(k.clone(), appcore::upload::qvalue(k, v)?);
    }
    if !params.contains_key("format") {
        return Err(AppError::BadRequest("format is required (svg, dxf, ai or pdf)".into()));
    }
    let r = query_cmd(b, "doc.export", Value::Object(params))?;
    let body = appcore::upload::decoded_content(&r)?;
    let n = r["warnings"].as_array().map_or(0, Vec::len);
    Ok(Answer::file("attachment", &b.stem(), r["extension"].as_str().unwrap_or("bin"), r["mime"].as_str().unwrap_or("application/octet-stream"), body, n))
}

fn print(b: &mut Backend, q: &HashMap<String, String>) -> Routed {
    check_query(q, &["format", "colors", "line_width_mm", "paper", "margin_mm", "area", "ids"])?;
    let mut params = serde_json::Map::new();
    for (k, v) in q {
        let key = if k == "paper" { "paper_mm".to_string() } else { k.clone() };
        params.insert(key, appcore::upload::qvalue(k, v)?);
    }
    let r = query_cmd(b, "doc.print", Value::Object(params))?;
    let body = appcore::upload::decoded_content(&r)?;
    let n = r["warnings"].as_array().map_or(0, Vec::len);
    Ok(Answer::file("inline", &b.stem(), r["extension"].as_str().unwrap_or("pdf"), r["mime"].as_str().unwrap_or("application/pdf"), body, n))
}

fn image_file(b: &mut Backend, id: &str) -> Routed {
    let r = query_cmd(b, "image.export", json!({ "id": id }))?;
    let body = STANDARD.decode(r["data_base64"].as_str().unwrap_or_default()).map_err(|e| AppError::Internal(e.to_string()))?;
    let name = b.session.document().object(id).map_or_else(|| id.to_owned(), |o| o.name.clone());
    Ok(Answer::file("attachment", &name, r["extension"].as_str().unwrap_or("png"), r["mime"].as_str().unwrap_or("image/png"), body, 0))
}

fn image_processed(b: &mut Backend, id: &str, q: &HashMap<String, String>) -> Routed {
    check_query(q, &["sub_layer"])?;
    let sub = q
        .get("sub_layer")
        .map(|v| v.parse::<usize>().map_err(|_| AppError::BadRequest("sub_layer must be a whole number".into())))
        .transpose()?;
    let r = query_cmd(b, "image.processed", json!({ "id": id, "sub_layer": sub }))?;
    let body = STANDARD.decode(r["png_base64"].as_str().unwrap_or_default()).map_err(|e| AppError::Internal(e.to_string()))?;
    let name = b.session.document().object(id).map_or_else(|| id.to_owned(), |o| format!("{} processed", o.name));
    Ok(Answer::file("attachment", &name, "png", "image/png", body, 0))
}

fn activate_document(b: &mut Backend, id: &str, origin: Origin) -> Result<Value, AppError> {
    let a = b.docs.activate(&mut b.session, &mut b.project, id)?;
    if !a.changed {
        return Ok(json!({ "ok": true, "active": id, "revision": a.revision, "changed": false }));
    }
    b.emit(appcore::Event::DocumentsChanged { active: id.to_owned() });
    b.announce(a.revision.wrapping_sub(1), a.revision, origin, a.hist);
    Ok(json!({ "ok": true, "active": id, "revision": a.revision, "changed": true }))
}

/// A new, empty document laid out for the active device (New Window); not activated.
fn new_document(b: &mut Backend) -> Result<String, AppError> {
    let device = b.devices.active().clone();
    let doc = appcore::layout::new_document_for(&device, b.profile_field());
    let session = commands::Session::new(doc);
    let project = appcore::project::ProjectState::clean_at(session.document().revision);
    let id = b.docs.add(session, project)?;
    b.emit(appcore::Event::DocumentsChanged { active: b.docs.active.clone() });
    Ok(id)
}

/// `POST /api/documents` without a project (projects are opened into a new document by the shell, which holds them:
/// `/core/open?kind=project&document=new`).
fn create_document(b: &mut Backend, body: &[u8]) -> Routed {
    let c: CreateBody = doc_body(body)?;
    let origin = parse_origin(c.origin.as_ref())?;
    if c.project.is_some() {
        return Err(AppError::BadRequest("a project opens through the browser's project storage (the worker shell)".into()));
    }
    let id = new_document(b)?;
    let v = if c.activate.unwrap_or(true) {
        let a = activate_document(b, &id, origin)?;
        json!({ "ok": true, "id": id, "active": id, "existing": false, "revision": a["revision"] })
    } else {
        json!({ "ok": true, "id": id, "active": b.docs.active, "existing": false })
    };
    Ok(Answer::with_status(201, &v))
}

fn close_document(b: &mut Backend, id: &str, discard: bool) -> Result<Value, AppError> {
    b.docs.check_close(&b.session, &b.project, id, discard)?;
    if b.docs.active == id {
        let next = b.docs.successor(id)?;
        activate_document(b, &next, Origin::Ui)?;
    }
    let active = b.docs.remove(id, discard)?;
    b.emit(appcore::Event::DocumentsChanged { active: active.clone() });
    Ok(json!({ "ok": true, "closed": id, "active": active }))
}
