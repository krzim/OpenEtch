# Attribution

Third-party code or data that enters this repository. Rust crate dependencies are pinned by `Cargo.lock` and are not
listed here.

| Source | License | What | Where |
|---|---|---|---|
| [luizbueno3d/openlaser-rs](https://github.com/luizbueno3d/openlaser-rs) @ `9d81a15` | MIT OR Apache-2.0 | LMC opcode table, packet layout, unit conversions (itself ported from galvoplotter) | `crates/lmc` (per-file headers) |
| [meerk40t/galvoplotter](https://github.com/meerk40t/galvoplotter) @ `fc35136` | MIT © 2023 MeerK40t | Original LMC protocol facts and formulas | `crates/lmc` (per-file headers) |
| [meerk40t/meerk40t](https://github.com/meerk40t/meerk40t) `balormk/` | MIT © 2021 meerk40t | `.cor` file layout and upload order (cross-checked); the list delay sign rule (`>= 0` is positive) and the polygon delay encoding (`list_laser_on_delay`, `list_polygon_delay` in `balormk/controller.py`) | `crates/corfile`, `crates/lmc` (`units.rs` header) |

## Fixtures

- `fixtures/jcz150.cor`, `fixtures/markcfg7.ini`: the owner's own machine calibration and configuration files from the
  ComMarker B6 MOPA EzCad install. Test fixtures only.

## Never copied

GPL, SSPL or unlicensed sources: Balor, balor-meerk40t, BurnCraft, LaserCraft, lmcv2u-loader. No BJJCZ firmware. No
LightBurn code, assets or text.
