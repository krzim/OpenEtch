//! The display preview, as the engine's `GET /api/preview` computes it, packed into flat arrays for the GPU toolpath
//! layer (web/src/components/toolpathGL.ts) instead of one JSON object per segment.
//!
//! The planning facts come from the engine (`GET /api/preview/context`): the active device's [`Target`], its learned
//! estimate correction (`engine::marktime`), the job counter (auto-rotating fills preview at the angle of the next
//! run) and, for a design-only device, why it cannot be planned for. With the engine's document and that context,
//! [`plan_preview`] returns exactly what the engine's preview does (the engine parity tests check it).

use std::collections::HashMap;

use doc::Document;
use plan::{JobOptions, PlanError, Preview, SegmentKind, Target};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// What the engine plans the preview with (`GET /api/preview/context`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Context {
    /// The active device's planning target; `None` while the engine has none to give (then nothing is planned here).
    pub target: Option<Target>,
    /// The device's learned estimate correction, applied to every time of the preview (`plan::scale_times`).
    #[serde(default = "one")]
    pub correction: f64,
    /// The job counter of the active device: the next run's index.
    #[serde(default)]
    pub run_index: u64,
    /// Why the active device cannot be planned for (a design-only device), or `None`.
    #[serde(default)]
    pub design_only: Option<String>,
}

fn one() -> f64 {
    1.0
}

impl From<appcore::preview::PreviewContext> for Context {
    fn from(c: appcore::preview::PreviewContext) -> Self {
        Context { target: Some(c.target), correction: c.correction, run_index: c.run_index, design_only: c.design_only }
    }
}

/// The engine's preview of `doc`: `Ok` with the preview (empty for a document with nothing to output), or `Err`
/// with the message the engine answers 422 with.
pub fn plan_preview(doc: &Document, ctx: &Context) -> Result<Preview, String> {
    if let Some(reason) = &ctx.design_only {
        return Err(PlanError::Target(reason.clone()).to_string());
    }
    let Some(target) = &ctx.target else {
        return Err("the engine gave no planning target".into());
    };
    let opts = JobOptions { preview: true, run_index: ctx.run_index, ..JobOptions::default() };
    // As the engine plans it: the pockets of empty and past-End production slots left out.
    let doc = &*doc::slots::without_skipped_slots(doc);
    match plan::plan_job_with(doc, target, &opts) {
        Ok(job) => {
            let mut p = plan::preview_document(doc, &job);
            plan::scale_times(&mut p, ctx.correction);
            Ok(p)
        }
        Err(PlanError::Empty) => Ok(Preview::default()),
        Err(e) => Err(e.to_string()),
    }
}

/// `info` bit: a mark (else a jump).
pub const MARK: u32 = 1;
/// `info` bit: the mark fires.
pub const EMITS: u32 = 2;
/// `info >> LAYER_SHIFT` is the segment's index in `layer_ids` plus one (0: no layer).
pub const LAYER_SHIFT: u32 = 2;
/// `pass` value of a segment without a pass.
pub const NO_PASS: u32 = u32::MAX;

/// A preview with its segments in flat arrays (segment `i` is `xy[4i..4i+4]`, `t[i]`, `info[i]`, `pass[i]`,
/// `power[i]`), its image rasters ([`plan::PreviewRaster`]) in three more, and everything else as JSON (`meta`).
///
/// A segment inside a raster (`meta.rasters[k].first..end`: an image's scan runs and the jumps between them) has
/// no geometry here (`xy` and `power` NaN): the raster draws it, so the page never walks hundreds of thousands of
/// runs. Its time, kind, layer and pass stay, so the time slider and Start Here count segments as the engine does.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Packed {
    /// x0, y0, x1, y1 per segment, field mm (NaN inside a raster).
    pub xy: Vec<f32>,
    /// Estimated time from the job's start to the segment's end, s (`PreviewSegment::t_s`).
    pub t: Vec<f32>,
    /// [`MARK`] | [`EMITS`] | (layer index + 1) << [`LAYER_SHIFT`].
    pub info: Vec<u32>,
    /// Index into `meta.passes`, or [`NO_PASS`].
    pub pass: Vec<u32>,
    /// The mark's own power in %, NaN where it is the pass's (`PreviewSegment::power_pct`) and inside a raster.
    pub power: Vec<f32>,
    /// The rasters' dots, row segments and row spans, as `meta.rasters` (each a [`plan::RasterHeader`]) index them.
    pub levels: Vec<u8>,
    pub row_segments: Vec<u32>,
    pub row_span: Vec<f32>,
    /// The engine's preview JSON without `segments`, plus `layer_ids` (the segments' layer table), `segment_count`
    /// and `rasters`; or `{"error": ...}` when the document cannot be planned.
    pub meta: Value,
}

/// Pack `preview` (of `doc`). The meta JSON matches the engine's (`unsupported` LightBurn settings per layer too).
pub fn pack(doc: &Document, preview: Preview) -> Packed {
    let n = preview.segments.len();
    let mut out = Packed {
        xy: Vec::with_capacity(4 * n),
        t: Vec::with_capacity(n),
        info: Vec::with_capacity(n),
        pass: Vec::with_capacity(n),
        power: Vec::with_capacity(n),
        ..Packed::default()
    };
    let mut layer_ids: Vec<String> = Vec::new();
    let mut index: HashMap<String, u32> = HashMap::new();
    let mut last: Option<(String, u32)> = None;
    let Preview { segments, passes, stats, layers, warnings, rasters } = preview;
    let mut inside = rasters.iter().map(|r| r.first..r.end).peekable();
    for (i, s) in segments.into_iter().enumerate() {
        while inside.peek().is_some_and(|r| r.end <= i) {
            inside.next();
        }
        if inside.peek().is_some_and(|r| r.contains(&i)) {
            out.xy.extend_from_slice(&[f32::NAN; 4]);
            out.power.push(f32::NAN);
        } else {
            #[allow(clippy::cast_possible_truncation)]
            out.xy.extend_from_slice(&[s.from[0] as f32, s.from[1] as f32, s.to[0] as f32, s.to[1] as f32]);
            #[allow(clippy::cast_possible_truncation)]
            out.power.push(s.power_pct.map_or(f32::NAN, |p| p as f32));
        }
        #[allow(clippy::cast_possible_truncation)]
        out.t.push(s.t_s as f32);
        let mut info = if s.kind == SegmentKind::Mark { MARK } else { 0 };
        if s.emits {
            info |= EMITS;
        }
        if let Some(l) = s.layer {
            // Runs of one layer: look the table up only when the layer changes.
            let k = match &last {
                Some((name, k)) if *name == l => *k,
                _ => {
                    let k = match index.get(&l) {
                        Some(&k) => k,
                        None => {
                            let k = u32::try_from(layer_ids.len()).unwrap_or(u32::MAX >> LAYER_SHIFT);
                            index.insert(l.clone(), k);
                            layer_ids.push(l.clone());
                            k
                        }
                    };
                    last = Some((l, k));
                    k
                }
            };
            info |= (k + 1) << LAYER_SHIFT;
        }
        out.info.push(info);
        out.pass.push(s.pass.and_then(|p| u32::try_from(p).ok()).unwrap_or(NO_PASS));
    }
    let packed = plan::PackedRasters::new(&rasters);
    let mut meta = json!({ "passes": passes, "stats": stats, "layers": layers, "warnings": warnings });
    mark_unsupported(doc, &mut meta);
    meta["layer_ids"] = json!(layer_ids);
    meta["segment_count"] = json!(n);
    meta["rasters"] = json!(packed.headers);
    (out.levels, out.row_segments, out.row_span) = (packed.levels, packed.row_segments, packed.row_span);
    out.meta = meta;
    out
}

/// The preview's summary only (stats, passes, layers, warnings: what the Design workspace shows), without packing
/// the segments.
pub fn summary(doc: &Document, preview: Preview) -> Value {
    let n = preview.segments.len();
    let Preview { passes, stats, layers, warnings, .. } = preview;
    let mut meta = json!({ "passes": passes, "stats": stats, "layers": layers, "warnings": warnings });
    mark_unsupported(doc, &mut meta);
    meta["segment_count"] = json!(n);
    meta
}

/// LAY-034, as the engine's preview route: each layer's LightBurn settings that will not be executed (only where
/// there are some).
fn mark_unsupported(doc: &Document, meta: &mut Value) {
    if let Some(layers) = meta.get_mut("layers").and_then(Value::as_array_mut) {
        for l in layers {
            let Some(layer) = l["layer_id"].as_str().and_then(|id| doc.layer(id)) else { continue };
            let u = lbrn::unexecuted_settings(layer);
            if !u.is_empty() {
                l["unsupported"] = json!(u);
            }
        }
    }
}

/// The error meta for a document that cannot be planned.
pub fn error_meta(message: &str) -> Value {
    json!({ "error": message })
}

#[cfg(test)]
mod tests {
    use super::*;
    use doc::{CutSettings, FillMode, Geometry, Object};
    use kurbo::Affine;

    fn target() -> Target {
        serde_json::from_value(json!({
            "field_mm": 110.0, "swap_xy": false, "negate_x": false, "negate_y": false, "max_power_pct": 100.0,
            "pulse_widths_ns": [2, 4, 6, 8, 13, 20, 30, 45, 60, 80, 100, 150, 200, 250, 350, 500],
            "timing": {
                "laser_on_delay_us": 100.0, "laser_off_delay_us": 100.0, "polygon_delay_us": 10.0, "end_delay_us": 0.0,
                "jump_speed_mm_s": 4000.0, "min_jump_delay_us": 100.0, "max_jump_delay_us": 400.0,
                "jump_distance_limit_mm": 10.0
            },
            "open_mo_delay_us": 8000.0
        }))
        .unwrap()
    }

    fn rect_doc() -> Document {
        let mut d = Document::default();
        d.layers[0].settings = CutSettings { mode: FillMode::Fill, ..CutSettings::default() };
        let g = Geometry::Rect { width_mm: 10.0, height_mm: 6.0, corner_radius_mm: 0.0 };
        d.objects.push(Object::new("obj-1".into(), "r", "layer-1".into(), g, Affine::translate((5.0, -3.0))));
        d
    }

    #[test]
    fn packs_every_segment_and_keeps_the_summary() {
        let d = rect_doc();
        let ctx = Context { target: Some(target()), correction: 1.5, run_index: 0, design_only: None };
        let p = plan_preview(&d, &ctx).unwrap();
        let segs = p.segments.clone();
        let packed = pack(&d, p.clone());
        assert_eq!(packed.t.len(), segs.len());
        assert_eq!(packed.xy.len(), 4 * segs.len());
        assert_eq!(packed.meta["segment_count"], segs.len());
        assert_eq!(packed.meta["layer_ids"], json!(["layer-1"]));
        assert_eq!(packed.meta["stats"], serde_json::to_value(p.stats).unwrap());
        for (i, s) in segs.iter().enumerate() {
            assert_eq!(packed.info[i] & MARK != 0, s.kind == SegmentKind::Mark);
            assert_eq!(packed.info[i] >> LAYER_SHIFT, 1);
            assert_eq!(packed.pass[i] as usize, s.pass.unwrap());
            assert!((f64::from(packed.xy[4 * i + 2]) - s.to[0]).abs() < 1e-5);
        }
        assert_eq!(summary(&d, p)["segment_count"], segs.len());
    }

    #[test]
    fn design_only_and_empty_documents_answer_like_the_engine() {
        let ctx = Context { target: Some(target()), correction: 1.0, run_index: 0, design_only: Some("a Ruida".into()) };
        assert_eq!(plan_preview(&rect_doc(), &ctx).unwrap_err(), "invalid machine target: a Ruida");
        let ctx = Context { design_only: None, ..ctx };
        assert!(plan_preview(&Document::default(), &ctx).unwrap().segments.is_empty());
        let none = Context { target: None, ..ctx };
        assert!(plan_preview(&rect_doc(), &none).is_err());
    }
}
