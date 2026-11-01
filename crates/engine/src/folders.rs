//! Project folders and locations (UX-013; module docs of [`crate::projects`]). Routes are merged into
//! `api::router`.
//!
//! | Method | Path | Body / result |
//! |---|---|---|
//! | GET | `/api/projects/folders` | `{folders: ["Clients", "Clients/Acme", "NAS", ...], locations}` (locations appear as top folders) |
//! | POST | `/api/projects/folders` | `{name}` (a folder path, at most 4 deep) → `{ok, name}`; creates it and the folders above it |
//! | GET | `/api/projects/locations` | `{locations: [{label, path, source: "cli"\|"settings", reachable}]}` |
//! | PUT | `/api/projects/locations` | `{locations: [{label, path}], origin?}` (UI only): replaces the saved locations (`<config-dir>/project-locations.json`); `--projects-mount` ones stay. Each path must be an existing absolute directory |

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::projects::{self, Location};
use crate::security::has_mcp_marker;
use crate::state::parse_origin;
use crate::{AppState, EngineError};

/// The saved locations in the config directory.
pub const LOCATIONS_FILE: &str = "project-locations.json";

/// Mount the command line's `--projects-mount` locations and the saved ones (`<config-dir>/project-locations.json`).
/// Returns notes for the console (a saved location that is not usable is left out, and said so).
pub fn open(st: &AppState, config_dir: &Path, cli: &[(String, PathBuf)]) -> Vec<String> {
    let mut notes = Vec::new();
    let mut locs: Vec<Location> = cli
        .iter()
        .map(|(label, path)| Location { label: label.clone(), path: path.clone(), source: "cli".into() })
        .collect();
    if let Err(e) = projects::validate_locations(&locs) {
        notes.push(format!("--projects-mount: {e}; no locations mounted from the command line"));
        locs.clear();
    }
    let path = config_dir.join(LOCATIONS_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<Vec<Location>>(&text) {
            Ok(saved) => {
                for mut l in saved {
                    l.source = "settings".into();
                    let mut next = locs.clone();
                    next.push(l.clone());
                    match projects::validate_locations(&next) {
                        Ok(()) => locs = next,
                        Err(e) => notes.push(format!("{}: location \"{}\" left out: {e}", path.display(), l.label)),
                    }
                }
            }
            Err(e) => notes.push(format!("{}: not usable ({e}); no saved locations", path.display())),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => notes.push(format!("{}: {e}", path.display())),
    }
    for l in &locs {
        tracing::info!(label = %l.label, path = %l.path.display(), source = %l.source, "projects location");
    }
    st.projects.set_locations(locs);
    notes
}

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/projects/folders", get(folders).post(create_folder))
        .route("/projects/locations", get(locations).put(set_locations))
}

fn locations_json(st: &AppState) -> Value {
    let locs: Vec<Value> = st
        .projects
        .locations()
        .into_iter()
        .map(|l| json!({ "label": l.label, "path": l.path.display().to_string(), "source": l.source, "reachable": l.path.is_dir() }))
        .collect();
    json!(locs)
}

async fn folders(State(st): State<Arc<AppState>>) -> Result<Json<Value>, EngineError> {
    let dir = st.projects.clone();
    let list = tokio::task::spawn_blocking(move || dir.folders()).await??;
    Ok(Json(json!({ "folders": list, "locations": locations_json(&st) })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FolderBody {
    name: String,
    #[serde(default)]
    #[allow(dead_code)]
    origin: Option<Value>,
}

async fn create_folder(State(st): State<Arc<AppState>>, body: Bytes) -> Result<Json<Value>, EngineError> {
    let b: FolderBody = serde_json::from_slice(&body).map_err(|e| EngineError::BadRequest(format!("bad JSON body: {e}")))?;
    let dir = st.projects.clone();
    let name = b.name.clone();
    tokio::task::spawn_blocking(move || dir.create_folder(&name)).await??;
    Ok(Json(json!({ "ok": true, "name": b.name })))
}

async fn locations(State(st): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({ "locations": locations_json(&st) }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LocationIn {
    label: String,
    path: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LocationsBody {
    locations: Vec<LocationIn>,
    #[serde(default)]
    origin: Option<Value>,
}

async fn set_locations(State(st): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> Result<Json<Value>, EngineError> {
    let b: LocationsBody = serde_json::from_slice(&body).map_err(|e| EngineError::BadRequest(format!("bad JSON body: {e}")))?;
    // Which directories the engine reads and writes is the operator's choice, never an agent's.
    if has_mcp_marker(&headers) || parse_origin(b.origin.as_ref())? != commands::Origin::Ui {
        return Err(EngineError::Forbidden("project locations are set only by a person in the OpenEtch UI".into()));
    }
    let fixed: Vec<Location> = st.projects.locations().into_iter().filter(|l| l.source == "cli").collect();
    let saved: Vec<Location> = b
        .locations
        .into_iter()
        .map(|l| Location { label: l.label.trim().to_owned(), path: l.path, source: "settings".into() })
        .collect();
    let mut all = fixed.clone();
    all.extend(saved.iter().cloned());
    projects::validate_locations(&all)?;
    let config_dir = st.files.config_dir()?;
    let text = serde_json::to_string_pretty(&saved).map_err(|e| EngineError::Internal(e.to_string()))?;
    crate::devices::write_atomic(&config_dir, LOCATIONS_FILE, &text).map_err(EngineError::Internal)?;
    st.projects.set_locations(all);
    tracing::info!(count = saved.len(), "project locations saved");
    Ok(Json(json!({ "ok": true, "locations": locations_json(&st) })))
}
