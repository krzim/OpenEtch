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
| [meerk40t/meerk40t](https://github.com/meerk40t/meerk40t) `balormk/` | MIT © 2021 meerk40t | `.cor` file layout and upload order (cross-checked); the list delay sign rule (`>= 0` is positive) and the polygon delay encoding (`list_laser_on_delay`, `list_polygon_delay` in `balormk/controller.py`) | `crates/corfile`, `crates/lmc` (`units.rs` header) |

## Fixtures

- `fixtures/jcz150.cor`, `fixtures/markcfg7.ini`: the owner's own machine calibration and configuration files from the
  ComMarker B6 MOPA EzCad install. Test fixtures only.
- `crates/lbrn/tests/fixtures/*.lbrn2`: hand-written synthetic LightBurn projects (no LightBurn output, no artwork).
  The owner's real LightBurn projects and material library are read only by env-gated tests
  (`OPENETCH_LBRN_SAMPLES`) and are never copied into the repository.

## Never copied

GPL, SSPL or unlicensed sources: Balor, balor-meerk40t, BurnCraft, LaserCraft, lmcv2u-loader. No BJJCZ firmware. No
LightBurn code, assets or text.
