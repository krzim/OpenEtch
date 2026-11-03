//! Keep-out zones of a device (`PUT /api/prefs` `keep_out`; moved from the engine's `zones` module, which applies
//! them to every planned run).

use serde::{Deserialize, Serialize};

/// Most keep-out zones one device keeps.
pub const MAX_DEVICE_ZONES: usize = doc::jig::MAX_KEEP_OUTS;
/// Most points one zone or region may have.
pub const MAX_POINTS: usize = 10_000;

/// A keep-out zone of a device: a polygon in field mm (`PUT /api/prefs` `keep_out`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceZone {
    pub name: String,
    /// The outline, field mm, at least 3 points (closed implicitly).
    pub points: Vec<[f64; 2]>,
    /// Off: kept but not enforced.
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

/// Why these device zones cannot be saved, or `None`.
pub fn device_zones_problem(zones: &[DeviceZone]) -> Option<String> {
    if zones.len() > MAX_DEVICE_ZONES {
        return Some(format!("at most {MAX_DEVICE_ZONES} keep-out zones per device"));
    }
    for z in zones {
        let name = z.name.trim();
        if name.is_empty() || name.chars().count() > 200 || name.contains(char::is_control) {
            return Some("a keep-out zone needs a name (at most 200 characters, no line breaks)".into());
        }
        if let Some(p) = polygon_problem(&z.points) {
            return Some(format!("keep-out zone \"{name}\": {p}"));
        }
    }
    None
}

/// Why `points` is not a usable polygon, or `None`.
pub fn polygon_problem(points: &[[f64; 2]]) -> Option<String> {
    if points.len() < 3 {
        return Some("needs at least 3 points".into());
    }
    if points.len() > MAX_POINTS {
        return Some(format!("has more than {MAX_POINTS} points"));
    }
    if points.iter().flatten().any(|v| !v.is_finite()) {
        return Some("has a coordinate that is not a finite number".into());
    }
    let area: f64 = (0..points.len())
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % points.len()]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f64>()
        / 2.0;
    if area.abs() < 1e-9 {
        return Some("has no area".into());
    }
    None
}

