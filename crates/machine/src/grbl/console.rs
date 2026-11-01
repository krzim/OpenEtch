//! The console's line policy: what a person may type to a GRBL board outside an armed run.
//!
//! The console never runs while a run holds the machine, so every console line is unarmed, and an unarmed line must
//! not be able to fire the laser, now or later. A GRBL laser fires when its spindle state is on (`M3` or `M4`) with
//! `S > 0` and it moves with `G1`/`G2`/`G3`, dwells (`G4`) in constant-power `M3`, or, with laser mode off (`$32=0`),
//! at any time at all. So the policy keeps the spindle off and the power at zero instead of judging single moves:
//!
//! - **G-code**: no `M3`/`M4` at all (an `M3 S0` would arm a later `S` word), no `S` above 0, only G and M codes that
//!   cannot turn the laser on (an allowlist), only known word letters. Motion (`G0`/`G1`/arcs, homing moves, probing)
//!   is allowed: with the spindle off it cannot fire. The controller also checks that the board's parser state says
//!   `M5` before it sends a line that moves ([`super::GrblController::console`]).
//! - **System (`$`) commands**: the queries (`$`, `$$`, `$#`, `$G`, `$I`, `$N`), unlock `$X`, home `$H`, check mode
//!   `$C`, sleep `$SLP`, jogs `$J=` (whose words GRBL limits to axes, feed, units and distance mode), setting writes
//!   `$<n>=<value>`, and start-up blocks `$N0=` / `$N1=` only when the block itself passes the G-code rule (start-up
//!   blocks run after every reset, including the stop's). Refused: `$RST=` (it restores defaults, e.g. laser mode
//!   off) and anything else.
//! - **Characters**: printable ASCII only, and no `?`, `!` or `~`: GRBL acts on those immediately wherever they
//!   appear (status, feed hold, resume), so they have their own buttons; bytes from 0x80 up (overrides) are refused
//!   with every other non-printable byte.
//!
//! Setting writes can change `$30`/`$31`/`$32`; every run re-reads them and is refused unless laser mode is on and
//! the power range matches the profile, so a changed setting can stop a run but never make one fire differently.

use super::program::{Word, parse_words};

/// Longest console line.
pub const MAX_CONSOLE_LINE: usize = 120;

/// G codes a console line may use: motion with the spindle off, dwell, planes, units, homing and probing moves,
/// offsets and coordinate systems, path control, distance and feed modes.
const G_ALLOWED: &[f64] = &[
    0.0, 1.0, 2.0, 3.0, 4.0, 10.0, 17.0, 18.0, 19.0, 20.0, 21.0, 28.0, 28.1, 30.0, 30.1, 38.2, 38.3, 38.4, 38.5, 40.0, 43.1,
    49.0, 53.0, 54.0, 55.0, 56.0, 57.0, 58.0, 59.0, 61.0, 80.0, 90.0, 91.0, 91.1, 92.0, 92.1, 93.0, 94.0,
];
/// M codes a console line may use: pause, program end, spindle off, coolant (air assist).
const M_ALLOWED: &[f64] = &[0.0, 1.0, 2.0, 5.0, 7.0, 8.0, 9.0, 30.0];

/// What an accepted console line is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsoleKind {
    /// A `$` query or setting: no motion.
    System,
    /// `$H`: the homing cycle (moves, never fires).
    Home,
    /// `$X`: unlock after an alarm.
    Unlock,
    /// `$J=`: a jog (moves, never fires).
    Jog,
    /// A G-code line; `moves` when it has a motion word or code.
    Gcode { moves: bool },
}

fn gcode_rule(words: &[Word]) -> Result<bool, String> {
    let mut moves = false;
    for w in words {
        match w.letter {
            'M' if w.is('M', 3.0) || w.is('M', 4.0) => {
                return Err(format!("M{} turns the laser on; it can only be sent by an approved run", w.value));
            }
            'M' if !M_ALLOWED.iter().any(|c| w.is('M', *c)) => return Err(format!("M{} is not allowed in the console", w.value)),
            'G' if !G_ALLOWED.iter().any(|c| w.is('G', *c)) => return Err(format!("G{} is not allowed in the console", w.value)),
            'S' if w.value > 0.0 => return Err(format!("S{} sets laser power; only an approved run may", w.value)),
            'S' if w.value < 0.0 => return Err("S must not be negative".into()),
            'G' => moves |= [0.0, 1.0, 2.0, 3.0, 28.0, 30.0].iter().any(|c| w.is('G', *c)) || (38.0..39.0).contains(&w.value),
            'X' | 'Y' | 'Z' => moves = true,
            'M' | 'S' | 'F' | 'I' | 'J' | 'K' | 'L' | 'N' | 'P' | 'R' | 'T' => {}
            c => return Err(format!("word letter '{c}' is not allowed in the console")),
        }
    }
    Ok(moves)
}

/// Check one console line (see the module docs). Returns the line to send (trimmed) and what it is.
pub fn check_console_line(line: &str) -> Result<(String, ConsoleKind), String> {
    let t = line.trim();
    if t.is_empty() {
        return Err("empty line".into());
    }
    if t.len() > MAX_CONSOLE_LINE {
        return Err(format!("longer than {MAX_CONSOLE_LINE} characters"));
    }
    if let Some(b) = t.bytes().find(|b| !(0x20..0x7f).contains(b)) {
        return Err(format!("byte 0x{b:02x} is not printable ASCII (realtime commands have their own buttons)"));
    }
    if let Some(c) = t.chars().find(|c| matches!(c, '?' | '!' | '~')) {
        return Err(format!("'{c}' is a realtime command GRBL acts on immediately; use the Status, Hold and Resume buttons"));
    }
    let kind = if let Some(sys) = t.strip_prefix('$') {
        let up = sys.to_ascii_uppercase();
        match up.as_str() {
            "" | "$" | "#" | "G" | "I" | "N" | "C" | "SLP" => ConsoleKind::System,
            "X" => ConsoleKind::Unlock,
            "H" => ConsoleKind::Home,
            _ if up.starts_with("J=") => {
                let words = parse_words(&sys[2..]).map_err(|e| format!("jog: {e}"))?;
                for w in &words {
                    let ok = match w.letter {
                        'G' => [20.0, 21.0, 53.0, 90.0, 91.0].iter().any(|c| w.is('G', *c)),
                        'X' | 'Y' | 'Z' | 'F' => true,
                        _ => false,
                    };
                    if !ok {
                        return Err(format!("jog: {}{} is not allowed (axes, F, G20/G21, G53, G90/G91 only)", w.letter, w.value));
                    }
                }
                ConsoleKind::Jog
            }
            _ if up.starts_with("RST=") => {
                return Err("$RST restores GRBL's defaults (laser mode off, another power range); do it with GRBL's own tools".into());
            }
            _ if up.starts_with("N0=") || up.starts_with("N1=") => {
                gcode_rule(&parse_words(&sys[3..]).map_err(|e| format!("start-up block: {e}"))?)
                    .map_err(|e| format!("start-up block (runs after every reset): {e}"))?;
                ConsoleKind::System
            }
            _ => {
                let (n, v) = up.split_once('=').ok_or_else(|| format!("${sys} is not an allowed system command"))?;
                if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) || v.trim().parse::<f64>().is_err() {
                    return Err(format!("${sys} is not an allowed system command"));
                }
                ConsoleKind::System
            }
        }
    } else {
        let words = parse_words(t)?;
        ConsoleKind::Gcode { moves: gcode_rule(&words)? }
    };
    Ok((t.to_owned(), kind))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_that_can_fire_gets_through() {
        for bad in [
            "M3 S1000", "M4 S1", "M3", "M4 S0", "m3s500", "S100", "G1 X10 F100 S5", "M3 (comment) S0", "G1 X1 ?", "!", "~",
            "\u{18}", "\u{85}", "$N0=M3 S1000", "$N1=S10", "$RST=*", "$RST=$", "$J=G91 X10 F100 S100", "$J=X1 F100 M3", "$Q",
            "$100=abc", "M62 P1", "G4 P1 M3", "Q5", "G1 X1 F100 (unclosed", "",
        ] {
            assert!(check_console_line(bad).is_err(), "{bad:?} should be refused");
        }
        for (good, kind) in [
            ("$$", ConsoleKind::System),
            ("$G", ConsoleKind::System),
            ("$X", ConsoleKind::Unlock),
            ("$h", ConsoleKind::Home),
            ("$J=G91 G21 X10 Y-5 F1000", ConsoleKind::Jog),
            ("$N0=", ConsoleKind::System),
            ("$N0=G21 G90", ConsoleKind::System),
            ("$110=5000", ConsoleKind::System),
            ("$32=1", ConsoleKind::System),
            ("G0 X10 Y10", ConsoleKind::Gcode { moves: true }),
            ("G1 X10 F500 S0", ConsoleKind::Gcode { moves: true }),
            ("M5", ConsoleKind::Gcode { moves: false }),
            ("G90 G21", ConsoleKind::Gcode { moves: false }),
            ("G92 X0 Y0", ConsoleKind::Gcode { moves: true }),
            ("M8", ConsoleKind::Gcode { moves: false }),
        ] {
            assert_eq!(check_console_line(good).map(|(_, k)| k), Ok(kind), "{good}");
        }
    }
}
