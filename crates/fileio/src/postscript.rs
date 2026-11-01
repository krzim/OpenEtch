//! EPS and PostScript-based Adobe Illustrator (`.ai` before version 9, and AI files saved without PDF content) import.
//!
//! No permissively licensed PostScript interpreter exists in Rust (Ghostscript is AGPL), so this is a small
//! interpreter of the subset vector art uses (written from the PostScript Language Reference and Adobe's
//! Illustrator File Format notes): numbers, names, strings, procedures, `def`/`bind`/`load`/`exec`, `if`/`ifelse`/
//! `repeat`/`for`, stack operators (`exch dup pop copy index roll clear mark cleartomark`), arithmetic, the path
//! operators (`newpath moveto rmoveto lineto rlineto curveto rcurveto closepath arc arcn rectfill rectstroke`),
//! painting (`stroke fill eofill`), `gsave`/`grestore`, the CTM (`translate scale rotate concat`), and colours
//! (`setrgbcolor setgray setcmykcolor`). Illustrator files (`%%Creator: Adobe Illustrator` or `%AI` comments) also
//! get Illustrator's one-letter path operators (`m l L c C v V y Y h H f F s S b B n N`, colours `g G k K Xa XA`);
//! their prolog and setup sections (procedure sets) are skipped. Anything else is ignored: text and images are left
//! out with a warning, and art built by unsupported operators (fonts, patterns, `loop`) is missing. DOS EPS files with
//! a binary preview header are read from their PostScript section. Units: points (1/72 in) → mm.

use std::collections::HashMap;
use std::rc::Rc;

use kurbo::{Affine, BezPath, Point};

use crate::{Art, FileError, Result, invalid};

/// Largest file read.
pub const MAX_BYTES: usize = 50 * 1024 * 1024;
/// Most operators executed (procedures and loops count each time, and so does every procedure call, so even empty
/// procedures in nested loops end).
pub const MAX_STEPS: usize = 20_000_000;
/// Most operands on the stack (PostScript Level 1 allows 500; `copy` and `dup` must not double it without bound).
pub const MAX_STACK: usize = 100_000;
/// Deepest `{` nesting in the file (deeper structures would be dropped and compared recursively).
pub const MAX_NESTING: usize = 256;
/// Deepest array nesting (`[ [ ... ] ]`, also when built up in a loop).
const MAX_ARRAY_DEPTH: u32 = 32;
/// Most path elements drawn in total (memory).
pub const MAX_PATH_ELEMENTS: usize = 4_000_000;

const MM_PER_PT: f64 = 25.4 / 72.0;

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Name(String),
    Lit(Rc<str>),
    Str(Rc<str>),
    Proc(Rc<Vec<Tok>>),
    ArrOpen,
    ArrClose,
    DictOpen,
    DictClose,
}

#[derive(Debug, Clone, PartialEq)]
enum Val {
    Num(f64),
    // Shared (`Rc`), so `dup`, `copy`, `index` and dictionary lookups never copy a long string or array.
    Lit(Rc<str>),
    Str(Rc<str>),
    Proc(Rc<Vec<Tok>>),
    Bool(bool),
    /// Items and nesting depth (1 for an array holding no arrays).
    Arr(Rc<Vec<Val>>, u32),
    Mark,
    Other,
}

/// Tokenise PostScript text (comments dropped).
fn tokenize(src: &str) -> Result<Vec<Tok>> {
    let b = src.as_bytes();
    let mut i = 0;
    // Stack of procedure bodies being built.
    let mut stack: Vec<Vec<Tok>> = vec![Vec::new()];
    let delim = |c: u8| c.is_ascii_whitespace() || b"()<>[]{}/%".contains(&c);
    while i < b.len() {
        let c = b[i];
        match c {
            b'%' => {
                while i < b.len() && b[i] != b'\n' && b[i] != b'\r' {
                    i += 1;
                }
            }
            c if c.is_ascii_whitespace() => i += 1,
            b'{' => {
                if stack.len() > MAX_NESTING {
                    return Err(FileError::TooLarge("the PostScript procedures nest too deeply".into()));
                }
                stack.push(Vec::new());
                i += 1;
            }
            b'}' => {
                let body = stack.pop().ok_or_else(|| FileError::Invalid("unbalanced }".into()))?;
                let top = stack.last_mut().ok_or_else(|| FileError::Invalid("unbalanced }".into()))?;
                top.push(Tok::Proc(Rc::new(body)));
                i += 1;
            }
            b'[' => {
                stack.last_mut().expect("stack").push(Tok::ArrOpen);
                i += 1;
            }
            b']' => {
                stack.last_mut().expect("stack").push(Tok::ArrClose);
                i += 1;
            }
            b'(' => {
                let mut depth = 1;
                let mut s = String::new();
                i += 1;
                while i < b.len() && depth > 0 {
                    match b[i] {
                        b'\\' => {
                            i += 1;
                            if let Some(&e) = b.get(i) {
                                s.push(match e {
                                    b'n' => '\n',
                                    b't' => '\t',
                                    b'r' => '\r',
                                    other => other as char,
                                });
                            }
                        }
                        b'(' => {
                            depth += 1;
                            s.push('(');
                        }
                        b')' => {
                            depth -= 1;
                            if depth > 0 {
                                s.push(')');
                            }
                        }
                        other => s.push(other as char),
                    }
                    i += 1;
                }
                stack.last_mut().expect("stack").push(Tok::Str(s.into()));
            }
            b'<' => {
                if b.get(i + 1) == Some(&b'<') {
                    stack.last_mut().expect("stack").push(Tok::DictOpen);
                    i += 2;
                } else {
                    // Hex or ASCII85 string: skipped as an opaque string.
                    let end = src[i..].find('>').map_or(b.len(), |e| i + e + 1);
                    stack.last_mut().expect("stack").push(Tok::Str("".into()));
                    i = end;
                }
            }
            b'>' => {
                if b.get(i + 1) == Some(&b'>') {
                    stack.last_mut().expect("stack").push(Tok::DictClose);
                    i += 2;
                } else {
                    i += 1;
                }
            }
            b'/' => {
                let s = i + 1;
                let mut e = s;
                // `//name` (immediately evaluated) is read as a literal.
                if b.get(e) == Some(&b'/') {
                    e += 1;
                }
                let s2 = e;
                while e < b.len() && !delim(b[e]) {
                    e += 1;
                }
                stack.last_mut().expect("stack").push(Tok::Lit(src[s2..e].into()));
                i = e;
            }
            _ => {
                let s = i;
                while i < b.len() && !delim(b[i]) {
                    i += 1;
                }
                if i == s {
                    i += 1;
                    continue;
                }
                let w = &src[s..i];
                let t = match w.parse::<f64>() {
                    Ok(v) if v.is_finite() => Tok::Num(v),
                    _ => match w.split_once('#') {
                        Some((radix, digits)) if radix.parse::<u32>().is_ok_and(|r| (2..=36).contains(&r)) => {
                            let r = radix.parse::<u32>().unwrap_or(10);
                            i64::from_str_radix(digits, r).map_or(Tok::Name(w.to_string()), |v| Tok::Num(v as f64))
                        }
                        _ => Tok::Name(w.to_string()),
                    },
                };
                stack.last_mut().expect("stack").push(t);
            }
        }
    }
    if stack.len() != 1 {
        return invalid("unbalanced { in the PostScript");
    }
    Ok(stack.pop().unwrap_or_default())
}

#[derive(Debug, Clone)]
struct GState {
    ctm: Affine,
    color: Option<[u8; 3]>,
}

struct Interp {
    stack: Vec<Val>,
    dict: HashMap<String, Val>,
    gs: GState,
    saved: Vec<GState>,
    path: BezPath,
    current: Option<Point>,
    start: Option<Point>,
    art: Art,
    ai: bool,
    steps: usize,
    /// Path elements drawn so far (painted or discarded).
    elements: usize,
    texts: usize,
    images: usize,
}

fn rgb(r: f64, g: f64, b: f64) -> [u8; 3] {
    [r, g, b].map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
}

fn cmyk(c: f64, m: f64, y: f64, k: f64) -> [u8; 3] {
    rgb((1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k))
}

impl Interp {
    fn pop(&mut self) -> Result<Val> {
        self.stack.pop().ok_or_else(|| FileError::Invalid("the PostScript stack ran empty".into()))
    }

    fn num(&mut self) -> Result<f64> {
        match self.pop()? {
            Val::Num(v) => Ok(v),
            Val::Bool(b) => Ok(f64::from(u8::from(b))),
            _ => invalid("a number was expected on the PostScript stack"),
        }
    }

    fn nums<const N: usize>(&mut self) -> Result<[f64; N]> {
        let mut out = [0.0; N];
        for k in (0..N).rev() {
            out[k] = self.num()?;
        }
        Ok(out)
    }

    fn user(&self, x: f64, y: f64) -> Point {
        self.gs.ctm * Point::new(x, y)
    }

    fn move_to(&mut self, p: Point) {
        self.path.move_to(p);
        self.current = Some(p);
        self.start = Some(p);
    }

    fn line_to(&mut self, p: Point) {
        if self.current.is_none() {
            self.move_to(p);
            return;
        }
        self.path.line_to(p);
        self.current = Some(p);
    }

    fn curve_to(&mut self, a: Point, b: Point, p: Point) {
        if self.current.is_none() {
            self.move_to(a);
        }
        self.path.curve_to(a, b, p);
        self.current = Some(p);
    }

    fn close(&mut self) {
        if self.current.is_some() {
            self.path.close_path();
            self.current = self.start;
        }
    }

    fn paint(&mut self) {
        let path = std::mem::take(&mut self.path);
        self.elements += path.elements().len();
        let scaled = Affine::scale(MM_PER_PT) * path;
        self.art.push(scaled, "path", self.gs.color);
        self.current = None;
        self.start = None;
    }

    fn discard(&mut self) {
        self.elements += self.path.elements().len();
        self.path = BezPath::new();
        self.current = None;
        self.start = None;
    }

    fn arc(&mut self, ccw: bool) -> Result<()> {
        let [x, y, r, a0, a1] = self.nums::<5>()?;
        let tau = std::f64::consts::TAU;
        let (a0, a1) = (a0.to_radians(), a1.to_radians());
        // PostScript: `arc` runs counter-clockwise, adding whole turns to the end angle until it is not below the
        // start; `arcn` the other way. Computed with `rem_euclid` (adding turns in a loop never ends for an angle like
        // 1e300), and more than one full turn is drawn as one turn plus the rest: the same drawing and end point with
        // a bounded segment count.
        let raw = if ccw { a1 - a0 } else { a0 - a1 };
        let turn = if raw < 0.0 {
            raw.rem_euclid(tau)
        } else if raw > tau {
            tau + (raw - tau).rem_euclid(tau)
        } else {
            raw
        };
        if !turn.is_finite() {
            return invalid("an arc angle is not a usable number");
        }
        let sweep = if ccw { turn } else { -turn };
        let n = ((sweep.abs() / std::f64::consts::FRAC_PI_2).ceil() as usize).clamp(1, 8);
        let step = sweep / n as f64;
        let start = self.user(x + r * a0.cos(), y + r * a0.sin());
        if self.current.is_some() {
            self.line_to(start);
        } else {
            self.move_to(start);
        }
        let k = 4.0 / 3.0 * (step / 4.0).tan();
        for i in 0..n {
            let t0 = a0 + step * i as f64;
            let t1 = t0 + step;
            let p0 = (x + r * t0.cos(), y + r * t0.sin());
            let p1 = (x + r * t1.cos(), y + r * t1.sin());
            let c1 = self.user(p0.0 - k * r * t0.sin(), p0.1 + k * r * t0.cos());
            let c2 = self.user(p1.0 + k * r * t1.sin(), p1.1 - k * r * t1.cos());
            let e = self.user(p1.0, p1.1);
            self.curve_to(c1, c2, e);
        }
        Ok(())
    }

    fn rect(&mut self) -> Result<()> {
        let [x, y, w, h] = self.nums::<4>()?;
        let pts = [(x, y), (x + w, y), (x + w, y + h), (x, y + h)].map(|(a, b)| self.user(a, b));
        self.discard();
        self.move_to(pts[0]);
        for p in &pts[1..] {
            self.line_to(*p);
        }
        self.close();
        self.paint();
        Ok(())
    }

    /// Count `n` steps of work.
    fn tick(&mut self, n: usize) -> Result<()> {
        self.steps = self.steps.saturating_add(n);
        if self.steps > MAX_STEPS {
            return Err(FileError::TooLarge("the PostScript program runs too long".into()));
        }
        Ok(())
    }

    fn exec_proc(&mut self, body: &Rc<Vec<Tok>>, depth: usize) -> Result<()> {
        // A call costs a step even when the procedure is empty: `{} 1000000 repeat` nested deeply must end.
        self.tick(1)?;
        if depth > 64 {
            return invalid("PostScript procedures nest too deeply");
        }
        let body = Rc::clone(body);
        self.run(&body, depth + 1)
    }

    fn run(&mut self, toks: &[Tok], depth: usize) -> Result<()> {
        let mut arrays: Vec<usize> = Vec::new();
        for t in toks {
            self.tick(1)?;
            if self.stack.len() > MAX_STACK {
                return Err(FileError::TooLarge("the PostScript stack grew too large".into()));
            }
            if self.elements.saturating_add(self.path.elements().len()) > MAX_PATH_ELEMENTS {
                return Err(FileError::TooLarge("the PostScript draws too many path segments".into()));
            }
            match t {
                Tok::Num(v) => self.stack.push(Val::Num(*v)),
                Tok::Lit(n) => self.stack.push(Val::Lit(n.clone())),
                Tok::Str(s) => self.stack.push(Val::Str(s.clone())),
                Tok::Proc(p) => self.stack.push(Val::Proc(Rc::clone(p))),
                Tok::ArrOpen | Tok::DictOpen => {
                    arrays.push(self.stack.len());
                    self.stack.push(Val::Mark);
                }
                Tok::ArrClose => {
                    let at = match arrays.pop() {
                        Some(a) => a,
                        None => match self.stack.iter().rposition(|v| *v == Val::Mark) {
                            Some(a) => a,
                            None => continue,
                        },
                    };
                    let items: Vec<Val> = self.stack.drain(at..).skip(1).collect();
                    let depth = 1 + items.iter().map(|v| if let Val::Arr(_, d) = v { *d } else { 0 }).max().unwrap_or(0);
                    if depth > MAX_ARRAY_DEPTH {
                        return Err(FileError::TooLarge("the PostScript arrays nest too deeply".into()));
                    }
                    self.tick(items.len())?;
                    self.stack.push(Val::Arr(Rc::new(items), depth));
                }
                Tok::DictClose => {
                    if let Some(at) = arrays.pop() {
                        self.stack.truncate(at);
                    }
                    self.stack.push(Val::Other);
                }
                Tok::Name(n) => self.op(n, depth)?,
            }
        }
        Ok(())
    }

    fn op(&mut self, name: &str, depth: usize) -> Result<()> {
        if let Some(v) = self.dict.get(name).cloned() {
            return match v {
                Val::Proc(p) => self.exec_proc(&p, depth),
                other => {
                    self.stack.push(other);
                    Ok(())
                }
            };
        }
        if self.ai && self.ai_op(name)? {
            return Ok(());
        }
        match name {
            // Stack.
            "pop" => {
                self.pop()?;
            }
            "exch" => {
                let a = self.pop()?;
                let b = self.pop()?;
                self.stack.push(a);
                self.stack.push(b);
            }
            "dup" => {
                let a = self.pop()?;
                self.stack.push(a.clone());
                self.stack.push(a);
            }
            "copy" => {
                let n = self.num()? as usize;
                let len = self.stack.len();
                if n <= len {
                    self.tick(n)?;
                    let top: Vec<Val> = self.stack[len - n..].to_vec();
                    self.stack.extend(top);
                }
            }
            "index" => {
                let n = self.num()? as usize;
                let v = self.stack.iter().rev().nth(n).cloned().unwrap_or(Val::Other);
                self.stack.push(v);
            }
            "roll" => {
                let j = self.num()? as i64;
                let n = self.num()? as usize;
                let len = self.stack.len();
                if n > 0 && n <= len {
                    let slice = &mut self.stack[len - n..];
                    let k = j.rem_euclid(n as i64) as usize;
                    slice.rotate_right(k);
                    self.tick(n)?;
                }
            }
            "clear" => self.stack.clear(),
            "mark" => self.stack.push(Val::Mark),
            "cleartomark" => {
                if let Some(at) = self.stack.iter().rposition(|v| *v == Val::Mark) {
                    self.stack.truncate(at);
                }
            }
            "counttomark" => {
                let n = self.stack.iter().rev().position(|v| *v == Val::Mark).unwrap_or(0);
                self.stack.push(Val::Num(n as f64));
            }
            "true" => self.stack.push(Val::Bool(true)),
            "false" => self.stack.push(Val::Bool(false)),
            // Arithmetic.
            "add" | "sub" | "mul" | "div" | "idiv" | "mod" | "atan" | "exp" => {
                let [a, b] = self.nums::<2>()?;
                let v = match name {
                    "add" => a + b,
                    "sub" => a - b,
                    "mul" => a * b,
                    "div" => {
                        if b == 0.0 {
                            0.0
                        } else {
                            a / b
                        }
                    }
                    "idiv" => {
                        if b == 0.0 {
                            0.0
                        } else {
                            (a / b).trunc()
                        }
                    }
                    "mod" => {
                        if b == 0.0 {
                            0.0
                        } else {
                            a % b
                        }
                    }
                    "atan" => a.atan2(b).to_degrees().rem_euclid(360.0),
                    _ => a.powf(b),
                };
                self.stack.push(Val::Num(if v.is_finite() { v } else { 0.0 }));
            }
            "neg" | "abs" | "sqrt" | "sin" | "cos" | "round" | "floor" | "ceiling" | "truncate" | "cvi" | "cvr" => {
                let a = self.num()?;
                let v = match name {
                    "neg" => -a,
                    "abs" => a.abs(),
                    "sqrt" => a.max(0.0).sqrt(),
                    "sin" => a.to_radians().sin(),
                    "cos" => a.to_radians().cos(),
                    "round" => a.round(),
                    "floor" => a.floor(),
                    "ceiling" => a.ceil(),
                    "truncate" | "cvi" => a.trunc(),
                    _ => a,
                };
                self.stack.push(Val::Num(v));
            }
            "eq" | "ne" | "gt" | "ge" | "lt" | "le" => {
                let b = self.pop()?;
                let a = self.pop()?;
                let r = match (&a, &b) {
                    (Val::Num(x), Val::Num(y)) => match name {
                        "eq" => x == y,
                        "ne" => x != y,
                        "gt" => x > y,
                        "ge" => x >= y,
                        "lt" => x < y,
                        _ => x <= y,
                    },
                    _ => match name {
                        "eq" => a == b,
                        "ne" => a != b,
                        _ => false,
                    },
                };
                self.stack.push(Val::Bool(r));
            }
            "not" => {
                let v = match self.pop()? {
                    Val::Bool(b) => Val::Bool(!b),
                    Val::Num(n) => Val::Num(!(n as i64) as f64),
                    _ => Val::Bool(false),
                };
                self.stack.push(v);
            }
            "and" | "or" => {
                let b = self.pop()?;
                let a = self.pop()?;
                let r = match (a, b) {
                    (Val::Bool(x), Val::Bool(y)) => Val::Bool(if name == "and" { x && y } else { x || y }),
                    _ => Val::Bool(false),
                };
                self.stack.push(r);
            }
            // Definitions and control.
            "def" => {
                let v = self.pop()?;
                if let Val::Lit(k) = self.pop()? {
                    self.dict.insert(k.to_string(), v);
                }
            }
            "bind" | "readonly" | "executeonly" | "noaccess" | "cvx" => {}
            "load" => {
                if let Val::Lit(k) = self.pop()? {
                    // A built-in operator loads as a procedure that runs it.
                    let v = self.dict.get(&*k).cloned().unwrap_or_else(|| Val::Proc(Rc::new(vec![Tok::Name(k.to_string())])));
                    self.stack.push(v);
                }
            }
            "where" => {
                if let Val::Lit(k) = self.pop()? {
                    if self.dict.contains_key(&*k) {
                        self.stack.push(Val::Other);
                        self.stack.push(Val::Bool(true));
                    } else {
                        self.stack.push(Val::Bool(false));
                    }
                }
            }
            "known" => {
                let k = self.pop()?;
                self.pop()?;
                let known = matches!(k, Val::Lit(ref n) if self.dict.contains_key(&**n));
                self.stack.push(Val::Bool(known));
            }
            "exec" => {
                if let Val::Proc(p) = self.pop()? {
                    self.exec_proc(&p, depth)?;
                }
            }
            "if" => {
                let p = self.pop()?;
                let c = self.pop()?;
                if let (Val::Proc(p), Val::Bool(true)) = (p, c) {
                    self.exec_proc(&p, depth)?;
                }
            }
            "ifelse" => {
                let f = self.pop()?;
                let t = self.pop()?;
                let c = self.pop()?;
                let p = if c == Val::Bool(true) { t } else { f };
                if let Val::Proc(p) = p {
                    self.exec_proc(&p, depth)?;
                }
            }
            "repeat" => {
                let p = self.pop()?;
                let n = self.num()?;
                if let Val::Proc(p) = p {
                    for _ in 0..(n.max(0.0) as usize).min(1_000_000) {
                        self.exec_proc(&p, depth)?;
                    }
                }
            }
            "for" => {
                let p = self.pop()?;
                let [start, inc, limit] = self.nums::<3>()?;
                if let Val::Proc(p) = p
                    && inc != 0.0
                {
                    let mut v = start;
                    let mut n = 0;
                    while (inc > 0.0 && v <= limit) || (inc < 0.0 && v >= limit) {
                        n += 1;
                        if n > 1_000_000 {
                            break;
                        }
                        self.stack.push(Val::Num(v));
                        self.exec_proc(&p, depth)?;
                        v += inc;
                    }
                }
            }
            "dict" => {
                self.pop()?;
                self.stack.push(Val::Other);
            }
            "begin" => {
                self.pop()?;
            }
            "end" | "save" | "restore" | "showpage" | "setlinewidth" | "setlinecap" | "setlinejoin" | "setmiterlimit"
            | "setdash" | "setflat" | "setoverprint" | "setstrokeadjust" | "clip" | "eoclip" | "initclip" | "setmatrix"
            | "currentdict" | "userdict" | "globaldict" | "systemdict" | "array" | "length" | "get" | "put" | "aload"
            | "astore" | "matrix" | "currentmatrix" | "findfont" | "scalefont" | "makefont" | "setfont"
            | "selectfont" | "definefont" | "setcolorspace" | "setcolor" | "sethsbcolor" | "currentpoint" | "rectclip"
            | "print" | "flush" | "stopped" | "string" | "cvs" | "type" | "loop" | "exit" | "forall" | "stop"
            | "countdictstack" | "count" | "pathbbox" | "flattenpath" | "reversepath" | "strokepath" => {
                self.stack_effect(name)?;
            }
            // Paths.
            "newpath" => self.discard(),
            "moveto" => {
                let [x, y] = self.nums::<2>()?;
                let p = self.user(x, y);
                self.move_to(p);
            }
            "rmoveto" | "rlineto" => {
                let [dx, dy] = self.nums::<2>()?;
                let cur = self.current.unwrap_or_default();
                let d = self.gs.ctm * kurbo::Vec2::new(dx, dy).to_point() - self.gs.ctm * Point::ZERO;
                if name == "rmoveto" { self.move_to(cur + d) } else { self.line_to(cur + d) }
            }
            "lineto" => {
                let [x, y] = self.nums::<2>()?;
                let p = self.user(x, y);
                self.line_to(p);
            }
            "curveto" => {
                let [a, b, c, d, e, f] = self.nums::<6>()?;
                let (p1, p2, p3) = (self.user(a, b), self.user(c, d), self.user(e, f));
                self.curve_to(p1, p2, p3);
            }
            "rcurveto" => {
                let [a, b, c, d, e, f] = self.nums::<6>()?;
                let cur = self.current.unwrap_or_default();
                let o = self.gs.ctm * Point::ZERO;
                let rel = |x: f64, y: f64| cur + (self.gs.ctm * Point::new(x, y) - o);
                let (p1, p2, p3) = (rel(a, b), rel(c, d), rel(e, f));
                self.curve_to(p1, p2, p3);
            }
            "closepath" => self.close(),
            "arc" => self.arc(true)?,
            "arcn" => self.arc(false)?,
            "rectfill" | "rectstroke" => {
                if matches!(self.stack.last(), Some(Val::Arr(..))) {
                    self.pop()?;
                } else {
                    self.rect()?;
                }
            }
            "stroke" | "fill" | "eofill" => self.paint(),
            "gsave" => self.saved.push(self.gs.clone()),
            "grestore" => {
                if let Some(g) = self.saved.pop() {
                    self.gs = g;
                }
            }
            "grestoreall" => {
                if let Some(g) = self.saved.first().cloned() {
                    self.gs = g;
                    self.saved.clear();
                }
            }
            "translate" => {
                if matches!(self.stack.last(), Some(Val::Arr(..))) {
                    self.pop()?;
                }
                let [x, y] = self.nums::<2>()?;
                self.gs.ctm *= Affine::translate((x, y));
            }
            "scale" => {
                if matches!(self.stack.last(), Some(Val::Arr(..))) {
                    self.pop()?;
                }
                let [x, y] = self.nums::<2>()?;
                self.gs.ctm *= Affine::scale_non_uniform(x, y);
            }
            "rotate" => {
                if matches!(self.stack.last(), Some(Val::Arr(..))) {
                    self.pop()?;
                }
                let a = self.num()?;
                self.gs.ctm *= Affine::rotate(a.to_radians());
            }
            "concat" => {
                if let Val::Arr(m, _) = self.pop()?
                    && m.len() == 6
                {
                    let v: Vec<f64> = m.iter().map(|x| if let Val::Num(n) = x { *n } else { 0.0 }).collect();
                    self.gs.ctm *= Affine::new([v[0], v[1], v[2], v[3], v[4], v[5]]);
                }
            }
            "setrgbcolor" => {
                let [r, g, b] = self.nums::<3>()?;
                self.gs.color = Some(rgb(r, g, b));
            }
            "setgray" => {
                let g = self.num()?;
                self.gs.color = Some(rgb(g, g, g));
            }
            "setcmykcolor" => {
                let [c, m, y, k] = self.nums::<4>()?;
                self.gs.color = Some(cmyk(c, m, y, k));
            }
            "show" | "ashow" | "widthshow" | "awidthshow" | "kshow" | "xshow" | "yshow" | "xyshow" | "glyphshow"
            | "charpath" => {
                self.texts += 1;
                self.stack_effect(name)?;
            }
            "image" | "colorimage" | "imagemask" => {
                self.images += 1;
                self.stack.clear();
            }
            _ => {}
        }
        Ok(())
    }

    /// Pop the operands of operators that are only skipped, so the stack stays balanced.
    fn stack_effect(&mut self, name: &str) -> Result<()> {
        let pops = match name {
            "setlinewidth" | "setlinecap" | "setlinejoin" | "setmiterlimit" | "setflat" | "setoverprint" | "setstrokeadjust"
            | "setmatrix" | "setfont" | "setcolorspace" | "length" | "aload" | "astore" | "findfont" | "definefont"
            | "currentmatrix" | "array" | "string" | "type" | "show" | "glyphshow" | "print" | "begin" | "restore"
            | "loop" | "stopped" => 1,
            "setdash" | "scalefont" | "makefont" | "selectfont" | "get" | "cvs" | "ashow" | "kshow" | "xshow" | "yshow"
            | "xyshow" | "charpath" | "forall" => 2,
            "put" | "sethsbcolor" => 3,
            "widthshow" | "rectclip" => 4,
            "awidthshow" => 6,
            _ => 0,
        };
        for _ in 0..pops {
            if self.stack.pop().is_none() {
                break;
            }
        }
        match name {
            "currentdict" | "userdict" | "globaldict" | "systemdict" | "matrix" | "save" | "findfont" | "scalefont"
            | "makefont" | "currentmatrix" | "array" | "string" | "get" | "cvs" | "type" | "definefont" => self.stack.push(Val::Other),
            "length" | "countdictstack" | "count" => self.stack.push(Val::Num(0.0)),
            "currentpoint" => {
                let p = self.gs.ctm.inverse() * self.current.unwrap_or_default();
                self.stack.push(Val::Num(p.x));
                self.stack.push(Val::Num(p.y));
            }
            "stopped" => self.stack.push(Val::Bool(false)),
            "pathbbox" => self.stack.extend([Val::Num(0.0), Val::Num(0.0), Val::Num(0.0), Val::Num(0.0)]),
            _ => {}
        }
        Ok(())
    }

    /// Illustrator's one-letter path and colour operators. Returns whether `name` was one.
    fn ai_op(&mut self, name: &str) -> Result<bool> {
        match name {
            "m" => {
                let [x, y] = self.nums::<2>()?;
                let p = self.user(x, y);
                self.move_to(p);
            }
            "l" | "L" => {
                let [x, y] = self.nums::<2>()?;
                let p = self.user(x, y);
                self.line_to(p);
            }
            "c" | "C" => {
                let [a, b, c, d, e, f] = self.nums::<6>()?;
                let (p1, p2, p3) = (self.user(a, b), self.user(c, d), self.user(e, f));
                self.curve_to(p1, p2, p3);
            }
            "v" | "V" => {
                let [c, d, e, f] = self.nums::<4>()?;
                let p1 = self.current.unwrap_or_default();
                let (p2, p3) = (self.user(c, d), self.user(e, f));
                self.curve_to(p1, p2, p3);
            }
            "y" | "Y" => {
                let [a, b, e, f] = self.nums::<4>()?;
                let (p1, p3) = (self.user(a, b), self.user(e, f));
                self.curve_to(p1, p3, p3);
            }
            "h" | "H" => self.close(),
            // Lowercase painting closes the path first.
            "f" | "s" | "b" => {
                self.close();
                self.paint();
            }
            "F" | "S" | "B" => self.paint(),
            "n" | "N" => self.discard(),
            "g" | "G" => {
                let g = self.num()?;
                self.gs.color = Some(rgb(g, g, g));
            }
            "k" | "K" => {
                let [c, m, y, k] = self.nums::<4>()?;
                self.gs.color = Some(cmyk(c, m, y, k));
            }
            "x" | "X" => {
                // Custom colour: c m y k (name) tint x.
                self.pop()?;
                self.pop()?;
                let [c, m, y, k] = self.nums::<4>()?;
                self.gs.color = Some(cmyk(c, m, y, k));
            }
            "Xa" | "XA" => {
                let [r, g, b] = self.nums::<3>()?;
                self.gs.color = Some(rgb(r, g, b));
            }
            "u" | "U" | "*u" | "*U" | "D" | "A" | "w" | "j" | "J" | "M" | "d" | "i" | "XR" | "Xw" | "Ln" | "Lb" | "LB" => {
                let pops = match name {
                    "D" | "A" | "w" | "j" | "J" | "M" | "i" | "XR" | "Xw" | "Ln" => 1,
                    "d" => 2,
                    "Lb" => 10,
                    _ => 0,
                };
                for _ in 0..pops {
                    self.stack.pop();
                }
            }
            "To" | "TP" | "Tp" | "TO" => {
                self.texts += usize::from(name == "To");
                self.stack.clear();
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}

/// The PostScript section of a DOS EPS (binary header `C5 D0 D3 C6`), else the bytes as they are.
fn postscript_section(bytes: &[u8]) -> &[u8] {
    if bytes.starts_with(&[0xC5, 0xD0, 0xD3, 0xC6]) && bytes.len() >= 12 {
        let at = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
        let len = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
        if let Some(s) = bytes.get(at..at.saturating_add(len)) {
            return s;
        }
    }
    bytes
}

/// Whether the text is an Illustrator file (its one-letter operators apply).
fn is_illustrator(text: &str) -> bool {
    let head: String = text.chars().take(8000).collect();
    head.contains("%%Creator: Adobe Illustrator") || head.contains("%AI3_") || head.contains("%AI5_") || head.contains("%AI7_")
        || head.contains("%AI8_") || head.contains("%%AI8_")
}

/// Remove the sections between `begin` and `end` comment lines.
fn strip_sections(text: &str, pairs: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut skip: Option<&str> = None;
    for line in text.split_inclusive(['\n', '\r']) {
        let t = line.trim_start();
        match skip {
            Some(end) => {
                if t.starts_with(end) {
                    skip = None;
                }
            }
            None => {
                if let Some((_, end)) = pairs.iter().find(|(b, _)| t.starts_with(b)) {
                    skip = Some(end);
                } else {
                    out.push_str(line);
                }
            }
        }
    }
    out
}

/// Parse an EPS / PostScript / PostScript-based AI file into paths (mm, +Y up).
pub fn parse(bytes: &[u8]) -> Result<Art> {
    if bytes.len() > MAX_BYTES {
        return Err(FileError::TooLarge("the file is larger than 50 MB".into()));
    }
    let ps = postscript_section(bytes);
    let text = String::from_utf8_lossy(ps);
    if !text.trim_start().starts_with("%!") {
        return invalid("this is not a PostScript or EPS file (it does not start with %!)");
    }
    let ai = is_illustrator(&text);
    let body = if ai {
        strip_sections(&text, &[("%%BeginProlog", "%%EndProlog"), ("%%BeginSetup", "%%EndSetup"), ("%%BeginResource", "%%EndResource"), ("%%BeginData", "%%EndData"), ("%AI5_BeginPalette", "%AI5_EndPalette")])
    } else {
        strip_sections(&text, &[("%%BeginData", "%%EndData"), ("%%BeginBinary", "%%EndBinary")])
    };
    let toks = tokenize(&body)?;
    let mut it = Interp {
        stack: Vec::new(),
        dict: HashMap::new(),
        gs: GState { ctm: Affine::IDENTITY, color: None },
        saved: Vec::new(),
        path: BezPath::new(),
        current: None,
        start: None,
        art: Art::default(),
        ai,
        steps: 0,
        elements: 0,
        texts: 0,
        images: 0,
    };
    if let Err(e) = it.run(&toks, 0) {
        if it.art.paths.is_empty() {
            return Err(e);
        }
        it.art.warn(format!("the PostScript stopped early ({e}); what was drawn before is imported"));
    }
    if it.texts > 0 {
        it.art.warn(format!("{} text item(s) left out (convert text to outlines before saving)", it.texts));
    }
    if it.images > 0 {
        it.art.warn(format!("{} embedded image(s) left out", it.images));
    }
    if it.art.paths.is_empty() {
        return invalid("no vector paths found in the PostScript");
    }
    Ok(it.art)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;

    #[test]
    fn plain_postscript_paths_with_procedures_transforms_and_colours() {
        let eps = "%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 0 0 100 100\n/m {moveto} bind def /l /lineto load def\n\
                   gsave 72 0 translate 1 0 0 setrgbcolor\nnewpath 0 0 m 72 0 l 72 72 l closepath stroke grestore\n\
                   newpath 36 36 36 0 360 arc fill\n(hello) show\nshowpage\n%%EOF";
        let art = parse(eps.as_bytes()).unwrap();
        assert_eq!(art.paths.len(), 2);
        let b = art.paths[0].path.bounding_box();
        assert!((b.x0 - 25.4).abs() < 1e-9 && (b.x1 - 50.8).abs() < 1e-9 && (b.y1 - 25.4).abs() < 1e-9, "{b:?}");
        assert_eq!(art.paths[0].color, Some([255, 0, 0]));
        let c = art.paths[1].path.bounding_box();
        assert!((c.width() - 25.4).abs() < 1e-6 && (c.center().x - 12.7).abs() < 1e-6, "{c:?}");
        assert!(art.warnings[0].contains("1 text"));
    }

    #[test]
    fn illustrator_operators_and_skipped_prolog() {
        let ai = "%!PS-Adobe-3.0\n%%Creator: Adobe Illustrator(R) 8.0\n%%BeginProlog\n/m {garbage} def\n%%EndProlog\n\
                  0 0 1 0 k\n0 0 m\n72 0 L\n72 72 72 72 0 72 c\nf\n0 0 m 10 10 20 0 v\n30 5 36 0 y\nS\n%%Trailer\n";
        let art = parse(ai.as_bytes()).unwrap();
        assert_eq!(art.paths.len(), 2);
        assert_eq!(art.paths[0].color, Some([255, 255, 0]));
        assert!(art.paths[0].path.elements().iter().any(|e| matches!(e, kurbo::PathEl::ClosePath)), "f closes");
        assert!(!art.paths[1].path.elements().iter().any(|e| matches!(e, kurbo::PathEl::ClosePath)));
    }

    #[test]
    fn loops_rect_and_dos_header() {
        let ps = "%!PS\n0 1 2 { 20 mul 0 10 10 rectfill } for\n";
        let art = parse(ps.as_bytes()).unwrap();
        assert_eq!(art.paths.len(), 3);
        let mut dos = vec![0xC5, 0xD0, 0xD3, 0xC6];
        dos.extend_from_slice(&32u32.to_le_bytes());
        dos.extend_from_slice(&(ps.len() as u32).to_le_bytes());
        dos.resize(32, 0);
        dos.extend_from_slice(ps.as_bytes());
        assert_eq!(parse(&dos).unwrap().paths.len(), 3);
        assert!(parse(b"hello").is_err());
        assert!(parse(b"%!PS\n1 2 add\n").is_err(), "nothing drawn");
    }

    /// Hostile programs end quickly with an error (or what was drawn before), never hang, exhaust memory or overflow
    /// the stack.
    #[test]
    fn hostile_programs_are_bounded() {
        let started = std::time::Instant::now();
        let refused = |ps: &str| {
            let t = std::time::Instant::now();
            let r = parse(ps.as_bytes());
            assert!(t.elapsed().as_secs() < 20, "{ps:.60}: took {:?}", t.elapsed());
            r
        };
        // Empty procedures in nested loops used to cost no steps: 10^18 iterations.
        assert!(refused("%!PS\n1000000 { 1000000 { 1000000 { } repeat } repeat } repeat 0 0 moveto 1 1 lineto stroke").is_err());
        // A doubling stack (2^40 operands).
        assert!(refused("%!PS\nmark 1 40 { counttomark copy } repeat").is_err());
        // Arrays nested a million deep in a loop (dropping them recursed off the thread's stack).
        assert!(refused("%!PS\n/a [] def 1000000 { [ a ] /a exch def } repeat").is_err());
        let deep = format!("%!PS\n{}{}", "{".repeat(100_000), "}".repeat(100_000));
        assert!(refused(&deep).is_err());
        // Copying a long string many times shares it.
        let long = format!("%!PS\n({}) 1000000 {{ dup pop }} repeat 0 0 moveto 1 0 lineto stroke", "x".repeat(10_000_000));
        assert_eq!(refused(&long).unwrap().paths.len(), 1);
        // An arc of 1e300 degrees (adding turns never changed the angle; the sweep needed 10^298 segments).
        let arc = refused("%!PS\n0 0 10 1e300 0 arc stroke 0 0 10 0 1e300 arc stroke 0 0 10 0 -1e300 arcn stroke").unwrap();
        assert_eq!(arc.paths.len(), 3);
        assert!(arc.paths.iter().all(|p| p.path.elements().len() <= 10));
        // Endless line segments in one path.
        assert!(refused("%!PS\n0 0 moveto 1000000 { 1000000 { 1 1 lineto } repeat } repeat stroke").is_err());
        assert!(started.elapsed().as_secs() < 60);
    }

    #[test]
    fn arcs_follow_postscript_turn_rules() {
        let sweep = |ps: &str| {
            let art = parse(format!("%!PS\n{ps} stroke").as_bytes()).unwrap();
            let b = art.paths[0].path.bounding_box();
            (b.width() / MM_PER_PT, b.height() / MM_PER_PT)
        };
        // arc 0 -> 90 counter-clockwise: the upper right quarter.
        let (w, h) = sweep("0 0 10 0 90 arc");
        assert!((w - 10.0).abs() < 1e-6 && (h - 10.0).abs() < 1e-6, "{w} {h}");
        // arc 90 -> 0 adds a turn: three quarters.
        let (w, h) = sweep("0 0 10 90 0 arc");
        assert!((w - 20.0).abs() < 1e-6 && (h - 20.0).abs() < 1e-6, "{w} {h}");
        // arcn 0 -> 90 clockwise: three quarters too.
        let (w, h) = sweep("0 0 10 0 90 arcn");
        assert!((w - 20.0).abs() < 1e-6 && (h - 20.0).abs() < 1e-6, "{w} {h}");
    }
}
