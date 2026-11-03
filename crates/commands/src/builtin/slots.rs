//! Production slots: the pockets of a jig that each get their own serial number or CSV row (OpenEtch's serial
//! ledger, short list #4). A **slot** is one variable-text offset (LightBurn's per-text Offset): every serial-number
//! or CSV text and barcode with that offset, plus its **pocket**, the highest group around it whose variable texts
//! all share that offset (so a pocket's design goes with its number; an ungrouped text is a pocket on its own).
//!
//! What a run marks ([`slot_plan`]):
//! - Slots in [`doc::VariableState::empty_slots`] are left out (their pockets' objects are not planned), and the
//!   slots that are marked take consecutive values: the k-th marked slot shows what the k-th slot would have shown.
//! - With [`doc::VariableState::stop_at_end`], a slot whose value would pass `end` is not marked either; once
//!   `current > end` no slot can be marked and a run is refused ([`SlotPlan::refusal`]).
//! - After a finished run the counter moves on by what was used ([`SlotPlan::advance`]): `advance_by` when every
//!   slot was marked, else the offset span of the marked slots.
//!
//! [`apply_slot_plan`] turns the document into the one the run plans (before `prepare_output` renders the values).
//! Date/time and cut-setting texts are not slots. Command: `var.slots` (Query).

use std::collections::{BTreeMap, BTreeSet};

use doc::{Document, Geometry, ObjectId, VarMode, VarText};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::text::{barcode_params, laser_name, var_of};
use crate::{BARCODE_KEY, Command, Result, Risk};

/// Most slots a document may mark empty (and most slots listed).
pub const MAX_SLOTS: usize = 1000;

/// One value a slot shows: a serial-number or CSV text or barcode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SlotValue {
    pub object: ObjectId,
    /// `serial` or `csv`.
    pub mode: String,
    /// What it shows when this slot is marked (its format evaluated with the slot's value).
    pub shown: String,
}

/// One slot of the document (see the module docs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Slot {
    /// 1-based position in offset order (what people call "slot 3").
    pub number: usize,
    /// The variable-text offset this slot stands for.
    pub offset: i64,
    /// The pocket group's name, else the text's name.
    pub label: String,
    /// Every object of the slot's pockets (the variable ones included).
    pub objects: Vec<ObjectId>,
    /// Marked empty ([`doc::VariableState::empty_slots`]).
    pub empty: bool,
    /// Past `end` with Stop at End on: not marked.
    pub past_end: bool,
    /// The serial number or CSV row this slot gets when it is marked (`current` + the offset it takes).
    pub value: i64,
    pub values: Vec<SlotValue>,
    /// The jig pocket the slot's variable texts sit in ([`doc::jig::object_pockets`]), when they all sit in one:
    /// then every object in that pocket belongs to the slot (a jig placement's clones are not grouped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pocket: Option<doc::jig::PocketPlace>,
}

impl Slot {
    /// Whether the next run marks this slot.
    pub fn marks(&self) -> bool {
        !self.empty && !self.past_end
    }
}

/// What the next run marks (see the module docs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SlotPlan {
    pub slots: Vec<Slot>,
    /// The counter before the run.
    pub current: i64,
    pub start: i64,
    pub end: i64,
    pub stop_at_end: bool,
    pub auto_advance: bool,
    /// How far the counter moves after a finished run that marked these slots.
    pub advance: i64,
    /// Every value up to `end` is used (Stop at End on and `current > end`).
    pub exhausted: bool,
}

impl SlotPlan {
    /// The slots the run marks.
    pub fn marked(&self) -> impl Iterator<Item = &Slot> {
        self.slots.iter().filter(|s| s.marks())
    }

    /// Whether some slot is left out (empty or past End).
    pub fn skips(&self) -> bool {
        self.slots.iter().any(|s| !s.marks())
    }

    /// Why a run of this plan must be refused, if it must: with Stop at End every value is used, or no slot is
    /// marked because of it.
    pub fn refusal(&self) -> Option<String> {
        if self.slots.is_empty() || !self.stop_at_end {
            return None;
        }
        if self.exhausted {
            return Some(format!(
                "every serial number / CSV row up to End ({}) is used (Stop at End): reset the counter or raise End",
                self.end
            ));
        }
        if self.marked().next().is_none() && self.slots.iter().any(|s| s.past_end) {
            return Some(format!("no slot has a value up to End ({}) left (Stop at End)", self.end));
        }
        None
    }

    /// The counter after a finished run: `current + advance`, held one past `end` with Stop at End.
    pub fn next_current(&self) -> i64 {
        let next = self.current.saturating_add(self.advance);
        if self.stop_at_end { next.min(self.end.saturating_add(1)) } else { next }
    }
}

fn mode_name(m: VarMode) -> Option<&'static str> {
    match m {
        VarMode::Serial => Some("serial"),
        VarMode::Csv => Some("csv"),
        VarMode::DateTime | VarMode::CutSetting => None,
    }
}

/// One counted variable object: `(object id, its variable settings, its format)`.
type Counted = (ObjectId, VarText, String);

/// The variable objects of `doc` that count (serial-number and CSV texts and barcodes).
fn counted(doc: &Document) -> Vec<Counted> {
    let mut out = Vec::new();
    for o in &doc.objects {
        match &o.geometry {
            Geometry::Text(t) => {
                if let Some(v) = t.variable.as_ref().filter(|v| mode_name(v.mode).is_some()) {
                    out.push((o.id.clone(), v.clone(), t.text.clone()));
                }
            }
            _ => {
                if let Some(b) = barcode_params(o)
                    && let Some(v) = b.variable.as_ref().and_then(var_of).filter(|v| mode_name(v.mode).is_some())
                {
                    out.push((o.id.clone(), v, b.data.clone()));
                }
            }
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

/// The slots of `doc` and what the next run marks.
pub fn slot_plan(doc: &Document) -> SlotPlan {
    let v = &doc.variables;
    let vars = counted(doc);
    // Each group's variable offsets, to find the pockets (groups whose variable texts all share one offset).
    let mut group_offsets: BTreeMap<String, BTreeSet<i64>> = BTreeMap::new();
    for (id, var, _) in &vars {
        for g in chain(doc, id) {
            group_offsets.entry(g).or_default().insert(var.offset);
        }
    }
    let mut by_offset: BTreeMap<i64, (Vec<Counted>, BTreeSet<String>)> = BTreeMap::new();
    for (id, var, format) in vars {
        // The highest enclosing group whose variable texts all have this offset.
        let pocket = chain(doc, &id)
            .into_iter()
            .take_while(|g| group_offsets.get(g).is_some_and(|s| s.len() == 1))
            .last();
        let e = by_offset.entry(var.offset).or_default();
        if let Some(p) = pocket {
            e.1.insert(p);
        }
        e.0.push((id, var, format));
    }
    let offsets: Vec<i64> = by_offset.keys().copied().take(MAX_SLOTS).collect();
    let empty: BTreeSet<i64> = v.empty_slots.iter().copied().collect();
    let laser = laser_name(doc);
    // Jig pockets (module docs of `doc::jig`): a slot whose variable texts all sit in one pocket, alone among the
    // slots, takes that pocket's objects too.
    let places = doc::jig::object_pockets(doc);
    let slot_pocket = |members: &[Counted]| -> Option<doc::jig::PocketPlace> {
        let mut it = members.iter().map(|(id, _, _)| places.get(id));
        let first = it.next()??;
        it.all(|p| p == Some(first)).then(|| first.clone())
    };
    let pocket_of: Vec<Option<doc::jig::PocketPlace>> = offsets.iter().map(|o| slot_pocket(&by_offset[o].0)).collect();
    let mut slots = Vec::new();
    let mut position = 0usize;
    for (n, offset) in offsets.iter().enumerate() {
        let (members, pockets) = &by_offset[offset];
        let is_empty = empty.contains(offset);
        // Marked slots take the offsets of the first slots in order (compaction past empty slots).
        let eff = if is_empty { *offset } else { offsets[position] };
        if !is_empty {
            position += 1;
        }
        let value = v.current.saturating_add(eff);
        let past_end = !is_empty && v.stop_at_end && value > v.end;
        let jig_pocket = pocket_of[n].clone().filter(|p| pocket_of.iter().filter(|q| q.as_ref() == Some(p)).count() == 1);
        let mut objects: Vec<ObjectId> = Vec::new();
        for o in &doc.objects {
            let in_pocket = o.group.is_some() && chain(doc, &o.id).iter().any(|g| pockets.contains(g));
            let in_jig_pocket = jig_pocket.as_ref().is_some_and(|p| places.get(&o.id) == Some(p));
            if in_pocket || in_jig_pocket || members.iter().any(|(id, _, _)| *id == o.id) {
                objects.push(o.id.clone());
            }
        }
        let label = pockets
            .iter()
            .filter_map(|g| doc.group(g).map(|gr| gr.name.clone()))
            .find(|n| !n.trim().is_empty())
            .or_else(|| pocket_of[n].as_ref().map(|p| format!("{} P{}", p.jig_name, p.number)))
            .or_else(|| members.iter().filter_map(|(id, _, _)| doc.object(id).map(|o| o.name.clone())).find(|n| !n.trim().is_empty()))
            .unwrap_or_else(|| format!("Slot {}", n + 1));
        let values = members
            .iter()
            .map(|(id, var, format)| {
                let layer = doc.object(id).map(|o| o.layer.clone()).unwrap_or_default();
                let shown = text::vars::evaluate(doc, &layer, format, &VarText { mode: var.mode, offset: eff }, None, &laser);
                SlotValue { object: id.clone(), mode: mode_name(var.mode).unwrap_or("serial").into(), shown }
            })
            .collect();
        slots.push(Slot { number: n + 1, offset: *offset, label, objects, empty: is_empty, past_end, value, values, pocket: pocket_of[n].clone() });
    }
    let marked = slots.iter().filter(|s| s.marks()).count();
    let advance = if marked == slots.len() {
        v.advance_by
    } else if marked == 0 {
        0
    } else {
        // Offsets are the texts' own (any i64): never an overflow.
        offsets[marked].saturating_sub(offsets[0])
    };
    SlotPlan {
        exhausted: v.stop_at_end && !slots.is_empty() && v.current > v.end,
        slots,
        current: v.current,
        start: v.start,
        end: v.end,
        stop_at_end: v.stop_at_end,
        auto_advance: v.auto_advance,
        advance,
    }
}

/// The document a run of `plan` plans: the objects of slots it does not mark left out, and the marked slots' texts
/// and barcodes at the offsets that give them their values (rendered by `prepare_output` afterwards). Unchanged when
/// every slot is marked.
pub fn apply_slot_plan(doc: &Document, plan: &SlotPlan) -> Document {
    let mut out = doc.clone();
    if !plan.skips() {
        return out;
    }
    let mut drop: BTreeSet<&str> = BTreeSet::new();
    let mut offsets: BTreeMap<&str, i64> = BTreeMap::new();
    for s in &plan.slots {
        if s.marks() {
            for v in &s.values {
                offsets.insert(v.object.as_str(), s.value.saturating_sub(plan.current));
            }
        } else {
            drop.extend(s.objects.iter().map(String::as_str));
        }
    }
    out.objects.retain(|o| !drop.contains(o.id.as_str()));
    for o in &mut out.objects {
        let Some(&eff) = offsets.get(o.id.as_str()) else { continue };
        if let Geometry::Text(t) = &mut o.geometry {
            if let Some(v) = &mut t.variable {
                v.offset = eff;
            }
        } else if let Some(mut b) = barcode_params(o) {
            if let Some(v) = &mut b.variable {
                v.offset = eff;
            }
            if let Ok(p) = serde_json::to_string(&b) {
                o.extras.insert(BARCODE_KEY.into(), p);
            }
        }
    }
    out.prune_groups();
    out
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SlotsInput {}

/// `var.slots`.
pub struct Slots;

impl Command for Slots {
    type Input = SlotsInput;
    const ID: &'static str = "var.slots";
    const TITLE: &'static str = "Production slots";
    const DESCRIPTION: &'static str = "The production slots: one per variable-text offset (serial-number and CSV texts \
        and barcodes), each with its pocket (the group around it), what it shows next, and whether the next run marks it \
        (`empty`: marked empty with var.set_serial empty_slots; `past_end`: past End with stop_at_end). Returns \
        `{slots: [{number, offset, label, objects, empty, past_end, value, values: [{object, mode, shown}]}], current, \
        start, end, stop_at_end, auto_advance, advance (how far a finished run moves the counter), exhausted, refusal}`.";
    const RISK: Risk = Risk::Query;

    fn apply(doc: &mut Document, _: SlotsInput) -> Result<(Value, String)> {
        let plan = slot_plan(doc);
        let mut v = serde_json::to_value(&plan).unwrap_or(Value::Null);
        v["refusal"] = json!(plan.refusal());
        let n = plan.slots.len();
        Ok((v, format!("{n} slot(s)")))
    }
}

#[cfg(test)]
#[path = "slots_tests.rs"]
mod tests;
