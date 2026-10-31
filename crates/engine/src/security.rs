//! Request guard: Host/Origin checks against DNS rebinding and cross-site requests, and a bearer token when the
//! engine listens off loopback.
//!
//! - **Host**: must be a loopback name (`localhost`, `*.localhost`), an IP literal (a rebinding attack needs a DNS
//!   name), or one of `--allow-host`. Applies to every route, including static files and `/mcp`.
//! - **Origin** (when present): its host must be a loopback name or address, one of `--allow-host`, or the same
//!   host as the request's `Host` (a same-origin page). `Origin: null` is refused.
//! - **Token**: when configured, `/api/*` and `/mcp` need `Authorization: Bearer <token>` or `?token=<token>` (for
//!   the WebSocket). Static files do not (the page reads the token from its own URL), and neither does
//!   `POST /api/machine/stop`, which is never gated.

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

/// Whether the request presents `token` as a bearer header or `?token=`.
pub fn token_presented(req: &Request, token: &str) -> bool {
    let bearer = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")))
        .is_some_and(|t| same(t.trim(), token));
    let query = req
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
    fn mcp_marker() {
        let mut h = HeaderMap::new();
        assert!(!has_mcp_marker(&h));
        h.insert("mcp-session-id", "abc".parse().unwrap());
        assert!(has_mcp_marker(&h));
    }
}
