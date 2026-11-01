//! Helpers shared by the path, node and generator commands: object geometry in the field frame, writing a field-frame
//! path back into an object's own frame, new path objects, and `geom` errors as command errors.

use doc::{Document, Geometry, Object, ObjectId};
use geom::GeomError;
use kurbo::{Affine, BezPath, Point, Shape};

use super::util::{self, invalid};
use crate::{CommandError, Result};

/// Most objects one path command may create, so a crafted input cannot flood the document.
pub const MAX_CREATED: usize = 10_000;

pub fn geom_err(cmd: &str) -> impl Fn(GeomError) -> CommandError + '_ {
    move |e| match e {
        GeomError::Invalid(m) => invalid(cmd, m),
        GeomError::Engine(m) => CommandError::Failed(format!("{cmd}: the geometry engine failed ({m})")),
    }
}

/// The object's outline in the field frame. Images are refused: they have no vector outline to edit.
pub fn vector_path(cmd: &str, o: &Object) -> Result<BezPath> {
    Ok(o.transform * vector_local(cmd, o)?)
}

/// A geometry's outline in its own frame as the path tools see it: like [`Geometry::local_path`], but a sharp
/// rectangle is just its four lines (the rounded-rectangle outline adds zero-length corner curves, which would show
/// up as extra nodes).
pub fn clean_local(g: &Geometry) -> BezPath {
    match *g {
        Geometry::Rect { width_mm, height_mm, corner_radius_mm: 0.0 } => {
            kurbo::Rect::from_center_size((0.0, 0.0), (width_mm, height_mm)).to_path(0.01)
        }
        _ => g.local_path(),
    }
}

/// The object's outline in its own (local) frame, as a path; images refused.
pub fn vector_local(cmd: &str, o: &Object) -> Result<BezPath> {
    if matches!(o.geometry, Geometry::Image { .. }) {
        return Err(invalid(cmd, format!("{} (\"{}\") is an image; this works on vector shapes only", o.id, o.name)));
    }
    Ok(clean_local(&o.geometry))
}

/// Replace an object's geometry with `local` (already in its own frame). It becomes a plain path: data kept from
/// other software about its old shape (e.g. LightBurn text or primitive attributes) is dropped, since it no longer
/// describes the shape (LightBurn turns text and primitives into paths on these edits too).
pub fn set_local_path(doc: &mut Document, id: &str, local: BezPath) -> Result<()> {
    let o = doc.object_mut(id).ok_or_else(|| CommandError::Failed(format!("object {id} vanished")))?;
    o.geometry = Geometry::Path { path: local };
    o.extras.clear();
    Ok(())
}

/// Replace an object's geometry with a field-frame path, mapped into the object's frame (its transform is kept).
pub fn set_field_path(doc: &mut Document, id: &str, field: &BezPath) -> Result<()> {
    let t = doc.object(id).ok_or_else(|| CommandError::Failed(format!("object {id} vanished")))?.transform;
    if t.determinant() == 0.0 {
        return Err(CommandError::Failed(format!("object {id} has a transform that cannot be inverted")));
    }
    set_local_path(doc, id, t.inverse() * field.clone())
}

/// Add a path object from a field-frame path: its local origin is the path's bounds centre, its transform a plain
/// translation. Returns the new id.
pub fn add_field_object(
    doc: &mut Document,
    name: &str,
    layer: &str,
    group: Option<String>,
    field: &BezPath,
) -> Result<ObjectId> {
    let c = if field.elements().is_empty() { Point::ORIGIN } else { field.bounding_box().center() };
    let local = Affine::translate(-c.to_vec2()) * field.clone();
    let id = doc.alloc_id("obj")?;
    let mut o = Object::new(id.clone(), name, layer.to_string(), Geometry::Path { path: local }, Affine::translate(c.to_vec2()));
    o.group = group;
    doc.objects.push(o);
    Ok(id)
}

/// Delete objects (already checked unlocked), repair groups and locks. Returns the removed group ids.
pub fn delete_objects(doc: &mut Document, ids: &[ObjectId]) -> Vec<String> {
    doc.objects.retain(|o| !ids.contains(&o.id));
    doc.prune_locked();
    doc.prune_groups()
}

/// Exactly one object id (groups refused), unlocked when `edit`.
pub fn single_object(cmd: &str, doc: &Document, id: &str, edit: bool) -> Result<Object> {
    if doc.object(id).is_none() && doc.group(id).is_some() {
        return Err(invalid(cmd, format!("{id} is a group; give one object id (ungroup first)")));
    }
    let o = util::object(doc, id)?.clone();
    if edit {
        util::check_unlocked(cmd, doc, std::slice::from_ref(&o.id))?;
    }
    Ok(o)
}

/// A field point as JSON-friendly `[x, y]` from a point in the object's local frame.
pub fn to_field(t: Affine, p: Point) -> serde_json::Value {
    util::pt_json(t * p)
}

/// A field point `[x, y]` into the object's local frame.
pub fn to_local(cmd: &str, field: &str, t: Affine, p: [f64; 2]) -> Result<Point> {
    let [x, y] = util::finite2(cmd, field, p)?;
    Ok(t.inverse() * Point::new(x, y))
}
