//! The fume extractor (short list #13), per device, in `<config-dir>/production.json`: switched on for a job, kept
//! on `run_on_s` after it ends (also after a Stop: the fumes are still there), and optionally a gate that refuses
//! Start until it runs.
//!
//! How it is switched ([`Output`]):
//! - `lmc_busy`: the board's Busy output (`MachineProfile.io.busy_light`, LightBurn's IO port; wire the extractor's
//!   relay to it). The board drives it during every job itself; after the job the engine holds it on
//!   (`Controller::hold_outputs`, which takes only the profile's job lights, never the laser enable, red pointer or
//!   focus bit) for the run-on time, then writes the idle word again. A stop, frame or job writes its own word over
//!   the hold. Unverified on hardware, like the IO ports.
//! - `gpio`: a Raspberry Pi pin through `pinctrl set <pin> op dh|dl` (Raspberry Pi OS; the engine's user must be
//!   allowed to use it). Optional, Linux only, unverified on a Pi.
//! - `home_assistant`: `switch.turn_on` / `turn_off` (the entity's own domain) through a Home Assistant
//!   notification target's URL and token.
//! - `webhook`: `{"event": "extractor", "state": "on"|"off"}` POSTed to a webhook notification target.
//!
//! The gate ([`Gate`]): `input` reads an input of the board (the extractor's airflow or running contact) at approval,
//! as the door interlock is read, and refuses while it is off; `switched_on` refuses unless the extractor was switched
//! on (by `auto` at this approval, or by hand in the cockpit) without an error. Both refuse with 409 and say why.
//! Switching by hand is UI-only; MCP can read the state. Nothing here can fire the laser.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::notify::{self, Request};
use super::now_unix;
use crate::libs::lock;
use crate::{AppState, EngineError};

pub const PRODUCTION_FILE: &str = "production.json";
const FORMAT: &str = "openetch-production";
/// Longest run-on, s.
pub const MAX_RUN_ON_S: u32 = 3600;

/// How the extractor is switched (see the module docs).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Output {
    #[default]
    None,
    LmcBusy,
    Gpio {
        pin: u8,
        #[serde(default)]
        active_low: bool,
    },
    HomeAssistant {
        target: String,
        entity_id: String,
    },
    Webhook {
        target: String,
    },
}

/// What Start needs from the extractor (see the module docs).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Gate {
    #[default]
    None,
    Input {
        bit: u8,
        #[serde(default)]
        active_low: bool,
    },
    SwitchedOn,
}

/// One device's extractor settings.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub output: Output,
    /// Switch it on when a run is approved.
    pub auto: bool,
    /// Keep it on this long after a job, s.
    pub run_on_s: u32,
    pub gate: Gate,
}

impl Config {
    pub fn problem(&self) -> Option<String> {
        if self.run_on_s > MAX_RUN_ON_S {
            return Some(format!("run_on_s must be within 0..={MAX_RUN_ON_S}"));
        }
        match &self.output {
            Output::Gpio { pin, .. } if *pin > 53 => return Some(format!("GPIO {pin} is not a Raspberry Pi pin (0-53)")),
            Output::HomeAssistant { entity_id, .. } if !valid_entity(entity_id) => {
                return Some(format!("\"{entity_id}\" is not a Home Assistant entity id (domain.name)"));
            }
            _ => {}
        }
        if let Gate::Input { bit, .. } = self.gate
            && bit > 15
        {
            return Some(format!("input {bit} is not a board input (0-15)"));
        }
        if self.gate == Gate::SwitchedOn && self.output == Output::None {
            return Some("the gate \"switched on\" needs an output to switch".into());
        }
        None
    }
}

fn valid_entity(e: &str) -> bool {
    let ok = |s: &str| !s.is_empty() && s.len() <= 100 && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    matches!(e.split_once('.'), Some((d, n)) if ok(d) && ok(n))
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct File {
    format: String,
    version: u32,
    extractor: BTreeMap<String, Config>,
}

/// Where the extractor is now.
#[derive(Debug, Clone, Default, Serialize)]
pub struct State {
    pub on: bool,
    pub since_unix_s: Option<u64>,
    /// When the run-on ends (it is switched off then).
    pub off_at_unix_s: Option<u64>,
    pub last_error: Option<String>,
}

/// The extractor (one field of `Production`).
#[derive(Debug, Default)]
pub struct Extractor {
    path: Mutex<Option<PathBuf>>,
    configs: Mutex<BTreeMap<String, Config>>,
    state: Mutex<State>,
    /// Bumped by every switch, so a run-on timer of an older one does nothing.
    generation: AtomicU64,
}

impl Extractor {
    pub fn open(&self, dir: &Path) -> Option<String> {
        let path = dir.join(PRODUCTION_FILE);
        let (configs, note) = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<File>(&text) {
                Ok(f) if f.format == FORMAT => (f.extractor, None),
                _ => (BTreeMap::new(), Some(format!("{}: not usable; no extractor is switched", path.display()))),
            },
            Err(_) => (BTreeMap::new(), None),
        };
        *lock(&self.configs) = configs;
        *lock(&self.path) = Some(path);
        note
    }

    pub fn config(&self, device: &str) -> Config {
        lock(&self.configs).get(device).cloned().unwrap_or_default()
    }

    pub fn state(&self) -> State {
        lock(&self.state).clone()
    }

    /// Store `device`'s settings (UI only).
    pub fn set_config(&self, device: &str, c: Config) -> Result<(), EngineError> {
        if let Some(p) = c.problem() {
            return Err(EngineError::BadRequest(p));
        }
        let mut all = lock(&self.configs);
        if c == Config::default() {
            all.remove(device);
        } else {
            all.insert(device.to_owned(), c);
        }
        let snapshot = all.clone();
        drop(all);
        self.save(snapshot)
    }

    fn save(&self, extractor: BTreeMap<String, Config>) -> Result<(), EngineError> {
        let Some(path) = lock(&self.path).clone() else { return Ok(()) };
        let file = File { format: FORMAT.into(), version: 1, extractor };
        let text = serde_json::to_string_pretty(&file).map_err(|e| EngineError::Internal(e.to_string()))?;
        crate::prefs::write_atomic(&path, text.as_bytes())
    }

    /// Forget removed devices' settings.
    pub fn forget(&self, gone: impl Fn(&str) -> bool) {
        let mut all = lock(&self.configs);
        let before = all.len();
        all.retain(|d, _| !gone(d));
        if all.len() != before {
            let snapshot = all.clone();
            drop(all);
            if let Err(e) = self.save(snapshot) {
                tracing::warn!(error = %e, "cannot save the extractor settings after removing a device");
            }
        }
    }
}

/// Switch `config`'s output on or off now. Blocking parts (curl, pinctrl, the board) run on blocking threads.
async fn switch(st: &Arc<AppState>, config: &Config, on: bool) -> Result<(), String> {
    match &config.output {
        Output::None => Err("no extractor output is set".into()),
        Output::LmcBusy => {
            let busy = st.machine.profile().io.busy_light.ok_or("the device has no Busy output (Device settings, IO ports)")?;
            let extra = if on { 1u16 << busy } else { 0 };
            let r = st
                .machine
                .blocking(move |m, ctl| {
                    // A job or a frame owns the port; the job switches the Busy output itself.
                    if m.is_running() || m.is_framing() {
                        return Ok(());
                    }
                    m.ensure_ready(ctl)?;
                    ctl.hold_outputs(extra)
                })
                .await;
            match r {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(e.to_string()),
                Err(e) => Err(e.to_string()),
            }
        }
        Output::Gpio { pin, active_low } => {
            let level = if on != *active_low { "dh" } else { "dl" };
            let pin = pin.to_string();
            tokio::task::spawn_blocking(move || gpio(&pin, level)).await.map_err(|e| e.to_string())?
        }
        Output::HomeAssistant { target, entity_id } => {
            let t = notify::target(st, target).ok_or_else(|| format!("no notification target '{target}'"))?;
            let domain = entity_id.split('.').next().unwrap_or("switch");
            let r = Request {
                url: format!("{}/api/services/{domain}/{}", t.url, if on { "turn_on" } else { "turn_off" }),
                bearer: t.token.clone(),
                body: json!({ "entity_id": entity_id }).to_string(),
            };
            tokio::task::spawn_blocking(move || notify::post(&r).map(|_| ())).await.map_err(|e| e.to_string())?
        }
        Output::Webhook { target } => {
            let t = notify::target(st, target).ok_or_else(|| format!("no notification target '{target}'"))?;
            let r = Request {
                url: t.url.clone(),
                bearer: t.token.clone(),
                body: json!({ "event": "extractor", "state": if on { "on" } else { "off" }, "device": st.active_device().id, "source": "openetch" })
                    .to_string(),
            };
            tokio::task::spawn_blocking(move || notify::post(&r).map(|_| ())).await.map_err(|e| e.to_string())?
        }
    }
}

/// `pinctrl set <pin> op <level>` (Raspberry Pi OS).
fn gpio(pin: &str, level: &str) -> Result<(), String> {
    if !cfg!(target_os = "linux") {
        return Err("GPIO pins can be switched only on a Raspberry Pi (Linux)".into());
    }
    let out = Command::new("pinctrl").args(["set", pin, "op", level]).output().map_err(|e| format!("cannot run pinctrl: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("pinctrl failed: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// Switch the active device's extractor (by hand, or for a job) and record the state.
pub async fn set(st: &Arc<AppState>, on: bool, off_at: Option<u64>) -> Result<(), EngineError> {
    let device = st.active_device().id;
    let config = st.production.extractor.config(&device);
    st.production.extractor.generation.fetch_add(1, Ordering::SeqCst);
    let r = switch(st, &config, on).await;
    {
        let mut s = lock(&st.production.extractor.state);
        match &r {
            Ok(()) => {
                if s.on != on {
                    s.since_unix_s = Some(now_unix());
                }
                s.on = on;
                s.off_at_unix_s = off_at;
                s.last_error = None;
            }
            Err(e) => s.last_error = Some(e.clone()),
        }
    }
    super::changed(st, "extractor");
    r.map_err(|e| EngineError::Conflict(format!("the extractor could not be switched {}: {e}", if on { "on" } else { "off" })))
}

/// Before a run is armed: the gate (with `auto` and the gate "switched on", the extractor is switched on first, so
/// a failed switch refuses the Start).
pub(crate) async fn before_run(st: &Arc<AppState>) -> Result<(), EngineError> {
    let device = st.active_device().id;
    let config = st.production.extractor.config(&device);
    if config.auto && config.output != Output::None && config.gate == Gate::SwitchedOn {
        set(st, true, None).await?;
    }
    match config.gate {
        Gate::None => Ok(()),
        Gate::SwitchedOn => {
            let s = st.production.extractor.state();
            if s.on && s.last_error.is_none() {
                Ok(())
            } else {
                Err(EngineError::Conflict("the extractor is not switched on: switch it on in the cockpit, then Start".into()))
            }
        }
        Gate::Input { bit, active_low } => {
            if st.machine.is_gantry() {
                return Ok(());
            }
            let port = machine::InputPort { bit, active_low };
            let read = st
                .machine
                .blocking(|m, ctl| {
                    if m.is_running() {
                        return None;
                    }
                    Some(m.ensure_ready(ctl).and_then(|()| m.read_inputs(ctl)))
                })
                .await?;
            match read {
                None => Err(EngineError::Conflict("another run is in progress".into())),
                Some(Ok(bits)) if port.active(bits) => Ok(()),
                Some(Ok(_)) => Err(EngineError::Conflict(format!(
                    "the extractor is not running (input {bit} reads off): switch it on, then Start"
                ))),
                Some(Err(e)) => Err(EngineError::Machine(e)),
            }
        }
    }
}

/// A run was approved: with `auto` (and no gate that already switched it), switch the extractor on for the job, in
/// the background (a failure is logged and shown in the cockpit; the job's own Busy output still drives `lmc_busy`).
pub(crate) fn run_started(st: &Arc<AppState>) {
    let config = st.production.extractor.config(&st.active_device().id);
    if !config.auto || config.output == Output::None || config.gate == Gate::SwitchedOn {
        return;
    }
    let st = st.clone();
    tokio::spawn(async move {
        if let Err(e) = set(&st, true, None).await {
            tracing::warn!(error = %e, "extractor not switched on for the job");
        }
    });
}

/// A run ended: keep the extractor on for the run-on time, then switch it off (unless a newer job or a hand switch
/// came in meanwhile). Without a run-on, an extractor switched on for the job is switched off at once.
pub(crate) fn after_run(st: &Arc<AppState>) {
    let device = st.active_device().id;
    let config = st.production.extractor.config(&device);
    let on = st.production.extractor.state().on;
    // The Busy output ran with the job even when nobody switched it: it gets its run-on too.
    let busy_ran = config.output == Output::LmcBusy && config.run_on_s > 0;
    if config.output == Output::None || !(on || busy_ran) {
        return;
    }
    let st = st.clone();
    tokio::spawn(async move {
        let run_on = u64::from(config.run_on_s);
        // The board wrote its idle word at the job's end: the Busy output is held on again for the run-on.
        if run_on > 0 {
            let _ = set(&st, true, Some(now_unix() + run_on)).await;
        }
        let generation = st.production.extractor.generation.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_secs(run_on)).await;
        if st.production.extractor.generation.load(Ordering::SeqCst) != generation || st.machine.is_running() {
            return;
        }
        if let Err(e) = set(&st, false, None).await {
            tracing::warn!(error = %e, "extractor run-on: could not switch it off");
        }
    });
}

/// The extractor of `device` as the cockpit and Settings show it.
pub fn status_json(st: &AppState, device: &str) -> Value {
    let config = st.production.extractor.config(device);
    let state = st.production.extractor.state();
    json!({
        "configured": config.output != Output::None,
        "config": config,
        "state": state,
        "busy_output": st.machine.profile().io.busy_light,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_checked_and_round_trip() {
        let c: Config = serde_json::from_value(json!({
            "output": { "kind": "home_assistant", "target": "ha", "entity_id": "switch.fume_extractor" },
            "auto": true, "run_on_s": 90, "gate": { "kind": "switched_on" }
        }))
        .unwrap();
        assert!(c.problem().is_none());
        assert_eq!(serde_json::from_value::<Config>(serde_json::to_value(&c).unwrap()).unwrap(), c);
        for bad in [
            json!({ "run_on_s": 99999 }),
            json!({ "output": { "kind": "gpio", "pin": 99 } }),
            json!({ "output": { "kind": "home_assistant", "target": "ha", "entity_id": "Switch Fan" } }),
            json!({ "gate": { "kind": "input", "bit": 16 } }),
            json!({ "gate": { "kind": "switched_on" } }),
        ] {
            let c: Config = serde_json::from_value(bad.clone()).unwrap();
            assert!(c.problem().is_some(), "{bad}");
        }
        assert!(serde_json::from_value::<Config>(json!({ "output": { "kind": "shell", "cmd": "rm" } })).is_err());
    }
}
