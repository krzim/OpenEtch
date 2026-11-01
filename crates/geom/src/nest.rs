//! Bounding-box nesting (like LightBurn's Quick Nest): a greedy shelf packer. Parts are rectangles (their bounding
//! boxes, optionally in several rotations); they never interlock, so irregular parts pack less tightly than a true
//! nest would.

use kurbo::{Point, Rect};

/// One part to place: its bounding-box size in each allowed orientation.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    /// `(width, height)` per orientation, in the caller's orientation order.
    pub sizes: Vec<(f64, f64)>,
}

/// Where a part went: the orientation index and the lower-left corner of its box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub orientation: usize,
    pub min: Point,
}

struct Shelf {
    y: f64,
    height: f64,
    x: f64,
}

/// Pack `parts` into `region` with at least `spacing` mm between boxes, filling shelves from the bottom left (lower
/// Y first, +Y up). Tallest parts go first; each takes its lowest orientation that fits, preferring an existing
/// shelf. Parts that fit nowhere get `None`.
pub fn shelf_pack(parts: &[Part], region: Rect, spacing: f64) -> Vec<Option<Placement>> {
    let spacing = if spacing.is_finite() { spacing.max(0.0) } else { 0.0 };
    let mut order: Vec<usize> = (0..parts.len()).collect();
    let tallest = |p: &Part| p.sizes.iter().map(|s| s.1.min(s.0)).fold(0.0, f64::max);
    order.sort_by(|&a, &b| tallest(&parts[b]).total_cmp(&tallest(&parts[a])).then(a.cmp(&b)));
    let mut shelves: Vec<Shelf> = Vec::new();
    let mut out = vec![None; parts.len()];
    let eps = 1e-9;
    for i in order {
        let mut orients: Vec<usize> = (0..parts[i].sizes.len()).collect();
        orients.sort_by(|&a, &b| parts[i].sizes[a].1.total_cmp(&parts[i].sizes[b].1).then(a.cmp(&b)));
        let mut placed = None;
        'search: for &o in &orients {
            let (w, h) = parts[i].sizes[o];
            if !(w.is_finite() && h.is_finite()) || w > region.width() + eps || h > region.height() + eps {
                continue;
            }
            for s in &mut shelves {
                if h <= s.height + eps && s.x + w <= region.x1 + eps {
                    placed = Some(Placement { orientation: o, min: Point::new(s.x, s.y) });
                    s.x += w + spacing;
                    break 'search;
                }
            }
        }
        if placed.is_none() {
            for &o in &orients {
                let (w, h) = parts[i].sizes[o];
                if !(w.is_finite() && h.is_finite()) || w > region.width() + eps {
                    continue;
                }
                let y = shelves.last().map_or(region.y0, |s| s.y + s.height + spacing);
                if y + h <= region.y1 + eps {
                    shelves.push(Shelf { y, height: h, x: region.x0 + w + spacing });
                    placed = Some(Placement { orientation: o, min: Point::new(region.x0, y) });
                    break;
                }
            }
        }
        out[i] = placed;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_without_overlap_and_reports_misfits() {
        let parts: Vec<Part> = (0..20).map(|i| Part { sizes: vec![(10.0 + i as f64 % 3.0, 5.0), (5.0, 10.0 + i as f64 % 3.0)] }).collect();
        let region = Rect::new(0.0, 0.0, 50.0, 50.0);
        let out = shelf_pack(&parts, region, 1.0);
        let boxes: Vec<Rect> = out
            .iter()
            .zip(&parts)
            .filter_map(|(p, part)| p.map(|p| {
                let (w, h) = part.sizes[p.orientation];
                Rect::new(p.min.x, p.min.y, p.min.x + w, p.min.y + h)
            }))
            .collect();
        assert!(boxes.len() >= 15, "{}", boxes.len());
        for (i, a) in boxes.iter().enumerate() {
            assert!(a.x0 >= -1e-9 && a.y0 >= -1e-9 && a.x1 <= 50.0 + 1e-9 && a.y1 <= 50.0 + 1e-9);
            for b in &boxes[i + 1..] {
                let o = a.intersect(*b);
                assert!(o.width() <= 1e-9 || o.height() <= 1e-9, "{a:?} overlaps {b:?}");
            }
        }
        let big = shelf_pack(&[Part { sizes: vec![(100.0, 1.0)] }], region, 0.0);
        assert_eq!(big, vec![None]);
        assert!(shelf_pack(&[], region, 0.0).is_empty());
    }
}
