//! The document model. Everything is in millimetres and degrees, in the field frame (origin at the galvo field
//! centre, +X right, +Y away from the operator). See ARCHITECTURE.md.
//!
//! This crate is plain data plus geometry helpers. It never changes itself: all edits go through `commands`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use kurbo::{Affine, BezPath, PathEl, Point, Rect, Shape};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub mod file;
pub mod job;
pub mod text;

pub use job::{
    CutDirection, CutOptions, DotMode, FillGrouping, ObjectOptions, Optimization, OrderBy, Perforation, Rotary, RotaryKind,
    StartPoint, Tabs, Taper, Wobble,
};
pub use text::{AlignH, AlignV, CsvData, FontKind, FontSpec, Text, TextPath, VarMode, VarText, VariableState};

pub type ObjectId = String;
pub type LayerId = String;
pub type GroupId = String;

/// Numbered ids ("obj-12") stay below 2^53, so every JSON reader (including ones that hold numbers as doubles) sees
/// them exactly, and the counter can never wrap.
pub const MAX_ID: u64 = 1 << 53;

/// [`Document::alloc_id`] ran out of ids (only reachable from a crafted file with a huge `next_id`).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("the document has used up its object ids (next id {0}); save it and start a new document")]
pub struct IdsExhausted(pub u64);

/// `^[A-Za-z_][A-Za-z0-9_.-]*$`: a name that is also a valid XML element name, so exporters can write it as a tag.
pub fn is_plain_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// Whether `key` can be an `extras` key ([`Layer::extras`], [`Object::extras`], [`Group::extras`],
/// [`Document::extras`]): `<format>.<name>` (e.g. `lbrn2.minPower`, `lbrn2.sub1.speed`), both parts
/// [plain names](is_plain_name), at most 128 bytes.
pub fn is_extras_key(key: &str) -> bool {
    key.len() <= 128 && key.split_once('.').is_some_and(|(format, name)| is_plain_name(format) && is_plain_name(name))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    /// Bumped by every successful command; clients use it to detect changes.
    pub revision: u64,
    pub name: String,
    /// Galvo field size in mm the document was laid out for. The engine fills it in from the machine profile for
    /// new and opened documents that have none; LightBurn import/export uses it to move between frames.
    #[serde(default)]
    pub field_mm: Option<f64>,
    /// Workspace `[width, height]` in mm of a bed (gantry) device, which need not be square, or of a galvo with a
    /// non-square field (LightBurn's separate field Width and Height; `extras` `lbrn2._origin` is then `center` and
    /// `field_mm` the width); `None` for a square galvo, whose workspace is the square `field_mm` (for a bed,
    /// `field_mm` is the largest square inside it, used where a square size is needed). The field frame's origin is
    /// the workspace's centre either way. Missing in older files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_mm: Option<[f64; 2]>,
    pub stock: Option<Stock>,
    pub layers: Vec<Layer>,
    pub objects: Vec<Object>,
    /// Groups of objects ([`Object::group`]), nested through [`Group::parent`]. A group always has at least one
    /// member object (directly or in a nested group): [`Document::prune_groups`] removes empty ones.
    #[serde(default)]
    pub groups: Vec<Group>,
    /// Counter for stable ids ("obj-1", "layer-1", "group-1").
    pub next_id: u64,
    /// Data from other software that OpenEtch does not model (e.g. LightBurn's `UIPrefs`, `Notes`, thumbnail and root
    /// attributes as `lbrn2.*`), kept so exports round-trip. Keys pass [`is_extras_key`]. Never used for planning.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extras: BTreeMap<String, String>,
    /// Locked objects (LightBurn's Lock Shapes): commands refuse to move, transform, reshape or delete them until
    /// `object.lock` unlocks them. Kept on the document rather than on [`Object`] so every existing `Object { .. }`
    /// literal stays valid; ids that no longer name an object mean nothing ([`Document::is_locked`]).
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub locked: BTreeSet<ObjectId>,
    /// Variable text state (LightBurn's Variable Text window: serial counter, CSV rows), shared by every variable
    /// [`Text`]. Left out of JSON while it is the default.
    #[serde(default, skip_serializing_if = "VariableState::is_default")]
    pub variables: VariableState,
    /// LightBurn's Optimization Settings (cut order), saved with the project. Left out of JSON while default.
    #[serde(default, skip_serializing_if = "Optimization::is_default")]
    pub optimization: Optimization,
    /// Per-object cut properties (cut order priority, power scale), by object id. Kept on the document, like
    /// `locked`, so every `Object { .. }` literal stays valid; ids that no longer name an object mean nothing.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub object_options: BTreeMap<ObjectId, ObjectOptions>,
    /// Galvo rotary setup saved with the project (LightBurn's "Save/Load Rotary Settings with Project Files").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotary: Option<Rotary>,
    /// Project notes (LightBurn's Notes window, `<Notes ShowOnLoad Notes>` in `.lbrn2`). Left out while empty.
    #[serde(default, skip_serializing_if = "Notes::is_default")]
    pub notes: Notes,
    /// LightBurn 2.2's Pages: the document's pages in order. Empty means one implicit page. The **active** page's
    /// shapes are the document's own `objects`/`groups` (so every command, preview and job works on it); every other
    /// page keeps its shapes in its [`Page`] entry, and the active entry's own lists stay empty. Layers, cut
    /// settings and every other document setting are shared by all pages, as in LightBurn.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pages: Vec<Page>,
    /// Id of the active page in `pages` (ignored while `pages` is empty).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_page: Option<String>,
    /// LightBurn's Apply Mask to Image: image object id → the closed vector object (or group) that masks it. The
    /// mask stays a live, editable shape; deleting it (or `image.remove_mask`) shows the whole image again. Jobs
    /// engrave only the part of the image inside the mask (even-odd). Ids that no longer name an image and an
    /// existing mask mean nothing ([`Document::image_mask`]).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub image_masks: BTreeMap<ObjectId, ObjectId>,
    /// Manual guidelines (LightBurn's guides dragged out of the rulers), saved with the project so every browser sees
    /// them. Left out of JSON while empty. They are drawing aids only: never planned, exported or burned.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guides: Vec<Guide>,
    /// Curve tolerance for job output in mm (LightBurn's Curve Tolerance): how far a flattened polyline may stray from
    /// a curve. `None` uses the device's setting, else [`DEFAULT_CURVE_TOLERANCE_MM`]. Left out of JSON while `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curve_tolerance_mm: Option<f64>,
    /// LightBurn's virtual arrays (Grid Array > Create Virtual Array): linked clones of their source objects that
    /// follow every edit of the sources. The clones are ordinary objects (so jobs, previews and exports need nothing
    /// new) marked with [`VIRTUAL_ARRAY_KEY`]; the `commands` crate lays them out again after every edit. Left out of
    /// JSON while empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub virtual_arrays: Vec<VirtualArray>,
}

/// Object extras key marking a virtual array's clone; the value is the array's id.
pub const VIRTUAL_ARRAY_KEY: &str = "openetch.virtual_array";

/// One virtual array (see [`Document::virtual_arrays`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VirtualArray {
    /// Stable id ("array-4").
    pub id: String,
    /// The original objects, in order (LightBurn: the array's originals; they stay editable).
    pub sources: Vec<ObjectId>,
    /// The clone objects, cell by cell (row-major, the originals' cell left out), each cell one clone per source.
    pub clones: Vec<ObjectId>,
    /// The grid settings (the `commands` crate's `art.array` settings, as JSON).
    pub settings: serde_json::Value,
}

/// Most guides a document keeps (`guide.add` refuses more; a loaded file with more is refused).
pub const MAX_GUIDES: usize = 1000;

/// Largest |position| of a guide, mm (1 km).
pub const MAX_GUIDE_MM: f64 = 1e6;

/// Default output curve tolerance in mm (finer than LightBurn's 0.05 mm default; galvo marks show facets sooner).
pub const DEFAULT_CURVE_TOLERANCE_MM: f64 = 0.01;

/// Allowed output curve tolerances in mm: finer than 0.001 mm only multiplies points, coarser than 1 mm visibly
/// facets small curves.
pub const CURVE_TOLERANCE_RANGE_MM: std::ops::RangeInclusive<f64> = 0.001..=1.0;

/// One manual guideline: a vertical (`axis: x`, the line `x = position_mm`) or horizontal (`axis: y`, the line
/// `y = position_mm`) line across the workspace, in field mm.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Guide {
    /// Stable id ("guide-3"), allocated from the document's id counter.
    pub id: String,
    pub axis: GuideAxis,
    pub position_mm: f64,
}

/// Which coordinate a [`Guide`] fixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GuideAxis {
    /// A vertical guide at `x = position_mm` (dragged from the top ruler).
    X,
    /// A horizontal guide at `y = position_mm` (dragged from the left ruler).
    Y,
}

/// Project notes (LightBurn's File > Show Notes, Ctrl+Alt+N).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Notes {
    pub text: String,
    /// Show the notes when the project is opened (LightBurn's "Show on File Open").
    pub show_on_load: bool,
}

impl Notes {
    pub fn is_default(&self) -> bool {
        self.text.is_empty() && !self.show_on_load
    }
}

/// One page of a multi-page document (see [`Document::pages`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page {
    pub id: String,
    pub name: String,
    /// The page's shapes while it is not the active page (empty for the active page).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub objects: Vec<Object>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<Group>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub locked: BTreeSet<ObjectId>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub object_options: BTreeMap<ObjectId, ObjectOptions>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub image_masks: BTreeMap<ObjectId, ObjectId>,
}

impl Default for Document {
    fn default() -> Self {
        let mut doc = Document {
            revision: 0,
            name: "Untitled".into(),
            field_mm: None,
            workspace_mm: None,
            stock: None,
            layers: vec![],
            objects: vec![],
            groups: vec![],
            next_id: 1,
            extras: BTreeMap::new(),
            locked: BTreeSet::new(),
            variables: VariableState::default(),
            optimization: Optimization::default(),
            object_options: BTreeMap::new(),
            rotary: None,
            notes: Notes::default(),
            pages: Vec::new(),
            active_page: None,
            image_masks: BTreeMap::new(),
            guides: Vec::new(),
            curve_tolerance_mm: None,
            virtual_arrays: Vec::new(),
        };
        let id = doc.alloc_id("layer").expect("a new document has ids to spare");
        doc.layers.push(Layer::new(id, "Layer 1", "#1f77b4"));
        doc
    }
}

impl Document {
    /// The mask object of image `image` ([`Document::image_masks`]), when both still exist.
    pub fn image_mask(&self, image: &str) -> Option<&ObjectId> {
        let m = self.image_masks.get(image)?;
        let is_image = self.object(image).is_some_and(|o| matches!(o.geometry, Geometry::Image { .. }));
        (is_image && (self.object(m).is_some() || self.group(m).is_some())).then_some(m)
    }

    /// The workspace `[width, height]` in mm: `workspace_mm`, else the square `field_mm`, else `None`.
    pub fn workspace(&self) -> Option<[f64; 2]> {
        self.workspace_mm.or(self.field_mm.map(|f| [f, f]))
    }

    /// A fresh `"{prefix}-{n}"` id. Ids stay below [`MAX_ID`]; past that this fails instead of wrapping.
    pub fn alloc_id(&mut self, prefix: &str) -> Result<String, IdsExhausted> {
        let n = self.next_id;
        let next = n.checked_add(1).filter(|v| *v <= MAX_ID).ok_or(IdsExhausted(n))?;
        self.next_id = next;
        Ok(format!("{prefix}-{n}"))
    }

    /// The first unusable number in the document: NaN or infinity anywhere (field size, stock, layer settings, object
    /// geometry, transforms, and object outlines in the field frame), or an object transform that cannot be inverted.
    /// `None` when every number is usable. JSON cannot store such values and planning cannot use them, so commands
    /// refuse to leave a document in this state.
    pub fn numeric_problem(&self) -> Option<String> {
        if let Some(f) = self.field_mm
            && !f.is_finite()
        {
            return Some(format!("field_mm is {f}"));
        }
        if let Some(g) = self.guides.iter().find(|g| !g.position_mm.is_finite()) {
            return Some(format!("guide {} is at {}", g.id, g.position_mm));
        }
        if let Some(t) = self.curve_tolerance_mm
            && !CURVE_TOLERANCE_RANGE_MM.contains(&t)
        {
            return Some(format!("curve_tolerance_mm is {t} (allowed 0.001..=1)"));
        }
        if let Some([w, h]) = self.workspace_mm
            && !(w.is_finite() && h.is_finite())
        {
            return Some(format!("workspace_mm is [{w}, {h}]"));
        }
        if let Some(s) = &self.stock {
            let dims = match s.shape {
                StockShape::Rect { width_mm, height_mm, corner_radius_mm } => {
                    vec![width_mm, height_mm, corner_radius_mm]
                }
                StockShape::Circle { diameter_mm } => vec![diameter_mm],
            };
            let placement = [s.thickness_mm, s.center[0], s.center[1], s.rotation_deg];
            if dims.into_iter().chain(placement).any(|v| !v.is_finite()) {
                return Some("the stock has a number that is not finite".into());
            }
        }
        for l in &self.layers {
            for s in std::iter::once(&l.settings).chain(&l.sub_layers) {
                if let Some(field) = s.non_finite_field() {
                    return Some(format!("layer {} (\"{}\"): {field} is not a finite number", l.id, l.name));
                }
            }
        }
        self.objects
            .iter()
            .find_map(|o| o.placement_problem().map(|p| format!("object {} (\"{}\"): {p}", o.id, o.name)))
    }

    /// Whether object `id` exists and is locked.
    pub fn is_locked(&self, id: &str) -> bool {
        self.locked.contains(id) && self.object(id).is_some()
    }

    /// Forget locks of objects that no longer exist.
    pub fn prune_locked(&mut self) {
        let ids: BTreeSet<&str> = self.objects.iter().map(|o| o.id.as_str()).collect();
        self.locked.retain(|id| ids.contains(id.as_str()));
    }

    pub fn object(&self, id: &str) -> Option<&Object> {
        self.objects.iter().find(|o| o.id == id)
    }

    pub fn object_mut(&mut self, id: &str) -> Option<&mut Object> {
        self.objects.iter_mut().find(|o| o.id == id)
    }

    pub fn layer(&self, id: &str) -> Option<&Layer> {
        self.layers.iter().find(|l| l.id == id)
    }

    pub fn layer_mut(&mut self, id: &str) -> Option<&mut Layer> {
        self.layers.iter_mut().find(|l| l.id == id)
    }

    /// Union of the field-frame bounding boxes of the given objects (a group id counts as all its members).
    pub fn bounds_of(&self, ids: &[ObjectId]) -> Option<Rect> {
        self.expand_group_ids(ids).iter().filter_map(|id| self.object(id)).map(Object::bounds).reduce(|a, b| a.union(b))
    }

    pub fn group(&self, id: &str) -> Option<&Group> {
        self.groups.iter().find(|g| g.id == id)
    }

    pub fn group_mut(&mut self, id: &str) -> Option<&mut Group> {
        self.groups.iter_mut().find(|g| g.id == id)
    }

    /// `group` and its ancestors, innermost first. Stops at a missing parent or a cycle (a sound document has
    /// neither).
    pub fn group_chain(&self, group: &str) -> Vec<&Group> {
        let mut out: Vec<&Group> = Vec::new();
        let mut next = self.group(group);
        while let Some(g) = next {
            if out.iter().any(|o| o.id == g.id) {
                break;
            }
            out.push(g);
            next = g.parent.as_deref().and_then(|p| self.group(p));
        }
        out
    }

    /// Whether `group` is `ancestor` or nested (at any depth) inside it.
    pub fn group_is_within(&self, group: &str, ancestor: &str) -> bool {
        self.group_chain(group).iter().any(|g| g.id == ancestor)
    }

    /// The outermost group an object belongs to (what a click on it selects), if any.
    pub fn top_group_of(&self, object: &str) -> Option<&Group> {
        let g = self.object(object)?.group.as_deref()?;
        self.group_chain(g).last().copied()
    }

    /// Ids of every object in `group` or its nested groups, in document (drawing) order.
    pub fn group_members(&self, group: &str) -> Vec<ObjectId> {
        self.objects
            .iter()
            .filter(|o| o.group.as_deref().is_some_and(|g| self.group_is_within(g, group)))
            .map(|o| o.id.clone())
            .collect()
    }

    /// `ids` with every group id replaced by its members ([`group_members`](Self::group_members)) and duplicates
    /// removed (the first occurrence stays). Ids that name neither a group nor an object are kept, so the caller's
    /// lookup can report them.
    pub fn expand_group_ids(&self, ids: &[String]) -> Vec<ObjectId> {
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        for id in ids {
            let members = if self.object(id).is_none() && self.group(id).is_some() {
                self.group_members(id)
            } else {
                vec![id.clone()]
            };
            for m in members {
                if seen.insert(m.clone()) {
                    out.push(m);
                }
            }
        }
        out
    }

    /// Repair the groups after objects or groups were removed: a reference to a group that no longer exists is
    /// cleared, and groups left without any member object (directly or through nested groups) are removed. Returns
    /// the ids of the removed groups. Commands that delete objects call this, so deleting the last member of a group
    /// removes the group.
    pub fn prune_groups(&mut self) -> Vec<GroupId> {
        let ids: BTreeSet<String> = self.groups.iter().map(|g| g.id.clone()).collect();
        for o in &mut self.objects {
            if o.group.as_ref().is_some_and(|g| !ids.contains(g)) {
                o.group = None;
            }
        }
        for g in &mut self.groups {
            if g.parent.as_ref().is_some_and(|p| !ids.contains(p) || *p == g.id) {
                g.parent = None;
            }
        }
        let mut used = BTreeSet::new();
        for o in &self.objects {
            if let Some(g) = &o.group {
                used.extend(self.group_chain(g).into_iter().map(|c| c.id.clone()));
            }
        }
        let removed: Vec<GroupId> =
            self.groups.iter().filter(|g| !used.contains(&g.id)).map(|g| g.id.clone()).collect();
        self.groups.retain(|g| used.contains(&g.id));
        removed
    }
}

/// A set of objects (and nested groups) that selects and moves as one unit, as in LightBurn. Objects point at their
/// innermost group ([`Object::group`]); groups nest through `parent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Group {
    pub id: GroupId,
    pub name: String,
    /// The enclosing group, if nested.
    #[serde(default)]
    pub parent: Option<GroupId>,
    /// Data from other software (e.g. a LightBurn group's `XForm` as `lbrn2.*`), kept so exports round-trip.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extras: BTreeMap<String, String>,
}

impl Group {
    pub fn new(id: GroupId, name: impl Into<String>, parent: Option<GroupId>) -> Self {
        Group { id, name: name.into(), parent, extras: BTreeMap::new() }
    }
}

/// The physical piece being marked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Stock {
    pub name: String,
    pub shape: StockShape,
    /// Material thickness. The B6's Z lift is not readable over USB, so this is always user-entered.
    pub thickness_mm: f64,
    /// Free text or a material-library key, e.g. "stainless 304".
    pub material: Option<String>,
    /// Centre of the stock in the field frame.
    pub center: [f64; 2],
    pub rotation_deg: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StockShape {
    Rect { width_mm: f64, height_mm: f64, corner_radius_mm: f64 },
    Circle { diameter_mm: f64 },
}

impl Stock {
    /// Outline in the field frame.
    pub fn outline(&self) -> BezPath {
        let local = match self.shape {
            StockShape::Rect { width_mm, height_mm, corner_radius_mm } => {
                let r = Rect::from_center_size((0.0, 0.0), (width_mm, height_mm));
                r.to_rounded_rect(corner_radius_mm).to_path(0.01)
            }
            StockShape::Circle { diameter_mm } => kurbo::Circle::new((0.0, 0.0), diameter_mm / 2.0).to_path(0.01),
        };
        self.transform() * local
    }

    pub fn transform(&self) -> Affine {
        Affine::translate((self.center[0], self.center[1])) * Affine::rotate(self.rotation_deg.to_radians())
    }

    pub fn bounds(&self) -> Rect {
        self.outline().bounding_box()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    /// CSS colour used for display.
    pub color: String,
    pub visible: bool,
    /// Excluded from jobs when false.
    pub output: bool,
    pub settings: CutSettings,
    /// The whole layer (main settings and sub-layers, in their burn order) runs this many times (LightBurn's global
    /// passes). Missing in older files: 1 (left out of JSON).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub global_passes: u32,
    /// Extra passes with their own settings (LightBurn sub-layers), run after `settings` in order; a sub-layer with
    /// [`CutSettings::cleanup`] runs between the main passes instead (see [`Cleanup`]).
    pub sub_layers: Vec<CutSettings>,
    /// Settings from other software that OpenEtch does not model (e.g. `lbrn2.minPower`), kept so exports
    /// round-trip. Keys are prefixed with the format they came from. Never used for planning.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extras: BTreeMap<String, String>,
    /// The material library preset this layer is linked to (LightBurn's Link): its settings follow the preset and
    /// are not edited directly. `None` (left out of JSON): not linked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<PresetLink>,
    /// LightBurn's tool layers T1 / T2 (LAY-042): a layer for guides, alignment marks and framing outlines that never
    /// outputs (planning skips it whatever `output` says, and commands keep `output` off), is framed with the job,
    /// and exports as LightBurn's tool layer of that number. `None` (left out of JSON): an ordinary layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<ToolLayer>,
}

/// Which of LightBurn's two tool layers a layer is ([`Layer::tool`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum ToolLayer {
    T1,
    T2,
}

impl ToolLayer {
    /// LightBurn's `CutIndex` (30 and 31).
    pub fn index(self) -> u32 {
        match self {
            ToolLayer::T1 => 30,
            ToolLayer::T2 => 31,
        }
    }

    /// The tool layer of LightBurn index `index`, if it is one.
    pub fn from_index(index: u32) -> Option<Self> {
        match index {
            30 => Some(ToolLayer::T1),
            31 => Some(ToolLayer::T2),
            _ => None,
        }
    }

    /// "T1" or "T2".
    pub fn name(self) -> &'static str {
        match self {
            ToolLayer::T1 => "T1",
            ToolLayer::T2 => "T2",
        }
    }
}

/// A layer's link to a material library preset (LightBurn's `LinkPath`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PresetLink {
    /// The library's name in the engine's library store (its file name without `.clb`).
    pub library: String,
    /// `material/thickness or title/description`, LightBurn's `LinkPath`.
    pub path: String,
}

fn one() -> u32 {
    1
}

fn is_one(v: &u32) -> bool {
    *v == 1
}

impl Layer {
    pub fn new(id: LayerId, name: &str, color: &str) -> Self {
        Layer {
            id,
            name: name.into(),
            color: color.into(),
            visible: true,
            output: true,
            settings: CutSettings::default(),
            global_passes: 1,
            sub_layers: vec![],
            extras: BTreeMap::new(),
            link: None,
            tool: None,
        }
    }

    /// Whether the layer can mark: output on and not a tool layer.
    pub fn emits(&self) -> bool {
        self.output && self.tool.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FillMode {
    /// Trace outlines.
    Line,
    /// Hatch the inside of closed shapes.
    Fill,
    /// Hatch, then trace outlines.
    FillThenLine,
    /// Concentric inward offsets of closed outlines at the hatch interval (LightBurn's Offset Fill): the outline
    /// itself, then copies shrunk by one interval at a time until nothing is left. Scan angle, cross-hatch and
    /// bidirectional do not apply.
    OffsetFill,
}

/// Per-layer laser parameters. Mirrors LightBurn's galvo cut settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct CutSettings {
    pub mode: FillMode,
    /// 0..=100.
    pub power_pct: f64,
    pub speed_mm_s: f64,
    pub frequency_khz: f64,
    /// MOPA pulse width. `None` leaves the source at its current setting.
    pub pulse_width_ns: Option<f64>,
    pub passes: u32,
    pub hatch: Hatch,
    /// `None` uses the machine profile's defaults.
    pub timing: Option<GalvoTiming>,
    /// Display name of a sub-layer (LightBurn's `subname`, e.g. "Cleanup"); `None` shows "Sub-layer n".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Makes a sub-layer a cleanup pass (LightBurn's `isCleanup`): it runs between the main passes instead of after
    /// them. Only meaningful on a sub-layer; ignored on a layer's main settings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<Cleanup>,
    /// How the layer's images ([`Geometry::Image`]) are engraved (LightBurn's Image mode). As in LightBurn, images on
    /// a layer always use these settings while its vectors keep [`CutSettings::mode`]; power, speed, frequency, pulse
    /// width, timing and passes are shared.
    pub image: ImageSettings,
    /// Cut options beyond the laser parameters: sub-layer output, pauses, dot mode, wobble, fill grouping and dot
    /// width, auto-rotate, flood fill, perforation, tabs, Z offset and step ([`CutOptions`]; empty JSON object while
    /// default).
    pub options: CutOptions,
}

/// A cleanup sub-layer: LightBurn's "Enable cleanup pass" (3D Sliced image mode) adds a Cleanup sub-layer that runs
/// after every "Clean after # of passes" passes of the main settings to clear debris, usually with a tighter
/// interval, higher frequency and lower power. OpenEtch runs it after main passes `n`, `2n`, ... and once more after
/// the last main pass when that one was not followed by a cleanup already, so the layer always ends cleaned (with
/// `n` at least the layer's passes it runs once, after all of them, as an ordinary sub-layer would). Each run does
/// the sub-layer's own `passes`. Cleanup sub-layers run before the layer's other sub-layers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Cleanup {
    /// Main passes between cleanups, at least 1 (LightBurn's "Clean after # of passes", stored as the cleanup
    /// sub-layer's `numPasses`).
    pub every_n_passes: u32,
}

impl Cleanup {
    /// After which main passes (1-based, ascending) a cleanup with this interval runs, for a layer of `main_passes`
    /// passes: every `every_n_passes`-th, plus the last. `every_n_passes == 0` counts as 1.
    pub fn runs_after(&self, main_passes: u32) -> impl Iterator<Item = u32> {
        let n = self.every_n_passes.max(1);
        (1..=main_passes).filter(move |k| k % n == 0 || *k == main_passes)
    }
}

impl CutSettings {
    /// The name of the first field holding NaN or infinity, if any.
    pub fn non_finite_field(&self) -> Option<&'static str> {
        let mut fields = vec![
            ("power_pct", self.power_pct),
            ("speed_mm_s", self.speed_mm_s),
            ("frequency_khz", self.frequency_khz),
            ("pulse_width_ns", self.pulse_width_ns.unwrap_or(0.0)),
            ("hatch.interval_mm", self.hatch.interval_mm),
            ("hatch.angle_deg", self.hatch.angle_deg),
            ("hatch.angle_increment_deg", self.hatch.angle_increment_deg),
            ("image.interval_mm", self.image.interval_mm),
            ("image.angle_deg", self.image.angle_deg),
            ("image.angle_increment_deg", self.image.angle_increment_deg),
            ("image.dot_width_mm", self.image.dot_width_mm),
            ("image.min_power_pct", self.image.min_power_pct),
            ("image.halftone_cells_per_inch", self.image.halftone_cells_per_inch),
            ("image.halftone_angle_deg", self.image.halftone_angle_deg),
        ];
        if let Some(t) = &self.timing {
            fields.extend([
                ("timing.laser_on_delay_us", t.laser_on_delay_us),
                ("timing.laser_off_delay_us", t.laser_off_delay_us),
                ("timing.polygon_delay_us", t.polygon_delay_us),
                ("timing.end_delay_us", t.end_delay_us),
                ("timing.jump_speed_mm_s", t.jump_speed_mm_s),
                ("timing.min_jump_delay_us", t.min_jump_delay_us),
                ("timing.max_jump_delay_us", t.max_jump_delay_us),
                ("timing.jump_distance_limit_mm", t.jump_distance_limit_mm),
            ]);
        }
        fields.into_iter().find(|(_, v)| !v.is_finite()).map(|(f, _)| f)
    }
}

impl Default for CutSettings {
    fn default() -> Self {
        CutSettings {
            mode: FillMode::Line,
            power_pct: 20.0,
            speed_mm_s: 1000.0,
            frequency_khz: 30.0,
            pulse_width_ns: None,
            passes: 1,
            hatch: Hatch::default(),
            timing: None,
            name: None,
            cleanup: None,
            image: ImageSettings::default(),
            options: CutOptions::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Hatch {
    /// Line spacing.
    pub interval_mm: f64,
    pub angle_deg: f64,
    /// Added to the angle on every pass (EzCad "auto-rotate hatch angle").
    pub angle_increment_deg: f64,
    /// Second hatch at +90°.
    pub cross_hatch: bool,
    /// Alternate direction on every line instead of always marking the same way.
    pub bidirectional: bool,
}

impl Default for Hatch {
    fn default() -> Self {
        Hatch { interval_mm: 0.05, angle_deg: 0.0, angle_increment_deg: 0.0, cross_hatch: false, bidirectional: true }
    }
}

/// LightBurn's image modes (`ditherMode` in `.lbrn2`): how grey levels become dots. Every mode but `Grayscale` turns
/// each dot on or off at the layer's power; `Grayscale` varies the power per dot between
/// [`ImageSettings::min_power_pct`] (lightest) and the layer's power (darkest), never firing on pure white.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DitherMode {
    /// On where darker than 50 % grey. For two-colour art.
    Threshold,
    /// 8 x 8 Bayer matrix: a regular pattern.
    Ordered,
    /// Atkinson error diffusion (passes on 6/8 of the error: crisp, loses some highlight and shadow detail).
    Atkinson,
    /// Floyd-Steinberg error diffusion (LightBurn's "Dither", `dithered` in its files; `dither` and `dithered` are
    /// accepted here too).
    #[serde(alias = "dither", alias = "dithered")]
    FloydSteinberg,
    /// Stucki error diffusion.
    Stucki,
    /// Jarvis-Judice-Ninke error diffusion (LightBurn: usually the best for photos).
    #[default]
    Jarvis,
    /// A fixed 45° clustered-dot screen with cells 4 dots wide (darker = bigger spots).
    Newsprint,
    /// Round dots on a screen of [`ImageSettings::halftone_cells_per_inch`] at [`ImageSettings::halftone_angle_deg`].
    Halftone,
    /// Only the edges, where light and dark meet (line drawings, handwriting).
    Sketch,
    /// Power varies with darkness (see the type docs).
    Grayscale,
    /// LightBurn's 3D Sliced mode (galvo only, `3dslice` in its files): the image is a depth map, darker = deeper.
    /// The main passes engrave one grey-level slice each: pass k of N fires where the darkness is above
    /// (N - k) / N, so the first pass marks only the deepest areas and every later pass a larger area, and a dot
    /// of darkness d gets ceil(d · N) passes (pure white none). A cleanup sub-layer cleans the area of the last
    /// main pass, or all of it with [`ImageSettings::clean_whole_image`].
    #[serde(alias = "3dslice")]
    Slice3d,
}

impl DitherMode {
    pub const ALL: [DitherMode; 11] = [
        DitherMode::Threshold,
        DitherMode::Ordered,
        DitherMode::Atkinson,
        DitherMode::FloydSteinberg,
        DitherMode::Stucki,
        DitherMode::Jarvis,
        DitherMode::Newsprint,
        DitherMode::Halftone,
        DitherMode::Sketch,
        DitherMode::Grayscale,
        DitherMode::Slice3d,
    ];

    /// The name LightBurn writes for this mode in `.lbrn2` files (`ditherMode`).
    pub fn lightburn_name(self) -> &'static str {
        match self {
            DitherMode::Threshold => "threshold",
            DitherMode::Ordered => "ordered",
            DitherMode::Atkinson => "atkinson",
            DitherMode::FloydSteinberg => "dithered",
            DitherMode::Stucki => "stucki",
            DitherMode::Jarvis => "jarvis",
            DitherMode::Newsprint => "newsprint",
            DitherMode::Halftone => "halftone",
            DitherMode::Sketch => "sketch",
            DitherMode::Grayscale => "grayscale",
            DitherMode::Slice3d => "3dslice",
        }
    }

    /// The mode a LightBurn `ditherMode` names (ignoring case; `dither` reads as `dithered`), if OpenEtch has it.
    pub fn from_lightburn(name: &str) -> Option<DitherMode> {
        let n = name.trim().to_ascii_lowercase();
        let n = if n == "dither" { "dithered" } else { n.as_str() };
        DitherMode::ALL.into_iter().find(|m| m.lightburn_name() == n)
    }
}

/// LightBurn's Image mode settings (Cut Settings Editor, Image mode). Defaults follow LightBurn where its docs state
/// them (scan angle 0, angle increment 0); the others are OpenEtch's choices. LightBurn leaves values at its defaults
/// out of its files, which shows only that its default dither is none of `dithered`, `stucki` and `3dslice`, and
/// its default halftone angle is not 22.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ImageSettings {
    pub dither: DitherMode,
    /// Scan line spacing in mm, also the dot pitch along a line: the image is resampled to square dots of this size.
    /// DPI = 25.4 / interval ([`ImageSettings::dpi`]). Ignored with `pass_through`.
    pub interval_mm: f64,
    /// Scan direction in degrees counter-clockwise from +X. 0 scans along +X with rows going from the bottom up
    /// (LightBurn: "0 = horizontal, bottom to top"; 180 scans top to bottom; 90 vertically, left to right).
    pub angle_deg: f64,
    /// Added to the scan angle on every pass (90 reproduces a cross-hatch).
    pub angle_increment_deg: f64,
    /// Engrave rows in alternating directions; off engraves every row in the scan direction and travels back.
    pub bidirectional: bool,
    /// Invert the image so light areas engrave (dark anodised, painted or slate surfaces).
    pub negative: bool,
    /// Engrave the image's own pixels as they are (an image already processed elsewhere): one dot per pixel, rows
    /// along the image's own rows, no resampling; `interval_mm` and `angle_deg` are then not used.
    pub pass_through: bool,
    /// Dot width correction in mm: every run is shortened by this much (half at each end) so dark areas do not
    /// bloom (LightBurn: about half the beam width, 0..=interval). 0 is off.
    pub dot_width_mm: f64,
    /// Grayscale only: the power of the lightest dot that still fires, in % (the layer's power is the darkest).
    pub min_power_pct: f64,
    /// Halftone only: screen cells per inch.
    pub halftone_cells_per_inch: f64,
    /// Halftone only: screen angle in degrees.
    pub halftone_angle_deg: f64,
    /// 3D Sliced only (LightBurn's "Clean whole image"): a cleanup sub-layer cleans every engraved area instead of
    /// only the slice of the main pass just before it.
    pub clean_whole_image: bool,
}

impl Default for ImageSettings {
    fn default() -> Self {
        ImageSettings {
            dither: DitherMode::Jarvis,
            interval_mm: 0.05,
            angle_deg: 0.0,
            angle_increment_deg: 0.0,
            bidirectional: true,
            negative: false,
            pass_through: false,
            dot_width_mm: 0.0,
            min_power_pct: 0.0,
            halftone_cells_per_inch: 50.0,
            halftone_angle_deg: 22.5,
            clean_whole_image: false,
        }
    }
}

impl ImageSettings {
    /// Dots per inch of the line interval (25.4 / `interval_mm`).
    pub fn dpi(&self) -> f64 {
        25.4 / self.interval_mm
    }
}

/// LightBurn's Adjust Image values of one image (Shape Properties, Image; the `Gamma Contrast Brightness
/// EnhanceRadius EnhanceAmount EnhanceDenoise` attributes of a `.lbrn2` Bitmap). Applied to the image's grey levels
/// before it is dithered, in this order: enhance, contrast, brightness, gamma. The defaults change nothing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ImageAdjust {
    /// Mid-tones: grey^gamma, so above 1 darkens them and below 1 lightens them (LightBurn: "lower values lighten the
    /// mid-tones and higher values darken them"). 0.01..=10.
    pub gamma: f64,
    /// -100..=100 %: light tones lighter and dark tones darker (LightBurn's files store a fraction: 0.2 = 20 %).
    pub contrast: f64,
    /// -100..=100 %: every tone lighter or darker (stored as a fraction like contrast).
    pub brightness: f64,
    /// Enhance (unsharp mask) radius in image pixels, 0..=100; 0 is off.
    pub enhance_radius: f64,
    /// Enhance strength in %, 0..=10000 (LightBurn users report around 400 for a clear effect); 0 is off.
    pub enhance_amount: f64,
    /// Enhance denoise: differences smaller than this many grey levels (of 255) are not sharpened.
    pub denoise: f64,
}

impl Default for ImageAdjust {
    fn default() -> Self {
        ImageAdjust { gamma: 1.0, contrast: 0.0, brightness: 0.0, enhance_radius: 0.0, enhance_amount: 0.0, denoise: 0.0 }
    }
}

impl ImageAdjust {
    pub fn is_default(&self) -> bool {
        *self == ImageAdjust::default()
    }

    /// Why these values cannot be used (a number that is not finite or out of its range), if so.
    pub fn problem(&self) -> Option<String> {
        let fields = [
            ("gamma", self.gamma, 0.01, 10.0),
            ("contrast", self.contrast, -100.0, 100.0),
            ("brightness", self.brightness, -100.0, 100.0),
            ("enhance_radius", self.enhance_radius, 0.0, 100.0),
            ("enhance_amount", self.enhance_amount, 0.0, 10_000.0),
            ("denoise", self.denoise, 0.0, 255.0),
        ];
        fields
            .into_iter()
            .find(|(_, v, lo, hi)| !(v.is_finite() && (*lo..=*hi).contains(v)))
            .map(|(f, v, lo, hi)| format!("adjust.{f} must be within {lo}..={hi} (got {v})"))
    }
}

/// Galvo timing constants in microseconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GalvoTiming {
    pub laser_on_delay_us: f64,
    pub laser_off_delay_us: f64,
    pub polygon_delay_us: f64,
    pub end_delay_us: f64,
    pub jump_speed_mm_s: f64,
    pub min_jump_delay_us: f64,
    pub max_jump_delay_us: f64,
    pub jump_distance_limit_mm: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Object {
    pub id: ObjectId,
    pub name: String,
    pub layer: LayerId,
    /// Geometry in object-local coordinates.
    pub geometry: Geometry,
    /// Local → field frame.
    pub transform: Affine,
    /// The innermost [`Group`] this object belongs to.
    #[serde(default)]
    pub group: Option<GroupId>,
    /// Data from other software that OpenEtch does not model (e.g. LightBurn text attributes as `lbrn2.*`), kept so
    /// exports round-trip. Keys pass [`is_extras_key`]. Never used for planning.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extras: BTreeMap<String, String>,
}

impl Object {
    /// An ungrouped object without extras.
    pub fn new(id: ObjectId, name: impl Into<String>, layer: LayerId, geometry: Geometry, transform: Affine) -> Self {
        Object { id, name: name.into(), layer, geometry, transform, group: None, extras: BTreeMap::new() }
    }

    /// Geometry in the field frame.
    pub fn field_path(&self) -> BezPath {
        self.transform * self.geometry.local_path()
    }

    pub fn bounds(&self) -> Rect {
        self.field_path().bounding_box()
    }

    /// Why this object cannot be placed in the field: a non-finite number in its geometry or transform, a transform
    /// that cannot be inverted, or an outline that overflows to infinity in the field frame. `None` when it is sound.
    /// Linear in the path size and allocation-free.
    pub fn placement_problem(&self) -> Option<&'static str> {
        let t = self.transform;
        if t.as_coeffs().iter().any(|v| !v.is_finite()) {
            return Some("its transform has a number that is not finite");
        }
        let det = t.determinant();
        if !(det.is_finite() && det != 0.0) {
            return Some("its transform cannot be inverted (zero or overflowing scale)");
        }
        let ok = |p: Point| p.x.is_finite() && p.y.is_finite() && {
            let q = t * p;
            q.x.is_finite() && q.y.is_finite()
        };
        // The outline lies within the hull of these points, so they bound it.
        let corners = |x: f64, y: f64| [(-x, -y), (x, -y), (x, y), (-x, y)].map(Point::from);
        let sound = match &self.geometry {
            Geometry::Rect { width_mm, height_mm, corner_radius_mm } => {
                corner_radius_mm.is_finite() && corners(width_mm / 2.0, height_mm / 2.0).into_iter().all(ok)
            }
            Geometry::Ellipse { rx_mm, ry_mm } => corners(*rx_mm, *ry_mm).into_iter().all(ok),
            Geometry::Polygon { radius_mm, corner_radius_mm, star_ratio, .. } => {
                corner_radius_mm.is_finite()
                    && star_ratio.is_none_or(f64::is_finite)
                    && corners(*radius_mm, *radius_mm).into_iter().all(ok)
            }
            Geometry::Image { width_mm, height_mm, adjust, .. } => {
                let a = adjust;
                [a.gamma, a.contrast, a.brightness, a.enhance_radius, a.enhance_amount, a.denoise]
                    .iter()
                    .all(|v| v.is_finite())
                    && corners(width_mm / 2.0, height_mm / 2.0).into_iter().all(ok)
            }
            Geometry::Path { path } => path.elements().iter().all(|el| match *el {
                PathEl::MoveTo(p) | PathEl::LineTo(p) => ok(p),
                PathEl::QuadTo(a, p) => ok(a) && ok(p),
                PathEl::CurveTo(a, b, p) => ok(a) && ok(b) && ok(p),
                PathEl::ClosePath => true,
            }),
            Geometry::Text(t) => {
                t.numbers().iter().all(|(_, v)| v.is_finite())
                    && t.outline.elements().iter().all(|el| match *el {
                        PathEl::MoveTo(p) | PathEl::LineTo(p) => ok(p),
                        PathEl::QuadTo(a, p) => ok(a) && ok(p),
                        PathEl::CurveTo(a, b, p) => ok(a) && ok(b) && ok(p),
                        PathEl::ClosePath => true,
                    })
            }
        };
        (!sound).then_some("its size or outline is not finite in the field frame")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Geometry {
    /// Centred on the local origin.
    Rect { width_mm: f64, height_mm: f64, corner_radius_mm: f64 },
    /// Centred on the local origin.
    Ellipse { rx_mm: f64, ry_mm: f64 },
    /// Arbitrary path (imported vector art, text outlines).
    Path { path: BezPath },
    /// A regular polygon or star (LightBurn's Polygon and Star tools), kept parametric so Shape Properties can edit
    /// it (`shape.update`): `sides` corners (3..=1000) on a circle of `radius_mm` around the local origin, the first
    /// corner straight up (+Y), counter-clockwise; with `star_ratio` (0 < r < 1) a star whose inner corners lie at
    /// `radius_mm * star_ratio`. `corner_radius_mm` rounds every corner where it fits (others stay sharp). The
    /// outline is [`Geometry::local_path`] (`geom::shapes`); LightBurn has no polygon shape in `.lbrn2`, so exports
    /// write it as a path.
    Polygon {
        sides: u32,
        radius_mm: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        star_ratio: Option<f64>,
        #[serde(default)]
        corner_radius_mm: f64,
    },
    /// A bitmap, `width_mm` x `height_mm` centred on the local origin (the transform may scale, rotate or mirror it).
    /// `data_base64` is the encoded file (of type `mime`, e.g. `image/png`) in standard base64. It is shared, so
    /// copies of the document (undo steps) do not copy the image. Jobs engrave it with its layer's
    /// [`ImageSettings`], after `adjust` (LightBurn's Adjust Image; left out of JSON while it changes nothing).
    Image {
        width_mm: f64,
        height_mm: f64,
        mime: String,
        data_base64: Arc<str>,
        #[serde(default, skip_serializing_if = "ImageAdjust::is_default")]
        adjust: ImageAdjust,
    },
    /// Live text (LightBurn's text objects): settings plus its rendered outline ([`Text::outline`]), which is what
    /// [`Geometry::local_path`] returns, so planning never needs fonts.
    Text(Box<Text>),
}

impl Geometry {
    pub fn local_path(&self) -> BezPath {
        match *self {
            Geometry::Rect { width_mm, height_mm, corner_radius_mm } => {
                Rect::from_center_size((0.0, 0.0), (width_mm, height_mm)).to_rounded_rect(corner_radius_mm).to_path(0.01)
            }
            Geometry::Ellipse { rx_mm, ry_mm } => kurbo::Ellipse::new((0.0, 0.0), (rx_mm, ry_mm), 0.0).to_path(0.01),
            Geometry::Path { ref path } => path.clone(),
            Geometry::Polygon { sides, radius_mm, star_ratio, corner_radius_mm } => {
                polygon_outline(sides, radius_mm, star_ratio, corner_radius_mm).unwrap_or_default()
            }
            Geometry::Text(ref t) => t.outline.clone(),
            Geometry::Image { width_mm, height_mm, .. } => {
                Rect::from_center_size((0.0, 0.0), (width_mm, height_mm)).to_path(0.01)
            }
        }
    }

    /// A short lowercase name of the kind: `rect`, `ellipse`, `polygon`, `path`, `image` or `text`.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Geometry::Rect { .. } => "rect",
            Geometry::Ellipse { .. } => "ellipse",
            Geometry::Polygon { .. } => "polygon",
            Geometry::Path { .. } => "path",
            Geometry::Image { .. } => "image",
            Geometry::Text(_) => "text",
        }
    }
}

/// Why polygon parameters cannot describe a shape (`None` when they can): sides outside 3..=1000, a radius that is
/// not a positive number up to 1e6 mm, a star ratio outside (0, 1), a negative or non-finite corner radius.
pub fn polygon_problem(sides: u32, radius_mm: f64, star_ratio: Option<f64>, corner_radius_mm: f64) -> Option<String> {
    if !(3..=geom::shapes::MAX_SIDES).contains(&sides) {
        return Some(format!("sides must be 3..={} (got {sides})", geom::shapes::MAX_SIDES));
    }
    if !(radius_mm.is_finite() && radius_mm > 0.0 && radius_mm <= 1e6) {
        return Some(format!("radius_mm must be > 0 and at most 1e6 (got {radius_mm})"));
    }
    if let Some(r) = star_ratio
        && !(r.is_finite() && r > 0.0 && r < 1.0)
    {
        return Some(format!("star_ratio must be between 0 and 1 exclusive (got {r})"));
    }
    if !(corner_radius_mm.is_finite() && corner_radius_mm >= 0.0) {
        return Some(format!("corner_radius_mm must be >= 0 (got {corner_radius_mm})"));
    }
    None
}

/// The outline of a [`Geometry::Polygon`] in its own frame, `None` when the parameters are unusable.
pub fn polygon_outline(sides: u32, radius_mm: f64, star_ratio: Option<f64>, corner_radius_mm: f64) -> Option<BezPath> {
    if polygon_problem(sides, radius_mm, star_ratio, corner_radius_mm).is_some() {
        return None;
    }
    let pts = geom::shapes::polygon_points(sides, radius_mm, star_ratio).ok()?;
    Some(geom::shapes::polygon_path(&pts, corner_radius_mm).0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polygon_geometry_is_parametric_and_round_trips() {
        let g = Geometry::Polygon { sides: 6, radius_mm: 10.0, star_ratio: None, corner_radius_mm: 0.0 };
        let p = g.local_path();
        let expect = 1.5 * 3f64.sqrt() * 100.0;
        assert!((p.area() - expect).abs() < 1e-9, "{}", p.area());
        assert_eq!(g.kind_name(), "polygon");
        let json = serde_json::to_value(&g).unwrap();
        let want = serde_json::json!({"kind": "polygon", "sides": 6, "radius_mm": 10.0, "corner_radius_mm": 0.0});
        assert_eq!(json, want);
        assert_eq!(serde_json::from_value::<Geometry>(json).unwrap(), g);
        let star = Geometry::Polygon { sides: 5, radius_mm: 10.0, star_ratio: Some(0.5), corner_radius_mm: 1.0 };
        assert!(star.local_path().area() > 0.0 && star.local_path().area() < p.area());
        let bad = Geometry::Polygon { sides: 2, radius_mm: 10.0, star_ratio: None, corner_radius_mm: 0.0 };
        assert!(bad.local_path().elements().is_empty());
        assert!(polygon_problem(5, 1.0, Some(1.0), 0.0).is_some());
        assert!(polygon_problem(5, f64::NAN, None, 0.0).is_some());
        assert!(polygon_problem(5, 1.0, None, -1.0).is_some());
        assert!(polygon_problem(5, 1.0, Some(0.4), 0.2).is_none());
    }

    #[test]
    fn guides_and_curve_tolerance_round_trip_and_are_checked() {
        let mut doc = Document::default();
        assert!(!serde_json::to_string(&doc).unwrap().contains("guides"), "left out while empty");
        doc.guides.push(Guide { id: "guide-2".into(), axis: GuideAxis::X, position_mm: 12.5 });
        doc.curve_tolerance_mm = Some(0.05);
        let back: Document = serde_json::from_str(&serde_json::to_string(&doc).unwrap()).unwrap();
        assert_eq!(back, doc);
        assert_eq!(doc.numeric_problem(), None);
        doc.curve_tolerance_mm = Some(5.0);
        assert!(doc.numeric_problem().unwrap().contains("curve_tolerance_mm"));
        doc.curve_tolerance_mm = None;
        doc.guides[0].position_mm = f64::NAN;
        assert!(doc.numeric_problem().unwrap().contains("guide-2"));
    }

    #[test]
    fn default_document_has_one_layer() {
        let doc = Document::default();
        assert_eq!(doc.layers.len(), 1);
        assert_eq!(doc.layers[0].id, "layer-1");
        assert_eq!(doc.next_id, 2);
    }

    #[test]
    fn object_bounds_follow_transform() {
        let obj = Object::new(
            "obj-1".into(),
            "r",
            "layer-1".into(),
            Geometry::Rect { width_mm: 10.0, height_mm: 4.0, corner_radius_mm: 0.0 },
            Affine::translate((5.0, -2.0)),
        );
        let b = obj.bounds();
        assert!((b.x0 - 0.0).abs() < 1e-9 && (b.x1 - 10.0).abs() < 1e-9);
        assert!((b.y0 + 4.0).abs() < 1e-9 && (b.y1 - 0.0).abs() < 1e-9);
    }

    #[test]
    fn document_round_trips_through_json() {
        let mut doc = Document {
            field_mm: Some(153.75),
            stock: Some(Stock {
                name: "tag".into(),
                shape: StockShape::Circle { diameter_mm: 30.0 },
                thickness_mm: 1.0,
                material: Some("stainless 304".into()),
                center: [0.0, 0.0],
                rotation_deg: 0.0,
            }),
            ..Document::default()
        };
        doc.layers[0].extras.insert("lbrn2.minPower".into(), "12".into());
        let json = serde_json::to_string(&doc).unwrap();
        let back: Document = serde_json::from_str(&json).unwrap();
        assert_eq!(doc, back);
    }

    #[test]
    fn alloc_id_stops_below_max_id_instead_of_overflowing() {
        let mut doc = Document { next_id: MAX_ID - 1, ..Document::default() };
        assert_eq!(doc.alloc_id("obj").unwrap(), format!("obj-{}", MAX_ID - 1));
        assert_eq!(doc.alloc_id("obj"), Err(IdsExhausted(MAX_ID)));
        let mut doc = Document { next_id: u64::MAX, ..Document::default() };
        assert_eq!(doc.alloc_id("obj"), Err(IdsExhausted(u64::MAX)), "no add-with-overflow panic");
        assert_eq!(doc.next_id, u64::MAX, "unchanged on error");
    }

    #[test]
    fn numeric_problem_finds_non_finite_and_singular_geometry() {
        let mut doc = Document::default();
        assert_eq!(doc.numeric_problem(), None);
        let obj = |geometry, transform| Object::new("obj-9".into(), "o", "layer-1".into(), geometry, transform);
        let rect = Geometry::Rect { width_mm: 2.0, height_mm: 2.0, corner_radius_mm: 0.0 };
        for (g, t) in [
            (rect.clone(), Affine::scale(1e200) * Affine::scale(1e200)),
            (rect.clone(), Affine::scale_non_uniform(1.0, 0.0)),
            (rect.clone(), Affine::new([1.0, 0.0, 0.0, 1.0, f64::NAN, 0.0])),
            (Geometry::Rect { width_mm: f64::INFINITY, height_mm: 1.0, corner_radius_mm: 0.0 }, Affine::IDENTITY),
            (Geometry::Ellipse { rx_mm: 1e300, ry_mm: 1.0 }, Affine::scale(1e10)),
        ] {
            doc.objects = vec![obj(g, t)];
            assert!(doc.numeric_problem().is_some_and(|p| p.contains("obj-9")), "{:?}", doc.objects[0]);
        }
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((1e308, 0.0));
        doc.objects = vec![obj(Geometry::Path { path }, Affine::scale(10.0))];
        assert!(doc.numeric_problem().is_some());
        doc.objects.clear();
        doc.layers[0].settings.hatch.angle_deg = f64::NAN;
        assert!(doc.numeric_problem().is_some_and(|p| p.contains("hatch.angle_deg")));
    }

    #[test]
    fn extras_keys_are_plain_names() {
        for good in ["lbrn2.minPower", "lbrn2.sub1.speed", "lbrn2.frequency_absent", "x._a-b"] {
            assert!(is_extras_key(good), "{good}");
        }
        for bad in ["lbrn2.", "lbrn2", "lbrn2.evil a=\"1\"/><x", "lbrn2.1x", ".a", "lbrn2.größe", "lbrn2.a\u{1}"] {
            assert!(!is_extras_key(bad), "{bad}");
        }
    }

    #[test]
    fn workspace_is_the_bed_or_the_square_field() {
        let mut d = Document::default();
        assert_eq!(d.workspace(), None);
        d.field_mm = Some(150.0);
        assert_eq!(d.workspace(), Some([150.0, 150.0]));
        assert!(serde_json::to_value(&d).unwrap().get("workspace_mm").is_none(), "galvo documents stay as they were");
        d.workspace_mm = Some([400.0, 300.0]);
        assert_eq!(d.workspace(), Some([400.0, 300.0]));
        let back: Document = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert_eq!(back.workspace_mm, Some([400.0, 300.0]));
        d.workspace_mm = Some([f64::NAN, 300.0]);
        assert!(d.numeric_problem().is_some_and(|p| p.contains("workspace_mm")));
    }

    #[test]
    fn older_json_without_new_fields_still_loads() {
        let mut v = serde_json::to_value(Document::default()).unwrap();
        for key in ["field_mm", "groups", "extras"] {
            v.as_object_mut().unwrap().remove(key);
        }
        assert!(v["layers"][0].get("extras").is_none(), "empty extras are not serialized");
        let back: Document = serde_json::from_value(v).unwrap();
        assert_eq!(back.field_mm, None);
        assert!(back.groups.is_empty() && back.extras.is_empty());
        assert!(back.layers[0].extras.is_empty());
    }

    #[test]
    fn sub_layer_name_and_cleanup_are_optional_in_json() {
        let v = serde_json::to_value(CutSettings::default()).unwrap();
        assert!(v.get("name").is_none() && v.get("cleanup").is_none(), "defaults are left out: {v}");
        let s = CutSettings {
            name: Some("Cleanup".into()),
            cleanup: Some(Cleanup { every_n_passes: 10 }),
            ..CutSettings::default()
        };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["name"], "Cleanup");
        assert_eq!(v["cleanup"]["every_n_passes"], 10);
        assert_eq!(serde_json::from_value::<CutSettings>(v).unwrap(), s);
    }

    #[test]
    fn cleanup_runs_after_every_n_main_passes_and_the_last() {
        let after = |n, main| Cleanup { every_n_passes: n }.runs_after(main).collect::<Vec<_>>();
        assert_eq!(after(1, 3), [1, 2, 3]);
        assert_eq!(after(2, 5), [2, 4, 5]);
        assert_eq!(after(2, 4), [2, 4]);
        assert_eq!(after(10, 3), [3], "a long interval runs once, after the main passes");
        assert_eq!(after(0, 2), [1, 2], "0 counts as 1");
        assert!(after(3, 0).is_empty());
    }

    fn grouped() -> Document {
        let mut d = Document::default();
        let rect = Geometry::Rect { width_mm: 2.0, height_mm: 2.0, corner_radius_mm: 0.0 };
        for (n, x) in [(1, 0.0), (2, 10.0), (3, 20.0)] {
            d.objects.push(Object::new(format!("obj-{n}"), "r", "layer-1".into(), rect.clone(), Affine::translate((x, 0.0))));
        }
        d.groups.push(Group::new("group-8".into(), "outer", None));
        d.groups.push(Group::new("group-9".into(), "inner", Some("group-8".into())));
        d.objects[0].group = Some("group-9".into());
        d.objects[1].group = Some("group-8".into());
        d
    }

    #[test]
    fn groups_nest_and_expand_to_members() {
        let d = grouped();
        assert_eq!(d.group_members("group-8"), ["obj-1", "obj-2"]);
        assert_eq!(d.group_members("group-9"), ["obj-1"]);
        assert_eq!(d.top_group_of("obj-1").map(|g| g.id.as_str()), Some("group-8"));
        assert_eq!(d.top_group_of("obj-3"), None);
        assert!(d.group_is_within("group-9", "group-8") && !d.group_is_within("group-8", "group-9"));
        let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(d.expand_group_ids(&ids(&["obj-2", "group-8", "nope"])), ["obj-2", "obj-1", "nope"]);
        let b = d.bounds_of(&ids(&["group-8"])).unwrap();
        assert!((b.x0 + 1.0).abs() < 1e-9 && (b.x1 - 11.0).abs() < 1e-9);
    }

    #[test]
    fn prune_groups_removes_empty_and_dangling_groups() {
        let mut d = grouped();
        assert!(d.prune_groups().is_empty());
        d.objects.remove(0);
        assert_eq!(d.prune_groups(), ["group-9"], "the inner group lost its last member");
        assert_eq!(d.groups.len(), 1);
        d.objects.remove(0);
        assert_eq!(d.prune_groups(), ["group-8"]);
        let mut d = grouped();
        d.groups.remove(0);
        d.prune_groups();
        assert_eq!(d.groups[0].parent, None, "a missing parent is cleared");
        assert_eq!(d.objects[1].group, None, "a missing group is cleared");
    }

    #[test]
    fn images_have_a_rectangular_outline() {
        let o = Object::new(
            "obj-1".into(),
            "photo",
            "layer-1".into(),
            Geometry::Image {
                width_mm: 30.0,
                height_mm: 20.0,
                mime: "image/png".into(),
                data_base64: "AAAA".into(),
                adjust: ImageAdjust::default(),
            },
            Affine::translate((5.0, 0.0)) * Affine::FLIP_X,
        );
        let b = o.bounds();
        assert!((b.x0 + 10.0).abs() < 1e-9 && (b.x1 - 20.0).abs() < 1e-9 && (b.height() - 20.0).abs() < 1e-9);
        assert_eq!(o.geometry.kind_name(), "image");
        assert_eq!(o.placement_problem(), None);
        let json = serde_json::to_value(&o).unwrap();
        assert_eq!(json["geometry"]["kind"], "image");
        assert_eq!(json["geometry"]["data_base64"], "AAAA");
        assert!(json["geometry"].get("adjust").is_none(), "an adjust that changes nothing is left out");
        assert_eq!(json["group"], serde_json::Value::Null);
        assert!(json.get("extras").is_none());
        let back: Object = serde_json::from_value(json).unwrap();
        assert_eq!(back, o);

        let mut o = o;
        let Geometry::Image { adjust, .. } = &mut o.geometry else { unreachable!() };
        adjust.gamma = 1.5;
        adjust.contrast = 20.0;
        let json = serde_json::to_value(&o).unwrap();
        assert_eq!(json["geometry"]["adjust"]["gamma"], 1.5);
        assert_eq!(serde_json::from_value::<Object>(json).unwrap(), o);
        let Geometry::Image { adjust, .. } = &mut o.geometry else { unreachable!() };
        adjust.brightness = f64::NAN;
        assert!(o.placement_problem().is_some());
    }

    #[test]
    fn image_settings_default_and_load_from_older_json() {
        let mut v = serde_json::to_value(CutSettings::default()).unwrap();
        assert_eq!(v["image"]["dither"], "jarvis");
        assert_eq!(v["image"]["bidirectional"], true);
        v.as_object_mut().unwrap().remove("image");
        let s: CutSettings = serde_json::from_value(v).unwrap();
        assert_eq!(s.image, ImageSettings::default());
        let partial: ImageSettings = serde_json::from_value(serde_json::json!({"dither": "dither", "negative": true})).unwrap();
        assert_eq!((partial.dither, partial.negative, partial.interval_mm), (DitherMode::FloydSteinberg, true, 0.05));
        assert!((ImageSettings { interval_mm: 0.0651, ..ImageSettings::default() }.dpi() - 390.17).abs() < 0.01);
        let mut s = CutSettings::default();
        s.image.dot_width_mm = f64::INFINITY;
        assert_eq!(s.non_finite_field(), Some("image.dot_width_mm"));
    }

    #[test]
    fn dither_modes_have_lightburn_names() {
        for m in DitherMode::ALL {
            assert_eq!(DitherMode::from_lightburn(m.lightburn_name()), Some(m));
            let json = serde_json::to_value(m).unwrap();
            assert_eq!(serde_json::from_value::<DitherMode>(json).unwrap(), m);
        }
        assert_eq!(DitherMode::from_lightburn("Dither"), Some(DitherMode::FloydSteinberg));
        assert_eq!(serde_json::to_value(DitherMode::FloydSteinberg).unwrap(), "floyd_steinberg");
        assert_eq!(DitherMode::from_lightburn("3dslice"), Some(DitherMode::Slice3d));
        assert_eq!(serde_json::from_value::<DitherMode>(serde_json::json!("3dslice")).unwrap(), DitherMode::Slice3d);
    }

    #[test]
    fn locked_objects_round_trip_and_older_files_load() {
        let mut d = grouped();
        assert!(serde_json::to_value(&d).unwrap().get("locked").is_none(), "nothing locked: left out");
        d.locked.insert("obj-2".into());
        d.locked.insert("obj-99".into());
        assert!(d.is_locked("obj-2") && !d.is_locked("obj-1") && !d.is_locked("obj-99"));
        let back: Document = serde_json::from_value(serde_json::to_value(&d).unwrap()).unwrap();
        assert_eq!(back.locked, d.locked);
        d.prune_locked();
        assert_eq!(d.locked.iter().collect::<Vec<_>>(), ["obj-2"]);
    }

    #[test]
    fn adjust_ranges_are_checked() {
        assert_eq!(ImageAdjust::default().problem(), None);
        let a = ImageAdjust { contrast: 120.0, ..ImageAdjust::default() };
        assert!(a.problem().unwrap().contains("adjust.contrast"));
        assert!(ImageAdjust { gamma: 0.0, ..ImageAdjust::default() }.problem().is_some());
        assert!(ImageAdjust { denoise: f64::NAN, ..ImageAdjust::default() }.problem().is_some());
    }
}
