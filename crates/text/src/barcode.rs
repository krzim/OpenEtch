//! Barcodes as outlines (LightBurn's Create Bar Code). The modules (2D) or bars (1D) become one welded outline per
//! island, so a fill layer burns every dark module once and a line layer traces their edges.
//!
//! rxing (a ZXing port, Apache-2.0) encodes QR Code, Data Matrix, Aztec, PDF417 (also compact), Code 128, Code 39,
//! Code 93, CodaBar, EAN-8, EAN-13, UPC-A, UPC-E, ITF (2 of 5 interleaved) and Telepen; ISBN goes through EAN-13 and
//! VIN through Code 39 after their checks. OpenEtch's own encoders (`crate::codes`) add Micro QR, rMQR, PDF417 Micro,
//! Chinese Sensible Code (Han Xin), Grid Matrix, 2 of 5 Industrial, 2 of 5 IATA, Plessey, AprilTag 16h5 / 25h9 /
//! 36h10 / 36h11, ArUco 7 x 7 (OpenCV's DICT_7X7_1000; ids 0-49, 0-99 and 0-249 are DICT_7X7_50, _100 and _250)
//! and ArUco MIP 36h11 (the ArUco library's ARUCO_MIP_36h12, ids 0-249).
//!
//! LightBurn's files store the type as a number (`<Shape Type="BarCode" ... Content="0" BarCodeType="261">` is
//! ArUco MIP 36h11 in LightBurn 2.2); no sample shows another type's number, so `.lbrn2` barcodes are not mapped yet
//! (they are kept for LightBurn export as unknown shapes).

use kurbo::{BezPath, Rect, Shape};

use crate::codes;
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
    Pdf417Compact,
    MicroQr,
    Rmqr,
    Industrial2of5,
    Iata2of5,
    Isbn,
    Vin,
    Plessey,
    AprilTag16h5,
    AprilTag25h9,
    AprilTag36h11,
    MicroPdf417,
    HanXin,
    GridMatrix,
    AprilTag36h10,
    Aruco7x7,
    ArucoMip36h12,
}

impl Kind {
    pub const ALL: [Kind; 31] = [
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
        Kind::Pdf417Compact,
        Kind::MicroQr,
        Kind::Rmqr,
        Kind::Industrial2of5,
        Kind::Iata2of5,
        Kind::Isbn,
        Kind::Vin,
        Kind::Plessey,
        Kind::AprilTag16h5,
        Kind::AprilTag25h9,
        Kind::AprilTag36h11,
        Kind::MicroPdf417,
        Kind::HanXin,
        Kind::GridMatrix,
        Kind::AprilTag36h10,
        Kind::Aruco7x7,
        Kind::ArucoMip36h12,
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
            Kind::Pdf417Compact => "pdf417_compact",
            Kind::MicroQr => "micro_qr",
            Kind::Rmqr => "rmqr",
            Kind::Industrial2of5 => "industrial_2of5",
            Kind::Iata2of5 => "iata_2of5",
            Kind::Isbn => "isbn",
            Kind::Vin => "vin",
            Kind::Plessey => "plessey",
            Kind::AprilTag16h5 => "apriltag_16h5",
            Kind::AprilTag25h9 => "apriltag_25h9",
            Kind::AprilTag36h11 => "apriltag_36h11",
            Kind::MicroPdf417 => "micro_pdf417",
            Kind::HanXin => "han_xin",
            Kind::GridMatrix => "grid_matrix",
            Kind::AprilTag36h10 => "apriltag_36h10",
            Kind::Aruco7x7 => "aruco_7x7",
            Kind::ArucoMip36h12 => "aruco_mip_36h12",
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
            "microqr" | "qrmicro" | "mqr" => Some(Kind::MicroQr),
            "qrmicrorectangular" | "rectangularmicroqr" => Some(Kind::Rmqr),
            "2of5industrial" | "standard2of5" => Some(Kind::Industrial2of5),
            "2of5iata" => Some(Kind::Iata2of5),
            "pdf417truncated" | "truncatedpdf417" => Some(Kind::Pdf417Compact),
            "ukplessey" => Some(Kind::Plessey),
            "pdf417micro" | "micropdf" => Some(Kind::MicroPdf417),
            "chinesesensiblecode" | "chinesesensible" | "csc" => Some(Kind::HanXin),
            "arucoopencv7x7" | "dict7x71000" | "dict7x750" | "dict7x7100" | "dict7x7250" => Some(Kind::Aruco7x7),
            "arucomip36h11" | "arucomip" | "dictarucomip36h12" => Some(Kind::ArucoMip36h12),
            _ => None,
        })
    }

    /// A 2D symbol (square modules) rather than a row of bars.
    pub fn is_2d(self) -> bool {
        matches!(
            self,
            Kind::Qr
                | Kind::DataMatrix
                | Kind::Aztec
                | Kind::Pdf417
                | Kind::Pdf417Compact
                | Kind::MicroQr
                | Kind::Rmqr
                | Kind::AprilTag16h5
                | Kind::AprilTag25h9
                | Kind::AprilTag36h11
                | Kind::MicroPdf417
                | Kind::HanXin
                | Kind::GridMatrix
                | Kind::AprilTag36h10
                | Kind::Aruco7x7
                | Kind::ArucoMip36h12
        )
    }

    /// A fiducial tag (the data is the tag id).
    pub fn is_tag(self) -> bool {
        matches!(
            self,
            Kind::AprilTag16h5
                | Kind::AprilTag25h9
                | Kind::AprilTag36h10
                | Kind::AprilTag36h11
                | Kind::Aruco7x7
                | Kind::ArucoMip36h12
        )
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
            Kind::Pdf417Compact => BarcodeFormat::PDF_417,
            Kind::Isbn => BarcodeFormat::EAN_13,
            Kind::Vin => BarcodeFormat::CODE_39,
            // Encoded by `crate::codes`, never through rxing.
            _ => BarcodeFormat::UNSUPORTED_FORMAT,
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
    /// Error correction: QR `L`, `M`, `Q`, `H`; Aztec the % of error words; PDF417 0-8; Chinese Sensible Code 1-4;
    /// Grid Matrix 1-5. `None` is the encoder's default.
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
    let ec = o.error_correction.as_deref();
    let one_row = |m: Vec<bool>| vec![m];
    match kind {
        Kind::MicroQr => return codes::qr_micro::micro_qr(data, ec),
        Kind::Rmqr => return codes::qr_micro::rmqr(data, ec),
        Kind::Industrial2of5 => return codes::linear::two_of_five_industrial(data).map(one_row),
        Kind::Iata2of5 => return codes::linear::two_of_five_iata(data).map(one_row),
        Kind::Plessey => return codes::linear::plessey(data).map(one_row),
        Kind::AprilTag16h5 => return codes::tags::TAG16H5.grid(data),
        Kind::AprilTag25h9 => return codes::tags::TAG25H9.grid(data),
        Kind::AprilTag36h11 => return codes::tags::TAG36H11.grid(data),
        Kind::AprilTag36h10 => return codes::tags::TAG36H10.grid(data),
        Kind::Aruco7x7 => return codes::tags::ARUCO_7X7.grid(data),
        Kind::ArucoMip36h12 => return codes::tags::ARUCO_MIP_36H12.grid(data),
        Kind::MicroPdf417 => return codes::micro_pdf417::micro_pdf417(data),
        Kind::HanXin => return codes::hanxin::han_xin(data, ec),
        Kind::GridMatrix => return codes::grid_matrix::grid_matrix(data, ec),
        _ => {}
    }
    let checked = match kind {
        Kind::Isbn => Some(codes::linear::isbn_to_ean13(data)?),
        Kind::Vin => Some(codes::linear::vin(data)?),
        _ => None,
    };
    let data = checked.as_deref().unwrap_or(data);
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
    if kind == Kind::Pdf417Compact {
        hints.Pdf417Compact = Some("true".into());
    }
    if !data.is_ascii() && matches!(kind, Kind::Qr | Kind::Aztec | Kind::Pdf417 | Kind::Pdf417Compact | Kind::DataMatrix) {
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
                Kind::Industrial2of5 | Kind::Iata2of5 => "123456",
                Kind::Isbn => "978-0-306-40615-7",
                Kind::Vin => "1M8GDM9AXKP042788",
                Kind::Plessey => "12AB",
                k if k.is_tag() => "3",
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

    #[test]
    fn new_types_encode() {
        let o = Options::default();
        // Truncated PDF417: no right row indicator, a one-module stop bar.
        let full = encode(Kind::Pdf417, "OpenEtch compact PDF417", &o).unwrap();
        let compact = encode(Kind::Pdf417Compact, "OpenEtch compact PDF417", &o).unwrap();
        assert!(compact[0].len() < full[0].len());
        assert!(compact.iter().all(|r| *r.last().unwrap()) && compact.iter().all(|r| !r[r.len() - 2]));
        // ISBN is EAN-13 of 978 + the ISBN-10 digits; VIN is Code 39 with the check.
        assert_eq!(encode(Kind::Isbn, "0-306-40615-2", &o).unwrap(), encode(Kind::Ean13, "9780306406157", &o).unwrap());
        assert!(encode(Kind::Isbn, "0-306-40615-3", &o).unwrap_err().contains("check digit"));
        assert_eq!(encode(Kind::Vin, "1M8GDM9AXKP042788", &o).unwrap(), encode(Kind::Code39, "1M8GDM9AXKP042788", &o).unwrap());
        assert!(encode(Kind::Vin, "1M8GDM9A1KP042788", &o).is_err());
        // Micro QR, rMQR and tags are 2D grids of their own sizes.
        assert_eq!(encode(Kind::MicroQr, "123", &o).unwrap().len(), 11);
        let r = encode(Kind::Rmqr, "123", &o).unwrap();
        assert_eq!((r.len(), r[0].len()), (11, 27), "the smallest area: R11x27");
        let d = draw(Kind::AprilTag36h11, "0", &Options { module_mm: 1.0, ..o.clone() }).unwrap();
        assert_eq!(d.modules, (8, 8));
        assert!((d.width_mm - 8.0).abs() < 1e-9);
        assert!(encode(Kind::AprilTag16h5, "30", &o).is_err());
        assert_eq!(Kind::from_name("QR Micro"), Some(Kind::MicroQr));
        assert_eq!(Kind::from_name("2 of 5 IATA"), Some(Kind::Iata2of5));
        assert_eq!(Kind::from_name("AprilTag 36h11"), Some(Kind::AprilTag36h11));
    }

    /// Every type in LightBurn's Bar Code list (its docs, 2026) by LightBurn's name.
    #[test]
    fn lightburn_types_resolve_and_draw() {
        let names = [
            "2 of 5", "2 of 5 IATA", "2 of 5 Industrial", "AprilTag 16h5", "AprilTag 25h9", "AprilTag 36h10",
            "AprilTag 36h11", "ArUco MIP 36h11", "ArUco OpenCV 7x7", "Aztec", "Chinese Sensible Code", "CodaBar", "Code 128", "Code 93",
            "Data Matrix", "EAN", "Grid Matrix", "ISBN", "PDF417", "PDF417 Compact", "PDF417 Micro", "Plessey",
            "QR Code", "QR Micro", "QR Micro Rectangular", "UPC A", "UPC E", "VIN",
        ];
        let o = Options::default();
        for n in names {
            let k = Kind::from_name(n).unwrap_or_else(|| panic!("{n}"));
            let data = match k {
                Kind::Itf => "12345678",
                Kind::Ean13 => "590123412345",
                Kind::UpcA => "03600029145",
                Kind::UpcE => "0123456",
                Kind::Codabar => "A123B",
                Kind::Industrial2of5 | Kind::Iata2of5 => "123456",
                Kind::Isbn => "978-0-306-40615-7",
                Kind::Vin => "1M8GDM9AXKP042788",
                Kind::Plessey => "12AB",
                k if k.is_tag() => "7",
                _ => "OpenEtch 12",
            };
            assert!(draw(k, data, &o).is_ok(), "{n}");
        }
        assert_eq!(Kind::from_name("ArUco MIP 36h11"), Some(Kind::ArucoMip36h12));
        assert_eq!(Kind::from_name("aruco_mip_36h12"), Some(Kind::ArucoMip36h12));
    }

    #[test]
    fn micro_pdf417_han_xin_and_grid_matrix() {
        let o = Options::default();
        // PDF417 Micro: 1 to 4 columns of 17 modules between 10-module row address patterns, rows two modules tall.
        let g = encode(Kind::MicroPdf417, "12345678", &o).unwrap();
        assert_eq!((g.len(), g[0].len()), (22, 38), "1 column x 11 rows");
        assert!(g.chunks(2).all(|p| p[0] == p[1]));
        let g = encode(Kind::MicroPdf417, &"OpenEtch ".repeat(12), &o).unwrap();
        assert!([55, 82, 99].contains(&g[0].len()));
        assert!(encode(Kind::MicroPdf417, &"x".repeat(400), &o).is_err());
        // Chinese Sensible Code: 23 x 23 up (version 1), finder patterns in the corners; Chinese characters go in GB
        // 18030 modes; the level is 1-4.
        let g = encode(Kind::HanXin, "OpenEtch", &o).unwrap();
        assert_eq!(g.len(), 23);
        assert_eq!(g[0][..7], [true; 7]);
        assert_eq!(encode(Kind::HanXin, "汉信码标准", &o).unwrap().len(), 23);
        let digits = "0123456789".repeat(40);
        assert_eq!(encode(Kind::HanXin, &digits, &o).unwrap().len(), 2 * 13 + 21, "170 codewords: version 13");
        let level = |l: &str| Options { error_correction: Some(l.into()), ..o.clone() };
        assert!(encode(Kind::HanXin, "x", &level("5")).is_err());
        assert!(encode(Kind::HanXin, &digits, &level("4")).unwrap().len() > 2 * 13 + 21, "level 4 needs more room");
        // Grid Matrix: 6 + 12 x version modules; GB 2312 text, else an ECI.
        assert_eq!(encode(Kind::GridMatrix, "1234", &o).unwrap().len(), 18);
        assert_eq!(encode(Kind::GridMatrix, "电池充电器", &o).unwrap().len(), 30, "12 codewords: version 2");
        let (gb, eci) = codes::grid_matrix::units("电池充电器 Ω");
        assert_eq!((gb.iter().filter(|u| **u > 0xFF).count(), eci), (6, 0));
        assert_eq!(codes::grid_matrix::units("Größe ÿ").1, 3, "Latin-1 after ECI 3");
        assert_eq!(codes::grid_matrix::units("Ωmega €").1, 26, "the euro sign is not in GB 2312: UTF-8");
        assert!(encode(Kind::GridMatrix, "Ωmega €", &o).is_ok());
        assert!(encode(Kind::GridMatrix, "x", &level("6")).is_err());
        assert!(encode(Kind::GridMatrix, &"Z".repeat(3000), &o).is_err());
        // The new tags: AprilTag 36h10 and ArUco 7 x 7 (bordered 8 x 8 and 9 x 9).
        assert_eq!(encode(Kind::AprilTag36h10, "2319", &o).unwrap().len(), 8);
        assert_eq!(encode(Kind::Aruco7x7, "999", &o).unwrap().len(), 9);
        assert!(encode(Kind::Aruco7x7, "1000", &o).is_err());
        assert_eq!(Kind::from_name("DICT_7X7_250"), Some(Kind::Aruco7x7));
        assert_eq!(Kind::from_name("han_xin"), Some(Kind::HanXin));
    }
}
