//! Areas applied to every planned run before it is encoded (short list #8 and #9, see ARCHITECTURE.md "Jigs and
//! placement"):
//!
//! - **Re-mark a region** (`RunOptions::region`): the planned toolpath, not the design, is clipped to a polygon
//!   (`plan::zones::clip_to_region`), together with Cut Selected Graphics when there is a selection. The run is an
//!   ordinary run: it is requested and approved like any other (`approve` alone arms it).
//! - **Keep-out zones**: the enabled zones of the document's jigs and of the active device (`DevicePrefs::keep_out`,
//!   set in the app only: `PUT /api/prefs`, which refuses MCP and other origins). A run that marks inside one is
//!   refused when it is planned (`PlanError::KeepOut`), whoever asked for it: Start, the foot switch, a pending run
//!   requested by an agent. Travel (laser off) never counts.

use doc::Document;
use plan::{JobPlan, PlanError, Target};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::EngineError;

// The device zone type and its checks (shared with the browser core).
pub use appcore::zones::{DeviceZone, MAX_DEVICE_ZONES, MAX_POINTS, device_zones_problem, polygon_problem};

/// The active device's keep-out zones (`DevicePrefs::keep_out`).
pub fn device_zones(st: &crate::state::AppState) -> Vec<DeviceZone> {
    let id = st.active_device().id;
    let prefs = st.libs.prefs.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    prefs.device(&id).keep_out
}

/// Every enabled keep-out zone that applies to `doc` on this device: the jigs' (flattened) and the device's.
pub fn zones_for(doc: &Document, device: &[DeviceZone]) -> Vec<(String, Vec<[f64; 2]>)> {
    let mut out: Vec<(String, Vec<[f64; 2]>)> = Vec::new();
    for (name, path) in doc::jig::document_keep_outs(doc) {
        for ring in rings(&path) {
            out.push((name.clone(), ring));
        }
    }
    out.extend(device.iter().filter(|z| z.enabled).map(|z| (format!("{} (device)", z.name), z.points.clone())));
    out
}

/// The subpaths of `path` flattened to 0.01 mm, each as a ring (a zone is closed whether or not it ends in
/// ClosePath); rings of fewer than 3 points are dropped.
fn rings(path: &kurbo::BezPath) -> Vec<Vec<[f64; 2]>> {
    let mut out = Vec::new();
    let mut cur: Vec<[f64; 2]> = Vec::new();
    kurbo::flatten(path, 0.01, |el| match el {
        kurbo::PathEl::MoveTo(p) => {
            out.push(std::mem::take(&mut cur));
            cur.push([p.x, p.y]);
        }
        kurbo::PathEl::LineTo(p) => cur.push([p.x, p.y]),
        kurbo::PathEl::ClosePath => out.push(std::mem::take(&mut cur)),
        _ => {}
    });
    out.push(cur);
    out.retain(|r| r.len() >= 3);
    out
}

/// Clip `job` to `region` (when given), then refuse it when a mark lies in a keep-out zone of `doc` or `device`.
pub fn apply(job: &mut JobPlan, doc: &Document, region: Option<&[[f64; 2]]>, device: &[DeviceZone], target: &Target) -> Result<(), EngineError> {
    if let Some(r) = region {
        *job = plan::zones::clip_to_region(job, r, target)?;
    }
    let zones = zones_for(doc, device);
    if let Some(hit) = plan::zones::keep_out_hit(job, &zones) {
        let layer = doc.layer(&hit.layer).map_or(hit.layer.clone(), |l| format!("\"{}\"", l.name));
        return Err(PlanError::KeepOut { zone: hit.zone, layer, point: hit.point }.into());
    }
    Ok(())
}

/// A pocket named by its number (3) or id ("p3").
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum PocketRef {
    Number(u32),
    Id(String),
}

/// Pockets of a jig to frame (`FrameRequest::jig_pockets`).
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JigPockets {
    /// The jig's id (or name).
    pub jig: String,
    /// Pockets by number or id; empty: every enabled pocket.
    #[serde(default)]
    pub pockets: Vec<PocketRef>,
}

/// The pockets' outlines as stand-in objects (field frame) for framing. Unknown jigs and pockets are 404.
pub fn pocket_objects(doc: &Document, req: &JigPockets) -> Result<Vec<doc::Object>, EngineError> {
    let jig = doc
        .jigs
        .iter()
        .find(|j| j.id == req.jig || j.name == req.jig)
        .ok_or_else(|| EngineError::NotFound(format!("no jig {}", req.jig)))?;
    let pockets: Vec<&doc::jig::Pocket> = if req.pockets.is_empty() {
        jig.enabled_pockets()
    } else {
        req.pockets
            .iter()
            .map(|r| {
                jig.pockets
                    .iter()
                    .find(|p| match r {
                        PocketRef::Number(n) => p.number == *n,
                        PocketRef::Id(id) => p.id == *id,
                    })
                    .ok_or_else(|| EngineError::NotFound(format!("jig {} has no pocket {r:?}", jig.id)))
            })
            .collect::<Result<_, _>>()?
    };
    Ok(pockets
        .into_iter()
        .map(|p| {
            let path = jig.pocket_outline(p);
            doc::Object::new(format!("{}/{}", jig.id, p.id), format!("Pocket {}", p.number), String::new(), doc::Geometry::Path { path }, kurbo::Affine::IDENTITY)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_zone_rules() {
        let z = |name: &str, points: Vec<[f64; 2]>| DeviceZone { name: name.into(), points, enabled: true };
        assert!(device_zones_problem(&[z("Clamp", vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]])]).is_none());
        assert!(device_zones_problem(&[z("", vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]])]).is_some());
        assert!(device_zones_problem(&[z("Line", vec![[0.0, 0.0], [1.0, 0.0], [2.0, 0.0]])]).unwrap().contains("no area"));
        assert!(device_zones_problem(&[z("NaN", vec![[0.0, 0.0], [f64::NAN, 0.0], [1.0, 1.0]])]).is_some());
        let off = DeviceZone { enabled: false, ..z("Off", vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]]) };
        assert!(zones_for(&Document::default(), &[off]).is_empty());
        let parsed: DeviceZone = serde_json::from_str(r#"{"name": "a", "points": [[0,0],[1,0],[0,1]]}"#).unwrap();
        assert!(parsed.enabled);
    }
}
