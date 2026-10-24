use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use http::{HeaderValue, Method, StatusCode, header};
use http_body_util::{BodyExt, Limited, StreamBody};
use serde::{Deserialize, Serialize};
use sha2::Digest;

use super::server::App;
use super::{Req, Resp, client_ip, error, full, json, no_content, not_found, parse_form, path_escape, redirect, response, set_header};
use crate::auth::connpass::{self, EntryStatus, Ttl};
use crate::auth::{Role, password, session};
use crate::presence::HEARTBEAT;
use crate::{log, turn, util};

/// Short enough to limit exposure if leaked, long enough to cover a call and
/// brief reconnects.
const TURN_CREDS_TTL: Duration = Duration::from_secs(6 * 3600);
const TURN_USERNAME_PREFIX: &str = "u";
const MAX_CONN_PASS_TTL: Duration = Duration::from_secs(366 * 24 * 3600);
/// Go's ParseForm cap on urlencoded bodies.
const MAX_FORM_BYTES: usize = 10 << 20;

/// Fingerprints the deployed frontend by hashing index.html; Vite's hashed
/// asset URLs make any rebuild shift it.
pub fn frontend_version(web_dir: &std::path::Path) -> String {
    let p = web_dir.join("index.html");
    match std::fs::read(&p) {
        Ok(data) => util::hex(&sha2::Sha256::digest(&data))[..12].to_string(),
        Err(e) => {
            log!("version: cannot read {}: {e}", p.display());
            "unknown".into()
        }
    }
}

pub fn health() -> Resp {
    #[derive(Serialize)]
    struct Health {
        status: &'static str,
    }
    json(StatusCode::OK, &Health { status: "ok" })
}

pub fn call_status(app: &App) -> Resp {
    let active = app.rooms.values().any(|r| call_in_progress(&r.peers()));
    let mut r = response(StatusCode::OK, full(if active { "active\n" } else { "idle\n" }));
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    r
}

/// Two non-chat-only peers make a call: chat-only peers neither send nor hear
/// audio, and one peer alone has no one on the other side.
pub fn call_in_progress(peers: &[crate::sfu::protocol::PeerInfo]) -> bool {
    peers.iter().filter(|p| !p.chat_only).take(2).count() == 2
}

pub fn version(app: &App) -> Resp {
    #[derive(Serialize)]
    struct Version<'a> {
        version: &'a str,
    }
    let mut r = json(StatusCode::OK, &Version { version: &app.version });
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

async fn read_body(req: Req, limit: usize) -> Option<Bytes> {
    Limited::new(req.into_body(), limit).collect().await.ok().map(|c| c.to_bytes())
}

fn content_type(req: &Req) -> String {
    req.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string()
}

/// r.PostFormValue over urlencoded and multipart bodies.
async fn post_form(req: Req) -> Result<HashMap<String, String>, ()> {
    let ct = content_type(&req);
    let media = ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    if media == "application/x-www-form-urlencoded" {
        let body = read_body(req, MAX_FORM_BYTES).await.ok_or(())?;
        return Ok(parse_form(&String::from_utf8_lossy(&body)));
    }
    if media == "multipart/form-data" {
        let boundary = multer::parse_boundary(&ct).map_err(|_| ())?;
        let stream = Limited::new(req.into_body(), MAX_FORM_BYTES).into_data_stream();
        let mut mp = multer::Multipart::new(stream, boundary);
        let mut out = HashMap::new();
        while let Some(field) = mp.next_field().await.map_err(|_| ())? {
            if field.file_name().is_some() {
                continue;
            }
            let name = field.name().unwrap_or("").to_string();
            let text = field.text().await.map_err(|_| ())?;
            out.entry(name).or_insert(text);
        }
        return Ok(out);
    }
    Ok(HashMap::new())
}

/// POST /api/login: admin password first, then any connection password.
pub async fn login(app: &App, req: Req, peer: SocketAddr) -> Resp {
    let ip = client_ip(peer, req.headers(), &app.cfg.trusted_proxies);
    if app.limiter.blocked(&ip) {
        let mut r = error(StatusCode::TOO_MANY_REQUESTS, "too many failed attempts");
        r.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_static("900"));
        return r;
    }
    let Ok(form) = post_form(req).await else {
        return error(StatusCode::BAD_REQUEST, "bad request");
    };
    let pass = form.get("password").cloned().unwrap_or_default();
    if pass.is_empty() {
        app.limiter.fail(&ip);
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    // argon2 is CPU-bound (~50 ms per hash); keep it off the async workers.
    let check = {
        let hash = app.cfg.admin_password_hash.clone();
        let pass = pass.clone();
        tokio::task::spawn_blocking(move || password::verify(&hash, &pass))
    };
    if check.await.unwrap_or(false) {
        app.limiter.success(&ip);
        return with_cookie(session::session_cookie(app.cfg.cookie_secure, &app.session_secret, Role::Admin, 0, &app.admin_ver, ""));
    }
    let verified = tokio::task::block_in_place(|| app.conn_pass.verify(&pass));
    if let Some((entry_id, gen_)) = verified {
        app.limiter.success(&ip);
        return with_cookie(session::session_cookie(app.cfg.cookie_secure, &app.session_secret, Role::User, gen_, "", &entry_id));
    }
    app.limiter.fail(&ip);
    error(StatusCode::UNAUTHORIZED, "unauthorized")
}

fn with_cookie(cookie: String) -> Resp {
    let mut r = no_content();
    r.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).expect("cookie is ASCII"));
    r
}

pub fn logout(app: &App) -> Resp {
    with_cookie(session::clear_cookie(app.cfg.cookie_secure))
}

/// TURN REST API credentials: username `<unix_expiry>:<id>`, credential
/// base64(HMAC-SHA1(secret, username)).
pub fn turn_credentials(secret: &str, id: &str, ttl: Duration) -> (String, String) {
    let id = if id.is_empty() { "anon" } else { id };
    let expiry = util::unix_secs(SystemTime::now() + ttl);
    let username = format!("{expiry}:{id}");
    let credential = turn::credential(secret, &username);
    (username, credential)
}

pub fn app_config(app: &App, req: &Req) -> Resp {
    #[derive(Serialize)]
    struct IceServer<'a> {
        urls: [&'a str; 1],
        #[serde(skip_serializing_if = "str::is_empty")]
        username: &'a str,
        #[serde(skip_serializing_if = "str::is_empty")]
        credential: &'a str,
    }
    #[derive(Serialize)]
    struct AppConfig<'a> {
        #[serde(rename = "iceServers")]
        ice_servers: [IceServer<'a>; 2],
        role: &'a str,
    }
    let role = session::from_headers(&app.session_secret, req.headers()).map(|s| s.role.as_str()).unwrap_or("");
    let (username, credential) = turn_credentials(&app.turn_secret, TURN_USERNAME_PREFIX, TURN_CREDS_TTL);
    json(
        StatusCode::OK,
        &AppConfig {
            ice_servers: [
                IceServer { urls: [&app.stun_url], username: "", credential: "" },
                IceServer { urls: [&app.turn_url], username: &username, credential: &credential },
            ],
            role,
        },
    )
}

/// GET /api/presence: SSE stream, snapshot first, heartbeat comment every 25s.
pub fn presence(app: &Arc<App>, req: &Req) -> Resp {
    let role = session::from_headers(&app.session_secret, req.headers()).map(|s| s.role).unwrap_or(Role::User);
    let cancel = tokio_util::sync::CancellationToken::new();
    let reg_id = app.ws_registry.add(role, cancel.clone());
    let subscription = app.presence.subscribe();
    let app = app.clone();
    let stream = futures_util::stream::unfold(
        (subscription, tokio::time::interval_at(tokio::time::Instant::now() + HEARTBEAT, HEARTBEAT), Some(Unregister(app, reg_id))),
        move |(sub, mut hb, guard)| {
            let cancel = cancel.clone();
            async move {
                let frame = tokio::select! {
                    _ = cancel.cancelled() => return None,
                    f = sub.sub.next() => f,
                    _ = hb.tick() => Bytes::from_static(b": heartbeat\n\n"),
                };
                Some((Ok::<_, std::io::Error>(http_body::Frame::data(frame)), (sub, hb, guard)))
            }
        },
    );
    let mut r = response(StatusCode::OK, StreamBody::new(stream).boxed_unsync());
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    h.insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
    h.insert("x-accel-buffering", HeaderValue::from_static("no"));
    r
}

struct Unregister(Arc<App>, u64);

impl Drop for Unregister {
    fn drop(&mut self) {
        self.0.ws_registry.remove(self.1);
    }
}

// --- Admin: connection passwords ---

#[derive(Serialize)]
struct PlaintextResponse<'a> {
    host: &'a str,
    id: &'a str,
    label: &'a str,
    password: &'a str,
    generation: u64,
    #[serde(with = "util::go_time")]
    created_at: SystemTime,
}

#[derive(Deserialize, Default)]
struct AdminBody {
    #[serde(default)]
    label: String,
    #[serde(default)]
    ttl_seconds: i64,
}

/// JSON body when Content-Type is application/json, else form values.
/// Malformed input yields defaults.
async fn admin_body(req: Req) -> AdminBody {
    if content_type(&req).starts_with("application/json") {
        let body = read_body(req, MAX_FORM_BYTES).await.unwrap_or_default();
        return serde_json::from_slice(&body).unwrap_or_default();
    }
    let form = post_form(req).await.unwrap_or_default();
    AdminBody {
        label: form.get("label").cloned().unwrap_or_default(),
        ttl_seconds: form.get("ttl_seconds").and_then(|v| v.parse().ok()).unwrap_or(0),
    }
}

/// 0 → never; >0 → seconds capped at a year; <0 → disabled now.
fn ttl_from_seconds(s: i64) -> Ttl {
    match s {
        0 => Ttl::Never,
        s if s < 0 => Ttl::Disabled,
        s => Ttl::For(Duration::from_secs(s as u64).min(MAX_CONN_PASS_TTL)),
    }
}

fn plaintext(host: &str, entry: &EntryStatus, password: &str) -> Resp {
    let mut r = json(
        StatusCode::OK,
        &PlaintextResponse {
            host,
            id: &entry.id,
            label: &entry.label,
            password,
            generation: entry.generation,
            created_at: entry.created_at,
        },
    );
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

fn connpass_error(e: connpass::Error, what: &str) -> Resp {
    match e {
        connpass::Error::NotFound => error(StatusCode::NOT_FOUND, "not found"),
        connpass::Error::TooManyEntries => error(StatusCode::CONFLICT, "too many entries"),
        connpass::Error::Io(msg) => {
            log!("connpass {what}: {msg}");
            error(StatusCode::INTERNAL_SERVER_ERROR, &format!("{what} failed"))
        }
    }
}

pub async fn admin(app: &App, req: Req, rest: &[String]) -> Resp {
    let store = &app.conn_pass;
    let m = req.method().clone();
    let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
    match (m, rest.as_slice()) {
        (Method::GET | Method::HEAD, []) => json(StatusCode::OK, &store.status()),
        (Method::POST, []) => {
            let body = admin_body(req).await;
            let r = tokio::task::block_in_place(|| store.create(&body.label, ttl_from_seconds(body.ttl_seconds)));
            match r {
                Ok((entry, plain)) => plaintext(&app.cfg.app_hostname, &entry, &plain),
                Err(e) => connpass_error(e, "create"),
            }
        }
        (Method::POST, ["disconnect-users"]) => {
            app.ws_registry.disconnect_users();
            no_content()
        }
        (Method::POST, [id, "rotate"]) => match tokio::task::block_in_place(|| store.rotate(id)) {
            Ok((entry, plain)) => plaintext(&app.cfg.app_hostname, &entry, &plain),
            Err(e) => connpass_error(e, "rotate"),
        },
        (Method::POST, [id, "rename"]) => {
            let id = id.to_string();
            let body = admin_body(req).await;
            match store.rename(&id, &body.label) {
                Ok(()) => no_content(),
                Err(e) => connpass_error(e, "rename"),
            }
        }
        (Method::POST, [id, "ttl"]) => {
            let id = id.to_string();
            let body = admin_body(req).await;
            match store.set_ttl(&id, ttl_from_seconds(body.ttl_seconds)) {
                Ok(entry) => json(StatusCode::OK, &entry),
                Err(e) => connpass_error(e, "set ttl"),
            }
        }
        (Method::DELETE, [id]) => match store.revoke(id) {
            Ok(()) => no_content(),
            Err(e) => connpass_error(e, "revoke"),
        },
        _ => not_found(),
    }
}

// --- Desktop updates ---

#[derive(Serialize, Deserialize)]
struct DesktopManifest {
    version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    notes: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub_date: String,
    platforms: BTreeMap<String, DesktopPlatform>,
}

#[derive(Serialize, Deserialize)]
struct DesktopPlatform {
    #[serde(default)]
    signature: String,
    #[serde(default)]
    url: String,
}

fn read_manifest(dir: &std::path::Path) -> Option<DesktopManifest> {
    let data = std::fs::read(dir.join("latest.json")).ok()?;
    let m: DesktopManifest = serde_json::from_slice(&data).ok()?;
    (!m.version.is_empty() && !m.platforms.is_empty()).then_some(m)
}

/// path.Base of the URL's (decoded) path; rejects empty, "." and "/".
fn desktop_filename(raw_url: &str) -> Option<String> {
    let after_scheme = raw_url.split_once("://").map(|(_, r)| r.find('/').map(|i| &r[i..]).unwrap_or("")).unwrap_or(raw_url);
    let path = after_scheme.split(['?', '#']).next().unwrap_or("");
    let path = String::from_utf8(super::percent_decode(path)?).ok()?;
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let name = trimmed.rsplit('/').next().unwrap_or("");
    (!name.is_empty() && name != ".").then(|| name.to_string())
}

fn request_origin(req: &Req) -> String {
    let host = req.headers().get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("");
    let fwd =
        req.headers().get("x-forwarded-proto").and_then(|v| v.to_str().ok()).and_then(|v| v.split(',').next()).map(str::trim).unwrap_or("");
    let scheme = if fwd == "http" || fwd == "https" {
        fwd
    } else if host.starts_with("localhost:") || host == "localhost" || host.starts_with("127.0.0.1:") {
        "http"
    } else {
        "https"
    };
    format!("{scheme}://{host}")
}

/// Mirrored Tauri manifest with artifact URLs rewritten to this request's
/// host, so a desktop client downloads from the server it is configured for.
pub fn desktop_manifest(app: &App, req: &Req) -> Resp {
    let Some(mut m) = read_manifest(&app.cfg.updates_dir) else {
        return not_found();
    };
    let origin = request_origin(req);
    for p in m.platforms.values_mut() {
        let Some(name) = desktop_filename(&p.url) else {
            return error(StatusCode::INTERNAL_SERVER_ERROR, "invalid update manifest");
        };
        p.url = format!("{origin}/desktop/files/{}", path_escape(&name));
    }
    let mut r = json(StatusCode::OK, &m);
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    r
}

pub fn desktop_download_windows(app: &App, method: &Method) -> Resp {
    let Some(m) = read_manifest(&app.cfg.updates_dir) else {
        return not_found();
    };
    let Some(p) = m.platforms.get("windows-x86_64") else {
        return not_found();
    };
    let Some(name) = desktop_filename(&p.url) else {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "invalid update manifest");
    };
    let mut r = redirect(method, &format!("/desktop/files/{}", path_escape(&name)), StatusCode::FOUND);
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    r
}

pub async fn desktop_file(app: &App, req: Req, name: &str) -> Resp {
    if name.is_empty() || name.contains('/') || name == "." || name == ".." {
        return not_found();
    }
    let path = app.cfg.updates_dir.join(name);
    if !std::fs::metadata(&path).is_ok_and(|m| m.is_file()) {
        return not_found();
    }
    let mut r = super::serve_fs(tower_http::services::ServeFile::new(&path), req).await;
    if r.status().is_success() {
        set_header(&mut r, "cache-control", "public, max-age=31536000, immutable");
        set_header(&mut r, "content-disposition", &format!("attachment; filename={}", go_quote(name)));
    }
    r
}

/// strconv.Quote for the characters that matter in header values.
pub fn go_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sfu::protocol::PeerInfo;

    #[test]
    fn call_detection() {
        let p = |chat_only| PeerInfo { chat_only, ..Default::default() };
        assert!(!call_in_progress(&[]));
        assert!(!call_in_progress(&[p(false)]));
        assert!(!call_in_progress(&[p(false), p(true), p(true)]));
        assert!(call_in_progress(&[p(false), p(true), p(false)]));
    }

    #[test]
    fn ttl_mapping() {
        assert_eq!(ttl_from_seconds(0), Ttl::Never);
        assert_eq!(ttl_from_seconds(-5), Ttl::Disabled);
        assert_eq!(ttl_from_seconds(60), Ttl::For(Duration::from_secs(60)));
        assert_eq!(ttl_from_seconds(i64::MAX), Ttl::For(MAX_CONN_PASS_TTL));
    }

    #[test]
    fn filenames() {
        assert_eq!(
            desktop_filename("https://github.com/o/r/releases/download/v1/App_1.0_x64-setup.exe").as_deref(),
            Some("App_1.0_x64-setup.exe")
        );
        assert_eq!(desktop_filename("https://x/a%20b.exe?x=1").as_deref(), Some("a b.exe"));
        assert_eq!(desktop_filename("https://x/"), None);
        assert_eq!(go_quote("a\"b"), "\"a\\\"b\"");
    }

    #[test]
    fn turn_creds_shape() {
        let (u, c) = turn_credentials("secret", "u", Duration::from_secs(60));
        assert!(u.ends_with(":u"));
        assert_eq!(c, turn::credential("secret", &u));
    }
}
