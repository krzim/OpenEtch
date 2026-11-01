# Attribution

Third-party code or data that enters this repository. Rust crate dependencies are pinned by `Cargo.lock` and are not
listed here.

| Source | License | What | Where |
|---|---|---|---|
| [luizbueno3d/openlaser-rs](https://github.com/luizbueno3d/openlaser-rs) @ `9d81a15` | MIT OR Apache-2.0 | LMC opcode table, packet layout, unit conversions (itself ported from galvoplotter) | `crates/lmc` (per-file headers) |
| [meerk40t/galvoplotter](https://github.com/meerk40t/galvoplotter) @ `fc35136` | MIT © 2023 MeerK40t | Original LMC protocol facts and formulas | `crates/lmc` (per-file headers) |
| [luizbueno3d/openlaser-rs](https://github.com/luizbueno3d/openlaser-rs) @ `9d81a15` | MIT OR Apache-2.0 | LightBurn `.lbrn2` facts and code: the compact `VertList`/`PrimList` scanner, the `c0x1`/`c1x1` "no control point" rule, live-Text-XForm-over-BackupPath, and the LightBurn layer palette values (which it transcribed from LightBurn's public colour-palette documentation; values only) | `crates/lbrn` (`path.rs`, `palette.rs` headers) |
| [tscircuit/lbrnts](https://github.com/tscircuit/lbrnts) | MIT | Format notes only, no code: tool layers T1/T2 (`CutIndex="30"`, `type="Tool"`, never output) | `crates/lbrn` (crate docs) |
| [jlucaso1/lbrn2-to-svg](https://github.com/jlucaso1/lbrn2-to-svg) | MIT | Format notes only, no code: `XForm` coefficient order, Rect/Ellipse/Bitmap centred on the local origin (cross-checked against real files) | `crates/lbrn` (crate docs) |
| [ixmilia/dxf-rs](https://github.com/ixmilia/dxf-rs) (`dxf` crate 0.6) | MIT © Brett V. Forsgren | Used as a dependency (not copied) to read DXF files for `art.import_dxf`; native builds only (it needs a random source on wasm32) | `crates/commands` (`builtin/dxf.rs`) |
| [iShape-Rust/iOverlay](https://github.com/iShape-Rust/iOverlay) (`i_overlay` crate 9.1) | MIT OR Apache-2.0 | Used as a dependency (not copied) for polygon booleans, offsets and line clipping (`path.boolean`, `path.weld`, `path.cut_shapes`, `path.offset`) | `crates/geom` (`region.rs`, `offset.rs`) |
| [meerk40t/meerk40t](https://github.com/meerk40t/meerk40t) `balormk/` | MIT © 2021 meerk40t | `.cor` file layout and upload order (cross-checked); the list delay sign rule (`>= 0` is positive) and the polygon delay encoding (`list_laser_on_delay`, `list_polygon_delay` in `balormk/controller.py`) | `crates/corfile`, `crates/lmc` (`units.rs` header) |
| The Hershey fonts (Dr. A. V. Hershey, U.S. National Bureau of Standards; data format by James Hurt, Cognition, Inc.), `.jhf` files as distributed in [kamalmostafa/hershey-fonts](https://github.com/kamalmostafa/hershey-fonts) (font data only; none of that project's GPL code) | Hershey font licence: any use, with the acknowledgements kept and not in the U.S. NTIS format | 12 single-line fonts bundled as OpenEtch's stroke fonts | `crates/text/fonts/hershey/` (licence and acknowledgements in its `README.md`) |
| [harfbuzz/harfrust](https://github.com/harfbuzz/harfrust) (`harfrust` 0.12), [googlefonts/fontations](https://github.com/googlefonts/fontations) (`skrifa` 0.44, `read-fonts`) | MIT; MIT OR Apache-2.0 | Used as dependencies (not copied): text shaping and glyph outlines | `crates/text` (`outline.rs`) |
| [RazrFalcon/fontdb](https://github.com/RazrFalcon/fontdb) (`fontdb` 0.24) | MIT | Used as a dependency (not copied): finding installed fonts (feature `system`) | `crates/text` (`registry.rs`) |
| [rxing-core/rxing](https://github.com/rxing-core/rxing) (`rxing` 0.9, a ZXing port) | Apache-2.0 | Used as a dependency (not copied), encoders only: barcodes | `crates/text` (`barcode.rs`) |
| EzCad `.jsf` fonts | - | Format only: the layout was worked out from the owner's EzCad2 fonts for interoperability (no EzCad code); the fonts are read in place by an env-gated test (`OPENETCH_JSF_DIR`) and never copied | `crates/text` (`jsf.rs`) |
| Autodesk AutoCAD Customization Guide, "Shape Descriptions" | Documentation | Format facts only: the shape byte codes of `.shx` fonts | `crates/text` (`shx.rs`) |
| LightBurn documentation (docs.lightburnsoftware.com: Text, Variable Text Formatting, Bar Codes, Shape Properties, Apply Path to Text) | Documentation | Behaviour only (no text copied): text options, variable text codes, barcode types | `crates/text`, `crates/commands` (`builtin/text.rs`) |
| [meerk40t/galvoplotter](https://github.com/meerk40t/galvoplotter) @ `fc35136`, [meerk40t/meerk40t](https://github.com/meerk40t/meerk40t) `balormk/controller.py` | MIT | Protocol facts only (no code): pause = `StopList` (0x0020), resume = `RestartList` (0x0013); `MoveAxisTo(position, invert)`, `AxisGoOrigin`, `GetAxisPos`, the `AXIS` status bit (0x40) | `crates/lmc` (`single.rs`), `crates/machine` (`controller.rs`) |
| LightBurn documentation (docs.lightburnsoftware.com: Optimization Settings, Live Framing / galvo framing, Galvo and Basic Settings, galvo Rotary) | Documentation | Behaviour only (no text copied): cut order options, framing styles and the Laser window's job controls, device settings names, galvo rotary workflow. The lens correction formulas are OpenEtch's own (LightBurn publishes none) | `crates/plan`, `crates/engine` (`frame.rs`, `job.rs`), `crates/lmc` (`calibration.rs`) |

## Fixtures

- `fixtures/jcz150.cor`, `fixtures/markcfg7.ini`: the owner's own machine calibration and configuration files from the
  ComMarker B6 MOPA EzCad install. Test fixtures only.
- `crates/lbrn/tests/fixtures/*.lbrn2`: hand-written synthetic LightBurn projects (no LightBurn output, no artwork).
  The owner's real LightBurn projects and material library are read only by env-gated tests
  (`OPENETCH_LBRN_SAMPLES`) and are never copied into the repository.

## Never copied

GPL, SSPL or unlicensed sources: Balor, balor-meerk40t, BurnCraft, LaserCraft, lmcv2u-loader. No BJJCZ firmware. No
LightBurn code, assets or text.
