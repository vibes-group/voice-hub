//! HTTP plumbing: body type, response helpers matching Go's net/http output,
//! form/query parsing and client-IP resolution.

pub mod handlers;
pub mod server;
pub mod sniff;
pub mod upload;
pub mod ws;

#[cfg(test)]
mod router_tests;

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use bytes::Bytes;
use http::{HeaderValue, Response, StatusCode, header};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Full};
use serde::Serialize;

use crate::config::Prefix;

pub type Body = UnsyncBoxBody<Bytes, std::io::Error>;
pub type Resp = Response<Body>;
/// Requests carry a boxed body so handlers are testable without a socket.
pub type Req = http::Request<Body>;

pub fn boxed_request(req: http::Request<hyper::body::Incoming>) -> Req {
    req.map(|b| b.map_err(std::io::Error::other).boxed_unsync())
}

pub fn full(b: impl Into<Bytes>) -> Body {
    Full::new(b.into()).map_err(|e: Infallible| match e {}).boxed_unsync()
}

pub fn empty() -> Body {
    full(Bytes::new())
}

pub fn response(status: StatusCode, body: Body) -> Resp {
    let mut r = Response::new(body);
    *r.status_mut() = status;
    r
}

/// Serves through tower-http's file services (Range, conditional GET),
/// with Go's 404 body.
pub async fn serve_fs<S>(svc: S, req: Req) -> Resp
where
    S: tower::Service<
            http::Request<http_body_util::Empty<Bytes>>,
            Response = Response<tower_http::services::fs::ServeFileSystemResponseBody>,
            Error = Infallible,
        >,
{
    use tower::ServiceExt;
    match svc.oneshot(req.map(|_| http_body_util::Empty::new())).await {
        Ok(r) if r.status() == StatusCode::NOT_FOUND => not_found(),
        Ok(r) => r.map(|b| b.map_err(std::io::Error::other).boxed_unsync()),
        Err(e) => match e {},
    }
}

/// http.Error: plain-text message plus newline.
pub fn error(status: StatusCode, msg: &str) -> Resp {
    let mut r = response(status, full(format!("{msg}\n")));
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    r
}

/// http.NotFound.
pub fn not_found() -> Resp {
    error(StatusCode::NOT_FOUND, "404 page not found")
}

pub fn no_content() -> Resp {
    response(StatusCode::NO_CONTENT, empty())
}

/// json.NewEncoder(w).Encode: compact JSON plus trailing newline.
pub fn json<T: Serialize>(status: StatusCode, v: &T) -> Resp {
    let mut body = serde_json::to_vec(v).expect("serializable response");
    body.push(b'\n');
    let mut r = response(status, full(body));
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    r
}

/// http.Redirect, including the tiny HTML body Go writes for GET/HEAD.
pub fn redirect(method: &http::Method, location: &str, status: StatusCode) -> Resp {
    let mut body = Bytes::new();
    let mut ct = None;
    if method == http::Method::GET || method == http::Method::HEAD {
        let text = status.canonical_reason().unwrap_or("");
        body = Bytes::from(format!("<a href=\"{}\">{text}</a>.\n\n", html_escape(location)));
        ct = Some("text/html; charset=utf-8");
    }
    let mut r = response(status, full(body));
    if let Ok(v) = HeaderValue::from_str(location) {
        r.headers_mut().insert(header::LOCATION, v);
    }
    if let Some(ct) = ct {
        r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(ct));
    }
    r
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&#34;").replace('\'', "&#39;")
}

pub fn set_header(r: &mut Resp, name: &'static str, value: &str) {
    if let Ok(v) = HeaderValue::from_str(value) {
        r.headers_mut().insert(name, v);
    }
}

/// Percent-decoding with `+` as space (application/x-www-form-urlencoded).
pub fn form_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => match (hex_val(b[i + 1]), hex_val(b[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push(h << 4 | l);
                    i += 2;
                }
                _ => out.push(b'%'),
            },
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Percent-decoding without `+` handling (RFC 3986 / RFC 5987).
pub fn percent_decode(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let h = hex_val(*b.get(i + 1)?)?;
            let l = hex_val(*b.get(i + 2)?)?;
            out.push(h << 4 | l);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    Some(out)
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// url.PathEscape: escapes everything outside unreserved and a few sub-delims.
pub fn path_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &c in s.as_bytes() {
        let keep = c.is_ascii_alphanumeric() || b"-_.~$&+=:@".contains(&c);
        if keep {
            out.push(c as char);
        } else {
            out.push_str(&format!("%{c:02X}"));
        }
    }
    out
}

/// First value of each key in an urlencoded string (like url.Values.Get).
pub fn parse_form(s: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for pair in s.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        m.entry(form_decode(k)).or_insert_with(|| form_decode(v));
    }
    m
}

pub fn query(uri: &http::Uri) -> HashMap<String, String> {
    parse_form(uri.query().unwrap_or(""))
}

/// The request's source IP. X-Forwarded-For counts only when the TCP peer is a
/// trusted proxy; the chain is walked right to left and the first untrusted
/// hop is the client. A malformed hop fails safe to the peer address.
pub fn client_ip(peer: SocketAddr, headers: &http::HeaderMap, trusted: &[Prefix]) -> String {
    let addr = unmap(peer.ip());
    if !trusted.iter().any(|p| p.contains(addr)) {
        return addr.to_string();
    }
    // Header.Get: first header line only.
    let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) else {
        return addr.to_string();
    };
    if xff.is_empty() {
        return addr.to_string();
    }
    for part in xff.split(',').rev() {
        let Ok(cand) = part.trim().parse::<IpAddr>() else {
            return addr.to_string();
        };
        let cand = unmap(cand);
        if !trusted.iter().any(|p| p.contains(cand)) {
            return cand.to_string();
        }
    }
    addr.to_string()
}

fn unmap(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

/// Go's time.Duration String() after Round(time.Millisecond).
pub fn go_duration_ms(d: Duration) -> String {
    let ms = (d.as_nanos() + 500_000) / 1_000_000;
    if ms == 0 {
        return "0s".into();
    }
    if ms < 1000 {
        return format!("{ms}ms");
    }
    let total_secs = ms / 1000;
    let frac = ms % 1000;
    let (h, m, s) = (total_secs / 3600, (total_secs / 60) % 60, total_secs % 60);
    let mut sec = s.to_string();
    if frac != 0 {
        sec.push_str(format!(".{frac:03}").trim_end_matches('0'));
    }
    if h > 0 {
        format!("{h}h{m}m{sec}s")
    } else if m > 0 {
        format!("{m}m{sec}s")
    } else {
        format!("{sec}s")
    }
}

/// Go's path.Clean for URL paths (always rooted).
pub fn clean_path(p: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    let mut out = format!("/{}", parts.join("/"));
    if p.ends_with('/') && out != "/" {
        out.push('/');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trusted() -> Vec<Prefix> {
        crate::config::parse_trusted_proxies("10.0.0.0/8").unwrap()
    }

    fn hdr(xff: &str) -> http::HeaderMap {
        let mut h = http::HeaderMap::new();
        h.insert("x-forwarded-for", xff.parse().unwrap());
        h
    }

    #[test]
    fn client_ip_rules() {
        let proxy: SocketAddr = "10.0.0.1:1234".parse().unwrap();
        let direct: SocketAddr = "1.2.3.4:1234".parse().unwrap();
        assert_eq!(client_ip(direct, &hdr("9.9.9.9"), &trusted()), "1.2.3.4");
        assert_eq!(client_ip(proxy, &hdr("9.9.9.9"), &trusted()), "9.9.9.9");
        assert_eq!(client_ip(proxy, &hdr("8.8.8.8, 9.9.9.9, 10.0.0.2"), &trusted()), "9.9.9.9");
        assert_eq!(client_ip(proxy, &hdr("9.9.9.9, garbage"), &trusted()), "10.0.0.1");
        assert_eq!(client_ip(proxy, &hdr("10.0.0.3"), &trusted()), "10.0.0.1");
        assert_eq!(client_ip(proxy, &http::HeaderMap::new(), &trusted()), "10.0.0.1");
        let mapped: SocketAddr = "[::ffff:1.2.3.4]:1".parse().unwrap();
        assert_eq!(client_ip(mapped, &http::HeaderMap::new(), &trusted()), "1.2.3.4");
    }

    #[test]
    fn durations() {
        assert_eq!(go_duration_ms(Duration::from_micros(400)), "0s");
        assert_eq!(go_duration_ms(Duration::from_millis(12)), "12ms");
        assert_eq!(go_duration_ms(Duration::from_millis(1234)), "1.234s");
        assert_eq!(go_duration_ms(Duration::from_millis(1500)), "1.5s");
        assert_eq!(go_duration_ms(Duration::from_secs(62)), "1m2s");
    }

    #[test]
    fn forms_and_paths() {
        let f = parse_form("password=a%2Bb+c&x=1&x=2");
        assert_eq!(f["password"], "a+b c");
        assert_eq!(f["x"], "1");
        assert_eq!(path_escape("a b/ü"), "a%20b%2F%C3%BC");
        assert_eq!(clean_path("/a//b/../c/"), "/a/c/");
        assert_eq!(clean_path("/../x"), "/x");
        assert_eq!(percent_decode("%E2%82%AC.txt").unwrap(), "€.txt".as_bytes());
    }
}
