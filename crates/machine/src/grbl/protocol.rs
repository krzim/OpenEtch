//! GRBL 1.1's serial protocol as OpenEtch uses it: realtime bytes, the lines a board sends back, status reports and
//! what error and alarm codes mean.
//!
//! Protocol facts only, from GRBL's documentation (the "Grbl v1.1 Interface", "Grbl v1.1 Commands" and "Grbl v1.1
//! Laser Mode" wiki pages and the error/alarm code tables in its `doc/csv` folder, listed in ATTRIBUTION.md). No GRBL
//! code is used; the descriptions are OpenEtch's own wording.

/// Bytes of GRBL's serial receive buffer a host may fill (its 128-byte buffer less one), the limit of the
/// character-counting streaming protocol.
pub const RX_BUFFER: usize = 127;

/// Realtime command: status report request.
pub const STATUS: u8 = b'?';
/// Realtime command: feed hold (decelerate to a stop and hold).
pub const FEED_HOLD: u8 = b'!';
/// Realtime command: cycle start / resume.
pub const CYCLE_START: u8 = b'~';
/// Realtime command: soft reset (stops everything, clears the buffers, turns the spindle/laser off).
pub const SOFT_RESET: u8 = 0x18;
/// Realtime command: jog cancel (stops and flushes jog motion without an alarm).
pub const JOG_CANCEL: u8 = 0x85;

/// Bytes GRBL acts on wherever they appear in the stream, never as part of a line: the realtime commands above,
/// and every byte from 0x80 up (overrides and other extended realtime commands).
pub fn is_realtime(b: u8) -> bool {
    matches!(b, STATUS | FEED_HOLD | CYCLE_START | SOFT_RESET) || b >= 0x80
}

/// The machine state of a status report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrblState {
    Idle,
    Run,
    /// Feed hold (`Hold:0` complete, `Hold:1` decelerating).
    Hold,
    Jog,
    Alarm,
    /// The safety door is (or was) open (`Door:0..3`).
    Door,
    Check,
    Home,
    Sleep,
}

impl GrblState {
    pub fn parse(s: &str) -> Option<Self> {
        let base = s.split(':').next().unwrap_or("");
        Some(match base {
            "Idle" => GrblState::Idle,
            "Run" => GrblState::Run,
            "Hold" => GrblState::Hold,
            "Jog" => GrblState::Jog,
            "Alarm" => GrblState::Alarm,
            "Door" => GrblState::Door,
            "Check" => GrblState::Check,
            "Home" => GrblState::Home,
            "Sleep" => GrblState::Sleep,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            GrblState::Idle => "Idle",
            GrblState::Run => "Run",
            GrblState::Hold => "Hold",
            GrblState::Jog => "Jog",
            GrblState::Alarm => "Alarm",
            GrblState::Door => "Door",
            GrblState::Check => "Check",
            GrblState::Home => "Home",
            GrblState::Sleep => "Sleep",
        }
    }
}

/// One status report (`<Idle|MPos:...|FS:...|Pn:...>`): the fields OpenEtch reads.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StatusReport {
    pub state: GrblState,
    /// Machine position, mm (from `MPos`, or `WPos` plus the last `WCO`).
    pub mpos: Option<[f64; 3]>,
    /// Work position, mm (from `WPos`, or `MPos` less the last `WCO`).
    pub wpos: Option<[f64; 3]>,
    /// Feed (mm/min) and spindle/laser S from `FS` (or the feed from `F`).
    pub feed: Option<f64>,
    pub spindle: Option<f64>,
    /// Input pins from `Pn` (`D` is the safety door).
    pub pins: String,
    /// `wpos` is the board's own (a `WPos` field, or `MPos` with a `WCO` seen): not `MPos` assumed to have no work
    /// offset because no `WCO` has come yet.
    #[serde(default)]
    pub wpos_known: bool,
}

impl StatusReport {
    /// The safety door is open: the `Door` state, or `D` among the input pins.
    pub fn door_open(&self) -> bool {
        self.state == GrblState::Door || self.pins.contains('D')
    }
}

fn floats(s: &str) -> Option<[f64; 3]> {
    let v: Vec<f64> = s.split(',').map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
    match v.as_slice() {
        [x, y] => Some([*x, *y, 0.0]),
        [x, y, z, ..] => Some([*x, *y, *z]),
        _ => None,
    }
}

/// Parse `<...>`; `wco` is the last work coordinate offset seen (GRBL sends it only now and then) and is updated.
pub fn parse_status(line: &str, wco: &mut Option<[f64; 3]>) -> Option<StatusReport> {
    let body = line.trim().strip_prefix('<')?.strip_suffix('>')?;
    let mut parts = body.split('|');
    let state = GrblState::parse(parts.next()?)?;
    let mut r = StatusReport { state, mpos: None, wpos: None, feed: None, spindle: None, pins: String::new(), wpos_known: false };
    for p in parts {
        let (k, v) = p.split_once(':').unwrap_or((p, ""));
        match k {
            "MPos" => r.mpos = floats(v),
            "WPos" => r.wpos = floats(v),
            "WCO" => *wco = floats(v),
            "FS" => {
                let mut it = v.split(',');
                r.feed = it.next().and_then(|f| f.parse().ok());
                r.spindle = it.next().and_then(|s| s.parse().ok());
            }
            "F" => r.feed = v.parse().ok(),
            "Pn" => r.pins = v.to_owned(),
            _ => {}
        }
    }
    r.wpos_known = r.wpos.is_some() || (r.mpos.is_some() && wco.is_some());
    if let Some(o) = *wco {
        match (r.mpos, r.wpos) {
            (Some(m), None) => r.wpos = Some([m[0] - o[0], m[1] - o[1], m[2] - o[2]]),
            (None, Some(w)) => r.mpos = Some([w[0] + o[0], w[1] + o[1], w[2] + o[2]]),
            _ => {}
        }
    } else if r.wpos.is_none() {
        r.wpos = r.mpos;
    }
    Some(r)
}

/// One line from the board.
#[derive(Debug, Clone, PartialEq)]
pub enum Response {
    /// A line was accepted.
    Ok,
    /// A line was refused (`error:N`).
    Error(u8),
    /// An alarm (`ALARM:N`): the board is locked until unlocked or homed.
    Alarm(u8),
    Status(StatusReport),
    /// The start-up message (`Grbl 1.1h ['$' for help]`) after a reset, with its version.
    Welcome(String),
    /// Any other line: `[MSG:...]`, `[GC:...]`, `$n=v` settings, `$Nn=` start-up blocks, `>...:ok` start-up echoes.
    Other(String),
}

/// Classify one line (without its line ending).
pub fn parse_line(line: &str, wco: &mut Option<[f64; 3]>) -> Response {
    let t = line.trim();
    if t == "ok" {
        return Response::Ok;
    }
    if let Some(n) = t.strip_prefix("error:") {
        return Response::Error(n.trim().parse().unwrap_or(0));
    }
    if let Some(n) = t.strip_prefix("ALARM:") {
        return Response::Alarm(n.trim().parse().unwrap_or(0));
    }
    if t.starts_with('<')
        && let Some(r) = parse_status(t, wco)
    {
        return Response::Status(r);
    }
    if let Some(rest) = t.strip_prefix("Grbl ") {
        return Response::Welcome(rest.split_whitespace().next().unwrap_or("").to_owned());
    }
    Response::Other(t.to_owned())
}

/// What a GRBL 1.1 error code means.
pub fn error_text(code: u8) -> &'static str {
    match code {
        1 => "a word has no letter",
        2 => "a number is badly formatted",
        3 => "unknown $ command",
        4 => "a value must not be negative",
        5 => "homing is not enabled ($22)",
        6 => "step pulse below 3 microseconds",
        7 => "settings could not be read from EEPROM",
        8 => "this $ command needs the machine idle",
        9 => "G-code is locked out (alarm or jog state): home or unlock first",
        10 => "soft limits need homing enabled",
        11 => "line too long",
        12 => "step rate above the board's maximum",
        13 => "the safety door is open",
        14 => "line too long for EEPROM",
        15 => "jog target beyond the machine travel",
        16 => "invalid jog command",
        17 => "laser mode needs PWM output",
        20 => "unsupported G-code command",
        21 => "two commands of one modal group in a line",
        22 => "feed rate not set",
        23 => "a command value must be an integer",
        24 => "two commands in a line both need axis words",
        25 => "a word is repeated in the line",
        26 => "no axis words where some are needed",
        27 => "invalid line number",
        28 => "a required value word is missing",
        29 => "work coordinate system not supported",
        30 => "G53 needs G0 or G1",
        31 => "axis words that no command uses",
        32 => "arc without an axis word in its plane",
        33 => "invalid motion target",
        34 => "invalid arc radius",
        35 => "arc without an offset in its plane",
        36 => "value words that no command uses",
        37 => "tool length offset on the wrong axis",
        38 => "tool number too high",
        _ => "unknown error",
    }
}

/// What a GRBL 1.1 alarm code means.
pub fn alarm_text(code: u8) -> &'static str {
    match code {
        1 => "hard limit switch triggered: position lost, home again",
        2 => "soft limit: the target is outside the machine travel",
        3 => "reset while moving: position may be lost, home or unlock",
        4 => "probe was not in its expected state",
        5 => "probe did not make contact",
        6 => "homing failed: reset during homing",
        7 => "homing failed: the safety door opened",
        8 => "homing failed: pull-off did not clear the limit switch",
        9 => "homing failed: limit switch not found",
        _ => "unknown alarm",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_classified() {
        let mut wco = None;
        assert_eq!(parse_line("ok\r", &mut wco), Response::Ok);
        assert_eq!(parse_line("error:20", &mut wco), Response::Error(20));
        assert_eq!(parse_line("ALARM:3", &mut wco), Response::Alarm(3));
        assert_eq!(parse_line("Grbl 1.1h ['$' for help]", &mut wco), Response::Welcome("1.1h".into()));
        assert!(matches!(parse_line("[MSG:'$H'|'$X' to unlock]", &mut wco), Response::Other(_)));
        let Response::Status(s) = parse_line("<Hold:0|MPos:1.000,2.500,0.000|FS:0,0|Pn:D|WCO:1.000,0.500,0.000>", &mut wco) else {
            panic!()
        };
        assert_eq!((s.state, s.mpos, s.wpos), (GrblState::Hold, Some([1.0, 2.5, 0.0]), Some([0.0, 2.0, 0.0])));
        assert!(s.door_open());
        let Response::Status(s) = parse_line("<Door:1|WPos:0.000,0.000,0.000|F:500>", &mut wco) else { panic!() };
        assert_eq!((s.state, s.feed, s.mpos), (GrblState::Door, Some(500.0), Some([1.0, 0.5, 0.0])));
        assert!(s.door_open());
        assert!(is_realtime(b'!') && is_realtime(0x85) && !is_realtime(b'G'));
        assert_eq!(error_text(9).split(':').next(), Some("G-code is locked out (alarm or jog state)"));
        assert!(alarm_text(3).contains("reset while moving"));
    }
}
