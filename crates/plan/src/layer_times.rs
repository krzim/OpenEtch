//! What each layer of a planned job takes ([`layer_times`]): the job ledger (the engine's `ledger` module) records it
//! with every run, so "how long did the frost layer take" can be answered later. Cheap: one walk over the passes in
//! the plan's own time model, without building preview segments.

use serde::{Deserialize, Serialize};

use crate::job::{JobPlan, pass_stats};

/// One layer's share of a planned job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerTime {
    pub layer_id: doc::LayerId,
    /// Job passes planned for it (repeats and sub-layers).
    pub passes: usize,
    pub mark_length_mm: f64,
    /// Motion and delays of its passes, s (as [`crate::Stats::estimated_time_s`], without the job's start-up wait).
    pub estimated_time_s: f64,
}

/// Every layer that has passes in `plan`, in the order its first pass burns. A pass's travel into it counts for it,
/// as in [`crate::preview_document`].
pub fn layer_times(plan: &JobPlan) -> Vec<LayerTime> {
    let mut out: Vec<LayerTime> = Vec::new();
    let mut pos = [0.0, 0.0];
    for pass in &plan.passes {
        let (s, _, end) = pass_stats(pass, pos);
        pos = end;
        let i = match out.iter().position(|l| l.layer_id == pass.layer_id) {
            Some(i) => i,
            None => {
                out.push(LayerTime { layer_id: pass.layer_id.clone(), passes: 0, mark_length_mm: 0.0, estimated_time_s: 0.0 });
                out.len() - 1
            }
        };
        let l = &mut out[i];
        l.passes += 1;
        l.mark_length_mm += s.mark_length_mm;
        l.estimated_time_s += s.estimated_time_s;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::target::tests::target;
    use crate::{plan_job, preview_document};
    use doc::{Document, FillMode, Geometry, Layer, Object};
    use kurbo::Affine;

    #[test]
    fn layer_times_match_the_preview_totals() {
        let mut d = Document::default();
        let mut engrave = Layer::new("layer-7".into(), "Engrave", "#ff0000");
        engrave.settings.mode = FillMode::Fill;
        engrave.settings.passes = 3;
        d.layers.push(engrave);
        let rect = |id: &str, layer: &str, x: f64| {
            let g = Geometry::Rect { width_mm: 10.0, height_mm: 8.0, corner_radius_mm: 0.0 };
            Object::new(id.into(), id, layer.into(), g, Affine::translate((x, 0.0)))
        };
        d.objects = vec![rect("obj-a", "layer-7", 15.0), rect("obj-b", "layer-1", -15.0)];
        let plan = plan_job(&d, &target()).unwrap();
        let times = layer_times(&plan);
        let preview = preview_document(&d, &plan);
        assert_eq!(times.len(), 2);
        for t in &times {
            let l = preview.layers.iter().find(|l| l.layer_id == t.layer_id).unwrap();
            assert_eq!(t.passes, l.passes, "{}", t.layer_id);
            assert!((t.estimated_time_s - l.estimated_time_s).abs() < 1e-9, "{t:?} vs {}", l.estimated_time_s);
            assert!((t.mark_length_mm - l.mark_length_mm).abs() < 1e-9);
        }
        let engrave = times.iter().find(|t| t.layer_id == "layer-7").unwrap();
        assert_eq!(engrave.passes, 3);
        assert!(engrave.estimated_time_s > 0.0);
    }
}
