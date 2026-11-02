//! User Tools (LightBurn 2.2's Web Tools menu with its Manage User Tools window, OTH-019): menu entries the owner
//! adds, kept in the engine's preferences (`preferences.json` `user_tools`, so the PC and the iPad see the same
//! menu) and changed only through `PUT /api/prefs` `{user_tools}`, which is UI only (MCP clients and non-`ui` origins
//! are refused, as for every preference).
//!
//! An entry either opens a web page (LightBurn's User Tool: a name, a URL, an optional category for a submenu) or
//! runs a saved sequence of document commands (OpenEtch's addition: a macro of `{id, params}` steps, run by the web
//! app as one `POST /api/batch`). **A user tool can never act on the machine**: saving refuses any command that is not
//! a registered document command of risk `query` or `edit` (so no framing, pointer or emitting action), and the batch
//! endpoint refuses machine actions again when it runs. Approvals and runs are not commands at all, so no entry can
//! hold one. URLs must be `http:` or `https:` (no `javascript:` or `data:`), and the web app opens them in a new tab
//! without access to the app (`noopener`).

use commands::Risk;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Most entries, the longest name / category, the longest URL, the most steps of one sequence and the largest
/// parameters of one step (bytes of JSON).
pub const MAX_TOOLS: usize = 50;
pub const MAX_NAME_CHARS: usize = 40;
pub const MAX_URL_CHARS: usize = 2000;
pub const MAX_STEPS: usize = 50;
pub const MAX_PARAMS_BYTES: usize = 64 * 1024;

/// One step of a command sequence: a document command and its parameters (as `POST /api/execute` takes them).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolStep {
    pub id: String,
    #[serde(default = "empty_object")]
    pub params: Value,
}

fn empty_object() -> Value {
    Value::Object(Default::default())
}

/// What a user tool does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolAction {
    /// Open a web page (LightBurn's User Tool).
    Url { url: String },
    /// Run document commands in order, as one undo step each (one `POST /api/batch`).
    Commands { steps: Vec<ToolStep> },
}

/// A User Tools menu entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserTool {
    /// The label in the menu.
    pub name: String,
    /// Groups entries into a submenu (LightBurn's Category); empty: top level.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub category: String,
    #[serde(flatten)]
    pub action: ToolAction,
}

fn label_problem(what: &str, s: &str, required: bool) -> Option<String> {
    let n = s.trim().chars().count();
    if (required && n == 0) || s.chars().count() > MAX_NAME_CHARS || s.chars().any(char::is_control) {
        return Some(format!("{what} \"{s}\" must be {}..={MAX_NAME_CHARS} printable characters", u8::from(required)));
    }
    None
}

/// Why `url` cannot be a user tool's address, if it cannot.
fn url_problem(url: &str) -> Option<String> {
    if url.is_empty() || url.chars().count() > MAX_URL_CHARS || url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Some(format!("the URL must be 1..={MAX_URL_CHARS} characters without spaces"));
    }
    let lower = url.to_ascii_lowercase();
    let rest = lower.strip_prefix("https://").or_else(|| lower.strip_prefix("http://"));
    match rest {
        Some(r) if !r.is_empty() && !r.starts_with('/') => None,
        _ => Some(format!("\"{url}\" is not an http:// or https:// address")),
    }
}

/// Why `tools` cannot be saved, if they cannot. `risk_of` gives a document command's risk (`None`: no such command).
pub fn problem(tools: &[UserTool], risk_of: impl Fn(&str) -> Option<Risk>) -> Option<String> {
    if tools.len() > MAX_TOOLS {
        return Some(format!("at most {MAX_TOOLS} user tools"));
    }
    for t in tools {
        if let Some(p) = label_problem("tool name", &t.name, true).or_else(|| label_problem("category", &t.category, false)) {
            return Some(p);
        }
        match &t.action {
            ToolAction::Url { url } => {
                if let Some(p) = url_problem(url) {
                    return Some(format!("tool \"{}\": {p}", t.name));
                }
            }
            ToolAction::Commands { steps } => {
                if steps.is_empty() || steps.len() > MAX_STEPS {
                    return Some(format!("tool \"{}\" must have 1..={MAX_STEPS} steps", t.name));
                }
                for s in steps {
                    match risk_of(&s.id) {
                        None => return Some(format!("tool \"{}\": \"{}\" is not a document command", t.name, s.id)),
                        Some(Risk::Query | Risk::Edit) => {}
                        Some(_) => {
                            return Some(format!(
                                "tool \"{}\": \"{}\" acts on the machine; user tools may only run document commands",
                                t.name, s.id
                            ));
                        }
                    }
                    if !s.params.is_object() {
                        return Some(format!("tool \"{}\": the parameters of \"{}\" must be a JSON object", t.name, s.id));
                    }
                    if serde_json::to_vec(&s.params).map_or(usize::MAX, |b| b.len()) > MAX_PARAMS_BYTES {
                        return Some(format!("tool \"{}\": the parameters of \"{}\" are larger than {MAX_PARAMS_BYTES} bytes", t.name, s.id));
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn risk(id: &str) -> Option<Risk> {
        match id {
            "shape.rect" => Some(Risk::Edit),
            "doc.inspect" => Some(Risk::Query),
            "machine.frame" => Some(Risk::Pointer),
            "job.burn" => Some(Risk::Emit),
            _ => None,
        }
    }

    fn tools(v: serde_json::Value) -> Vec<UserTool> {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn urls_and_document_commands_only() {
        let ok = tools(json!([
            {"name": "Box maker", "category": "Generators", "kind": "url", "url": "https://example.com/box?x=1"},
            {"name": "Tag", "kind": "commands", "steps": [{"id": "shape.rect", "params": {"width_mm": 10, "height_mm": 5}}, {"id": "doc.inspect"}]},
        ]));
        assert_eq!(problem(&ok, risk), None);
        assert_eq!(ok[1].action, ToolAction::Commands { steps: vec![
            ToolStep { id: "shape.rect".into(), params: json!({"width_mm": 10, "height_mm": 5}) },
            ToolStep { id: "doc.inspect".into(), params: json!({}) },
        ] });
        // Round trip: the category is left out when empty.
        let v = serde_json::to_value(&ok).unwrap();
        assert_eq!(v[1], json!({"name": "Tag", "kind": "commands", "steps": [{"id": "shape.rect", "params": {"width_mm": 10, "height_mm": 5}}, {"id": "doc.inspect", "params": {}}]}));
        for (bad, why) in [
            (json!([{"name": "Fire", "kind": "commands", "steps": [{"id": "job.burn"}]}]), "acts on the machine"),
            (json!([{"name": "Frame", "kind": "commands", "steps": [{"id": "machine.frame"}]}]), "acts on the machine"),
            (json!([{"name": "X", "kind": "commands", "steps": [{"id": "nope"}]}]), "not a document command"),
            (json!([{"name": "X", "kind": "commands", "steps": []}]), "steps"),
            (json!([{"name": "X", "kind": "commands", "steps": [{"id": "shape.rect", "params": [1]}]}]), "JSON object"),
            (json!([{"name": "X", "kind": "url", "url": "javascript:alert(1)"}]), "http"),
            (json!([{"name": "X", "kind": "url", "url": "data:text/html,hi"}]), "http"),
            (json!([{"name": "X", "kind": "url", "url": "https://"}]), "http"),
            (json!([{"name": "X", "kind": "url", "url": "https://a b"}]), "spaces"),
            (json!([{"name": " ", "kind": "url", "url": "https://a"}]), "tool name"),
            (json!([{"name": "X\n", "kind": "url", "url": "https://a"}]), "tool name"),
        ] {
            let p = problem(&tools(bad.clone()), risk).unwrap_or_else(|| panic!("{bad} should be refused"));
            assert!(p.contains(why), "{bad}: {p}");
        }
        assert!(serde_json::from_value::<Vec<UserTool>>(json!([{"name": "X", "kind": "run_job"}])).is_err(), "unknown kinds do not load");
        let many: Vec<UserTool> = (0..=MAX_TOOLS).map(|i| UserTool { name: format!("t{i}"), category: String::new(), action: ToolAction::Url { url: "https://a".into() } }).collect();
        assert!(problem(&many, risk).is_some());
    }
}
