//! Routes for the worker shell (`/core/...`), which keeps projects, autosaves, backups and templates in the
//! browser's storage and passes their text here. Each one does what the engine does after it has read the file.
//!
//! | Method | Path | Body | Answer |
//! |---|---|---|---|
//! | POST | `/core/open?kind=project&name=N[&origin=]` | the `.openetch` text | as `POST /api/projects/open`: `{ok, name, result, revision}`; the document then belongs to project `N`, saved at that revision |
//! | POST | `/core/open?kind=project&name=N&new_document=1` | the `.openetch` text | as `POST /api/documents {project: N}`: **201** `{ok, id, active, existing, revision, result?}`; a document already holding `N` is activated instead (`existing: true`) |
//! | POST | `/core/open?kind=template&format=openetch\|lbt` | the template | as `POST /api/templates/open`: `{ok, result, revision}`, a new untitled document |
//! | POST | `/core/open?kind=backup&project=P` | an autosave or backup | as `POST /api/backups/restore`: `{ok, result, revision}`, unsaved, downloads named `P` |
//! | POST | `/core/open?kind=recovery&project=P` or `&display=D` | a tab's recovery copy | `{ok, result, revision}`: the document with its unsaved changes, still project `P`'s (Save goes there) or named `D` |
//! | POST | `/core/saved?name=N&revision=R&document=D` | – | after the shell stored the text `GET /api/document/download?format=openetch` gave (its `X-OpenEtch-Revision` and `X-OpenEtch-Document`): document `D` is project `N` saved at `R`; emits `project_saved`; `{ok, name, revision}` |
//! | POST | `/core/thumbnail.svg?width=` | an `.openetch` | the project drawn as SVG (as `GET /api/projects/{name}/thumbnail.png`, before rasterising) |
//! | POST | `/core/thumbnail.png?width=32..=1024` | an `.openetch` | the PNG, when the core is built with resvg (feature `png`); else 404 |
//! | POST | `/core/template/check?format=openetch\|lbt` | a template | `{ok}` when it would open (as `POST /api/templates/upload` checks a `.lbt`) |
//! | PUT | `/core/store?store=S&key=K` | the record's text | another tab changed a record of a store this backend keeps: taken as at start (204) |
//! | DELETE | `/core/store?store=S&key=K` | – | another tab deleted the record (204) |

use std::collections::HashMap;

use appcore::AppError;
use appcore::http::{check_query, parse_origin};
use appcore::project::ProjectState;
use commands::{Invocation, Origin};
use serde_json::json;

use super::Backend;
use super::reply::{Answer, Routed};

/// A route of this module.
pub fn route(b: &mut Backend, method: &str, segs: &[&str], q: HashMap<String, String>, body: Vec<u8>) -> Routed {
    match (method, segs) {
        ("POST", ["open"]) => open(b, &q, body),
        ("POST", ["saved"]) => saved(b, &q),
        ("POST", ["thumbnail.svg"]) => {
            let (svg, _) = thumbnail_svg(b, &q, &body)?;
            Ok(Answer::bytes("image/svg+xml", svg.into_bytes()).header("cache-control", "no-cache"))
        }
        #[cfg(feature = "png")]
        ("POST", ["thumbnail.png"]) => {
            let (svg, width) = thumbnail_svg(b, &q, &body)?;
            let png = appcore::render::svg_to_png(&svg, width)?;
            Ok(Answer::bytes("image/png", png).header("cache-control", "no-cache"))
        }
        ("PUT" | "DELETE", ["store"]) => {
            check_query(&q, &["store", "key"])?;
            let need = |k: &str| q.get(k).cloned().ok_or_else(|| AppError::BadRequest(format!("{k} is required")));
            let (store, key) = (need("store")?, need("key")?);
            let value = if method == "PUT" { Some(text_of(body, "store record")?) } else { None };
            b.reload_record(&store, &key, value.as_deref())?;
            Ok(Answer::empty(204))
        }
        ("POST", ["template", "check"]) => {
            check_query(&q, &["format"])?;
            let text = text_of(body, "template")?;
            match q.get("format").map_or("openetch", String::as_str) {
                "openetch" => {
                    doc::file::from_openetch_json(&text).map_err(|e| AppError::BadRequest(format!("not an OpenEtch template: {e}")))?;
                }
                "lbt" => {
                    let lb = fileio::lbrn_legacy::to_lbrn2(&text).map_err(|e| AppError::BadRequest(e.to_string()))?;
                    lbrn::parse(&lb).map_err(|e| AppError::BadRequest(format!("not a LightBurn template: {e}")))?;
                }
                other => return Err(AppError::BadRequest(format!("unknown format '{other}' (openetch or lbt)"))),
            }
            Ok(Answer::json(&json!({ "ok": true })))
        }
        _ => Err(AppError::NotFound("no such core route".into())),
    }
}

fn text_of(body: Vec<u8>, what: &str) -> Result<String, AppError> {
    appcore::upload::upload_text(body, what)
}

fn open(b: &mut Backend, q: &HashMap<String, String>, body: Vec<u8>) -> Routed {
    check_query(q, &["kind", "name", "format", "project", "display", "origin", "new_document"])?;
    let origin = parse_origin(q.get("origin").map(|o| json!(o)).as_ref())?;
    let kind = q.get("kind").map(String::as_str).ok_or_else(|| AppError::BadRequest("kind is required (project, template or backup)".into()))?;
    let text = text_of(body, ".openetch JSON")?;
    match kind {
        "project" => {
            let name = q.get("name").cloned().ok_or_else(|| AppError::BadRequest("name is required".into()))?;
            appcore::project::validate_project_name(&name)?;
            let new_document = matches!(q.get("new_document").map(String::as_str), Some("1" | "true"));
            if new_document {
                // As the engine's `POST /api/documents {project}`: an open document holding it is activated instead.
                if let Some(id) = b.docs.with_project(&b.project, &name) {
                    let a = super::docs::activate_document(b, &id, origin)?;
                    return Ok(Answer::with_status(201, &json!({ "ok": true, "id": id, "active": id, "existing": true, "revision": a["revision"] })));
                }
                let id = super::docs::new_document(b)?;
                super::docs::activate_document(b, &id, origin.clone())?;
                let inv = Invocation { id: "doc.load".into(), params: appcore::batch::content_params(text) };
                let o = match b.batch_opening(vec![inv], origin, Some(name.clone()), None) {
                    Ok(mut o) => o.remove(0),
                    Err(e) => {
                        // Not left behind empty: the document goes again (as the engine closes it).
                        if let Ok(next) = b.docs.successor(&id) {
                            let _ = super::docs::activate_document(b, &next, Origin::Ui);
                            let _ = b.docs.remove(&id, true);
                        }
                        return Err(e);
                    }
                };
                let result = json!({ "ok": true, "name": name, "result": o.result, "revision": o.revision });
                return Ok(Answer::with_status(201, &json!({ "ok": true, "id": id, "active": id, "existing": false, "revision": o.revision, "result": result })));
            }
            let inv = Invocation { id: "doc.load".into(), params: appcore::batch::content_params(text) };
            let o = b.batch_opening(vec![inv], origin, Some(name.clone()), None)?.remove(0);
            Ok(Answer::json(&json!({ "ok": true, "name": name, "result": o.result, "revision": o.revision })))
        }
        "template" => {
            let inv = match q.get("format").map_or("openetch", String::as_str) {
                "openetch" => Invocation { id: "doc.load".into(), params: appcore::batch::content_params(text) },
                "lbt" => Invocation { id: "doc.import_lbrn2".into(), params: json!({ "content": text, "mode": "replace" }) },
                other => return Err(AppError::BadRequest(format!("unknown format '{other}' (openetch or lbt)"))),
            };
            // A new, untitled document: the template is never the current project.
            let o = b.batch_opening(vec![inv], origin, None, None)?.remove(0);
            Ok(Answer::json(&json!({ "ok": true, "result": o.result, "revision": o.revision })))
        }
        "backup" => {
            let project = q.get("project").cloned();
            let inv = Invocation { id: "doc.load".into(), params: appcore::batch::content_params(text) };
            let o = b.batch_opening(vec![inv], origin, None, project)?.remove(0);
            Ok(Answer::json(&json!({ "ok": true, "result": o.result, "revision": o.revision })))
        }
        "recovery" => {
            let project = q.get("project").cloned();
            if let Some(p) = &project {
                appcore::project::validate_project_name(p)?;
            }
            let inv = Invocation { id: "doc.load".into(), params: appcore::batch::content_params(text) };
            let o = b.batch_opening(vec![inv], origin, None, project.clone().or_else(|| q.get("display").cloned()))?.remove(0);
            // Unsaved changes: it belongs to the project still (Save goes there), but no file holds this revision.
            b.project = ProjectState {
                name: project.clone(),
                saved_revision: None,
                clean_revision: None,
                display_name: if project.is_some() { None } else { q.get("display").cloned() },
            };
            Ok(Answer::json(&json!({ "ok": true, "result": o.result, "revision": o.revision })))
        }
        other => Err(AppError::BadRequest(format!("unknown kind '{other}' (project, template, backup or recovery)"))),
    }
}

fn saved(b: &mut Backend, q: &HashMap<String, String>) -> Routed {
    check_query(q, &["name", "revision", "document"])?;
    let name = q.get("name").cloned().ok_or_else(|| AppError::BadRequest("name is required".into()))?;
    appcore::project::validate_project_name(&name)?;
    let revision: u64 = q
        .get("revision")
        .and_then(|r| r.parse().ok())
        .ok_or_else(|| AppError::BadRequest("revision is required (X-OpenEtch-Revision of the download)".into()))?;
    let document = q.get("document").cloned().unwrap_or_else(|| b.docs.active.clone());
    // The document saved may have stopped being the active one while the shell stored the file.
    b.docs.set_project_of(&mut b.project, &document, ProjectState::saved(name.clone(), revision));
    b.emit(appcore::Event::ProjectSaved { name: name.clone(), revision, document_id: document });
    Ok(Answer::json(&json!({ "ok": true, "name": name, "revision": revision })))
}

/// A project drawn on its field (no toolpath), and the PNG width asked for.
fn thumbnail_svg(b: &Backend, q: &HashMap<String, String>, body: &[u8]) -> Result<(String, u32), AppError> {
    check_query(q, &["width"])?;
    let width = match q.get("width") {
        Some(w) => w.parse::<u32>().ok().filter(|w| (32..=1024).contains(w)).ok_or_else(|| AppError::BadRequest("width must be 32..=1024".into()))?,
        None => 256,
    };
    let text = std::str::from_utf8(body).map_err(|_| AppError::BadRequest("the project is not UTF-8 text".into()))?;
    let doc = doc::file::from_openetch_json(text).map_err(|e| AppError::BadRequest(format!("project: {e}")))?;
    let field = doc.field_mm.unwrap_or_else(|| b.profile_field());
    let svg = appcore::render::preview_svg(&doc, field, None, appcore::render::RenderOptions { show_jumps: false });
    Ok((svg, width))
}
