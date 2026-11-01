//! LightBurn device import: the synthetic `fixtures/lightburn-prefs.json` (written for these tests; it mimics the
//! owner's B6 device and adds a second, deliberately odd one) and, when `OPENETCH_LIGHTBURN_PREFS` is set, real
//! `prefs.ini` files (a `;`-separated path list; never copied into the repository). Run with `--nocapture` to see the
//! diff tables, e.g.
//!
//! ```text
//! OPENETCH_LIGHTBURN_PREFS="C:\Users\me\AppData\Local\LightBurn\prefs.ini" cargo test -p machine --test lightburn -- --nocapture
//! ```
//!
//! Only device-list values are printed; the tests also check that nothing else of the file survives parsing.

use machine::lightburn::diff_table;
use machine::device::{self, Bases, DeviceKind, DeviceSource};
use machine::{MachineProfile, ProfileDiff, find_device, parse_devices, profile_from_lightburn};

const PREFS: &str = include_str!("fixtures/lightburn-prefs.json");
const B6: &str = include_str!("../../../fixtures/b6-mopa-60w.toml");
const MARKCFG: &[u8] = include_bytes!("../../../fixtures/markcfg7.ini");

fn b6() -> MachineProfile {
    MachineProfile::from_toml(B6).unwrap()
}

fn diff<'a>(diffs: &'a [ProfileDiff], field: &str) -> Option<&'a ProfileDiff> {
    diffs.iter().find(|d| d.field == field)
}

#[test]
fn synthetic_prefs_parse() {
    let prefs = parse_devices(PREFS).unwrap();
    assert_eq!(prefs.devices.len(), 2);
    let d = prefs.default_device().unwrap();
    assert_eq!((d.name.as_str(), d.display_name.as_str(), d.kind.as_str()), ("JCZFiber", "JCZFiber (Unknown)", "LibUSB"));
    assert_eq!((d.width_mm, d.height_mm), (Some(153.8), Some(153.8)));
    assert_eq!(d.correction_file.as_deref(), Some("jcz150.cor"), "the folders are dropped");
    let s = &d.settings;
    assert_eq!((s.laser_on_tc_us, s.laser_off_tc_us, s.polygon_tc_us, s.end_tc_us), (Some(-300.0), Some(100.0), Some(100.0), Some(300.0)));
    assert_eq!((s.jump_speed_mm_s, s.min_jump_delay_us, s.max_jump_delay_us, s.max_jump_distance_mm), (Some(4000.0), Some(500.0), Some(500.0), Some(0.0)));
    assert_eq!((s.galvo_1_is_x, s.galvo_1_sign, s.galvo_2_sign), (Some(false), Some(1.0), Some(1.0)));
    assert_eq!((s.min_freq_khz, s.max_freq_khz, s.max_speed_mm_s, s.frame_speed_mm_s), (Some(1.0), Some(4000.0), Some(7000.0), Some(3000.0)));
    assert_eq!((s.q_pulse_width_enabled, s.pwm_enabled, s.open_mo_delay_us), (Some(true), Some(true), Some(8000.0)));
    assert_eq!((s.start_mark_port_bit, s.start_mark_bit_high, s.start_mark_pulse_mode), (Some(14), Some(true), Some(true)));
    assert_eq!((s.red_light_port_bit, s.red_light_jump_delay_us), (Some(-1), Some(100.0)));
    assert_eq!((s.laser_type, s.laser_fiber_type), (Some(1), Some(1)));
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);

    // Lookup by name or display name, ignoring case.
    for q in ["JCZFiber", "jczfiber", " JCZFIBER (unknown) ", "Bench 110 LENS", "bench110"] {
        assert!(find_device(&prefs, q).is_some(), "{q}");
    }
    assert!(find_device(&prefs, "JCZ").is_none());
    assert!(find_device(&prefs, "").is_none());

    // Nothing but the typed device list survives.
    let stored = serde_json::to_string(&prefs).unwrap();
    for leaked in ["secret", "license", "guid", "someone", "Documents", "DoorProtect", "Sim_", "AppTheme"] {
        assert!(!stored.to_lowercase().contains(&leaked.to_lowercase()), "{leaked} in {stored}");
    }
}

#[test]
fn b6_device_onto_the_b6_profile() {
    let prefs = parse_devices(PREFS).unwrap();
    let dev = find_device(&prefs, "JCZFiber").unwrap();
    let base = b6();
    let (p, diffs) = profile_from_lightburn(dev, &base);
    println!("{}", diff_table(&diffs));
    p.validate().unwrap();
    // Same lens: LightBurn's 153.8 is markcfg's 153.75 rounded, so the base's is kept (and reported).
    assert_eq!(p.field_mm, 153.75);
    assert!(diff(&diffs, "field_mm").unwrap().note.contains("same lens"));
    // Agrees with the markcfg-based profile on everything both describe.
    assert_eq!((p.swap_xy, p.negate_x, p.negate_y), (true, false, false));
    assert!(diff(&diffs, "swap_xy").is_none() && diff(&diffs, "negate_x").is_none());
    assert_eq!(p.cor_file, base.cor_file);
    assert_eq!((p.red_light_speed_mm_s, p.start_mark_input_bit), (3000.0, Some(14)));
    assert_eq!((p.frequency_khz_min, p.frequency_khz_max), (1.0, 4000.0));
    // LightBurn's values for the rest.
    assert_eq!((p.timing.laser_on_delay_us, p.timing.laser_off_delay_us, p.timing.polygon_delay_us), (-300.0, 100.0, 100.0));
    assert_eq!((p.timing.jump_speed_mm_s, p.timing.jump_delay_min_us, p.timing.jump_delay_max_us), (4000.0, 500.0, 500.0));
    assert_eq!(p.timing.jump_distance_limit_mm, 0.0);
    assert_eq!(p.mark_speed.max_mm_s, 7000.0, "LightBurn's MaxSpeed lowers the hard limit");
    assert_eq!((p.pulse_width_enabled, p.open_mo_delay_us), (true, 8000.0));
    // Hard limits the import never touches.
    assert_eq!((p.max_jump_speed_mm_s, p.max_delay_us, p.max_power_pct), (base.max_jump_speed_mm_s, base.max_delay_us, base.max_power_pct));
    let fields: Vec<&str> = diffs.iter().map(|d| d.field.as_str()).collect();
    assert_eq!(
        fields,
        [
            "field_mm",
            "timing.jump_speed_mm_s",
            "timing.jump_delay_max_us",
            "timing.jump_delay_min_us",
            "timing.jump_distance_limit_mm",
            "mark_speed.max_mm_s",
            // LightBurn's StartMarkBitHigh = true, but the owner's foot switch reads low while pressed (measured).
            "start_mark_active_low",
        ]
    );
    // Importing again onto the result changes nothing more.
    let (again, diffs2) = profile_from_lightburn(dev, &p);
    assert_eq!((again.timing, again.field_mm), (p.timing.clone(), p.field_mm));
    assert_eq!(diffs2.iter().map(|d| d.field.as_str()).collect::<Vec<_>>(), ["field_mm"], "only the same-lens note");
    // The TOML round-trips.
    assert_eq!(MachineProfile::from_toml(&p.to_toml().unwrap()).unwrap(), p);
}

#[test]
fn lightburn_and_markcfg_agree_on_the_axes() {
    let prefs = parse_devices(PREFS).unwrap();
    let from_markcfg = MachineProfile::from_markcfg_bytes(MARKCFG).unwrap();
    let generic = MachineProfile::generic_lmc_fiber();
    let (from_lb, _) = profile_from_lightburn(prefs.default_device().unwrap(), &generic);
    assert_eq!(
        (from_lb.swap_xy, from_lb.negate_x, from_lb.negate_y),
        (from_markcfg.swap_xy, from_markcfg.negate_x, from_markcfg.negate_y),
        "Galvo_1_is_X=false and GALVOX=1 describe the same B6"
    );
    assert_eq!(from_lb.start_mark_input_bit, from_markcfg.start_mark_input_bit);
    assert_eq!(from_lb.cor_file, from_markcfg.cor_file);
    assert!((from_lb.field_mm - from_markcfg.field_mm).abs() <= 0.05 + 1e-9, "153.8 vs 153.75: the same lens");
    // Onto the generic 150 mm profile the LightBurn width is taken as is.
    assert_eq!(from_lb.field_mm, 153.8);
}

#[test]
fn odd_device_is_clamped_and_reported() {
    let prefs = parse_devices(PREFS).unwrap();
    let dev = find_device(&prefs, "Bench 110 lens").unwrap();
    let base = b6();
    let (p, diffs) = profile_from_lightburn(dev, &base);
    println!("{}", diff_table(&diffs));
    p.validate().unwrap();
    assert_eq!(p.field_mm, 110.0);
    assert_eq!(p.field_height_mm, Some(100.0), "a 110 x 100 field keeps its own height");
    assert!(diff(&diffs, "field_height_mm").is_some());
    // Galvo 1 is X: not swapped, X on galvo 1 (sign -1) and mirrored: negate_x = true ^ true.
    assert!(!p.swap_xy);
    assert!(!p.negate_x && !p.negate_y);
    assert!(diff(&diffs, "swap_xy").unwrap().note.contains("Galvo_1_is_X"));
    // Empty correction file keeps the base's.
    assert_eq!(p.cor_file, base.cor_file);
    // Clamped to the profile's hard limits, never raised.
    assert_eq!(p.timing.jump_speed_mm_s, base.max_jump_speed_mm_s);
    assert_eq!(p.red_light_speed_mm_s, base.max_jump_speed_mm_s);
    assert_eq!(p.timing.laser_on_delay_us, -base.max_delay_us);
    assert_eq!((p.timing.jump_delay_min_us, p.timing.jump_delay_max_us), (50.0, base.max_delay_us));
    assert_eq!(p.mark_speed.max_mm_s, base.mark_speed.max_mm_s);
    assert_eq!((p.frequency_khz_min, p.frequency_khz_max), (20.0, base.frequency_khz_max));
    assert_eq!(p.max_jump_speed_mm_s, base.max_jump_speed_mm_s);
    assert_eq!(p.red_light_port_bit, 8);
    assert_eq!(p.start_mark_input_bit, None);
    for (field, words) in [
        ("timing.jump_speed_mm_s", "clamped"),
        ("red_light_speed_mm_s", "clamped"),
        ("timing.laser_on_delay_us", "clamped"),
        ("timing.jump_delay_max_us", "clamped"),
        ("mark_speed.max_mm_s", "hard limit stays"),
        ("frequency_khz_max", "limit stays"),
        ("red_light_port_bit", "not applied"),
        ("calibration.angle_deg", "not applied"),
        ("cor_file", "kept"),
    ] {
        let d = diff(&diffs, field).unwrap_or_else(|| panic!("{field}: {diffs:?}"));
        assert!(d.note.contains(words), "{field}: {}", d.note);
    }
}

#[test]
fn a_change_the_profile_refuses_is_reported_not_applied() {
    let prefs = parse_devices(r#"{"DeviceList": [{"Name": "x", "Settings": {"Default_End_TC": 99999999}}]}"#).unwrap();
    let base = b6();
    let (p, diffs) = profile_from_lightburn(&prefs.devices[0], &base);
    p.validate().unwrap();
    assert_eq!(p.timing.end_delay_us, base.timing.end_delay_us);
    assert!(diff(&diffs, "timing.end_delay_us").unwrap().note.contains("not applied"), "{diffs:?}");
}

/// Real `prefs.ini` files from `OPENETCH_LIGHTBURN_PREFS` (`;`-separated). Prints each device's diff table against
/// the B6 profile.
#[test]
fn real_prefs_files() {
    let Ok(list) = std::env::var("OPENETCH_LIGHTBURN_PREFS") else {
        println!("OPENETCH_LIGHTBURN_PREFS is not set; skipping the real prefs.ini check");
        return;
    };
    let base = b6();
    let mut checked = 0;
    for path in list.split(';').map(str::trim).filter(|p| !p.is_empty()) {
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"));
        let prefs = parse_devices(&text).unwrap_or_else(|e| panic!("{path}: {e}"));
        let stored = serde_json::to_string(&prefs).unwrap();
        for leaked in ["RestPendant", "Secret", "Users", "Documents", "GUID"] {
            assert!(!stored.contains(leaked), "{path}: {leaked} survived parsing");
        }
        println!("== {path}: {} device(s), default {:?}", prefs.devices.len(), prefs.default_device().map(|d| &d.name));
        for dev in &prefs.devices {
            let (p, diffs) = profile_from_lightburn(dev, &base);
            p.validate().unwrap_or_else(|e| panic!("{path} {}: {e}", dev.name));
            println!(
                "-- device \"{}\" (\"{}\", {}), field {:?} x {:?} mm, correction {:?}",
                dev.name, dev.display_name, dev.kind, dev.width_mm, dev.height_mm, dev.correction_file
            );
            println!("{}", diff_table(&diffs));
            checked += 1;
        }
    }
    assert!(checked > 0, "no devices in {list}");
}

/// The same real files as OpenEtch devices: every device classifies, and LightBurn's default device, when it is a JCZ
/// galvo of the B6's lens, is built on the B6 profile. Prints only the device's typed summary.
#[test]
fn real_prefs_files_as_devices() {
    let Ok(list) = std::env::var("OPENETCH_LIGHTBURN_PREFS") else {
        println!("OPENETCH_LIGHTBURN_PREFS is not set; skipping the real device check");
        return;
    };
    let bases = Bases { builtin: b6(), generic: MachineProfile::generic_jcz_galvo() };
    for path in list.split(';').map(str::trim).filter(|p| !p.is_empty()) {
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"));
        let prefs = parse_devices(&text).unwrap_or_else(|e| panic!("{path}: {e}"));
        for dev in &prefs.devices {
            let d = match device::from_lightburn(dev, DeviceSource::LightburnPrefs, &bases) {
                Ok(d) => d,
                Err(e) => {
                    println!("-- \"{}\": skipped: {e}", dev.name);
                    continue;
                }
            };
            println!(
                "-- \"{}\": {} {} {:?} mm, origin {}, base {:?}, controllable {}",
                d.display_name,
                d.kind.name(),
                d.lightburn_type,
                d.workspace_mm,
                d.origin.name(),
                d.base,
                d.controllable
            );
            if let Some(p) = &d.profile {
                p.validate().unwrap();
            }
            let text = d.to_toml().unwrap() + &d.settings_view().to_string();
            for leaked in ["RestPendant", "Secret", "Users", "Documents", "GUID"] {
                assert!(!text.contains(leaked), "{path}: {leaked} in the device");
            }
            if prefs.default_device() == Some(dev) && d.kind == DeviceKind::JczGalvo && device::same_lens(dev, &bases.builtin) {
                assert_eq!(d.base, Some(device::ProfileBase::Builtin));
                assert_eq!(d.workspace_mm, [bases.builtin.field_mm; 2]);
            }
        }
    }
}
