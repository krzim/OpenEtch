//! 3D model import as a depth map (LightBurn 2.2's Import 3D Model / Create Depth Map), for 3D Sliced engraving.
//!
//! Reads STL (binary and ASCII), OBJ, OFF, PLY (ASCII and binary little/big endian, triangles and polygons), 3MF
//! (zip + XML, build items with their transforms and component references) and AMF (XML, plain or zipped). The model
//! is rotated (`rotation_deg` about X, then Y, then Z), seen from above (+Z towards the viewer), scaled to
//! `width_mm` (else its own size, the file's units taken as mm), and rendered with a z-buffer into a grey image at
//! `px_per_mm`: the highest point white, the floor black, like LightBurn's maps (black engraves deepest; the
//! background is black, full depth, or transparent). `depth_cutoff_mm` puts the floor that far below the top
//! (LightBurn's Depth Cutoff; anything lower is background), otherwise the model's lowest point (Automatic Depth
//! Cutoff). `invert` swaps light and dark on the model; `equalize` spreads the model's grey levels evenly
//! (histogram equalisation). 8-bit or 16-bit PNG.

use kurbo::Point;

use crate::{FileError, Result, invalid};

/// Most triangles read.
pub const MAX_TRIANGLES: usize = 20_000_000;
/// Most depth map pixels.
pub const MAX_PIXELS: u64 = 64_000_000;
/// Largest file read.
pub const MAX_BYTES: usize = 512 * 1024 * 1024;
/// Most component references a 3MF expands (components can refer to each other many times over).
pub const MAX_COMPONENT_VISITS: usize = 1_000_000;
/// Most pixel tests the depth map rendering does (the triangles' bounding boxes summed): many large triangles over a
/// big map would otherwise run for hours.
pub const MAX_RASTER_WORK: u64 = 2_000_000_000;

pub type Vec3 = [f64; 3];

/// Triangles of a model.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mesh {
    pub triangles: Vec<[Vec3; 3]>,
}

impl Mesh {
    fn push(&mut self, t: [Vec3; 3]) -> Result<()> {
        if self.triangles.len() >= MAX_TRIANGLES {
            return Err(FileError::TooLarge(format!("the model has more than {MAX_TRIANGLES} triangles")));
        }
        if t.iter().flatten().all(|v| v.is_finite()) {
            self.triangles.push(t);
        }
        Ok(())
    }

    /// Fan-triangulate a polygon.
    fn polygon(&mut self, verts: &[Vec3]) -> Result<()> {
        for k in 1..verts.len().saturating_sub(1) {
            self.push([verts[0], verts[k], verts[k + 1]])?;
        }
        Ok(())
    }
}

/// Model file formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Stl,
    Obj,
    Off,
    Ply,
    ThreeMf,
    Amf,
}

impl Format {
    /// From a file extension (`stl`, `obj`, `off`, `ply`, `3mf`, `amf`).
    pub fn from_extension(ext: &str) -> Option<Format> {
        Some(match ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
            "stl" => Format::Stl,
            "obj" => Format::Obj,
            "off" => Format::Off,
            "ply" => Format::Ply,
            "3mf" => Format::ThreeMf,
            "amf" => Format::Amf,
            _ => return None,
        })
    }

    /// Guess from the content.
    pub fn sniff(bytes: &[u8]) -> Option<Format> {
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).to_ascii_lowercase();
        if crate::zip::is_zip(bytes) {
            let names = crate::zip::names(bytes).ok()?;
            return Some(if names.iter().any(|n| n.to_ascii_lowercase().ends_with(".model")) { Format::ThreeMf } else { Format::Amf });
        }
        if head.starts_with("ply") {
            return Some(Format::Ply);
        }
        if head.starts_with("off") || head.starts_with("coff") || head.starts_with("noff") {
            return Some(Format::Off);
        }
        if head.contains("<amf") {
            return Some(Format::Amf);
        }
        if head.starts_with("solid") && head.contains("facet") {
            return Some(Format::Stl);
        }
        if head.lines().any(|l| l.starts_with("v ") || l.starts_with("f ")) {
            return Some(Format::Obj);
        }
        // Binary STL: 80-byte header, count, 50 bytes per triangle.
        if bytes.len() >= 84 {
            let n = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as usize;
            if 84 + n * 50 == bytes.len() {
                return Some(Format::Stl);
            }
        }
        None
    }
}

fn f32_at(b: &[u8], at: usize, le: bool) -> f64 {
    let a: [u8; 4] = b[at..at + 4].try_into().unwrap_or([0; 4]);
    f64::from(if le { f32::from_le_bytes(a) } else { f32::from_be_bytes(a) })
}

fn parse_stl(b: &[u8]) -> Result<Mesh> {
    let mut m = Mesh::default();
    let ascii = b.starts_with(b"solid") && String::from_utf8_lossy(&b[..b.len().min(1024)]).contains("facet");
    if ascii {
        let text = String::from_utf8_lossy(b);
        let mut verts: Vec<Vec3> = Vec::new();
        for line in text.lines() {
            let mut it = line.split_whitespace();
            match it.next() {
                Some("vertex") => {
                    let v: Vec<f64> = it.filter_map(|s| s.parse().ok()).collect();
                    if v.len() == 3 {
                        verts.push([v[0], v[1], v[2]]);
                    }
                }
                Some("endloop") => {
                    m.polygon(&verts)?;
                    verts.clear();
                }
                _ => {}
            }
        }
    } else {
        if b.len() < 84 {
            return invalid("the STL file is cut short");
        }
        let n = u32::from_le_bytes([b[80], b[81], b[82], b[83]]) as usize;
        if b.len() < 84 + n.saturating_mul(50) {
            return invalid("the STL file is cut short");
        }
        for i in 0..n {
            let at = 84 + i * 50 + 12;
            let v = |k: usize| [f32_at(b, at + k * 12, true), f32_at(b, at + k * 12 + 4, true), f32_at(b, at + k * 12 + 8, true)];
            m.push([v(0), v(1), v(2)])?;
        }
    }
    Ok(m)
}

fn parse_obj(text: &str) -> Result<Mesh> {
    let mut v: Vec<Vec3> = Vec::new();
    let mut m = Mesh::default();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        match it.next() {
            Some("v") => {
                let c: Vec<f64> = it.take(3).filter_map(|s| s.parse().ok()).collect();
                if c.len() == 3 {
                    v.push([c[0], c[1], c[2]]);
                }
            }
            Some("f") => {
                let idx: Vec<Vec3> = it
                    .filter_map(|s| s.split('/').next()?.parse::<i64>().ok())
                    .filter_map(|i| {
                        let k = if i < 0 { v.len() as i64 + i } else { i - 1 };
                        v.get(usize::try_from(k).ok()?).copied()
                    })
                    .collect();
                m.polygon(&idx)?;
            }
            _ => {}
        }
    }
    Ok(m)
}

fn parse_off(text: &str) -> Result<Mesh> {
    let mut toks = text.lines().map(|l| l.split('#').next().unwrap_or("")).flat_map(str::split_whitespace);
    let magic = toks.next().unwrap_or_default();
    if !magic.ends_with("OFF") {
        return invalid("not an OFF file");
    }
    let mut num = || toks.next().and_then(|t| t.parse::<f64>().ok());
    let (nv, nf) = (num().unwrap_or(0.0) as usize, num().unwrap_or(0.0) as usize);
    let _ = num();
    if nv > MAX_TRIANGLES * 3 || nf > MAX_TRIANGLES {
        return Err(FileError::TooLarge("the OFF model is too large".into()));
    }
    // The header's counts are only claims: read what the file holds (a short file declaring 60 million vertices
    // must not allocate or loop for them).
    let mut v = Vec::with_capacity(nv.min(text.len() / 6));
    for _ in 0..nv {
        let (Some(x), Some(y), Some(z)) = (num(), num(), num()) else { break };
        v.push([x, y, z]);
    }
    let mut m = Mesh::default();
    for _ in 0..nf {
        let Some(n) = num() else { break };
        let mut poly: Vec<Vec3> = Vec::new();
        for _ in 0..(n.max(0.0) as usize) {
            let Some(i) = num() else { break };
            if let Some(p) = v.get(i as usize) {
                poly.push(*p);
            }
        }
        m.polygon(&poly)?;
    }
    Ok(m)
}

/// A PLY element: name, count and properties (name, type, list count type).
type PlyElement = (String, usize, Vec<(String, String, Option<String>)>);

fn parse_ply(b: &[u8]) -> Result<Mesh> {
    let end = b.windows(10).position(|w| w == b"end_header").ok_or_else(|| FileError::Invalid("the PLY header has no end_header".into()))?;
    let header = String::from_utf8_lossy(&b[..end]);
    let mut body = end + 10;
    while body < b.len() && (b[body] == b'\r' || b[body] == b'\n') {
        body += 1;
        if b[body - 1] == b'\n' {
            break;
        }
    }
    let mut format = "ascii";
    // Elements in order: (name, count, properties (name, type, list count type)).
    let mut elements: Vec<PlyElement> = Vec::new();
    for line in header.lines() {
        let w: Vec<&str> = line.split_whitespace().collect();
        match w.as_slice() {
            ["format", f, ..] => format = if f.contains("big") { "big" } else if f.contains("little") { "little" } else { "ascii" },
            ["element", name, n] => elements.push((name.to_string(), n.parse().unwrap_or(0), Vec::new())),
            ["property", "list", ct, it, name] => {
                if let Some(e) = elements.last_mut() {
                    e.2.push((name.to_string(), it.to_string(), Some(ct.to_string())));
                }
            }
            ["property", t, name] => {
                if let Some(e) = elements.last_mut() {
                    e.2.push((name.to_string(), t.to_string(), None));
                }
            }
            _ => {}
        }
    }
    let size = |t: &str| match t {
        "char" | "uchar" | "int8" | "uint8" => 1,
        "short" | "ushort" | "int16" | "uint16" => 2,
        "double" | "float64" => 8,
        _ => 4,
    };
    let le = format == "little";
    let mut at = body;
    let ascii_text = String::from_utf8_lossy(&b[body..]).into_owned();
    let mut ascii = ascii_text.split_whitespace();
    let mut read = |t: &str| -> Option<f64> {
        if format == "ascii" {
            return ascii.next()?.parse().ok();
        }
        let n = size(t);
        let s = b.get(at..at + n)?;
        at += n;
        let arr = |k: usize| -> [u8; 8] {
            let mut a = [0u8; 8];
            if le {
                a[..k].copy_from_slice(s);
            } else {
                for (i, x) in s.iter().rev().enumerate() {
                    a[i] = *x;
                }
            }
            a
        };
        let a = arr(n);
        Some(match t {
            "char" | "int8" => f64::from(a[0] as i8),
            "uchar" | "uint8" => f64::from(a[0]),
            "short" | "int16" => f64::from(i16::from_le_bytes([a[0], a[1]])),
            "ushort" | "uint16" => f64::from(u16::from_le_bytes([a[0], a[1]])),
            "int" | "int32" => f64::from(i32::from_le_bytes([a[0], a[1], a[2], a[3]])),
            "uint" | "uint32" => f64::from(u32::from_le_bytes([a[0], a[1], a[2], a[3]])),
            "double" | "float64" => f64::from_le_bytes(a),
            _ => f64::from(f32::from_le_bytes([a[0], a[1], a[2], a[3]])),
        })
    };
    let mut v: Vec<Vec3> = Vec::new();
    let mut m = Mesh::default();
    // The header's counts are only claims: reading stops where the data ends (a short file declaring millions of
    // elements must not loop or allocate for them).
    'elements: for (name, count, props) in &elements {
        if *count > MAX_TRIANGLES * 3 {
            return Err(FileError::TooLarge("the PLY model is too large".into()));
        }
        for _ in 0..*count {
            let mut xyz = [0.0; 3];
            let mut face: Vec<usize> = Vec::new();
            for (pname, t, list) in props {
                match list {
                    Some(ct) => {
                        let Some(n) = read(ct) else { break 'elements };
                        for _ in 0..(n.max(0.0) as usize).min(1 << 16) {
                            let Some(i) = read(t) else { break 'elements };
                            face.push(i as usize);
                        }
                    }
                    None => {
                        let Some(x) = read(t) else { break 'elements };
                        match pname.as_str() {
                            "x" => xyz[0] = x,
                            "y" => xyz[1] = x,
                            "z" => xyz[2] = x,
                            _ => {}
                        }
                    }
                }
            }
            if name == "vertex" {
                v.push(xyz);
            } else if name == "face" {
                let poly: Vec<Vec3> = face.iter().filter_map(|&i| v.get(i).copied()).collect();
                m.polygon(&poly)?;
            }
        }
    }
    Ok(m)
}

fn attr(n: roxmltree::Node, k: &str) -> f64 {
    n.attribute(k).and_then(|v| v.trim().parse().ok()).unwrap_or(0.0)
}

/// A 3MF `transform` (3 x 4, row vectors) as a function.
fn mf_transform(s: Option<&str>) -> [f64; 12] {
    let v: Vec<f64> = s.unwrap_or("").split_whitespace().filter_map(|x| x.parse().ok()).collect();
    if v.len() == 12 { v.try_into().unwrap_or([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]) } else { [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0] }
}

fn apply(t: &[f64; 12], p: Vec3) -> Vec3 {
    [
        p[0] * t[0] + p[1] * t[3] + p[2] * t[6] + t[9],
        p[0] * t[1] + p[1] * t[4] + p[2] * t[7] + t[10],
        p[0] * t[2] + p[1] * t[5] + p[2] * t[8] + t[11],
    ]
}

fn compose(a: &[f64; 12], b: &[f64; 12]) -> [f64; 12] {
    // a then b.
    let r = |p: Vec3| apply(b, apply(a, p));
    let o = r([0.0; 3]);
    let x = r([1.0, 0.0, 0.0]);
    let y = r([0.0, 1.0, 0.0]);
    let z = r([0.0, 0.0, 1.0]);
    [x[0] - o[0], x[1] - o[1], x[2] - o[2], y[0] - o[0], y[1] - o[1], y[2] - o[2], z[0] - o[0], z[1] - o[1], z[2] - o[2], o[0], o[1], o[2]]
}

/// Objects of all model parts, by `"<part>#<id>"`: their own triangles and their components (object key, transform).
type Objects = std::collections::HashMap<String, (Vec<[Vec3; 3]>, Vec<(String, [f64; 12])>)>;

/// A part name as a key: no leading slash, lower case.
fn part_key(name: &str) -> String {
    name.trim_start_matches('/').to_ascii_lowercase()
}

fn parse_3mf(bytes: &[u8]) -> Result<(Mesh, Vec<String>)> {
    let entries = crate::zip::read(bytes)?;
    let parts: Vec<&crate::zip::Entry> = entries.iter().filter(|e| e.name.to_ascii_lowercase().ends_with(".model")).collect();
    let root = parts
        .iter()
        .find(|e| e.name.eq_ignore_ascii_case("3D/3dmodel.model"))
        .or_else(|| parts.first())
        .ok_or_else(|| FileError::Invalid("the 3MF has no 3D model part".into()))?;
    let root_key = part_key(&root.name);
    let mut warnings = Vec::new();
    let mut objects: Objects = Default::default();
    let mut items: Vec<(String, [f64; 12])> = Vec::new();
    let mut unit = String::from("millimeter");
    for part in &parts {
        let key = part_key(&part.name);
        let text = String::from_utf8_lossy(&part.data);
        let xml = roxmltree::Document::parse_with_options(&text, roxmltree::ParsingOptions { allow_dtd: false, ..Default::default() })
            .map_err(|e| FileError::Invalid(format!("the 3MF model XML cannot be read ({e})")))?;
        for obj in xml.descendants().filter(|n| n.has_tag_name("object")) {
            let Some(id) = obj.attribute("id") else { continue };
            let mut tris = Vec::new();
            if let Some(mesh) = obj.children().find(|n| n.has_tag_name("mesh")) {
                let verts: Vec<Vec3> = mesh
                    .descendants()
                    .filter(|n| n.has_tag_name("vertex"))
                    .map(|v| [attr(v, "x"), attr(v, "y"), attr(v, "z")])
                    .collect();
                for t in mesh.descendants().filter(|n| n.has_tag_name("triangle")) {
                    let idx = |k: &str| t.attribute(k).and_then(|v| v.parse::<usize>().ok()).and_then(|i| verts.get(i).copied());
                    if let (Some(a), Some(b), Some(c)) = (idx("v1"), idx("v2"), idx("v3")) {
                        tris.push([a, b, c]);
                    }
                }
            }
            let comps = obj
                .descendants()
                .filter(|n| n.has_tag_name("component"))
                .filter_map(|c| {
                    let oid = c.attribute("objectid")?;
                    // The production extension's `p:path` names the part holding the object.
                    let in_part = c.attributes().find(|a| a.name() == "path").map_or(key.clone(), |a| part_key(a.value()));
                    Some((format!("{in_part}#{oid}"), mf_transform(c.attribute("transform"))))
                })
                .collect();
            objects.insert(format!("{key}#{id}"), (tris, comps));
        }
        if key == root_key {
            unit = xml.root_element().attribute("unit").unwrap_or("millimeter").to_string();
            items = xml
                .descendants()
                .filter(|n| n.has_tag_name("item"))
                .filter_map(|n| {
                    let in_part = n.attributes().find(|a| a.name() == "path").map_or(key.clone(), |a| part_key(a.value()));
                    Some((format!("{in_part}#{}", n.attribute("objectid")?), mf_transform(n.attribute("transform"))))
                })
                .collect();
        }
    }
    let mut m = Mesh::default();
    // `visits` counts every object reference expanded: objects referring to themselves (or each other) many times
    // over would otherwise expand without end when they hold no triangles to hit the triangle limit.
    fn emit(m: &mut Mesh, objects: &Objects, id: &str, t: &[f64; 12], depth: usize, visits: &mut usize) -> Result<()> {
        *visits += 1;
        if *visits > MAX_COMPONENT_VISITS {
            return Err(FileError::TooLarge("the 3MF components refer to each other too many times".into()));
        }
        let Some((tris, comps)) = objects.get(id) else { return Ok(()) };
        if depth > 16 {
            return Ok(());
        }
        for tri in tris {
            m.push(tri.map(|p| apply(t, p)))?;
        }
        for (cid, ct) in comps {
            emit(m, objects, cid, &compose(ct, t), depth + 1, visits)?;
        }
        Ok(())
    }
    let mut visits = 0;
    if items.is_empty() {
        let mut keys: Vec<String> = objects.keys().cloned().collect();
        keys.sort();
        for id in keys {
            emit(&mut m, &objects, &id, &mf_transform(None), 0, &mut visits)?;
        }
    } else {
        for (id, t) in &items {
            emit(&mut m, &objects, id, t, 0, &mut visits)?;
        }
    }
    let k = match unit.as_str() {
        "micron" => 0.001,
        "centimeter" => 10.0,
        "inch" => 25.4,
        "foot" => 304.8,
        "meter" => 1000.0,
        _ => 1.0,
    };
    if k != 1.0 {
        for t in &mut m.triangles {
            for p in t.iter_mut() {
                for c in p.iter_mut() {
                    *c *= k;
                }
            }
        }
        warnings.push(format!("model units: {unit} (converted to mm)"));
    }
    Ok((m, warnings))
}

fn parse_amf(bytes: &[u8]) -> Result<Mesh> {
    let data = if crate::zip::is_zip(bytes) {
        crate::zip::read(bytes)?.into_iter().next().map(|e| e.data).ok_or_else(|| FileError::Invalid("the AMF archive is empty".into()))?
    } else {
        bytes.to_vec()
    };
    let text = String::from_utf8_lossy(&data);
    let xml = roxmltree::Document::parse_with_options(&text, roxmltree::ParsingOptions { allow_dtd: false, ..Default::default() })
        .map_err(|e| FileError::Invalid(format!("the AMF XML cannot be read ({e})")))?;
    let mut m = Mesh::default();
    let num = |n: roxmltree::Node, tag: &str| n.children().find(|c| c.has_tag_name(tag)).and_then(|c| c.text()).and_then(|t| t.trim().parse::<f64>().ok());
    for mesh in xml.descendants().filter(|n| n.has_tag_name("mesh")) {
        let verts: Vec<Vec3> = mesh
            .descendants()
            .filter(|n| n.has_tag_name("coordinates"))
            .map(|c| [num(c, "x").unwrap_or(0.0), num(c, "y").unwrap_or(0.0), num(c, "z").unwrap_or(0.0)])
            .collect();
        for t in mesh.descendants().filter(|n| n.has_tag_name("triangle")) {
            let v = |tag: &str| num(t, tag).and_then(|i| verts.get(i as usize).copied());
            if let (Some(a), Some(b), Some(c)) = (v("v1"), v("v2"), v("v3")) {
                m.push([a, b, c])?;
            }
        }
    }
    Ok(m)
}

/// Read a model.
pub fn load(bytes: &[u8], format: Option<Format>) -> Result<(Mesh, Vec<String>)> {
    if bytes.len() > MAX_BYTES {
        return Err(FileError::TooLarge("the model file is larger than 512 MB".into()));
    }
    let f = format.or_else(|| Format::sniff(bytes)).ok_or_else(|| FileError::Unsupported("not a 3D model format OpenEtch reads (STL, OBJ, OFF, PLY, 3MF, AMF)".into()))?;
    let (m, w) = match f {
        Format::Stl => (parse_stl(bytes)?, vec![]),
        Format::Obj => (parse_obj(&String::from_utf8_lossy(bytes))?, vec![]),
        Format::Off => (parse_off(&String::from_utf8_lossy(bytes))?, vec![]),
        Format::Ply => (parse_ply(bytes)?, vec![]),
        Format::ThreeMf => parse_3mf(bytes)?,
        Format::Amf => (parse_amf(bytes)?, vec![]),
    };
    if m.triangles.is_empty() {
        return invalid("the model has no triangles");
    }
    Ok((m, w))
}

#[derive(Debug, Clone, PartialEq)]
pub struct DepthOptions {
    /// Rotation about X, then Y, then Z, degrees.
    pub rotation_deg: Vec3,
    /// Width of the map in mm (the model's X extent after rotation); default the model's own size.
    pub width_mm: Option<f64>,
    /// Depth map resolution, pixels per mm.
    pub px_per_mm: f64,
    pub sixteen_bit: bool,
    pub invert: bool,
    pub transparent_background: bool,
    /// Floor this far below the top (mm, after scaling); `None`: the model's lowest point.
    pub depth_cutoff_mm: Option<f64>,
    pub equalize: bool,
}

impl Default for DepthOptions {
    fn default() -> Self {
        DepthOptions {
            rotation_deg: [0.0; 3],
            width_mm: None,
            px_per_mm: 10.0,
            sixteen_bit: false,
            invert: false,
            transparent_background: false,
            depth_cutoff_mm: None,
            equalize: false,
        }
    }
}

/// A rendered depth map.
#[derive(Debug, Clone, PartialEq)]
pub struct DepthMap {
    pub png: Vec<u8>,
    pub width_px: u32,
    pub height_px: u32,
    pub width_mm: f64,
    pub height_mm: f64,
    /// Depth from the top to the floor, mm.
    pub depth_mm: f64,
    /// The model's outline seen from above (pixel-edge polygons, LightBurn's Generate Outline), in map mm centred on
    /// the map's middle, +Y up.
    pub outline: Vec<Vec<Point>>,
}

fn rotate(p: Vec3, r: Vec3) -> Vec3 {
    let (sx, cx) = r[0].to_radians().sin_cos();
    let (sy, cy) = r[1].to_radians().sin_cos();
    let (sz, cz) = r[2].to_radians().sin_cos();
    let p = [p[0], p[1] * cx - p[2] * sx, p[1] * sx + p[2] * cx];
    let p = [p[0] * cy + p[2] * sy, p[1], -p[0] * sy + p[2] * cy];
    [p[0] * cz - p[1] * sz, p[0] * sz + p[1] * cz, p[2]]
}

/// Render a depth map of `mesh`.
pub fn depth_map(mesh: &Mesh, o: &DepthOptions) -> Result<DepthMap> {
    if !(o.px_per_mm.is_finite() && o.px_per_mm > 0.0) {
        return invalid("the resolution must be a positive number of pixels per mm");
    }
    let tris: Vec<[Vec3; 3]> = mesh.triangles.iter().map(|t| t.map(|p| rotate(p, o.rotation_deg))).collect();
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for p in tris.iter().flatten() {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let (w0, h0) = (hi[0] - lo[0], hi[1] - lo[1]);
    if !(w0 > 0.0 && h0 > 0.0) {
        return invalid("the model is flat when seen from above; rotate it");
    }
    let s = match o.width_mm {
        Some(w) if w.is_finite() && w > 0.0 => w / w0,
        Some(_) => return invalid("the width must be a positive number of mm"),
        None => 1.0,
    };
    let (wmm, hmm) = (w0 * s, h0 * s);
    let (wp, hp) = ((wmm * o.px_per_mm - 1e-6).ceil().max(1.0), (hmm * o.px_per_mm - 1e-6).ceil().max(1.0));
    if wp * hp > MAX_PIXELS as f64 {
        return Err(FileError::TooLarge(format!("a {wp} x {hp} pixel depth map is too large; lower the resolution")));
    }
    let (wp, hp) = (wp as usize, hp as usize);
    let k = s * o.px_per_mm;
    // Pixel coordinates (x right, y down; z scaled to mm) and the pixel box of a triangle.
    let place = |t: &[Vec3; 3]| {
        let q = t.map(|p| [(p[0] - lo[0]) * k, (hi[1] - p[1]) * k, (p[2] - lo[2]) * s]);
        let x0 = q.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min).floor().max(0.0) as usize;
        let x1 = (q.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max).ceil() as usize).min(wp);
        let y0 = q.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min).floor().max(0.0) as usize;
        let y1 = (q.iter().map(|p| p[1]).fold(f64::NEG_INFINITY, f64::max).ceil() as usize).min(hp);
        (q, x0, x1, y0, y1)
    };
    // The work is known before any is done: refuse at once rather than after hours.
    let work: u64 = tris
        .iter()
        .map(|t| {
            let (_, x0, x1, y0, y1) = place(t);
            (x1.saturating_sub(x0) as u64) * (y1.saturating_sub(y0) as u64)
        })
        .fold(0u64, u64::saturating_add);
    if work > MAX_RASTER_WORK {
        return Err(FileError::TooLarge("the model is too complex for a depth map at this resolution; lower it".into()));
    }
    let mut z = vec![f64::NEG_INFINITY; wp * hp];
    for t in &tris {
        let (q, x0, x1, y0, y1) = place(t);
        let area = (q[1][0] - q[0][0]) * (q[2][1] - q[0][1]) - (q[2][0] - q[0][0]) * (q[1][1] - q[0][1]);
        if area.abs() < 1e-12 {
            continue;
        }
        for py in y0..y1 {
            let cy = py as f64 + 0.5;
            for px in x0..x1 {
                let cx = px as f64 + 0.5;
                let w0 = ((q[1][0] - cx) * (q[2][1] - cy) - (q[2][0] - cx) * (q[1][1] - cy)) / area;
                let w1 = ((q[2][0] - cx) * (q[0][1] - cy) - (q[0][0] - cx) * (q[2][1] - cy)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 >= -1e-9 && w1 >= -1e-9 && w2 >= -1e-9 {
                    let d = w0 * q[0][2] + w1 * q[1][2] + w2 * q[2][2];
                    let i = py * wp + px;
                    if d > z[i] {
                        z[i] = d;
                    }
                }
            }
        }
    }
    let top = (hi[2] - lo[2]) * s;
    let floor = match o.depth_cutoff_mm {
        Some(c) if c.is_finite() && c > 0.0 => (top - c).max(0.0),
        Some(_) => return invalid("the depth cutoff must be a positive number of mm"),
        None => 0.0,
    };
    let range = (top - floor).max(1e-9);
    // Model levels 0..1 (1 = top) or None for background.
    let mut level: Vec<Option<f64>> = z.iter().map(|&d| (d.is_finite() && d >= floor - 1e-9).then(|| ((d - floor) / range).clamp(0.0, 1.0))).collect();
    if o.equalize {
        let mut sorted: Vec<f64> = level.iter().flatten().copied().collect();
        sorted.sort_by(f64::total_cmp);
        let n = sorted.len().max(1) as f64;
        for l in level.iter_mut().flatten() {
            let rank = sorted.partition_point(|v| *v < *l) as f64;
            *l = rank / n;
        }
    }
    if o.invert {
        for l in level.iter_mut().flatten() {
            *l = 1.0 - *l;
        }
    }
    let max = if o.sixteen_bit { 65535.0 } else { 255.0 };
    let value = |l: Option<f64>| l.map_or(0.0, |l| (l * max).round());
    let img = match (o.sixteen_bit, o.transparent_background) {
        (false, false) => image::DynamicImage::ImageLuma8(image::GrayImage::from_raw(wp as u32, hp as u32, level.iter().map(|l| value(*l) as u8).collect()).ok_or_else(|| FileError::Invalid("bad size".into()))?),
        (false, true) => image::DynamicImage::ImageLumaA8(
            image::GrayAlphaImage::from_raw(wp as u32, hp as u32, level.iter().flat_map(|l| [value(*l) as u8, if l.is_some() { 255 } else { 0 }]).collect())
                .ok_or_else(|| FileError::Invalid("bad size".into()))?,
        ),
        (true, false) => image::DynamicImage::ImageLuma16(
            image::ImageBuffer::from_raw(wp as u32, hp as u32, level.iter().map(|l| value(*l) as u16).collect()).ok_or_else(|| FileError::Invalid("bad size".into()))?,
        ),
        (true, true) => image::DynamicImage::ImageLumaA16(
            image::ImageBuffer::from_raw(wp as u32, hp as u32, level.iter().flat_map(|l| [value(*l) as u16, if l.is_some() { 65535 } else { 0 }]).collect())
                .ok_or_else(|| FileError::Invalid("bad size".into()))?,
        ),
    };
    let png = crate::bitmap::png(&img, Some(o.px_per_mm * 25.4))?;
    // Outline: trace the model's silhouette.
    let mask: Vec<bool> = level.iter().map(Option::is_some).collect();
    let to_mm = |p: Point| Point::new(p.x / o.px_per_mm - wmm / 2.0, hmm / 2.0 - p.y / o.px_per_mm);
    let outline = crate::trace::contours(&mask, wp, hp).into_iter().map(|c| c.into_iter().map(to_mm).collect()).collect();
    Ok(DepthMap { png, width_px: wp as u32, height_px: hp as u32, width_mm: wp as f64 / o.px_per_mm, height_mm: hp as f64 / o.px_per_mm, depth_mm: range, outline })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 10 x 10 x 5 mm box as binary STL.
    fn box_stl() -> Vec<u8> {
        let v = |x: f64, y: f64, z: f64| [x, y, z];
        let quads = [
            [v(0.0, 0.0, 5.0), v(10.0, 0.0, 5.0), v(10.0, 10.0, 5.0), v(0.0, 10.0, 5.0)],
            [v(0.0, 0.0, 0.0), v(0.0, 10.0, 0.0), v(10.0, 10.0, 0.0), v(10.0, 0.0, 0.0)],
        ];
        let mut tris = Vec::new();
        for q in quads {
            tris.push([q[0], q[1], q[2]]);
            tris.push([q[0], q[2], q[3]]);
        }
        // A ramp on one side, from z 0 at x 10 to 5 at x 0... keep simple: a sloped quad over x 10..20.
        tris.push([v(10.0, 0.0, 5.0), v(20.0, 0.0, 0.0), v(20.0, 10.0, 0.0)]);
        tris.push([v(10.0, 0.0, 5.0), v(20.0, 10.0, 0.0), v(10.0, 10.0, 5.0)]);
        let mut b = vec![0u8; 80];
        b.extend_from_slice(&(tris.len() as u32).to_le_bytes());
        for t in tris {
            b.extend_from_slice(&[0u8; 12]);
            for p in t {
                for c in p {
                    b.extend_from_slice(&(c as f32).to_le_bytes());
                }
            }
            b.extend_from_slice(&[0, 0]);
        }
        b
    }

    #[test]
    fn stl_depth_map_is_white_on_top_and_ramps_down() {
        let (m, _) = load(&box_stl(), None).unwrap();
        assert_eq!(m.triangles.len(), 6);
        let d = depth_map(&m, &DepthOptions { px_per_mm: 1.0, ..DepthOptions::default() }).unwrap();
        assert_eq!((d.width_px, d.height_px), (20, 10));
        let img = image::load_from_memory(&d.png).unwrap().into_luma8();
        assert_eq!(img.get_pixel(2, 5).0, [255], "top of the box");
        let ramp = img.get_pixel(15, 5).0[0];
        assert!((110..=120).contains(&ramp), "z 2.25 of 5 at x 15.5: {ramp}");
        assert!(img.get_pixel(19, 5).0[0] < 20);
        assert!((d.depth_mm - 5.0).abs() < 1e-9);
        assert_eq!(d.outline.len(), 1);
        // Inverted, scaled to 40 mm wide, transparent background and 16-bit.
        let o = DepthOptions { px_per_mm: 0.5, width_mm: Some(40.0), invert: true, transparent_background: true, sixteen_bit: true, ..DepthOptions::default() };
        let d = depth_map(&m, &o).unwrap();
        assert_eq!((d.width_px, d.height_px), (20, 10));
        let img = image::load_from_memory(&d.png).unwrap();
        assert!(matches!(img, image::DynamicImage::ImageLumaA16(_)));
    }

    #[test]
    fn cutoff_and_rotation() {
        let (m, _) = load(&box_stl(), Some(Format::Stl)).unwrap();
        let d = depth_map(&m, &DepthOptions { px_per_mm: 1.0, depth_cutoff_mm: Some(2.5), transparent_background: true, ..DepthOptions::default() }).unwrap();
        let img = image::load_from_memory(&d.png).unwrap().into_luma_alpha8();
        assert_eq!(img.get_pixel(19, 5).0[1], 0, "below the cutoff: background");
        assert_eq!(img.get_pixel(2, 5).0, [255, 255]);
        // Seen from the side (rotated 90° about X) the box is 20 x 5.
        let d = depth_map(&m, &DepthOptions { px_per_mm: 1.0, rotation_deg: [90.0, 0.0, 0.0], ..DepthOptions::default() }).unwrap();
        assert_eq!((d.width_px, d.height_px), (20, 5));
    }

    #[test]
    fn text_formats_and_3mf() {
        let obj = "v 0 0 0\nv 10 0 0\nv 10 10 1\nv 0 10 1\nf 1 2 3 4\n";
        let (m, _) = load(obj.as_bytes(), None).unwrap();
        assert_eq!(m.triangles.len(), 2);
        let off = "OFF\n4 1 0\n0 0 0\n10 0 0\n10 10 1\n0 10 1\n4 0 1 2 3\n";
        assert_eq!(load(off.as_bytes(), None).unwrap().0.triangles.len(), 2);
        let ply = "ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n0 0 0\n1 0 0\n0 1 1\n3 0 1 2\n";
        assert_eq!(load(ply.as_bytes(), None).unwrap().0.triangles.len(), 1);
        let stl = "solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid t\n";
        assert_eq!(load(stl.as_bytes(), None).unwrap().0.triangles.len(), 1);
        let model = r#"<?xml version="1.0"?><model unit="centimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><resources><object id="1" type="model"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="1"/></vertices><triangles><triangle v1="0" v2="1" v3="2"/></triangles></mesh></object></resources><build><item objectid="1" transform="1 0 0 0 1 0 0 0 1 5 0 0"/></build></model>"#;
        let z = crate::zip::write(&[crate::zip::Entry { name: "3D/3dmodel.model".into(), data: model.as_bytes().to_vec() }]).unwrap();
        assert_eq!(Format::sniff(&z), Some(Format::ThreeMf));
        let (m, w) = load(&z, None).unwrap();
        assert_eq!(m.triangles[0][0], [50.0, 0.0, 0.0], "item transform then centimetres");
        assert!(w[0].contains("centimeter"));
        assert!(load(b"hello", None).is_err());
    }

    /// Headers that claim far more than the file holds, self-referring 3MF components and huge renders end quickly.
    #[test]
    fn hostile_models_are_bounded() {
        let t = std::time::Instant::now();
        // 60 million vertices claimed (1.4 GB of zeros were pushed), and a face of 10^18 corners.
        assert!(load(b"OFF\n59999999 1 0\n0 0 0\n1 0 0\n0 1 0\n", None).is_err());
        let r = load(b"OFF\n3 1 0\n0 0 0\n1 0 0\n0 1 0\n1000000000000000000 0 1 2\n", None).unwrap();
        assert_eq!(r.0.triangles.len(), 1);
        let ply = "ply\nformat ascii 1.0\nelement vertex 59999999\nproperty float x\nproperty float y\nproperty float z\nelement face 19999999\nproperty list uchar int vertex_indices\nend_header\n0 0 0\n1 0 0\n0 1 1\n";
        assert!(load(ply.as_bytes(), None).is_err(), "no faces");
        let bin = "ply\nformat binary_little_endian 1.0\nelement face 19999999\nproperty list uint int vertex_indices\nend_header\n";
        assert!(load(bin.as_bytes(), None).is_err());
        // An object holding nothing but 100 references to itself: 100^16 expansions.
        let comps = r#"<component objectid="1"/>"#.repeat(100);
        let model = format!(r#"<?xml version="1.0"?><model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><resources><object id="1"><components>{comps}</components></object></resources><build><item objectid="1"/></build></model>"#);
        let z = crate::zip::write(&[crate::zip::Entry { name: "3D/3dmodel.model".into(), data: model.into_bytes() }]).unwrap();
        assert!(matches!(load(&z, None), Err(FileError::TooLarge(_))));
        // 100 000 triangles each covering a 64-megapixel map.
        let big = [[0.0, 0.0, 0.0], [8000.0, 0.0, 0.0], [0.0, 8000.0, 1.0]];
        let mesh = Mesh { triangles: vec![big; 100_000] };
        let r = depth_map(&mesh, &DepthOptions { px_per_mm: 1.0, ..DepthOptions::default() });
        assert!(matches!(r, Err(FileError::TooLarge(_))), "{:?}", r.map(|d| d.width_px));
        assert!(t.elapsed().as_secs() < 30, "{:?}", t.elapsed());
    }

    /// The owner's STL and 3MF samples (OPENETCH_LBRN_SAMPLES), read in place.
    #[test]
    fn real_models_render() {
        let Some(dirs) = std::env::var_os("OPENETCH_LBRN_SAMPLES") else { return };
        let mut n = 0;
        let mut stack: Vec<std::path::PathBuf> = std::env::split_paths(&dirs).collect();
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().to_string();
                if p.is_dir() {
                    if !(name.starts_with('.') || name == "target" || name == "node_modules" || p.join("Cargo.toml").exists()) {
                        stack.push(p);
                    }
                    continue;
                }
                let Some(f) = p.extension().and_then(|x| Format::from_extension(&x.to_string_lossy())) else { continue };
                let bytes = std::fs::read(&p).unwrap();
                let (m, _) = load(&bytes, Some(f)).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
                let d = depth_map(&m, &DepthOptions { px_per_mm: 2.0, ..DepthOptions::default() }).unwrap();
                assert!(d.width_px > 0 && !d.outline.is_empty(), "{}", p.display());
                n += 1;
            }
        }
        eprintln!("{n} model sample(s) rendered");
    }
}
