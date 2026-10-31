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
pub const USAGE: &str = "galvo-studio: laser design and control engine

USAGE: galvo-studio [OPTIONS]

OPTIONS:
  --listen <addr>       Address to serve on (default 127.0.0.1:7878). Off loopback a token is required.
  --profile <path>      Machine profile TOML (default fixtures/b6-mopa-60w.toml, found from the current dir up).
  --web-dir <path>      Built web UI to serve (default web/dist, found from the current dir up).
  --usb                 Drive the real laser over USB (needs a build with --features usb). Default: simulator.
  --token <token>       Bearer token to require (default: generated when listening off loopback).
  --allow-host <name>   Extra host name accepted in Host/Origin headers (repeatable), e.g. galvo.local.
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
}

/// What the command line asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum Parsed {
    Run(Config),
    Help,
}

/// Find `rel` in the current directory or one of its ancestors, then next to this crate's repo root; fall back to
/// `rel` as given.
pub fn resolve_default(rel: &str) -> PathBuf {
    if let Ok(cwd) = std::env::current_dir() {
        for dir in cwd.ancestors() {
            let p = dir.join(rel);
            if p.exists() {
                return p;
            }
        }
    }
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join(rel);
    if repo.exists() {
        return repo;
    }
    PathBuf::from(rel)
}

/// Parse arguments (without the program name).
pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Parsed, String> {
    let mut listen: Option<String> = None;
    let mut profile: Option<PathBuf> = None;
    let mut web_dir: Option<PathBuf> = None;
    let mut usb = false;
    let mut token = None;
    let mut allow_hosts = Vec::new();
    let mut it = args.into_iter();
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
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Config {
        match parse(args.iter().map(|s| s.to_string())).unwrap() {
            Parsed::Run(c) => c,
            Parsed::Help => panic!("help"),
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
    }

    #[test]
    fn errors() {
        assert!(parse(["--bogus".to_string()]).is_err());
        assert!(parse(["--listen".to_string()]).is_err());
        assert!(parse(["--listen".to_string(), "nope".to_string()]).is_err());
        assert!(parse(["--token".to_string(), "short".to_string()]).is_err());
    }
}
