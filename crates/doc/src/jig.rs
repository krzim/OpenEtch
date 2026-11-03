//! Jigs (production fixtures holding several blanks in numbered pockets), keep-out zones and the per-project
//! pre-flight settings. Plain data saved with the document ([`crate::Document::jigs`],
//! [`crate::Document::preflight`]); the `commands` crate edits them and places designs into the pockets, `plan` and the
//! engine refuse jobs that mark inside a keep-out zone.
//!
//! A jig has its own frame: its **datum** (a point on the physical jig, e.g. a corner pin) sits at
//! [`Jig::datum`] in the field frame and the jig is turned by [`Jig::rotation_deg`] about it. Pockets, keep-out walls
//! and the plate outline are stored in the jig frame, so moving the whole jig changes two numbers. Every field is
//! optional or defaulted in JSON so later additions keep older files loading (the file format stays version 1).

use std::collections::BTreeSet;

use kurbo::{Affine, BezPath, Rect, Shape};
use serde::{Deserialize, Serialize};

/// Object extras key on clones placed in jig pockets: the pocket's id (`"<jig id>/<pocket id>"`). Display only; the
/// placement itself is a [`crate::VirtualArray`] whose settings name the jig.
pub const JIG_POCKET_KEY: &str = "openetch.jig_pocket";

/// Most pockets one jig may hold (a typo must not make thousands of clones).
pub const MAX_POCKETS: usize = 500;

/// Most keep-out zones on one jig (and on one device).
pub const MAX_KEEP_OUTS: usize = 64;

/// Most jigs one document keeps.
pub const MAX_JIGS: usize = 32;

fn yes() -> bool {
    true
}

fn is_true(v: &bool) -> bool {
    *v
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

/// A production fixture: numbered pockets, a datum, the material height and optional keep-out walls.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Jig {
    /// Stable id ("jig-4"), from the document's id counter.
    pub id: String,
    pub name: String,
    /// Where the jig's datum is in the field frame, mm.
    pub datum: [f64; 2],
    /// Turn of the whole jig about its datum, degrees counter-clockwise.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation_deg: f64,
    /// Height of the blanks' top surface (what the lens focuses on), mm. Recorded with the jig for the operator; the
    /// B6's lift is not driven by the board, so nothing moves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material_height_mm: Option<f64>,
    /// The pockets, in their number order (the burn and serial-number order).
    pub pockets: Vec<Pocket>,
    /// Walls, clamps and pins the laser must never mark: a job that marks inside one is refused.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keep_out: Vec<KeepOut>,
    /// The jig plate's outline, jig frame (drawn only; never planned, framed or exported).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outline: Option<BezPath>,
}

/// One pocket of a [`Jig`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pocket {
    /// Stable within its jig ("p1", "p2", ...): what the production ledger records per slot. Never reused after a
    /// renumbering.
    pub id: String,
    /// The number shown on the canvas and in the jig window, 1-based; the order pockets burn and count serials in.
    pub number: u32,
    /// Centre in the jig frame, mm.
    pub center: [f64; 2],
    /// Turn of the pocket about its centre (within the jig), degrees counter-clockwise.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation_deg: f64,
    pub shape: PocketShape,
    /// Off: the pocket is skipped (empty this run, damaged): no copy of the design, no serial number.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
}

/// The outline of a pocket in its own frame (centred on the pocket's centre).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PocketShape {
    Rect {
        width_mm: f64,
        height_mm: f64,
        #[serde(default, skip_serializing_if = "is_zero")]
        corner_radius_mm: f64,
    },
    Circle {
        diameter_mm: f64,
    },
    /// Any closed outline (from a DXF or SVG jig drawing), relative to the pocket centre.
    Path { path: BezPath },
}

/// An area the laser must never mark (a jig wall, a clamp, the device's fixture). Closed outline; jig frame on a
/// jig, field frame on a device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeepOut {
    /// Stable id ("k1", ...).
    pub id: String,
    pub name: String,
    pub path: BezPath,
    /// Off: kept but not enforced.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
}

impl PocketShape {
    /// Outline in the pocket's own frame.
    pub fn local_path(&self) -> BezPath {
        match self {
            PocketShape::Rect { width_mm, height_mm, corner_radius_mm } => {
                Rect::from_center_size((0.0, 0.0), (*width_mm, *height_mm)).to_rounded_rect(*corner_radius_mm).to_path(0.01)
            }
            PocketShape::Circle { diameter_mm } => kurbo::Circle::new((0.0, 0.0), diameter_mm / 2.0).to_path(0.01),
            PocketShape::Path { path } => path.clone(),
        }
    }

    /// Every number of the shape (for the finite check).
    fn numbers(&self) -> Vec<f64> {
        match self {
            PocketShape::Rect { width_mm, height_mm, corner_radius_mm } => vec![*width_mm, *height_mm, *corner_radius_mm],
            PocketShape::Circle { diameter_mm } => vec![*diameter_mm],
            PocketShape::Path { path } => path.elements().iter().flat_map(|e| points_of(*e)).flat_map(|p| [p.x, p.y]).collect(),
        }
    }
}

fn points_of(e: kurbo::PathEl) -> Vec<kurbo::Point> {
    match e {
        kurbo::PathEl::MoveTo(p) | kurbo::PathEl::LineTo(p) => vec![p],
        kurbo::PathEl::QuadTo(a, b) => vec![a, b],
        kurbo::PathEl::CurveTo(a, b, c) => vec![a, b, c],
        kurbo::PathEl::ClosePath => vec![],
    }
}

impl Jig {
    /// Jig frame -> field frame.
    pub fn transform(&self) -> Affine {
        Affine::translate((self.datum[0], self.datum[1])) * Affine::rotate(self.rotation_deg.to_radians())
    }

    /// Pocket frame -> field frame for `pocket` (its centre at the origin, turned by its own and the jig's rotation).
    pub fn pocket_transform(&self, pocket: &Pocket) -> Affine {
        self.transform() * Affine::translate((pocket.center[0], pocket.center[1])) * Affine::rotate(pocket.rotation_deg.to_radians())
    }

    /// The pocket's outline in the field frame.
    pub fn pocket_outline(&self, pocket: &Pocket) -> BezPath {
        self.pocket_transform(pocket) * pocket.shape.local_path()
    }

    /// The pocket's centre in the field frame.
    pub fn pocket_center(&self, pocket: &Pocket) -> kurbo::Point {
        self.transform() * kurbo::Point::new(pocket.center[0], pocket.center[1])
    }

    pub fn pocket(&self, id: &str) -> Option<&Pocket> {
        self.pockets.iter().find(|p| p.id == id)
    }

    /// The enabled pockets in number order (the burn and serial-number order).
    pub fn enabled_pockets(&self) -> Vec<&Pocket> {
        let mut v: Vec<&Pocket> = self.pockets.iter().filter(|p| p.enabled).collect();
        v.sort_by_key(|p| p.number);
        v
    }

    /// The enabled keep-out zones in the field frame, each with its name.
    pub fn keep_out_zones(&self) -> Vec<(String, BezPath)> {
        let t = self.transform();
        self.keep_out.iter().filter(|k| k.enabled).map(|k| (format!("{} / {}", self.name, k.name), t * k.path.clone())).collect()
    }

    /// The plate outline in the field frame, if any.
    pub fn field_outline(&self) -> Option<BezPath> {
        self.outline.as_ref().map(|o| self.transform() * o.clone())
    }

    /// Field-frame bounds of every pocket, wall and the plate.
    pub fn bounds(&self) -> Option<Rect> {
        let t = self.transform();
        self.pockets
            .iter()
            .map(|p| self.pocket_outline(p).bounding_box())
            .chain(self.keep_out.iter().map(|k| (t * k.path.clone()).bounding_box()))
            .chain(self.field_outline().map(|o| o.bounding_box()))
            .reduce(|a, b| a.union(b))
    }

    /// Why this jig cannot be used (a number that is not finite, a pocket without size, duplicate ids or numbers), or
    /// `None` when it is sound.
    pub fn problem(&self) -> Option<String> {
        let mut nums = vec![self.datum[0], self.datum[1], self.rotation_deg];
        nums.extend(self.material_height_mm);
        if nums.iter().any(|v| !v.is_finite()) {
            return Some(format!("jig {}: a number is not finite", self.id));
        }
        if self.pockets.is_empty() {
            return Some(format!("jig {} has no pockets", self.id));
        }
        if self.pockets.len() > MAX_POCKETS {
            return Some(format!("jig {} has more than {MAX_POCKETS} pockets", self.id));
        }
        if self.keep_out.len() > MAX_KEEP_OUTS {
            return Some(format!("jig {} has more than {MAX_KEEP_OUTS} keep-out zones", self.id));
        }
        let mut ids = BTreeSet::new();
        let mut numbers = BTreeSet::new();
        for p in &self.pockets {
            if !ids.insert(&p.id) {
                return Some(format!("jig {}: pocket id {} is used twice", self.id, p.id));
            }
            if p.number == 0 || !numbers.insert(p.number) {
                return Some(format!("jig {}: pocket number {} is 0 or used twice", self.id, p.number));
            }
            let mut v = vec![p.center[0], p.center[1], p.rotation_deg];
            v.extend(p.shape.numbers());
            if v.iter().any(|x| !x.is_finite()) {
                return Some(format!("jig {}: pocket {} has a number that is not finite", self.id, p.number));
            }
            let b = p.shape.local_path().bounding_box();
            if !(b.width() > 0.0 && b.height() > 0.0) {
                return Some(format!("jig {}: pocket {} has no size", self.id, p.number));
            }
        }
        let mut kids = BTreeSet::new();
        for k in &self.keep_out {
            if !kids.insert(&k.id) {
                return Some(format!("jig {}: keep-out id {} is used twice", self.id, k.id));
            }
            if k.path.elements().iter().flat_map(|e| points_of(*e)).any(|p| !(p.x.is_finite() && p.y.is_finite())) {
                return Some(format!("jig {}: keep-out {} has a number that is not finite", self.id, k.name));
            }
        }
        None
    }
}

/// Every enabled keep-out zone of the document's jigs, field frame, with its name.
pub fn document_keep_outs(doc: &crate::Document) -> Vec<(String, BezPath)> {
    doc.jigs.iter().flat_map(Jig::keep_out_zones).collect()
}

/// The jig pocket an object sits in ([`object_pockets`]): what the serial ledger, the cockpit and the job ledger
/// record per slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PocketPlace {
    /// The jig's id ("jig-4").
    pub jig: String,
    pub jig_name: String,
    /// The pocket's stable id within its jig ("p3").
    pub pocket: String,
    /// The pocket's number (burn and serial order), 1-based.
    pub number: u32,
}

/// How far an object may reach past a pocket's bounds and still count as sitting in it, mm.
const POCKET_SLACK_MM: f64 = 0.5;

/// The jig pocket of every object that sits in one, by object id:
/// - clones placed by `jig.place` (their [`JIG_POCKET_KEY`] tag);
/// - the originals of a placement (its source pocket);
/// - else, for designs laid out by hand (a LightBurn file on a jig), the enabled pocket whose bounds hold the whole
///   object (within 0.5 mm) and whose outline holds its centre.
///
/// Objects on no jig (or spanning several pockets, like the plate outline) are left out.
pub fn object_pockets(doc: &crate::Document) -> std::collections::BTreeMap<crate::ObjectId, PocketPlace> {
    let mut out = std::collections::BTreeMap::new();
    if doc.jigs.is_empty() {
        return out;
    }
    let place = |jig_id: &str, pocket_id: &str| -> Option<PocketPlace> {
        let jig = doc.jigs.iter().find(|j| j.id == jig_id)?;
        let p = jig.pocket(pocket_id)?;
        Some(PocketPlace { jig: jig.id.clone(), jig_name: jig.name.clone(), pocket: p.id.clone(), number: p.number })
    };
    for o in &doc.objects {
        if let Some((j, p)) = o.extras.get(JIG_POCKET_KEY).and_then(|t| t.split_once('/'))
            && let Some(pl) = place(j, p)
        {
            out.insert(o.id.clone(), pl);
        }
    }
    for a in &doc.virtual_arrays {
        let (Some(j), Some(p)) = (a.settings.get("jig").and_then(|v| v.as_str()), a.settings.get("source_pocket").and_then(|v| v.as_str())) else {
            continue;
        };
        if let Some(pl) = place(j, p) {
            for s in &a.sources {
                out.entry(s.clone()).or_insert_with(|| pl.clone());
            }
        }
    }
    let pockets: Vec<(PocketPlace, BezPath, Rect)> = doc
        .jigs
        .iter()
        .flat_map(|j| {
            j.enabled_pockets().into_iter().map(move |p| {
                let outline = j.pocket_outline(p);
                let b = outline.bounding_box().inflate(POCKET_SLACK_MM, POCKET_SLACK_MM);
                (PocketPlace { jig: j.id.clone(), jig_name: j.name.clone(), pocket: p.id.clone(), number: p.number }, outline, b)
            })
        })
        .collect();
    for o in &doc.objects {
        if out.contains_key(&o.id) {
            continue;
        }
        let path = o.field_path();
        if path.elements().is_empty() {
            continue;
        }
        let b = path.bounding_box();
        if !(b.x0.is_finite() && b.y0.is_finite() && b.x1.is_finite() && b.y1.is_finite()) {
            continue;
        }
        let hit = pockets.iter().find(|(_, outline, pb)| {
            pb.contains(kurbo::Point::new(b.x0, b.y0)) && pb.contains(kurbo::Point::new(b.x1, b.y1)) && outline.contains(b.center())
        });
        if let Some((pl, _, _)) = hit {
            out.insert(o.id.clone(), pl.clone());
        }
    }
    out
}

/// The pre-flight checks a project has switched off ([`crate::Document::preflight`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preflight {
    /// Ids of the checks not to warn about in this project (`cut_before_fill`, `empty_layer`, ...).
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub disabled: BTreeSet<String>,
}

impl Preflight {
    pub fn is_default(&self) -> bool {
        self.disabled.is_empty()
    }

    pub fn enabled(&self, check: &str) -> bool {
        !self.disabled.contains(check)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jig() -> Jig {
        Jig {
            id: "jig-1".into(),
            name: "Coins".into(),
            datum: [10.0, 0.0],
            rotation_deg: 90.0,
            material_height_mm: Some(3.0),
            pockets: vec![
                Pocket { id: "p1".into(), number: 1, center: [5.0, 0.0], rotation_deg: 0.0, shape: PocketShape::Circle { diameter_mm: 4.0 }, enabled: true },
                Pocket {
                    id: "p2".into(),
                    number: 2,
                    center: [15.0, 0.0],
                    rotation_deg: 0.0,
                    shape: PocketShape::Rect { width_mm: 4.0, height_mm: 2.0, corner_radius_mm: 0.0 },
                    enabled: false,
                },
            ],
            keep_out: vec![KeepOut { id: "k1".into(), name: "Clamp".into(), path: Rect::new(0.0, 0.0, 1.0, 1.0).to_path(0.01), enabled: true }],
            outline: None,
        }
    }

    #[test]
    fn pockets_follow_the_datum_and_rotation() {
        let j = jig();
        let c = j.pocket_center(&j.pockets[0]);
        assert!((c.x - 10.0).abs() < 1e-9 && (c.y - 5.0).abs() < 1e-9, "{c:?}");
        let b = j.pocket_outline(&j.pockets[1]).bounding_box();
        // The 4 x 2 pocket turned 90 degrees with the jig: 2 wide, 4 tall, centred at (10, 15).
        assert!((b.width() - 2.0).abs() < 1e-6 && (b.height() - 4.0).abs() < 1e-6, "{b:?}");
        assert_eq!(j.enabled_pockets().len(), 1);
        assert_eq!(j.keep_out_zones()[0].0, "Coins / Clamp");
        assert!(j.problem().is_none());
    }

    #[test]
    fn serde_keeps_defaults_out_and_reads_them_back() {
        let j = jig();
        let s = serde_json::to_string(&j).unwrap();
        assert!(!s.contains("\"enabled\":true"), "{s}");
        assert!(s.contains("\"enabled\":false"));
        let back: Jig = serde_json::from_str(&s).unwrap();
        assert_eq!(back, j);
        let mut bad = j.clone();
        bad.pockets[1].number = 1;
        assert!(bad.problem().unwrap().contains("used twice"));
        bad.pockets[1].number = 2;
        bad.datum[0] = f64::NAN;
        assert!(bad.problem().is_some());
    }

    #[test]
    fn preflight_settings() {
        let mut p = Preflight::default();
        assert!(p.is_default() && p.enabled("empty_layer"));
        p.disabled.insert("empty_layer".into());
        assert!(!p.enabled("empty_layer"));
        assert_eq!(serde_json::to_string(&p).unwrap(), r#"{"disabled":["empty_layer"]}"#);
    }
}
