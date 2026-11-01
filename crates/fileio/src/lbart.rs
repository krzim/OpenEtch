//! LightBurn Art Library files (`.lbart`): the binary container. The art itself is LightBurn's clipboard XML
//! (`<LightBurnShapes>`), which `lbrn::parse_shapes` reads.
//!
//! # The format
//!
//! Decoded from two libraries saved by LightBurn Pro 2.2.00 with known contents (three items: a 20 x 10 mm rectangle,
//! a 15 mm circle and the text "AB" converted to paths; and a library holding only the circle). It is not a
//! `QDataStream` of Qt containers but a plain big-endian layout:
//!
//! ```text
//! u32 index_offset        byte offset of the index at the end of the file
//! u32 count               number of items
//! ...                     per item, in index order: its thumbnail PNG, then its art (qCompress: u32 BE uncompressed
//!                         length + a zlib stream of the <LightBurnShapes> XML, UTF-8)
//! index: count x 136 bytes, at index_offset (which is where the data ends):
//!     104 bytes           the item name, UTF-16BE, padded with zero bytes (so at most 51 characters plus a 0)
//!     u32 png_offset, u32 png_length
//!     u32 art_offset,  u32 art_length       (the qCompress block, length prefix included)
//!     f64 width_mm, f64 height_mm           the art's size (LightBurn stores the float32 value as a double)
//! ```
//!
//! LightBurn lists the items by name (the sample's index and data are in alphabetical order: Circle15, Rect20x10,
//! TextAB); thumbnails are 256 x 256 PNGs. The art XML is `<LightBurnShapes FormatVersion="1" MirrorX="False"
//! MirrorY="False">` holding `<Shape>`s exactly as in a project, at the positions they had when added (no cut
//! settings: a shape's `CutIndex` names its layer colour).
//!
//! [`read`] checks every offset and length against the file and caps the inflated size; [`write`] lays an
//! [`Lbart`] out the same way. Writing what [`read`] returned gives the same bytes ([`Item::art_block`] keeps each
//! compressed block as read; a new or changed item is compressed here).

use crate::{FileError, Result, invalid};

/// Bytes of one index record.
pub const INDEX_RECORD: usize = 136;
/// Bytes of the name field (UTF-16BE, zero padded).
pub const NAME_BYTES: usize = 104;
/// Most UTF-16 code units of a name (one is left for the terminating zero).
pub const MAX_NAME_UNITS: usize = NAME_BYTES / 2 - 1;
/// Most items read.
pub const MAX_ITEMS: usize = 10_000;
/// Largest inflated art of one item.
pub const MAX_ART_BYTES: usize = 64 * 1024 * 1024;
/// The first bytes of a PNG file.
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// A LightBurn Art Library.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Lbart {
    pub items: Vec<Item>,
}

/// One graphic of a library.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub name: String,
    /// The thumbnail PNG (LightBurn: 256 x 256).
    pub thumbnail_png: Vec<u8>,
    /// The art: `<LightBurnShapes>` XML.
    pub shapes_xml: String,
    /// The art's width and height in mm, as stored.
    pub size_mm: [f64; 2],
    /// The qCompress block as read (written again while it still inflates to [`Item::shapes_xml`]).
    pub art_block: Option<Vec<u8>>,
}

impl Item {
    /// A new item (its art is compressed when written).
    pub fn new(name: impl Into<String>, thumbnail_png: Vec<u8>, shapes_xml: String, size_mm: [f64; 2]) -> Item {
        Item { name: name.into(), thumbnail_png, shapes_xml, size_mm, art_block: None }
    }
}

/// Whether `bytes` look like an `.lbart` file (a plausible header followed by a PNG).
pub fn is_lbart(bytes: &[u8]) -> bool {
    if bytes.len() < 8 + PNG_SIGNATURE.len() {
        return false;
    }
    let index = be32(bytes, 0) as usize;
    let count = be32(bytes, 4) as usize;
    // `checked_mul`: a hostile count must not overflow on 32-bit targets (wasm).
    count > 0
        && bytes[8..].starts_with(PNG_SIGNATURE)
        && count.checked_mul(INDEX_RECORD).and_then(|n| index.checked_add(n)) == Some(bytes.len())
}

fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn be64(b: &[u8], at: usize) -> f64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[at..at + 8]);
    f64::from_be_bytes(a)
}

/// The part `offset .. offset + len` of `bytes`, checked to lie within the data area (`end`).
fn span<'a>(bytes: &'a [u8], offset: u32, len: u32, end: usize, what: &str) -> Result<&'a [u8]> {
    let (o, l) = (offset as usize, len as usize);
    match o.checked_add(l) {
        Some(e) if o >= 8 && e <= end => Ok(&bytes[o..e]),
        _ => invalid(format!("the .lbart file is damaged: {what} lies outside the file")),
    }
}

/// Inflate a qCompress block (u32 BE length + zlib stream), at most [`MAX_ART_BYTES`].
pub fn q_uncompress(block: &[u8]) -> Result<Vec<u8>> {
    if block.len() < 4 {
        return invalid("the .lbart file is damaged: an item's art is truncated");
    }
    let want = be32(block, 0) as usize;
    if want > MAX_ART_BYTES {
        return Err(FileError::TooLarge(format!(
            "an .lbart item's art would inflate to {want} bytes (at most {MAX_ART_BYTES})"
        )));
    }
    let out = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&block[4..], want)
        .map_err(|_| FileError::Invalid("the .lbart file is damaged: an item's art does not inflate".into()))?;
    if out.len() != want {
        return invalid("the .lbart file is damaged: an item's art has the wrong length");
    }
    Ok(out)
}

/// Qt's `qCompress`: u32 BE length + a zlib stream.
pub fn q_compress(data: &[u8]) -> Vec<u8> {
    let mut out = (data.len() as u32).to_be_bytes().to_vec();
    out.extend(miniz_oxide::deflate::compress_to_vec_zlib(data, 6));
    out
}

/// Read an `.lbart` file.
pub fn read(bytes: &[u8]) -> Result<Lbart> {
    if bytes.len() < 8 {
        return invalid("not a LightBurn art library (.lbart): the file is too short");
    }
    let index = be32(bytes, 0) as usize;
    let count = be32(bytes, 4) as usize;
    if count > MAX_ITEMS {
        return Err(FileError::TooLarge(format!("the .lbart file has {count} items (at most {MAX_ITEMS})")));
    }
    if index < 8 || index.checked_add(count * INDEX_RECORD) != Some(bytes.len()) {
        return invalid("not a LightBurn art library (.lbart): its index does not fit the file");
    }
    let mut items = Vec::with_capacity(count);
    let mut total = 0usize;
    for k in 0..count {
        let r = &bytes[index + k * INDEX_RECORD..index + (k + 1) * INDEX_RECORD];
        let units: Vec<u16> = r[..NAME_BYTES].chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        let len = units.iter().position(|&u| u == 0).unwrap_or(units.len());
        let name = String::from_utf16(&units[..len])
            .map_err(|_| FileError::Invalid(format!("the .lbart file is damaged: item {} has a broken name", k + 1)))?;
        let png = span(bytes, be32(r, NAME_BYTES), be32(r, NAME_BYTES + 4), index, "a thumbnail")?;
        if !png.starts_with(PNG_SIGNATURE) {
            return invalid(format!("the .lbart file is damaged: the thumbnail of \"{name}\" is not a PNG"));
        }
        let block = span(bytes, be32(r, NAME_BYTES + 8), be32(r, NAME_BYTES + 12), index, "an item's art")?;
        let xml = q_uncompress(block)?;
        total = total.saturating_add(xml.len());
        if total > MAX_ART_BYTES {
            return Err(FileError::TooLarge(format!("the .lbart file's art inflates to more than {MAX_ART_BYTES} bytes")));
        }
        let shapes_xml = String::from_utf8(xml)
            .map_err(|_| FileError::Invalid(format!("the art of \"{name}\" is not UTF-8 text")))?;
        let size_mm = [be64(r, NAME_BYTES + 16), be64(r, NAME_BYTES + 24)];
        items.push(Item { name, thumbnail_png: png.to_vec(), shapes_xml, size_mm, art_block: Some(block.to_vec()) });
    }
    Ok(Lbart { items })
}

/// Write an `.lbart` file. Names longer than [`MAX_NAME_UNITS`] UTF-16 units are cut (at a character boundary).
pub fn write(lib: &Lbart) -> Result<Vec<u8>> {
    if lib.items.len() > MAX_ITEMS {
        return Err(FileError::TooLarge(format!("an .lbart file holds at most {MAX_ITEMS} items")));
    }
    let mut out = vec![0u8; 8];
    let mut records = Vec::with_capacity(lib.items.len() * INDEX_RECORD);
    for it in &lib.items {
        if !it.thumbnail_png.starts_with(PNG_SIGNATURE) {
            return invalid(format!("the thumbnail of \"{}\" is not a PNG", it.name));
        }
        let kept = it.art_block.as_ref().filter(|b| q_uncompress(b).is_ok_and(|x| x == it.shapes_xml.as_bytes()));
        let block = kept.cloned().unwrap_or_else(|| q_compress(it.shapes_xml.as_bytes()));
        let pos = |at: usize, len: usize| -> Result<(u32, u32)> {
            let o = u32::try_from(at).map_err(|_| FileError::TooLarge("the .lbart file would exceed 4 GB".into()))?;
            let l = u32::try_from(len).map_err(|_| FileError::TooLarge("an .lbart item is larger than 4 GB".into()))?;
            Ok((o, l))
        };
        let (png_o, png_l) = pos(out.len(), it.thumbnail_png.len())?;
        out.extend_from_slice(&it.thumbnail_png);
        let (art_o, art_l) = pos(out.len(), block.len())?;
        out.extend_from_slice(&block);
        let mut name = [0u8; NAME_BYTES];
        let mut units = Vec::new();
        for c in it.name.chars() {
            let mut buf = [0u16; 2];
            let enc = c.encode_utf16(&mut buf);
            if units.len() + enc.len() > MAX_NAME_UNITS {
                break;
            }
            units.extend_from_slice(enc);
        }
        for (i, u) in units.iter().enumerate() {
            name[2 * i..2 * i + 2].copy_from_slice(&u.to_be_bytes());
        }
        records.extend_from_slice(&name);
        for v in [png_o, png_l, art_o, art_l] {
            records.extend_from_slice(&v.to_be_bytes());
        }
        records.extend_from_slice(&it.size_mm[0].to_be_bytes());
        records.extend_from_slice(&it.size_mm[1].to_be_bytes());
    }
    let index = u32::try_from(out.len()).map_err(|_| FileError::TooLarge("the .lbart file would exceed 4 GB".into()))?;
    out[0..4].copy_from_slice(&index.to_be_bytes());
    out[4..8].copy_from_slice(&(lib.items.len() as u32).to_be_bytes());
    out.extend(records);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake";
    const XML: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<LightBurnShapes FormatVersion=\"1\" MirrorX=\"False\" \
        MirrorY=\"False\">\n    <Shape Type=\"Rect\" CutIndex=\"0\" W=\"20\" H=\"10\" Cr=\"0\">\n        \
        <XForm>1 0 0 1 34 130</XForm>\n    </Shape>\n</LightBurnShapes>\n";

    #[test]
    fn writes_and_reads_the_layout_lightburn_uses() {
        let lib = Lbart {
            items: vec![
                Item::new("Rect20x10", PNG.to_vec(), XML.into(), [20.0, 10.0]),
                Item::new("Ünïcode 😀", PNG.to_vec(), XML.replace("20", "30"), [32.056_648_254_394_53, 17.895_507_812_5]),
            ],
        };
        let bytes = write(&lib).unwrap();
        assert!(is_lbart(&bytes));
        // Header, the first item's data right after it, the index at the end.
        assert_eq!(&bytes[4..8], &[0, 0, 0, 2]);
        assert!(bytes[8..].starts_with(PNG));
        let index = be32(&bytes, 0) as usize;
        assert_eq!(bytes.len(), index + 2 * INDEX_RECORD);
        assert_eq!(&bytes[index..index + 4], &[0, b'R', 0, b'e']);
        assert_eq!(be32(&bytes, index + NAME_BYTES), 8);
        assert_eq!(be32(&bytes, index + NAME_BYTES + 4), PNG.len() as u32);
        let art = be32(&bytes, index + NAME_BYTES + 8) as usize;
        assert_eq!(be32(&bytes, art), XML.len() as u32);
        assert_eq!(&bytes[art + 4..art + 6], &[0x78, 0x9c]);
        let back = read(&bytes).unwrap();
        assert_eq!(back.items.len(), 2);
        assert_eq!(back.items[1].name, "Ünïcode 😀");
        assert_eq!(back.items[1].size_mm, lib.items[1].size_mm);
        assert_eq!(back.items[0].shapes_xml, XML);
        // What was read writes back byte for byte.
        assert_eq!(write(&back).unwrap(), bytes);
    }

    #[test]
    fn long_names_are_cut_and_damaged_files_refused() {
        let long = "x".repeat(80);
        let bytes = write(&Lbart { items: vec![Item::new(long, PNG.to_vec(), XML.into(), [1.0, 1.0])] }).unwrap();
        assert_eq!(read(&bytes).unwrap().items[0].name.len(), MAX_NAME_UNITS);
        for cut in [0, 7, 20, bytes.len() - 1] {
            assert!(read(&bytes[..cut]).is_err());
        }
        // An offset pointing into the index, or a length running past it.
        let index = be32(&bytes, 0) as usize;
        let mut bad = bytes.clone();
        bad[index + NAME_BYTES + 4..index + NAME_BYTES + 8].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(read(&bad).is_err());
        // A claimed inflated size beyond the cap is refused before inflating.
        let mut bomb = bytes.clone();
        let art = be32(&bytes, index + NAME_BYTES + 8) as usize;
        bomb[art..art + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(matches!(read(&bomb), Err(FileError::TooLarge(_))));
        assert!(!is_lbart(b"not an lbart file at all"));
        // A header claiming u32::MAX items (or an index past the end) is no .lbart and does not overflow.
        for header in [[0xff; 8], [0, 0, 0, 8, 0xff, 0xff, 0xff, 0xff], [0xff, 0xff, 0xff, 0xff, 0, 0, 0, 1]] {
            let mut b = header.to_vec();
            b.extend_from_slice(PNG);
            b.extend(vec![0u8; 300]);
            assert!(!is_lbart(&b) && read(&b).is_err(), "{header:?}");
        }
    }

    /// The owner's LightBurn libraries (`OPENETCH_LBRN_SAMPLES`, see the samples' README): their known contents, and
    /// a byte-for-byte rewrite.
    #[test]
    fn real_lbart_files() {
        let Some(dirs) = std::env::var_os("OPENETCH_LBRN_SAMPLES") else {
            println!("OPENETCH_LBRN_SAMPLES is not set; skipping the real-file test");
            return;
        };
        let mut seen = 0;
        for dir in std::env::split_paths(&dirs) {
            for name in ["art-sample-1.lbart", "art-sample-2.lbart"] {
                let Ok(bytes) = std::fs::read(dir.join(name)) else { continue };
                seen += 1;
                assert!(is_lbart(&bytes), "{name}");
                let lib = read(&bytes).unwrap();
                let names: Vec<&str> = lib.items.iter().map(|i| i.name.as_str()).collect();
                if name == "art-sample-1.lbart" {
                    assert_eq!(names, ["Circle15", "Rect20x10", "TextAB"]);
                    assert_eq!(lib.items[1].size_mm, [20.0, 10.0]);
                    assert!((lib.items[2].size_mm[0] - 32.057).abs() < 1e-3 && (lib.items[2].size_mm[1] - 17.896).abs() < 1e-3);
                    assert_eq!(lib.items[2].shapes_xml.matches("<Shape Type=\"Path\"").count(), 5);
                } else {
                    assert_eq!(names, ["OnlyCircle"]);
                }
                assert_eq!(lib.items[0].size_mm, [15.0, 15.0]);
                for it in &lib.items {
                    assert!(it.shapes_xml.contains("<LightBurnShapes"), "{}", it.name);
                    let img = image::load_from_memory(&it.thumbnail_png).unwrap();
                    assert_eq!((img.width(), img.height()), (256, 256));
                }
                assert_eq!(write(&lib).unwrap(), bytes, "{name} does not write back byte for byte");
                // Freshly compressed art reads back the same.
                let fresh = Lbart { items: lib.items.iter().map(|i| Item { art_block: None, ..i.clone() }).collect() };
                assert_eq!(read(&write(&fresh).unwrap()).unwrap().items.iter().map(|i| &i.shapes_xml).collect::<Vec<_>>(),
                    lib.items.iter().map(|i| &i.shapes_xml).collect::<Vec<_>>());
            }
        }
        println!("{seen} .lbart sample(s) checked");
    }
}
