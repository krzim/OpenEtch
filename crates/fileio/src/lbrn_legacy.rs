//! Legacy LightBurn projects (`.lbrn`, LightBurn before 0.9.x's `.lbrn2`).
//!
//! Both formats share their structure and cut settings (LightBurn's forum: "both file types use the same overall
//! structure, and the cut settings are stored identically"). The difference is in paths: `.lbrn` writes each vertex
//! and primitive as an element, `<V vx="357" vy="141" c0x=".." c0y=".." c1x=".." c1y=".."/>` and
//! `<P T="L" p0="0" p1="1"/>` (`T` is `L` line or `B` Bézier), where `.lbrn2` packs them into `<VertList>` /
//! `<PrimList>` text (`V357 141c0x..c0y..c1x..c1y..`, `L0 1B1 2`). [`to_lbrn2`] rewrites the elements into that text
//! and leaves the rest of the file byte for byte, so the `.lbrn2` reader does everything else. No `.lbrn` file was
//! available as a sample: the element names come from LightBurn's forum (a LightBurn developer's description) and
//! the lbrnts project's model, and are tested on synthetic files.

use crate::{FileError, Result};

/// Whether the XML uses the legacy vertex elements.
pub fn is_legacy(xml: &str) -> bool {
    xml.contains("<V ") && xml.contains("vx=") && !xml.contains("<VertList")
}

/// Rewrite a legacy `.lbrn` file as `.lbrn2` text (unchanged when it has no legacy paths).
pub fn to_lbrn2(xml: &str) -> Result<String> {
    if !is_legacy(xml) {
        return Ok(xml.to_string());
    }
    let doc = roxmltree::Document::parse_with_options(xml, roxmltree::ParsingOptions { allow_dtd: false, ..Default::default() })
        .map_err(|e| FileError::Invalid(format!("the LightBurn file is not valid XML ({e})")))?;
    if !doc.root_element().has_tag_name("LightBurnProject") {
        return Err(FileError::Invalid("not a LightBurn project".into()));
    }
    // (start, end, replacement) per shape, in file order.
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for shape in doc.descendants().filter(|n| n.has_tag_name("Shape")) {
        let kids: Vec<roxmltree::Node> = shape.children().filter(|n| n.has_tag_name("V") || n.has_tag_name("P")).collect();
        let (Some(first), Some(last)) = (kids.first(), kids.last()) else { continue };
        let mut verts = String::new();
        let mut prims = String::new();
        for k in &kids {
            if k.has_tag_name("V") {
                let a = |n: &str| k.attribute(n);
                verts.push_str(&format!("V{} {}", a("vx").unwrap_or("0"), a("vy").unwrap_or("0")));
                if let (Some(x), Some(y)) = (a("c0x"), a("c0y")) {
                    verts.push_str(&format!("c0x{x}c0y{y}"));
                }
                if let (Some(x), Some(y)) = (a("c1x"), a("c1y")) {
                    verts.push_str(&format!("c1x{x}c1y{y}"));
                }
            } else {
                let t = match k.attribute("T").unwrap_or("L") {
                    "B" | "b" => "B",
                    _ => "L",
                };
                prims.push_str(&format!("{t}{} {}", k.attribute("p0").unwrap_or("0"), k.attribute("p1").unwrap_or("0")));
            }
        }
        let text = format!("<VertList>{verts}</VertList><PrimList>{prims}</PrimList>");
        edits.push((first.range().start, last.range().end, text));
    }
    let mut out = String::with_capacity(xml.len());
    let mut at = 0;
    for (s, e, t) in edits {
        if s < at {
            continue;
        }
        out.push_str(&xml[at..s]);
        out.push_str(&t);
        at = e;
    }
    out.push_str(&xml[at..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEGACY: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<LightBurnProject AppVersion="0.8.07" FormatVersion="0" MaterialHeight="0" MirrorX="False" MirrorY="False">
    <CutSetting type="Cut">
        <index Value="0"/>
        <name Value="C00"/>
    </CutSetting>
    <Shape Type="Path" CutIndex="0">
        <XForm>1 0 0 1 0 0</XForm>
        <V vx="357" vy="141" c1x="293.41559" c1y="102.6198"/>
        <V vx="439" vy="111" c0x="524.99817" c0y="109.78444"/>
        <V vx="400" vy="50"/>
        <P T="L" p0="0" p1="1"/>
        <P T="B" p0="1" p1="2"/>
    </Shape>
</LightBurnProject>
"#;

    #[test]
    fn vertex_and_primitive_elements_become_lists() {
        assert!(is_legacy(LEGACY));
        let out = to_lbrn2(LEGACY).unwrap();
        assert!(out.contains("<VertList>V357 141c1x293.41559c1y102.6198V439 111c0x524.99817c0y109.78444V400 50</VertList><PrimList>L0 1B1 2</PrimList>"), "{out}");
        assert!(out.contains(r#"<XForm>1 0 0 1 0 0</XForm>"#) && out.contains("FormatVersion=\"0\""));
        assert!(!is_legacy(&out));
        roxmltree::Document::parse(&out).unwrap();
        // .lbrn2 text passes through.
        assert_eq!(to_lbrn2("<LightBurnProject><Shape><VertList>V1 2</VertList></Shape></LightBurnProject>").unwrap(), "<LightBurnProject><Shape><VertList>V1 2</VertList></Shape></LightBurnProject>");
    }
}
