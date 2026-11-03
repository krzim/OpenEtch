//! The structure of the production slots (OpenEtch's serial ledger; `commands::slot_plan` builds the full plan on
//! it): which variable-text objects count, which objects belong to each slot's pocket, and which slots the next run
//! marks. Here rather than in `commands` so that the browser preview (crates/webcore, which has no `commands`) leaves
//! out the same pockets as the engine's preview and its runs.
//!
//! A **slot** is one variable-text offset: every serial-number or CSV text and barcode with that offset, plus its
//! pocket, the highest group around it whose variable texts all share that offset (an ungrouped text is a pocket on
//! its own), and, when the slot's texts all sit in one jig pocket alone among the slots, that jig pocket's objects.
//! Slots in [`crate::VariableState::empty_slots`] are not marked, and the marked ones take consecutive values (the
//! k-th marked slot shows what the k-th slot would have shown); with Stop at End a slot whose value would pass `end`
//! is not marked either.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use crate::jig::PocketPlace;
use crate::{Document, Geometry, ObjectId, VarMode, VarText};

/// Most slots a document may mark empty (and most slots listed).
pub const MAX_SLOTS: usize = 1000;

/// Object extra holding a barcode's settings (JSON; `commands::BARCODE_KEY`).
pub const BARCODE_KEY: &str = "openetch.barcode";

/// One counted variable object: its id, its variable settings and its format (a text's text, a barcode's data).
#[derive(Debug, Clone, PartialEq)]
pub struct Counted {
    pub object: ObjectId,
    pub var: VarText,
    pub format: String,
}

/// One slot's structure (see the module docs).
#[derive(Debug, Clone, PartialEq)]
pub struct SlotLayout {
    pub offset: i64,
    /// The counted objects with this offset.
    pub members: Vec<Counted>,
    /// The groups that are this slot's pockets.
    pub pockets: BTreeSet<String>,
    /// The jig pocket the members all sit in, when they do.
    pub jig_pocket: Option<PocketPlace>,
    /// Every object of the slot (its pockets', the members, the jig pocket's when it is the slot's alone).
    pub objects: Vec<ObjectId>,
    /// Marked empty.
    pub empty: bool,
    /// Past `end` with Stop at End on.
    pub past_end: bool,
    /// The offset the slot's values take when it is marked (compaction past empty slots).
    pub effective_offset: i64,
}

impl SlotLayout {
    /// Whether the next run marks this slot.
    pub fn marks(&self) -> bool {
        !self.empty && !self.past_end
    }
}

/// Serial-number and CSV modes count; date/time and cut-setting texts are not slots.
pub fn counts(mode: VarMode) -> bool {
    matches!(mode, VarMode::Serial | VarMode::Csv)
}

/// A barcode's variable settings and data, from its [`BARCODE_KEY`] extra.
fn barcode_var(o: &crate::Object) -> Option<(VarText, String)> {
    let v: serde_json::Value = serde_json::from_str(o.extras.get(BARCODE_KEY)?).ok()?;
    let var = v.get("variable")?;
    let mode = match var.get("mode")?.as_str()? {
        "serial" => VarMode::Serial,
        "csv" => VarMode::Csv,
        "date_time" => VarMode::DateTime,
        "cut_setting" => VarMode::CutSetting,
        _ => return None,
    };
    let offset = match var.get("offset") {
        None | Some(serde_json::Value::Null) => 0,
        Some(n) => n.as_i64()?,
    };
    Some((VarText { mode, offset }, v.get("data")?.as_str()?.to_owned()))
}

/// The variable objects of `doc` that count (serial-number and CSV texts and barcodes), in document order.
pub fn counted(doc: &Document) -> Vec<Counted> {
    let mut out = Vec::new();
    for o in &doc.objects {
        let found = match &o.geometry {
            Geometry::Text(t) => t.variable.clone().map(|v| (v, t.text.clone())),
            _ => barcode_var(o),
        };
        if let Some((var, format)) = found.filter(|(v, _)| counts(v.mode)) {
            out.push(Counted { object: o.id.clone(), var, format });
        }
    }
    out
}

/// The group chain of `obj` from its own group outward.
fn chain(doc: &Document, obj: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut g = doc.object(obj).and_then(|o| o.group.clone());
    while let Some(id) = g {
        if out.contains(&id) || out.len() > 64 {
            break;
        }
        g = doc.group(&id).and_then(|gr| gr.parent.clone());
        out.push(id);
    }
    out
}

/// The slots of `doc` in offset order (at most [`MAX_SLOTS`]), with what the next run marks.
pub fn slot_layouts(doc: &Document) -> Vec<SlotLayout> {
    let v = &doc.variables;
    let vars = counted(doc);
    if vars.is_empty() {
        return Vec::new();
    }
    // Each group's variable offsets, to find the pockets (groups whose variable texts all share one offset).
    let mut group_offsets: BTreeMap<String, BTreeSet<i64>> = BTreeMap::new();
    for c in &vars {
        for g in chain(doc, &c.object) {
            group_offsets.entry(g).or_default().insert(c.var.offset);
        }
    }
    let mut by_offset: BTreeMap<i64, (Vec<Counted>, BTreeSet<String>)> = BTreeMap::new();
    for c in vars {
        // The highest enclosing group whose variable texts all have this offset.
        let pocket = chain(doc, &c.object)
            .into_iter()
            .take_while(|g| group_offsets.get(g).is_some_and(|s| s.len() == 1))
            .last();
        let e = by_offset.entry(c.var.offset).or_default();
        if let Some(p) = pocket {
            e.1.insert(p);
        }
        e.0.push(c);
    }
    let offsets: Vec<i64> = by_offset.keys().copied().take(MAX_SLOTS).collect();
    let empty: BTreeSet<i64> = v.empty_slots.iter().copied().collect();
    let places = crate::jig::object_pockets(doc);
    let slot_pocket = |members: &[Counted]| -> Option<PocketPlace> {
        let mut it = members.iter().map(|c| places.get(&c.object));
        let first = it.next()??;
        it.all(|p| p == Some(first)).then(|| first.clone())
    };
    let pocket_of: Vec<Option<PocketPlace>> = offsets.iter().map(|o| slot_pocket(&by_offset[o].0)).collect();
    let mut out = Vec::new();
    let mut position = 0usize;
    for (n, offset) in offsets.iter().enumerate() {
        let (members, pockets) = by_offset.remove(offset).unwrap_or_default();
        let is_empty = empty.contains(offset);
        let eff = if is_empty { *offset } else { offsets[position] };
        if !is_empty {
            position += 1;
        }
        let past_end = !is_empty && v.stop_at_end && v.current.saturating_add(eff) > v.end;
        let alone = pocket_of[n].clone().filter(|p| pocket_of.iter().filter(|q| q.as_ref() == Some(p)).count() == 1);
        let mut objects: Vec<ObjectId> = Vec::new();
        for o in &doc.objects {
            let in_pocket = o.group.is_some() && chain(doc, &o.id).iter().any(|g| pockets.contains(g));
            let in_jig_pocket = alone.as_ref().is_some_and(|p| places.get(&o.id) == Some(p));
            if in_pocket || in_jig_pocket || members.iter().any(|c| c.object == o.id) {
                objects.push(o.id.clone());
            }
        }
        out.push(SlotLayout {
            offset: *offset,
            members,
            pockets,
            jig_pocket: pocket_of[n].clone(),
            objects,
            empty: is_empty,
            past_end,
            effective_offset: eff,
        });
    }
    out
}

/// The objects of the slots the next run does not mark (empty, or past End).
pub fn skipped_objects(doc: &Document) -> BTreeSet<ObjectId> {
    slot_layouts(doc).into_iter().filter(|s| !s.marks()).flat_map(|s| s.objects).collect()
}

/// `doc` as its previews and runs plan its geometry: the objects of slots the next run does not mark left out (then
/// emptied groups pruned). Borrowed unchanged when every slot is marked.
pub fn without_skipped_slots(doc: &Document) -> Cow<'_, Document> {
    if doc.variables.empty_slots.is_empty() && !doc.variables.stop_at_end {
        return Cow::Borrowed(doc);
    }
    let skip = skipped_objects(doc);
    if skip.is_empty() {
        return Cow::Borrowed(doc);
    }
    let mut out = doc.clone();
    out.objects.retain(|o| !skip.contains(&o.id));
    out.prune_groups();
    Cow::Owned(out)
}
