//! The operator PIN that approving a run requires, and the optional physical confirmation input.
//!
//! **Approval modes** ([`ApprovalMode`], `--approval pin|click`, fixed at startup): with `pin` (the default) every
//! approval needs the PIN below. With `click` there is no PIN: a run is approved only by the Start button of the
//! OpenEtch web app (a browser request carrying `Sec-Fetch-Site: same-origin` and an accepted `Origin`, which page
//! scripts cannot set) or by a press of the foot switch (the profile's Start Marking input). Neither path is open to
//! MCP. A non-browser HTTP client could imitate those headers (on loopback it needs no token; off loopback it needs
//! the access token), and an agent driving the browser can click Start; the foot switch is the path backed by
//! hardware, and the token stays the network guard. Everything else (checklist, Require Framing, the
//! door interlock, Stop) applies in both modes.
//! A person sets the PIN with `openetch set-pin` (interactive; refused unless stdin is a terminal; changing an
//! existing PIN asks for the current one first; `set-pin --reset` deletes it after a typed confirmation). It is
//! stored as a salted SHA-256 hash in `operator-pin` in the engine's config directory (on Unix the directory is
//! `0700` and the file `0600`).
//!
//! The engine loads the hash **once**, when it starts ([`PinStore::new`]). Every approval attempt (and every
//! `GET /api/machine`) checks that the file is still exactly what was loaded (size, modification time and content
//! hash); once it has changed, approvals are refused with `pin_changed` until the engine is restarted. So a process
//! that rewrites the file cannot approve with its own PIN without the operator noticing a restart is needed.
//!
//! Every [`MAX_FAILURES`] wrong PINs in a row lock approvals; each lockout is twice the previous one ([`LOCKOUT`],
//! then 120 s, 240 s, ... up to [`MAX_LOCKOUT`]); only the right PIN resets the escalation.
//!
//! **Simulator test PIN** (`--sim-pin`, [`PinStore::simulator`]): for testing the approval flow without a laser, the
//! engine can take a PIN on its command line instead. It is accepted only when the engine drives the simulator
//! ([`check_sim_pin_allowed`]: never with `--usb`, and in release builds only with [`ALLOW_SIM_PIN_ENV`]` = 1`), is
//! kept in memory as a salted hash and never written to disk, and `approve` refuses it again on every attempt when
//! the machine is not the simulator. `GET /api/machine` reports `approval.sim_pin: true`.
//!
//! The MCP server never sees, asks for or handles a PIN. This keeps an agent that only has MCP from approving; it
//! does **not** stop a local process running with the user's OS privileges (it could read the hash file and guess
//! offline, restart the engine, or open the USB device itself). The hardware E-stop/key switch is the real
//! guarantee; the physical confirmation input ([`PhysicalConfirm`]) only protects against a guessed or stolen PIN
//! used through the engine's API.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::EngineError;

/// File name of the PIN hash inside the config directory.
pub const PIN_FILE: &str = "operator-pin";
/// Shortest and longest PIN, in digits.
pub const PIN_DIGITS: std::ops::RangeInclusive<usize> = 6..=12;
/// Wrong PINs in a row before approvals lock.
pub const MAX_FAILURES: u32 = 5;
/// The first lockout after [`MAX_FAILURES`] wrong PINs; each further lockout doubles it.
pub const LOCKOUT: Duration = Duration::from_secs(60);
/// The longest lockout.
pub const MAX_LOCKOUT: Duration = Duration::from_secs(3600);
/// What `openetch set-pin --reset` asks the person to type.
pub const RESET_PHRASE: &str = "delete the operator PIN";
/// How long an approved run waits for the physical confirmation input.
pub const CONFIRM_TIMEOUT: Duration = Duration::from_secs(30);
/// Pause between `ReadPort` polls while waiting for the physical input.
pub const CONFIRM_POLL: Duration = Duration::from_millis(50);
/// Consecutive reads the input must be seen pressed (galvoplotter's `input_passes_required` is 3).
pub const CONFIRM_READS: u32 = 3;

/// Environment variable a release build needs (set to `1`) to accept `--sim-pin`.
pub const ALLOW_SIM_PIN_ENV: &str = "OPENETCH_ALLOW_SIM_PIN";

/// Domain prefix hashed before the salt, so the hash is useless for anything but this file.
const HASH_DOMAIN: &[u8] = b"openetch-operator-pin-v1";
/// File format tag.
const FORMAT: &str = "sha256";

/// Why an approval was refused for PIN reasons. Rendered as 403 `{ok: false, error, code}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinError {
    /// No PIN was set when the engine started (run `openetch set-pin`), or the PIN file was unreadable.
    NotSet,
    /// The PIN file changed (or appeared, or was deleted) while the engine was running: approvals stay refused
    /// until the engine is restarted.
    Changed,
    /// The request carried no PIN.
    Required,
    /// The PIN is wrong.
    Invalid,
    /// Too many wrong PINs; approvals are locked for a while.
    Locked,
}

impl PinError {
    /// The machine-readable `code`.
    pub fn code(self) -> &'static str {
        match self {
            PinError::NotSet => "pin_not_set",
            PinError::Changed => "pin_changed",
            PinError::Required => "pin_required",
            PinError::Invalid => "pin_invalid",
            PinError::Locked => "pin_locked",
        }
    }

    fn message(self) -> String {
        match self {
            PinError::NotSet => "no operator PIN is set: run `openetch set-pin` on the engine's computer, then restart \
                                 the engine"
                .into(),
            PinError::Changed => "the operator PIN file changed while the engine was running; approvals are refused \
                                  until the engine is restarted"
                .into(),
            PinError::Required => "approving a run needs the operator PIN".into(),
            PinError::Invalid => "wrong operator PIN".into(),
            PinError::Locked => "too many wrong PINs: approvals are locked for a while; wait and try again".into(),
        }
    }
}

impl From<PinError> for EngineError {
    fn from(e: PinError) -> Self {
        EngineError::Approval { code: e.code(), message: e.message() }
    }
}

/// The default config directory: the OS config dir + `openetch` (`%APPDATA%\openetch`, `~/.config/openetch`
/// or `$XDG_CONFIG_HOME/openetch`, `~/Library/Application Support/openetch`). `None` when the environment names none.
pub fn default_config_dir() -> Option<PathBuf> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let base = if cfg!(windows) {
        env("APPDATA")
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library").join("Application Support"))
    } else {
        env("XDG_CONFIG_HOME").or_else(|| env("HOME").map(|h| h.join(".config")))
    };
    base.map(|b| b.join("openetch"))
}

/// Create the config directory (and its parents). On Unix the directory itself is made `0700`, also when it
/// already existed; a failure to do so is an error. On Windows it inherits the parent's ACL (`%APPDATA%` is
/// per-user).
pub fn create_config_dir(dir: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("cannot make {} private (chmod 700): {e}", dir.display()))?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    Ok(())
}

/// Check a PIN's form: 6-12 ASCII digits.
pub fn validate_pin(pin: &str) -> Result<(), String> {
    if !PIN_DIGITS.contains(&pin.len()) || !pin.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("the PIN must be {} to {} digits", PIN_DIGITS.start(), PIN_DIGITS.end()));
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| s.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok())).collect()
}

fn hash(salt: &[u8], pin: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(HASH_DOMAIN);
    h.update(salt);
    h.update(pin.as_bytes());
    h.finalize().into()
}

/// Constant-time comparison.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A loaded PIN hash.
#[derive(Debug, Clone)]
struct PinHash {
    salt: Vec<u8>,
    digest: Vec<u8>,
}

impl PinHash {
    /// True when `pin` (already trimmed) has the right form and matches.
    fn matches(&self, pin: &str) -> bool {
        validate_pin(pin).is_ok() && same(&hash(&self.salt, pin), &self.digest)
    }
}

/// Write `line` to `path` through a fresh temp file in the same directory, then rename it into place. The temp file
/// is created with `create_new` (so a planted file or symlink is never followed) and, on Unix, mode `0600`, checked
/// again with an explicit chmod whose failure is reported.
fn write_private(dir: &Path, path: &Path, line: &str) -> Result<(), String> {
    let tmp = dir.join(format!("{PIN_FILE}.tmp"));
    match std::fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("cannot remove the stale {}: {e}", tmp.display())),
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let written = (|| {
        let mut f = opts.open(&tmp).map_err(|e| format!("cannot create {}: {e}", tmp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|e| format!("cannot make {} private (chmod 600): {e}", tmp.display()))?;
        }
        f.write_all(line.as_bytes()).and_then(|_| f.sync_all()).map_err(|e| format!("cannot write {}: {e}", tmp.display()))
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("cannot write {}: {e}", path.display())
    })
}

/// Hash `pin` with a fresh salt and write it to `dir/operator-pin` (creating `dir`), replacing any old PIN. This is
/// the storage step only; `openetch set-pin` goes through [`set_pin_dialogue`], which asks for the current PIN.
pub fn set_pin(dir: &Path, pin: &str) -> Result<PathBuf, String> {
    validate_pin(pin)?;
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|e| format!("no random source for the salt: {e}"))?;
    let line = format!("{FORMAT}${}${}\n", hex(&salt), hex(&hash(&salt, pin)));
    create_config_dir(dir)?;
    let path = dir.join(PIN_FILE);
    write_private(dir, &path, &line)?;
    Ok(path)
}

/// Ask `prompt` on `out` and read one trimmed line from `input`.
fn ask(input: &mut impl BufRead, out: &mut impl Write, prompt: &str) -> Result<String, String> {
    write!(out, "{prompt}").and_then(|_| out.flush()).map_err(|e| e.to_string())?;
    let mut line = String::new();
    if input.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
        return Err("no input".into());
    }
    Ok(line.trim().to_owned())
}

/// `openetch set-pin`'s dialogue. If a PIN is already set, the current PIN is asked for first (a PIN file that
/// exists but cannot be read is refused: use `set-pin --reset`). Then the new PIN is read twice, checked and stored.
/// The caller makes sure `input` is a terminal. std has no portable way to turn echo off, so PINs are visible while
/// typed.
pub fn set_pin_dialogue(input: &mut impl BufRead, out: &mut impl Write, dir: &Path) -> Result<PathBuf, String> {
    let path = dir.join(PIN_FILE);
    match read_pin_file(&path) {
        Some(current) => {
            let given = ask(input, out, "Current operator PIN: ")?;
            if !current.matches(&given) {
                return Err("wrong current PIN; nothing was changed (forgot it? run `openetch set-pin --reset`)".into());
            }
        }
        None if path.exists() => {
            return Err(format!(
                "{} exists but is not a valid PIN file; run `openetch set-pin --reset` first",
                path.display()
            ));
        }
        None => {}
    }
    let first = ask(
        input,
        out,
        &format!(
            "New operator PIN ({}-{} digits; it is shown as you type, so make sure nobody is watching): ",
            PIN_DIGITS.start(),
            PIN_DIGITS.end()
        ),
    )?;
    validate_pin(&first)?;
    let second = ask(input, out, "Repeat the PIN: ")?;
    if !same(first.as_bytes(), second.as_bytes()) {
        return Err("the two PINs differ; nothing was changed".into());
    }
    set_pin(dir, &first)
}

/// What `set-pin --reset` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResetOutcome {
    /// The PIN file was deleted.
    Deleted(PathBuf),
    /// There was no PIN file.
    NothingToDelete(PathBuf),
}

/// `openetch set-pin --reset`: delete the PIN file once the person types [`RESET_PHRASE`]. Approvals stay disabled
/// until a new PIN is set and the engine is restarted (a running engine sees the file change: `pin_changed`).
pub fn reset_pin_dialogue(input: &mut impl BufRead, out: &mut impl Write, dir: &Path) -> Result<ResetOutcome, String> {
    let path = dir.join(PIN_FILE);
    if !path.exists() {
        return Ok(ResetOutcome::NothingToDelete(path));
    }
    writeln!(
        out,
        "This deletes the operator PIN ({}). Runs cannot be approved until a new PIN is set with `openetch set-pin` \
         and the engine is restarted.",
        path.display()
    )
    .map_err(|e| e.to_string())?;
    let typed = ask(input, out, &format!("Type \"{RESET_PHRASE}\" to continue: "))?;
    if typed != RESET_PHRASE {
        return Err("confirmation phrase not typed; nothing was deleted".into());
    }
    std::fs::remove_file(&path).map_err(|e| format!("cannot delete {}: {e}", path.display()))?;
    Ok(ResetOutcome::Deleted(path))
}

/// Read `dir/operator-pin`, or `None` if it is missing or malformed.
fn read_pin_file(path: &Path) -> Option<PinHash> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut parts = text.trim().split('$');
    let (fmt, salt, digest) = (parts.next()?, parts.next()?, parts.next()?);
    if fmt != FORMAT || parts.next().is_some() {
        return None;
    }
    let (salt, digest) = (unhex(salt)?, unhex(digest)?);
    (!salt.is_empty() && digest.len() == 32).then_some(PinHash { salt, digest })
}

/// What the PIN file looked like: compared on every approval attempt with the state at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FileState {
    Missing,
    /// It exists but could not be read (the error kind).
    Unreadable(std::io::ErrorKind),
    Present { len: u64, modified: Option<SystemTime>, content: [u8; 32] },
}

impl FileState {
    fn of(path: &Path) -> Self {
        let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        match std::fs::read(path) {
            Ok(bytes) => {
                FileState::Present { len: bytes.len() as u64, modified, content: Sha256::digest(&bytes).into() }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => FileState::Missing,
            Err(e) => FileState::Unreadable(e.kind()),
        }
    }
}

#[derive(Debug, Default)]
struct Attempts {
    /// Wrong PINs since the last lockout or success.
    failures: u32,
    /// Lockouts since the last right PIN: the next lockout lasts `lockout * 2^lockouts`.
    lockouts: u32,
    /// The current lockout's end, as an `Instant` (for the check) and as wall-clock time (for clients).
    locked_until: Option<(Instant, SystemTime)>,
}

fn unix_s(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_secs() + u64::from(d.subsec_nanos() > 0)).unwrap_or(0)
}

/// Checks approval PINs against the hash loaded at startup, with the escalating wrong-PIN lockout and the
/// file-change check (see the module docs).
#[derive(Debug)]
pub struct PinStore {
    path: PathBuf,
    loaded: Option<PinHash>,
    at_load: FileState,
    /// Set (for good) once the file is seen to differ from `at_load`.
    changed: AtomicBool,
    /// The PIN is the simulator test PIN (`--sim-pin`), held in memory only; the PIN file is not used.
    sim: bool,
    lockout: Duration,
    max_lockout: Duration,
    attempts: Mutex<Attempts>,
}

impl PinStore {
    /// Load the PIN of config directory `dir` (once; see the module docs).
    pub fn new(dir: &Path) -> Self {
        Self::with_lockout(dir, LOCKOUT, MAX_LOCKOUT)
    }

    /// With a different first and longest lockout (tests).
    pub fn with_lockout(dir: &Path, lockout: Duration, max_lockout: Duration) -> Self {
        let path = dir.join(PIN_FILE);
        let at_load = FileState::of(&path);
        let loaded = read_pin_file(&path);
        PinStore {
            path,
            loaded,
            at_load,
            changed: AtomicBool::new(false),
            sim: false,
            lockout,
            max_lockout,
            attempts: Mutex::new(Attempts::default()),
        }
    }

    /// The simulator test PIN (`--sim-pin`): `pin` (6-12 digits) becomes the operator PIN for this engine run. It is
    /// held in memory as a salted hash and never written to disk; the PIN file in `dir` is neither read nor written.
    /// The caller checks [`check_sim_pin_allowed`] first, and `approve` refuses it unless the machine is the
    /// simulator.
    pub fn simulator(dir: &Path, pin: &str) -> Result<Self, String> {
        validate_pin(pin).map_err(|e| format!("--sim-pin: {e}"))?;
        let mut salt = vec![0u8; 16];
        getrandom::fill(&mut salt).map_err(|e| format!("no random source for the salt: {e}"))?;
        let digest = hash(&salt, pin).to_vec();
        Ok(PinStore {
            path: dir.join(PIN_FILE),
            loaded: Some(PinHash { salt, digest }),
            at_load: FileState::Missing,
            changed: AtomicBool::new(false),
            sim: true,
            lockout: LOCKOUT,
            max_lockout: MAX_LOCKOUT,
            attempts: Mutex::new(Attempts::default()),
        })
    }

    /// True when the PIN is the simulator test PIN (`--sim-pin`).
    pub fn is_sim(&self) -> bool {
        self.sim
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// True when a valid PIN was loaded at startup.
    pub fn is_set(&self) -> bool {
        self.loaded.is_some()
    }

    /// True once the PIN file differs from what was loaded at startup (it stays true until the engine restarts).
    pub fn has_changed(&self) -> bool {
        if self.sim {
            // The simulator test PIN does not come from the file.
            return false;
        }
        if self.changed.load(Ordering::SeqCst) {
            return true;
        }
        if FileState::of(&self.path) != self.at_load {
            if !self.changed.swap(true, Ordering::SeqCst) {
                tracing::warn!(path = %self.path.display(), "the operator PIN file changed; approvals are refused until the engine restarts");
            }
            return true;
        }
        false
    }

    fn attempts(&self) -> std::sync::MutexGuard<'_, Attempts> {
        self.attempts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// When the current lockout ends (Unix seconds, rounded up), or `None` when approvals are not locked.
    pub fn locked_until_unix_s(&self) -> Option<u64> {
        match self.attempts().locked_until {
            Some((t, wall)) if Instant::now() < t => Some(unix_s(wall)),
            _ => None,
        }
    }

    /// How long the `n`th lockout (0-based) lasts: the first lockout doubled `n` times, at most the longest.
    fn lockout_for(&self, n: u32) -> Duration {
        let factor = 1u32.checked_shl(n).unwrap_or(u32::MAX);
        self.lockout.checked_mul(factor).map_or(self.max_lockout, |d| d.min(self.max_lockout))
    }

    /// Check `pin`. Order: file changed, not set, locked, missing, wrong. A wrong PIN counts toward the lockout;
    /// every [`MAX_FAILURES`]th locks approvals and is answered with [`PinError::Locked`]. A right PIN resets the
    /// count and the escalation.
    pub fn verify(&self, pin: Option<&str>) -> Result<(), PinError> {
        self.verify_reporting(pin).map_err(|(e, _)| e)
    }

    /// [`PinStore::verify`], also returning when a lockout ends (Unix seconds) if this attempt started it.
    pub fn verify_reporting(&self, pin: Option<&str>) -> Result<(), (PinError, Option<u64>)> {
        if self.has_changed() {
            return Err((PinError::Changed, None));
        }
        let Some(loaded) = &self.loaded else { return Err((PinError::NotSet, None)) };
        let mut a = self.attempts();
        if let Some((t, _)) = a.locked_until {
            if Instant::now() < t {
                return Err((PinError::Locked, None));
            }
            a.locked_until = None;
        }
        let pin = pin.map(str::trim).filter(|p| !p.is_empty()).ok_or((PinError::Required, None))?;
        if loaded.matches(pin) {
            *a = Attempts::default();
            return Ok(());
        }
        a.failures += 1;
        if a.failures >= MAX_FAILURES {
            let d = self.lockout_for(a.lockouts);
            a.lockouts = a.lockouts.saturating_add(1);
            a.failures = 0;
            let wall = SystemTime::now() + d;
            a.locked_until = Some((Instant::now() + d, wall));
            tracing::warn!(lockouts = a.lockouts, lockout_s = d.as_secs(), "approvals locked after repeated wrong operator PINs");
            return Err((PinError::Locked, Some(unix_s(wall))));
        }
        Err((PinError::Invalid, None))
    }

    /// The API error for `e`; a lockout says how long it still lasts.
    pub fn error(&self, e: PinError) -> EngineError {
        let mut err = EngineError::from(e);
        if e == PinError::Locked
            && let Some(until) = self.locked_until_unix_s()
            && let EngineError::Approval { message, .. } = &mut err
        {
            let left = until.saturating_sub(unix_s(SystemTime::now()));
            *message = format!(
                "too many wrong PINs: approvals are locked for another {left} s (until {until}, Unix time); each \
                 further lockout is twice as long"
            );
        }
        err
    }
}

/// Optional physical confirmation: after PIN approval the run waits until input `input_bit` (`ReadPort`, reply
/// word 1) is seen released and then pressed for [`CONFIRM_READS`] reads in a row, for at most `timeout`. Requiring
/// the release first means a stuck or inverted input can never confirm on its own. It is enforced by the engine, so
/// it protects against a guessed or stolen PIN used through the API, not against a process that restarts the
/// engine or opens the USB device itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysicalConfirm {
    /// Input bit 0-15. The B6's `markcfg` has `STARTMARKPORT=14`, which suggests a foot pedal on 14 (unverified).
    pub input_bit: u8,
    pub timeout: Duration,
    pub poll: Duration,
}

impl PhysicalConfirm {
    /// The confirmation on `input_bit` with the default timeout and poll interval.
    pub fn new(input_bit: u8) -> Self {
        PhysicalConfirm { input_bit, timeout: CONFIRM_TIMEOUT, poll: CONFIRM_POLL }
    }
}

/// How runs are approved (`--approval`). Fixed when the engine starts ([`crate::AppState::set_approval_mode`]); no
/// API, MCP tool or UI setting changes it, so nothing that talks to a running engine can switch the PIN off.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ApprovalMode {
    /// The operator PIN (`openetch set-pin`), typed in the web app's approval dialog. The default.
    #[default]
    Pin,
    /// No PIN: a run is approved only by the Start button of the OpenEtch web app (a same-origin browser request,
    /// see `api::approve_run` and [`crate::security::same_origin_browser_request`]) or by a press of the foot switch
    /// (the profile's Start Marking input, `approve::foot_switch_pressed`). Every other gate stays.
    Click,
}

impl ApprovalMode {
    /// `pin` or `click` (the `--approval` value and `GET /api/machine`'s `approval.mode`).
    pub fn as_str(self) -> &'static str {
        match self {
            ApprovalMode::Pin => "pin",
            ApprovalMode::Click => "click",
        }
    }

    /// Parse an `--approval` value.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim() {
            "pin" => Ok(ApprovalMode::Pin),
            "click" => Ok(ApprovalMode::Click),
            other => Err(format!("--approval must be pin or click (got '{other}')")),
        }
    }
}

/// How runs are approved.
#[derive(Debug)]
pub struct ApprovalConfig {
    pub pin: PinStore,
    pub confirm: Option<PhysicalConfirm>,
}

impl ApprovalConfig {
    /// `GET /api/machine`'s `approval` field.
    pub fn status_json(&self) -> Value {
        json!({
            "pin_set": self.pin.is_set(),
            "pin_changed": self.pin.has_changed(),
            "locked_until_unix_s": self.pin.locked_until_unix_s(),
            "sim_pin": self.pin.is_sim(),
            "physical_confirm": self.confirm.map(|c| json!({
                "input_bit": c.input_bit,
                "timeout_s": c.timeout.as_secs(),
            })),
        })
    }
}

/// Whether `--sim-pin` may be used: only when the engine drives the simulator (`simulated`: no `--usb`, no USB
/// link), and in a release build only when [`ALLOW_SIM_PIN_ENV`] is `1` (`allow_env`: its value).
pub fn check_sim_pin_allowed(simulated: bool, debug_build: bool, allow_env: Option<&str>) -> Result<(), String> {
    if !simulated {
        return Err("--sim-pin is only accepted with the simulator; it is refused with --usb or any real USB link".into());
    }
    if !debug_build && allow_env.map(str::trim) != Some("1") {
        return Err(format!("--sim-pin is refused in release builds unless {ALLOW_SIM_PIN_ENV}=1 is also set"));
    }
    Ok(())
}

/// The error `approve` answers with when the simulator test PIN meets a machine that is not the simulator.
pub fn sim_pin_on_real_link() -> EngineError {
    EngineError::Approval {
        code: "sim_pin_real_link",
        message: "the simulator test PIN (--sim-pin) cannot approve runs on a real laser; restart the engine without \
                  --sim-pin and set an operator PIN with `openetch set-pin`"
            .into(),
    }
}

#[cfg(test)]
pub(crate) fn temp_dir(tag: &str) -> PathBuf {
    use std::sync::atomic::AtomicU64;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::SeqCst);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    std::env::temp_dir().join(format!("openetch-test-{tag}-{}-{n}-{nanos}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_must_be_6_to_12_digits() {
        for ok in ["123456", "000000000000"] {
            assert!(validate_pin(ok).is_ok(), "{ok}");
        }
        for bad in ["1234", "12345", "1234567890123", "12a456", " 123456", "", "１２３４５６"] {
            assert!(validate_pin(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn set_and_verify() {
        let dir = temp_dir("pin");
        let store = PinStore::new(&dir);
        assert!(!store.is_set());
        assert_eq!(store.verify(Some("123456")), Err(PinError::NotSet));
        let path = set_pin(&dir, "432100").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("sha256$") && !text.contains("432100"), "{text}");
        // Loaded once: a PIN set while the engine runs is not used, and approvals stop until a restart.
        assert!(store.has_changed() && !store.is_set());
        assert_eq!(store.verify(Some("432100")), Err(PinError::Changed));

        let store = PinStore::new(&dir);
        assert!(path.ends_with(PIN_FILE) && store.is_set() && !store.has_changed());
        assert_eq!(store.verify(None), Err(PinError::Required));
        assert_eq!(store.verify(Some("  ")), Err(PinError::Required));
        assert_eq!(store.verify(Some("111111")), Err(PinError::Invalid));
        assert_eq!(store.verify(Some("432100")), Ok(()));
        assert!(set_pin(&dir, "1234").is_err(), "4 digits are too short");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_changed_pin_file_refuses_approvals_until_restart() {
        let dir = temp_dir("changed");
        let path = set_pin(&dir, "246800").unwrap();
        let store = PinStore::new(&dir);
        assert_eq!(store.verify(Some("246800")), Ok(()));
        let original = std::fs::read(&path).unwrap();
        // An attacker's new PIN never works in this engine, and neither does the old one any more.
        set_pin(&dir, "999999").unwrap();
        assert_eq!(store.verify(Some("999999")), Err(PinError::Changed));
        assert_eq!(store.verify(Some("246800")), Err(PinError::Changed));
        assert_eq!(store.error(PinError::Changed).body()["code"], "pin_changed");
        // Sticky: even restoring the old content does not re-enable approvals.
        std::fs::write(&path, &original).unwrap();
        assert_eq!(store.verify(Some("246800")), Err(PinError::Changed));
        let fresh = temp_dir("changed2");
        set_pin(&fresh, "246800").unwrap();
        // Deleting the file counts as a change too.
        let other = PinStore::new(&fresh);
        std::fs::remove_file(fresh.join(PIN_FILE)).unwrap();
        assert_eq!(other.verify(Some("246800")), Err(PinError::Changed));
        let status = ApprovalConfig { pin: other, confirm: None }.status_json();
        assert_eq!(status["pin_changed"], true);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&fresh);
    }

    #[test]
    fn wrong_pins_lock_approvals_with_doubling_lockouts() {
        let dir = temp_dir("lock");
        set_pin(&dir, "246800").unwrap();
        let store = PinStore::with_lockout(&dir, Duration::from_millis(100), Duration::from_millis(400));
        assert_eq!(store.locked_until_unix_s(), None);
        let lock_round = |store: &PinStore| {
            for _ in 1..MAX_FAILURES {
                assert_eq!(store.verify(Some("000000")), Err(PinError::Invalid));
            }
            let (e, until) = store.verify_reporting(Some("000000")).unwrap_err();
            assert_eq!(e, PinError::Locked);
            let until = until.expect("this attempt started the lockout");
            assert_eq!(store.locked_until_unix_s(), Some(until));
            // Even the right PIN is refused while locked, and that refusal starts no new lockout.
            assert_eq!(store.verify_reporting(Some("246800")), Err((PinError::Locked, None)));
        };
        let wait_unlocked = |store: &PinStore| {
            let start = Instant::now();
            while store.locked_until_unix_s().is_some() {
                std::thread::sleep(Duration::from_millis(5));
            }
            start.elapsed()
        };
        assert_eq!(
            [0, 1, 2, 3, 10, 40].map(|n| store.lockout_for(n).as_millis()),
            [100, 200, 400, 400, 400, 400],
            "doubles, capped"
        );
        lock_round(&store);
        assert!(wait_unlocked(&store) < Duration::from_millis(190));
        // The second lockout (no right PIN in between) lasts twice as long.
        lock_round(&store);
        let second = wait_unlocked(&store);
        assert!(second > Duration::from_millis(120), "{second:?}");
        assert_eq!(store.attempts().lockouts, 2);
        // The right PIN resets the count and the escalation.
        assert_eq!(store.verify(Some("246800")), Ok(()));
        assert_eq!(store.attempts().lockouts, 0);
        for _ in 1..MAX_FAILURES {
            assert_eq!(store.verify(Some("000000")), Err(PinError::Invalid));
        }
        assert_eq!(store.verify(Some("246800")), Ok(()));
        assert_eq!(store.verify(Some("000000")), Err(PinError::Invalid));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_lockouts_escalate_to_an_hour() {
        let store = PinStore::new(&temp_dir("defaults"));
        let secs: Vec<u64> = (0..8).map(|n| store.lockout_for(n).as_secs()).collect();
        assert_eq!(secs, [60, 120, 240, 480, 960, 1920, 3600, 3600]);
        assert_eq!(store.lockout_for(u32::MAX), MAX_LOCKOUT);
    }

    #[test]
    fn dialogue_reads_the_pin_twice() {
        let dir = temp_dir("dialogue");
        let mut out = Vec::new();
        let err = set_pin_dialogue(&mut "123456\n123457\n".as_bytes(), &mut out, &dir).unwrap_err();
        assert!(err.contains("differ"));
        assert!(!PinStore::new(&dir).is_set());
        assert!(set_pin_dialogue(&mut "1234\n1234\n".as_bytes(), &mut out, &dir).is_err());
        assert!(set_pin_dialogue(&mut "".as_bytes(), &mut out, &dir).is_err());
        set_pin_dialogue(&mut "135790\r\n135790\r\n".as_bytes(), &mut out, &dir).unwrap();
        assert_eq!(PinStore::new(&dir).verify(Some("135790")), Ok(()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn changing_a_pin_needs_the_current_one() {
        let dir = temp_dir("change");
        set_pin(&dir, "135790").unwrap();
        let before = std::fs::read(dir.join(PIN_FILE)).unwrap();
        let mut out = Vec::new();
        let err = set_pin_dialogue(&mut "000000\n246800\n246800\n".as_bytes(), &mut out, &dir).unwrap_err();
        assert!(err.contains("wrong current PIN"), "{err}");
        assert!(set_pin_dialogue(&mut "".as_bytes(), &mut out, &dir).is_err());
        assert_eq!(std::fs::read(dir.join(PIN_FILE)).unwrap(), before, "nothing changed");
        assert!(String::from_utf8_lossy(&out).contains("Current operator PIN"));
        set_pin_dialogue(&mut "135790\n246800\n246800\n".as_bytes(), &mut out, &dir).unwrap();
        assert_eq!(PinStore::new(&dir).verify(Some("246800")), Ok(()));
        // A PIN file that exists but cannot be checked must be reset first.
        std::fs::write(dir.join(PIN_FILE), "garbage").unwrap();
        let err = set_pin_dialogue(&mut "246800\n246800\n".as_bytes(), &mut out, &dir).unwrap_err();
        assert!(err.contains("--reset"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reset_needs_the_typed_phrase() {
        let dir = temp_dir("reset");
        let mut out = Vec::new();
        assert!(matches!(reset_pin_dialogue(&mut "".as_bytes(), &mut out, &dir), Ok(ResetOutcome::NothingToDelete(_))));
        let path = set_pin(&dir, "135790").unwrap();
        assert!(reset_pin_dialogue(&mut "yes\n".as_bytes(), &mut out, &dir).is_err());
        assert!(reset_pin_dialogue(&mut "".as_bytes(), &mut out, &dir).is_err());
        assert!(path.exists(), "not deleted without the phrase");
        let running = PinStore::new(&dir);
        let typed = format!("{RESET_PHRASE}\n");
        assert_eq!(reset_pin_dialogue(&mut typed.as_bytes(), &mut out, &dir), Ok(ResetOutcome::Deleted(path.clone())));
        assert!(!path.exists());
        assert!(String::from_utf8_lossy(&out).contains("restarted"));
        assert_eq!(running.verify(Some("135790")), Err(PinError::Changed));
        // With no file, setting a new PIN needs no current PIN.
        set_pin_dialogue(&mut "246800\n246800\n".as_bytes(), &mut out, &dir).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn pin_file_and_dir_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("perms").join("nested");
        let path = set_pin(&dir, "135790").unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&dir), 0o700);
        // A stale temp file is replaced, never written through.
        std::fs::write(dir.join(format!("{PIN_FILE}.tmp")), "stale").unwrap();
        set_pin(&dir, "246800").unwrap();
        assert_eq!(mode(&path), 0o600);
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    #[test]
    fn writing_leaves_no_temp_file() {
        let dir = temp_dir("tmp");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{PIN_FILE}.tmp")), "stale").unwrap();
        let path = set_pin(&dir, "135790").unwrap();
        assert!(!dir.join(format!("{PIN_FILE}.tmp")).exists());
        assert!(read_pin_file(&path).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn approval_status() {
        let dir = temp_dir("status");
        let cfg = ApprovalConfig { pin: PinStore::new(&dir), confirm: Some(PhysicalConfirm::new(14)) };
        assert_eq!(
            cfg.status_json(),
            json!({"pin_set": false, "pin_changed": false, "locked_until_unix_s": null, "sim_pin": false,
                   "physical_confirm": {"input_bit": 14, "timeout_s": 30}})
        );
        set_pin(&dir, "123456").unwrap();
        let cfg = ApprovalConfig { pin: PinStore::new(&dir), confirm: None };
        assert_eq!(
            cfg.status_json(),
            json!({"pin_set": true, "pin_changed": false, "locked_until_unix_s": null, "sim_pin": false,
                   "physical_confirm": null})
        );
        for _ in 0..MAX_FAILURES {
            let _ = cfg.pin.verify(Some("000000"));
        }
        let until = cfg.status_json()["locked_until_unix_s"].as_u64().unwrap();
        let now = unix_s(SystemTime::now());
        assert!(until >= now + 59 && until <= now + 61, "{until} vs {now}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn approval_modes() {
        assert_eq!(ApprovalMode::default(), ApprovalMode::Pin, "a fresh install asks for the PIN");
        for m in [ApprovalMode::Pin, ApprovalMode::Click] {
            assert_eq!(ApprovalMode::parse(m.as_str()), Ok(m));
        }
        for bad in ["", "PIN", "none", "pedal"] {
            assert!(ApprovalMode::parse(bad).unwrap_err().contains("pin or click"), "{bad}");
        }
    }

    #[test]
    fn error_codes() {
        let e: EngineError = PinError::Locked.into();
        assert_eq!(e.status(), axum::http::StatusCode::FORBIDDEN);
        assert_eq!(e.body()["code"], "pin_locked");
        assert_eq!(e.body()["ok"], false);
        assert_eq!(EngineError::from(PinError::Changed).body()["code"], "pin_changed");
    }

    #[test]
    fn the_sim_pin_needs_the_simulator_and_a_debug_build_or_the_env_var() {
        assert!(check_sim_pin_allowed(true, true, None).is_ok());
        assert!(check_sim_pin_allowed(false, true, None).unwrap_err().contains("--usb"));
        assert!(check_sim_pin_allowed(false, false, Some("1")).is_err(), "never on USB, whatever the env");
        assert!(check_sim_pin_allowed(true, false, None).unwrap_err().contains(ALLOW_SIM_PIN_ENV));
        assert!(check_sim_pin_allowed(true, false, Some("0")).is_err());
        assert!(check_sim_pin_allowed(true, false, Some("1")).is_ok());
    }

    #[test]
    fn the_sim_pin_approves_from_memory_and_is_never_written() {
        let dir = temp_dir("simpin");
        assert!(PinStore::simulator(&dir, "12345").is_err(), "6-12 digits");
        assert!(PinStore::simulator(&dir, "12a456").is_err());
        let store = PinStore::simulator(&dir, "424242").unwrap();
        assert!(store.is_set() && store.is_sim() && !store.has_changed());
        assert_eq!(store.verify(Some("000000")), Err(PinError::Invalid));
        assert_eq!(store.verify(None), Err(PinError::Required));
        assert_eq!(store.verify(Some("424242")), Ok(()));
        // Nothing reaches the disk: no config directory, no PIN file.
        assert!(!dir.exists(), "the sim PIN created {}", dir.display());
        assert!(!store.path().exists());
        // A PIN file on disk is neither used nor watched.
        set_pin(&dir, "111111").unwrap();
        assert!(!store.has_changed());
        assert_eq!(store.verify(Some("111111")), Err(PinError::Invalid));
        assert_eq!(store.verify(Some("424242")), Ok(()));
        let cfg = ApprovalConfig { pin: store, confirm: None };
        assert_eq!(cfg.status_json()["sim_pin"], true);
        assert_eq!(cfg.status_json()["pin_set"], true);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
