//! Notifications (short list #3): production events sent to ntfy, a generic webhook or Home Assistant, and kept in
//! an event feed that MCP agents read (`production_events`) and the app shows.
//!
//! **Events** ([`Kind`]): `job_done`, `waiting_for_foot_switch` (the foot switch cycle frames and waits for a press,
//! or a run waits for its start input, for longer than `waiting_delay_s`), `door` (a run or a foot switch press
//! stopped by the door interlock), `error` (a run failed, a frame error), `stop_at_reached`, `layer_pause` (a run
//! waits at a layer for Continue), `maintenance_due` (module `meter`), and `test` (the Settings Test button). Every
//! event carries `operation`: `frame` (it happened while framing, e.g. the cycle waiting with the job framed) or
//! `mark` (a run), as LightBurn's own events cannot tell them apart.
//!
//! **Targets** (`<config-dir>/notifications.json`, private: on Unix `0600`): up to [`MAX_TARGETS`], each `ntfy`
//! (JSON publish to the server URL with the topic; an access token as `Authorization: Bearer`), `webhook` (the event
//! JSON POSTed to the URL) or `home_assistant` (the event fired on Home Assistant's REST API as `openetch_event`,
//! with a long-lived access token). Each target picks its events. Tokens are never sent back to clients
//! (`token_set` only), never logged and never put on a command line.
//!
//! **Outbound only.** The engine sends HTTP requests through the system `curl` (as the update check does: no TLS
//! stack in the engine), with the request on curl's standard input (`--config -`), only `http` and `https`, no
//! redirects, 10 s at most, one at a time from a bounded queue (a full queue drops the newest event and logs it).
//! Nothing that comes back is acted on, and no route or tool takes events in: nothing from outside can start a burn.
//! Configuring targets is UI-only (an agent could otherwise send the feed anywhere); reading the feed is open to
//! agents (read-only).

use std::collections::VecDeque;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};

use super::now_unix;
use crate::libs::lock;
use crate::runs::RunState;
use crate::{AppState, EngineError, Event};

pub const NOTIFICATIONS_FILE: &str = "notifications.json";
const FORMAT: &str = "openetch-notifications";
/// Most targets.
pub const MAX_TARGETS: usize = 16;
/// Events kept for the feed.
pub const FEED_LEN: usize = 200;
/// Longest a feed read may wait for a new event, s.
pub const MAX_WAIT_S: u64 = 25;
/// Longest a request to a target may take, s.
pub const SEND_TIMEOUT_S: u64 = 10;
/// Events waiting to be sent before new ones are dropped.
const QUEUE: usize = 64;
/// Default delay before "waiting for the foot switch" is sent, s.
pub const DEFAULT_WAITING_DELAY_S: u64 = 30;

/// What happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    JobDone,
    WaitingForFootSwitch,
    Door,
    Error,
    StopAtReached,
    LayerPause,
    MaintenanceDue,
    Test,
}

impl Kind {
    /// Every kind a target can subscribe to (Test always goes to the target tested).
    pub const ALL: [Kind; 7] = [
        Kind::JobDone,
        Kind::WaitingForFootSwitch,
        Kind::Door,
        Kind::Error,
        Kind::StopAtReached,
        Kind::LayerPause,
        Kind::MaintenanceDue,
    ];

    fn title(self) -> &'static str {
        match self {
            Kind::JobDone => "Job finished",
            Kind::WaitingForFootSwitch => "Waiting for you",
            Kind::Door => "Door open",
            Kind::Error => "Laser error",
            Kind::StopAtReached => "Stop At reached",
            Kind::LayerPause => "Paused at a layer",
            Kind::MaintenanceDue => "Maintenance due",
            Kind::Test => "Test notification",
        }
    }

    /// ntfy priority (3 default, 4 high) and tag.
    fn ntfy(self) -> (u8, &'static str) {
        match self {
            Kind::JobDone => (3, "white_check_mark"),
            Kind::WaitingForFootSwitch => (4, "hourglass"),
            Kind::Door => (4, "door"),
            Kind::Error => (4, "warning"),
            Kind::StopAtReached => (4, "checkered_flag"),
            Kind::LayerPause => (4, "pause_button"),
            Kind::MaintenanceDue => (3, "wrench"),
            Kind::Test => (3, "bell"),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::JobDone => "job_done",
            Kind::WaitingForFootSwitch => "waiting_for_foot_switch",
            Kind::Door => "door",
            Kind::Error => "error",
            Kind::StopAtReached => "stop_at_reached",
            Kind::LayerPause => "layer_pause",
            Kind::MaintenanceDue => "maintenance_due",
            Kind::Test => "test",
        }
    }
}

/// Whether the event happened while framing or while marking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Frame,
    Mark,
    None,
}

/// One production event (the feed entry and the body webhooks get).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotifyEvent {
    /// Feed position: increases by one per event (from 1 at engine start).
    pub seq: u64,
    pub event: Kind,
    pub operation: Operation,
    pub title: String,
    pub message: String,
    pub time_unix_s: u64,
    pub device: String,
    pub project: Option<String>,
    pub run_id: Option<String>,
    /// The job counter and Stop At of the device.
    pub count: u64,
    pub stop_at: Option<u64>,
    pub detail: Option<String>,
    pub source: String,
}

/// How a target is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Ntfy,
    Webhook,
    HomeAssistant,
}

/// One notification target, as stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub id: String,
    pub name: String,
    pub kind: TargetKind,
    /// ntfy: the server (`https://ntfy.sh`); webhook: the URL; Home Assistant: its base URL
    /// (`http://homeassistant.local:8123`).
    pub url: String,
    /// ntfy's topic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// ntfy access token or Home Assistant long-lived access token (sent as `Authorization: Bearer`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default)]
    pub events: Vec<Kind>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

/// A target as clients see it: the token replaced by whether one is set.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TargetView {
    pub id: String,
    pub name: String,
    pub kind: TargetKind,
    pub url: String,
    pub topic: Option<String>,
    pub token_set: bool,
    pub events: Vec<Kind>,
    pub enabled: bool,
}

/// A target as clients send it (`PUT /api/notifications`): `token` absent keeps the stored one, `""` clears it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetInput {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    pub kind: TargetKind,
    pub url: String,
    #[serde(default)]
    pub topic: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub events: Vec<Kind>,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub token_set: Option<bool>,
}

/// `notifications.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub format: String,
    pub version: u32,
    /// How long the cycle (or a run's start input) must wait for the foot switch before that is sent, s.
    pub waiting_delay_s: u64,
    pub targets: Vec<Target>,
}

impl Default for Config {
    fn default() -> Self {
        Config { format: FORMAT.into(), version: 1, waiting_delay_s: DEFAULT_WAITING_DELAY_S, targets: Vec::new() }
    }
}

/// Body of `PUT /api/notifications`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigInput {
    #[serde(default)]
    pub waiting_delay_s: Option<u64>,
    pub targets: Vec<TargetInput>,
    #[serde(default)]
    pub origin: Option<Value>,
}

/// One request to send.
#[derive(Debug, Clone)]
struct Delivery {
    target: Target,
    event: NotifyEvent,
}

/// The notifier (one field of `Production`).
#[derive(Debug, Default)]
pub struct Notifier {
    path: Mutex<Option<PathBuf>>,
    config: Mutex<Config>,
    feed: Mutex<VecDeque<NotifyEvent>>,
    seq: AtomicU64,
    arrived: tokio::sync::Notify,
    sender: Mutex<Option<mpsc::Sender<Delivery>>>,
    /// Bumped whenever a wait for the foot switch starts or ends, so a delayed check of an older wait sends nothing.
    waiting: AtomicU64,
    /// The last delivery's outcome per target id: `(time, ok, text)`.
    results: Mutex<std::collections::BTreeMap<String, (u64, bool, String)>>,
}

impl Notifier {
    /// Read the targets from `<dir>/notifications.json` (none when missing; an unreadable file is reported).
    pub fn open(&self, dir: &Path) -> Option<String> {
        let path = dir.join(NOTIFICATIONS_FILE);
        let (config, note) = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<Config>(&text) {
                Ok(c) if c.format == FORMAT => (c, None),
                Ok(_) => (Config::default(), Some(format!("{}: not a notifications file; none are sent", path.display()))),
                Err(e) => (Config::default(), Some(format!("{}: {e}; no notifications are sent", path.display()))),
            },
            Err(_) => (Config::default(), None),
        };
        *lock(&self.config) = config;
        *lock(&self.path) = Some(path);
        note
    }

    pub fn config(&self) -> Config {
        lock(&self.config).clone()
    }

    /// `GET /api/notifications`: the targets without their tokens, the event kinds and the last delivery per target.
    pub fn view(&self) -> Value {
        let c = self.config();
        let results = lock(&self.results).clone();
        let targets: Vec<Value> = c
            .targets
            .iter()
            .map(|t| {
                let mut v = serde_json::to_value(view_of(t)).unwrap_or(Value::Null);
                if let Some((at, ok, text)) = results.get(&t.id) {
                    v["last"] = json!({ "time_unix_s": at, "ok": ok, "result": text });
                }
                v
            })
            .collect();
        json!({
            "waiting_delay_s": c.waiting_delay_s,
            "targets": targets,
            "kinds": Kind::ALL,
            "max_targets": MAX_TARGETS,
        })
    }

    /// The cockpit's summary: how many targets, the newest event.
    pub fn summary_json(&self) -> Value {
        let c = lock(&self.config);
        let enabled = c.targets.iter().filter(|t| t.enabled).count();
        drop(c);
        json!({ "targets": enabled, "last": lock(&self.feed).back().cloned() })
    }

    /// Replace the targets (`PUT /api/notifications`, UI only). A target sent without `token` keeps the stored one
    /// of the same id; `""` clears it.
    pub fn replace(&self, input: ConfigInput) -> Result<Value, EngineError> {
        if input.targets.len() > MAX_TARGETS {
            return Err(EngineError::BadRequest(format!("at most {MAX_TARGETS} notification targets")));
        }
        let old = self.config();
        let mut targets = Vec::with_capacity(input.targets.len());
        for (i, t) in input.targets.into_iter().enumerate() {
            let id = match t.id.filter(|id| !id.is_empty()) {
                Some(id) => id,
                None => new_id(&targets, i),
            };
            let kept = old.targets.iter().find(|o| o.id == id).and_then(|o| o.token.clone());
            let token = match t.token {
                Some(s) if s.is_empty() => None,
                Some(s) => Some(s),
                None => kept,
            };
            let mut events = t.events;
            events.retain(|k| *k != Kind::Test);
            events.sort_by_key(|k| Kind::ALL.iter().position(|a| a == k));
            events.dedup();
            let target = Target {
                id,
                name: t.name.trim().to_owned(),
                kind: t.kind,
                url: t.url.trim().trim_end_matches('/').to_owned(),
                topic: t.topic.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()),
                token,
                events,
                enabled: t.enabled,
            };
            check_target(&target)?;
            if targets.iter().any(|o: &Target| o.id == target.id) {
                return Err(EngineError::BadRequest(format!("two targets have the id '{}'", target.id)));
            }
            targets.push(target);
        }
        let waiting_delay_s = input.waiting_delay_s.unwrap_or(old.waiting_delay_s);
        if waiting_delay_s > 3600 {
            return Err(EngineError::BadRequest("waiting_delay_s must be within 0..=3600".into()));
        }
        let config = Config { waiting_delay_s, targets, ..Config::default() };
        self.save(&config)?;
        *lock(&self.config) = config;
        Ok(self.view())
    }

    fn save(&self, c: &Config) -> Result<(), EngineError> {
        let Some(path) = lock(&self.path).clone() else { return Ok(()) };
        let text = serde_json::to_string_pretty(c).map_err(|e| EngineError::Internal(e.to_string()))?;
        write_private(&path, text.as_bytes())
    }

    /// The feed after `after` (oldest first, at most `limit`), and the newest sequence number.
    pub fn feed(&self, after: u64, limit: usize) -> (Vec<NotifyEvent>, u64) {
        let f = lock(&self.feed);
        let events: Vec<NotifyEvent> = f.iter().filter(|e| e.seq > after).take(limit).cloned().collect();
        (events, self.seq.load(Ordering::SeqCst))
    }

    /// [`Notifier::feed`], waiting up to `wait` for a new event when there is none yet (a long poll).
    pub async fn feed_wait(&self, after: u64, limit: usize, wait: Duration) -> (Vec<NotifyEvent>, u64) {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let arrived = self.arrived.notified();
            let (events, last) = self.feed(after, limit);
            if !events.is_empty() || wait.is_zero() {
                return (events, last);
            }
            if tokio::time::timeout_at(deadline, arrived).await.is_err() {
                return self.feed(after, limit);
            }
        }
    }

    /// Record an event in the feed and send it to every enabled target that takes it.
    pub fn emit(&self, st: &AppState, kind: Kind, op: Operation, message: &str, run_id: Option<&str>) -> NotifyEvent {
        self.emit_detail(st, kind, op, message, run_id, None)
    }

    pub fn emit_detail(
        &self,
        st: &AppState,
        kind: Kind,
        op: Operation,
        message: &str,
        run_id: Option<&str>,
        detail: Option<String>,
    ) -> NotifyEvent {
        let device = st.active_device().id;
        let event = NotifyEvent {
            seq: self.seq.fetch_add(1, Ordering::SeqCst) + 1,
            event: kind,
            operation: op,
            title: format!("OpenEtch: {}", kind.title()),
            message: message.to_owned(),
            time_unix_s: now_unix(),
            count: st.job.count(&device),
            stop_at: st.job.stop_at(),
            device,
            project: st.project_name(),
            run_id: run_id.map(str::to_owned),
            detail,
            source: "openetch".into(),
        };
        {
            let mut f = lock(&self.feed);
            f.push_back(event.clone());
            while f.len() > FEED_LEN {
                f.pop_front();
            }
        }
        self.arrived.notify_waiters();
        tracing::info!(event = kind.name(), message, "production event");
        let targets: Vec<Target> =
            self.config().targets.into_iter().filter(|t| t.enabled && t.events.contains(&kind)).collect();
        for target in targets {
            self.queue(Delivery { target, event: event.clone() });
        }
        event
    }

    fn queue(&self, d: Delivery) {
        let tx = lock(&self.sender).clone();
        match tx {
            Some(tx) => {
                if tx.try_send(d).is_err() {
                    tracing::warn!("notification queue full or closed; an event was not sent");
                }
            }
            None => tracing::debug!(target = %d.target.id, "notifications are not running; not sent"),
        }
    }

    fn note_result(&self, id: &str, ok: bool, text: String) {
        lock(&self.results).insert(id.to_owned(), (now_unix(), ok, text));
    }
}

fn new_id(taken: &[Target], i: usize) -> String {
    let mut n = i + 1;
    loop {
        let id = format!("target-{n}");
        if !taken.iter().any(|t| t.id == id) {
            return id;
        }
        n += 1;
    }
}

fn view_of(t: &Target) -> TargetView {
    TargetView {
        id: t.id.clone(),
        name: t.name.clone(),
        kind: t.kind,
        url: t.url.clone(),
        topic: t.topic.clone(),
        token_set: t.token.is_some(),
        events: t.events.clone(),
        enabled: t.enabled,
    }
}

/// An `http(s)` URL without whitespace, quotes, backslashes or control characters.
pub(crate) fn check_url(url: &str) -> Result<(), EngineError> {
    let lower = url.to_ascii_lowercase();
    let scheme_ok = lower.starts_with("http://") || lower.starts_with("https://");
    let rest_ok = url.len() > 8 && url.len() <= 500 && url.chars().all(|c| c.is_ascii_graphic() && c != '"' && c != '\\');
    if !(scheme_ok && rest_ok) {
        return Err(EngineError::BadRequest(format!(
            "URL \"{url}\" must be an http:// or https:// address (no spaces or quotes, at most 500 characters)"
        )));
    }
    Ok(())
}

fn check_token(token: &str) -> Result<(), EngineError> {
    if token.len() > 1024 || !token.chars().all(|c| c.is_ascii_graphic() && c != '"' && c != '\\') {
        return Err(EngineError::BadRequest("the token must be printable ASCII without spaces or quotes".into()));
    }
    Ok(())
}

fn check_target(t: &Target) -> Result<(), EngineError> {
    if t.id.is_empty() || t.id.len() > 40 || !t.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(EngineError::BadRequest(format!("target id \"{}\" must be 1-40 letters, digits, - or _", t.id)));
    }
    let n = t.name.chars().count();
    if n == 0 || n > 60 || t.name.chars().any(char::is_control) {
        return Err(EngineError::BadRequest(format!("target name \"{}\" must be 1-60 printable characters", t.name)));
    }
    check_url(&t.url)?;
    if let Some(tok) = &t.token {
        check_token(tok)?;
    }
    match t.kind {
        TargetKind::Ntfy => {
            let topic = t.topic.as_deref().unwrap_or_default();
            if topic.is_empty() || topic.len() > 64 || !topic.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                return Err(EngineError::BadRequest("an ntfy target needs a topic of 1-64 letters, digits, - or _".into()));
            }
        }
        TargetKind::HomeAssistant if t.token.is_none() => {
            return Err(EngineError::BadRequest("a Home Assistant target needs a long-lived access token".into()));
        }
        TargetKind::Webhook | TargetKind::HomeAssistant => {}
    }
    Ok(())
}

/// Write `bytes` to `path` through a temporary file, private to the user on Unix (`0600`).
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<(), EngineError> {
    let dir = path.parent().ok_or_else(|| EngineError::Internal(format!("{} has no directory", path.display())))?;
    std::fs::create_dir_all(dir).map_err(|e| EngineError::Internal(format!("cannot create {}: {e}", dir.display())))?;
    let tmp = dir.join(format!(".{}.{}.tmp", path.file_name().and_then(|n| n.to_str()).unwrap_or("file"), std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let r = (|| {
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    r.map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        EngineError::Internal(format!("cannot write {}: {e}", path.display()))
    })
}

// ---- requests ------------------------------------------------------------------------------------------------------

/// One HTTP request for curl: POST `body` (JSON) to `url`, with an optional bearer token.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Request {
    pub url: String,
    pub bearer: Option<String>,
    pub body: String,
}

/// A string as a quoted value of a curl config file.
fn quote(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '"' => o.push_str("\\\""),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if c.is_control() => {}
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// The curl config (read from standard input with `--config -`) for `r`: POST, JSON, only http(s), no redirects,
/// [`SEND_TIMEOUT_S`], the status code on the last line of the output.
pub(crate) fn curl_config(r: &Request) -> String {
    let mut c = String::new();
    c.push_str(&format!("url = {}\n", quote(&r.url)));
    c.push_str("request = \"POST\"\n");
    c.push_str("proto = \"=http,https\"\n");
    c.push_str(&format!("max-time = \"{SEND_TIMEOUT_S}\"\n"));
    c.push_str("silent\nshow-error\n");
    c.push_str("header = \"Content-Type: application/json\"\n");
    c.push_str("header = \"User-Agent: OpenEtch\"\n");
    if let Some(t) = &r.bearer {
        c.push_str(&format!("header = {}\n", quote(&format!("Authorization: Bearer {t}"))));
    }
    c.push_str(&format!("data-binary = {}\n", quote(&r.body)));
    c.push_str("write-out = \"\\n%{http_code}\"\n");
    c
}

/// The request `target` gets for `event`.
pub(crate) fn request_for(target: &Target, event: &NotifyEvent) -> Request {
    match target.kind {
        TargetKind::Ntfy => {
            let (priority, tag) = event.event.ntfy();
            let body = json!({
                "topic": target.topic.clone().unwrap_or_default(),
                "title": event.title,
                "message": event.message,
                "tags": [tag, format!("openetch_{}", event.operation_name())],
                "priority": priority,
            });
            Request { url: target.url.clone(), bearer: target.token.clone(), body: body.to_string() }
        }
        TargetKind::Webhook => Request {
            url: target.url.clone(),
            bearer: target.token.clone(),
            body: serde_json::to_string(event).unwrap_or_default(),
        },
        TargetKind::HomeAssistant => Request {
            url: format!("{}/api/events/openetch_event", target.url),
            bearer: target.token.clone(),
            body: serde_json::to_string(event).unwrap_or_default(),
        },
    }
}

impl NotifyEvent {
    fn operation_name(&self) -> &'static str {
        match self.operation {
            Operation::Frame => "frame",
            Operation::Mark => "mark",
            Operation::None => "none",
        }
    }
}

/// Send `r` with curl (blocking). `Ok(status)` for a 2xx answer, else what went wrong.
pub(crate) fn post(r: &Request) -> Result<u16, String> {
    let mut child = Command::new("curl")
        .args(["--config", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run curl: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(curl_config(r).as_bytes()).map_err(|e| format!("curl: {e}"))?;
    }
    let out = child.wait_with_output().map_err(|e| format!("curl: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let code = stdout.lines().last().and_then(|l| l.trim().parse::<u16>().ok()).unwrap_or(0);
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().chars().take(200).collect::<String>();
        return Err(if err.is_empty() { format!("curl failed ({})", out.status) } else { err });
    }
    if (200..300).contains(&code) { Ok(code) } else { Err(format!("HTTP {code}")) }
}

/// Send a test event to `target` now (the Settings Test button). Blocking.
pub fn test_target(st: &AppState, target: &Target) -> Result<u16, String> {
    let device = st.active_device().id;
    let event = NotifyEvent {
        seq: 0,
        event: Kind::Test,
        operation: Operation::None,
        title: format!("OpenEtch: {}", Kind::Test.title()),
        message: format!("Notifications from OpenEtch reach this target ({}).", target.name),
        time_unix_s: now_unix(),
        count: st.job.count(&device),
        stop_at: st.job.stop_at(),
        device,
        project: st.project_name(),
        run_id: None,
        detail: None,
        source: "openetch".into(),
    };
    let r = post(&request_for(target, &event));
    st.production.notify.note_result(&target.id, r.is_ok(), match &r {
        Ok(c) => format!("HTTP {c}"),
        Err(e) => e.clone(),
    });
    r
}

/// The stored target `id` (with its token), for the Test button.
pub fn target(st: &AppState, id: &str) -> Option<Target> {
    st.production.notify.config().targets.into_iter().find(|t| t.id == id)
}

// ---- the tasks -----------------------------------------------------------------------------------------------------

/// Start the sender (one request at a time) and the listener that turns engine events into production events.
pub fn spawn(st: Arc<AppState>) {
    let (tx, rx) = mpsc::channel::<Delivery>(QUEUE);
    *lock(&st.production.notify.sender) = Some(tx);
    tokio::spawn(sender(st.clone(), rx));
    let events = st.subscribe();
    tokio::spawn(listen(st, events));
}

async fn sender(st: Arc<AppState>, mut rx: mpsc::Receiver<Delivery>) {
    while let Some(d) = rx.recv().await {
        let request = request_for(&d.target, &d.event);
        let result = tokio::task::spawn_blocking(move || post(&request)).await.unwrap_or_else(|e| Err(e.to_string()));
        match &result {
            Ok(code) => tracing::debug!(target = %d.target.id, code, event = d.event.event.name(), "notification sent"),
            Err(e) => tracing::warn!(target = %d.target.id, error = %e, event = d.event.event.name(), "notification not sent"),
        }
        let (ok, text) = match result {
            Ok(c) => (true, format!("HTTP {c}")),
            Err(e) => (false, e),
        };
        st.production.notify.note_result(&d.target.id, ok, text);
    }
}

async fn listen(st: Arc<AppState>, mut events: broadcast::Receiver<Event>) {
    loop {
        match events.recv().await {
            Ok(ev) => on_event(&st, ev),
            Err(broadcast::error::RecvError::Lagged(_)) => {}
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

fn summary_of(st: &AppState, run_id: &str) -> String {
    st.runs().get(run_id).map(|r| r.summary.clone()).unwrap_or_default()
}

/// Map one engine event to a production event (see the module docs).
pub(crate) fn on_event(st: &Arc<AppState>, ev: Event) {
    let n = &st.production.notify;
    match ev {
        Event::RunFinished { id, ok, state, error } => {
            n.waiting.fetch_add(1, Ordering::SeqCst);
            let error = error.unwrap_or_default();
            if ok {
                n.emit(st, Kind::JobDone, Operation::Mark, &format!("Job finished: {}", summary_of(st, &id)), Some(&id));
            } else if error.contains("door") {
                n.emit(st, Kind::Door, Operation::Mark, &format!("The run stopped: {error}"), Some(&id));
            } else if state == RunState::Failed {
                n.emit(st, Kind::Error, Operation::Mark, &format!("The run failed: {error}"), Some(&id));
            }
        }
        Event::Machine { event, detail } => match (event.as_str(), detail) {
            ("foot_cycle", Some(phase)) if phase == "framing" => schedule_waiting(st, Operation::Frame),
            ("foot_cycle", _) | ("run_started", _) | ("stopped", _) => {
                n.waiting.fetch_add(1, Ordering::SeqCst);
            }
            ("waiting_for_start_input", _) => schedule_waiting(st, Operation::Mark),
            ("foot_cycle_notice", Some(text)) if text.contains("door") => {
                n.emit(st, Kind::Door, Operation::Frame, &text, None);
            }
            ("error", Some(text)) => {
                let op = if st.machine.is_framing() { Operation::Frame } else { Operation::Mark };
                n.emit(st, Kind::Error, op, &text, None);
            }
            _ => {}
        },
        _ => {}
    }
}

/// What a wait for the foot switch is: the cycle's frame (its start) or a run's start input (run and iteration).
fn waiting_key(st: &AppState, op: Operation) -> Option<String> {
    match op {
        Operation::Frame => {
            let c = crate::cycle::status_json(st);
            (c["phase"] == "framing").then(|| format!("frame:{}", c["since_unix_s"]))
        }
        _ => {
            let j = st.job_status();
            (j["state"] == "waiting_for_start_input").then(|| format!("run:{}:{}", j["run_id"], j["iteration"]))
        }
    }
}

/// Send "waiting for the foot switch" if the wait that just began is still the same after `waiting_delay_s`.
fn schedule_waiting(st: &Arc<AppState>, op: Operation) {
    let n = &st.production.notify;
    let generation = n.waiting.fetch_add(1, Ordering::SeqCst) + 1;
    let delay = Duration::from_secs(n.config().waiting_delay_s);
    let st = st.clone();
    tokio::spawn(async move {
        // The state the wait is in once it has settled (the event may come just before the status shows it).
        tokio::time::sleep(Duration::from_millis(200)).await;
        let Some(key) = waiting_key(&st, op) else { return };
        tokio::time::sleep(delay.saturating_sub(Duration::from_millis(200))).await;
        let n = &st.production.notify;
        if n.waiting.load(Ordering::SeqCst) != generation || waiting_key(&st, op).as_deref() != Some(key.as_str()) {
            return;
        }
        let msg = match op {
            Operation::Frame => "The job is framed: press the foot switch to burn the next piece.",
            _ => "A run waits for the foot switch to start its next iteration.",
        };
        let run = st.job.active_run();
        n.emit(&st, Kind::WaitingForFootSwitch, op, msg, run.as_deref());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event() -> NotifyEvent {
        NotifyEvent {
            seq: 3,
            event: Kind::JobDone,
            operation: Operation::Mark,
            title: "OpenEtch: Job finished".into(),
            message: "Job finished: 6 \"discs\"\nnext".into(),
            time_unix_s: 1,
            device: "b6".into(),
            project: Some("coins".into()),
            run_id: Some("run-1".into()),
            count: 4,
            stop_at: Some(6),
            detail: None,
            source: "openetch".into(),
        }
    }

    fn target(kind: TargetKind) -> Target {
        Target {
            id: "t1".into(),
            name: "Phone".into(),
            kind,
            url: "https://ntfy.example".into(),
            topic: Some("laser".into()),
            token: Some("tk_secret".into()),
            events: vec![Kind::JobDone],
            enabled: true,
        }
    }

    #[test]
    fn requests_per_kind() {
        let r = request_for(&target(TargetKind::Ntfy), &event());
        let body: Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!((body["topic"].as_str(), body["priority"].as_u64()), (Some("laser"), Some(3)));
        assert_eq!(body["tags"][1], "openetch_mark");
        assert_eq!(r.url, "https://ntfy.example");
        let r = request_for(&target(TargetKind::HomeAssistant), &event());
        assert_eq!(r.url, "https://ntfy.example/api/events/openetch_event");
        let body: Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!((body["event"].as_str(), body["operation"].as_str()), (Some("job_done"), Some("mark")));
        let r = request_for(&target(TargetKind::Webhook), &event());
        assert_eq!(serde_json::from_str::<NotifyEvent>(&r.body).unwrap(), event());
    }

    #[test]
    fn curl_config_quotes_everything_and_keeps_the_token_off_the_command_line() {
        let r = request_for(&target(TargetKind::Webhook), &event());
        let c = curl_config(&r);
        assert!(c.contains("header = \"Authorization: Bearer tk_secret\"\n"), "{c}");
        assert!(c.contains("proto = \"=http,https\""));
        // The body's quotes and backslashes are escaped, and no line of the config can be ended early.
        let data = c.lines().find(|l| l.starts_with("data-binary = ")).unwrap();
        assert!(data.ends_with('"') && data.contains(r#"\\\"discs\\\""#) && data.contains(r"\\n"), "{data}");
        assert_eq!(c.lines().filter(|l| l.starts_with("url = ")).count(), 1);
        assert_eq!(quote("a\"b\\c\nd\u{7}"), "\"a\\\"b\\\\c\\nd\"");
    }

    #[test]
    fn targets_are_checked() {
        let ok = target(TargetKind::Ntfy);
        assert!(check_target(&ok).is_ok());
        for bad in [
            Target { url: "file:///etc/passwd".into(), ..ok.clone() },
            Target { url: "https://x\" -o /tmp/y".into(), ..ok.clone() },
            Target { url: "https://a b".into(), ..ok.clone() },
            Target { topic: None, ..ok.clone() },
            Target { topic: Some("a/b".into()), ..ok.clone() },
            Target { token: Some("a b".into()), ..ok.clone() },
            Target { name: String::new(), ..ok.clone() },
            Target { kind: TargetKind::HomeAssistant, token: None, ..ok.clone() },
        ] {
            assert!(check_target(&bad).is_err(), "{bad:?}");
        }
        assert!(check_target(&Target { kind: TargetKind::Webhook, topic: None, token: None, ..ok }).is_ok());
    }

    #[test]
    fn replace_keeps_tokens_and_never_shows_them() {
        let n = Notifier::default();
        let input = |token: Option<&str>| ConfigInput {
            waiting_delay_s: Some(5),
            targets: vec![TargetInput {
                id: Some("t1".into()),
                name: "Phone".into(),
                kind: TargetKind::Ntfy,
                url: "https://ntfy.sh/".into(),
                topic: Some("laser".into()),
                token: token.map(str::to_owned),
                events: vec![Kind::Door, Kind::JobDone, Kind::Test, Kind::Door],
                enabled: true,
                token_set: None,
            }],
            origin: None,
        };
        let v = n.replace(input(Some("tk_1"))).unwrap();
        assert_eq!(v["targets"][0]["token_set"], true);
        assert!(!v.to_string().contains("tk_1"), "the token never goes back to clients");
        assert_eq!(v["targets"][0]["url"], "https://ntfy.sh");
        assert_eq!(v["targets"][0]["events"], json!(["job_done", "door"]));
        n.replace(input(None)).unwrap();
        assert_eq!(n.config().targets[0].token.as_deref(), Some("tk_1"), "kept when not sent");
        n.replace(input(Some(""))).unwrap();
        assert_eq!(n.config().targets[0].token, None, "cleared with an empty token");
    }

    #[tokio::test]
    async fn the_feed_is_read_after_a_cursor_and_waits_for_news() {
        let n = Arc::new(Notifier::default());
        for i in 0..3 {
            let e = NotifyEvent { seq: i + 1, ..event() };
            n.feed.lock().unwrap().push_back(e);
            n.seq.store(i + 1, Ordering::SeqCst);
        }
        let (events, last) = n.feed(1, 10);
        assert_eq!((events.len(), last), (2, 3));
        let waiter = {
            let n = n.clone();
            tokio::spawn(async move { n.feed_wait(3, 10, Duration::from_secs(5)).await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        n.feed.lock().unwrap().push_back(NotifyEvent { seq: 4, ..event() });
        n.seq.store(4, Ordering::SeqCst);
        n.arrived.notify_waiters();
        let (events, last) = waiter.await.unwrap();
        assert_eq!((events.len(), last), (1, 4));
        let (none, _) = n.feed_wait(4, 10, Duration::from_millis(20)).await;
        assert!(none.is_empty());
    }
}
