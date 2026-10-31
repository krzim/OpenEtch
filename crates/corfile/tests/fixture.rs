//! Checks against the owner's real EzCad file, `fixtures/jcz150.cor` (ComMarker B6 MOPA, 150 mm lens).

use corfile::{COR_TABLE_LOAD, CorFormat, CorLine, CorTable, NODE_COUNT};

const JCZ150: &[u8] = include_bytes!("../../../fixtures/jcz150.cor");

fn table() -> CorTable {
    CorTable::parse(JCZ150).expect("fixture parses")
}

#[test]
fn file_layout() {
    assert_eq!(JCZ150.len(), 33840);
    assert!(JCZ150.starts_with(b"JCZ_COR_2_1\0"));
    assert_eq!(36 + NODE_COUNT * 8 + 4, JCZ150.len());
}

#[test]
fn header() {
    let t = table();
    assert_eq!(t.format(), CorFormat::Jcz21);
    assert_eq!(t.scale_units_per_mm(), Some(426.2439024390244));
    let field = t.field_mm().expect("scale present");
    // markcfg7.ini FIELDSIZE=1.537500e+002
    assert!((field - 153.75).abs() < 1e-12, "field {field}");
}

#[test]
fn known_nodes() {
    let t = table();
    assert_eq!(t.nodes().len(), NODE_COUNT);
    assert_eq!(t.node(0, 0), Some([-2520.0, -345.0]));
    assert_eq!(t.node(32, 32), Some([0.0, 0.0]));
    assert_eq!(t.node(64, 64), Some([2969.0, -610.0]));
    assert_eq!(t.trailer(), &7i32.to_le_bytes());
}

#[test]
fn round_trips_byte_for_byte() {
    let t = table();
    let bytes = t.write_jcz_2_1();
    assert_eq!(bytes.as_slice(), JCZ150);
    assert_eq!(CorTable::parse(&bytes), Ok(t));
}

#[test]
fn upload_sequence() {
    let lines = table().upload_lines();
    assert_eq!(COR_TABLE_LOAD, 1);
    assert_eq!(lines.len(), NODE_COUNT);
    // (-2520, -345) → sign-magnitude.
    assert_eq!(lines[0], CorLine { dx: 0x8000 + 2520, dy: 0x8000 + 345, first: true });
    assert_eq!(lines[0].flag(), 0);
    assert_eq!(lines[32 * 65 + 32], CorLine { dx: 0, dy: 0, first: false });
    assert_eq!(lines[NODE_COUNT - 1], CorLine { dx: 2969, dy: 0x8000 + 610, first: false });
    assert!(lines[1..].iter().all(|l| l.flag() == 1));
}

#[test]
fn upload_matches_raw_file_words() {
    // Independent re-derivation from the raw bytes: every i32 v → v or (-v + 0x8000) & 0xFFFF.
    let lines = table().upload_lines();
    for (i, line) in lines.iter().enumerate() {
        let at = 36 + i * 8;
        let raw = |o: usize| i32::from_le_bytes(JCZ150[o..o + 4].try_into().expect("4 bytes"));
        let enc = |v: i32| if v < 0 { ((-v + 0x8000) & 0xFFFF) as u16 } else { v as u16 };
        assert_eq!((line.dx, line.dy), (enc(raw(at)), enc(raw(at + 4))), "node {i}");
    }
}

#[test]
fn centre_is_uncorrected_and_corners_move() {
    let t = table();
    assert_eq!(t.correct(0.0, 0.0), (0.0, 0.0));
    let half = t.field_mm().expect("scale") / 2.0;
    let per_unit = 1.0 / 426.2439024390244;
    let (x, y) = t.correct(-half, -half);
    assert!((x - (-half - 2520.0 * per_unit)).abs() < 1e-9);
    assert!((y - (-half - 345.0 * per_unit)).abs() < 1e-9);
}
