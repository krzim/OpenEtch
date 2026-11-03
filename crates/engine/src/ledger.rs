//! The job ledger: what every run burned, kept so it can be found, judged and burned again (short list #5).
//!
//! **What is kept.** When a run is requested ([`remember`], from `AppState::request_run_at`) the engine notes, next to
//! the pending run, the document it was planned from (shared, not copied), every used layer's settings with its share
//! of the estimate (`plan::layer_times`), the variable text values (serial number / CSV row, each text's and
//! barcode's value) and the run options. When the run ends ([`record`], from `libs::after_run`) that note becomes
//! part of its job log line (`joblog::JobLogEntry`: `layers`, `variables`, `options`, `estimated_time_s`,
//! `mark_time_s`), and in the background a small thumbnail of the document (module `ledger_thumb`) and the document
//! itself (gzipped `.openetch`) are written to `<config-dir>/job-ledger/<id>.png` and `<id>.openetch.gz`. A run
//! that was never approved leaves nothing (its note is dropped after [`MAX_CAPTURES`] newer ones).
//!
//! **Storage is capped** ([`LedgerSettings`], `<config-dir>/job-ledger/settings.json`, `PUT /api/job-ledger/settings`
//! from the UI only): documents are kept for the newest `keep_documents` jobs (default 100) and only up to
//! `max_document_mb` each, thumbnails for the newest `keep_thumbnails` (default 2000) at `thumbnail_px` (default
//! 160). Older files are deleted after every run and whenever the settings change; the log lines (with the layer
//! settings and values) stay, as the job log always did.
//!
//! **Verdicts and notes.** A run can be marked OK or scrap with a note (`notes.jsonl`, append-only, the last line for
//! an id wins), by a person in the Job log window or by an agent over MCP (`job_ledger_mark`; recorded with its
//! origin).
//!
//! **Search.** [`search`] filters by free text (every word must appear in the project, document, device, summary,
//! layer names and settings, variable values, note or error), verdict, state, device, project and time.
//!
//! **Re-run exactly as burned** never starts anything: [`open_document`] opens the stored document as a new open
//! document (module `documents`) and [`rerun`] then requests a *pending* run of it with the stored run options,
//! through the same `AppState::request_run_with` as Request run; a person still approves it in the app (operator PIN
//! or Start), with every gate of `approve`. The stored document keeps the counter it was burned with, so the serial
//! number or CSV row comes out the same. MCP offers the open (`job_ledger_open`) and the reads; requesting the run is
//! `job_request_run`, approving it is never an agent's.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use commands::{Invocation, Origin};
use doc::{Document, Geometry, VarMode, VarText};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::joblog::{JobLogEntry, LayerSnapshot, VariableValue, VariablesUsed};
use crate::libs::lock;
use crate::runs::{RunOptions, RunView};
use crate::{AppState, EngineError, Event};

pub const LEDGER_DIR: &str = "job-ledger";
const NOTES_FILE: &str = "notes.jsonl";
const SETTINGS_FILE: &str = "settings.json";
/// Notes on runs that were requested but have not ended (pending runs that may never be approved), newest kept.
pub const MAX_CAPTURES: usize = 32;
/// Longest note, characters.
pub const MAX_NOTE_CHARS: usize = 2000;
/// Most entries one search returns.
pub const MAX_RESULTS: usize = 2000;
/// Images larger than this (encoded, base64) are left out of a thumbnail (drawn as their outline).
pub(crate) const MAX_THUMB_IMAGE_B64: usize = 48 * 1024 * 1024;

/// How much the ledger keeps (see the module docs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct LedgerSettings {
    /// Keep the burned document of the newest this many jobs, for Re-run (0: none). 0..=2000.
    pub keep_documents: u32,
    /// Do not keep a document larger than this, MB (its `.openetch` JSON before compression). 1..=200.
    pub max_document_mb: u32,
    /// Keep thumbnails of the newest this many jobs (0: none). 0..=50000.
    pub keep_thumbnails: u32,
    /// Thumbnail size: its longer side, px. 64..=320.
    pub thumbnail_px: u32,
}

impl Default for LedgerSettings {
    fn default() -> Self {
        LedgerSettings { keep_documents: 100, max_document_mb: 25, keep_thumbnails: 2000, thumbnail_px: 160 }
    }
}

impl LedgerSettings {
    pub fn check(&self) -> Result<(), EngineError> {
        let bad = |what: &str, v: u32, lo: u32, hi: u32| {
            (!(lo..=hi).contains(&v)).then(|| EngineError::BadRequest(format!("{what} must be within {lo}..={hi} (got {v})")))
        };
        if let Some(e) = bad("keep_documents", self.keep_documents, 0, 2000)
            .or_else(|| bad("max_document_mb", self.max_document_mb, 1, 200))
            .or_else(|| bad("keep_thumbnails", self.keep_thumbnails, 0, 50_000))
            .or_else(|| bad("thumbnail_px", self.thumbnail_px, 64, 320))
        {
            return Err(e);
        }
        Ok(())
    }
}

/// A run's verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// The part came out right.
    Ok,
    /// The part is scrap.
    Scrap,
}

/// One line of `notes.jsonl`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub time_unix_s: u64,
    #[serde(default)]
    pub verdict: Option<Verdict>,
    #[serde(default)]
    pub note: String,
    /// Who wrote it: `ui`, `script` or `agent:<name>`.
    #[serde(default)]
    pub origin: String,
}

/// What [`remember`] notes for a requested run.
#[derive(Debug)]
pub struct Capture {
    run_id: String,
    /// The document the run was planned from; `None` for a queue of projects (each planned from its own file).
    doc: Option<Arc<Document>>,
    layers: Vec<LayerSnapshot>,
    variables: Option<VariablesUsed>,
    jigs: Vec<crate::joblog::JigUsed>,
    estimated_time_s: f64,
    options: Option<Value>,
}

/// The ledger's files and settings (one field of `joblog::JobLog`; no directory: nothing is written).
#[derive(Debug, Default)]
pub struct Ledger {
    dir: Option<PathBuf>,
    settings: Mutex<LedgerSettings>,
    captures: Mutex<VecDeque<Arc<Capture>>>,
    /// Held while files are written, read back or pruned.
    files: Mutex<()>,
}

impl Ledger {
    /// The ledger under `<config_dir>/job-ledger` (created when the first file is written).
    pub fn open(config_dir: &Path) -> Ledger {
        let dir = config_dir.join(LEDGER_DIR);
        let settings = std::fs::read_to_string(dir.join(SETTINGS_FILE))
            .ok()
            .and_then(|t| serde_json::from_str::<LedgerSettings>(&t).ok())
            .filter(|s| s.check().is_ok())
            .unwrap_or_default();
        Ledger { dir: Some(dir), settings: Mutex::new(settings), ..Ledger::default() }
    }

    pub fn settings(&self) -> LedgerSettings {
        lock(&self.settings).clone()
    }

    /// Change the settings (checked), save them and delete what they no longer keep.
    pub fn set_settings(&self, s: LedgerSettings) -> Result<(), EngineError> {
        s.check()?;
        if let Some(dir) = &self.dir {
            let text = serde_json::to_string_pretty(&s).map_err(|e| EngineError::Internal(e.to_string()))?;
            crate::prefs::write_atomic(&dir.join(SETTINGS_FILE), text.as_bytes())?;
        }
        *lock(&self.settings) = s;
        self.prune();
        Ok(())
    }

    fn remember(&self, c: Capture) {
        let mut q = lock(&self.captures);
        q.retain(|o| o.run_id != c.run_id);
        q.push_back(Arc::new(c));
        while q.len() > MAX_CAPTURES {
            q.pop_front();
        }
    }

    fn take(&self, run_id: &str) -> Option<Arc<Capture>> {
        let mut q = lock(&self.captures);
        let i = q.iter().position(|c| c.run_id == run_id)?;
        q.remove(i)
    }

    fn path(&self, id: &str, ext: &str) -> Option<PathBuf> {
        Some(self.dir.as_ref()?.join(format!("{id}{ext}")))
    }

    /// The thumbnail PNG of entry `id`, if kept.
    pub fn thumbnail(&self, id: &str) -> Option<Vec<u8>> {
        check_id(id).ok()?;
        std::fs::read(self.path(id, ".png")?).ok()
    }

    /// The stored `.openetch` JSON of entry `id`, if kept.
    pub fn document_json(&self, id: &str) -> Result<String, EngineError> {
        check_id(id)?;
        let missing = || EngineError::NotFound(format!("the job ledger keeps no document of '{id}' (see its storage settings)"));
        let path = self.path(id, ".openetch.gz").ok_or_else(missing)?;
        let _f = lock(&self.files);
        let file = std::fs::File::open(&path).map_err(|_| missing())?;
        let mut text = String::new();
        flate2::read::GzDecoder::new(file)
            .read_to_string(&mut text)
            .map_err(|e| EngineError::Internal(format!("the stored document of '{id}' cannot be read: {e}")))?;
        Ok(text)
    }

    /// Names of the files in the ledger directory.
    fn file_names(&self) -> HashSet<String> {
        let Some(dir) = &self.dir else { return HashSet::new() };
        std::fs::read_dir(dir)
            .map(|rd| rd.filter_map(|e| e.ok()?.file_name().into_string().ok()).collect())
            .unwrap_or_default()
    }

    /// Every note, the newest per id.
    pub fn notes(&self) -> HashMap<String, Note> {
        let Some(path) = self.path(NOTES_FILE, "") else { return HashMap::new() };
        // A damaged byte loses its own line, never every verdict.
        let text = String::from_utf8_lossy(&std::fs::read(path).unwrap_or_default()).into_owned();
        let mut out = HashMap::new();
        for n in text.lines().filter_map(|l| serde_json::from_str::<Note>(l).ok()) {
            out.insert(n.id.clone(), n);
        }
        out
    }

    fn append_note(&self, n: &Note) -> Result<(), EngineError> {
        let Some(path) = self.path(NOTES_FILE, "") else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| EngineError::Internal(format!("cannot create {}: {e}", dir.display())))?;
        }
        let line = serde_json::to_string(n).map_err(|e| EngineError::Internal(e.to_string()))?;
        crate::production::ledger::append_line(&path, &line)
            .map_err(|e| EngineError::Internal(format!("cannot write {}: {e}", path.display())))
    }

    /// Write entry `id`'s thumbnail and document (as the settings allow), then prune. Blocking; errors are logged.
    fn write_files(&self, id: &str, doc: &Document) {
        let Some(dir) = self.dir.clone() else { return };
        let s = self.settings();
        if s.keep_thumbnails == 0 && s.keep_documents == 0 {
            return;
        }
        let _f = lock(&self.files);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            return tracing::warn!(error = %e, "job ledger: cannot create its directory");
        }
        if s.keep_thumbnails > 0 {
            match crate::ledger_thumb::thumbnail_png(doc, s.thumbnail_px) {
                Ok(png) => {
                    if let Err(e) = crate::prefs::write_atomic(&dir.join(format!("{id}.png")), &png) {
                        tracing::warn!(error = %e, "job ledger: cannot write the thumbnail");
                    }
                }
                Err(e) => tracing::warn!(error = %e, "job ledger: no thumbnail"),
            }
        }
        if s.keep_documents > 0 {
            match doc::file::try_to_openetch_json(doc) {
                Ok(json) if json.len() as u64 > u64::from(s.max_document_mb) * 1024 * 1024 => tracing::info!(
                    id,
                    mb = json.len() / (1024 * 1024),
                    "job ledger: the document is larger than max_document_mb; not kept"
                ),
                Ok(json) => {
                    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                    let bytes = gz.write_all(json.as_bytes()).and_then(|()| gz.finish());
                    match bytes {
                        Ok(b) => {
                            if let Err(e) = crate::prefs::write_atomic(&dir.join(format!("{id}.openetch.gz")), &b) {
                                tracing::warn!(error = %e, "job ledger: cannot write the document");
                            }
                        }
                        Err(e) => tracing::warn!(error = %e, "job ledger: cannot compress the document"),
                    }
                }
                Err(e) => tracing::warn!(error = %e, "job ledger: cannot write the document as .openetch"),
            }
        }
        drop(_f);
        self.prune();
    }

    /// Delete thumbnails and documents beyond the newest the settings keep. Ids start with the end time (ten digits
    /// until the year 2286), so their names sort by age.
    pub fn prune(&self) {
        let Some(dir) = &self.dir else { return };
        let s = self.settings();
        let _f = lock(&self.files);
        let names = self.file_names();
        for (ext, keep) in [(".png", s.keep_thumbnails), (".openetch.gz", s.keep_documents)] {
            let mut ids: Vec<&str> = names.iter().filter_map(|n| n.strip_suffix(ext)).filter(|id| check_id(id).is_ok()).collect();
            ids.sort_unstable();
            let excess = ids.len().saturating_sub(keep as usize);
            for id in &ids[..excess] {
                if let Err(e) = std::fs::remove_file(dir.join(format!("{id}{ext}"))) {
                    tracing::warn!(error = %e, id, "job ledger: cannot delete an old file");
                }
            }
        }
    }

    /// How many thumbnails and documents are kept and their size in bytes.
    pub fn usage(&self) -> Value {
        let Some(dir) = &self.dir else { return json!({}) };
        let mut u = [(0u64, 0u64); 2];
        for name in self.file_names() {
            let k = if name.ends_with(".png") {
                0
            } else if name.ends_with(".openetch.gz") {
                1
            } else {
                continue;
            };
            u[k].0 += 1;
            u[k].1 += std::fs::metadata(dir.join(&name)).map(|m| m.len()).unwrap_or(0);
        }
        json!({ "thumbnails": u[0].0, "thumbnail_bytes": u[0].1, "documents": u[1].0, "document_bytes": u[1].1 })
    }
}

/// Ledger ids are `<unix seconds>-run-<n>`: digits, letters and `-` only, so they are safe as file names.
pub fn check_id(id: &str) -> Result<(), EngineError> {
    let ok = !id.is_empty() && id.len() <= 80 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    if ok { Ok(()) } else { Err(EngineError::BadRequest(format!("'{id}' is not a job ledger id"))) }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn origin_name(o: &Origin) -> String {
    match o {
        Origin::Ui => "ui".into(),
        Origin::Script => "script".into(),
        Origin::Agent(name) => format!("agent:{name}"),
    }
}

/// Every layer with objects in `doc` (the page being burned), with its settings and its share of the plan.
pub fn layer_snapshots(doc: &Document, times: &[plan::LayerTime]) -> Vec<LayerSnapshot> {
    let mut counts: HashMap<&str, (usize, usize)> = HashMap::new();
    for o in &doc.objects {
        let c = counts.entry(o.layer.as_str()).or_default();
        c.0 += 1;
        c.1 += usize::from(matches!(o.geometry, Geometry::Image { .. }));
    }
    doc.layers
        .iter()
        .filter(|l| counts.contains_key(l.id.as_str()))
        .map(|l| {
            let t = times.iter().find(|t| t.layer_id == l.id);
            LayerSnapshot {
                id: l.id.clone(),
                name: l.name.clone(),
                color: l.color.clone(),
                output: l.emits(),
                objects: counts[l.id.as_str()].0,
                images: counts[l.id.as_str()].1,
                settings: serde_json::to_value(&l.settings).unwrap_or(Value::Null),
                sub_layers: l.sub_layers.iter().map(|s| serde_json::to_value(s).unwrap_or(Value::Null)).collect(),
                preset: l.link.as_ref().map(|k| format!("{}/{}", k.library, k.path)),
                passes: t.map_or(0, |t| t.passes),
                estimated_time_s: t.map_or(0.0, |t| t.estimated_time_s),
                mark_length_mm: t.map_or(0.0, |t| t.mark_length_mm),
            }
        })
        .collect()
}

fn mode_name(m: VarMode) -> &'static str {
    match m {
        VarMode::Serial => "serial",
        VarMode::Csv => "csv",
        VarMode::DateTime => "date_time",
        VarMode::CutSetting => "cut_setting",
    }
}

/// The values `doc`'s variable texts and barcodes show now (what a run planned now marks); `None` without any.
pub fn variables_used(doc: &Document) -> Option<VariablesUsed> {
    let moment = text::Moment::now();
    let laser = doc.extras.get("lbrn2._root_attrs").and_then(|r| commands::lbrn2_device_name(r)).unwrap_or_default();
    let pockets = doc::jig::object_pockets(doc);
    let mut texts = Vec::new();
    for o in &doc.objects {
        let (format, var) = match &o.geometry {
            Geometry::Text(t) => match &t.variable {
                Some(v) => (t.text.clone(), v.clone()),
                None => continue,
            },
            _ => match barcode_variable(o) {
                Some(x) => x,
                None => continue,
            },
        };
        let value = text::vars::evaluate(doc, &o.layer, &format, &var, Some(&moment), &laser);
        let pocket = pockets.get(&o.id).cloned();
        texts.push(VariableValue { object: o.id.clone(), name: o.name.clone(), mode: mode_name(var.mode).into(), value, pocket });
    }
    if texts.is_empty() {
        return None;
    }
    Some(VariablesUsed {
        current: doc.variables.current,
        csv_file: doc.variables.csv.as_ref().map(|c| c.name.clone()).filter(|n| !n.is_empty()),
        texts,
    })
}

/// A barcode's data format and variable text, from its kept settings (`commands::BARCODE_KEY`).
fn barcode_variable(o: &doc::Object) -> Option<(String, VarText)> {
    let b: Value = serde_json::from_str(o.extras.get(commands::BARCODE_KEY)?).ok()?;
    let v = b.get("variable")?;
    let mode = match v.get("mode")?.as_str()? {
        "serial" => VarMode::Serial,
        "csv" => VarMode::Csv,
        "date_time" => VarMode::DateTime,
        "cut_setting" => VarMode::CutSetting,
        _ => return None,
    };
    let offset = v.get("offset").and_then(Value::as_i64).unwrap_or(0);
    Some((b.get("data")?.as_str()?.to_owned(), VarText { mode, offset }))
}

/// Note what run `view` was planned from (`doc`, the active document as it was planned; `times`, its layers' share),
/// so [`record`] can keep it when the run ends.
pub(crate) fn remember(st: &AppState, view: &RunView, doc: Arc<Document>, times: Vec<plan::LayerTime>) {
    let queue = view.options.queue.is_some();
    let options = (view.options != RunOptions::default()).then(|| serde_json::to_value(&view.options).unwrap_or(Value::Null));
    let c = Capture {
        run_id: view.id.clone(),
        layers: if queue { Vec::new() } else { layer_snapshots(&doc, &times) },
        variables: if queue { None } else { variables_used(&doc) },
        jigs: if queue { Vec::new() } else { crate::joblog::jigs_used(&doc) },
        doc: (!queue).then_some(doc),
        estimated_time_s: view.stats.estimated_time_s,
        options,
    };
    st.libs.job_ledger(|l| l.remember(c));
}

/// Append run `entry`'s job log line with what [`remember`] noted for it (and the board's mark time), then write its
/// thumbnail and document in the background.
pub(crate) fn record(st: &Arc<AppState>, mut entry: JobLogEntry) {
    let capture = st.libs.job_ledger(|l| l.take(&entry.run_id));
    if let Some(r) = st.runs().get(&entry.run_id) {
        entry.mark_time_s = r.report.as_ref().and_then(machine::RunReport::mark_time_s);
        entry.revision = Some(r.revision);
    }
    if let Some(c) = &capture {
        entry.layers = c.layers.clone();
        entry.variables = c.variables.clone();
        entry.jigs = c.jigs.clone();
        entry.estimated_time_s = Some(c.estimated_time_s);
        entry.options = c.options.clone();
        entry.document = c.doc.as_ref().map(|d| d.name.clone());
    }
    st.libs.append_job_log(&entry);
    let id = entry.id();
    st.emit(Event::machine("job_log_changed", Some(id.clone())));
    if let Some(doc) = capture.and_then(|c| c.doc.clone()) {
        let st = st.clone();
        tokio::task::spawn_blocking(move || st.libs.job_ledger(|l| l.write_files(&id, &doc)));
    }
}

/// A search of the ledger (`GET /api/job-log`, MCP `job_ledger_search`). Every filter is optional.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LedgerQuery {
    /// Words that must all appear (ignoring case) in the project, document, device, summary, layer names, colours,
    /// settings, variable values, note, origin or error. E.g. "blue coin".
    #[serde(default)]
    pub q: Option<String>,
    /// `ok`, `scrap`, `unmarked` (no verdict) or `any` (default).
    #[serde(default)]
    pub verdict: Option<String>,
    /// The run's end: `finished`, `failed`, `aborted` or `cancelled`.
    #[serde(default)]
    pub state: Option<String>,
    /// The device id (exact).
    #[serde(default)]
    pub device: Option<String>,
    /// The project name (ignoring case, part of it).
    #[serde(default)]
    pub project: Option<String>,
    /// Only runs that ended at or after this Unix time, s.
    #[serde(default)]
    pub since_unix_s: Option<u64>,
    /// Only runs that ended before this Unix time, s.
    #[serde(default)]
    pub until_unix_s: Option<u64>,
    /// Most entries (newest first; default 200, at most 2000).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// One ledger entry as clients see it: the log line plus its id, verdict and note, and what is kept of it.
#[derive(Debug, Clone, Serialize)]
pub struct EntryView {
    pub id: String,
    #[serde(flatten)]
    pub entry: JobLogEntry,
    pub verdict: Option<Verdict>,
    pub note: Option<String>,
    pub noted_unix_s: Option<u64>,
    pub noted_by: Option<String>,
    /// A thumbnail is kept (`GET /api/job-log/{id}/thumbnail.png`).
    pub thumbnail: bool,
    /// The document is kept: Re-run and Open as burned work.
    pub rerun: bool,
    /// Why Re-run is not available, when it is not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rerun_reason: Option<String>,
}

fn haystack(e: &JobLogEntry, note: Option<&Note>) -> String {
    let mut h = vec![
        e.run_id.clone(),
        e.state.clone(),
        e.origin.clone(),
        e.device.clone(),
        e.project.clone().unwrap_or_default(),
        e.document.clone().unwrap_or_default(),
        e.summary.clone(),
        e.error.clone().unwrap_or_default(),
    ];
    for l in &e.layers {
        h.push(l.name.clone());
        h.push(l.color.clone());
        h.push(crate::joblog::layer_short(l));
        h.extend(l.preset.clone());
    }
    if let Some(v) = &e.variables {
        h.push(crate::joblog::variables_short(v));
    }
    for j in &e.jigs {
        h.push(j.name.clone());
    }
    if let Some(n) = note {
        h.push(n.note.clone());
        h.extend(n.verdict.map(|v| if v == Verdict::Ok { "ok".to_string() } else { "scrap".to_string() }));
    }
    h.join("\n").to_lowercase()
}

/// The entries matching `q`, newest first.
pub fn search(st: &AppState, q: &LedgerQuery) -> Result<Vec<EntryView>, EngineError> {
    let verdict = match q.verdict.as_deref().map(str::to_ascii_lowercase).as_deref() {
        None | Some("") | Some("any") => None,
        Some("ok") => Some(Some(Verdict::Ok)),
        Some("scrap") => Some(Some(Verdict::Scrap)),
        Some("unmarked") | Some("none") => Some(None),
        Some(other) => return Err(EngineError::BadRequest(format!("unknown verdict '{other}' (ok, scrap, unmarked or any)"))),
    };
    let words: Vec<String> = q.q.as_deref().unwrap_or_default().split_whitespace().map(str::to_lowercase).collect();
    let project = q.project.as_deref().map(str::to_lowercase).filter(|p| !p.is_empty());
    let limit = q.limit.unwrap_or(200).clamp(1, MAX_RESULTS);
    let all = st.libs.read_job_log(crate::joblog::MAX_READ)?;
    let (notes, files, settings) = st.libs.job_ledger(|l| (l.notes(), l.file_names(), l.settings()));
    let mut out = Vec::new();
    for e in all.into_iter().rev() {
        if out.len() >= limit {
            break;
        }
        let id = e.id();
        let note = notes.get(&id);
        if verdict.is_some_and(|v| note.and_then(|n| n.verdict) != v)
            || q.state.as_deref().is_some_and(|s| !s.is_empty() && s != e.state)
            || q.device.as_deref().is_some_and(|d| !d.is_empty() && d != e.device)
            || project.as_deref().is_some_and(|p| !e.project.as_deref().unwrap_or_default().to_lowercase().contains(p))
            || q.since_unix_s.is_some_and(|t| e.time_unix_s < t)
            || q.until_unix_s.is_some_and(|t| e.time_unix_s >= t)
        {
            continue;
        }
        if !words.is_empty() {
            let h = haystack(&e, note);
            if !words.iter().all(|w| h.contains(w.as_str())) {
                continue;
            }
        }
        out.push(view(e, note, &files, &settings));
    }
    Ok(out)
}

fn view(entry: JobLogEntry, note: Option<&Note>, files: &HashSet<String>, settings: &LedgerSettings) -> EntryView {
    let id = entry.id();
    let rerun = files.contains(&format!("{id}.openetch.gz"));
    let queue = entry.options.as_ref().is_some_and(|o| o.get("queue").is_some_and(|q| !q.is_null()));
    let rerun_reason = (!rerun).then(|| {
        if queue {
            "a batch of projects: run the batch again from its projects".to_string()
        } else if entry.layers.is_empty() {
            "logged before the job ledger kept documents".to_string()
        } else if settings.keep_documents == 0 {
            "the job ledger keeps no documents (Storage settings)".to_string()
        } else {
            format!(
                "not kept: documents are kept for the newest {} jobs, up to {} MB each",
                settings.keep_documents, settings.max_document_mb
            )
        }
    });
    EntryView {
        thumbnail: files.contains(&format!("{id}.png")),
        verdict: note.and_then(|n| n.verdict),
        note: note.map(|n| n.note.clone()).filter(|n| !n.is_empty()),
        noted_unix_s: note.map(|n| n.time_unix_s),
        noted_by: note.map(|n| n.origin.clone()),
        rerun,
        rerun_reason,
        id,
        entry,
    }
}

/// Entry `id`, or 404.
pub fn get(st: &AppState, id: &str) -> Result<EntryView, EngineError> {
    check_id(id)?;
    let all = st.libs.read_job_log(crate::joblog::MAX_READ)?;
    let e = all
        .into_iter()
        .rev()
        .find(|e| e.id() == id)
        .ok_or_else(|| EngineError::NotFound(format!("no job ledger entry '{id}'")))?;
    let (notes, files, settings) = st.libs.job_ledger(|l| (l.notes(), l.file_names(), l.settings()));
    Ok(view(e, notes.get(id), &files, &settings))
}

/// Set entry `id`'s verdict and note (`None` / empty clear them). Returns the entry.
pub fn mark(st: &AppState, id: &str, verdict: Option<Verdict>, note: &str, origin: &Origin) -> Result<EntryView, EngineError> {
    get(st, id)?;
    let note = note.trim();
    if note.chars().count() > MAX_NOTE_CHARS {
        return Err(EngineError::BadRequest(format!("a note may have at most {MAX_NOTE_CHARS} characters")));
    }
    let n = Note { id: id.to_owned(), time_unix_s: now_unix(), verdict, note: note.to_owned(), origin: origin_name(origin) };
    st.libs.job_ledger(|l| l.append_note(&n))?;
    st.emit(Event::machine("job_log_changed", Some(id.to_owned())));
    get(st, id)
}

/// Open entry `id`'s stored document as a new open document and make it the active one (nothing is planned or
/// requested). Returns `{ok, id, document_id, revision, device_mismatch?}`.
pub(crate) async fn open_document(st: &Arc<AppState>, id: &str, origin: Origin) -> Result<Value, EngineError> {
    // Before a document is opened: its activation would be refused (module `production::lock`).
    crate::production::lock::check_timeline(st)?;
    let entry = get(st, id)?;
    let (owned, st2) = (id.to_owned(), st.clone());
    let content = tokio::task::spawn_blocking(move || st2.libs.job_ledger(|l| l.document_json(&owned))).await??;
    let doc_id = st.new_document()?;
    let label = format!(
        "{} (as burned {})",
        entry.entry.project.clone().or(entry.entry.document.clone()).unwrap_or_else(|| "job".into()),
        entry.entry.run_id
    );
    let (did, org) = (doc_id.clone(), origin.clone());
    let loaded = st
        .blocking(move |st| {
            let inv = Invocation { id: "doc.load".into(), params: json!({ "content": content }) };
            let _s = lock(&st.session);
            let mut d = lock(&st.documents);
            let o = d
                .others
                .iter_mut()
                .find(|o| o.id == did)
                .ok_or_else(|| EngineError::Conflict(format!("document '{did}' was closed or made active while it was loaded")))?;
            let out = o.session.execute(inv, org)?;
            o.project = crate::projects::ProjectState {
                display_name: Some(label),
                ..crate::projects::ProjectState::clean_at(out.revision)
            };
            Ok(out.revision)
        })
        .await;
    if let Err(e) = loaded {
        let _ = st.close_document(&doc_id, true);
        return Err(e);
    }
    let a = st.activate_document(&doc_id, origin)?;
    tracing::info!(entry = id, document = %doc_id, "job ledger: the burned document was opened");
    let active = st.active_device().id;
    let mut v = json!({ "ok": true, "id": id, "document_id": doc_id, "revision": a["revision"] });
    if active != entry.entry.device {
        v["device_mismatch"] = json!(format!(
            "this job was burned on device '{}', and '{active}' is active: check the device before running it",
            entry.entry.device
        ));
    }
    Ok(v)
}

/// Re-run exactly as burned: [`open_document`], then a *pending* run of it with the stored run options (never
/// approved here: a person starts it in the app). When the device differs from the one it was burned on, or the run
/// cannot be planned, the document stays open and `run_error` says why. Returns `open_document`'s answer plus `run`
/// (the pending run) or `run_error`.
pub(crate) async fn rerun(st: &Arc<AppState>, id: &str, origin: Origin) -> Result<Value, EngineError> {
    let entry = get(st, id)?;
    let mut v = open_document(st, id, origin.clone()).await?;
    if let Some(m) = v.get("device_mismatch").and_then(Value::as_str) {
        v["run_error"] = json!(format!("no run was requested: {m}"));
        return Ok(v);
    }
    // Options that no longer read (an edited log, a newer engine's) are never replaced by the defaults: those would
    // burn the whole document instead of the selection or region that was burned.
    let options = match entry.entry.options.clone() {
        Some(o) => match serde_json::from_value::<RunOptions>(o) {
            Ok(o) => o,
            Err(e) => {
                v["run_error"] = json!(format!("no run was requested: the stored run options cannot be read ({e})"));
                return Ok(v);
            }
        },
        None => RunOptions::default(),
    };
    match st.request_run_with(origin, options).await {
        Ok(run) => v["run"] = serde_json::to_value(&run).unwrap_or(Value::Null),
        Err(e) => v["run_error"] = json!(e.to_string()),
    }
    Ok(v)
}

#[cfg(test)]
#[path = "ledger_tests.rs"]
mod tests;
