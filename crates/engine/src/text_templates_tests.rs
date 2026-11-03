//! Engine-level tests of the example projects, template fields and the contact sheet: what the web UI and MCP
//! agents reach (`/api/execute`, the MCP `execute` tool). Agents may design (open an example, fill a template, run
//! the contact sheet check) and never approve or start a run.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use rmcp::model::{CallToolResult, JsonObject};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::mcp::GalvoMcp;
use crate::tests::test_state;
use crate::{AppState, api};

fn app(st: &Arc<AppState>) -> Router {
    api::router(st.clone(), None)
}

async fn execute(app: &Router, id: &str, params: Value) -> (StatusCode, Value) {
    let r = Request::builder()
        .method("POST")
        .uri("/api/execute")
        .header("host", "localhost:7878")
        .header("content-type", "application/json")
        .body(Body::from(json!({ "id": id, "params": params }).to_string()))
        .unwrap();
    let res = app.clone().oneshot(r).await.unwrap();
    let s = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (s, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn text(r: &CallToolResult) -> Value {
    let t = r.content.iter().find_map(|c| c.as_text().map(|t| t.text.clone())).unwrap_or_default();
    serde_json::from_str(&t).unwrap_or(Value::String(t))
}

fn obj(v: Value) -> JsonObject {
    v.as_object().cloned().unwrap_or_default()
}

#[tokio::test]
async fn an_example_opens_in_the_active_devices_field_as_a_clean_new_document() {
    let st = test_state().await;
    let app = app(&st);
    let (s, v) = execute(&app, "example.list", json!({})).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let ids: Vec<&str> = v["result"].as_array().unwrap().iter().map(|e| e["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"two-tone-coin") && ids.len() >= 5, "{ids:?}");
    let (s, v) = execute(&app, "doc.new", json!({ "example": "two-tone-coin" })).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["ok"], true, "{v}");
    let d = st.document();
    assert_eq!(d.name, "Two-tone MOPA coin");
    assert_eq!(d.field_mm, Some(st.machine.profile().field_mm), "laid out for the active device");
    assert!(!st.project_status()["dirty"].as_bool().unwrap_or(true), "an example opens clean, like New");
    assert_eq!(st.history().len(), 1, "a fresh timeline");
}

#[tokio::test]
async fn agents_fill_templates_and_check_rows_but_cannot_start_anything() {
    let st = test_state().await;
    let mcp = GalvoMcp::new(st.clone());
    let r = mcp.call("execute", obj(json!({ "id": "doc.new", "params": { "example": "two-tone-coin" } }))).await;
    assert_eq!(r.is_error, Some(false), "{}", text(&r));
    let r = mcp.call("execute", obj(json!({ "id": "template.fields", "params": {} }))).await;
    let v = text(&r);
    assert_eq!(v["result"]["fields"][0]["id"], "name", "{v}");
    let r = mcp.call("execute", obj(json!({ "id": "template.fill", "params": { "values": { "name": "Lin", "ring": "Black", "date": "2026-12-24" } } }))).await;
    assert_eq!(r.is_error, Some(false), "{}", text(&r));
    let v = text(&r);
    assert_eq!(v["result"]["overflow"], json!([]), "{v}");
    let d = st.document();
    let name = d.objects.iter().find(|o| o.name == "Name").expect("the name field's text keeps its given name");
    assert!(matches!(&name.geometry, doc::Geometry::Text(t) if t.text == "Lin"));
    assert!(d.objects.iter().any(|o| matches!(&o.geometry, doc::Geometry::Text(t) if t.text == "24.12.2026")));
    assert_eq!(st.history().last().unwrap().origin, commands::Origin::Agent(crate::mcp::MCP_AGENT.into()));
    // Filling in requested no run.
    assert!(st.runs().views(0).is_empty(), "nothing was queued");

    // The tag set: the contact sheet is a check an agent runs before asking for a run.
    mcp.call("execute", obj(json!({ "id": "doc.new", "params": { "example": "serial-tags" } }))).await;
    let r = mcp.call("execute", obj(json!({ "id": "var.contact_sheet", "params": { "count": 500 } }))).await;
    let v = text(&r);
    assert_eq!(v["result"]["total"], 10, "ten jig loads of six tags: {v}");
    assert_eq!(v["result"]["summary"]["problems"], json!({}), "{v}");
    // Every template and sheet command is a design (Edit) or Query command, never a machine action.
    let r = mcp.call("list_commands", JsonObject::new()).await;
    let list = text(&r);
    let all = list.as_array().or_else(|| list["commands"].as_array()).cloned().unwrap_or_default();
    for id in ["template.fill", "template.set_field", "template.fields", "var.contact_sheet", "font.preview", "example.list"] {
        let c = all.iter().find(|c| c["id"] == id).unwrap_or_else(|| panic!("{id} is listed: {list}"));
        assert!(matches!(c["risk"].as_str(), Some("edit" | "query")), "{id}: {c}");
    }
}
