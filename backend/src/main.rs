use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto;
use hyper_util::server::graceful::GracefulShutdown;
use tokio::net::TcpListener;
use tower_http::services::ServeDir;

use voice_hub::auth::{self, connpass, limiter::AuthLimiter, secrets, session};
use voice_hub::config::{self, Config};
use voice_hub::http::handlers::frontend_version;
use voice_hub::http::server::{self, App, HEADER_READ_TIMEOUT};
use voice_hub::presence::Hub;
use voice_hub::sfu::{Room, RoomConfig, net::Mux};
use voice_hub::{filestore, log, ratelimit, turn, util};

/// Internal endpoints get their own listener so the public port can never
/// serve /internal/*. Bound on all interfaces so sibling containers reach it.
const INTERNAL_ADDR: &str = "0.0.0.0:8081";
const ROOMS: [&str; 3] = ["room1", "room2", "room3"];
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

fn fatal(msg: impl std::fmt::Display) -> ! {
    log!("{msg}");
    std::process::exit(1);
}

#[tokio::main]
async fn main() {
    let cfg = Config::load().unwrap_or_else(|e| fatal(format!("config: {e}")));
    let allow_insecure = config::env_flag("APP_ALLOW_INSECURE");
    if let Err(e) = config::validate_insecure_config(&cfg, allow_insecure) {
        fatal(format!("config: {e}"));
    }
    if allow_insecure {
        log!("WARNING: APP_ALLOW_INSECURE=1 — insecure dev mode active, do not expose publicly");
    }
    if cfg.admin_password_hash.is_empty() {
        fatal("APP_ADMIN_PASSWORD_HASH must be set (generate: cargo run --release --bin hashpass)");
    }
    if !auth::password::valid_hash(&cfg.admin_password_hash) {
        fatal("APP_ADMIN_PASSWORD_HASH is not an argon2id hash from hashpass (in a compose .env, wrap it in single quotes)");
    }
    if cfg.public_ip.is_empty() {
        fatal("PUBLIC_IP must be set (used by SFU NAT mapping and TURN relay address)");
    }
    let public_ip: IpAddr = cfg.public_ip.parse().unwrap_or_else(|_| fatal(format!("PUBLIC_IP {:?} is not an IP address", cfg.public_ip)));

    // ./data resolves to /app/data under Docker (WORKDIR /app), the volume.
    let data_dir = Path::new("./data");
    let session_secret = secrets::load_or_create(data_dir, "session.secret", 32).unwrap_or_else(|e| fatal(format!("session secret: {e}")));
    let turn_secret =
        util::hex(&secrets::load_or_create(data_dir, "turn.secret", 32).unwrap_or_else(|e| fatal(format!("turn secret: {e}"))));
    let conn_pass = connpass::Store::load(data_dir).unwrap_or_else(|e| fatal(format!("connpass store: {e}")));

    let files = filestore::Store::new(filestore::Config {
        temp_dir: cfg.upload_temp_dir.clone(),
        ttl: cfg.upload_ttl,
        ttl_hard_cap: cfg.upload_ttl_hard_cap,
        max_upload_bytes: cfg.upload_max_bytes,
        max_total_bytes: cfg.upload_total_bytes,
        janitor_interval: Duration::from_secs(30),
    })
    .unwrap_or_else(|e| fatal(format!("file store: {e}")));

    // Rotating APP_ADMIN_PASSWORD_HASH (restart) invalidates old admin cookies.
    let admin_ver = session::admin_password_version(&session_secret, &cfg.admin_password_hash);

    // Literal IP, not the hostname: it may proxy through a CDN that drops
    // UDP. The IP only reaches clients via the auth-gated /api/config.
    let stun_url = format!("stun:{}:3478", cfg.public_ip);
    let turn_url = format!("turn:{}:3478?transport=udp", cfg.public_ip);

    let mux = Mux::bind(public_ip, cfg.udp_port_min, cfg.udp_port_max).await.unwrap_or_else(|e| fatal(format!("sfu init: {e}")));
    let presence = Arc::new(Hub::default());
    let rooms: BTreeMap<String, Arc<Room>> = ROOMS
        .iter()
        .map(|slug| {
            let room = Room::new(RoomConfig {
                room_id: slug.to_string(),
                app_hostname: cfg.app_hostname.clone(),
                mux: mux.clone(),
                presence: Some(presence.clone()),
                files: Some(files.clone()),
            });
            (slug.to_string(), room)
        })
        .collect();
    {
        let rooms = rooms.clone();
        presence.set_rooms(Box::new(move || rooms.iter().map(|(slug, r)| (slug.clone(), r.peers())).collect()));
    }

    let turn_server = turn::start(turn::Config {
        realm: cfg.turn_realm.clone(),
        shared_secret: turn_secret.clone(),
        public_ip: cfg.public_ip.clone(),
        listen_addr: "0.0.0.0:3478".into(),
        min_relay_port: cfg.turn_relay_min,
        max_relay_port: cfg.turn_relay_max,
    })
    .await
    .unwrap_or_else(|e| fatal(format!("turn init: {e}")));

    let version = frontend_version(&cfg.web_dir);
    log!("frontend version: {version}");

    let app = Arc::new(App {
        static_files: ServeDir::new(&cfg.web_dir),
        // Global per-IP backstop over /api, above the per-login limiter.
        api_limiter: ratelimit::Limiter::new(ratelimit::Config::new(5.0, 60)),
        limiter: AuthLimiter::new(10, Duration::from_secs(15 * 60)),
        ws_registry: Default::default(),
        session_secret,
        turn_secret,
        admin_ver,
        version,
        conn_pass,
        rooms,
        presence,
        files: files.clone(),
        stun_url,
        turn_url,
        cfg: cfg.clone(),
    });

    log!("auth enabled (cookie_secure={}, connpass_entries={})", cfg.cookie_secure, app.conn_pass.status().entries.len());
    log!("listening on {}, serving web from {}", cfg.addr, cfg.web_dir.display());

    let public = TcpListener::bind(config::listen_addr(&cfg.addr)).await.unwrap_or_else(|e| fatal(format!("http server: {e}")));
    let internal = TcpListener::bind(INTERNAL_ADDR).await.unwrap_or_else(|e| fatal(format!("http server: {e}")));
    let graceful = GracefulShutdown::new();
    let stop = shutdown_signal();
    tokio::pin!(stop);

    loop {
        tokio::select! {
            sig = &mut stop => {
                log!("shutdown: received {sig}, draining...");
                break;
            }
            Ok((tcp, peer)) = public.accept() => {
                let app = app.clone();
                let svc = service_fn(move |req| {
                    let app = app.clone();
                    async move { Ok::<_, std::convert::Infallible>(server::handle(app, voice_hub::http::boxed_request(req), peer).await) }
                });
                serve_conn(&graceful, tcp, svc);
            }
            Ok((tcp, _)) = internal.accept() => {
                let app = app.clone();
                let svc = service_fn(move |req| {
                    let app = app.clone();
                    async move { Ok::<_, std::convert::Infallible>(server::handle_internal(app, voice_hub::http::boxed_request(req)).await) }
                });
                serve_conn(&graceful, tcp, svc);
            }
        }
    }

    // Rooms first: open WebSockets would otherwise hold the drain open.
    for room in app.rooms.values() {
        room.close();
    }
    app.ws_registry.disconnect_all();
    if tokio::time::timeout(SHUTDOWN_GRACE, graceful.shutdown()).await.is_err() {
        log!("shutdown: http: drain timed out");
    }
    if let Err(e) = turn_server.close().await {
        log!("shutdown: turn: {e}");
    }
    files.close();
    log!("shutdown: done");
}

fn serve_conn<S>(graceful: &GracefulShutdown, tcp: tokio::net::TcpStream, svc: S)
where
    S: hyper::service::Service<http::Request<hyper::body::Incoming>, Response = voice_hub::http::Resp, Error = std::convert::Infallible>
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    let mut builder = auto::Builder::new(TokioExecutor::new());
    builder.http1().timer(TokioTimer::new()).header_read_timeout(HEADER_READ_TIMEOUT);
    let conn = builder.serve_connection_with_upgrades(TokioIo::new(tcp), svc).into_owned();
    let fut = graceful.watch(conn);
    tokio::spawn(async move {
        let _ = fut.await;
    });
}

async fn shutdown_signal() -> &'static str {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
    let mut int = signal(SignalKind::interrupt()).expect("SIGINT handler");
    tokio::select! {
        _ = term.recv() => "terminated",
        _ = int.recv() => "interrupt",
    }
}
