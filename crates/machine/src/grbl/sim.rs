//! [`SimGrbl`]: a GRBL 1.1 laser simulated in Rust, for tests and the engine's simulator mode. Never touches
//! hardware. Written from GRBL's documented protocol (see [`super::protocol`]), not from its code.
//!
//! What it models:
//!
//! - **The receive buffer.** Line bytes wait in a 128-byte buffer until the parser takes the line; a host that has
//!   more than [`RX_BUFFER`] bytes outstanding is recorded as an overflow ([`SimGrblReport::overflowed`]), so
//!   tests prove the character-counting flow control. Realtime bytes (`?`, `!`, `~`, 0x18, 0x85, 0x80+) never enter
//!   the buffer and act at once.
//! - **The planner.** A parsed motion line becomes a block in a 15-block planner; `ok` is sent when the line is
//!   parsed, which only happens while the planner has room. Each `read` by the host executes one block, so motion
//!   takes "time" relative to the host's reads and the buffers really fill up.
//! - **The G-code subset** a laser uses: `G0 G1 G2 G3` (arcs as straight moves to their end point), `G4`, units,
//!   distance mode, `G17-19`, `G28/G30`, `G53-59`, `G92`, `G94`, `M0-2 M3-5 M7-9 M30`, `S`, `F`, `$` commands
//!   (`$$ $# $G $I $N $X $H $C $J= $SLP $RST= $Nn= $n=`), check mode, alarm lock-out (`error:9`), unknown words
//!   (`error:20`), missing feed (`error:22`).
//! - **States**: Idle, Run, Hold (feed hold), Jog, Alarm (reset while moving is `ALARM:3`; with homing enabled the
//!   board starts locked), Door (a [`SimGrblHandle::set_door`] while moving holds and reports `Door`), Check.
//! - **Whether the laser would have been on** ([`Fire`]): a `G1`/`G2`/`G3` block executed with `M3`/`M4` and
//!   `S > 0`; a `G0` or jog block with the spindle on and laser mode off (`$32=0`); a `G4` dwell with `M3` and
//!   `S > 0`; and with laser mode off, `M3`/`M4` with `S > 0` taking effect even without motion. Tests read these
//!   through [`SimGrblHandle::report`].

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use super::SerialLink;
use super::program::{Word, parse_words};
use super::protocol::{CYCLE_START, FEED_HOLD, JOG_CANCEL, RX_BUFFER, SOFT_RESET, STATUS};
use crate::link::LinkError;

/// Planner blocks GRBL holds (its 16-block buffer less one).
pub const PLANNER_BLOCKS: usize = 15;
/// GRBL's serial receive buffer.
const RX_CAPACITY: usize = 128;
/// Lines kept in [`SimGrblReport::lines`] (the last ones).
const KEEP_LINES: usize = 2000;

/// How the simulated board is set up.
#[derive(Debug, Clone, PartialEq)]
pub struct SimGrblConfig {
    /// `$32`: laser mode.
    pub laser_mode: bool,
    /// `$30` / `$31`.
    pub s_max: f64,
    pub s_min: f64,
    /// `$22`: homing enabled (the board then starts in the alarm state).
    pub homing: bool,
    /// Start-up blocks `$N0`, `$N1`.
    pub startup: [String; 2],
    /// Status reports carry the work coordinate offset (`WCO`, on the first and every 10th, as GRBL's default);
    /// off: never, so the driver has no work position it can trust.
    pub report_wco: bool,
}

impl Default for SimGrblConfig {
    fn default() -> Self {
        SimGrblConfig { laser_mode: true, s_max: 1000.0, s_min: 0.0, homing: false, startup: [String::new(), String::new()], report_wco: true }
    }
}

/// How the laser came to be on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FireKind {
    /// A cutting move (`G1`/`G2`/`G3`) with the spindle on and `S > 0`.
    Cut,
    /// A rapid (`G0`) with the spindle on and laser mode off.
    Rapid,
    /// A jog with the spindle on.
    Jog,
    /// A dwell (`G4`) in constant power `M3`, or with laser mode off.
    Dwell,
    /// Laser mode off: `M3`/`M4` with `S > 0` lights the laser without any motion.
    Static,
}

/// One time the laser would have been on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fire {
    pub kind: FireKind,
    pub from: [f64; 2],
    pub to: [f64; 2],
    pub s: f64,
    /// Air assist (coolant `M7`/`M8`) was on.
    pub air: bool,
}

/// A realtime byte as it arrived, with the buffers' fill at that moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RealtimeEvent {
    pub byte: u8,
    /// Line bytes waiting in the receive buffer.
    pub rx_bytes: usize,
    /// Blocks waiting in the planner.
    pub planner_blocks: usize,
    /// Blocks executed so far.
    pub executed: usize,
}

/// What happened on the simulated board.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SimGrblReport {
    pub fires: Vec<Fire>,
    /// Lines received (all of them, counted) and the last [`KEEP_LINES`] as text.
    pub lines_received: usize,
    pub lines: Vec<String>,
    /// Most line bytes ever waiting in the receive buffer, and whether the host ever sent more than
    /// [`RX_BUFFER`] (the character-counting limit).
    pub max_rx_bytes: usize,
    pub overflowed: bool,
    /// Line bytes and planner blocks waiting right now.
    pub rx_bytes: usize,
    pub planner_blocks: usize,
    pub realtime: Vec<RealtimeEvent>,
    pub blocks_executed: usize,
    pub resets: usize,
    pub position: [f64; 2],
    pub state: String,
    /// The laser is lit right now (spindle on with `S > 0` and laser mode off).
    pub lit_now: bool,
    /// Air assist (coolant `M7`/`M8`) is on in the parser state right now.
    pub air_now: bool,
}

impl SimGrblReport {
    /// Fires that came from cutting moves only.
    pub fn cuts(&self) -> usize {
        self.fires.iter().filter(|f| f.kind == FireKind::Cut).count()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Rapid,
    Linear,
    Dwell,
    Jog,
}

#[derive(Debug, Clone, Copy)]
struct Block {
    kind: Kind,
    from: [f64; 2],
    to: [f64; 2],
    /// 0 off, 3, 4.
    spindle: u8,
    s: f64,
    air: bool,
}

#[derive(Debug, Clone)]
struct Modal {
    motion: u8,
    absolute: bool,
    mm: bool,
    spindle: u8,
    s: f64,
    f: f64,
    /// Coolant (air assist): `M7`/`M8` on, `M9` off; a reset turns it off.
    air: bool,
}

impl Default for Modal {
    fn default() -> Self {
        Modal { motion: 0, absolute: true, mm: true, spindle: 0, s: 0.0, f: 0.0, air: false }
    }
}

#[derive(Debug)]
struct Board {
    settings: BTreeMap<u16, f64>,
    startup: [String; 2],
    rx: VecDeque<u8>,
    out: VecDeque<u8>,
    planner: VecDeque<Block>,
    modal: Modal,
    /// Where the last parsed block ends.
    planned: [f64; 2],
    /// Where the head is.
    position: [f64; 2],
    held: bool,
    alarm: bool,
    door: bool,
    /// Execution held from outside (a long, slow move): the state stays Run.
    stalled: bool,
    check: bool,
    report: SimGrblReport,
    disconnected: bool,
    /// Lines containing this text are answered with this error code.
    reject: Option<(String, u8)>,
    /// Feed hold and soft reset bytes are lost on the way (a flaky cable): the board never sees them.
    lose_stop_bytes: bool,
    /// Status reports sent (the work offset rides on every 10th), and whether it is reported at all.
    reports: u32,
    report_wco: bool,
}

fn fmt(v: f64) -> String {
    format!("{v:.3}")
}

impl Board {
    fn new(cfg: &SimGrblConfig) -> Self {
        let mut settings: BTreeMap<u16, f64> = [
            (0, 10.0), (1, 25.0), (2, 0.0), (3, 0.0), (4, 0.0), (5, 0.0), (6, 0.0), (10, 1.0), (11, 0.01), (12, 0.002),
            (13, 0.0), (20, 0.0), (21, 0.0), (23, 0.0), (24, 25.0), (25, 500.0), (26, 250.0), (27, 1.0), (100, 80.0),
            (101, 80.0), (102, 80.0), (110, 6000.0), (111, 6000.0), (112, 500.0), (120, 500.0), (121, 500.0), (122, 10.0),
            (130, 400.0), (131, 400.0), (132, 200.0),
        ]
        .into_iter()
        .collect();
        settings.insert(22, if cfg.homing { 1.0 } else { 0.0 });
        settings.insert(30, cfg.s_max);
        settings.insert(31, cfg.s_min);
        settings.insert(32, if cfg.laser_mode { 1.0 } else { 0.0 });
        let mut b = Board {
            settings,
            startup: cfg.startup.clone(),
            rx: VecDeque::new(),
            out: VecDeque::new(),
            planner: VecDeque::new(),
            modal: Modal::default(),
            planned: [0.0, 0.0],
            position: [0.0, 0.0],
            held: false,
            alarm: false,
            door: false,
            stalled: false,
            check: false,
            reports: 0,
            report_wco: cfg.report_wco,
            report: SimGrblReport::default(),
            disconnected: false,
            reject: None,
            lose_stop_bytes: false,
        };
        b.boot(false);
        b
    }

    fn laser_mode(&self) -> bool {
        self.settings.get(&32).copied().unwrap_or(0.0) != 0.0
    }

    fn homing(&self) -> bool {
        self.settings.get(&22).copied().unwrap_or(0.0) != 0.0
    }

    fn say(&mut self, line: &str) {
        self.out.extend(line.bytes());
        self.out.extend(b"\r\n");
    }

    /// Power-up or reset: welcome, then the lock message or the start-up blocks.
    fn boot(&mut self, alarm_3: bool) {
        if alarm_3 {
            self.say("ALARM:3");
            self.alarm = true;
        } else if self.homing() {
            self.alarm = true;
        }
        self.say("");
        self.say("Grbl 1.1h ['$' for help]");
        if self.alarm {
            self.say("[MSG:'$H'|'$X' to unlock]");
            return;
        }
        for i in 0..2 {
            let block = self.startup[i].clone();
            if block.is_empty() {
                continue;
            }
            match self.gcode(&block) {
                Ok(()) => self.say(&format!(">{block}:ok")),
                Err(code) => self.say(&format!(">{block}:error:{code}")),
            }
        }
    }

    fn state_name(&self) -> &'static str {
        if self.alarm {
            "Alarm"
        } else if self.door {
            "Door:0"
        } else if self.held && !self.planner.is_empty() {
            "Hold:0"
        } else if self.planner.front().is_some_and(|b| b.kind == Kind::Jog) {
            "Jog"
        } else if !self.planner.is_empty() {
            "Run"
        } else if self.check {
            "Check"
        } else {
            "Idle"
        }
    }

    fn status(&mut self) {
        let pins = if self.door { "|Pn:D" } else { "" };
        let s = self.planner.front().map_or(0.0, |b| if b.spindle != 0 { b.s } else { 0.0 });
        // GRBL adds the work coordinate offset to the first report and then every 10th (no offsets here: G54 = 0).
        let wco = if self.report_wco && self.reports % 10 == 0 { "|WCO:0.000,0.000,0.000" } else { "" };
        self.reports = self.reports.wrapping_add(1);
        let line = format!(
            "<{}|MPos:{},{},0.000|FS:{},{}{pins}{wco}>",
            self.state_name(),
            fmt(self.position[0]),
            fmt(self.position[1]),
            self.modal.f,
            s
        );
        self.say(&line);
    }

    fn realtime(&mut self, b: u8) {
        self.report.realtime.push(RealtimeEvent {
            byte: b,
            rx_bytes: self.rx.len(),
            planner_blocks: self.planner.len(),
            executed: self.report.blocks_executed,
        });
        match b {
            STATUS => {
                // The report itself is not kept in the realtime log (polls would flood it).
                self.report.realtime.pop();
                self.status();
            }
            FEED_HOLD => {
                if !self.planner.is_empty() {
                    self.held = true;
                }
            }
            CYCLE_START => {
                if !self.door {
                    self.held = false;
                }
            }
            SOFT_RESET => {
                let moving = !self.planner.is_empty() && !self.alarm;
                self.rx.clear();
                self.planner.clear();
                self.modal = Modal::default();
                self.planned = self.position;
                self.held = false;
                self.check = false;
                self.report.resets += 1;
                self.boot(moving);
            }
            JOG_CANCEL => {
                self.planner.retain(|b| b.kind != Kind::Jog);
                self.planned = self.planner.back().map_or(self.position, |b| b.to);
                if self.planner.is_empty() {
                    self.held = false;
                }
            }
            _ => {}
        }
    }

    /// Take complete lines from the receive buffer while the planner has room.
    fn parse(&mut self) {
        while self.planner.len() < PLANNER_BLOCKS {
            let Some(end) = self.rx.iter().position(|&b| b == b'\n') else { return };
            let raw: Vec<u8> = self.rx.drain(..=end).collect();
            let line = String::from_utf8_lossy(&raw).trim().to_owned();
            self.report.lines_received += 1;
            self.report.lines.push(line.clone());
            if self.report.lines.len() > KEEP_LINES {
                self.report.lines.remove(0);
            }
            let rejected = self.reject.as_ref().filter(|(text, _)| line.contains(text.as_str())).map(|(_, code)| *code);
            let result = if let Some(code) = rejected {
                Err(code)
            } else if line.is_empty() {
                Ok(())
            } else if let Some(sys) = line.strip_prefix('$') {
                self.system(&sys.to_ascii_uppercase())
            } else if self.alarm {
                Err(9)
            } else {
                self.gcode(&line)
            };
            match result {
                Ok(()) => self.say("ok"),
                Err(code) => self.say(&format!("error:{code}")),
            }
        }
    }

    fn gc_state(&self) -> String {
        let m = &self.modal;
        format!(
            "[GC:G{} G54 G17 {} {} G94 M{} M{} T0 F{} S{}]",
            m.motion,
            if m.mm { "G21" } else { "G20" },
            if m.absolute { "G90" } else { "G91" },
            if m.spindle == 0 { 5 } else { m.spindle },
            if m.air { 8 } else { 9 },
            m.f,
            m.s
        )
    }

    fn system(&mut self, cmd: &str) -> Result<(), u8> {
        match cmd {
            "" => self.say("[HLP:$$ $# $G $I $N $x=val $Nx=line $J=line $SLP $C $X $H ~ ! ? ctrl-x]"),
            "$" => {
                let lines: Vec<String> = self.settings.iter().map(|(k, v)| format!("${k}={}", trim(*v))).collect();
                for l in lines {
                    self.say(&l);
                }
            }
            "#" => self.say("[G54:0.000,0.000,0.000]"),
            "G" => {
                let s = self.gc_state();
                self.say(&s);
            }
            "I" => {
                self.say("[VER:1.1h.20190825:]");
                self.say("[OPT:V,15,128]");
            }
            "N" => {
                let lines = [format!("$N0={}", self.startup[0]), format!("$N1={}", self.startup[1])];
                for l in lines {
                    self.say(&l);
                }
            }
            "X" => {
                if self.alarm {
                    self.alarm = false;
                    self.say("[MSG:Caution: Unlocked]");
                }
            }
            "H" => {
                if !self.homing() {
                    return Err(5);
                }
                if !self.planner.is_empty() {
                    return Err(8);
                }
                if self.door {
                    self.alarm = true;
                    self.say("ALARM:7");
                    return Ok(());
                }
                self.position = [0.0, 0.0];
                self.planned = [0.0, 0.0];
                self.alarm = false;
            }
            "C" => {
                if !self.planner.is_empty() {
                    return Err(8);
                }
                self.check = !self.check;
                self.say(if self.check { "[MSG:Enabled]" } else { "[MSG:Disabled]" });
            }
            "SLP" => self.say("[MSG:Sleeping]"),
            _ if cmd.starts_with("J=") => {
                if self.alarm {
                    return Err(9);
                }
                if self.planner.iter().any(|b| b.kind != Kind::Jog) {
                    return Err(8);
                }
                self.jog(&cmd[2..])?;
            }
            _ if cmd.starts_with("RST=") => {
                *self = Board::new(&SimGrblConfig::default());
            }
            _ if cmd.starts_with("N0=") || cmd.starts_with("N1=") => {
                let i = usize::from(cmd.as_bytes()[1] - b'0');
                self.startup[i] = cmd[3..].to_owned();
            }
            _ => {
                let (k, v) = cmd.split_once('=').ok_or(3)?;
                let k: u16 = k.parse().map_err(|_| 3)?;
                let v: f64 = v.parse().map_err(|_| 2)?;
                if v < 0.0 {
                    return Err(4);
                }
                self.settings.insert(k, v);
            }
        }
        Ok(())
    }

    fn jog(&mut self, line: &str) -> Result<(), u8> {
        let words = parse_words(line).map_err(|_| 2)?;
        let (mut absolute, mut f) = (self.modal.absolute, None);
        let (mut x, mut y) = (None, None);
        for w in &words {
            match w.letter {
                'G' if w.is('G', 90.0) => absolute = true,
                'G' if w.is('G', 91.0) => absolute = false,
                'G' if w.is('G', 21.0) || w.is('G', 20.0) || w.is('G', 53.0) => {}
                'X' => x = Some(w.value),
                'Y' => y = Some(w.value),
                'Z' => {}
                'F' => f = Some(w.value),
                _ => return Err(16),
            }
        }
        if f.is_none_or(|f| f <= 0.0) {
            return Err(16);
        }
        let to = target(self.planned, absolute, x, y);
        self.push(Block { kind: Kind::Jog, from: self.planned, to, spindle: self.modal.spindle, s: self.modal.s, air: self.modal.air });
        Ok(())
    }

    fn push(&mut self, b: Block) {
        self.planned = b.to;
        if !self.check {
            self.planner.push_back(b);
        }
    }

    fn gcode(&mut self, line: &str) -> Result<(), u8> {
        let words: Vec<Word> = parse_words(line).map_err(|_| 1)?;
        let mut m = self.modal.clone();
        let (mut x, mut y, mut p) = (None, None, None);
        let mut dwell = false;
        for w in &words {
            let code = (w.value * 10.0).round() as i64;
            match (w.letter, code) {
                ('G', 0 | 10 | 20 | 30) => m.motion = (code / 10) as u8,
                ('G', 40) => dwell = true,
                ('G', 200) => m.mm = false,
                ('G', 210) => m.mm = true,
                ('G', 900) => m.absolute = true,
                ('G', 910) => m.absolute = false,
                ('G', 100 | 170 | 180 | 190 | 280 | 281 | 300 | 301 | 382..=385 | 400 | 431 | 490 | 530..=590 | 610 | 800 | 911 | 920 | 921 | 930 | 940) => {}
                ('M', 30) => m.spindle = 3,
                ('M', 40) => m.spindle = 4,
                ('M', 50 | 20 | 300) => m.spindle = 0,
                ('M', 70 | 80) => m.air = true,
                ('M', 90) => m.air = false,
                ('M', 0 | 10) => {}
                ('G' | 'M', _) => return Err(20),
                ('S', _) => m.s = w.value,
                ('F', _) => m.f = w.value,
                ('X', _) => x = Some(w.value),
                ('Y', _) => y = Some(w.value),
                ('P', _) => p = Some(w.value),
                ('Z' | 'I' | 'J' | 'K' | 'R' | 'L' | 'N' | 'T', _) => {}
                _ => return Err(20),
            }
        }
        if (x.is_some() || y.is_some()) && m.motion != 0 && m.f <= 0.0 {
            return Err(22);
        }
        self.modal = m;
        if self.check {
            return Ok(());
        }
        let lit = self.modal.spindle != 0 && self.modal.s > 0.0;
        if lit && !self.laser_mode() && !self.report.lit_now {
            self.report.fires.push(Fire { kind: FireKind::Static, from: self.planned, to: self.planned, s: self.modal.s, air: self.modal.air });
        }
        self.report.lit_now = lit && !self.laser_mode();
        if dwell {
            let _secs = p.ok_or(28)?;
            self.push(Block {
                kind: Kind::Dwell,
                from: self.planned,
                to: self.planned,
                spindle: self.modal.spindle,
                s: self.modal.s,
                air: self.modal.air,
            });
        } else if x.is_some() || y.is_some() {
            let to = target(self.planned, self.modal.absolute, x, y);
            let kind = if self.modal.motion == 0 { Kind::Rapid } else { Kind::Linear };
            self.push(Block { kind, from: self.planned, to, spindle: self.modal.spindle, s: self.modal.s, air: self.modal.air });
        }
        Ok(())
    }

    /// Execute the next block, unless held, locked or the door is open.
    fn tick(&mut self) {
        if self.alarm || self.held || self.door || self.stalled {
            return;
        }
        let Some(b) = self.planner.pop_front() else { return };
        self.report.blocks_executed += 1;
        let on = b.spindle != 0 && b.s > 0.0;
        let laser_mode = self.laser_mode();
        let kind = match b.kind {
            Kind::Linear if on => Some(FireKind::Cut),
            Kind::Rapid if on && !laser_mode => Some(FireKind::Rapid),
            Kind::Jog if on => Some(FireKind::Jog),
            Kind::Dwell if on && (b.spindle == 3 || !laser_mode) => Some(FireKind::Dwell),
            _ => None,
        };
        if let Some(kind) = kind {
            self.report.fires.push(Fire { kind, from: b.from, to: b.to, s: b.s, air: b.air });
        }
        self.position = b.to;
        self.parse();
    }

    fn snapshot(&self) -> SimGrblReport {
        let mut r = self.report.clone();
        r.position = self.position;
        r.rx_bytes = self.rx.len();
        r.planner_blocks = self.planner.len();
        r.state = self.state_name().to_owned();
        r.air_now = self.modal.air;
        r
    }
}

fn trim(v: f64) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn target(from: [f64; 2], absolute: bool, x: Option<f64>, y: Option<f64>) -> [f64; 2] {
    if absolute {
        [x.unwrap_or(from[0]), y.unwrap_or(from[1])]
    } else {
        [from[0] + x.unwrap_or(0.0), from[1] + y.unwrap_or(0.0)]
    }
}

/// The simulated board, as the [`SerialLink`] a [`super::GrblController`] talks to. See the module docs.
#[derive(Debug)]
pub struct SimGrbl {
    board: Arc<Mutex<Board>>,
}

/// Looks at and pokes the simulated board while a controller owns its link.
#[derive(Debug, Clone)]
pub struct SimGrblHandle {
    board: Arc<Mutex<Board>>,
}

fn lock(b: &Mutex<Board>) -> std::sync::MutexGuard<'_, Board> {
    b.lock().unwrap_or_else(PoisonError::into_inner)
}

impl SimGrbl {
    /// A board with [`SimGrblConfig::default`] (laser mode on, `$30=1000`, no homing).
    pub fn new() -> Self {
        Self::with_config(&SimGrblConfig::default())
    }

    pub fn with_config(cfg: &SimGrblConfig) -> Self {
        SimGrbl { board: Arc::new(Mutex::new(Board::new(cfg))) }
    }

    pub fn handle(&self) -> SimGrblHandle {
        SimGrblHandle { board: self.board.clone() }
    }
}

impl Default for SimGrbl {
    fn default() -> Self {
        Self::new()
    }
}

impl SimGrblHandle {
    /// Everything so far.
    pub fn report(&self) -> SimGrblReport {
        lock(&self.board).snapshot()
    }

    /// Open or close the safety door. Opening it while moving holds the motion (state `Door`); closing it lets a
    /// resume (`~`) continue.
    pub fn set_door(&self, open: bool) {
        let mut b = lock(&self.board);
        if open && !b.planner.is_empty() {
            b.held = true;
        }
        b.door = open;
    }

    /// Change a setting as if typed on another terminal (`$32=0` turns laser mode off).
    pub fn set_setting(&self, n: u16, v: f64) {
        lock(&self.board).settings.insert(n, v);
    }

    /// Set a start-up block as if stored by another program.
    pub fn set_startup(&self, i: usize, block: &str) {
        lock(&self.board).startup[i.min(1)] = block.to_owned();
    }

    /// Execute up to `n` planner blocks (as if time passed without the host reading).
    pub fn run_blocks(&self, n: usize) {
        let mut b = lock(&self.board);
        for _ in 0..n {
            b.tick();
        }
    }

    /// Stall execution (a long, slow move): the planner keeps its blocks and the state stays Run until
    /// `stall(false)`.
    pub fn stall(&self, on: bool) {
        lock(&self.board).stalled = on;
    }

    /// Answer every line containing `text` with `error:code` (a line the board cannot plan).
    pub fn reject_line(&self, text: &str, code: u8) {
        lock(&self.board).reject = Some((text.to_owned(), code));
    }

    /// Lose every feed hold (`!`) and soft reset (0x18) byte while `on`, as on a flaky cable: the board keeps
    /// running its buffers, keeps its parser state and never sends a start-up message.
    pub fn lose_stop_bytes(&self, on: bool) {
        lock(&self.board).lose_stop_bytes = on;
    }

    /// Every transfer fails from now on.
    pub fn disconnect(&self) {
        lock(&self.board).disconnected = true;
    }
}

impl SerialLink for SimGrbl {
    fn write(&mut self, bytes: &[u8]) -> Result<(), LinkError> {
        let mut b = lock(&self.board);
        if b.disconnected {
            return Err(LinkError::Disconnected);
        }
        for &byte in bytes {
            if matches!(byte, SOFT_RESET | FEED_HOLD) && b.lose_stop_bytes {
                continue;
            }
            if matches!(byte, STATUS | FEED_HOLD | CYCLE_START | SOFT_RESET) || byte >= 0x80 {
                b.realtime(byte);
            } else {
                b.rx.push_back(byte);
                if b.rx.len() > RX_BUFFER {
                    b.report.overflowed = true;
                }
                b.report.max_rx_bytes = b.report.max_rx_bytes.max(b.rx.len());
                if b.rx.len() > RX_CAPACITY {
                    // A real board would lose bytes here; the overflow is recorded above.
                    b.rx.pop_back();
                }
            }
        }
        b.parse();
        Ok(())
    }

    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, LinkError> {
        let n = {
            let mut b = lock(&self.board);
            if b.disconnected {
                return Err(LinkError::Disconnected);
            }
            // Each read is a moment of machine time: one planner block runs.
            b.tick();
            let n = buf.len().min(b.out.len());
            for (i, byte) in b.out.drain(..n).enumerate() {
                buf[i] = byte;
            }
            n
        };
        if n == 0 && !timeout.is_zero() {
            std::thread::sleep(timeout.min(Duration::from_millis(1)));
        }
        Ok(n)
    }
}
