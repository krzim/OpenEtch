//! Font favourites and hidden fonts (LightBurn 2.1's Font Manager), kept in `<config-dir>/font-prefs.json` so the
//! PC, the iPad and the phone see the same list. The font picker puts favourites first and leaves hidden fonts out
//! (a "show hidden" switch brings them back). Changed only from the UI, like the other preferences. Routes are
//! merged into `api::router`.
//!
//! | Method | Path | Body / result |
//! |---|---|---|
//! | GET | `/api/fonts/prefs` | `{favourites: [{family, kind}], hidden: [{family, kind}]}` |
//! | PUT | `/api/fonts/prefs` | the same (each list replaced when given; at most 2000 fonts each, a font in one list only) → the stored lists |

use std::sync::{Arc, Mutex, PoisonError};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::security::has_mcp_marker;
use crate::state::parse_origin;
use crate::{AppState, EngineError};

/// The file in the config directory.
pub const FONT_PREFS_FILE: &str = "font-prefs.json";
// The lists and their rules (shared with the browser core).
pub use appcore::fonts::{FontPrefs, FontRef, MAX_FONTS};

/// Serialises read-modify-write of the file.
static WRITE: Mutex<()> = Mutex::new(());

fn path(st: &AppState) -> Result<std::path::PathBuf, EngineError> {
    Ok(st.files.config_dir()?.join(FONT_PREFS_FILE))
}

/// The stored lists (empty when there is no file yet).
pub fn load(st: &AppState) -> Result<FontPrefs, EngineError> {
    let p = path(st)?;
    match std::fs::read_to_string(&p) {
        Ok(t) => serde_json::from_str(&t).map_err(|e| EngineError::Internal(format!("{} is not usable ({e}); fix or remove it", p.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(FontPrefs::default()),
        Err(e) => Err(EngineError::Internal(format!("{}: {e}", p.display()))),
    }
}

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/fonts/prefs", get(view).put(edit))
}

async fn view(State(st): State<Arc<AppState>>) -> Result<Json<Value>, EngineError> {
    Ok(Json(json!(tokio::task::spawn_blocking(move || load(&st)).await.map_err(|e| EngineError::Internal(e.to_string()))??)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    favourites: Option<Vec<FontRef>>,
    hidden: Option<Vec<FontRef>>,
    #[serde(default)]
    origin: Option<Value>,
}

async fn edit(State(st): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> Result<Json<Value>, EngineError> {
    let b: Body = serde_json::from_slice(&body).map_err(|e| EngineError::BadRequest(format!("bad JSON body: {e}")))?;
    if has_mcp_marker(&headers) || parse_origin(b.origin.as_ref())? != commands::Origin::Ui {
        return Err(EngineError::Forbidden("font favourites are changed only by a person in the OpenEtch UI".into()));
    }
    let out = tokio::task::spawn_blocking(move || {
        let _g = WRITE.lock().unwrap_or_else(PoisonError::into_inner);
        let cur = load(&st)?;
        let next = FontPrefs { favourites: b.favourites.unwrap_or(cur.favourites), hidden: b.hidden.unwrap_or(cur.hidden) }.clean()?;
        let p = path(&st)?;
        let text = serde_json::to_string_pretty(&next).map_err(|e| EngineError::Internal(e.to_string()))?;
        let tmp = p.with_extension("json.tmp");
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| EngineError::Internal(format!("{}: {e}", dir.display())))?;
        }
        std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, &p)).map_err(|e| EngineError::Internal(format!("{}: {e}", p.display())))?;
        Ok::<_, EngineError>(next)
    })
    .await
    .map_err(|e| EngineError::Internal(e.to_string()))??;
    Ok(Json(json!(out)))
}

#[cfg(test)]
mod tests {
    use axum::body::Body as HttpBody;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use serde_json::json;
    use tower::ServiceExt;

    use crate::tests::test_state;
    use crate::{api, pin};

    async fn send(app: &axum::Router, method: &str, body: Option<serde_json::Value>, mcp: bool) -> (StatusCode, serde_json::Value) {
        let mut b = Request::builder().method(method).uri("/api/fonts/prefs").header("host", "localhost:7878");
        if mcp {
            b = b.header("mcp-session-id", "x");
        }
        let r = match body {
            Some(v) => b.header("content-type", "application/json").body(HttpBody::from(v.to_string())).unwrap(),
            None => b.body(HttpBody::empty()).unwrap(),
        };
        let res = app.clone().oneshot(r).await.unwrap();
        let s = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (s, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    #[tokio::test]
    async fn favourites_and_hidden_fonts_are_stored_cleaned_and_ui_only() {
        let st = test_state().await;
        let _ = st.files.open(&pin::temp_dir("font-prefs"));
        let app = api::router(st.clone(), None);
        let (s, v) = send(&app, "GET", None, false).await;
        assert_eq!((s, v), (StatusCode::OK, json!({"favourites": [], "hidden": []})));
        let body = json!({"favourites": [{"family": " Hershey Script ", "kind": "stroke"}, {"family": "hershey script", "kind": "stroke"}],
            "hidden": [{"family": "Wingdings"}, {"family": "Hershey Script", "kind": "stroke"}]});
        let (s, v) = send(&app, "PUT", Some(body), false).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v, json!({"favourites": [{"family": "Hershey Script", "kind": "stroke"}], "hidden": [{"family": "Wingdings", "kind": "system"}]}));
        // Only the list given changes; the file survives.
        let (_, v) = send(&app, "PUT", Some(json!({"hidden": []})), false).await;
        assert_eq!(v["favourites"].as_array().unwrap().len(), 1);
        let (_, v) = send(&app, "GET", None, false).await;
        assert_eq!(v["hidden"], json!([]));
        let (s, _) = send(&app, "PUT", Some(json!({"hidden": []})), true).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "agents do not change preferences");
        let (s, _) = send(&app, "PUT", Some(json!({"hidden": [{"family": ""}]})), false).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
    }
}
