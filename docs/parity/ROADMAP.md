# LightBurn parity roadmap

Status lives in [`parity-data.js`](parity-data.js) (view it with [`index.html`](index.html)). This file is the plan:
how OpenEtch reaches every galvo-relevant item, and the path for the items that need other hardware.

## Galvo parity (target: every galvo-relevant item done)

Work goes in batches. Each batch is built against a written contract, verified end to end on a side engine, then
swapped into the running server. Every finished item gets its tracker line updated in the same commit
(`python -I docs/parity/set_status.py ID STATUS EVIDENCE [NOTE]`).

| Batch | Scope | Main tracker items |
|---|---|---|
| 1 (done) | Sub-layers and cleanup passes, gantry origins, LightBurn device registry | LAY-067, LAY-032 (partial), device items |
| 2 | Fusion-style snapping and circle tool; image engraving (all dither modes, DPI, negative, angle, grayscale, adjust image) | DES-049, LAY-023..030, OTH-022, UX-028 |
| 3 | Geometry engine and editing: booleans and weld, offsets, Offset Fill, kerf, break apart, auto-join, close path, delete duplicates, convert to path, fillet, trim, node editing, pen/line/polygon/star tools, draw modifiers, arrange and draw order, lock, transforms (scale %, two-point, mirror across line, shear, same size), arrays (grid, circular, along path), measure, rulers, guides, nesting | DES-005..079, LAY-046, LAY-050 |
| 4 (engine done, UI pending) | Text: system fonts, single-line/stroke fonts, formatting, welded text, bend text, text on path, variable text (serials, date, CSV merge, cut settings), barcodes/QR/DataMatrix | DES-050..058, JOB-001..003, OTH-032..033 |
| 5 | Running jobs and the machine: pause/resume, progress and time remaining, counter, repeat, run continuously, stop at count, start/stop marking inputs, cut selected graphics; all framing modes, live framing, red-dot offset, blanking, focus light; galvo calibration (field offset/angle, scale/bulge/skew/trapezoid, lens calibration wizard, COR load/clear, open-MO delay, Q-pulse and laser type); device settings editor, .lbdev export, markcfg import, add-galvo wizard, controller info; Z axis and red-dot jog; rotary (chuck/roller, split marking, cylinder correction, taper warp); preview time slider, play, start here | JOB-007..084, OTH-005..030 |
| 6 | Cut order and per-layer options: optimization toggles, order by group/priority, inner first, overlap removal, best direction, global passes, timing and jump overrides, pause before/after, dot mode, wobble, dot width adjust, fill grouping, LPI, auto-rotate, flood fill, perforation, tabs, Z offset/step, per-shape power scale, sort cuts last, sub-layer output toggle | JOB-008..018, LAY-001..066 |
| 7 | Layers, libraries and workspace: Cuts/Layers window (columns, reorder, context menu, bulk toggles, palette, Multi rows), copy/paste and default layer settings, material library window (.clb: browse, assign, link, edit, save), material/interval/focus tests, Laser window, numeric edit toolbar, status bar, dockable windows, hotkeys (LightBurn defaults, cheat sheet, editor), context menus, units, theme, grid/snap/nudge preferences, UI scale, crosshair, i18n framework, help/about/updates, debug log, job log | LAY-037..071, UX-041..074 |
| 8 | Files: SVG/DXF/AI export, export selection, processed-bitmap export, print, autosave and backups, recent projects, notes, thumbnails, several open documents, system clipboard, AI/PDF/EPS/HPGL/TIFF/TGA import, import options, legacy .lbrn, templates, art library, .ezd import, Gerber, 3D model to depth map, trace image (incl. centre line), convert to bitmap, image masks and replace, 3D sliced mode, galvo camera, print and cut, pages, Fusion 360 "send to laser" | UX-002..049, DES-019..074, LAY-031, JOB-006..007, OTH-002..028 |

Items that need hardware the B6 lacks (galvo camera, rotary, Z axis, focus light, foot pedal inputs) are built and
tested on the simulator and marked "unverified on hardware" in their notes until someone runs them on a machine.

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
