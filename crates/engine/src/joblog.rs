//! The job log (LightBurn's Save Job Log, which writes `LightBurnJobs.csv`): one JSON line per run that ended,
//! appended to `<config-dir>/job-log.jsonl` (never rewritten), with who asked for it (`origin`: the UI, a script or
//! an agent), the device, the project, the summary, the outcome and the time it took. `GET /api/job-log` reads the
//! newest entries back, as JSON or CSV.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::EngineError;

pub const JOB_LOG_FILE: &str = "job-log.jsonl";
/// Most entries one read returns.
pub const MAX_READ: usize = 10_000;
/// Largest log file that is read back (older lines beyond it are skipped).
const MAX_READ_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
}

/// The log file (none: entries are dropped, e.g. in tests without a config directory).
#[derive(Debug, Default)]
pub struct JobLog {
    path: Option<PathBuf>,
}

impl JobLog {
    pub fn open(dir: &Path) -> JobLog {
        JobLog { path: Some(dir.join(JOB_LOG_FILE)) }
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

/// The entries as CSV with a header row (LightBurn's job log is a CSV file).
pub fn to_csv(entries: &[JobLogEntry]) -> String {
    let mut out = String::from("time_unix_s,run_id,state,origin,device,project,duration_s,completed,summary,error\n");
    for e in entries {
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
        ];
        out.push_str(&row.iter().map(|f| csv_field(f)).collect::<Vec<_>>().join(","));
        out.push('\n');
    }
    out
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
}
