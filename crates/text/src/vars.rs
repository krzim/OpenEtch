//! Variable text, following LightBurn's Variable Text Formatting reference (docs 2.1): date/time codes, serial
//! number codes, CSV columns (`%0`, `%1`, ...) and cut-setting codes. In every mode text in single quotes is copied
//! as it is (`''` is one quote) and other characters pass through. A text's value is the document's current serial
//! number or CSV row plus the text's offset.

use chrono::{Datelike, NaiveDateTime, Timelike};
use doc::{CsvData, Document, Layer, VarMode, VarText, VariableState};

/// The moment a date/time text shows, and its time zone's name (LightBurn's `t`).
#[derive(Debug, Clone, PartialEq)]
pub struct Moment {
    pub local: NaiveDateTime,
    pub zone: String,
}

impl Moment {
    /// Now, in local time (native builds). The zone is written as an offset, e.g. `UTC+02:00`.
    #[cfg(feature = "system")]
    pub fn now() -> Moment {
        let now = chrono::Local::now();
        let off = now.offset().local_minus_utc();
        let (sign, off) = if off < 0 { ('-', -off) } else { ('+', off) };
        Moment { local: now.naive_local(), zone: format!("UTC{sign}{:02}:{:02}", off / 3600, off % 3600 / 60) }
    }
}

/// Split a format into literal and code runs: quoted text is literal, `''` is a quote.
fn quoted(format: &str, mut code: impl FnMut(&[char], &mut usize, &mut String)) -> String {
    let chars: Vec<char> = format.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\'' {
            if chars.get(i + 1) == Some(&'\'') {
                out.push('\'');
                i += 2;
                continue;
            }
            i += 1;
            while i < chars.len() {
                if chars[i] == '\'' {
                    if chars.get(i + 1) == Some(&'\'') {
                        out.push('\'');
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                out.push(chars[i]);
                i += 1;
            }
            continue;
        }
        code(&chars, &mut i, &mut out);
    }
    out
}

fn run_len(chars: &[char], i: usize) -> usize {
    chars[i..].iter().take_while(|c| **c == chars[i]).count()
}

const WEEKDAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November",
    "December",
];

/// Date/time codes (Qt's `QDateTime::toString` syntax, which LightBurn uses, plus its `w` / `W` ISO week). A run of
/// one letter is read as the longest code it starts with (`ddddd` is `dddd` then `d`). English names.
pub fn format_date(format: &str, m: &Moment) -> String {
    let t = m.local;
    // `h` is a 12-hour clock when the format has an AM/PM code outside quotes.
    let mut twelve = false;
    quoted(format, |chars, i, _| {
        twelve |= matches!(chars[*i], 'a' | 'A');
        *i += 1;
    });
    let h12 = match t.hour() % 12 {
        0 => 12,
        h => h,
    };
    quoted(format, |chars, i, out| {
        let c = chars[*i];
        let n = run_len(chars, *i);
        let (take, text): (usize, String) = match c {
            'd' => match n.min(4) {
                4 => (4, WEEKDAYS[t.weekday().num_days_from_monday() as usize].into()),
                3 => (3, WEEKDAYS[t.weekday().num_days_from_monday() as usize][..3].into()),
                2 => (2, format!("{:02}", t.day())),
                _ => (1, t.day().to_string()),
            },
            'M' => match n.min(4) {
                4 => (4, MONTHS[t.month0() as usize].into()),
                3 => (3, MONTHS[t.month0() as usize][..3].into()),
                2 => (2, format!("{:02}", t.month())),
                _ => (1, t.month().to_string()),
            },
            'y' if n >= 4 => (4, format!("{:04}", t.year())),
            'y' if n >= 2 => (2, format!("{:02}", t.year().rem_euclid(100))),
            'h' => {
                let h = if twelve { h12 } else { t.hour() };
                if n >= 2 { (2, format!("{h:02}")) } else { (1, h.to_string()) }
            }
            'H' => {
                if n >= 2 { (2, format!("{:02}", t.hour())) } else { (1, t.hour().to_string()) }
            }
            'm' => {
                if n >= 2 { (2, format!("{:02}", t.minute())) } else { (1, t.minute().to_string()) }
            }
            's' => {
                if n >= 2 { (2, format!("{:02}", t.second())) } else { (1, t.second().to_string()) }
            }
            'z' => {
                let ms = t.and_utc().timestamp_subsec_millis();
                if n >= 3 {
                    (3, format!("{ms:03}"))
                } else {
                    let s = format!("{ms:03}");
                    let s = s.trim_end_matches('0');
                    (1, if s.is_empty() { "0".into() } else { s.into() })
                }
            }
            'A' | 'a' => {
                let pm = t.hour() >= 12;
                let up = c == 'A';
                let next_p = chars.get(*i + 1).is_some_and(|x| *x == if up { 'P' } else { 'p' });
                let s = match (pm, up) {
                    (true, true) => "PM",
                    (false, true) => "AM",
                    (true, false) => "pm",
                    (false, false) => "am",
                };
                (if next_p { 2 } else { 1 }, s.into())
            }
            't' => (1, m.zone.clone()),
            'w' => (1, t.iso_week().week().to_string()),
            'W' => (1, format!("{}/{:02}", t.iso_week().year(), t.iso_week().week())),
            other => (1, other.to_string()),
        };
        out.push_str(&text);
        *i += take;
    })
}

/// Serial number codes: one run of `d` (decimal), `h` or `H` (hexadecimal) whose length is the digit width, `0`
/// before it pads with zeros; a number wider than the run shows its last digits. Two runs, or decimal and hex
/// mixed, are LightBurn's "bad serial format".
pub fn format_serial(format: &str, value: i64) -> String {
    let mut runs = 0usize;
    let mut kinds = Vec::new();
    let out = quoted(format, |chars, i, out| {
        let c = chars[*i];
        let pad = c == '0' && chars.get(*i + 1).is_some_and(|n| matches!(n, 'd' | 'h' | 'H'));
        let at = if pad { *i + 1 } else { *i };
        let k = chars[at];
        if !matches!(k, 'd' | 'h' | 'H') {
            out.push(c);
            *i += 1;
            return;
        }
        let n = run_len(chars, at);
        runs += 1;
        kinds.push(k);
        let digits = match k {
            'd' => value.unsigned_abs().to_string(),
            'h' => format!("{:x}", value.unsigned_abs()),
            _ => format!("{:X}", value.unsigned_abs()),
        };
        let s = if digits.len() > n { &digits[digits.len() - n..] } else { &digits };
        if value < 0 {
            out.push('-');
        }
        if pad {
            out.push_str(&"0".repeat(n - s.len()));
        }
        out.push_str(s);
        *i = at + n;
    });
    let mixed = kinds.contains(&'d') && kinds.iter().any(|k| *k != 'd');
    if runs > 1 || mixed { "bad serial format".into() } else { out }
}

/// CSV merge: `%N` is column N (from 0) of `row`; `%%` is a percent sign; anything else passes through. A missing
/// column or row is empty.
pub fn format_csv(format: &str, row: Option<&[String]>) -> String {
    let chars: Vec<char> = format.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '%' {
            if chars.get(i + 1) == Some(&'%') {
                out.push('%');
                i += 2;
                continue;
            }
            let digits: String = chars[i + 1..].iter().take_while(|c| c.is_ascii_digit()).collect();
            if !digits.is_empty() {
                let col: usize = digits.parse().unwrap_or(usize::MAX);
                if let Some(v) = row.and_then(|r| r.get(col)) {
                    out.push_str(v);
                }
                i += 1 + digits.len();
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// A layer's values for the cut-setting codes.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CutValues {
    pub speed_mm_s: f64,
    pub power_pct: f64,
    pub min_power_pct: f64,
    pub passes: u32,
    pub interval_mm: f64,
    pub z_offset_mm: f64,
    pub pulse_width_ns: Option<f64>,
    pub frequency_khz: f64,
}

impl CutValues {
    pub fn of(layer: &Layer) -> CutValues {
        let s = &layer.settings;
        let extra = |k: &str| layer.extras.get(k).and_then(|v| v.trim().parse::<f64>().ok());
        CutValues {
            speed_mm_s: s.speed_mm_s,
            power_pct: s.power_pct,
            min_power_pct: extra("lbrn2.minPower").unwrap_or(0.0),
            passes: s.passes,
            interval_mm: s.hatch.interval_mm,
            z_offset_mm: extra("lbrn2.zOffset").unwrap_or(0.0),
            pulse_width_ns: s.pulse_width_ns,
            frequency_khz: s.frequency_khz,
        }
    }
}

/// A number as LightBurn shows it: up to `places` decimals, trailing zeros dropped.
fn num(v: f64, places: usize) -> String {
    let s = format!("{v:.places$}");
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
    if s == "-0" { "0".into() } else { s }
}

/// Cut-setting codes over `layer` (the text's own), or another layer picked with `C##` for the rest of the string.
/// `pick(n)` finds layer `C##`. `laser` is the device name for `L`.
pub fn format_cut(format: &str, own: Option<CutValues>, pick: &dyn Fn(u32) -> Option<CutValues>, laser: &str) -> String {
    let mut current = own;
    quoted(format, |chars, i, out| {
        let c = chars[*i];
        let digits = |from: usize, max: usize| -> String {
            chars.get(from..).unwrap_or_default().iter().take(max).take_while(|c| c.is_ascii_digit()).collect()
        };
        if c == 'C' {
            let d = digits(*i + 1, 2);
            if d.len() == 2 {
                current = pick(d.parse().unwrap_or(0));
                *i += 3;
                return;
            }
        }
        if c == 'L' {
            let start = digits(*i + 1, 6);
            let mut k = *i + 1 + start.len();
            let mut count = None;
            if !start.is_empty() && chars.get(k) == Some(&',') {
                let n = digits(k + 1, 6);
                if !n.is_empty() {
                    count = n.parse::<usize>().ok();
                    k += 1 + n.len();
                }
            }
            let from: usize = start.parse().unwrap_or(0);
            let name: String = laser.chars().skip(from).take(count.unwrap_or(usize::MAX)).collect();
            out.push_str(&name);
            *i = k;
            return;
        }
        let Some(v) = &current else {
            out.push(c);
            *i += 1;
            return;
        };
        let text = match c {
            's' => num(v.speed_mm_s, 2),
            'S' => format!("{} mm/sec", num(v.speed_mm_s, 2)),
            'p' => num(v.power_pct, 2),
            'P' => format!("{}%", num(v.power_pct, 2)),
            'm' => num(v.min_power_pct, 2),
            'M' => format!("{}%", num(v.min_power_pct, 2)),
            'n' => v.passes.to_string(),
            'd' | 'D' => {
                if v.interval_mm > 0.0 {
                    num((25.4 / v.interval_mm).round(), 0)
                } else {
                    "0".into()
                }
            }
            'i' => num(v.interval_mm, 4),
            'I' => format!("{} mm", num(v.interval_mm, 4)),
            'z' => num(v.z_offset_mm, 3),
            'Z' => format!("{} mm", num(v.z_offset_mm, 3)),
            'q' => v.pulse_width_ns.map(|q| num(q, 1)).unwrap_or_default(),
            'Q' => v.pulse_width_ns.map(|q| format!("{}ns", num(q, 1))).unwrap_or_default(),
            'k' => num(v.frequency_khz, 3),
            'K' => format!("{}kHz", num(v.frequency_khz, 3)),
            other => other.to_string(),
        };
        out.push_str(&text);
        *i += 1;
    })
}

/// The row (`index`) of a CSV file, if it has one.
pub fn csv_row(csv: Option<&CsvData>, index: i64) -> Option<&[String]> {
    let i = usize::try_from(index).ok()?;
    csv?.rows.get(i).map(Vec::as_slice)
}

/// Parse CSV text (RFC 4180: commas, double-quoted fields with `""` escapes and line breaks inside quotes; a UTF-8
/// BOM is dropped; CRLF or LF). Rows are kept as they are, the first one included (LightBurn counts it as row 0).
pub fn parse_csv(text: &str) -> Result<Vec<Vec<String>>, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    let mut any = false;
    while let Some(c) = chars.next() {
        any = true;
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => quoted = false,
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => quoted = true,
            ',' => row.push(std::mem::take(&mut field)),
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' | '\r' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                any = false;
            }
            _ => field.push(c),
        }
    }
    if quoted {
        return Err("the CSV file ends inside a quoted field".into());
    }
    if any {
        row.push(field);
        rows.push(row);
    }
    Ok(rows)
}

/// What a variable text of `doc` shows now: its format evaluated with the document's counter (plus the text's
/// offset), CSV row, `layer`'s settings and `moment` (needed for date/time; without one the format is shown).
pub fn evaluate(doc: &Document, layer: &str, format: &str, var: &VarText, moment: Option<&Moment>, laser: &str) -> String {
    let v: &VariableState = &doc.variables;
    let value = v.current.saturating_add(var.offset);
    match var.mode {
        VarMode::Serial => format_serial(format, value),
        VarMode::Csv => format_csv(format, csv_row(v.csv.as_ref(), value)),
        VarMode::DateTime => match moment {
            Some(m) => format_date(format, m),
            None => format.to_string(),
        },
        VarMode::CutSetting => {
            let own = doc.layer(layer).map(CutValues::of);
            let pick = |n: u32| {
                doc.layers
                    .iter()
                    .find(|l| l.extras.get("lbrn2.index").and_then(|i| i.trim().parse::<u32>().ok()) == Some(n))
                    .or_else(|| doc.layers.get(n as usize))
                    .map(CutValues::of)
            };
            format_cut(format, own, &pick, laser)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn moment() -> Moment {
        let t = NaiveDate::from_ymd_opt(2025, 12, 2).unwrap().and_hms_milli_opt(15, 4, 5, 120).unwrap();
        Moment { local: t, zone: "UTC+01:00".into() }
    }

    #[test]
    fn serial_codes_follow_lightburns_table() {
        for (f, want) in [
            ("d", "4"),
            ("dd", "34"),
            ("ddd", "234"),
            ("dddd", "1234"),
            ("ddddd", "1234"),
            ("dddddd", "1234"),
            ("0d", "4"),
            ("0dddd", "1234"),
            ("0ddddd", "01234"),
            ("0dddddd", "001234"),
        ] {
            assert_eq!(format_serial(f, 1234), want, "{f}");
        }
        assert_eq!(format_serial("h", 255), "f");
        assert_eq!(format_serial("0HHHH", 255), "00FF");
        assert_eq!(format_serial("'SN-'0dddd", 42), "SN-0042");
        assert_eq!(format_serial("ddd-ddd", 1), "bad serial format");
        assert_eq!(format_serial("dh", 1), "bad serial format");
        assert_eq!(format_serial("'It''s 'd", 7), "It's 7");
        assert_eq!(format_serial("No.", 5), "No.", "a format without a code passes through");
    }

    #[test]
    fn date_codes_follow_qt() {
        let m = moment();
        assert_eq!(format_date("yyyy-MM-dd HH:mm:ss", &m), "2025-12-02 15:04:05");
        assert_eq!(format_date("d/M/yy", &m), "2/12/25");
        assert_eq!(format_date("dddd, MMMM d", &m), "Tuesday, December 2");
        assert_eq!(format_date("ddd MMM", &m), "Tue Dec");
        assert_eq!(format_date("h:mm AP", &m), "3:04 PM");
        assert_eq!(format_date("hh:mm ap", &m), "03:04 pm");
        assert_eq!(format_date("H:mm", &m), "15:04");
        assert_eq!(format_date("s.z", &m), "5.12");
        assert_eq!(format_date("zzz", &m), "120");
        assert_eq!(format_date("'Made 'yyyy", &m), "Made 2025");
        assert_eq!(format_date("w W", &m), "49 2025/49");
        assert_eq!(format_date("t", &m), "UTC+01:00");
    }

    #[test]
    fn csv_columns_and_parsing() {
        let rows = parse_csv("LightBurn,laser cutter\r\nMillMage,CNC router\n\"Quote \"\"me\"\", ok\",\"two\nlines\"").unwrap();
        assert_eq!(rows[0], ["LightBurn", "laser cutter"]);
        assert_eq!(rows[2], ["Quote \"me\", ok", "two\nlines"]);
        assert_eq!(format_csv("I use %0 with my %1", Some(&rows[0])), "I use LightBurn with my laser cutter");
        assert_eq!(format_csv("%5|100%%|%x", Some(&rows[0])), "|100%|%x");
        assert_eq!(format_csv("%0", None), "");
        assert!(parse_csv("\"open").is_err());
        assert_eq!(parse_csv("\u{feff}a\n").unwrap(), vec![vec!["a".to_string()]]);
        assert!(parse_csv("").unwrap().is_empty());
    }

    #[test]
    fn cut_setting_codes() {
        let v = CutValues {
            speed_mm_s: 1500.0,
            power_pct: 35.5,
            min_power_pct: 0.0,
            passes: 2,
            interval_mm: 0.05,
            z_offset_mm: 0.0,
            pulse_width_ns: Some(200.0),
            frequency_khz: 60.0,
        };
        let other = CutValues { power_pct: 80.0, ..v.clone() };
        let pick = |n: u32| (n == 3).then(|| other.clone());
        let f = |s: &str| format_cut(s, Some(v.clone()), &pick, "Ruida 6442G");
        assert_eq!(f("P S K Q n"), "35.5% 1500 mm/sec 60kHz 200ns 2");
        assert_eq!(f("p 'Power' C03p"), "35.5 Power 80");
        assert_eq!(f("d 'dpi'"), "508 dpi");
        assert_eq!(f("dpi"), "50835.50.05", "unquoted letters are all codes");
        assert_eq!(f("L6 L6,4 L"), "6442G 6442 Ruida 6442G");
        assert_eq!(f("C09p"), "p", "an unknown layer leaves the codes as they are");
    }

    #[test]
    fn evaluate_uses_offset_and_rows() {
        let mut d = Document::default();
        d.variables.current = 10;
        d.variables.csv = Some(CsvData { name: "a.csv".into(), rows: vec![vec!["x".into()]; 12] });
        let serial = VarText { mode: VarMode::Serial, offset: 2 };
        assert_eq!(evaluate(&d, "layer-1", "0ddd", &serial, None, ""), "012");
        let csv = VarText { mode: VarMode::Csv, offset: 1 };
        assert_eq!(evaluate(&d, "layer-1", "%0", &csv, None, ""), "x");
        let csv = VarText { mode: VarMode::Csv, offset: 5 };
        assert_eq!(evaluate(&d, "layer-1", "%0", &csv, None, ""), "", "past the last row");
        let cut = VarText { mode: VarMode::CutSetting, offset: 0 };
        assert_eq!(evaluate(&d, "layer-1", "p", &cut, None, ""), "20");
        let date = VarText { mode: VarMode::DateTime, offset: 0 };
        assert_eq!(evaluate(&d, "layer-1", "yyyy", &date, Some(&moment()), ""), "2025");
    }
}
