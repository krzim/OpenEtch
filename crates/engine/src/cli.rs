//! Command-line options (a tiny hand-written parser; no clap).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Default listen address (loopback only).
pub const DEFAULT_LISTEN: &str = "127.0.0.1:7878";
/// Default machine profile, relative to the repo root.
pub const DEFAULT_PROFILE: &str = "fixtures/b6-mopa-60w.toml";
/// Default web UI build, relative to the repo root.
pub const DEFAULT_WEB_DIR: &str = "web/dist";

/// Usage text for `--help`.
pub const USAGE: &str = "openetch: laser design and control engine

USAGE: openetch [OPTIONS]
       openetch set-pin [--reset] [--config-dir <path>]
       openetch profile from-lightburn [--prefs <path>] [--device <name>] [--base <profile>] --out <file> [--force]
       openetch devices list [--config-dir <path>] [--profile <path>]
       openetch devices import <file> [--add all|<name>] [--config-dir <path>] [--profile <path>]

COMMANDS:
  set-pin               Set the operator PIN (6-12 digits) that approving a run needs. Interactive: run it yourself
                        in a terminal on the engine's computer. Changing an existing PIN asks for the current one
                        first. Stored as a salted SHA-256 hash in <config-dir>/operator-pin. A running engine refuses
                        approvals once the file changes: restart it to use the new PIN.
  set-pin --reset       Delete the operator PIN (forgotten PIN) after typing a confirmation phrase. Approvals stay
                        disabled until a new PIN is set and the engine is restarted.
  profile from-lightburn
                        Write a machine profile TOML (--out) for a LightBurn device: LightBurn's device settings
                        (field, timing, speeds, frequency range, ...) applied onto --base (default the B6 profile),
                        hard limits never raised. Prints every difference. Reads only the device list of LightBurn's
                        prefs.ini (--prefs, default the OS location); --device picks one by name or display name
                        (default LightBurn's default device). Refuses to replace --out without --force.
  devices list          List OpenEtch's devices (<config-dir>/devices; like LightBurn's Devices window): the active
                        one is marked *. On the first run LightBurn's devices on this computer are imported.
  devices import <file> Import the devices of a LightBurn .lbdev file or prefs.ini (only the typed device settings
                        are read; --add picks one by name). A device imported before (same name, controller,
                        connection and size) is updated. Restart the engine to see them.

OPTIONS:
  --listen <addr>       Address to serve on (default 127.0.0.1:7878). Off loopback a token is required.
  --profile <path>      Machine profile TOML (default fixtures/b6-mopa-60w.toml, found from the current dir up).
  --web-dir <path>      Built web UI to serve (default web/dist, found from the current dir up).
  --usb                 Drive the real laser over USB (needs a build with --features usb). Default: simulator.
  --token <token>       Bearer token to require (default: generated when listening off loopback).
  --allow-host <name>   Extra host name accepted in Host/Origin headers (repeatable), e.g. galvo.local.
  --config-dir <path>   Engine config directory (operator PIN). Default: the OS config dir + openetch, e.g.
                        %APPDATA%\\openetch or ~/.config/openetch.
  --projects-dir <path> Where named projects (.openetch) and LightBurn exports are saved. Default: the OS data dir
                        + openetch/projects, e.g. %APPDATA%\\openetch\\projects or ~/.local/share/openetch/projects;
                        created on the first save.
  --confirm-input <bit> Also require a physical input (0-15, ReadPort) after PIN approval: the run waits up to 30 s
                        for the input to be released and then pressed. The B6's markcfg suggests a foot pedal on 14
                        (unverified).
  --lightburn-prefs <path>
                        LightBurn's prefs.ini, for LightBurn's device list (only that is read; the file is never
                        written). Default: the OS location (%LOCALAPPDATA%\\LightBurn\\prefs.ini,
                        ~/Library/Preferences/LightBurn/prefs.ini, ~/.local/share/LightBurn or ~/.config/LightBurn);
                        a missing file is fine. LightBurn imports without a field size use the device's field.
  --lightburn-device <name>
                        The LightBurn device (name or display name) GET /api/lightburn and the machine status compare
                        the profile with. Default: LightBurn's default device.
  --device <id or name> The OpenEtch device to use for this run (default: the active device, set in the UI's Devices
                        window). Its profile drives the machine; a design-only device (not a JCZ galvo) only sets
                        the workspace.
  -h, --help            Show this help.";

/// Parsed options.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub listen: SocketAddr,
    pub profile: PathBuf,
    /// True when `--profile` was not given (a missing default falls back to a generic profile).
    pub profile_is_default: bool,
    pub web_dir: PathBuf,
    pub usb: bool,
    pub token: Option<String>,
    pub allow_hosts: Vec<String>,
    /// Where the operator PIN lives.
    pub config_dir: PathBuf,
    /// Input bit for physical run confirmation, if any.
    pub confirm_input: Option<u8>,
    /// Where named projects are saved.
    pub projects_dir: PathBuf,
    /// `--lightburn-prefs`: LightBurn's `prefs.ini` (`None`: the OS locations).
    pub lightburn_prefs: Option<PathBuf>,
    /// `--lightburn-device`: the LightBurn device used when a file names none.
    pub lightburn_device: Option<String>,
    /// `--device`: the OpenEtch device for this run (id or name).
    pub device: Option<String>,
}

/// What the command line asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum Parsed {
    Run(Config),
    /// `openetch set-pin [--reset]`.
    SetPin {
        config_dir: PathBuf,
        /// `--reset`: delete the PIN instead of setting one.
        reset: bool,
    },
    /// `openetch profile from-lightburn ...`.
    ProfileFromLightBurn(crate::lightburn::ProfileFromLightBurn),
    /// `openetch devices list|import ...`.
    Devices(crate::devices::DevicesCommand),
    Help,
}

/// `--config-dir`, or the default from [`crate::pin::default_config_dir`].
fn config_dir(given: Option<PathBuf>) -> Result<PathBuf, String> {
    given
        .or_else(crate::pin::default_config_dir)
        .ok_or_else(|| "cannot find the OS config directory; pass --config-dir <path>".into())
}

/// Find `rel` (a `/`-separated relative path) in the current directory or one of its ancestors, then in this crate's
/// repo root; fall back to `rel` as given. The result uses the OS path separator throughout.
pub fn resolve_default(rel: &str) -> PathBuf {
    let rel: PathBuf = rel.split('/').collect();
    if let Ok(cwd) = std::env::current_dir() {
        for dir in cwd.ancestors() {
            let p = dir.join(&rel);
            if p.exists() {
                return p;
            }
        }
    }
    // crates/engine -> the repo root, without `..` components.
    if let Some(root) = Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent) {
        let repo = root.join(&rel);
        if repo.exists() {
            return repo;
        }
    }
    rel
}

/// Parse arguments (without the program name).
pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Parsed, String> {
    let mut args = args.into_iter().peekable();
    if args.peek().is_some_and(|a| a == "set-pin") {
        args.next();
        return parse_set_pin(args);
    }
    if args.peek().is_some_and(|a| a == "profile") {
        args.next();
        return parse_profile(args);
    }
    if args.peek().is_some_and(|a| a == "devices") {
        args.next();
        return parse_devices(args);
    }
    let mut listen: Option<String> = None;
    let mut config: Option<PathBuf> = None;
    let mut projects_dir: Option<PathBuf> = None;
    let mut confirm_input = None;
    let mut profile: Option<PathBuf> = None;
    let mut web_dir: Option<PathBuf> = None;
    let mut usb = false;
    let mut token = None;
    let mut allow_hosts = Vec::new();
    let mut lightburn_prefs = None;
    let mut lightburn_device = None;
    let mut device = None;
    let mut it = args;
    while let Some(arg) = it.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_owned(), Some(v.to_owned())),
            _ => (arg.clone(), None),
        };
        let mut value = |name: &str| -> Result<String, String> {
            inline.clone().or_else(|| it.next()).ok_or_else(|| format!("{name} needs a value"))
        };
        match flag.as_str() {
            "-h" | "--help" => return Ok(Parsed::Help),
            "--listen" => listen = Some(value("--listen")?),
            "--profile" => profile = Some(PathBuf::from(value("--profile")?)),
            "--web-dir" => web_dir = Some(PathBuf::from(value("--web-dir")?)),
            "--usb" => usb = true,
            "--token" => {
                let t = value("--token")?;
                if t.len() < 16 || !t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                    return Err("--token must be at least 16 characters of [A-Za-z0-9_-]".into());
                }
                token = Some(t);
            }
            "--allow-host" => allow_hosts.push(value("--allow-host")?),
            "--config-dir" => config = Some(PathBuf::from(value("--config-dir")?)),
            "--projects-dir" => projects_dir = Some(PathBuf::from(value("--projects-dir")?)),
            "--confirm-input" => {
                let v = value("--confirm-input")?;
                match v.parse::<u8>() {
                    Ok(bit) if bit <= 15 => confirm_input = Some(bit),
                    _ => return Err(format!("--confirm-input must be an input bit 0-15 (got '{v}')")),
                }
            }
            "--lightburn-prefs" => lightburn_prefs = Some(PathBuf::from(value("--lightburn-prefs")?)),
            "--lightburn-device" => {
                let v = value("--lightburn-device")?;
                if v.trim().is_empty() {
                    return Err("--lightburn-device needs a device name".into());
                }
                lightburn_device = Some(v);
            }
            "--device" => {
                let v = value("--device")?;
                if v.trim().is_empty() {
                    return Err("--device needs a device id or name".into());
                }
                device = Some(v);
            }
            "set-pin" | "profile" | "devices" => {
                return Err(format!("{flag} is a command of its own and comes first (see --help)"));
            }
            other => return Err(format!("unknown option '{other}' (see --help)")),
        }
    }
    let listen_text = listen.unwrap_or_else(|| DEFAULT_LISTEN.to_owned());
    let listen: SocketAddr = listen_text.parse().map_err(|e| format!("--listen {listen_text}: {e}"))?;
    let profile_is_default = profile.is_none();
    Ok(Parsed::Run(Config {
        listen,
        profile: profile.unwrap_or_else(|| resolve_default(DEFAULT_PROFILE)),
        profile_is_default,
        web_dir: web_dir.unwrap_or_else(|| resolve_default(DEFAULT_WEB_DIR)),
        usb,
        token,
        allow_hosts,
        config_dir: config_dir(config)?,
        confirm_input,
        projects_dir: projects_dir
            .or_else(crate::projects::default_projects_dir)
            .ok_or("cannot find the OS data directory; pass --projects-dir <path>")?,
        lightburn_prefs,
        lightburn_device,
        device,
    }))
}

/// `openetch devices list|import <file> [--add all|<name>] [--config-dir P] [--profile P]`.
fn parse_devices(mut it: impl Iterator<Item = String>) -> Result<Parsed, String> {
    const USE: &str = "usage: openetch devices list [--config-dir <path>] [--profile <path>] | openetch devices import \
                       <file> [--add all|<name>] [--config-dir <path>] [--profile <path>]";
    let import = match it.next().as_deref() {
        Some("list") => false,
        Some("import") => true,
        Some("-h" | "--help") => return Ok(Parsed::Help),
        _ => return Err(USE.into()),
    };
    let (mut file, mut add, mut dir, mut profile) = (None, None, None, None);
    while let Some(arg) = it.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_owned(), Some(v.to_owned())),
            _ => (arg.clone(), None),
        };
        let mut value = |name: &str| -> Result<String, String> {
            inline.clone().or_else(|| it.next()).ok_or_else(|| format!("{name} needs a value"))
        };
        match flag.as_str() {
            "--config-dir" => dir = Some(PathBuf::from(value("--config-dir")?)),
            "--profile" => profile = Some(PathBuf::from(value("--profile")?)),
            "--add" if import => add = Some(value("--add")?),
            "-h" | "--help" => return Ok(Parsed::Help),
            f if import && file.is_none() && !f.starts_with("--") => file = Some(PathBuf::from(arg)),
            other => return Err(format!("devices: unexpected argument '{other}' ({USE})")),
        }
    }
    let action = if import {
        let file = file.ok_or_else(|| format!("devices import needs a file ({USE})"))?;
        crate::devices::DevicesAction::Import { file, filter: crate::devices::ImportFilter::parse(add.as_deref()) }
    } else {
        crate::devices::DevicesAction::List
    };
    let profile_is_default = profile.is_none();
    Ok(Parsed::Devices(crate::devices::DevicesCommand {
        action,
        config_dir: config_dir(dir)?,
        profile: profile.unwrap_or_else(|| resolve_default(DEFAULT_PROFILE)),
        profile_is_default,
        prefs_paths: crate::lightburn::default_prefs_paths(),
    }))
}

/// `openetch profile from-lightburn [--prefs P] [--device NAME] [--base PROFILE] --out FILE [--force]`.
fn parse_profile(mut it: impl Iterator<Item = String>) -> Result<Parsed, String> {
    const USE: &str = "usage: openetch profile from-lightburn [--prefs <path>] [--device <name>] [--base <profile>] \
                       --out <file> [--force]";
    match it.next().as_deref() {
        Some("from-lightburn") => {}
        Some("-h" | "--help") => return Ok(Parsed::Help),
        _ => return Err(USE.into()),
    }
    let (mut prefs, mut device, mut base, mut out, mut force) = (None, None, None, None, false);
    while let Some(arg) = it.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_owned(), Some(v.to_owned())),
            _ => (arg.clone(), None),
        };
        let mut value = |name: &str| -> Result<String, String> {
            inline.clone().or_else(|| it.next()).ok_or_else(|| format!("{name} needs a value"))
        };
        match flag.as_str() {
            "--prefs" => prefs = Some(PathBuf::from(value("--prefs")?)),
            "--device" => device = Some(value("--device")?),
            "--base" => base = Some(PathBuf::from(value("--base")?)),
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            "--force" => force = true,
            "-h" | "--help" => return Ok(Parsed::Help),
            other => return Err(format!("profile from-lightburn: unknown option '{other}' ({USE})")),
        }
    }
    let out = out.ok_or_else(|| format!("profile from-lightburn needs --out <file> ({USE})"))?;
    Ok(Parsed::ProfileFromLightBurn(crate::lightburn::ProfileFromLightBurn { prefs, device, base, out, force }))
}

fn parse_set_pin(mut it: impl Iterator<Item = String>) -> Result<Parsed, String> {
    let mut dir = None;
    let mut reset = false;
    while let Some(arg) = it.next() {
        match arg.split_once('=') {
            Some(("--config-dir", v)) => dir = Some(PathBuf::from(v)),
            _ if arg == "--config-dir" => {
                dir = Some(PathBuf::from(it.next().ok_or("--config-dir needs a value")?));
            }
            _ if arg == "--reset" => reset = true,
            _ if arg == "-h" || arg == "--help" => return Ok(Parsed::Help),
            _ => return Err(format!("set-pin: unknown option '{arg}' (only --reset and --config-dir)")),
        }
    }
    Ok(Parsed::SetPin { config_dir: config_dir(dir)?, reset })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Config {
        match parse(args.iter().map(|s| s.to_string())).unwrap() {
            Parsed::Run(c) => c,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn defaults() {
        let c = run(&[]);
        assert_eq!(c.listen, DEFAULT_LISTEN.parse().unwrap());
        assert!(!c.usb && c.token.is_none() && c.profile_is_default);
        assert!(c.profile.ends_with("b6-mopa-60w.toml"));
    }

    #[test]
    fn options() {
        let c = run(&["--listen", "0.0.0.0:9000", "--usb", "--allow-host=galvo.local", "--token", "abcdefghijklmnop"]);
        assert_eq!(c.listen.port(), 9000);
        assert!(c.usb);
        assert_eq!(c.allow_hosts, vec!["galvo.local"]);
        assert_eq!(c.token.as_deref(), Some("abcdefghijklmnop"));
        assert_eq!(parse(["--help".to_string()]).unwrap(), Parsed::Help);
        let c = run(&["--config-dir", "cfg", "--confirm-input=14"]);
        assert_eq!((c.config_dir, c.confirm_input), (PathBuf::from("cfg"), Some(14)));
        assert_eq!(run(&[]).confirm_input, None);
        assert_eq!(run(&["--projects-dir", "p"]).projects_dir, PathBuf::from("p"));
        assert_eq!(run(&["--projects-dir=q"]).projects_dir, PathBuf::from("q"));
        assert!(run(&[]).projects_dir.ends_with(Path::new("openetch").join("projects")));
    }

    #[test]
    fn set_pin_command() {
        let p = |a: &[&str]| parse(a.iter().map(|s| s.to_string()));
        let set = |dir: &str, reset| Parsed::SetPin { config_dir: PathBuf::from(dir), reset };
        assert_eq!(p(&["set-pin", "--config-dir", "x"]).unwrap(), set("x", false));
        assert_eq!(p(&["set-pin", "--config-dir=y"]).unwrap(), set("y", false));
        assert_eq!(p(&["set-pin", "--reset", "--config-dir", "z"]).unwrap(), set("z", true));
        assert!(p(&["set-pin", "--usb"]).is_err());
        assert!(p(&["--usb", "set-pin"]).is_err());
    }

    #[test]
    fn lightburn_options_and_profile_command() {
        let c = run(&["--lightburn-prefs", "p.ini", "--lightburn-device=JCZFiber"]);
        assert_eq!((c.lightburn_prefs, c.lightburn_device), (Some(PathBuf::from("p.ini")), Some("JCZFiber".into())));
        let c = run(&[]);
        assert_eq!((c.lightburn_prefs, c.lightburn_device), (None, None));
        assert!(parse(["--lightburn-device".to_string(), " ".to_string()]).is_err());
        let p = |a: &[&str]| parse(a.iter().map(|s| s.to_string()));
        let cmd = p(&["profile", "from-lightburn", "--prefs", "a.ini", "--device=JCZ", "--base", "b.toml", "--out", "o.toml", "--force"]);
        assert_eq!(
            cmd.unwrap(),
            Parsed::ProfileFromLightBurn(crate::lightburn::ProfileFromLightBurn {
                prefs: Some("a.ini".into()),
                device: Some("JCZ".into()),
                base: Some("b.toml".into()),
                out: "o.toml".into(),
                force: true,
            })
        );
        let minimal = p(&["profile", "from-lightburn", "--out", "o.toml"]).unwrap();
        assert!(matches!(minimal, Parsed::ProfileFromLightBurn(o) if o.prefs.is_none() && !o.force));
        assert!(p(&["profile", "from-lightburn"]).unwrap_err().contains("--out"));
        assert!(p(&["profile", "to-lightburn", "--out", "x"]).is_err());
        assert!(p(&["profile"]).is_err());
        assert!(p(&["profile", "from-lightburn", "--out", "x", "--usb"]).is_err());
        assert!(p(&["--usb", "profile"]).is_err());
    }

    #[test]
    fn device_option_and_devices_commands() {
        use crate::devices::{DevicesAction, ImportFilter};
        assert_eq!(run(&["--device", "jczfiber-unknown"]).device.as_deref(), Some("jczfiber-unknown"));
        assert_eq!(run(&["--device=Test Bed 400"]).device.as_deref(), Some("Test Bed 400"));
        assert_eq!(run(&[]).device, None);
        assert!(parse(["--device".to_string(), " ".to_string()]).is_err());
        let p = |a: &[&str]| parse(a.iter().map(|s| s.to_string()));
        match p(&["devices", "list", "--config-dir", "cfg"]).unwrap() {
            Parsed::Devices(c) => {
                assert_eq!((c.action, c.config_dir), (DevicesAction::List, PathBuf::from("cfg")));
                assert!(c.profile_is_default && c.profile.ends_with(Path::new("fixtures").join("b6-mopa-60w.toml")));
            }
            other => panic!("{other:?}"),
        }
        match p(&["devices", "import", "x.lbdev", "--add", "Test Bed", "--config-dir=cfg"]).unwrap() {
            Parsed::Devices(c) => assert_eq!(
                c.action,
                DevicesAction::Import { file: "x.lbdev".into(), filter: ImportFilter::Named("Test Bed".into()) }
            ),
            other => panic!("{other:?}"),
        }
        assert!(p(&["devices", "import"]).unwrap_err().contains("needs a file"));
        assert!(p(&["devices", "list", "--add", "x"]).is_err());
        assert!(p(&["devices", "remove", "x"]).is_err());
        assert!(p(&["--usb", "devices"]).is_err());
    }

    #[test]
    fn default_paths_use_one_separator() {
        let p = resolve_default(DEFAULT_PROFILE).display().to_string();
        let other = if std::path::MAIN_SEPARATOR == '/' { '\\' } else { '/' };
        assert!(!p.contains(other) && !p.contains(".."), "{p}");
    }

    #[test]
    fn errors() {
        assert!(parse(["--bogus".to_string()]).is_err());
        assert!(parse(["--listen".to_string()]).is_err());
        assert!(parse(["--listen".to_string(), "nope".to_string()]).is_err());
        assert!(parse(["--token".to_string(), "short".to_string()]).is_err());
        for bad in ["16", "-1", "pedal"] {
            assert!(parse(["--confirm-input".to_string(), bad.to_string()]).is_err(), "{bad}");
        }
    }
}
