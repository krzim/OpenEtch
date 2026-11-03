//! Data the browser moves: document change sets (`GET /api/document/sync`, `sync` in mutating answers), compressed
//! API answers and the binary preview (`GET /api/preview.bin`, `GET /api/preview/stats`).

use std::collections::HashMap;
use std::io::Read;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::tests::test_state;
use crate::{AppState, api};

fn app(st: &std::sync::Arc<AppState>) -> axum::Router {
    api::router(st.clone(), None)
}

fn req(method: &str, uri: &str, body: Option<Value>, headers: &[(&str, &str)]) -> Request<Body> {
    let mut b = Request::builder().method(method).uri(uri).header("host", "localhost:7878");
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    match body {
        Some(v) => b.header("content-type", "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    }
}

/// Status, headers and the body (decompressed).
async fn send_raw(app: &axum::Router, r: Request<Body>) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let res = app.clone().oneshot(r).await.unwrap();
    let (status, headers) = (res.status(), res.headers().clone());
    let bytes = res.into_body().collect().await.unwrap().to_bytes().to_vec();
    let body = match headers.get("content-encoding").and_then(|v| v.to_str().ok()) {
        Some("br") => {
            let mut out = Vec::new();
            brotli::BrotliDecompress(&mut bytes.as_slice(), &mut out).unwrap();
            out
        }
        Some("gzip") => {
            let mut out = Vec::new();
            flate2::read::GzDecoder::new(bytes.as_slice()).read_to_end(&mut out).unwrap();
            out
        }
        Some(other) => panic!("unexpected encoding {other}"),
        None => bytes,
    };
    (status, headers, body)
}

async fn send(app: &axum::Router, r: Request<Body>) -> Value {
    let (s, _, b) = send_raw(app, r).await;
    assert!(s.is_success(), "{s}: {}", String::from_utf8_lossy(&b));
    serde_json::from_slice(&b).unwrap()
}

/// The web app's side of a change set (web/src/api/docSync.ts does the same).
fn apply(doc: &Value, changes: &Value) -> Value {
    assert_eq!(doc["revision"], changes["from"], "{changes}");
    let mut out = doc.as_object().unwrap().clone();
    for k in changes["unset"].as_array().unwrap() {
        out.remove(k.as_str().unwrap());
    }
    for (k, v) in changes["set"].as_object().unwrap() {
        out.insert(k.clone(), v.clone());
    }
    let mut by_id: HashMap<String, Value> =
        doc["objects"].as_array().unwrap().iter().map(|o| (o["id"].as_str().unwrap().to_owned(), o.clone())).collect();
    for id in changes["objects"]["remove"].as_array().unwrap() {
        by_id.remove(id.as_str().unwrap());
    }
    for o in changes["objects"]["upsert"].as_array().unwrap() {
        by_id.insert(o["id"].as_str().unwrap().to_owned(), o.clone());
    }
    let order: Vec<String> = match changes["objects"].get("order") {
        Some(o) => o.as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect(),
        None => doc["objects"].as_array().unwrap().iter().map(|o| o["id"].as_str().unwrap().to_owned()).collect(),
    };
    out.insert("objects".into(), Value::Array(order.iter().map(|id| by_id[id].clone()).collect()));
    Value::Object(out)
}

/// Apply a sync payload to the client's copy (or take the whole document).
fn follow(doc: &mut Value, sync: &Value) {
    if let Some(d) = sync.get("document") {
        *doc = d.clone();
    } else {
        *doc = apply(doc, &sync["changes"]);
    }
}

#[tokio::test]
async fn edits_answer_with_the_changes_the_client_needs() {
    let st = test_state().await;
    let app = app(&st);
    // First contact: the whole document, the timeline and the project.
    let first = send(&app, req("GET", "/api/document/sync", None, &[])).await;
    let epoch = first["epoch"].as_str().unwrap().to_owned();
    assert!(first["document"].is_object() && first["changes"].is_null());
    assert!(first["history"]["steps"].is_array() && first["project"]["dirty"].is_boolean());
    let mut doc = first["document"].clone();

    let since = |doc: &Value| format!("{epoch}:{}", doc["revision"]);
    for (id, params) in [
        ("shape.rect", json!({"width_mm": 10, "height_mm": 5})),
        ("shape.ellipse", json!({"rx_mm": 4, "ry_mm": 4})),
        ("object.transform", json!({"ids": ["obj-2"], "translate": [3, 4]})),
        ("layer.update", json!({"id": "layer-1", "settings": {"power_pct": 33}})),
        ("object.delete", json!({"ids": ["obj-2"]})),
    ] {
        let r = send(&app, req("POST", "/api/execute", Some(json!({"id": id, "params": params})), &[(api::SYNC_HEADER, &since(&doc))])).await;
        assert_eq!(r["ok"], true, "{id}: {r}");
        let sync = &r["sync"];
        assert!(sync["document"].is_null(), "{id}: a known revision gets changes, not the document");
        assert_eq!(sync["revision"], r["revision"]);
        follow(&mut doc, sync);
        assert_eq!(doc, send(&app, req("GET", "/api/document", None, &[])).await, "{id}");
        assert_eq!(sync["history"], serde_json::to_value(st.history_view()).unwrap());
    }
    // Undo, redo and the timeline answer with changes too.
    for (path, body) in [("/api/undo", json!({})), ("/api/undo", json!({})), ("/api/redo", json!({})), ("/api/history/goto", json!({"marker": 1}))] {
        let r = send(&app, req("POST", path, Some(body), &[(api::SYNC_HEADER, &since(&doc))])).await;
        follow(&mut doc, &r["sync"]);
        assert_eq!(doc, send(&app, req("GET", "/api/document", None, &[])).await, "{path}");
    }
    // A client several edits behind (edits by someone else) still gets one merged change set.
    let behind = doc.clone();
    for w in [4, 5, 6] {
        send(&app, req("POST", "/api/execute", Some(json!({"id": "shape.rect", "params": {"width_mm": w, "height_mm": 2}})), &[])).await;
    }
    let r = send(&app, req("GET", &format!("/api/document/sync?since={}&epoch={epoch}", behind["revision"]), None, &[])).await;
    assert!(r["changes"].is_object());
    let mut caught_up = behind.clone();
    follow(&mut caught_up, &r);
    assert_eq!(caught_up, send(&app, req("GET", "/api/document", None, &[])).await);
    // Another engine run's revision, or one the log never had, gets the whole document.
    for q in [format!("since={}&epoch=other", behind["revision"]), format!("since=9999&epoch={epoch}")] {
        let r = send(&app, req("GET", &format!("/api/document/sync?{q}"), None, &[])).await;
        assert!(r["document"].is_object(), "{q}");
    }
    // Without the header nothing is added; a failed command still answers ok: false.
    let r = send(&app, req("POST", "/api/execute", Some(json!({"id": "shape.rect", "params": {"width_mm": 1, "height_mm": 1}})), &[])).await;
    assert!(r.get("sync").is_none());
    let r = send(&app, req("POST", "/api/execute", Some(json!({"id": "object.delete", "params": {"ids": ["nope"]}})), &[(api::SYNC_HEADER, &since(&doc))])).await;
    assert_eq!(r["ok"], false);
}

#[test]
fn members_are_spliced_into_objects() {
    assert_eq!(api::splice_member(br#"{"ok":true}"#, "sync", b"1").unwrap(), br#"{"ok":true,"sync":1}"#);
    assert_eq!(api::splice_member(b" { } ", "sync", b"[]").unwrap(), br#" { "sync":[]}"#);
    assert!(api::splice_member(b"[1]", "sync", b"1").is_none());
    assert!(api::splice_member(b"", "sync", b"1").is_none());
}

#[tokio::test]
async fn large_answers_are_compressed_small_ones_are_not() {
    let st = test_state().await;
    let app = app(&st);
    for i in 0..40 {
        send(&app, req("POST", "/api/execute", Some(json!({"id": "shape.rect", "params": {"width_mm": 1 + i, "height_mm": 2}})), &[])).await;
    }
    let plain = send_raw(&app, req("GET", "/api/document", None, &[])).await;
    assert!(plain.1.get("content-encoding").is_none());
    for enc in ["br", "gzip"] {
        let (s, h, body) = send_raw(&app, req("GET", "/api/document", None, &[("accept-encoding", enc)])).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(h["content-encoding"], enc);
        assert_eq!(body, plain.2, "{enc} decompresses to the same document");
    }
    let (_, h, _) = send_raw(&app, req("GET", "/api/project", None, &[("accept-encoding", "br")])).await;
    assert!(h.get("content-encoding").is_none(), "a small answer is sent as it is");
}

#[tokio::test]
async fn the_binary_preview_matches_the_json_one_and_is_planned_once() {
    let st = test_state().await;
    let app = app(&st);
    send(&app, req("POST", "/api/execute", Some(json!({"id": "shape.rect", "params": {"width_mm": 20, "height_mm": 10}})), &[])).await;
    send(&app, req("POST", "/api/execute", Some(json!({"id": "shape.ellipse", "params": {"rx_mm": 6, "ry_mm": 3}})), &[])).await;
    let json_preview = send(&app, req("GET", "/api/preview", None, &[])).await;
    let planned = st.previews_computed();
    for enc in [None, Some("br"), Some("gzip")] {
        let headers: Vec<(&str, &str)> = enc.map(|e| ("accept-encoding", e)).into_iter().collect();
        let (s, h, body) = send_raw(&app, req("GET", "/api/preview.bin", None, &headers)).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(h.get("content-encoding").and_then(|v| v.to_str().ok()), enc);
        assert_eq!(h[api::REVISION_HEADER], st.revision().to_string());
        let (header, segs) = crate::preview_bin::decode(&body);
        assert_eq!(header["stats"], json_preview["stats"]);
        assert_eq!(header["passes"], json_preview["passes"]);
        assert_eq!(header["layers"], json_preview["layers"]);
        let want = json_preview["segments"].as_array().unwrap();
        assert_eq!(segs.len(), want.len());
        for (a, b) in segs.iter().zip(want) {
            for k in ["kind", "emits", "pass", "layer"] {
                assert_eq!(a[k], b[k]);
            }
            for k in ["from", "to"] {
                for i in 0..2 {
                    assert!((a[k][i].as_f64().unwrap() - b[k][i].as_f64().unwrap()).abs() < 1e-4, "{a} vs {b}");
                }
            }
            assert!((a["t_s"].as_f64().unwrap() - b["t_s"].as_f64().unwrap()).abs() < 1e-4);
        }
    }
    let stats = send(&app, req("GET", "/api/preview/stats", None, &[])).await;
    assert_eq!(stats["stats"], json_preview["stats"]);
    assert_eq!(stats["segments"], json!(json_preview["segments"].as_array().unwrap().len()));
    assert_eq!(stats["revision"], json!(st.revision()));
    assert!(stats.get("segments_list").is_none());
    assert_eq!(st.previews_computed(), planned, "the JSON, binary and stats answers of one revision share one plan");
    // Options: no jumps, a level of detail, bad values refused.
    let (_, _, body) = send_raw(&app, req("GET", "/api/preview.bin?jumps=false&lod=0.5", None, &[])).await;
    let (header, segs) = crate::preview_bin::decode(&body);
    assert_eq!(header["jumps"], false);
    assert!(segs.iter().all(|s| s["kind"] == "mark"));
    let (s, _, _) = send_raw(&app, req("GET", "/api/preview.bin?lod=-1", None, &[])).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // An edit plans again.
    send(&app, req("POST", "/api/execute", Some(json!({"id": "object.transform", "params": {"ids": ["obj-2"], "translate": [200, 0]}})), &[])).await;
    let (s, _, _) = send_raw(&app, req("GET", "/api/preview.bin", None, &[])).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "a plan error is the JSON preview's 422");
    assert_eq!(st.previews_computed(), planned + 1);
    let (s, _, _) = send_raw(&app, req("GET", "/api/preview/stats", None, &[])).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(st.previews_computed(), planned + 1);
}
