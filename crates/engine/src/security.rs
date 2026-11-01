//! Request guard: Host/Origin checks against DNS rebinding and cross-site requests, and a bearer token when the
//! engine listens off loopback.
//!
//! - **Host**: must be a loopback name (`localhost`, `*.localhost`), an IP literal (a rebinding attack needs a DNS
//!   name), or one of `--allow-host`. Applies to every route, including static files and `/mcp`.
//! - **Origin** (when present): its host must be a loopback name or address, one of `--allow-host`, or the same
//!   host as the request's `Host` (a same-origin page). `Origin: null` is refused.
//! - **Token**: when configured, `/api/*` and `/mcp` need `Authorization: Bearer <token>`. Only `GET /api/events`
//!   (the WebSocket, which browsers cannot give headers) also accepts `?token=<token>`; everywhere else a query token
//!   is ignored, so it does not end up in logs and history. Static files need no token (the page reads it from its
//!   own URL), and neither does `POST /api/machine/stop`, which is never gated.

use std::net::IpAddr;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
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
    if let Some(token) = &cfg.token
        && protected
        && !stop
        && !token_presented(&req, token)
    {
        return refuse(StatusCode::UNAUTHORIZED, "missing or wrong token (open the URL the engine printed at startup)");
    }
    next.run(req).await
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
    fn mcp_marker() {
        let mut h = HeaderMap::new();
        assert!(!has_mcp_marker(&h));
        h.insert("mcp-session-id", "abc".parse().unwrap());
        assert!(has_mcp_marker(&h));
    }
}
