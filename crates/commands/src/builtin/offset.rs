//! `path.offset`: LightBurn's Offset Shapes.

use doc::{Document, ObjectId};
use geom::offset::{Join, offset_polyline, offset_region};
use geom::region::{Fill, Region};
use kurbo::BezPath;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::pathutil::{self, MAX_CREATED, geom_err};
use super::util::{self, invalid, transact};
use crate::{Command, CommandError, Result, Risk};

/// Largest offset distance accepted, in mm.
const MAX_DISTANCE_MM: f64 = 1000.0;

/// Corner style of the offset.
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OffsetJoin {
    /// Arcs around outward corners (the true offset). Default.
    #[default]
    Round,
    /// Sharp corners (LightBurn's "Corner"), bevelled past `miter_limit`.
    Miter,
    /// Corners cut off straight.
    Bevel,
}

/// Which way to offset.
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OffsetDirection {
    /// Away from the shapes (holes shrink). Default.
    #[default]
    Outward,
    /// Into the shapes (closed shapes only; parts thinner than twice the distance vanish).
    Inward,
    /// Both: two results per shape.
    Both,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OffsetInput {
    /// Exact object or group ids (a group stands for all its members). Images are refused.
    pub ids: Vec<ObjectId>,
    /// Offset distance in mm, |d| <= 1000. Negative flips the direction (-1 outward = 1 inward). 0 rebuilds the
    /// outline without overlaps.
    pub distance_mm: f64,
    /// Corner style: round (default), miter or bevel.
    #[serde(default)]
    pub join: OffsetJoin,
    /// For miter: the longest a corner may reach, in multiples of the distance, before it is bevelled (>= 1,
    /// default 4).
    pub miter_limit: Option<f64>,
    /// Keep the original shapes (default true; LightBurn's "Delete original objects" is keep_original false).
    #[serde(default = "yes")]
    pub keep_original: bool,
    /// Merge all shapes first and make one result object (default false: one result per shape and direction).
    #[serde(default)]
    pub combine: bool,
    /// outward (default), inward or both.
    #[serde(default)]
    pub direction: OffsetDirection,
}

fn yes() -> bool {
    true
}

/// `path.offset`.
pub struct PathOffset;

impl Command for PathOffset {
    type Input = OffsetInput;
    const ID: &'static str = "path.offset";
    const TITLE: &'static str = "Offset shapes";
    const DESCRIPTION: &'static str = "Make outlines at a distance (mm, field frame) around or inside shapes \
        (LightBurn's Offset Shapes). Closed shapes are filled even-odd and grown or shrunk (holes move the other \
        way); an open path offset outward becomes a closed band around it with round ends; inward skips open paths. \
        New objects go on the source's layer named \"<name> offset\"; results are polylines within 0.01 mm (an \
        offset circle is a many-sided polygon). Returns created ids, `skipped` sources with reasons, deleted ids \
        (keep_original false) and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: OffsetInput) -> Result<(Value, String)> {
        transact(doc, |doc| offset(doc, i))
    }
}

fn offset(doc: &mut Document, i: OffsetInput) -> Result<(Value, String)> {
    let cmd = PathOffset::ID;
    let ids = util::object_ids(cmd, doc, &i.ids)?;
    let d = util::finite(cmd, "distance_mm", i.distance_mm)?;
    if d.abs() > MAX_DISTANCE_MM {
        return Err(invalid(cmd, format!("distance_mm must be within ±{MAX_DISTANCE_MM}")));
    }
    let join = match i.join {
        OffsetJoin::Round => Join::Round,
        OffsetJoin::Bevel => Join::Bevel,
        OffsetJoin::Miter => {
            let limit = util::finite(cmd, "miter_limit", i.miter_limit.unwrap_or(4.0))?;
            if limit < 1.0 {
                return Err(invalid(cmd, "miter_limit must be >= 1"));
            }
            Join::Miter { limit }
        }
    };
    if i.miter_limit.is_some() && i.join != OffsetJoin::Miter {
        return Err(invalid(cmd, "miter_limit is only for join \"miter\""));
    }
    if !i.keep_original {
        util::check_unlocked(cmd, doc, &ids)?;
    }
    let signs: Vec<(f64, &str)> = match i.direction {
        OffsetDirection::Outward => vec![(d, "offset")],
        OffsetDirection::Inward => vec![(-d, "inset")],
        OffsetDirection::Both => vec![(d, "offset"), (-d, "inset")],
    };
    // Per source: id, name, layer, its closed region and its open polylines.
    type Source = (ObjectId, String, String, Region, Vec<Vec<kurbo::Point>>);
    let mut sources: Vec<Source> = Vec::new();
    for id in &ids {
        let o = util::object(doc, id)?;
        let path = pathutil::vector_path(cmd, o)?;
        let region = Region::from_path(&path, Fill::EvenOdd).map_err(geom_err(cmd))?;
        let open: Vec<Vec<kurbo::Point>> = geom::flatten(&path, geom::TOLERANCE)
            .map_err(geom_err(cmd))?
            .into_iter()
            .filter(|l| !l.closed)
            .map(|l| l.points)
            .collect();
        sources.push((o.id.clone(), o.name.clone(), o.layer.clone(), region, open));
    }
    let grow = |region: &Region, open: &[Vec<kurbo::Point>], dist: f64| -> Result<Option<Region>> {
        let mut parts = Vec::new();
        if !region.is_empty() {
            parts.push(offset_region(region, dist, join).map_err(geom_err(cmd))?);
        }
        if dist > 0.0 {
            for l in open {
                parts.push(offset_polyline(l, dist, join).map_err(geom_err(cmd))?);
            }
        }
        let r = Region::union_all(parts).map_err(geom_err(cmd))?;
        Ok((!r.is_empty()).then_some(r))
    };
    let mut created: Vec<ObjectId> = Vec::new();
    let mut skipped: Vec<Value> = Vec::new();
    let mut outputs: Vec<(String, String, BezPath)> = Vec::new();
    if i.combine {
        let region = Region::union_all(sources.iter().map(|s| s.3.clone())).map_err(geom_err(cmd))?;
        let open: Vec<Vec<kurbo::Point>> = sources.iter().flat_map(|s| s.4.iter().cloned()).collect();
        for &(dist, label) in &signs {
            match grow(&region, &open, dist)? {
                Some(r) => outputs.push((format!("{} {label}", sources[0].1), sources[0].2.clone(), r.to_path())),
                None => skipped.push(json!({ "id": sources[0].0, "reason": format!("the {label} is empty") })),
            }
        }
    } else {
        for (id, name, layer, region, open) in &sources {
            for &(dist, label) in &signs {
                match grow(region, open, dist)? {
                    Some(r) => outputs.push((format!("{name} {label}"), layer.clone(), r.to_path())),
                    None => {
                        let reason = if region.is_empty() && dist <= 0.0 {
                            "open paths can only be offset outward".to_string()
                        } else {
                            format!("the {label} is empty (the shape is thinner than twice the distance)")
                        };
                        skipped.push(json!({ "id": id, "reason": reason }));
                    }
                }
            }
        }
    }
    if outputs.is_empty() {
        let why = skipped.first().and_then(|s| s["reason"].as_str()).unwrap_or("nothing to offset").to_string();
        return Err(CommandError::Failed(format!("{cmd}: no result ({why})")));
    }
    if outputs.len() > MAX_CREATED {
        return Err(CommandError::Failed(format!("{cmd}: more than {MAX_CREATED} results")));
    }
    for (name, layer, path) in &outputs {
        created.push(pathutil::add_field_object(doc, name, layer, None, path)?);
    }
    let mut deleted = Vec::new();
    if !i.keep_original {
        deleted = ids.clone();
        pathutil::delete_objects(doc, &deleted);
    }
    let mut r = util::edit_result(doc, &created, &[]);
    r["skipped"] = json!(skipped);
    r["deleted"] = json!(deleted);
    Ok((r, format!("offset {} by {d} mm into {}", ids.join(", "), created.join(", "))))
}
