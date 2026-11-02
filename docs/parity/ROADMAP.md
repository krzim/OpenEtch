# LightBurn parity roadmap

Status lives in [`parity-data.js`](parity-data.js) (view it with [`index.html`](index.html)). This file is the plan:
how OpenEtch reaches every galvo-relevant item, and the path for the items that need other hardware.

## Galvo parity (target: every galvo-relevant item done)

Work goes in batches. Each batch is built against a written contract, verified end to end on a side engine, then
swapped into the running server. Every finished item gets its tracker line updated in the same commit
(`python -I docs/parity/set_status.py ID STATUS EVIDENCE [NOTE]`).

Audit of 2026-11-02 (after the B6 hardware session, the gantry merge and the GRBL UI): 315 galvo items, **311 done,
1 partial, 0 missing, 3 n/a** (343 items in all: 315 done, 9 partial, 4 missing, 15 n/a). Every batch below is done; what is left of each is listed under
[Remaining galvo items](#remaining-galvo-items). UX-061 (language) counts as done because every UI string is in the
message tables and the language switches live; only English is complete (the machine-translated Spanish covers 568
of 2807 keys), and more languages are translation content, not code.

| Batch | Scope | Main tracker items | State |
|---|---|---|---|
| 1 | Sub-layers and cleanup passes, gantry origins, LightBurn device registry | LAY-067, LAY-032, device items | done |
| 2 | Fusion-style snapping and circle tool; image engraving (all dither modes, DPI, negative, angle, grayscale, adjust image) | DES-049, LAY-023..030, OTH-022, UX-028 | done |
| 3 | Geometry engine and editing: booleans and weld, offsets, Offset Fill, kerf, break apart, auto-join, close path, delete duplicates, convert to path, fillet, trim, node editing, pen/line/polygon/star tools, draw modifiers, arrange and draw order, lock, transforms, arrays, measure, rulers, guides, nesting | DES-005..079, LAY-046, LAY-050 | done |
| 4 | Text: system fonts, single-line/stroke fonts, formatting, welded text, bend text, text on path, variable text, barcodes/QR/DataMatrix | DES-050..058, JOB-001..003, OTH-032..033 | done |
| 5 | Running jobs and the machine: pause/resume, progress, counter, repeat, run continuously, start/stop inputs, cut selected; framing modes, live framing, red dot, blanking, focus light; galvo calibration and lens wizard; device editor, .lbdev, markcfg, add-galvo wizard; Z axis; rotary; preview slider, play, start here; camera | JOB-007..084, OTH-005..030 | done |
| 6 | Cut order and per-layer options | JOB-008..018, LAY-001..066 | done |
| 7 | Layers, libraries and workspace: Cuts/Layers window, defaults, material library, material/interval/focus tests, Laser window, numeric toolbar, docks, hotkeys, units, theme, preferences, i18n, help, logs | LAY-037..071, UX-041..074 | done |
| 8 | Files: exports, print, autosave and backups, notes, thumbnails, several documents, system clipboard, AI/PDF/EPS/HPGL/TIFF imports, import options, legacy .lbrn, templates, art library, .ezd, Gerber, 3D models, trace, masks, 3D sliced, print and cut, pages, CAD "send to laser" | UX-002..049, DES-019..074, LAY-031, JOB-006..007, OTH-002..028 | done; UX-045, UX-068 left; OTH-002 n/a (owner's choice) |

Items that need hardware the B6 lacks (galvo camera, rotary, Z axis, focus light, foot pedal inputs) are built and
tested on the simulator and marked "unverified on hardware" in their notes until someone runs them on a machine.
30 galvo items carry that note today (LAY-023, LAY-030..032, LAY-049, LAY-058, JOB-001..002, JOB-004, JOB-006..007,
JOB-028, JOB-036..037, JOB-041..042, JOB-044, JOB-050, JOB-057..058, JOB-061, JOB-071, JOB-082..084, DES-054,
OTH-006, OTH-012, OTH-027, OTH-029); the B6 session of 2026-11-01 verified JOB-038, JOB-043, JOB-049, JOB-052..053
and JOB-055..056 among others. They count as done; a run on the B6 (and on a rotary, camera, pedal or non-square
field where named) is the only check left for them.

### Next checks on the B6 (no new code needed)

The build on the Pi since 2026-11-02 carries fixes tested on the simulator only. On the next burns: the foot switch
should take the burn press first time while the cycle frames (JOB-053); a slow, dense fill whose packets take the
board over 5 s should run instead of failing with "did not report READY" (JOB-050); and `sudo systemctl restart
openetch` (or `tools/pi/safe-restart.sh`) during a burn should let it finish (JOB-047; the Pi's unit has
`TimeoutStopSec=3700`). The time estimate (motion_time_scale 1.30) should be compared with the wall clock again.

## Remaining galvo items

Every galvo item that is not done, with what blocks it and the next step. The tracker note of each item says the same
in more detail.

### Remaining software work (no external blocker)

None: DES-055 (barcode types) was the last one; ArUco MIP 36h11 is now done (the ArUco library's ARUCO_MIP_36h12).

### Needs a sample file from the owner

2026-11-01: the owner's LightBurn 2.2 samples (`Test files/lightburn-samples`) settled DES-045, UX-045, OTH-022 and DES-055 and most of UX-068; DES-051 was closed with two OFL 1.1 SHX fonts found online. Before that: the 26 sample projects in the sample folders hold only Path, Rect, Ellipse, Text, Group and
Bitmap shapes (no Polygon), every Enhance value in them is 0, and no .lbart, .lbzip or .shx file is on the computer.

| Item | File needed | Then |
|---|---|---|
| UX-068 .lbzip (import and export done with the owner's LightBurn 2.2 bundle; material libraries and image presets mapped from fields inferred from LightBurn.exe's strings) | A bundle exported with a material library loaded (and hotkeys / image presets ticked), and the owner importing OpenEtch's own `.lbzip` in LightBurn | Check the inferred `import-cutlib` / `import-imagepresets` items in `crates/engine/src/lbzip.rs` against it; map `import-hotkeys` once a `hotkeys.json` shows its layout |

### Needs other hardware (n/a for the B6)

| Item | Hardware |
|---|---|
| OTH-004 EzCad3 / BSL boards | An EzCad3 or BSL galvo board to capture and test its protocol |
| OTH-011 CO2 / UV / SPI sources | A CO2, UV or SPI galvo source on an LMC board |

## Nice to have (not parity)

- **CAD "send to laser" (OTH-002, n/a).** The owner does not use CorelDRAW, so no macro is planned. The Fusion 360
  add-in (`tools/fusion360-addin`) and the Inkscape extension (`tools/inkscape-extension`) stay as extras; they are
  tested against a stub engine only and need one run inside each program before anyone relies on them.

## Logged for later (owner, 2026-11-01)

- **Red-light frame slightly trapezoidal on the B6.** With the field trammed (0.6°) and the red pointer scaled
  (X 1.005, Y 1.02 then; Y is 1.01 since), the framed jig rectangle's left side matched but the right side did not; the burn itself is
  square (a 100 mm test square measured 100 x 100). `machine::RedDot::correction` (frame-only angle, skew,
  trapezoid, bulge) exists for this but is neutral and untuned. The owner treats it as a non-issue for now: the
  coin circles frame close enough. Next: tune `red_dot.correction.trapezoid_*` by eye against the jig outline.

## The rest: gantry and controller-specific items

These are `na` today because OpenEtch only drives JCZ/BJJCZ galvos. They become reachable in this order:

1. **A machine-driver trait for gantries.** Split `machine::Controller` into a driver interface with the current LMC
   driver as one implementation. Planning already produces polylines; gantries need motion planning in mm/min and
   a job-origin model (`JOB-063`, `JOB-062`).
   *Status (on main since 2026-11-02):* done. `machine::MachineDriver` + the `machine::Driver` enum (LMC | GRBL); the
   engine drives the enum, the LMC `Controller` is unchanged and its wire bytes are pinned by a test. Feeds are
   mm/min in G-code; the job origin is the bed centre for now (absolute coordinates; `JOB-062`/`JOB-063` still open).
2. **GRBL over serial (USB).** The simplest open protocol: G-code streaming with character-counting flow control.
   Unlocks homing and jogging (`JOB-064`, `UX-075`), console and macros (`JOB-074`), constant-power and other GRBL
   layer options (`LAY-060`), air assist (`LAY-059`), overscanning (`LAY-012`), ramp, lead-in/out and overcut
   (`LAY-053`, `LAY-054`), hide backlash (`JOB-019`), scanning offset table (`OTH-013`), framing with the laser on
   (`JOB-035`), and G-code import/export (`UX-036`, `UX-007`). Test on GRBL's simulator before hardware.
   *Status (on main since 2026-11-02, all unverified on hardware, no gantry):* the GRBL 1.1 driver over serial
   (`serial2`, feature `serial`) with character counting, status polling, feed hold / resume / soft reset, alarms,
   door, homing `$H`, unlock `$X`, jogs `$J=` and a console that refuses anything that could fire; G-code from the
   planner (M4 by default, M3 option, S scaled to `$30`); `.gcode` export (`GET /api/export/gcode`) and import
   (`art.import_gcode`); OpenEtch's own GRBL simulator (`machine::grbl::SimGrbl`) in place of GRBL's (GPL).
   *Also on main (unverified on hardware, no gantry):* the web Move window (jog grid, distance and speed, arrow
   keys, Home, Unlock, Go to origin, Move to position, position readout), the Console window with history and macros
   saved per device (checked against the console policy when saved and when sent), File > Save G-code, and the GRBL
   layer options constant power (M3), air assist (M8/M9), overscan and overcut, all shown only for GRBL devices.
   *Also on branch `grbl-extras` (unverified on hardware, no gantry):* the fill ramp (`LAY-053`), lead-in/out and
   start/end pauses (`LAY-054`), saved Move positions (`UX-075`) and Show Last Position (`JOB-064`).
   Still open: the ramp on Threshold images, Cut Through pauses, a user origin, backlash, scanning offsets and framing
   with the laser on.
3. **Ruida (UDP and USB).** The most common DSP controller. The protocol is publicly documented by community
   projects (MIT/BSD ones only). Unlocks controller file lists (`OTH-017`), Send / machine files (`JOB-065`),
   RDWorks `.rd` import (`UX-037`), rotary mode on gantries (`OTH-014`), auto-focus (`JOB-073`) and feeders
   (`OTH-016`).
4. **Gantry camera features** (head camera, Camera Print and Cut; `OTH-018`) and the Center Finder (`OTH-015`) once
   a gantry can move the head.
5. **Dual-source galvos** (`LAY-018`) when someone with such a machine can test.
6. **Generic generators** that need no hardware and can land any time: tabs and slot resize (`DES-024`), dogbone
   (`OTH-003`), web/user tools menu (`OTH-019`), multi-file trace (`OTH-026`).
   *Status (branch `tools`, 2026-11-02):* all four done (see ARCHITECTURE.md "Tabs and joinery", "User Tools" and
   "Multi-File Trace"); tabs are unverified on hardware (no cut-through run), and LightBurn's tab tags are still not
   imported from `.lbrn2` (no sample with hand-placed tabs).
