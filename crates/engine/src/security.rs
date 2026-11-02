//! Request guard: Host/Origin checks against DNS rebinding and cross-site requests, and a bearer token when the
//! engine listens off loopback (or with `--require-token`, for an HTTPS proxy in front of a loopback engine).
//!
//! - **Host**: must be a loopback name (`localhost`, `*.localhost`), an IP literal (a rebinding attack needs a DNS
//!   name), or one of `--allow-host`. Applies to every route, including static files and `/mcp`.
//! - **Origin** (when present): its host must be a loopback name or address, one of `--allow-host`, or the same
//!   host as the request's `Host` (a same-origin page). `Origin: null` is refused.
//! - **Token**: when configured, `/api/*` and `/mcp` need `Authorization: Bearer <token>`. Only `GET /api/events`
//!   (the WebSocket, which browsers cannot give headers) also accepts `?token=<token>`; everywhere else a query token
//!   is ignored, so it does not end up in logs and history. Static files need no token (the page reads it from its
//!   own URL), and neither does `POST /api/machine/stop`, which is never gated.
//! - **Cookie** (app windows): Safari's Add to Dock and iOS Home Screen apps do not see the browser tab's
//!   localStorage, where the page keeps the token. So when a page is opened with the right `?token=`, or an `/api/*`
//!   request presents the right bearer token, the answer also sets [`TOKEN_COOKIE`] (HttpOnly, SameSite=Strict, 400
//!   days), and `/api/*` (never `/mcp`) accepts it in place of the header. A browser sends a cookie on its own, so it
//!   counts only when the request has no `Origin` (a same-origin GET) or one with the request's own host *and port*
//!   (or an `--allow-host` name): another service on the same machine but another port is same-site for the browser
//!   and would otherwise ride on it.
//!
//! The token itself is kept in `<config-dir>/access-token` ([`TOKEN_FILE`], mode 0600) and reused across restarts, so
//! a device is paired once; `openetch token [--rotate]` prints it (or makes a new one).

use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::AppState;

/// Headers an MCP client sends (Streamable HTTP). A request carrying any of them is treated as coming from an
/// agent, and the UI-only approval endpoint refuses it.
pub const MCP_MARKER_HEADERS: [&str; 3] = ["mcp-session-id", "mcp-protocol-version", "x-galvo-agent"];

/// The guard's configuration.
#[derive(Debug, Clone, Default)]
pub struct SecurityConfig {
    /// Required bearer token, if any.
    pub token: Option<String>,
    /// Extra host names (no port) accepted in `Host`/`Origin`, lower case.
    pub allow_hosts: Vec<String>,
}

impl SecurityConfig {
    /// Normalise `allow_hosts` (lower case, port stripped).
    pub fn new(token: Option<String>, allow_hosts: impl IntoIterator<Item = String>) -> Self {
        let allow_hosts =
            allow_hosts.into_iter().map(|h| split_host_port(&h.to_ascii_lowercase()).0.to_owned()).collect();
        SecurityConfig { token, allow_hosts }
    }
}

/// True if the request carries an MCP client header.
pub fn has_mcp_marker(headers: &HeaderMap) -> bool {
    MCP_MARKER_HEADERS.iter().any(|h| headers.contains_key(*h))
}

/// The no-PIN approval mode's check (`--approval click`, `POST /api/runs/{id}/approve`): the request must carry the
/// fetch-metadata headers a browser adds to a `fetch` from a page of this engine, which page scripts cannot set
/// (forbidden header names): `Sec-Fetch-Site: same-origin` and an `Origin` that [`origin_allowed`] accepts for the
/// request's `Host`. A script in another site's page sends `cross-site` and its own origin; an MCP client sends
/// neither. `Err` is the message for the 403.
///
/// What this does **not** prove: a program that is not a browser (curl, a script, an agent with a shell) can send
/// the same two headers, with no token on loopback and with the access token off it. The token stays the network
/// guard, and the foot switch is the approval path backed by hardware. Browsers send `Sec-Fetch-*` only to secure
/// origins (`https://`, or `localhost`/`127.0.0.1`): from a page opened as `http://<lan-name>:7878` on another device
/// (the owner's PC or iPad) the request has no `Sec-Fetch-Site`, and the `Origin` check alone decides. Browsers set
/// `Origin` on every `fetch` POST and page scripts cannot change it, so a script in another site's page is still
/// refused; a non-browser program could send it, exactly as it could send `Sec-Fetch-Site`.
pub fn same_origin_browser_request(headers: &HeaderMap, cfg: &SecurityConfig) -> Result<(), String> {
    const HOW: &str = "starting a run without the PIN needs the Start button of the OpenEtch app in a browser, or the \
                       foot switch";
    let site = headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()).map(str::trim);
    match site {
        Some(s) if s.eq_ignore_ascii_case("same-origin") => {}
        Some(s) => return Err(format!("{HOW}: this request came from another site (Sec-Fetch-Site: {s})")),
        // Plain http:// on the network: browsers leave Sec-Fetch-* out, and the Origin check below decides.
        None => {}
    }
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or_default();
    match headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        Some(o) if origin_allowed(o, host, cfg) => Ok(()),
        Some(_) => Err(format!("{HOW}: origin not allowed")),
        None => Err(format!("{HOW}: this request has no Origin header")),
    }
}

/// A fresh random token: 32 bytes as hex.
pub fn generate_token() -> Result<String, String> {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf).map_err(|e| format!("no random source for the token: {e}"))?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// The file in the config directory that keeps the access token across restarts.
pub const TOKEN_FILE: &str = "access-token";
/// The cookie that carries the token for app windows (see the module docs).
pub const TOKEN_COOKIE: &str = "openetch_token";
/// How long the cookie lives: 400 days, the most Chromium keeps a cookie.
pub const TOKEN_COOKIE_MAX_AGE_S: u64 = 400 * 24 * 3600;

/// A token's form: at least 16 characters of `[A-Za-z0-9_-]` (what `--token` accepts and what a cookie may carry).
pub fn valid_token(t: &str) -> bool {
    t.len() >= 16 && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `<dir>/access-token`.
pub fn token_path(dir: &Path) -> PathBuf {
    dir.join(TOKEN_FILE)
}

/// The persisted token, if the file exists. A file that holds anything but one valid token is an error: it is never
/// replaced silently, which would log out every paired device.
pub fn read_token(dir: &Path) -> Result<Option<String>, String> {
    let path = token_path(dir);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let token = text.trim();
    if !valid_token(token) {
        return Err(format!(
            "{} does not hold a valid access token; run `openetch token --rotate` to make a new one",
            path.display()
        ));
    }
    #[cfg(unix)]
    {
        // Tighten a copied or hand-made file: on the network the token is as good as a password.
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&path)
            && meta.permissions().mode() & 0o077 != 0
        {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| format!("cannot make {} private (chmod 600): {e}", path.display()))?;
        }
    }
    Ok(Some(token.to_owned()))
}

/// The persisted token, or a fresh one written to `<dir>/access-token` (mode 0600) when there is none yet. The flag
/// is true when it was just created.
pub fn load_or_create_token(dir: &Path) -> Result<(String, bool), String> {
    if let Some(t) = read_token(dir)? {
        return Ok((t, false));
    }
    let t = generate_token()?;
    write_token(dir, &t)?;
    Ok((t, true))
}

/// Replace the persisted token with a fresh one (`openetch token --rotate`). Every paired device has to be paired
/// again; a running engine keeps the old token until it restarts.
pub fn rotate_token(dir: &Path) -> Result<String, String> {
    let t = generate_token()?;
    write_token(dir, &t)?;
    Ok(t)
}

/// Write `token` to `<dir>/access-token` through a fresh temp file (`create_new`, so a planted file or symlink is
/// never followed; mode 0600 on Unix) renamed into place, creating the private config directory first.
fn write_token(dir: &Path, token: &str) -> Result<(), String> {
    crate::pin::create_config_dir(dir)?;
    let path = token_path(dir);
    let tmp = dir.join(format!("{TOKEN_FILE}.tmp"));
    match std::fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("cannot remove the stale {}: {e}", tmp.display())),
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let written = (|| {
        let mut f = opts.open(&tmp).map_err(|e| format!("cannot create {}: {e}", tmp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|e| format!("cannot make {} private (chmod 600): {e}", tmp.display()))?;
        }
        f.write_all(format!("{token}\n").as_bytes())
            .and_then(|_| f.sync_all())
            .map_err(|e| format!("cannot write {}: {e}", tmp.display()))
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("cannot write {}: {e}", path.display())
    })
}

/// `host[:port]` or `[v6]:port` -> (host, port).
fn split_host_port(authority: &str) -> (&str, Option<&str>) {
    if let Some(rest) = authority.strip_prefix('[') {
        if let Some((host, tail)) = rest.split_once(']') {
            return (host, tail.strip_prefix(':'));
        }
        return (rest, None);
    }
    match authority.rsplit_once(':') {
        // A bare IPv6 address has several colons and no brackets.
        Some((h, p)) if !h.contains(':') => (h, Some(p)),
        _ => (authority, None),
    }
}

fn is_loopback_name(host: &str) -> bool {
    host == "localhost" || host.ends_with(".localhost") || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Whether a `Host` header value is acceptable.
pub fn host_allowed(authority: &str, cfg: &SecurityConfig) -> bool {
    let lower = authority.trim().to_ascii_lowercase();
    let (host, _) = split_host_port(&lower);
    !host.is_empty()
        && (is_loopback_name(host) || host.parse::<IpAddr>().is_ok() || cfg.allow_hosts.iter().any(|a| a == host))
}

/// Whether an `Origin` header value is acceptable for a request whose `Host` is `request_host`.
pub fn origin_allowed(origin: &str, request_host: &str, cfg: &SecurityConfig) -> bool {
    let origin = origin.trim().to_ascii_lowercase();
    let Some((scheme, rest)) = origin.split_once("://") else { return false };
    if scheme != "http" && scheme != "https" {
        return false;
    }
    let authority = rest.split('/').next().unwrap_or_default();
    let (host, _) = split_host_port(authority);
    let req = request_host.trim().to_ascii_lowercase();
    let (req_host, _) = split_host_port(&req);
    !host.is_empty() && (is_loopback_name(host) || cfg.allow_hosts.iter().any(|a| a == host) || host == req_host)
}

/// Constant-time string comparison.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The one route that also accepts the token as `?token=`: the WebSocket upgrade.
pub fn query_token_allowed(method: &Method, path: &str) -> bool {
    method == Method::GET && path == "/api/events"
}

/// Whether the request presents `token` as a bearer header, or as `?token=` on `GET /api/events`.
pub fn token_presented(req: &Request, token: &str) -> bool {
    let bearer = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")))
        .is_some_and(|t| same(t.trim(), token));
    let query = query_token_allowed(req.method(), req.uri().path())
        && req
            .uri()
            .query()
            .is_some_and(|q| q.split('&').filter_map(|kv| kv.strip_prefix("token=")).any(|t| same(t, token)));
    bearer || query
}

/// The value of the [`TOKEN_COOKIE`] cookie, if the request carries one.
pub fn token_cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == TOKEN_COOKIE)
        .map(|(_, v)| v.trim())
}

/// `scheme://host[:port]` -> `host[:port]`, lower case, with the scheme's default port dropped.
fn origin_authority(origin: &str) -> Option<String> {
    let origin = origin.trim().to_ascii_lowercase();
    let (scheme, rest) = origin.split_once("://")?;
    let authority = rest.split('/').next().unwrap_or_default();
    let default_port = match scheme {
        "http" => ":80",
        "https" => ":443",
        _ => return None,
    };
    let authority = authority.strip_suffix(default_port).unwrap_or(authority);
    (!authority.is_empty()).then(|| authority.to_owned())
}

/// Whether the token cookie may stand in for the token on this request: no `Origin` (browsers send none on a
/// same-origin GET), an `Origin` with the request's own host and port, or an `--allow-host` name (a proxy in front
/// that rewrites `Host`). Stricter than [`origin_allowed`], which ignores the port: the cookie must not ride on
/// requests from another service on the same machine.
pub fn cookie_origin_ok(headers: &HeaderMap, request_host: &str, cfg: &SecurityConfig) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else { return true };
    let Some(authority) = origin.to_str().ok().and_then(origin_authority) else { return false };
    let req = request_host.trim().to_ascii_lowercase();
    let req = req.strip_suffix(":80").unwrap_or(&req);
    if authority == req {
        return true;
    }
    let (host, _) = split_host_port(&authority);
    cfg.allow_hosts.iter().any(|a| a == host)
}

/// Whether the answer to this request should set the token cookie: a page navigation (not `/api` or `/mcp`) with the
/// right `?token=`, or an `/api/*` request that presented the right token (header, or the WebSocket's `?token=`)
/// without already carrying a cookie that counts. Never for MCP clients.
fn wants_cookie(req: &Request, token: &str, presented: bool, cookie_ok: bool) -> bool {
    let path = req.uri().path();
    if cookie_ok || has_mcp_marker(req.headers()) || path.starts_with("/mcp") {
        return false;
    }
    if path.starts_with("/api/") || path == "/api" {
        return presented;
    }
    req.method() == Method::GET
        && req
            .uri()
            .query()
            .is_some_and(|q| q.split('&').filter_map(|kv| kv.strip_prefix("token=")).any(|t| same(t, token)))
}

/// The `Set-Cookie` value for `token`: HttpOnly (page scripts never see it), SameSite=Strict, the whole site, 400
/// days; `Secure` when the request came over HTTPS through a proxy that says so (`X-Forwarded-Proto: https`).
pub fn token_set_cookie(token: &str, https: bool) -> String {
    format!(
        "{TOKEN_COOKIE}={token}; Path=/; Max-Age={TOKEN_COOKIE_MAX_AGE_S}; HttpOnly; SameSite=Strict{}",
        if https { "; Secure" } else { "" }
    )
}

fn forwarded_https(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .is_some_and(|p| p.trim().eq_ignore_ascii_case("https"))
}

fn refuse(status: StatusCode, msg: &str) -> Response {
    (status, axum::Json(json!({ "ok": false, "error": msg }))).into_response()
}

/// The axum middleware. See the module docs.
pub async fn guard(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let cfg = &state.security;
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .or_else(|| req.uri().authority().map(|a| a.as_str().to_owned()));
    let Some(host) = host else { return refuse(StatusCode::BAD_REQUEST, "missing Host header") };
    if !host_allowed(&host, cfg) {
        tracing::warn!(%host, "refused request with unknown Host (add it with --allow-host)");
        return refuse(StatusCode::FORBIDDEN, "host not allowed (start the engine with --allow-host <name>)");
    }
    if let Some(origin) = req.headers().get(header::ORIGIN) {
        let ok = origin.to_str().is_ok_and(|o| origin_allowed(o, &host, cfg));
        if !ok {
            tracing::warn!(origin = ?origin, "refused cross-origin request");
            return refuse(StatusCode::FORBIDDEN, "origin not allowed");
        }
    }
    let path = req.uri().path();
    let protected = path.starts_with("/api/") || path == "/api" || path.starts_with("/mcp");
    let stop = req.method() == Method::POST && path == "/api/machine/stop";
    let Some(token) = &cfg.token else { return next.run(req).await };
    let presented = protected && token_presented(&req, token);
    let cookie_ok =
        token_cookie(req.headers()).is_some_and(|c| same(c, token)) && cookie_origin_ok(req.headers(), &host, cfg);
    // MCP stays bearer-only: an agent never rides on a browser's cookie.
    let cookie_counts = cookie_ok && !path.starts_with("/mcp");
    if protected && !stop && !presented && !cookie_counts {
        return refuse(
            StatusCode::UNAUTHORIZED,
            "missing or wrong access token (paste it in the app; `openetch token` prints it on the engine's computer)",
        );
    }
    let https = forwarded_https(req.headers());
    let set_cookie = wants_cookie(&req, token, presented, cookie_ok).then(|| token_set_cookie(token, https));
    let mut res = next.run(req).await;
    if let Some(c) = set_cookie
        && (res.status().is_success() || res.status().is_redirection())
        && let Ok(v) = HeaderValue::from_str(&c)
    {
        res.headers_mut().append(header::SET_COOKIE, v);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> SecurityConfig {
        SecurityConfig::new(None, ["Galvo.Local:7878".to_owned()])
    }

    #[test]
    fn hosts() {
        let c = cfg();
        for ok in ["localhost:7878", "127.0.0.1", "[::1]:7878", "app.localhost", "192.168.1.20:7878", "galvo.local"] {
            assert!(host_allowed(ok, &c), "{ok}");
        }
        for bad in ["evil.example:7878", "localhost.evil.example", "", "galvo.local.evil"] {
            assert!(!host_allowed(bad, &c), "{bad}");
        }
    }

    #[test]
    fn origins() {
        let c = cfg();
        assert!(origin_allowed("http://localhost:5173", "127.0.0.1:7878", &c));
        assert!(origin_allowed("http://192.168.1.20:7878", "192.168.1.20:7878", &c));
        assert!(origin_allowed("http://galvo.local:7878", "192.168.1.20:7878", &c));
        assert!(!origin_allowed("http://192.168.1.99", "127.0.0.1:7878", &c));
        assert!(!origin_allowed("https://evil.example", "localhost:7878", &c));
        assert!(!origin_allowed("null", "localhost:7878", &c));
    }

    #[test]
    fn tokens_are_random_hex() {
        let a = generate_token().unwrap();
        assert_eq!(a.len(), 64);
        assert_ne!(a, generate_token().unwrap());
    }

    #[test]
    fn query_token_only_for_the_websocket() {
        let token = "t0k3n-abcdefghijklmnop";
        let r = |method: &str, uri: &str| {
            Request::builder().method(method).uri(uri).body(axum::body::Body::empty()).unwrap()
        };
        assert!(token_presented(&r("GET", &format!("/api/events?token={token}")), token));
        for (m, uri) in [("GET", "/api/machine"), ("POST", "/api/events"), ("POST", "/mcp"), ("GET", "/api/document")] {
            assert!(!token_presented(&r(m, &format!("{uri}?token={token}")), token), "{m} {uri}");
        }
        let mut req = r("GET", "/api/machine");
        req.headers_mut().insert(header::AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
        assert!(token_presented(&req, token));
    }

    #[test]
    fn same_origin_browser_requests() {
        let c = cfg();
        let h = |pairs: &[(&'static str, &str)]| {
            let mut h = HeaderMap::new();
            for (k, v) in pairs {
                h.insert(*k, v.parse().unwrap());
            }
            h
        };
        let host = ("host", "localhost:7878");
        let ok = h(&[host, ("sec-fetch-site", "same-origin"), ("origin", "http://localhost:7878")]);
        assert_eq!(same_origin_browser_request(&ok, &c), Ok(()));
        // The Vite dev server proxies the app from another loopback port: still accepted.
        assert!(same_origin_browser_request(&h(&[host, ("sec-fetch-site", "same-origin"), ("origin", "http://localhost:5173")]), &c).is_ok());
        // A page opened over plain http:// on the network sends no Sec-Fetch-Site: its Origin decides.
        assert_eq!(same_origin_browser_request(&h(&[host, ("origin", "http://localhost:7878")]), &c), Ok(()));
        let foreign = same_origin_browser_request(&h(&[host, ("origin", "http://evil.example")]), &c).unwrap_err();
        assert!(foreign.contains("origin not allowed") && foreign.contains("foot switch"), "{foreign}");
        assert!(same_origin_browser_request(&h(&[host]), &c).unwrap_err().contains("no Origin"));
        for site in ["cross-site", "same-site", "none"] {
            let e = same_origin_browser_request(&h(&[host, ("sec-fetch-site", site), ("origin", "http://localhost:7878")]), &c);
            assert!(e.is_err(), "{site}");
        }
        assert!(same_origin_browser_request(&h(&[host, ("sec-fetch-site", "same-origin")]), &c).unwrap_err().contains("Origin"));
        let evil = h(&[host, ("sec-fetch-site", "same-origin"), ("origin", "https://evil.example")]);
        assert!(same_origin_browser_request(&evil, &c).is_err());
        assert!(same_origin_browser_request(&h(&[host, ("sec-fetch-site", "same-origin"), ("origin", "null")]), &c).is_err());
    }

    #[test]
    fn token_cookie_and_its_origin_rule() {
        let h = |pairs: &[(&'static str, &str)]| {
            let mut h = HeaderMap::new();
            for (k, v) in pairs {
                h.append(*k, v.parse().unwrap());
            }
            h
        };
        assert_eq!(token_cookie(&h(&[("cookie", "a=1; openetch_token=abc ; b=2")])), Some("abc"));
        assert_eq!(token_cookie(&h(&[("cookie", "a=1"), ("cookie", "openetch_token=xyz")])), Some("xyz"));
        assert_eq!(token_cookie(&h(&[("cookie", "openetch_token_old=abc")])), None);
        let c = cfg();
        let host = "192.168.1.20:7878";
        assert!(cookie_origin_ok(&h(&[]), host, &c), "a same-origin GET has no Origin");
        assert!(cookie_origin_ok(&h(&[("origin", "http://192.168.1.20:7878")]), host, &c));
        assert!(!cookie_origin_ok(&h(&[("origin", "http://192.168.1.20:8080")]), host, &c), "another port");
        assert!(!cookie_origin_ok(&h(&[("origin", "http://192.168.1.20")]), host, &c));
        assert!(!cookie_origin_ok(&h(&[("origin", "null")]), host, &c));
        assert!(cookie_origin_ok(&h(&[("origin", "http://pi.local")]), "pi.local:80", &c), "default port");
        assert!(cookie_origin_ok(&h(&[("origin", "https://galvo.local")]), "127.0.0.1:7878", &c), "--allow-host");
        assert!(!cookie_origin_ok(&h(&[("origin", "https://evil.example")]), "127.0.0.1:7878", &c));
        let set = token_set_cookie("abcdefghijklmnop", false);
        assert_eq!(set, "openetch_token=abcdefghijklmnop; Path=/; Max-Age=34560000; HttpOnly; SameSite=Strict");
        assert!(token_set_cookie("abcdefghijklmnop", true).ends_with("; Secure"));
        assert!(valid_token("abcdefghijklmnop") && !valid_token("short") && !valid_token("abcdefghijklmnop;x=1"));
    }

    #[test]
    fn mcp_marker() {
        let mut h = HeaderMap::new();
        assert!(!has_mcp_marker(&h));
        h.insert("mcp-session-id", "abc".parse().unwrap());
        assert!(has_mcp_marker(&h));
    }
}
