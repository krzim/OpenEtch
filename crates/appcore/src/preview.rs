//! Preview assembly (moved from the engine's `hw`, `state`, `preview_context` and `preview_bin` modules): the planning
//! target of a machine profile, what the display preview is planned with ([`PreviewContext`]), the plan itself
//! ([`plan_preview`]) and the JSON members `GET /api/preview`, `/api/preview/stats` and the binary preview share.

use doc::Document;
use machine::grbl::GrblProfile;
use machine::ruida::RuidaProfile;
use machine::{AxisRole, FiberKind, MachineProfile};
use plan::{GcodeSettings, PlanError, Preview, Target};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The planning target for a machine profile. Hard limits come only from the profile; so do the open-MO delay,
/// the lens correction, whether pulse widths are sent (Q-pulse enabled on a MOPA) and which extension axis the board
/// drives.
pub fn target_from_profile(p: &MachineProfile) -> Target {
    Target {
        field_mm: p.field_mm,
        field_height_mm: p.field_height_mm,
        curve_tolerance_mm: p.curve_tolerance_mm,
        swap_xy: p.swap_xy,
        negate_x: p.negate_x,
        negate_y: p.negate_y,
        max_power_pct: p.max_power_pct,
        pulse_widths_ns: p.pulse_widths_ns.clone(),
        timing: doc::GalvoTiming {
            laser_on_delay_us: p.timing.laser_on_delay_us,
            laser_off_delay_us: p.timing.laser_off_delay_us,
            polygon_delay_us: p.timing.polygon_delay_us,
            end_delay_us: p.timing.end_delay_us,
            jump_speed_mm_s: p.timing.jump_speed_mm_s,
            min_jump_delay_us: p.timing.jump_delay_min_us,
            max_jump_delay_us: p.timing.jump_delay_max_us,
            jump_distance_limit_mm: p.timing.jump_distance_limit_mm,
        },
        open_mo_delay_us: p.open_mo_delay_us,
        motion_time_scale: p.motion_time_scale,
        calibration: p.calibration,
        pulse_width_enabled: p.pulse_width_enabled && p.fiber_kind == FiberKind::Mopa,
        z_axis: p.axis.as_ref().is_some_and(|a| a.role == AxisRole::Z),
        rotary_axis: p.axis.as_ref().is_some_and(|a| a.role == AxisRole::Rotary),
        port_outputs: p.io.run_outputs(),
        gantry: false,
    }
}

/// G-code output settings for a GRBL profile.
pub fn gcode_settings(g: &GrblProfile) -> GcodeSettings {
    let (mirror_x, mirror_y) = g.mirror();
    GcodeSettings {
        work_area_mm: g.work_area_mm,
        mirror_x,
        mirror_y,
        s_max: g.s_max,
        max_power_pct: g.max_power_pct,
        max_feed_mm_min: g.max_feed_mm_min,
        dynamic_power: g.dynamic_power,
    }
}

/// The planning target for a GRBL profile: the bed, its power limit, no galvo transform.
pub fn target_from_grbl(g: &GrblProfile) -> Target {
    gcode_settings(g).target()
}

/// The planning target for a Ruida profile: the bed (as for GRBL), its power limit, its speed limit for rapid-move
/// time estimates.
pub fn target_from_ruida(r: &RuidaProfile) -> Target {
    let (mirror_x, mirror_y) = r.mirror();
    GcodeSettings {
        work_area_mm: r.work_area_mm,
        mirror_x,
        mirror_y,
        s_max: 1000.0,
        max_power_pct: r.max_power_pct,
        max_feed_mm_min: r.max_speed_mm_s * 60.0,
        dynamic_power: false,
    }
    .target()
}

/// The planning target of a device: its Ruida bed, its GRBL bed, else its galvo profile (`None` for a device without
/// any, which cannot be planned for).
pub fn target_for_device(d: &machine::Device) -> Option<Target> {
    if let Some(r) = &d.ruida {
        return Some(target_from_ruida(r));
    }
    if let Some(g) = &d.grbl {
        return Some(target_from_grbl(g));
    }
    d.profile.as_ref().map(target_from_profile)
}

/// What the display preview is planned with besides the document (the body of `GET /api/preview/context`; webcore's
/// browser preview takes it as its context).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreviewContext {
    /// The active device's planning target (field, limits, timing, lens correction).
    pub target: Target,
    /// Multiply every estimated time by this (1 until the device has enough finished runs).
    pub correction: f64,
    /// The next run's index on the active device.
    pub run_index: u64,
    /// Why the active device cannot be planned for, or `None`.
    pub design_only: Option<String>,
    /// The machine configuration's generation: it changes with the device.
    pub generation: u64,
}

/// The preview of `doc` as a run would plan it: the pockets of empty and past-End production slots left out
/// (`doc::slots`), refused with the device's reason while it is design-only, an empty document as an empty preview,
/// every time scaled by `correction`. Returns the document planned (slots left out) with the result.
pub fn plan_preview(doc: Document, target: &Target, run_index: u64, correction: f64, design_only: Option<String>) -> (Document, Result<Preview, PlanError>) {
    let trimmed = match doc::slots::without_skipped_slots(&doc) {
        std::borrow::Cow::Owned(d) => Some(d),
        std::borrow::Cow::Borrowed(_) => None,
    };
    let doc = trimmed.unwrap_or(doc);
    let preview = match design_only {
        // Still a result, so the SVG preview renders the document.
        Some(reason) => Err(PlanError::Target(reason)),
        None => match plan::plan_job_with(&doc, target, &plan::JobOptions { preview: true, run_index, ..Default::default() }) {
            Ok(job) => {
                let mut p = plan::preview_document(&doc, &job);
                plan::scale_times(&mut p, correction);
                Ok(p)
            }
            Err(PlanError::Empty) => Ok(Preview::default()),
            Err(e) => Err(e),
        },
    };
    (doc, preview)
}

/// The header members `GET /api/preview` and the binary preview share: the preview without its segments, each layer
/// with the LightBurn settings it will not execute (LAY-034).
pub fn summary(doc: &Document, p: &Preview, layers: Option<&[String]>) -> Value {
    let mut layer_list: Vec<&plan::LayerPreview> = p.layers.iter().collect();
    if let Some(keep) = layers {
        layer_list.retain(|l| keep.contains(&l.layer_id));
    }
    let mut v = json!({
        "stats": p.stats,
        "passes": p.passes,
        "layers": layer_list,
        "warnings": p.warnings,
    });
    add_unsupported(doc, &mut v);
    v
}

/// Each layer entry of a preview's JSON gets the LightBurn settings it will not execute, where there are some.
pub fn add_unsupported(doc: &Document, v: &mut Value) {
    if let Some(layers) = v.get_mut("layers").and_then(Value::as_array_mut) {
        for l in layers {
            let Some(layer) = l["layer_id"].as_str().and_then(|id| doc.layer(id)) else { continue };
            let u = lbrn::unexecuted_settings(layer);
            if !u.is_empty() {
                l["unsupported"] = json!(u);
            }
        }
    }
}

/// `GET /api/preview` (optionally `?layers=a,b`: only those layers' segments and entries) as JSON.
pub fn preview_json(doc: &Document, p: &Preview, layers: Option<&str>) -> Result<Value, serde_json::Error> {
    let mut v = match layers {
        None => serde_json::to_value(p),
        Some(list) => {
            let ids: Vec<&str> = list.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
            let mut p = p.clone();
            p.retain_layers(&ids);
            serde_json::to_value(&p)
        }
    }?;
    add_unsupported(doc, &mut v);
    Ok(v)
}

/// `GET /api/preview/stats`: the summary with the revision and segment count.
pub fn stats_json(doc: &Document, p: &Preview, revision: u64) -> Value {
    let mut v = summary(doc, p, None);
    v["revision"] = json!(revision);
    v["segments"] = json!(p.segments.len());
    v
}
