//! Graceful shutdown: a stop or a restart of the engine (`systemctl restart openetch` sends SIGTERM; Ctrl-C sends
//! SIGINT) never cuts a burn short by itself.
//!
//! On the first signal ([`run`]):
//! - New work is refused for good: approvals, Start, the foot switch, run requests (also MCP's `job_request_run`),
//!   frames and the red-dot jog, axis moves, GRBL motion, device changes and every request that touches the board
//!   (the board info and inputs, a GRBL or Ruida status refresh, a USB scan, a change to the active device) answer
//!   **503** `code: "shutting_down"`
//!   ([`AppState::refuse_if_shutting_down`]). The run claim and the shutdown share one atomic word
//!   (`hw::Machine::begin_shutdown`), so a run is either claimed before the shutdown (and waited for) or refused.
//!   Pending runs are cancelled, the foot switch cycle ends, and any frame stops (pointer off).
//! - A run that holds the machine (approved, waiting for its confirmation or start input, running, paused) is left to
//!   end by itself. It still obeys Stop, the door and the stop input while the engine waits. A continuous run is
//!   asked to end after its current iteration (it would never end otherwise; waiting for its start input between
//!   iterations, it ends there without firing, module `approve`); a run with a fixed number of
//!   iterations finishes all of them. The log says what is waited for, with the time estimate, every
//!   [`Options::report_every`].
//! - Then the engine exits cleanly: the pointer off and the board left idle (no abort sequence is sent to an idle
//!   board).
//!
//! A second signal, or the grace limit (`--shutdown-grace-secs`, default [`DEFAULT_GRACE_SECS`]; 0 aborts at once)
//! running out, aborts the run with the normal abort sequence (`AppState::stop`) before exiting. Stop in the app
//! works throughout. The abort is bounded by [`Options::abort_wait`]: a stuck board that holds the controller never
//! keeps the engine from exiting, and then the exit status is a failure ([`Outcome::failed`]).
//!
//! State: `GET /api/machine` `shutting_down` and `shutdown` ([`ShutdownState::status_json`]); machine event
//! `shutting_down` (detail: what the engine waits for). systemd kills the process `TimeoutStopSec` after SIGTERM
//! (90 s by default), so the unit needs `TimeoutStopSec` at least the grace limit (ARCHITECTURE.md "Deployment").
//!
//! The coordinator takes its signals from a channel, so tests drive it without real signals; [`forward_signals`]
//! feeds it from the OS.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};

use crate::libs::lock;
use crate::{AppState, EngineError, Event};

/// Default grace limit: how long a shutdown waits for a run before aborting it.
pub const DEFAULT_GRACE_SECS: u64 = 3600;
/// Largest grace limit `--shutdown-grace-secs` accepts (a week).
pub const MAX_GRACE_SECS: u64 = 7 * 24 * 3600;
/// After the shutdown finished, open connections (event sockets, MCP streams) get this long to close.
pub const CLOSE_WAIT: Duration = Duration::from_secs(5);

/// How [`run`] waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// How long to wait for a run before aborting it (`Duration::ZERO`: abort at once).
    pub grace: Duration,
    /// How often the run is checked.
    pub poll: Duration,
    /// How often the log says what is waited for.
    pub report_every: Duration,
    /// After an abort, how long to wait for the run to let go of the machine.
    pub abort_wait: Duration,
}

impl Options {
    pub fn new(grace: Duration) -> Self {
        Options { grace, poll: Duration::from_millis(200), report_every: Duration::from_secs(30), abort_wait: Duration::from_secs(30) }
    }
}

/// Why a shutdown aborted the run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbortReason {
    /// A second signal (its name).
    Signal(&'static str),
    /// The grace limit ran out (0: abort at once).
    GraceExpired,
}

/// How a shutdown ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// No run held the machine.
    Idle,
    /// The run ended by itself (finished, or stopped by a person, the door or the stop input) and the engine waited.
    RunEnded,
    /// The run was aborted. `released`: it let go of the machine within [`Options::abort_wait`].
    Aborted { reason: AbortReason, released: bool },
}

impl Outcome {
    /// Whether the engine should exit with a failure status: an aborted run that never let go of the machine (the
    /// board may still be busy; `systemctl status` then shows the failure).
    pub fn failed(&self) -> bool {
        matches!(self, Outcome::Aborted { released: false, .. })
    }
}

/// What `GET /api/machine` shows of a shutdown.
#[derive(Debug, Clone)]
struct Info {
    signal: &'static str,
    since_unix_s: u64,
    grace_s: u64,
    aborting: bool,
}

/// The shutdown's state (one field of [`AppState`]). The refusal itself is `hw::Machine`'s (one atomic word with the
/// run claim).
#[derive(Debug)]
pub struct ShutdownState {
    info: Mutex<Option<Info>>,
    /// Set once the shutdown finished: the event sockets close, so the server can exit.
    closing: watch::Sender<bool>,
}

impl Default for ShutdownState {
    fn default() -> Self {
        ShutdownState { info: Mutex::new(None), closing: watch::Sender::new(false) }
    }
}

impl ShutdownState {
    /// A receiver that sees `true` once the shutdown finished (connections should close).
    pub fn closing(&self) -> watch::Receiver<bool> {
        self.closing.subscribe()
    }

    /// `GET /api/machine` `shutdown`: null, or `{signal, since_unix_s, grace_s, abort_at_unix_s, aborting,
    /// waiting_for_run}`.
    pub fn status_json(&self, waiting_for_run: Option<String>) -> Value {
        match lock(&self.info).as_ref() {
            None => Value::Null,
            Some(i) => json!({
                "signal": i.signal,
                "since_unix_s": i.since_unix_s,
                "grace_s": i.grace_s,
                "abort_at_unix_s": i.since_unix_s.saturating_add(i.grace_s),
                "aborting": i.aborting,
                "waiting_for_run": waiting_for_run,
            }),
        }
    }
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl AppState {
    /// True once the engine has started shutting down.
    pub fn shutting_down(&self) -> bool {
        self.machine.is_shutting_down()
    }

    /// Refuse `what` (e.g. "approving a run") while the engine shuts down: 503 `shutting_down`.
    pub fn refuse_if_shutting_down(&self, what: &str) -> Result<(), EngineError> {
        if self.shutting_down() { Err(shutting_down_error(what)) } else { Ok(()) }
    }
}

/// The refusal of `what` while the engine shuts down.
pub(crate) fn shutting_down_error(what: &str) -> EngineError {
    EngineError::ShuttingDown(format!(
        "the engine is shutting down: {what} is refused. It waits for a running job to finish, then exits; try again \
         once it has restarted"
    ))
}

/// Wait for the first signal on `signals`, then shut down as the module docs say. Returns once the engine may exit
/// (the HTTP server keeps serving until then, so Stop and `GET /api/machine` keep working). A closed channel is
/// never a signal.
pub async fn run(st: Arc<AppState>, mut signals: mpsc::UnboundedReceiver<&'static str>, opts: Options) -> Outcome {
    let Some(first) = signals.recv().await else {
        return std::future::pending().await;
    };
    let outcome = shut_down(&st, first, &mut signals, opts).await;
    // Turn the pointer off and leave the board idle (never an abort sequence on an idle board).
    if !st.machine.is_running() {
        frame_off(&st, opts).await;
    }
    tracing::info!(?outcome, "shutdown: the engine exits");
    st.emit(Event::machine("shutting_down", Some("exiting".into())));
    st.shutdown.closing.send_replace(true);
    outcome
}

async fn shut_down(
    st: &Arc<AppState>,
    signal: &'static str,
    signals: &mut mpsc::UnboundedReceiver<&'static str>,
    opts: Options,
) -> Outcome {
    let (held, cancelled) = {
        let mut runs = st.runs();
        let held = st.machine.begin_shutdown();
        (held, runs.cancel_pending("cancelled: the engine is shutting down"))
    };
    *lock(&st.shutdown.info) =
        Some(Info { signal, since_unix_s: now_unix(), grace_s: opts.grace.as_secs(), aborting: false });
    crate::cycle::end(st, None, Some("The engine is shutting down: the foot switch cycle ended."));
    // Any frame stops (with a run on the board, only the frame loop ends: the run already ended it).
    frame_off(st, opts).await;
    let waiting = describe(st);
    tracing::warn!(
        signal,
        run_held = held,
        cancelled_pending_runs = cancelled,
        grace_s = opts.grace.as_secs(),
        "shutting down: new runs, approvals, Start, the foot switch and frames are refused; {}",
        waiting.as_deref().unwrap_or("no run holds the machine")
    );
    st.emit(Event::machine("shutting_down", Some(waiting.clone().unwrap_or_else(|| "idle".into()))));
    if !st.machine.is_running() {
        return Outcome::Idle;
    }
    if opts.grace.is_zero() {
        return abort(st, AbortReason::GraceExpired, opts).await;
    }
    let deadline = Instant::now() + opts.grace;
    let mut next_report = Instant::now() + opts.report_every;
    let mut signals_open = true;
    while st.machine.is_running() {
        end_continuous(st);
        let now = Instant::now();
        if now >= deadline {
            return abort(st, AbortReason::GraceExpired, opts).await;
        }
        if now >= next_report {
            next_report = now + opts.report_every;
            tracing::warn!(
                left_s = (deadline - now).as_secs(),
                "shutdown: still waiting: {}",
                describe(st).as_deref().unwrap_or("the machine is busy")
            );
        }
        // A frame cannot start now (refused), but never leave one lit.
        if st.machine.is_framing() {
            frame_off(st, opts).await;
        }
        let nap = opts.poll.min(deadline - now);
        tokio::select! {
            s = signals.recv(), if signals_open => match s {
                Some(s) => return abort(st, AbortReason::Signal(s), opts).await,
                None => signals_open = false,
            },
            () = tokio::time::sleep(nap) => {}
        }
    }
    tracing::info!("shutdown: the run ended; exiting");
    Outcome::RunEnded
}

/// Stop any frame (pointer off), bounded by [`Options::abort_wait`] like the abort: a stuck board that holds the
/// controller never holds the shutdown (the frame loop has been told to end at once either way).
async fn frame_off(st: &AppState, opts: Options) {
    match tokio::time::timeout(opts.abort_wait, st.frame_stop()).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => tracing::warn!(error = %e, "shutdown: could not turn the pointer off"),
        Err(_) => tracing::error!("shutdown: the board did not take the frame stop (it is busy or stuck)"),
    }
}

/// A continuous run never ends by itself: ask it to end after its current iteration (Repeat and a fixed count run to
/// the end).
fn end_continuous(st: &AppState) {
    let job = st.job_status();
    if job["active"] == json!(true) && job["iterations"].is_null() && job["end_after_current"] != json!(true) {
        st.job.end_after_current();
        tracing::warn!(run_id = %job["run_id"], "shutdown: the continuous run ends after its current iteration");
    }
}

/// What the shutdown waits for, for the log and the `shutting_down` event: the run, its state and the time left.
fn describe(st: &AppState) -> Option<String> {
    if !st.machine.is_running() {
        return None;
    }
    let job = st.job_status();
    if job["active"] != json!(true) {
        return Some("waiting for a machine operation (axis move or device change) to end".into());
    }
    let run_id = job["run_id"].as_str().unwrap_or("?");
    let state = job["state"].as_str().unwrap_or("?");
    let iterations = match job["iterations"].as_u64() {
        Some(n) => format!("iteration {} of {n}", job["iteration"]),
        None => format!("iteration {} (continuous: ends after this one)", job["iteration"]),
    };
    let left = match job["remaining_s"].as_f64() {
        Some(s) if state == "running" || state == "paused" => format!(", about {} s left in this iteration", s.ceil()),
        _ => String::new(),
    };
    let detail = match state {
        "waiting_for_start_input" => " (waiting for the foot pedal / start input)",
        "paused" => " (paused: resume or stop it)",
        "starting" => " (approved; waiting for the confirmation input or the board)",
        _ => "",
    };
    Some(format!("waiting for run {run_id} to finish: {state}{detail}, {iterations}{left}"))
}

/// Abort the run with the normal abort sequence (Stop), then wait for it to let go of the machine.
async fn abort(st: &Arc<AppState>, reason: AbortReason, opts: Options) -> Outcome {
    if let Some(i) = lock(&st.shutdown.info).as_mut() {
        i.aborting = true;
    }
    let why = match &reason {
        AbortReason::Signal(s) => format!("a second signal ({s})"),
        AbortReason::GraceExpired if opts.grace.is_zero() => "--shutdown-grace-secs 0".to_owned(),
        AbortReason::GraceExpired => format!("the grace limit ({} s) ran out", opts.grace.as_secs()),
    };
    tracing::error!(
        "shutdown: aborting the run because of {why}: {}",
        describe(st).as_deref().unwrap_or("the machine is busy")
    );
    st.emit(Event::machine("shutting_down", Some(format!("aborting the run: {why}"))));
    let until = Instant::now() + opts.abort_wait;
    // Bounded: a board stuck under the run holds the controller, and the abort sequence would wait behind it for
    // ever (the stop epoch has moved on at once, so the run still stops should the board come back).
    match tokio::time::timeout(opts.abort_wait, st.stop()).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => tracing::error!(error = %e, "shutdown: the abort sequence failed; use the hardware E-stop"),
        Err(_) => tracing::error!(
            wait_s = opts.abort_wait.as_secs(),
            "shutdown: the abort sequence could not reach the board (it is busy or stuck); use the hardware E-stop"
        ),
    }
    while st.machine.is_running() && Instant::now() < until {
        tokio::time::sleep(opts.poll.min(Duration::from_millis(50))).await;
    }
    let released = !st.machine.is_running();
    if !released {
        tracing::error!("shutdown: the run did not end after the abort; exiting anyway (use the hardware E-stop)");
    }
    Outcome::Aborted { reason, released }
}

/// Forward SIGTERM and SIGINT (Unix) or Ctrl-C (elsewhere) to [`run`], every one of them (the second aborts). If
/// the handlers cannot be installed, the engine keeps the old behaviour of the OS (no graceful shutdown) and says so.
pub async fn forward_signals(tx: mpsc::UnboundedSender<&'static str>) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let (mut term, mut int) = match (signal(SignalKind::terminate()), signal(SignalKind::interrupt())) {
            (Ok(t), Ok(i)) => (t, i),
            (Err(e), _) | (_, Err(e)) => {
                tracing::error!(error = %e, "cannot handle SIGTERM/SIGINT; a stop or restart will not wait for a run");
                return std::future::pending().await;
            }
        };
        loop {
            let name = tokio::select! {
                _ = term.recv() => "SIGTERM",
                _ = int.recv() => "SIGINT",
            };
            if tx.send(name).is_err() {
                return;
            }
        }
    }
    #[cfg(not(unix))]
    loop {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::error!(error = %e, "cannot handle Ctrl-C; a stop will not wait for a run");
            return std::future::pending().await;
        }
        if tx.send("Ctrl-C").is_err() {
            return;
        }
    }
}

/// Resolves [`CLOSE_WAIT`] after the shutdown finished: the server then exits even if a connection stays open.
pub async fn closed_then_wait(mut closing: watch::Receiver<bool>) {
    if closing.wait_for(|c| *c).await.is_err() {
        return std::future::pending().await;
    }
    tokio::time::sleep(CLOSE_WAIT).await;
}

#[cfg(test)]
#[path = "shutdown_tests.rs"]
mod tests;
