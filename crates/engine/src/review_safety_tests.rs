//! Safety review of the production, jig, materials, text and ledger features: what a re-mark, a hostile finish file
//! and the hour meter must never do.

use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{Value, json};

use crate::approve::tests::{browser, pedal_profile, req, send, state, wait_final};
use crate::pin::ApprovalMode;
use crate::runs::RunState;
use crate::api;

async fn exec(app: &axum::Router, id: &str, params: Value) -> Value {
    let (s, v) = send(app, req("POST", "/api/execute", Some(json!({"id": id, "params": params, "origin": "ui"})))).await;
    assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(true)), "{id}: {v}");
    v["result"].clone()
}

/// A re-mark is planned like every run of the device: one that marks inside a device keep-out zone is refused.
#[tokio::test]
async fn a_re_mark_is_refused_inside_a_device_keep_out_zone() {
    let st = state(pedal_profile(), ApprovalMode::Click).await;
    let app = api::router(st.clone(), None);
    for k in 0..3 {
        exec(
            &app,
            "shape.text",
            json!({"text": "0dd", "font": {"family": "Hershey Sans", "kind": "stroke"}, "height_mm": 4,
                   "center": [-30 + 30 * k, 0], "variable": {"mode": "serial", "offset": k}}),
        )
        .await;
    }
    exec(&app, "var.set_serial", json!({"start": 1, "end": 99, "current": 7, "auto_advance": true})).await;
    let rev = send(&app, req("GET", "/api/document", None)).await.1["revision"].as_u64().unwrap();
    let (s, v) = send(&app, browser(req("POST", "/api/job/start", Some(json!({"origin": "ui", "revision": rev}))))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let id = v["id"].as_str().unwrap().to_owned();
    assert_eq!(wait_final(&st, &id).await.state, RunState::Finished);
    // A clamp over slot 3's text, set in the app after the burn.
    let zone = json!([{"name": "Clamp", "points": [[24, -6], [36, -6], [36, 6], [24, 6]]}]);
    let (s, v) = send(&app, req("PUT", "/api/prefs", Some(json!({"keep_out": zone})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let pending = st.runs().views(0).len();
    let (s, v) = send(&app, req("POST", "/api/production/remark", Some(json!({"run_id": id, "slot": 3, "origin": "ui"})))).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert!(v["error"].as_str().unwrap().contains("Clamp (device)"), "{v}");
    assert_eq!(st.runs().views(0).len(), pending, "nothing was requested");
    // Slot 1 is away from the clamp: its re-mark is still a plain pending run.
    let (s, v) = send(&app, req("POST", "/api/production/remark", Some(json!({"run_id": id, "slot": 1, "origin": "ui"})))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let remark = v["id"].as_str().unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(st.runs().get(remark).unwrap().state, RunState::Pending);
}

/// A finish file whose photo claims a type a browser would run is refused (the photo is served with its type).
#[test]
fn a_finish_photo_must_be_an_image() {
    let photo = |mime: &str, average: &str| {
        json!({"format": "openetch-finish", "version": 1, "entries": {"Steel/Black": {"photo": {
            "mime": mime, "data_base64": "PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==", "width": 1, "height": 1, "average": average
        }}}})
        .to_string()
    };
    for (mime, average) in [("text/html", "#000000"), ("image/svg+xml", "#000000"), ("image/png\r\nX-A: b", "#000000"), ("image/png", "<b>")] {
        let e = crate::finish::parse_sidecar(photo(mime, average).as_bytes()).unwrap_err();
        assert!(e.to_string().contains("photo"), "{mime} {average}: {e}");
    }
    assert!(crate::finish::parse_sidecar(photo("image/png", "#102030").as_bytes()).is_ok());
}
