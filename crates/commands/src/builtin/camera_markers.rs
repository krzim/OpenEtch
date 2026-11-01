//! Camera alignment markers (JOB-007): `generate.camera_markers` adds square fiducials in the ArUco layout (a black
//! border one cell wide around a 4 x 4 grid of black/white cells, 6 x 6 cells in all) whose cells carry an id, so
//! the engine's camera can find them in a frame and align the camera to the field without clicking
//! (`engine::markers`, `POST /api/camera/align_markers`).
//!
//! The ids come from OpenEtch's own 16-code dictionary ([`CODES`]), chosen so every two codes, in every rotation,
//! differ in at least 5 of the 16 cells and no code looks like itself turned (so a marker's id and its rotation read
//! unambiguously with one cell misread). OpenCV's ArUco dictionaries (e.g. `DICT_4X4_50`) use the same layout but
//! other codes; their tables are not included, so markers printed by other tools are not recognised.

use doc::{CutSettings, Document, FillMode, Geometry, Layer, Object, ObjectId};
use kurbo::{Affine, BezPath};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::layer::{SettingsPatch, validate_settings};
use super::util::{self, invalid, transact};
use crate::{Command, Result, Risk};

/// The marker codes: bit 15 is the top-left inner cell, then row by row; a 1 is a black (marked) cell.
pub const CODES: [u16; 16] = [
    0x9c92, 0x2494, 0x9b56, 0xaa1e, 0x51e8, 0xe59b, 0xc279, 0x3be1, 0xc6d2, 0x5483, 0x987a, 0xaed7, 0x5ac2, 0x71d6, 0x4c25, 0x123e,
];
/// Cells along a marker's side (border included).
pub const CELLS: usize = 6;
/// Object `extras` key holding a marker object's id.
pub const MARKER_KEY: &str = "camera.marker";

/// The 6 x 6 cells of marker `id` (row 0 at the top, `true` = black).
pub fn marker_cells(id: usize) -> Option<[[bool; CELLS]; CELLS]> {
    let code = *CODES.get(id)?;
    let mut cells = [[true; CELLS]; CELLS];
    for (r, row) in cells.iter_mut().enumerate().take(5).skip(1) {
        for (c, cell) in row.iter_mut().enumerate().take(5).skip(1) {
            *cell = code >> (15 - ((r - 1) * 4 + (c - 1))) & 1 == 1;
        }
    }
    Some(cells)
}

/// The 4 x 4 code turned a quarter clockwise.
pub fn rotate_code(code: u16) -> u16 {
    let mut out = 0u16;
    for r in 0..4 {
        for c in 0..4 {
            // New (r, c) is old (3 - c, r).
            let bit = code >> (15 - ((3 - c) * 4 + r)) & 1;
            out |= bit << (15 - (r * 4 + c));
        }
    }
    out
}

/// The dictionary entry nearest `code` in any rotation, within `max_errors` cells: `(id, quarter turns clockwise
/// that bring the code read to the dictionary's, errors)`.
pub fn match_code(code: u16, max_errors: u32) -> Option<(usize, u32, u32)> {
    let mut best: Option<(usize, u32, u32)> = None;
    let mut c = code;
    for turns in 0..4 {
        for (id, want) in CODES.iter().enumerate() {
            let e = (c ^ want).count_ones();
            if e <= max_errors && best.is_none_or(|b| e < b.2) {
                best = Some((id, turns, e));
            }
        }
        c = rotate_code(c);
    }
    best
}

/// Marker `id` as a closed outline of its black cells, `size` mm wide, centred on the origin, +Y up.
pub fn marker_path(id: usize, size: f64) -> Option<BezPath> {
    let cells = marker_cells(id)?;
    let cell = size / CELLS as f64;
    let mut p = BezPath::new();
    for (r, row) in cells.iter().enumerate() {
        // Runs of black cells in this row as rectangles (they only touch, so an even-odd fill marks them all).
        let mut c = 0;
        while c < CELLS {
            if !row[c] {
                c += 1;
                continue;
            }
            let start = c;
            while c < CELLS && row[c] {
                c += 1;
            }
            let (x0, x1) = (-size / 2.0 + start as f64 * cell, -size / 2.0 + c as f64 * cell);
            let (y1, y0) = (size / 2.0 - r as f64 * cell, size / 2.0 - (r + 1) as f64 * cell);
            p.move_to((x0, y0));
            p.line_to((x1, y0));
            p.line_to((x1, y1));
            p.line_to((x0, y1));
            p.close_path();
        }
    }
    Some(p)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CameraMarkersInput {
    /// Marker centres in field mm, one marker each (ids 0, 1, ... in order), 4..=16. Default: four markers at 35 %
    /// of the workspace towards its corners (top-left, top-right, bottom-right, bottom-left).
    pub positions: Option<Vec<[f64; 2]>>,
    /// Marker side in mm (6 cells; keep a cell well above the camera's resolution). Default 8.
    pub size_mm: Option<f64>,
    /// Settings of the markers' layer (fill mode, partial over the defaults: fill, 0.05 mm interval).
    pub settings: Option<SettingsPatch>,
}

/// `generate.camera_markers`.
pub struct CameraMarkers;

impl Command for CameraMarkers {
    type Input = CameraMarkersInput;
    const ID: &'static str = "generate.camera_markers";
    const TITLE: &'static str = "Camera alignment markers";
    const DESCRIPTION: &'static str = "Add camera alignment markers (JOB-007): square ArUco-layout fiducials from \
        OpenEtch's dictionary on their own fill layer, one per position (ids 0, 1, ... in order; default four at 35 % \
        towards the workspace corners), size_mm wide (default 8). Mark them on the material; the engine then finds \
        them in a camera frame and aligns the camera (POST /api/camera/align_markers). Each object keeps its id in \
        extras camera.marker. Returns the created ids, layer, group and markers [{id, object, center}].";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: CameraMarkersInput) -> Result<(Value, String)> {
        let cmd = Self::ID;
        let size = util::positive(cmd, "size_mm", i.size_mm.unwrap_or(8.0))?;
        if size > 100.0 {
            return Err(invalid(cmd, format!("size_mm must be at most 100 (got {size})")));
        }
        let positions = match i.positions {
            Some(p) => p,
            None => {
                let [w, h] = doc.workspace().ok_or_else(|| invalid(cmd, "the document has no field size; pass positions"))?;
                let (x, y) = (w * 0.35, h * 0.35);
                vec![[-x, y], [x, y], [x, -y], [-x, -y]]
            }
        };
        if !(4..=CODES.len()).contains(&positions.len()) {
            return Err(invalid(cmd, format!("give 4..={} positions (got {})", CODES.len(), positions.len())));
        }
        for (n, p) in positions.iter().enumerate() {
            util::finite2(cmd, "positions", *p)?;
            if let Some(q) = positions[..n].iter().find(|q| (q[0] - p[0]).hypot(q[1] - p[1]) < size * 1.5) {
                return Err(invalid(cmd, format!("markers at {p:?} and {q:?} overlap: keep them at least 1.5 x size_mm apart")));
            }
        }
        let mut s = CutSettings { mode: FillMode::Fill, ..CutSettings::default() };
        s.hatch.interval_mm = 0.05;
        if let Some(p) = i.settings {
            p.apply(cmd, &mut s)?;
        }
        validate_settings(cmd, &s)?;
        transact(doc, |doc| {
            let lid = doc.alloc_id("layer")?;
            let mut l = Layer::new(lid.clone(), "Camera markers", "#202020");
            l.settings = s;
            doc.layers.push(l);
            let mut created: Vec<ObjectId> = Vec::new();
            let mut markers = Vec::new();
            for (id, p) in positions.iter().enumerate() {
                let path = marker_path(id, size).ok_or_else(|| invalid(cmd, "no such marker"))?;
                let oid = doc.alloc_id("obj")?;
                let mut o = Object::new(oid.clone(), format!("Camera marker {id}"), lid.clone(), Geometry::Path { path }, Affine::translate((p[0], p[1])));
                o.extras.insert(MARKER_KEY.into(), id.to_string());
                doc.objects.push(o);
                markers.push(json!({ "id": id, "object": oid, "center": p }));
                created.push(oid);
            }
            let gid = doc.alloc_id("group")?;
            doc.groups.push(doc::Group::new(gid.clone(), "Camera markers", None));
            for id in &created {
                if let Some(o) = doc.object_mut(id) {
                    o.group = Some(gid.clone());
                }
            }
            let mut r = util::edit_result(doc, &created, &[]);
            r["layer"] = json!(lid);
            r["group"] = json!(gid);
            r["markers"] = json!(markers);
            Ok((r, format!("added {} camera alignment markers", created.len())))
        })
    }
}

/// The marker objects of `doc` (made by `generate.camera_markers`): `(id, field centre)`, last one per id.
pub fn document_markers(doc: &Document) -> Vec<(usize, [f64; 2])> {
    let mut out: Vec<(usize, [f64; 2])> = Vec::new();
    for o in &doc.objects {
        let Some(id) = o.extras.get(MARKER_KEY).and_then(|v| v.parse::<usize>().ok()).filter(|i| *i < CODES.len()) else {
            continue;
        };
        let c = o.bounds().center();
        out.retain(|(i, _)| *i != id);
        out.push((id, [c.x, c.y]));
    }
    out.sort_by_key(|(i, _)| *i);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dictionary_reads_unambiguously() {
        for (i, a) in CODES.iter().enumerate() {
            let mut r = *a;
            for turn in 1..4 {
                r = rotate_code(r);
                assert!((r ^ a).count_ones() >= 5, "code {i} looks like itself turned {turn} times");
            }
            for b in &CODES[i + 1..] {
                let mut r = *b;
                for _ in 0..4 {
                    assert!((r ^ a).count_ones() >= 5, "{a:#06x} vs {b:#06x}");
                    r = rotate_code(r);
                }
            }
        }
        assert_eq!(rotate_code(rotate_code(rotate_code(rotate_code(0x1234)))), 0x1234);
        // A turned, once misread code still names its marker and turn.
        let turned = rotate_code(CODES[5]) ^ 0x0100;
        assert_eq!(match_code(turned, 1), Some((5, 3, 1)));
        assert_eq!(match_code(CODES[5] ^ 0x0303, 1), None);
    }

    #[test]
    fn markers_are_added_where_asked() {
        let mut d = Document { field_mm: Some(100.0), ..Document::default() };
        let (r, _) = CameraMarkers::apply(&mut d, serde_json::from_value(json!({"size_mm": 6})).unwrap()).unwrap();
        assert_eq!(r["markers"].as_array().unwrap().len(), 4);
        let m = document_markers(&d);
        assert_eq!(m.iter().map(|(i, _)| *i).collect::<Vec<_>>(), [0, 1, 2, 3]);
        assert!((m[1].1[0] - 35.0).abs() < 1e-9 && (m[1].1[1] - 35.0).abs() < 1e-9, "{m:?}");
        let o = d.objects.iter().find(|o| o.extras.get(MARKER_KEY).map(String::as_str) == Some("0")).unwrap();
        assert!((o.bounds().width() - 6.0).abs() < 1e-9);
        let bad = |v: Value| CameraMarkers::apply(&mut d.clone(), serde_json::from_value(v).unwrap()).is_err();
        assert!(bad(json!({"positions": [[0, 0], [1, 0], [20, 0], [30, 0]]})), "overlapping");
        assert!(bad(json!({"positions": [[0, 0], [20, 0], [40, 0]]})), "too few");
        assert!(bad(json!({"size_mm": 0})));
    }
}
