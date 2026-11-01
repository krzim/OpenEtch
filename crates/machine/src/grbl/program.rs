//! G-code words, the [`GcodeProgram`] a GRBL run streams, and [`check_program`]: the last check of a program
//! against the [`GrblProfile`]'s hard limits right before it is sent (the GRBL counterpart of
//! [`crate::check_job`]).

use super::profile::GrblProfile;
use super::protocol::RX_BUFFER;

/// One G-code word: a letter and its number (`G1`, `X12.5`, `S300`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Word {
    pub letter: char,
    pub value: f64,
}

impl Word {
    /// `G` or `M` with this code (`G28.1` is `is('G', 28.1)`).
    pub fn is(&self, letter: char, code: f64) -> bool {
        self.letter == letter && (self.value - code).abs() < 1e-6
    }
}

/// A line without its comments (`(...)` and `; ...`), upper-cased, spaces removed. Refused: unbalanced or nested
/// parentheses.
pub fn strip_comments(line: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut depth = 0;
    for c in line.chars() {
        match c {
            '(' if depth == 0 => depth = 1,
            '(' => return Err("nested comment".into()),
            ')' if depth == 1 => depth = 0,
            ')' => return Err("')' without '('".into()),
            ';' if depth == 0 => break,
            _ if depth > 0 => {}
            c if c.is_whitespace() => {}
            c => out.push(c.to_ascii_uppercase()),
        }
    }
    if depth != 0 {
        return Err("unclosed comment".into());
    }
    Ok(out)
}

/// The words of a G-code line (comments stripped, see [`strip_comments`]).
pub fn parse_words(line: &str) -> Result<Vec<Word>, String> {
    let s = strip_comments(line)?;
    let b = s.as_bytes();
    let mut words = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let letter = b[i] as char;
        if !letter.is_ascii_uppercase() {
            return Err(format!("'{letter}' where a word letter was expected"));
        }
        i += 1;
        let start = i;
        while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.' || b[i] == b'-' || b[i] == b'+') {
            i += 1;
        }
        let num = &s[start..i];
        let value: f64 = num.parse().map_err(|_| format!("'{letter}' has no usable number ({num:?})"))?;
        if !value.is_finite() {
            return Err(format!("'{letter}{num}' is not a finite number"));
        }
        words.push(Word { letter, value });
    }
    Ok(words)
}

/// A G-code program for a GRBL laser: plain lines, no line endings, no comments.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GcodeProgram {
    pub lines: Vec<String>,
}

impl GcodeProgram {
    pub fn new(lines: Vec<String>) -> Self {
        GcodeProgram { lines }
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

/// Why a program was refused (nothing has been sent).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("G-code line {line}: {reason}")]
pub struct ProgramError {
    /// 1-based.
    pub line: usize,
    pub reason: String,
}

/// Most lines one program may have.
pub const MAX_PROGRAM_LINES: usize = 4_000_000;

/// Check `program` against `profile` right before it is sent. Accepted, and nothing else:
///
/// - printable ASCII lines that fit GRBL's receive buffer with their newline, no realtime characters (`?`, `!`,
///   `~`), no comments, no `$` system commands;
/// - `G0`, `G1`, `G4 P<s>` (0..=60 s), `G21` and `G90` (millimetres, absolute: required before the first motion),
///   `M3` / `M4` / `M5`, `S` within `0..=max_power_pct` of `$30`, `F` within `0..=max_feed_mm_min` (and set before
///   the first `G1`), `X` / `Y` inside the bed (machine coordinates, mm from the origin corner);
/// - no `G4` dwell while constant power (`M3`) is on with `S > 0` (the laser would fire standing still);
/// - the laser is off (`M5`) at the end.
pub fn check_program(profile: &GrblProfile, program: &GcodeProgram) -> Result<(), ProgramError> {
    if program.is_empty() {
        return Err(ProgramError { line: 0, reason: "the program is empty".into() });
    }
    if program.len() > MAX_PROGRAM_LINES {
        return Err(ProgramError { line: 0, reason: format!("more than {MAX_PROGRAM_LINES} lines") });
    }
    let [w, h] = profile.work_area_mm;
    let s_limit = profile.s_limit() + 1e-6;
    let (mut mm, mut absolute, mut feed_set) = (false, false, false);
    // 0 = M5, 3 = M3, 4 = M4.
    let (mut spindle, mut s, mut motion) = (5u8, 0.0f64, None::<u8>);
    for (i, line) in program.lines.iter().enumerate() {
        let n = i + 1;
        let err = |reason: String| ProgramError { line: n, reason };
        if line.len() + 1 > RX_BUFFER {
            return Err(err(format!("{} characters is longer than GRBL's buffer allows", line.len())));
        }
        if !line.bytes().all(|b| (0x20..0x7f).contains(&b)) || line.contains(['?', '!', '~', '(', ')', ';', '$', '%']) {
            return Err(err(format!("\"{line}\" holds a character a program may not send")));
        }
        let words = parse_words(line).map_err(err)?;
        let mut dwell = false;
        let mut p = None;
        let (mut x, mut y) = (None, None);
        for wd in &words {
            match wd.letter {
                'G' if wd.is('G', 0.0) => motion = Some(0),
                'G' if wd.is('G', 1.0) => motion = Some(1),
                'G' if wd.is('G', 4.0) => dwell = true,
                'G' if wd.is('G', 21.0) => mm = true,
                'G' if wd.is('G', 90.0) => absolute = true,
                'M' if wd.is('M', 3.0) => spindle = 3,
                'M' if wd.is('M', 4.0) => spindle = 4,
                'M' if wd.is('M', 5.0) => spindle = 5,
                'S' => {
                    if !(0.0..=s_limit).contains(&wd.value) {
                        return Err(err(format!("S{} is above the limit S{:.3} ({} % of $30)", wd.value, profile.s_limit(), profile.max_power_pct)));
                    }
                    s = wd.value;
                }
                'F' => {
                    if !(wd.value > 0.0 && wd.value <= profile.max_feed_mm_min + 1e-6) {
                        return Err(err(format!("F{} is outside 0..={} mm/min", wd.value, profile.max_feed_mm_min)));
                    }
                    feed_set = true;
                }
                'X' => x = Some(wd.value),
                'Y' => y = Some(wd.value),
                'P' => p = Some(wd.value),
                _ => return Err(err(format!("{}{} is not allowed in a program", wd.letter, wd.value))),
            }
        }
        if dwell {
            let secs = p.ok_or_else(|| err("G4 needs P (seconds)".into()))?;
            if !(0.0..=60.0).contains(&secs) {
                return Err(err(format!("G4 P{secs}: a dwell must be 0..=60 s")));
            }
            if spindle == 3 && s > 0.0 {
                return Err(err("G4 while constant power (M3) is on would fire in place".into()));
            }
            if x.is_some() || y.is_some() {
                return Err(err("G4 with axis words".into()));
            }
        } else if p.is_some() {
            return Err(err("P without G4".into()));
        }
        if x.is_some() || y.is_some() {
            if !(mm && absolute) {
                return Err(err("motion before G21 and G90 (millimetres, absolute)".into()));
            }
            match motion {
                Some(1) if !feed_set => return Err(err("G1 before any feed (F)".into())),
                Some(_) => {}
                None => return Err(err("axis words without G0 or G1".into())),
            }
            for (v, max, axis) in [(x, w, 'X'), (y, h, 'Y')] {
                if let Some(v) = v
                    && !(-1e-6..=max + 1e-6).contains(&v)
                {
                    return Err(err(format!("{axis}{v} is outside the bed (0..={max} mm)")));
                }
            }
        }
    }
    if spindle != 5 {
        return Err(ProgramError { line: program.len(), reason: "the program must end with the laser off (M5)".into() });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prog(lines: &[&str]) -> GcodeProgram {
        GcodeProgram::new(lines.iter().map(|s| (*s).to_owned()).collect())
    }

    #[test]
    fn words_and_comments() {
        let w = parse_words("g1 x10.5 Y-2 (move) s300 ; tail").unwrap();
        assert_eq!(w.len(), 4);
        assert!(w[0].is('G', 1.0) && w[2] == Word { letter: 'Y', value: -2.0 } && w[3].value == 300.0);
        assert!(parse_words("G1 X(").is_err());
        assert!(parse_words("G1 X").is_err());
        assert!(parse_words("(a(b))").is_err());
        assert_eq!(strip_comments("M3 (M5) S0").unwrap(), "M3S0");
    }

    #[test]
    fn programs_are_checked_against_the_profile() {
        let p = GrblProfile::default();
        let good = prog(&["G90 G21", "M4 S0", "G0 X10 Y10", "G1 X20 Y10 S500 F1200", "G1 X20 Y20", "M5"]);
        check_program(&p, &good).unwrap();
        for (bad, why) in [
            (vec!["G90 G21", "M4 S0", "G0 X10 Y10", "G1 X20 Y10 S1001 F1200", "M5"], "above the limit"),
            (vec!["G90 G21", "M4 S0", "G1 X20 Y10 S10 F7000", "M5"], "mm/min"),
            (vec!["G90 G21", "M4", "G1 X500 Y10 S10 F100", "M5"], "outside the bed"),
            (vec!["G90 G21", "M4 S10", "G1 X5 Y5 F100"], "M5"),
            (vec!["M4 S10", "G1 X5 Y5 F100", "M5"], "G21 and G90"),
            (vec!["G90 G21", "M4 S10", "G1 X5 Y5", "M5"], "feed"),
            (vec!["G90 G21", "M3 S10", "G4 P1", "M5"], "fire in place"),
            (vec!["G90 G21", "$H", "M5"], "character"),
            (vec!["G90 G21", "G1 X1 Y1 F100 ?", "M5"], "character"),
            (vec!["G90 G21", "G91", "M5"], "not allowed"),
            (vec!["G90 G21", "G0 Z5", "M5"], "not allowed"),
        ] {
            let e = check_program(&p, &prog(&bad)).unwrap_err();
            assert!(e.to_string().contains(why), "{bad:?}: {e}");
        }
        let half = GrblProfile { max_power_pct: 50.0, ..p.clone() };
        assert!(check_program(&half, &good).is_ok());
        assert!(check_program(&half, &prog(&["G90 G21", "M4 S501", "M5"])).is_err());
        check_program(&p, &prog(&["G90 G21", "M4 S300", "G4 P0.5", "M5"])).unwrap();
        let long = format!("G1 X1 Y1 F100{}", " ".repeat(120));
        assert!(check_program(&p, &prog(&["G90 G21", &long, "M5"])).is_err());
    }
}
