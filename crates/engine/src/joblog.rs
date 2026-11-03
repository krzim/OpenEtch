//! The job log (LightBurn's Save Job Log, which writes `LightBurnJobs.csv`): one JSON line per run that ended,
//! appended to `<config-dir>/job-log.jsonl` (never rewritten), with who asked for it (`origin`: the UI, a script or
//! an agent), the device, the project, the summary, the outcome and the time it took. `GET /api/job-log` reads the
//! newest entries back, as JSON or CSV.
//!
//! Since the job ledger (module `ledger`) a line also carries what was burned: every used layer's settings and its
//! share of the time, the serial number and CSV row and each variable text's value, the run options and the
//! estimate. Older lines without these fields still read (every new field has a default).

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::EngineError;

pub const JOB_LOG_FILE: &str = "job-log.jsonl";
/// Most entries one read returns.
pub const MAX_READ: usize = 10_000;
/// Largest log file that is read back (older lines beyond it are skipped).
const MAX_READ_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct JobLogEntry {
    /// When the run ended, Unix seconds.
    pub time_unix_s: u64,
    pub run_id: String,
    /// `finished`, `failed`, `aborted` or `cancelled`.
    pub state: String,
    /// Who requested the run: `ui`, `script` or `agent:<name>`.
    pub origin: String,
    pub device: String,
    /// The current project, if the document was one.
    pub project: Option<String>,
    pub summary: String,
    /// From approval to the end, s.
    pub duration_s: f64,
    /// Iterations that ran to the end.
    pub completed: u32,
    pub error: Option<String>,
    /// The document's name (also when it was no saved project).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document: Option<String>,
    /// The document revision the run was planned from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    /// Every layer that had objects in the job, with its settings as burned (job ledger).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<LayerSnapshot>,
    /// The variable text values the run used (serial number, CSV row, each text's value), if it had any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variables: Option<VariablesUsed>,
    /// The planner's estimate for one iteration (with the device's learned correction), s.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_time_s: Option<f64>,
    /// The board's own mark time of the last iteration, s (galvo boards that report it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark_time_s: Option<f64>,
    /// The run options (repeat, continuous, selection, start here, queue, table) when not the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<serde_json::Value>,
}

/// One layer of a ledger entry: its settings as burned (`settings` and `sub_layers` are `doc::CutSettings` as JSON,
/// kept as JSON so a later change of the settings model never makes an old line unreadable).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LayerSnapshot {
    pub id: String,
    pub name: String,
    pub color: String,
    /// Whether the layer was output (a layer with output off is listed but did not burn).
    pub output: bool,
    /// Objects on it in the job.
    pub objects: usize,
    /// `doc::CutSettings` of its main settings.
    pub settings: serde_json::Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sub_layers: Vec<serde_json::Value>,
    /// The library preset it was linked to (`library/path`), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// Job passes planned for it.
    #[serde(default)]
    pub passes: usize,
    /// Its share of one iteration's estimate, s.
    #[serde(default)]
    pub estimated_time_s: f64,
    #[serde(default)]
    pub mark_length_mm: f64,
}

/// The variable text of a ledger entry.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct VariablesUsed {
    /// The serial number / CSV row counter the run used (`Document.variables.current`).
    pub current: i64,
    /// The loaded CSV file's name, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub csv_file: Option<String>,
    /// Each variable text as it was marked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub texts: Vec<VariableValue>,
}

/// One variable text object's value.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct VariableValue {
    pub object: String,
    pub name: String,
    /// `serial`, `csv`, `date_time` or `cut_setting`.
    pub mode: String,
    pub value: String,
}

impl JobLogEntry {
    /// The ledger id: the end time and the run id (`1730000000-run-3`), unique across engine restarts (run ids start
    /// again at 1, the clock does not).
    pub fn id(&self) -> String {
        format!("{}-{}", self.time_unix_s, self.run_id)
    }
}

/// The log file (none: entries are dropped, e.g. in tests without a config directory) and the job ledger's store.
#[derive(Debug, Default)]
pub struct JobLog {
    path: Option<PathBuf>,
    /// The job ledger: thumbnails, stored documents, notes, its settings (module `ledger`).
    pub ledger: crate::ledger::Ledger,
}

impl JobLog {
    pub fn open(dir: &Path) -> JobLog {
        JobLog { path: Some(dir.join(JOB_LOG_FILE)), ledger: crate::ledger::Ledger::open(dir) }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Append one entry (one line; a failed write is logged, never fatal).
    pub fn append(&self, e: &JobLogEntry) {
        let Some(path) = &self.path else { return };
        let line = match serde_json::to_string(e) {
            Ok(l) => l,
            Err(err) => return tracing::warn!(error = %err, "job log: cannot encode the entry"),
        };
        let written = std::fs::OpenOptions::new().create(true).append(true).open(path).and_then(|mut f| writeln!(f, "{line}"));
        if let Err(err) = written {
            tracing::warn!(path = %path.display(), error = %err, "job log: cannot append");
        }
    }

    /// The newest `limit` entries, oldest first (lines that do not parse are skipped).
    pub fn read(&self, limit: usize) -> Result<Vec<JobLogEntry>, EngineError> {
        let Some(path) = &self.path else { return Ok(Vec::new()) };
        let text = match std::fs::metadata(path) {
            Ok(m) if m.len() > MAX_READ_BYTES => {
                use std::io::{Read, Seek, SeekFrom};
                let mut f = std::fs::File::open(path).map_err(|e| EngineError::Internal(e.to_string()))?;
                f.seek(SeekFrom::End(-(MAX_READ_BYTES as i64))).map_err(|e| EngineError::Internal(e.to_string()))?;
                let mut buf = Vec::new();
                f.read_to_end(&mut buf).map_err(|e| EngineError::Internal(e.to_string()))?;
                String::from_utf8_lossy(&buf).into_owned()
            }
            Ok(_) => std::fs::read_to_string(path).map_err(|e| EngineError::Internal(e.to_string()))?,
            Err(_) => return Ok(Vec::new()),
        };
        let all: Vec<JobLogEntry> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        let n = limit.min(MAX_READ);
        Ok(all[all.len().saturating_sub(n)..].to_vec())
    }
}

fn csv_field(s: &str) -> String {
    // A text cell starting like a formula would run as one in a spreadsheet (project names and error messages are
    // free text): it is prefixed with an apostrophe, the usual guard.
    let s = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) { format!("'{s}") } else { s.to_string() };
    if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s }
}

/// The entries as CSV with a header row (LightBurn's job log is a CSV file). The ledger's columns come last (see
/// [`to_csv_with`]).
pub fn to_csv(entries: &[JobLogEntry]) -> String {
    to_csv_with(entries, &|_| None)
}

/// [`to_csv`] with each entry's verdict and note (`notes` by entry id), the output layers' settings in short ("Fill:
/// 40% 1000 mm/s 30 kHz 200 ns x2") and the variable text values.
pub fn to_csv_with(entries: &[JobLogEntry], notes: &dyn Fn(&str) -> Option<(String, String)>) -> String {
    let mut out = String::from(
        "time_unix_s,run_id,state,origin,device,project,duration_s,completed,summary,error,verdict,note,layers,variables\n",
    );
    for e in entries {
        let (verdict, note) = notes(&e.id()).unwrap_or_default();
        let row = [
            e.time_unix_s.to_string(),
            e.run_id.clone(),
            e.state.clone(),
            e.origin.clone(),
            e.device.clone(),
            e.project.clone().unwrap_or_default(),
            format!("{:.1}", e.duration_s),
            e.completed.to_string(),
            e.summary.clone(),
            e.error.clone().unwrap_or_default(),
            verdict,
            note,
            e.layers.iter().filter(|l| l.output).map(layer_short).collect::<Vec<_>>().join("; "),
            e.variables.as_ref().map(variables_short).unwrap_or_default(),
        ];
        out.push_str(&row.iter().map(|f| csv_field(f)).collect::<Vec<_>>().join(","));
        out.push('\n');
    }
    out
}

/// "Fill: 40% 1000 mm/s 30 kHz 200 ns x2" (power, speed, frequency, pulse width, passes; sub-layers after "+").
pub fn layer_short(l: &LayerSnapshot) -> String {
    let one = |s: &serde_json::Value| {
        let n = |k: &str| s.get(k).and_then(serde_json::Value::as_f64);
        let mut t = Vec::new();
        if let Some(v) = n("power_pct") {
            t.push(format!("{}%", trim(v)));
        }
        if let Some(v) = n("speed_mm_s") {
            t.push(format!("{} mm/s", trim(v)));
        }
        if let Some(v) = n("frequency_khz") {
            t.push(format!("{} kHz", trim(v)));
        }
        if let Some(v) = n("pulse_width_ns") {
            t.push(format!("{} ns", trim(v)));
        }
        if let Some(v) = n("passes").filter(|v| *v > 1.0) {
            t.push(format!("x{}", trim(v)));
        }
        t.join(" ")
    };
    let mut out = format!("{}: {}", l.name, one(&l.settings));
    for s in &l.sub_layers {
        out.push_str(" + ");
        out.push_str(&one(s));
    }
    out
}

/// "#42 (rows.csv); Serial: 0042, Name: Ana".
pub fn variables_short(v: &VariablesUsed) -> String {
    let mut out = format!("#{}", v.current);
    if let Some(f) = &v.csv_file {
        out.push_str(&format!(" ({f})"));
    }
    if !v.texts.is_empty() {
        out.push_str("; ");
        out.push_str(&v.texts.iter().map(|t| format!("{}: {}", t.name, t.value)).collect::<Vec<_>>().join(", "));
    }
    out
}

/// A number without trailing zeros (40, 0.5, 1000).
fn trim(v: f64) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(n: u64) -> JobLogEntry {
        JobLogEntry {
            time_unix_s: n,
            run_id: format!("run-{n}"),
            state: "finished".into(),
            origin: "agent:claude".into(),
            device: "b6".into(),
            project: Some("tags, \"v2\"".into()),
            summary: "3 polylines".into(),
            duration_s: 1.25,
            completed: 1,
            error: None,
            ..JobLogEntry::default()
        }
    }

    #[test]
    fn appends_and_reads_the_newest() {
        let dir = std::env::temp_dir().join(format!("openetch-joblog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = JobLog::open(&dir);
        for n in 1..=5 {
            log.append(&entry(n));
        }
        std::fs::OpenOptions::new().append(true).open(log.path().unwrap()).unwrap().write_all(b"not json\n").unwrap();
        log.append(&entry(6));
        let last = log.read(3).unwrap();
        assert_eq!(last.iter().map(|e| e.time_unix_s).collect::<Vec<_>>(), [4, 5, 6]);
        let csv = to_csv(&last);
        assert!(csv.lines().nth(1).unwrap().contains("\"tags, \"\"v2\"\"\""), "{csv}");
        assert_eq!(JobLog::default().read(5).unwrap(), vec![]);
        let mut bad = entry(7);
        bad.project = Some("=HYPERLINK(\"http://x\")".into());
        bad.error = Some("@SUM(1)".into());
        let csv = to_csv(&[bad]);
        assert!(csv.contains(",\"'=HYPERLINK(\"\"http://x\"\")\",") && csv.contains(",'@SUM(1)"), "{csv}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_lines_still_read_and_ledger_columns_follow() {
        let old = r#"{"time_unix_s":5,"run_id":"run-2","state":"finished","origin":"ui","device":"b6","project":null,"summary":"s","duration_s":2.0,"completed":1,"error":null}"#;
        let e: JobLogEntry = serde_json::from_str(old).unwrap();
        assert_eq!((e.id(), e.layers.len(), e.variables.is_none()), ("5-run-2".to_string(), 0, true));
        let mut e = entry(9);
        e.layers = vec![LayerSnapshot {
            name: "Blue".into(),
            output: true,
            settings: serde_json::json!({"power_pct": 40.0, "speed_mm_s": 1000.0, "frequency_khz": 30.0, "pulse_width_ns": 200.0, "passes": 2}),
            sub_layers: vec![serde_json::json!({"power_pct": 10.5, "speed_mm_s": 3000.0, "frequency_khz": 600.0})],
            ..LayerSnapshot::default()
        }];
        e.variables = Some(VariablesUsed {
            current: 42,
            csv_file: Some("rows.csv".into()),
            texts: vec![VariableValue { name: "Serial".into(), value: "0042".into(), ..VariableValue::default() }],
        });
        let csv = to_csv_with(std::slice::from_ref(&e), &|id| (id == "9-run-9").then(|| ("scrap".to_string(), "too dark".to_string())));
        let row = csv.lines().nth(1).unwrap();
        assert!(
            row.ends_with(",scrap,too dark,Blue: 40% 1000 mm/s 30 kHz 200 ns x2 + 10.5% 3000 mm/s 600 kHz,#42 (rows.csv); Serial: 0042"),
            "{row}"
        );
        let back: JobLogEntry = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(back, e);
    }
}
