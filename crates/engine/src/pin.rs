//! The operator PIN that approving a run requires, and the optional physical confirmation input.
//!
//! A person sets the PIN once with `openetch set-pin` (interactive; refused unless stdin is a terminal). It is stored
//! as a salted SHA-256 hash in `operator-pin` in the engine's config directory and read again on every approval, so
//! a new PIN takes effect without a restart. Five wrong PINs in a row lock approvals for [`LOCKOUT`].
//!
//! The MCP server never sees, asks for or handles a PIN. This keeps an agent that only has MCP from approving; it
//! does **not** stop a local process running with the user's OS privileges (it could read the hash file, attach to
//! the engine, or drive the browser). The hardware E-stop/key switch and, when configured, the physical confirmation
//! input ([`PhysicalConfirm`]) are the real guarantees.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::EngineError;

/// File name of the PIN hash inside the config directory.
pub const PIN_FILE: &str = "operator-pin";
/// Shortest and longest PIN, in digits.
pub const PIN_DIGITS: std::ops::RangeInclusive<usize> = 4..=12;
/// Wrong PINs in a row before approvals lock.
pub const MAX_FAILURES: u32 = 5;
/// How long approvals stay locked after [`MAX_FAILURES`] wrong PINs.
pub const LOCKOUT: Duration = Duration::from_secs(60);
/// How long an approved run waits for the physical confirmation input.
pub const CONFIRM_TIMEOUT: Duration = Duration::from_secs(30);
/// Pause between `ReadPort` polls while waiting for the physical input.
pub const CONFIRM_POLL: Duration = Duration::from_millis(50);
/// Consecutive reads the input must be seen pressed (galvoplotter's `input_passes_required` is 3).
pub const CONFIRM_READS: u32 = 3;

/// Domain prefix hashed before the salt, so the hash is useless for anything but this file.
const HASH_DOMAIN: &[u8] = b"openetch-operator-pin-v1";
/// File format tag.
const FORMAT: &str = "sha256";

/// Why an approval was refused for PIN reasons. Rendered as 403 `{ok: false, error, code}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinError {
    /// No PIN has been set (run `openetch set-pin`), or the PIN file is unreadable.
    NotSet,
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
            PinError::Required => "pin_required",
            PinError::Invalid => "pin_invalid",
            PinError::Locked => "pin_locked",
        }
    }

    fn message(self) -> String {
        match self {
            PinError::NotSet => {
                "no operator PIN is set: run `openetch set-pin` on the engine's computer, then approve again".into()
            }
            PinError::Required => "approving a run needs the operator PIN".into(),
            PinError::Invalid => "wrong operator PIN".into(),
            PinError::Locked => {
                format!("too many wrong PINs: approvals are locked for {} s; wait and try again", LOCKOUT.as_secs())
            }
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

/// Check a PIN's form: 4-12 ASCII digits.
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

/// Hash `pin` with a fresh salt and write it to `dir/operator-pin` (creating `dir`), replacing any old PIN.
pub fn set_pin(dir: &Path, pin: &str) -> Result<PathBuf, String> {
    validate_pin(pin)?;
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|e| format!("no random source for the salt: {e}"))?;
    let line = format!("{FORMAT}${}${}\n", hex(&salt), hex(&hash(&salt, pin)));
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let path = dir.join(PIN_FILE);
    let tmp = dir.join(format!("{PIN_FILE}.tmp"));
    std::fs::write(&tmp, line).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, &path).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

/// `openetch set-pin`'s dialogue: read the PIN twice from `input`, check it, store it. The caller makes sure
/// `input` is a terminal. std has no portable way to turn echo off, so the PIN is visible while typed.
pub fn set_pin_dialogue(input: &mut impl BufRead, out: &mut impl Write, dir: &Path) -> Result<PathBuf, String> {
    let mut ask = |prompt: &str| -> Result<String, String> {
        write!(out, "{prompt}").and_then(|_| out.flush()).map_err(|e| e.to_string())?;
        let mut line = String::new();
        if input.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            return Err("no input".into());
        }
        Ok(line.trim().to_owned())
    };
    let first = ask(&format!(
        "New operator PIN ({}-{} digits; it is shown as you type, so make sure nobody is watching): ",
        PIN_DIGITS.start(),
        PIN_DIGITS.end()
    ))?;
    validate_pin(&first)?;
    let second = ask("Repeat the PIN: ")?;
    if !same(first.as_bytes(), second.as_bytes()) {
        return Err("the two PINs differ; nothing was changed".into());
    }
    set_pin(dir, &first)
}

/// Read `dir/operator-pin`: `(salt, hash)`, or `None` if it is missing or malformed.
fn read_pin_file(path: &Path) -> Option<(Vec<u8>, Vec<u8>)> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut parts = text.trim().split('$');
    let (fmt, salt, digest) = (parts.next()?, parts.next()?, parts.next()?);
    if fmt != FORMAT || parts.next().is_some() {
        return None;
    }
    let (salt, digest) = (unhex(salt)?, unhex(digest)?);
    (!salt.is_empty() && digest.len() == 32).then_some((salt, digest))
}

#[derive(Debug, Default)]
struct Attempts {
    failures: u32,
    locked_until: Option<Instant>,
}

/// Checks approval PINs against the hash file, with the wrong-PIN lockout.
#[derive(Debug)]
pub struct PinStore {
    path: PathBuf,
    lockout: Duration,
    attempts: Mutex<Attempts>,
}

impl PinStore {
    /// The store for config directory `dir`.
    pub fn new(dir: &Path) -> Self {
        Self::with_lockout(dir, LOCKOUT)
    }

    /// With a different lockout time (tests).
    pub fn with_lockout(dir: &Path, lockout: Duration) -> Self {
        PinStore { path: dir.join(PIN_FILE), lockout, attempts: Mutex::new(Attempts::default()) }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// True when a valid PIN file exists.
    pub fn is_set(&self) -> bool {
        read_pin_file(&self.path).is_some()
    }

    /// Check `pin`. Order: not set, locked, missing, wrong. A wrong PIN counts toward the lockout; the
    /// [`MAX_FAILURES`]th locks approvals and is answered with [`PinError::Locked`]. A right PIN resets the count.
    pub fn verify(&self, pin: Option<&str>) -> Result<(), PinError> {
        let Some((salt, digest)) = read_pin_file(&self.path) else { return Err(PinError::NotSet) };
        let mut a = self.attempts.lock().unwrap_or_else(PoisonError::into_inner);
        match a.locked_until {
            Some(t) if Instant::now() < t => return Err(PinError::Locked),
            Some(_) => *a = Attempts::default(),
            None => {}
        }
        let pin = pin.map(str::trim).filter(|p| !p.is_empty()).ok_or(PinError::Required)?;
        if validate_pin(pin).is_ok() && same(&hash(&salt, pin), &digest) {
            a.failures = 0;
            return Ok(());
        }
        a.failures += 1;
        if a.failures >= MAX_FAILURES {
            a.locked_until = Some(Instant::now() + self.lockout);
            tracing::warn!(failures = a.failures, "approvals locked after repeated wrong operator PINs");
            return Err(PinError::Locked);
        }
        Err(PinError::Invalid)
    }
}

/// Optional physical confirmation: after PIN approval the run waits until input `input_bit` (`ReadPort`, reply
/// word 1) is seen released and then pressed for [`CONFIRM_READS`] reads in a row, for at most `timeout`. Requiring
/// the release first means a stuck or inverted input can never confirm on its own.
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
            "physical_confirm": self.confirm.map(|c| json!({
                "input_bit": c.input_bit,
                "timeout_s": c.timeout.as_secs(),
            })),
        })
    }
}

#[cfg(test)]
pub(crate) fn temp_dir(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::SeqCst);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    std::env::temp_dir().join(format!("openetch-test-{tag}-{}-{n}-{nanos}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_must_be_4_to_12_digits() {
        for ok in ["1234", "000000000000"] {
            assert!(validate_pin(ok).is_ok(), "{ok}");
        }
        for bad in ["123", "1234567890123", "12a4", " 1234", "", "１２３４"] {
            assert!(validate_pin(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn set_and_verify() {
        let dir = temp_dir("pin");
        let store = PinStore::new(&dir);
        assert!(!store.is_set());
        assert_eq!(store.verify(Some("1234")), Err(PinError::NotSet));
        let path = set_pin(&dir, "4321").unwrap();
        assert!(path.ends_with(PIN_FILE) && store.is_set());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("sha256$") && !text.contains("4321"), "{text}");
        assert_eq!(store.verify(None), Err(PinError::Required));
        assert_eq!(store.verify(Some("  ")), Err(PinError::Required));
        assert_eq!(store.verify(Some("1111")), Err(PinError::Invalid));
        assert_eq!(store.verify(Some("4321")), Ok(()));
        // A new PIN gets a new salt and replaces the old one.
        set_pin(&dir, "98765").unwrap();
        assert_ne!(std::fs::read_to_string(&path).unwrap(), text);
        assert_eq!(store.verify(Some("4321")), Err(PinError::Invalid));
        assert_eq!(store.verify(Some("98765")), Ok(()));
        assert!(set_pin(&dir, "12").is_err());
        std::fs::write(&path, "garbage").unwrap();
        assert_eq!(store.verify(Some("98765")), Err(PinError::NotSet));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wrong_pins_lock_approvals() {
        let dir = temp_dir("lock");
        set_pin(&dir, "2468").unwrap();
        let store = PinStore::with_lockout(&dir, Duration::from_millis(200));
        for _ in 1..MAX_FAILURES {
            assert_eq!(store.verify(Some("0000")), Err(PinError::Invalid));
        }
        assert_eq!(store.verify(Some("0000")), Err(PinError::Locked));
        // Even the right PIN is refused while locked.
        assert_eq!(store.verify(Some("2468")), Err(PinError::Locked));
        std::thread::sleep(Duration::from_millis(250));
        assert_eq!(store.verify(Some("2468")), Ok(()));
        // A success resets the count.
        for _ in 1..MAX_FAILURES {
            assert_eq!(store.verify(Some("0000")), Err(PinError::Invalid));
        }
        assert_eq!(store.verify(Some("2468")), Ok(()));
        assert_eq!(store.verify(Some("0000")), Err(PinError::Invalid));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dialogue_reads_the_pin_twice() {
        let dir = temp_dir("dialogue");
        let mut out = Vec::new();
        let err = set_pin_dialogue(&mut "1234\n1235\n".as_bytes(), &mut out, &dir).unwrap_err();
        assert!(err.contains("differ"));
        assert!(!PinStore::new(&dir).is_set());
        assert!(set_pin_dialogue(&mut "12\n12\n".as_bytes(), &mut out, &dir).is_err());
        assert!(set_pin_dialogue(&mut "".as_bytes(), &mut out, &dir).is_err());
        set_pin_dialogue(&mut "135790\r\n135790\r\n".as_bytes(), &mut out, &dir).unwrap();
        assert_eq!(PinStore::new(&dir).verify(Some("135790")), Ok(()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn approval_status() {
        let dir = temp_dir("status");
        let cfg = ApprovalConfig { pin: PinStore::new(&dir), confirm: Some(PhysicalConfirm::new(14)) };
        assert_eq!(
            cfg.status_json(),
            json!({"pin_set": false, "physical_confirm": {"input_bit": 14, "timeout_s": 30}})
        );
        set_pin(&dir, "1234").unwrap();
        let cfg = ApprovalConfig { confirm: None, ..cfg };
        assert_eq!(cfg.status_json(), json!({"pin_set": true, "physical_confirm": null}));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn error_codes() {
        let e: EngineError = PinError::Locked.into();
        assert_eq!(e.status(), axum::http::StatusCode::FORBIDDEN);
        assert_eq!(e.body()["code"], "pin_locked");
        assert_eq!(e.body()["ok"], false);
    }
}
