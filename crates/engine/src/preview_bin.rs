//! `GET /api/preview.bin`: the preview of `GET /api/preview` in a compact binary form, for clients that draw it.
//!
//! A large job is hundreds of thousands of segments (a 400 x 300 px image alone is 300 000), about 155 bytes each as
//! JSON: tens of MB that a Raspberry Pi takes seconds to write and Wi-Fi seconds more to move. Here a segment is
//! 13 bytes in the common case (its end point, its time and a flag byte), an image's scan run 5 (its time and flag:
//! the image is sent as a raster of its dots instead), and the answer is compressed once per revision.
//!
//! # Format (version 2, all numbers little-endian)
//!
//! | bytes | content |
//! |---|---|
//! | 0..4 | magic `OEPV` |
//! | 4..8 | u32 format version (2) |
//! | 8..12 | u32 length `H` of the header |
//! | 12..12+H | header, UTF-8 JSON (below) |
//! | then | sections at the byte offsets the header gives (each 4-byte aligned from the start of the buffer) |
//!
//! Header: everything of `GET /api/preview` but `segments` (`stats`, `passes`, `layers` with their `unsupported`
//! settings, `warnings`), plus
//!
//! - `revision`: the document revision the preview is of;
//! - `count`: segments; `lod_mm` and `jumps`: what the request asked for (with `lod_mm` > 0 or `jumps` false the
//!   segment numbers are not the run's, so Start Here must not use them);
//! - `runs`: `[{pass, layer, count}]`, consecutive segments of one pass (`pass` and `layer` as a segment's in
//!   `GET /api/preview`, `null` when absent), in order, adding up to `count`;
//! - `rasters`: the image passes as rasters (`plan::RasterHeader`: `pass`, `first`, `end`, `grid`, `cols`, `rows`,
//!   `powers`, `pass_power_pct`, `levels_at`, `rows_at`; see `plan::PreviewRaster`), in this answer's segment
//!   numbering: segments `first..end` (an image's runs and the jumps between them) are drawn by the raster and carry
//!   no geometry here;
//! - `sections`: `{name: [byte offset, element count]}` for
//!   - `flags` (u8 per segment): bit 0 a mark (else a jump), bit 1 it emits, bit 2 it has its own power (next value
//!     of `power`), bit 3 it starts where the previous segment ended (its start is not stored), bit 4 a raster draws
//!     it (nothing in `xy` or `power`);
//!   - `xy` (f32): per segment without bit 4 its start `x, y` unless bit 3, then its end `x, y` (field mm);
//!   - `t` (f32 per segment): `t_s`;
//!   - `power` (f32): `power_pct` of the segments with bit 2, in order;
//!   - `levels` (u8), `row_segments` (u32, two per row) and `row_span` (f32, two per row): the rasters' arrays.
//!
//! Version 1 was the same without rasters (and bit 4). The engine's own plan (runs, framing) never comes from here:
//! this is display data.

use doc::Document;
use plan::{PackedRasters, Preview, PreviewRaster, SegmentKind};
use serde_json::{Value, json};

/// Format magic.
pub const MAGIC: &[u8; 4] = b"OEPV";
/// Format version.
pub const VERSION: u32 = 2;

const MARK: u8 = 1;
const EMITS: u8 = 2;
const POWER: u8 = 4;
const CHAINED: u8 = 8;
const RASTER: u8 = 16;

/// What a request asks for.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BinOptions {
    /// Only these layers' segments and layer entries (stats stay the whole job's), as `GET /api/preview?layers=`.
    pub layers: Option<Vec<String>>,
    /// Leave travel out.
    pub no_jumps: bool,
    /// Level of detail, mm: consecutive collinear marks of one pass that join end to start are drawn as one while
    /// the joined mark stays at most this long (its power the length-weighted mean); 0 keeps every segment.
    pub lod_mm: f64,
}

impl BinOptions {
    /// A key for caching encoded answers.
    pub fn key(&self) -> String {
        format!("{:?}|{}|{}", self.layers, self.no_jumps, self.lod_mm.to_bits())
    }
}

/// The header members `GET /api/preview` and this format share (`appcore::preview`).
pub use appcore::preview::{add_unsupported, summary};

/// One segment as the encoder sees it.
#[derive(Clone)]
struct Seg<'a> {
    mark: bool,
    emits: bool,
    from: [f64; 2],
    to: [f64; 2],
    t_s: f64,
    power: Option<f64>,
    pass: Option<usize>,
    layer: Option<&'a str>,
    /// Drawn by a raster: no geometry is sent.
    raster: bool,
}

impl Seg<'_> {
    fn len(&self) -> f64 {
        (self.to[0] - self.from[0]).hypot(self.to[1] - self.from[1])
    }
}

/// Whether `b` continues `a` in a straight line (same direction, `b` starting where `a` ends) with the same kind.
fn joins(a: &Seg, b: &Seg) -> bool {
    if a.raster || b.raster {
        return false;
    }
    if !(a.mark && b.mark && a.emits == b.emits && a.pass == b.pass && a.layer == b.layer && a.to == b.from) {
        return false;
    }
    let (ax, ay) = (a.to[0] - a.from[0], a.to[1] - a.from[1]);
    let (bx, by) = (b.to[0] - b.from[0], b.to[1] - b.from[1]);
    let (la, lb) = (ax.hypot(ay), bx.hypot(by));
    if la == 0.0 || lb == 0.0 {
        return false;
    }
    let cross = ax * by - ay * bx;
    let dot = ax * bx + ay * by;
    dot > 0.0 && cross.abs() <= 1e-6 * la * lb
}

/// Encode `p` (planned from `doc` at `revision`) as described in the module docs.
pub fn encode(doc: &Document, p: &Preview, revision: u64, opts: &BinOptions) -> Vec<u8> {
    let keep = |l: Option<&str>| opts.layers.as_ref().is_none_or(|ls| l.is_some_and(|l| ls.iter().any(|k| k == l)));
    let rasters: Vec<&PreviewRaster> =
        p.rasters.iter().filter(|r| keep(p.passes.get(r.pass).map(|q| q.layer_id.as_str()))).collect();
    // With rasters: each segment's index in this answer (segments are left out and joined), to renumber them.
    let mut renumber: Vec<u32> = Vec::with_capacity(if rasters.is_empty() { 0 } else { p.segments.len() + 1 });
    let mut segs: Vec<Seg> = Vec::with_capacity(p.segments.len());
    let mut inside = rasters.iter().map(|r| r.first..r.end).peekable();
    for (i, s) in p.segments.iter().enumerate() {
        if !rasters.is_empty() {
            renumber.push(u32::try_from(segs.len()).unwrap_or(u32::MAX));
        }
        while inside.peek().is_some_and(|r| r.end <= i) {
            inside.next();
        }
        let mark = s.kind == SegmentKind::Mark;
        if (!mark && opts.no_jumps) || !keep(s.layer.as_deref()) {
            continue;
        }
        let seg = Seg {
            mark,
            emits: s.emits,
            from: s.from,
            to: s.to,
            t_s: s.t_s,
            power: s.power_pct,
            pass: s.pass,
            layer: s.layer.as_deref(),
            raster: inside.peek().is_some_and(|r| r.contains(&i)),
        };
        if opts.lod_mm > 0.0
            && let Some(last) = segs.last_mut()
            && joins(last, &seg)
        {
            let (la, lb) = (last.len(), seg.len());
            if last.power == seg.power || la + lb <= opts.lod_mm {
                if last.power != seg.power {
                    // Length-weighted mean of the two powers (a missing one is the pass's, which the encoder does not
                    // know: keep the explicit one).
                    last.power = match (last.power, seg.power) {
                        (Some(a), Some(b)) => Some((a * la + b * lb) / (la + lb)),
                        (a, b) => a.or(b),
                    };
                }
                last.to = seg.to;
                last.t_s = seg.t_s;
                continue;
            }
        }
        segs.push(seg);
    }
    if !rasters.is_empty() {
        renumber.push(u32::try_from(segs.len()).unwrap_or(u32::MAX));
    }
    // The rasters in this answer's numbering.
    let mut packed = PackedRasters::new(&rasters.iter().map(|r| (*r).clone()).collect::<Vec<_>>());
    let at = |i: usize| renumber.get(i).copied().unwrap_or(u32::MAX);
    for h in &mut packed.headers {
        (h.first, h.end) = (at(h.first) as usize, at(h.end) as usize);
    }
    packed.row_segments.iter_mut().for_each(|i| *i = at(*i as usize));

    let n = segs.len();
    let mut flags = Vec::with_capacity(n);
    let mut xy: Vec<f32> = Vec::with_capacity(n * 2 + 16);
    let mut t: Vec<f32> = Vec::with_capacity(n);
    let mut power: Vec<f32> = Vec::new();
    let mut runs: Vec<Value> = Vec::new();
    let mut run: Option<(Option<usize>, Option<&str>, usize)> = None;
    let mut prev_to: Option<[f64; 2]> = None;
    for s in &segs {
        let mut f = 0u8;
        if s.mark {
            f |= MARK;
        }
        if s.emits {
            f |= EMITS;
        }
        if s.raster {
            f |= RASTER;
            prev_to = None;
        } else {
            if let Some(pw) = s.power {
                f |= POWER;
                power.push(pw as f32);
            }
            if prev_to == Some(s.from) {
                f |= CHAINED;
            } else {
                xy.extend([s.from[0] as f32, s.from[1] as f32]);
            }
            xy.extend([s.to[0] as f32, s.to[1] as f32]);
            prev_to = Some(s.to);
        }
        t.push(s.t_s as f32);
        flags.push(f);
        match &mut run {
            Some((pass, layer, count)) if *pass == s.pass && *layer == s.layer => *count += 1,
            _ => {
                if let Some((pass, layer, count)) = run.take() {
                    runs.push(json!({ "pass": pass, "layer": layer, "count": count }));
                }
                run = Some((s.pass, s.layer, 1));
            }
        }
    }
    if let Some((pass, layer, count)) = run {
        runs.push(json!({ "pass": pass, "layer": layer, "count": count }));
    }

    let mut header = summary(doc, p, opts.layers.as_deref());
    header["format"] = json!("openetch-preview");
    header["version"] = json!(VERSION);
    header["revision"] = json!(revision);
    header["count"] = json!(n);
    header["lod_mm"] = json!(opts.lod_mm);
    header["jumps"] = json!(!opts.no_jumps);
    header["runs"] = Value::Array(runs);
    header["rasters"] = json!(packed.headers);
    // Section offsets depend on the header's length, which depends on the offsets' digits: lay out with the final
    // header length found by iterating (it settles in a step or two).
    let words = xy.len() + t.len() + power.len() + packed.row_segments.len() + packed.row_span.len();
    let mut header_len = 0usize;
    let mut text;
    loop {
        let base = align4(12 + header_len);
        let xy_at = base;
        let t_at = xy_at + xy.len() * 4;
        let power_at = t_at + t.len() * 4;
        let rows_at = power_at + power.len() * 4;
        let span_at = rows_at + packed.row_segments.len() * 4;
        let flags_at = span_at + packed.row_span.len() * 4;
        let levels_at = flags_at + flags.len();
        header["sections"] = json!({
            "xy": [xy_at, xy.len()],
            "t": [t_at, t.len()],
            "power": [power_at, power.len()],
            "row_segments": [rows_at, packed.row_segments.len()],
            "row_span": [span_at, packed.row_span.len()],
            "flags": [flags_at, flags.len()],
            "levels": [levels_at, packed.levels.len()],
        });
        text = serde_json::to_vec(&header).unwrap_or_default();
        if text.len() == header_len {
            break;
        }
        header_len = text.len();
    }
    let base = align4(12 + header_len);
    let mut out = Vec::with_capacity(base + words * 4 + flags.len() + packed.levels.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(header_len as u32).to_le_bytes());
    out.extend_from_slice(&text);
    out.resize(base, b' ');
    for v in xy.iter().chain(&t).chain(&power) {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for v in &packed.row_segments {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for v in &packed.row_span {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&flags);
    out.extend_from_slice(&packed.levels);
    out
}

/// Brotli quality of the binary preview: on a Raspberry Pi 4 it keeps well ahead of Wi-Fi.
pub const BROTLI_QUALITY: u32 = 4;
/// Gzip level of the binary preview (clients without brotli).
pub const GZIP_LEVEL: u32 = 3;

/// The content encoding to use for a request's `Accept-Encoding`: brotli, else gzip, else none.
pub fn pick_encoding(accept: Option<&str>) -> Option<&'static str> {
    let accept = accept.unwrap_or_default().to_ascii_lowercase();
    let offered = |name: &str| {
        accept.split(',').any(|part| {
            let mut it = part.split(';');
            it.next().is_some_and(|n| n.trim() == name) && !it.any(|q| matches!(q.trim(), "q=0" | "q=0.0" | "q=0.00" | "q=0.000"))
        })
    };
    if offered("br") {
        Some("br")
    } else if offered("gzip") {
        Some("gzip")
    } else {
        None
    }
}

/// `raw` compressed with `encoding` (and the encoding actually used: none when compressing failed).
pub fn compress(raw: Vec<u8>, encoding: Option<&'static str>) -> (Vec<u8>, Option<&'static str>) {
    use std::io::Write;
    match encoding {
        Some("br") => {
            let params = brotli::enc::BrotliEncoderParams { quality: BROTLI_QUALITY as i32, lgwin: 22, ..Default::default() };
            let mut out = Vec::with_capacity(raw.len() / 3);
            match brotli::BrotliCompress(&mut raw.as_slice(), &mut out, &params) {
                Ok(_) => (out, Some("br")),
                Err(_) => (raw, None),
            }
        }
        Some("gzip") => {
            let mut enc = flate2::write::GzEncoder::new(Vec::with_capacity(raw.len() / 3), flate2::Compression::new(GZIP_LEVEL));
            match enc.write_all(&raw).and_then(|()| enc.finish()) {
                Ok(out) => (out, Some("gzip")),
                Err(_) => (raw, None),
            }
        }
        _ => (raw, None),
    }
}

fn align4(n: usize) -> usize {
    n.div_ceil(4) * 4
}

/// Read a buffer back into segments as `GET /api/preview` lists them (tests, and the reference for clients); a
/// segment a raster draws has `"raster": true` and no `from` / `to` / `power_pct`. Also the rasters.
#[cfg(test)]
pub fn decode(buf: &[u8]) -> (Value, Vec<Value>) {
    let (header, segs, _) = decode_with_rasters(buf);
    (header, segs)
}

#[cfg(test)]
pub fn decode_with_rasters(buf: &[u8]) -> (Value, Vec<Value>, Vec<PreviewRaster>) {
    assert_eq!(&buf[..4], MAGIC);
    assert_eq!(u32::from_le_bytes(buf[4..8].try_into().unwrap()), VERSION);
    let h = u32::from_le_bytes(buf[8..12].try_into().unwrap()) as usize;
    let header: Value = serde_json::from_slice(&buf[12..12 + h]).unwrap();
    let sec = |name: &str| (header["sections"][name][0].as_u64().unwrap() as usize, header["sections"][name][1].as_u64().unwrap() as usize);
    let words = |(at, n): (usize, usize)| -> Vec<[u8; 4]> {
        assert_eq!(at % 4, 0);
        (0..n).map(|i| buf[at + 4 * i..at + 4 * i + 4].try_into().unwrap()).collect()
    };
    let f32s = |s| words(s).into_iter().map(f32::from_le_bytes).collect::<Vec<f32>>();
    let (xy, t, power) = (f32s(sec("xy")), f32s(sec("t")), f32s(sec("power")));
    let (flags_at, n) = sec("flags");
    let flags = &buf[flags_at..flags_at + n];
    let (levels_at, levels_n) = sec("levels");
    let packed = PackedRasters {
        headers: serde_json::from_value(header["rasters"].clone()).unwrap(),
        levels: buf[levels_at..levels_at + levels_n].to_vec(),
        row_segments: words(sec("row_segments")).into_iter().map(u32::from_le_bytes).collect(),
        row_span: f32s(sec("row_span")),
    };
    let mut runs = header["runs"].as_array().unwrap().iter().flat_map(|r| {
        std::iter::repeat_n((r["pass"].clone(), r["layer"].clone()), r["count"].as_u64().unwrap() as usize)
    });
    let (mut k, mut p) = (0, 0);
    let mut prev = [0f32; 2];
    let mut out = Vec::with_capacity(n);
    for (i, &f) in flags.iter().enumerate() {
        let (pass, layer) = runs.next().unwrap();
        let kind = if f & MARK != 0 { "mark" } else { "jump" };
        if f & RASTER != 0 {
            out.push(json!({ "kind": kind, "emits": f & EMITS != 0, "t_s": t[i], "pass": pass, "layer": layer, "raster": true }));
            continue;
        }
        let from = if f & CHAINED != 0 {
            prev
        } else {
            k += 2;
            [xy[k - 2], xy[k - 1]]
        };
        let to = [xy[k], xy[k + 1]];
        k += 2;
        prev = to;
        let mut s = json!({
            "kind": kind, "from": from, "to": to, "emits": f & EMITS != 0, "t_s": t[i], "pass": pass, "layer": layer,
        });
        if f & POWER != 0 {
            s["power_pct"] = json!(power[p]);
            p += 1;
        }
        out.push(s);
    }
    (header, out, packed.unpack().expect("the raster arrays hold every header"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use plan::{PreviewSegment, Stats};

    fn seg(kind: SegmentKind, from: [f64; 2], to: [f64; 2], t: f64, power: Option<f64>, pass: usize) -> PreviewSegment {
        PreviewSegment {
            kind,
            from,
            to,
            layer: Some(if pass == 0 { "layer-1".into() } else { "layer-2".into() }),
            pass: Some(pass),
            emits: kind == SegmentKind::Mark,
            power_pct: power,
            t_s: t,
        }
    }

    fn sample() -> Preview {
        use SegmentKind::{Jump, Mark};
        Preview {
            segments: vec![
                seg(Jump, [0.0, 0.0], [1.0, 1.0], 0.1, None, 0),
                seg(Mark, [1.0, 1.0], [2.0, 1.0], 0.2, Some(40.0), 0),
                seg(Mark, [2.0, 1.0], [3.0, 1.0], 0.3, Some(60.0), 0),
                seg(Mark, [3.0, 1.0], [3.0, 2.0], 0.4, None, 0),
                seg(Jump, [3.0, 2.0], [-5.5, 7.25], 0.5, None, 1),
                seg(Mark, [-5.5, 7.25], [-5.5, 7.25], 0.6, None, 1),
            ],
            stats: Stats { segment_count: 4, ..Stats::default() },
            ..Preview::default()
        }
    }

    #[test]
    fn round_trips_every_segment() {
        let p = sample();
        let buf = encode(&Document::default(), &p, 7, &BinOptions::default());
        let (header, segs) = decode(&buf);
        assert_eq!(header["revision"], 7);
        assert_eq!(header["count"], 6);
        assert_eq!(header["stats"]["segment_count"], 4);
        assert_eq!(header["runs"].as_array().unwrap().len(), 2);
        let json: Vec<Value> = p.segments.iter().map(|s| serde_json::to_value(s).unwrap()).collect();
        assert_eq!(segs.len(), json.len());
        for (a, b) in segs.iter().zip(&json) {
            for k in ["kind", "from", "to", "emits", "pass", "layer", "power_pct"] {
                assert_eq!(a.get(k), b.get(k), "{k}: {a} vs {b}");
            }
            assert!((a["t_s"].as_f64().unwrap() - b["t_s"].as_f64().unwrap()).abs() < 1e-6);
        }
        // Chained segments store only their end: 6 ends + 1 start (the first) = 14 floats.
        assert_eq!(header["sections"]["xy"][1], 14);
    }

    #[test]
    fn encodings() {
        assert_eq!(pick_encoding(Some("gzip, deflate, br, zstd")), Some("br"));
        assert_eq!(pick_encoding(Some("gzip;q=1.0, br;q=0")), Some("gzip"));
        assert_eq!(pick_encoding(Some("identity")), None);
        assert_eq!(pick_encoding(None), None);
        let raw = encode(&Document::default(), &sample(), 1, &BinOptions::default());
        for enc in ["br", "gzip"] {
            let (out, used) = compress(raw.clone(), Some(enc));
            assert_eq!(used, Some(enc));
            let mut back = Vec::new();
            if enc == "br" {
                brotli::BrotliDecompress(&mut out.as_slice(), &mut back).unwrap();
            } else {
                use std::io::Read;
                flate2::read::GzDecoder::new(out.as_slice()).read_to_end(&mut back).unwrap();
            }
            assert_eq!(back, raw);
        }
    }

    #[test]
    fn filters_and_level_of_detail() {
        let p = sample();
        let doc = Document::default();
        let (h, segs) = decode(&encode(&doc, &p, 1, &BinOptions { no_jumps: true, ..Default::default() }));
        assert!(segs.iter().all(|s| s["kind"] == "mark") && segs.len() == 4);
        assert_eq!(h["jumps"], false);
        let only = BinOptions { layers: Some(vec!["layer-2".into()]), ..Default::default() };
        let (_, segs) = decode(&encode(&doc, &p, 1, &only));
        assert_eq!(segs.len(), 2);
        // The two collinear marks join (2 mm <= lod) with the mean power; the turn does not.
        let (_, segs) = decode(&encode(&doc, &p, 1, &BinOptions { lod_mm: 2.0, ..Default::default() }));
        assert_eq!(segs.len(), 5);
        assert_eq!(segs[1]["from"], json!([1.0, 1.0]));
        assert_eq!(segs[1]["to"], json!([3.0, 1.0]));
        assert_eq!(segs[1]["power_pct"], json!(50.0));
        assert_eq!(segs[1]["t_s"].as_f64().map(|t| (t * 10.0).round()), Some(3.0));
        // A shorter limit keeps them apart (their powers differ).
        let (_, segs) = decode(&encode(&doc, &p, 1, &BinOptions { lod_mm: 1.5, ..Default::default() }));
        assert_eq!(segs.len(), 6);
    }
}
