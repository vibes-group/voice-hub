//! Application state, routing and middleware. Routes are method-qualified,
//! `{param}` is one path segment, and the authenticated static handler is the
//! catch-all `/`.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::{HeaderValue, Method, StatusCode, header};
use http_body::{Frame, SizeHint};
use http_body_util::BodyExt;
use tower_http::services::ServeDir;

use super::{Body, Req, Resp, client_ip, error, go_duration_ms, handlers, not_found, redirect, upload, ws};
use crate::auth::{self, Role, WsRegistry, connpass, limiter::AuthLimiter, session};
use crate::config::Config;
use crate::filestore;
use crate::presence::Hub;
use crate::ratelimit;
use crate::sfu::Room;
use crate::{log, util};

pub struct App {
    pub cfg: Config,
    pub session_secret: Vec<u8>,
    pub turn_secret: String,
    pub admin_ver: String,
    pub version: String,
    pub conn_pass: connpass::Store,
    pub ws_registry: WsRegistry,
    pub limiter: AuthLimiter,
    pub api_limiter: ratelimit::Limiter,
    pub rooms: BTreeMap<String, Arc<Room>>,
    pub presence: Arc<Hub>,
    pub files: Arc<filestore::Store>,
    pub stun_url: String,
    pub turn_url: String,
    pub static_files: ServeDir,
}

impl App {
    pub fn authenticated(&self, headers: &http::HeaderMap) -> bool {
        auth::authenticated(&self.session_secret, &self.conn_pass, &self.admin_ver, headers)
    }

    pub fn is_admin(&self, headers: &http::HeaderMap) -> bool {
        session::from_headers(&self.session_secret, headers)
            .is_some_and(|s| s.role == Role::Admin && auth::password::constant_time_str_eq(&s.admin_version, &self.admin_ver))
    }
}

/// Entry point for every public request: access log, then the /api rate
/// limit, then routing.
pub async fn handle(app: Arc<App>, req: Req, peer: SocketAddr) -> Resp {
    let path = req.uri().path().to_string();
    // Health-check spam and long-lived WebSockets are not logged.
    if path == "/healthz" || path.starts_with("/ws/") {
        return route(app, req, peer).await;
    }
    let start = Instant::now();
    let method = req.method().clone();
    let ip = client_ip(peer, req.headers(), &app.cfg.trusted_proxies);
    let resp = if path.starts_with("/api/") {
        match app.api_limiter.allow(&ip) {
            Ok(()) => route(app, req, peer).await,
            Err(retry) => {
                let mut r = error(StatusCode::TOO_MANY_REQUESTS, "rate limited");
                if !retry.is_zero() {
                    r.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from(util::ceil_secs(retry).max(1)));
                }
                r
            }
        }
    } else {
        route(app, req, peer).await
    };
    // Logged once the body is fully written (or dropped), so streamed
    // responses report their real duration.
    let status = resp.status().as_u16();
    resp.map(|body| LoggedBody { inner: body, line: Some(AccessLine { method, path, status, start, ip }) }.boxed_unsync())
}

struct LoggedBody {
    inner: Body,
    line: Option<AccessLine>,
}

struct AccessLine {
    method: Method,
    path: String,
    status: u16,
    start: Instant,
    ip: String,
}

impl http_body::Body for LoggedBody {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, std::io::Error>>> {
        Pin::new(&mut self.inner).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

impl Drop for LoggedBody {
    fn drop(&mut self) {
        if let Some(l) = self.line.take() {
            log!("http {} {:?} {} {} ip={:?}", l.method, l.path, l.status, go_duration_ms(l.start.elapsed()), l.ip);
        }
    }
}

async fn route(app: Arc<App>, mut req: Req, peer: SocketAddr) -> Resp {
    let path = req.uri().path().to_string();
    let cleaned = super::clean_path(&path);
    if cleaned != path && req.method() != Method::CONNECT {
        let loc = match req.uri().query() {
            Some(q) => format!("{cleaned}?{q}"),
            None => cleaned,
        };
        return redirect(req.method(), &loc, StatusCode::MOVED_PERMANENTLY);
    }
    let get = req.method() == Method::GET || req.method() == Method::HEAD;
    let post = req.method() == Method::POST;
    let segs: Vec<&str> = path[1..].split('/').collect();
    let param =
        |s: &str| -> String { super::percent_decode(s).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_else(|| s.to_string()) };

    match segs.as_slice() {
        ["desktop", "latest.json"] if get => return handlers::desktop_manifest(&app, &req),
        ["desktop", "download", "windows"] if get => return handlers::desktop_download_windows(&app, req.method()),
        ["desktop", "files", name] if get && !name.is_empty() => {
            let name = param(name);
            return handlers::desktop_file(&app, req, &name).await;
        }
        ["healthz"] if get => return handlers::health(),
        ["api", "version"] if get => return handlers::version(&app),
        ["api", "login"] if post => return handlers::login(&app, req, peer).await,
        ["api", "logout"] if post => return handlers::logout(&app),
        ["api", "presence"] if get => {
            if !app.authenticated(req.headers()) {
                return error(StatusCode::UNAUTHORIZED, "unauthorized");
            }
            return handlers::presence(&app, &req);
        }
        ["ws", room_id] if get && !room_id.is_empty() => {
            if !app.authenticated(req.headers()) {
                return error(StatusCode::UNAUTHORIZED, "unauthorized");
            }
            let Some(sess) = session::from_headers(&app.session_secret, req.headers()) else {
                return error(StatusCode::UNAUTHORIZED, "unauthorized");
            };
            let Some(room) = app.rooms.get(&param(room_id)).cloned() else {
                return not_found();
            };
            if room.is_closed() {
                return error(StatusCode::SERVICE_UNAVAILABLE, "shutting down");
            }
            let (resp, on_upgrade) = match ws::accept(&mut req, &room.origin_patterns()) {
                Ok(v) => v,
                Err(r) => {
                    log!("sfu: ws accept: rejected handshake ({})", r.status());
                    return *r;
                }
            };
            let cancel = tokio_util::sync::CancellationToken::new();
            let app2 = app.clone();
            tokio::spawn(async move {
                let Some(socket) = ws::upgraded(on_upgrade).await else { return };
                let id = app2.ws_registry.add(sess.role, cancel.clone());
                room.serve_ws(socket, cancel).await;
                app2.ws_registry.remove(id);
            });
            return resp;
        }
        ["api", "config"] if get => {
            if !app.authenticated(req.headers()) {
                return error(StatusCode::UNAUTHORIZED, "unauthorized");
            }
            return handlers::app_config(&app, &req);
        }
        ["api", "upload"] if post => {
            if !app.authenticated(req.headers()) {
                return error(StatusCode::UNAUTHORIZED, "unauthorized");
            }
            return upload::upload(&app, req).await;
        }
        ["api", "file", upload_id] if get && !upload_id.is_empty() => {
            if !app.authenticated(req.headers()) {
                return error(StatusCode::UNAUTHORIZED, "unauthorized");
            }
            let id = param(upload_id);
            return upload::download(&app, req, &id).await;
        }
        ["api", "admin", "connection-passwords", rest @ ..] => {
            let m = req.method().clone();
            let admin_route = match rest {
                [] => get || post,
                ["disconnect-users"] => post,
                [id, "rotate" | "rename" | "ttl"] => post && !id.is_empty(),
                [id] => m == Method::DELETE && !id.is_empty(),
                _ => false,
            };
            if admin_route {
                if !app.is_admin(req.headers()) {
                    return error(StatusCode::FORBIDDEN, "forbidden");
                }
                let rest: Vec<String> = rest.iter().map(|s| param(s)).collect();
                return handlers::admin(&app, req, &rest).await;
            }
        }
        _ => {}
    }
    let mut r = static_files(&app, req).await;
    r.headers_mut().insert("permissions-policy", HeaderValue::from_static("geolocation=(), payment=()"));
    r
}

/// Authenticated static frontend. Public assets (login page, icons, PWA
/// files, Vite bundles) pass without a cookie; anything else redirects to
/// the login page.
async fn static_files(app: &App, req: Req) -> Resp {
    let path = req.uri().path();
    // connect.html is Tauri-only; hide it from browsers.
    if path == "/connect.html" {
        return not_found();
    }
    let public = matches!(
        path,
        "/login.html"
            | "/login.js"
            | "/favicon.ico"
            | "/favicon.svg"
            | "/favicon.png"
            | "/apple-touch-icon.png"
            | "/manifest.webmanifest"
            | "/sw.js"
    ) || path.starts_with("/assets/")
        || path.starts_with("/vendor/");
    if !public && !app.authenticated(req.headers()) {
        return redirect(req.method(), "/login.html", StatusCode::SEE_OTHER);
    }
    // http.FileServer canonicalises /index.html to its directory.
    if path.ends_with("/index.html") {
        return redirect(req.method(), "./", StatusCode::MOVED_PERMANENTLY);
    }
    super::serve_fs(app.static_files.clone(), req).await
}

/// Internal listener (:8081): reachable by sibling containers only, never
/// routed through the public proxy.
pub async fn handle_internal(app: Arc<App>, req: Req) -> Resp {
    if req.uri().path() == "/internal/call-status" && (req.method() == Method::GET || req.method() == Method::HEAD) {
        return handlers::call_status(&app);
    }
    not_found()
}

pub const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);
