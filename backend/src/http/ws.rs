//! WebSocket upgrade over hyper, with the handshake checks and Origin policy
//! of coder/websocket, which clients already rely on.

use http::{HeaderMap, HeaderValue, StatusCode, header};
use hyper::upgrade::{OnUpgrade, Upgraded};
use hyper_util::rt::TokioIo;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};

use super::{Req, Resp, empty, error, response};

pub type WebSocket = WebSocketStream<TokioIo<Upgraded>>;

/// Generous for SDP: an answer grows with every m-line the session has held.
pub const READ_LIMIT: usize = 256 * 1024;

/// Validates the handshake and returns the 101 response plus the pending
/// upgrade, or the error response to send instead.
pub fn accept(req: &mut Req, origin_patterns: &[String]) -> Result<(Resp, OnUpgrade), Box<Resp>> {
    let h = req.headers();
    if !header_has_token(h, header::CONNECTION, "upgrade") {
        let mut r = error(StatusCode::UPGRADE_REQUIRED, "WebSocket protocol violation: Connection header \"\" does not contain Upgrade");
        r.headers_mut().insert(header::CONNECTION, HeaderValue::from_static("Upgrade"));
        r.headers_mut().insert(header::UPGRADE, HeaderValue::from_static("websocket"));
        return Err(Box::new(r));
    }
    if !header_has_token(h, header::UPGRADE, "websocket") {
        let mut r = error(StatusCode::UPGRADE_REQUIRED, "WebSocket protocol violation: Upgrade header does not contain websocket");
        r.headers_mut().insert(header::CONNECTION, HeaderValue::from_static("Upgrade"));
        r.headers_mut().insert(header::UPGRADE, HeaderValue::from_static("websocket"));
        return Err(Box::new(r));
    }
    if req.method() != http::Method::GET {
        return Err(Box::new(error(StatusCode::METHOD_NOT_ALLOWED, "WebSocket protocol violation: handshake request method is not GET")));
    }
    if h.get(header::SEC_WEBSOCKET_VERSION).map(|v| v.as_bytes()) != Some(b"13") {
        let mut r = error(StatusCode::BAD_REQUEST, "unsupported WebSocket protocol version (only 13 is supported)");
        r.headers_mut().insert(header::SEC_WEBSOCKET_VERSION, HeaderValue::from_static("13"));
        return Err(Box::new(r));
    }
    let Some(key) = h.get(header::SEC_WEBSOCKET_KEY).filter(|k| !k.is_empty()) else {
        return Err(Box::new(error(StatusCode::BAD_REQUEST, "WebSocket protocol violation: missing Sec-WebSocket-Key")));
    };
    let accept_key = derive_accept_key(key.as_bytes());
    if let Err(msg) = authenticate_origin(h, origin_patterns) {
        return Err(Box::new(error(StatusCode::FORBIDDEN, &msg)));
    }
    let on_upgrade = hyper::upgrade::on(req);
    let mut r = response(StatusCode::SWITCHING_PROTOCOLS, empty());
    let rh = r.headers_mut();
    rh.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
    rh.insert(header::CONNECTION, HeaderValue::from_static("Upgrade"));
    rh.insert(header::SEC_WEBSOCKET_ACCEPT, HeaderValue::from_str(&accept_key).expect("base64"));
    Ok((r, on_upgrade))
}

pub async fn upgraded(on: OnUpgrade) -> Option<WebSocket> {
    let io = on.await.ok()?;
    let mut cfg = WebSocketConfig::default();
    cfg.max_message_size = Some(READ_LIMIT);
    cfg.max_frame_size = Some(READ_LIMIT);
    Some(WebSocketStream::from_raw_socket(TokioIo::new(io), Role::Server, Some(cfg)).await)
}

fn header_has_token(h: &HeaderMap, name: header::HeaderName, token: &str) -> bool {
    h.get_all(name).iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(',')).any(|t| t.trim().eq_ignore_ascii_case(token))
}

/// No Origin, a same-host Origin, or one matching a pattern is allowed.
fn authenticate_origin(h: &HeaderMap, patterns: &[String]) -> Result<(), String> {
    let Some(origin) = h.get(header::ORIGIN).and_then(|v| v.to_str().ok()).filter(|o| !o.is_empty()) else {
        return Ok(());
    };
    let Some((scheme, rest)) = origin.split_once("://") else {
        return Err(format!("failed to parse Origin header {origin:?}"));
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let req_host = h.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("");
    if req_host.eq_ignore_ascii_case(host) {
        return Ok(());
    }
    for p in patterns {
        let target = if p.contains("://") { format!("{scheme}://{host}") } else { host.to_string() };
        if glob_match(&p.to_ascii_lowercase(), &target.to_ascii_lowercase()) {
            return Ok(());
        }
    }
    Err(format!("request Origin {origin:?} is not a host in AcceptOptions.OriginPatterns"))
}

/// path.Match subset: `*` (any run without '/') and `?`.
fn glob_match(pat: &str, s: &str) -> bool {
    let (p, s) = (pat.as_bytes(), s.as_bytes());
    let (mut pi, mut si) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while si < s.len() {
        if pi < p.len() && (p[pi] == b'?' && s[si] != b'/' || p[pi] == s[si] && p[pi] != b'*') {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = Some((pi, si));
            pi += 1;
        } else if let Some((sp, ss)) = star {
            if s[ss] == b'/' {
                return false;
            }
            pi = sp + 1;
            si = ss + 1;
            star = Some((sp, ss + 1));
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(origin: &str, host: &str) -> HeaderMap {
        let mut m = HeaderMap::new();
        m.insert(header::ORIGIN, origin.parse().unwrap());
        m.insert(header::HOST, host.parse().unwrap());
        m
    }

    #[test]
    fn origin_policy() {
        let dev = vec!["localhost:*".to_string(), "127.0.0.1:*".to_string()];
        assert!(authenticate_origin(&h("http://localhost:5173", "localhost:8080"), &dev).is_ok());
        assert!(authenticate_origin(&h("http://evil.com", "localhost:8080"), &dev).is_err());
        assert!(authenticate_origin(&h("https://vh.example", "vh.example"), &[]).is_ok());
        let prod = vec!["vh.example".to_string()];
        assert!(authenticate_origin(&h("https://vh.example", "app:8080"), &prod).is_ok());
        assert!(authenticate_origin(&h("https://vh.example.evil", "app:8080"), &prod).is_err());
        assert!(authenticate_origin(&HeaderMap::new(), &prod).is_ok());
    }

    #[test]
    fn globs() {
        assert!(glob_match("localhost:*", "localhost:5173"));
        assert!(glob_match("localhost:*", "localhost:"));
        assert!(!glob_match("localhost:*", "localhostx:1"));
        assert!(glob_match("a?c", "abc"));
    }
}
