//! The serial ledger (short list #4): one JSON line per run that marked serial-number or CSV slots, appended to
//! `<config-dir>/serial-ledger.jsonl` next to the job log (never rewritten): which value went on which slot (pocket)
//! in which run, which slots were left out (marked empty, or past End with Stop at End), how the run ended, and the
//! re-mark it was, if any. `GET /api/production/ledger` reads it back as JSON or CSV (one row per slot), MCP's
//! `serial_ledger` as JSON. A run that streamed (finished, stopped or failed) is recorded; a cancelled one marked
//! nothing and is not. The values are those the run was planned with (`commands::slot_plan` at the request).

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

use super::{RunRecord, now_unix};
use crate::runs::RunState;
use crate::EngineError;

pub const LEDGER_FILE: &str = "serial-ledger.jsonl";
/// Most entries one read returns.
pub const MAX_READ: usize = 10_000;
/// Largest ledger that is read back (older lines beyond it are skipped).
const MAX_READ_BYTES: u64 = 32 * 1024 * 1024;

/// One slot of a ledger entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LedgerSlot {
    /// 1-based slot number in offset order.
    pub number: usize,
    pub offset: i64,
    pub label: String,
    /// The serial number or CSV row this slot was given.
    pub value: i64,
    /// What its texts and barcodes showed: `[object, mode, shown]`.
    pub values: Vec<commands::SlotValue>,
    /// Marked in this run.
    pub marked: bool,
    /// Why it was left out: `empty` or `past_end`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
}

/// One run in the ledger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LedgerEntry {
    /// When the run ended, Unix seconds.
    pub time_unix_s: u64,
    pub run_id: String,
    /// `finished`, `aborted` (stopped: the slots may be partly marked) or `failed`.
    pub state: String,
    pub device: String,
    pub project: Option<String>,
    pub document: String,
    /// Iterations that ran to the end (repeats of one run share its values).
    pub completed: u32,
    /// The counter the run was planned with.
    pub counter: i64,
    pub slots: Vec<LedgerSlot>,
    /// A re-mark of slot `slot` of run `run_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remark_of: Option<RemarkOf>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemarkOf {
    pub run_id: String,
    pub slot: usize,
}

/// The ledger entry of a run that ended.
pub(crate) fn entry(run_id: &str, device: &str, state: RunState, completed: u32, r: &RunRecord) -> LedgerEntry {
    let state = serde_json::to_value(state).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
    LedgerEntry {
        time_unix_s: now_unix(),
        run_id: run_id.to_owned(),
        state,
        device: device.to_owned(),
        project: r.project.clone(),
        document: r.document.clone(),
        completed,
        counter: r.plan.current,
        slots: r
            .plan
            .slots
            .iter()
            .map(|s| LedgerSlot {
                number: s.number,
                offset: s.offset,
                label: s.label.clone(),
                value: s.value,
                values: s.values.clone(),
                marked: s.marks(),
                skipped: if s.empty {
                    Some("empty".into())
                } else if s.past_end {
                    Some("past_end".into())
                } else {
                    None
                },
            })
            .collect(),
        remark_of: r.remark_of.as_ref().map(|(run_id, slot)| RemarkOf { run_id: run_id.clone(), slot: *slot }),
    }
}

/// The ledger file (none: entries are dropped, e.g. in tests without a config directory, which keep them in memory).
#[derive(Debug, Default)]
pub struct Ledger {
    path: RwLock<Option<PathBuf>>,
    /// Without a file: the entries (tests).
    memory: RwLock<Vec<LedgerEntry>>,
}

impl Ledger {
    pub fn open(&self, dir: &Path) {
        *self.path.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(dir.join(LEDGER_FILE));
    }

    fn path(&self) -> Option<PathBuf> {
        self.path.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    /// Append one entry (one line; a failed write is logged, never fatal).
    pub fn append(&self, e: &LedgerEntry) {
        let Some(path) = self.path() else {
            self.memory.write().unwrap_or_else(std::sync::PoisonError::into_inner).push(e.clone());
            return;
        };
        let line = match serde_json::to_string(e) {
            Ok(l) => l,
            Err(err) => return tracing::warn!(error = %err, "serial ledger: cannot encode the entry"),
        };
        let written = std::fs::OpenOptions::new().create(true).append(true).open(&path).and_then(|mut f| writeln!(f, "{line}"));
        if let Err(err) = written {
            tracing::warn!(path = %path.display(), error = %err, "serial ledger: cannot append");
        }
    }

    /// The newest `limit` entries, oldest first (lines that do not parse are skipped).
    pub fn read(&self, limit: usize) -> Result<Vec<LedgerEntry>, EngineError> {
        let n = limit.min(MAX_READ);
        let Some(path) = self.path() else {
            let m = self.memory.read().unwrap_or_else(std::sync::PoisonError::into_inner);
            return Ok(m[m.len().saturating_sub(n)..].to_vec());
        };
        let text = match std::fs::metadata(&path) {
            Ok(m) if m.len() > MAX_READ_BYTES => {
                use std::io::{Read, Seek, SeekFrom};
                let mut f = std::fs::File::open(&path).map_err(|e| EngineError::Internal(e.to_string()))?;
                #[allow(clippy::cast_possible_wrap)]
                f.seek(SeekFrom::End(-(MAX_READ_BYTES as i64))).map_err(|e| EngineError::Internal(e.to_string()))?;
                let mut buf = Vec::new();
                f.read_to_end(&mut buf).map_err(|e| EngineError::Internal(e.to_string()))?;
                String::from_utf8_lossy(&buf).into_owned()
            }
            Ok(_) => std::fs::read_to_string(&path).map_err(|e| EngineError::Internal(e.to_string()))?,
            Err(_) => return Ok(Vec::new()),
        };
        let all: Vec<LedgerEntry> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        Ok(all[all.len().saturating_sub(n)..].to_vec())
    }

    /// The entry of run `run_id` (the newest, should there be several: run ids start again when the engine
    /// restarts), or the one that ended at `time_unix_s` when it is given.
    pub fn find(&self, run_id: &str, time_unix_s: Option<u64>) -> Result<Option<LedgerEntry>, EngineError> {
        Ok(self.read(MAX_READ)?.into_iter().rev().find(|e| e.run_id == run_id && time_unix_s.is_none_or(|t| t == e.time_unix_s)))
    }
}

fn csv_field(s: &str) -> String {
    // Free text that starts like a formula would run as one in a spreadsheet: prefixed with an apostrophe.
    let s = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) { format!("'{s}") } else { s.to_string() };
    if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s }
}

/// The entries as CSV, one row per slot (Ruby-style job ledger export).
pub fn to_csv(entries: &[LedgerEntry]) -> String {
    let mut out = String::from("time_unix_s,run_id,state,device,project,document,completed,slot,label,value,shown,marked,skipped,remark_of_run,remark_of_slot\n");
    for e in entries {
        for s in &e.slots {
            let shown: Vec<&str> = s.values.iter().map(|v| v.shown.as_str()).collect();
            let row = [
                e.time_unix_s.to_string(),
                e.run_id.clone(),
                e.state.clone(),
                e.device.clone(),
                e.project.clone().unwrap_or_default(),
                e.document.clone(),
                e.completed.to_string(),
                s.number.to_string(),
                s.label.clone(),
                s.value.to_string(),
                shown.join(" | "),
                s.marked.to_string(),
                s.skipped.clone().unwrap_or_default(),
                e.remark_of.as_ref().map(|r| r.run_id.clone()).unwrap_or_default(),
                e.remark_of.as_ref().map(|r| r.slot.to_string()).unwrap_or_default(),
            ];
            out.push_str(&row.iter().map(|f| csv_field(f)).collect::<Vec<_>>().join(","));
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(n: u64) -> LedgerEntry {
        LedgerEntry {
            time_unix_s: n,
            run_id: format!("run-{n}"),
            state: "finished".into(),
            device: "b6".into(),
            project: Some("=coins".into()),
            document: "Discs".into(),
            completed: 1,
            counter: 7,
            slots: vec![
                LedgerSlot {
                    number: 1,
                    offset: 0,
                    label: "Pocket, 1".into(),
                    value: 7,
                    values: vec![commands::SlotValue { object: "obj-2".into(), mode: "serial".into(), shown: "SN 007".into() }],
                    marked: true,
                    skipped: None,
                },
                LedgerSlot {
                    number: 2,
                    offset: 1,
                    label: "Pocket 2".into(),
                    value: 1,
                    values: vec![],
                    marked: false,
                    skipped: Some("empty".into()),
                },
            ],
            remark_of: None,
        }
    }

    #[test]
    fn appends_reads_and_exports() {
        let dir = std::env::temp_dir().join(format!("openetch-ledger-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let l = Ledger::default();
        l.open(&dir);
        for n in 1..=4 {
            l.append(&entry(n));
        }
        let last = l.read(2).unwrap();
        assert_eq!(last.iter().map(|e| e.time_unix_s).collect::<Vec<_>>(), [3, 4]);
        assert_eq!(l.find("run-2", None).unwrap().unwrap().time_unix_s, 2);
        assert!(l.find("run-2", Some(3)).unwrap().is_none());
        let csv = to_csv(&last);
        assert_eq!(csv.lines().count(), 5, "a header and one row per slot");
        assert!(csv.contains(",'=coins,"), "{csv}");
        assert!(csv.contains("\"Pocket, 1\",7,SN 007,true,,"), "{csv}");
        assert!(csv.contains(",Pocket 2,1,,false,empty,"), "{csv}");
        // Without a file the entries stay in memory.
        let m = Ledger::default();
        m.append(&entry(9));
        assert_eq!(m.read(5).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
