//! Built-in document commands.

use doc::Document;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{Command, Registry, Result, Risk};

pub(crate) fn register_all(r: &mut Registry) {
    r.register::<Inspect>();
}

#[derive(Deserialize, JsonSchema)]
pub struct InspectInput {}

/// Placeholder until the full command set lands.
pub struct Inspect;

impl Command for Inspect {
    type Input = InspectInput;
    const ID: &'static str = "doc.inspect";
    const TITLE: &'static str = "Inspect document";
    const DESCRIPTION: &'static str = "Summarise the document.";
    const RISK: Risk = Risk::Query;

    fn apply(doc: &mut Document, _input: InspectInput) -> Result<(Value, String)> {
        Ok((json!({ "revision": doc.revision, "objects": doc.objects.len() }), String::new()))
    }
}
