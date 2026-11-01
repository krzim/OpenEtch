//! LightBurn's shapes XML (`<LightBurnShapes>`): the art of an Art Library item (`.lbart`, `fileio::lbart`) and the
//! clipboard. It holds `<Shape>`s exactly as a project does, with no cut settings (a shape's `CutIndex` names its
//! colour layer) and no thumbnail, notes or settings. LightBurn 2.2 writes the root as
//! `<LightBurnShapes FormatVersion="1" MirrorX="False" MirrorY="False">` after the usual XML declaration.

use doc::Document;

use crate::{Frame, Imported, LbrnError, LbrnProject, from_document_in, parse_shapes, to_document_in, write_shapes};

/// The root attributes LightBurn 2.2 writes.
pub const SHAPES_ROOT: [(&str, &str); 3] = [("FormatVersion", "1"), ("MirrorX", "False"), ("MirrorY", "False")];

/// Shapes XML to objects (as [`crate::to_document_in`]; layers are named after their colour index, with
/// OpenEtch's default settings, without the warnings about missing cut settings and the field that do not apply).
pub fn shapes_to_document(text: &str, frame: Frame) -> Result<Imported, LbrnError> {
    let p = parse_shapes(text)?;
    let mut i = to_document_in(&p, frame)?;
    // Art has no cut settings, and its positions are where it was drawn, not a place in this field.
    i.warnings.retain(|w| !w.contains("has no cut settings in the file") && !w.contains("outside the"));
    Ok(i)
}

/// A document's objects as shapes XML (what [`shapes_to_document`] read writes back the same, numbers and path
/// lists in their source spelling). Returns the XML and the export's warnings.
pub fn shapes_from_document(d: &Document, frame: Frame) -> Result<(String, Vec<String>), LbrnError> {
    let (p, warnings) = from_document_in(d, frame)?;
    let shapes = LbrnProject {
        attributes: SHAPES_ROOT.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        shapes: p.shapes,
        ..LbrnProject::default()
    };
    Ok((write_shapes(&shapes), warnings))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ART: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<LightBurnShapes FormatVersion="1" MirrorX="False" MirrorY="False">
    <Shape Type="Ellipse" CutIndex="0" Rx="7.5000014" Ry="7.5">
        <XForm>1 0 0 1 71 127.5</XForm>
    </Shape>
    <Shape Type="Path" CutIndex="3">
        <XForm>1 0 0 1 23.686426 85.05941</XForm>
        <VertList>V-11.547852 -8.5449219c0x1c1x1V-4.0649414 -8.5449219c0x1c1x1V-1.9775391 -13.964844c0x1c1x1</VertList>
        <PrimList>LineClosed</PrimList>
    </Shape>
</LightBurnShapes>
"#;

    fn document(i: Imported) -> Document {
        Document { layers: i.layers, objects: i.objects, groups: i.groups, field_mm: Some(110.0), ..Document::default() }
    }

    #[test]
    fn shapes_xml_round_trips_byte_for_byte() {
        let frame = Frame::Galvo { field_mm: 110.0 };
        let i = shapes_to_document(ART, frame).unwrap();
        assert!(i.warnings.is_empty(), "{:?}", i.warnings);
        assert_eq!(i.objects.len(), 2);
        assert_eq!(i.layers.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["C00", "C03"]);
        let (xml, _) = shapes_from_document(&document(i), frame).unwrap();
        assert_eq!(xml, ART);
        assert!(parse_shapes("<LightBurnProject/>").is_err());
    }
}
