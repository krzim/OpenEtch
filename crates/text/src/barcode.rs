//! Barcodes as outlines (LightBurn's Create Bar Code), encoded with rxing (a ZXing port, Apache-2.0). The modules
//! (2D) or bars (1D) become one welded outline per island, so a fill layer burns every dark module once and a line
//! layer traces their edges. Of LightBurn's 28 types, those ZXing can encode are offered: QR Code, Data Matrix,
//! Aztec, PDF417, Code 128, Code 39, Code 93, CodaBar, EAN-8, EAN-13, UPC-A, UPC-E, ITF (2 of 5 interleaved) and
//! Telepen. Micro QR, AprilTag/ArUco, Grid Matrix, Han Xin, Plessey and the other 2 of 5 variants are not.

use kurbo::{BezPath, Rect, Shape};
use rxing::{BarcodeFormat, EncodeHints, MultiFormatWriter, Writer};

/// A barcode type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Qr,
    DataMatrix,
    Aztec,
    Pdf417,
    Code128,
    Code39,
    Code93,
    Codabar,
    Ean8,
    Ean13,
    UpcA,
    UpcE,
    Itf,
    Telepen,
}

impl Kind {
    pub const ALL: [Kind; 14] = [
        Kind::Qr,
        Kind::DataMatrix,
        Kind::Aztec,
        Kind::Pdf417,
        Kind::Code128,
        Kind::Code39,
        Kind::Code93,
        Kind::Codabar,
        Kind::Ean8,
        Kind::Ean13,
        Kind::UpcA,
        Kind::UpcE,
        Kind::Itf,
        Kind::Telepen,
    ];

    /// The name used in commands (`qr`, `data_matrix`, `code128`, ...).
    pub fn name(self) -> &'static str {
        match self {
            Kind::Qr => "qr",
            Kind::DataMatrix => "data_matrix",
            Kind::Aztec => "aztec",
            Kind::Pdf417 => "pdf417",
            Kind::Code128 => "code128",
            Kind::Code39 => "code39",
            Kind::Code93 => "code93",
            Kind::Codabar => "codabar",
            Kind::Ean8 => "ean8",
            Kind::Ean13 => "ean13",
            Kind::UpcA => "upc_a",
            Kind::UpcE => "upc_e",
            Kind::Itf => "itf",
            Kind::Telepen => "telepen",
        }
    }

    pub fn from_name(s: &str) -> Option<Kind> {
        let n: String = s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
        Kind::ALL.into_iter().find(|k| k.name().replace('_', "") == n).or(match n.as_str() {
            "qrcode" => Some(Kind::Qr),
            "datamatrix" | "dm" => Some(Kind::DataMatrix),
            "ean" => Some(Kind::Ean13),
            "upc" => Some(Kind::UpcA),
            "2of5" | "interleaved2of5" => Some(Kind::Itf),
            _ => None,
        })
    }

    /// A 2D symbol (square modules) rather than a row of bars.
    pub fn is_2d(self) -> bool {
        matches!(self, Kind::Qr | Kind::DataMatrix | Kind::Aztec | Kind::Pdf417)
    }

    fn format(self) -> BarcodeFormat {
        match self {
            Kind::Qr => BarcodeFormat::QR_CODE,
            Kind::DataMatrix => BarcodeFormat::DATA_MATRIX,
            Kind::Aztec => BarcodeFormat::AZTEC,
            Kind::Pdf417 => BarcodeFormat::PDF_417,
            Kind::Code128 => BarcodeFormat::CODE_128,
            Kind::Code39 => BarcodeFormat::CODE_39,
            Kind::Code93 => BarcodeFormat::CODE_93,
            Kind::Codabar => BarcodeFormat::CODABAR,
            Kind::Ean8 => BarcodeFormat::EAN_8,
            Kind::Ean13 => BarcodeFormat::EAN_13,
            Kind::UpcA => BarcodeFormat::UPC_A,
            Kind::UpcE => BarcodeFormat::UPC_E,
            Kind::Itf => BarcodeFormat::ITF,
            Kind::Telepen => BarcodeFormat::TELEPEN,
        }
    }
}

/// How to draw a barcode.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    /// Size of one module (2D) or the narrowest bar (1D), mm.
    pub module_mm: f64,
    /// Bar height for 1D codes, mm (2D codes are as tall as their modules make them).
    pub height_mm: f64,
    /// Light margin round the code, in modules (only drawn when `invert`).
    pub quiet_zone: u32,
    /// Mark the light parts instead of the dark ones (for materials that turn light when marked): the quiet zone
    /// and the light modules become the outline.
    pub invert: bool,
    /// Error correction: QR `L`, `M`, `Q`, `H`; Aztec the % of error words; PDF417 0-8. `None` is the encoder's
    /// default.
    pub error_correction: Option<String>,
    /// Data Matrix: keep the symbol square (LightBurn's Force Square, on by default).
    pub force_square: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { module_mm: 0.5, height_mm: 15.0, quiet_zone: 2, invert: false, error_correction: None, force_square: true }
    }
}

/// A drawn barcode: its outline centred on the origin, size, and the module grid's dimensions.
#[derive(Debug, Clone)]
pub struct Drawn {
    pub path: BezPath,
    pub width_mm: f64,
    pub height_mm: f64,
    pub modules: (u32, u32),
}

/// The module grid of `data`: `grid[y][x]` dark, row 0 at the top.
pub fn encode(kind: Kind, data: &str, o: &Options) -> Result<Vec<Vec<bool>>, String> {
    if data.is_empty() {
        return Err("the barcode data is empty".into());
    }
    let mut hints = EncodeHints { Margin: Some("0".into()), ..EncodeHints::default() };
    if let Some(ec) = &o.error_correction {
        hints.ErrorCorrection = Some(ec.clone());
    }
    if kind == Kind::DataMatrix {
        hints.DataMatrixShape = Some(if o.force_square {
            rxing::datamatrix::encoder::SymbolShapeHint::FORCE_SQUARE
        } else {
            rxing::datamatrix::encoder::SymbolShapeHint::FORCE_NONE
        });
    }
    if !data.is_ascii() && matches!(kind, Kind::Qr | Kind::Aztec | Kind::Pdf417 | Kind::DataMatrix) {
        hints.CharacterSet = Some("UTF-8".into());
    }
    let m = std::panic::catch_unwind(|| MultiFormatWriter.encode_with_hints(data, &kind.format(), 0, 0, &hints))
        .map_err(|_| format!("the {} encoder failed on this data", kind.name()))?
        .map_err(|e| format!("{} cannot encode {data:?}: {e}", kind.name()))?;
    let (w, h) = (m.width(), m.height());
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > 4_000_000 {
        return Err(format!("the {} symbol has an unusable size {w} x {h}", kind.name()));
    }
    let rows = if kind.is_2d() { h } else { 1 };
    Ok((0..rows).map(|y| (0..w).map(|x| m.get(x, y)).collect()).collect())
}

/// Draw `data` as a barcode outline centred on the origin.
pub fn draw(kind: Kind, data: &str, o: &Options) -> Result<Drawn, String> {
    if !(o.module_mm.is_finite() && o.module_mm > 0.0 && o.module_mm <= 100.0) {
        return Err("module_mm must be within 0..=100 and above 0".into());
    }
    if !(o.height_mm.is_finite() && o.height_mm > 0.0 && o.height_mm <= 2000.0) {
        return Err("height_mm must be within 0..=2000 and above 0".into());
    }
    let grid = encode(kind, data, o)?;
    let cols = grid[0].len();
    let m = o.module_mm;
    let (cell_h, rows) = if kind.is_2d() { (m, grid.len()) } else { (o.height_mm, 1) };
    let q = if o.invert { f64::from(o.quiet_zone.min(100)) * m } else { 0.0 };
    let width = cols as f64 * m + 2.0 * q;
    let height = rows as f64 * cell_h + 2.0 * q;
    let (x0, y_top) = (-width / 2.0 + q, height / 2.0 - q);
    // Runs of equal modules per row become one rectangle each.
    let mut rects = Vec::new();
    for (y, row) in grid.iter().enumerate() {
        let mut x = 0;
        while x < cols {
            let dark = row[x] != o.invert;
            let start = x;
            while x < cols && (row[x] != o.invert) == dark {
                x += 1;
            }
            if dark {
                let top = y_top - y as f64 * cell_h;
                rects.push(Rect::new(x0 + start as f64 * m, top - cell_h, x0 + x as f64 * m, top));
            }
        }
    }
    if o.invert && q > 0.0 {
        let (l, r, t, b) = (-width / 2.0, width / 2.0, height / 2.0, -height / 2.0);
        rects.extend([Rect::new(l, t - q, r, t), Rect::new(l, b, r, b + q), Rect::new(l, b + q, l + q, t - q), Rect::new(r - q, b + q, r, t - q)]);
    }
    let paths: Vec<BezPath> = rects.iter().map(|r| r.to_path(0.01)).collect();
    let region = geom::Region::from_paths(&paths, geom::Fill::NonZero).map_err(|e| e.to_string())?;
    Ok(Drawn {
        path: region.to_path(),
        width_mm: width,
        height_mm: height,
        modules: (cols as u32, grid.len() as u32),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_code_is_square_with_finder_patterns() {
        let g = encode(Kind::Qr, "OpenEtch", &Options::default()).unwrap();
        assert_eq!(g.len(), g[0].len());
        assert_eq!(g.len(), 21, "version 1");
        assert!(g[0][..7].iter().all(|d| *d), "finder pattern top row");
        let d = draw(Kind::Qr, "OpenEtch", &Options { module_mm: 1.0, ..Options::default() }).unwrap();
        assert!((d.width_mm - 21.0).abs() < 1e-9 && (d.height_mm - 21.0).abs() < 1e-9);
        let b = geom::bounds(&d.path).unwrap();
        assert!((b.x0 + 10.5).abs() < 1e-6 && (b.y1 - 10.5).abs() < 1e-6, "centred: {b:?}");
    }

    #[test]
    fn one_d_codes_are_bars() {
        let d = draw(Kind::Code128, "ABC-123", &Options { module_mm: 0.25, height_mm: 10.0, ..Options::default() }).unwrap();
        assert_eq!(d.modules.1, 1);
        assert!((d.height_mm - 10.0).abs() < 1e-9);
        assert!(draw(Kind::Ean13, "12345", &Options::default()).is_err(), "EAN-13 needs 12 or 13 digits");
        assert!(draw(Kind::Ean13, "590123412345", &Options::default()).is_ok());
        for k in Kind::ALL {
            let data = match k {
                Kind::Ean8 => "9638507",
                Kind::Ean13 => "590123412345",
                Kind::UpcA => "03600029145",
                Kind::UpcE => "0123456",
                Kind::Itf => "12345678",
                Kind::Codabar => "A123B",
                _ => "OPENETCH1",
            };
            assert!(draw(k, data, &Options::default()).is_ok(), "{k:?}");
            assert_eq!(Kind::from_name(k.name()), Some(k));
        }
    }

    #[test]
    fn invert_marks_the_light_modules_and_the_quiet_zone() {
        let o = Options { module_mm: 1.0, invert: true, quiet_zone: 2, ..Options::default() };
        let d = draw(Kind::Qr, "x", &o).unwrap();
        assert!((d.width_mm - 25.0).abs() < 1e-9);
        let b = geom::bounds(&d.path).unwrap();
        assert!((b.width() - 25.0).abs() < 1e-6);
        assert!(draw(Kind::Qr, "", &Options::default()).is_err());
        assert!(draw(Kind::Qr, "x", &Options { module_mm: f64::NAN, ..Options::default() }).is_err());
    }
}
