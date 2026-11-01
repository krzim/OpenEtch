# LightBurn parity roadmap

Status lives in [`parity-data.js`](parity-data.js) (view it with [`index.html`](index.html)). This file is the plan:
how OpenEtch reaches every galvo-relevant item, and the path for the items that need other hardware.

## Galvo parity (target: every galvo-relevant item done)

Work goes in batches. Each batch is built against a written contract, verified end to end on a side engine, then
swapped into the running server. Every finished item gets its tracker line updated in the same commit
(`python -I docs/parity/set_status.py ID STATUS EVIDENCE [NOTE]`).

Audit of 2026-11-01 (after the final pass): 315 galvo items, **306 done, 7 partial, 0 missing, 2 n/a** (343 items
in all: 306 done, 7 partial, 6 missing, 24 n/a). Every batch below is done; what is left of each is listed under
[Remaining galvo items](#remaining-galvo-items). UX-061 (language) counts as done because every UI string is in the
message tables and the language switches live; only English is complete (the machine-translated Spanish covers 568
of 2807 keys), and more languages are translation content, not code.

| Batch | Scope | Main tracker items | State |
|---|---|---|---|
| 1 | Sub-layers and cleanup passes, gantry origins, LightBurn device registry | LAY-067, LAY-032, device items | done |
| 2 | Fusion-style snapping and circle tool; image engraving (all dither modes, DPI, negative, angle, grayscale, adjust image) | DES-049, LAY-023..030, OTH-022, UX-028 | done; OTH-022 left (sample files) |
| 3 | Geometry engine and editing: booleans and weld, offsets, Offset Fill, kerf, break apart, auto-join, close path, delete duplicates, convert to path, fillet, trim, node editing, pen/line/polygon/star tools, draw modifiers, arrange and draw order, lock, transforms, arrays, measure, rulers, guides, nesting | DES-005..079, LAY-046, LAY-050 | done; DES-045 left |
| 4 | Text: system fonts, single-line/stroke fonts, formatting, welded text, bend text, text on path, variable text, barcodes/QR/DataMatrix | DES-050..058, JOB-001..003, OTH-032..033 | done; DES-051, DES-055 left |
| 5 | Running jobs and the machine: pause/resume, progress, counter, repeat, run continuously, start/stop inputs, cut selected; framing modes, live framing, red dot, blanking, focus light; galvo calibration and lens wizard; device editor, .lbdev, markcfg, add-galvo wizard; Z axis; rotary; preview slider, play, start here; camera | JOB-007..084, OTH-005..030 | done |
| 6 | Cut order and per-layer options | JOB-008..018, LAY-001..066 | done |
| 7 | Layers, libraries and workspace: Cuts/Layers window, defaults, material library, material/interval/focus tests, Laser window, numeric toolbar, docks, hotkeys, units, theme, preferences, i18n, help, logs | LAY-037..071, UX-041..074 | done |
| 8 | Files: exports, print, autosave and backups, notes, thumbnails, several documents, system clipboard, AI/PDF/EPS/HPGL/TIFF imports, import options, legacy .lbrn, templates, art library, .ezd, Gerber, 3D models, trace, masks, 3D sliced, print and cut, pages, CAD "send to laser" | UX-002..049, DES-019..074, LAY-031, JOB-006..007, OTH-002..028 | done; UX-045, UX-068, OTH-002 left |

Items that need hardware the B6 lacks (galvo camera, rotary, Z axis, focus light, foot pedal inputs) are built and
tested on the simulator and marked "unverified on hardware" in their notes until someone runs them on a machine.
39 galvo items carry that note today (LAY-023, LAY-030..032, LAY-049, LAY-058, JOB-001..002, JOB-004, JOB-006..007,
JOB-028, JOB-036..038, JOB-041..044, JOB-049..050, JOB-052..053, JOB-055..059, JOB-061, JOB-071, JOB-082..084,
DES-054, OTH-006, OTH-009, OTH-012, OTH-027, OTH-029). They count as done; a run on the B6 (and on a rotary, camera,
pedal or non-square field where named) is the only check left for them.

## Remaining galvo items

Every galvo item that is not done, with what blocks it and the next step. The tracker note of each item says the same
in more detail.

### Remaining software work (no external blocker)

| Item | What is missing | Next step |
|---|---|---|
| DES-055 Barcode types | Engine only (the Create Bar Code dialog lists whatever kinds `shape.barcode` offers; 25 types today, 22 of LightBurn's 28): PDF417 Micro, Chinese Sensible Code, Grid Matrix, AprilTag 36h10, ArUco MIP 36h11, ArUco OpenCV 7x7 (none started, audit 2026-11-01) | Encoders in `crates/text/src/codes/` from permissive references (Zint BSD-3 for MicroPDF417, Han Xin, Grid Matrix; the AprilTag BSD-2 36h10 table; OpenCV Apache-2 DICT_7X7_1000). ArUco MIP 36h11 alone needs a sample from the owner: LightBurn's docs do not name the dictionary (OpenCV has DICT_ARUCO_MIP_36h12), so a LightBurn file or SVG export holding markers 0, 1 and 2 of that type settles it |

### Needs a sample file from the owner

Rechecked 2026-11-01: the 26 sample projects in the sample folders hold only Path, Rect, Ellipse, Text, Group and
Bitmap shapes (no Polygon), every Enhance value in them is 0, and no .lbart, .lbzip or .shx file is on the computer.

| Item | File needed | Then |
|---|---|---|
| DES-045 Polygon in .lbrn2 | A .lbrn2 saved by LightBurn holding polygons drawn with its Polygon tool (a 6-sided one and a star, sizes noted) | Map the element in `crates/lbrn` to `Geometry::Polygon` both ways |
| UX-045 .lbart | Two or three .lbart libraries saved by LightBurn with known content (a 20 mm square; then plus a circle and a text) | Decode the binary layout; reader in `crates/fileio` used by the Art Library's Load |
| UX-068 .lbzip | One bundle from File > Bundles > Export Bundle with every component ticked | Unpack it and route its parts to the .lbprefs, device, .clb, preset and art imports |
| OTH-022 Enhance scale and presets | A .lbrn2 with non-zero Enhance Radius / Amount / Denoise plus the same image exported as LightBurn's processed bitmap; an Adjust Image preset exported from LightBurn | Fit the file scale and Enhance formula; read the preset file |
| DES-051 Real SHX font | A real .shx the owner may use (not a LightBurn file; the owner's LightBurn has no SHX folder set) | Point `OPENETCH_SHX_SAMPLES` at it and run `text::shx::real_shx_fonts_parse` |

### Needs an external program

| Item | Program | Then |
|---|---|---|
| OTH-002 CAD "send to laser" | Autodesk Fusion and Inkscape to run the add-in and extension against a live engine; CorelDRAW to write and test a macro | Fix what the first real runs show; add the CorelDRAW macro if CorelDRAW is available |

### Needs other hardware (n/a for the B6)

| Item | Hardware |
|---|---|
| OTH-004 EzCad3 / BSL boards | An EzCad3 or BSL galvo board to capture and test its protocol |
| OTH-011 CO2 / UV / SPI sources | A CO2, UV or SPI galvo source on an LMC board |

## The rest: gantry and controller-specific items

These are `na` today because OpenEtch only drives JCZ/BJJCZ galvos. They become reachable in this order:

1. **A machine-driver trait for gantries.** Split `machine::Controller` into a driver interface with the current LMC
   driver as one implementation. Planning already produces polylines; gantries need motion planning in mm/min and
   a job-origin model (`JOB-063`, `JOB-062`).
2. **GRBL over serial (USB).** The simplest open protocol: G-code streaming with character-counting flow control.
   Unlocks homing and jogging (`JOB-064`, `UX-075`), console and macros (`JOB-074`), constant-power and other GRBL
   layer options (`LAY-060`), air assist (`LAY-059`), overscanning (`LAY-012`), ramp, lead-in/out and overcut
   (`LAY-053`, `LAY-054`), hide backlash (`JOB-019`), scanning offset table (`OTH-013`), framing with the laser on
   (`JOB-035`), and G-code import/export (`UX-036`, `UX-007`). Test on GRBL's simulator before hardware.
3. **Ruida (UDP and USB).** The most common DSP controller. The protocol is publicly documented by community
   projects (MIT/BSD ones only). Unlocks controller file lists (`OTH-017`), Send / machine files (`JOB-065`),
   RDWorks `.rd` import (`UX-037`), rotary mode on gantries (`OTH-014`), auto-focus (`JOB-073`) and feeders
   (`OTH-016`).
4. **Gantry camera features** (head camera, Camera Print and Cut; `OTH-018`) and the Center Finder (`OTH-015`) once
   a gantry can move the head.
5. **Dual-source galvos** (`LAY-018`) when someone with such a machine can test.
6. **Generic generators** that need no hardware and can land any time: tabs and slot resize (`DES-024`), dogbone
   (`OTH-003`), web/user tools menu (`OTH-019`), multi-file trace (`OTH-026`).
