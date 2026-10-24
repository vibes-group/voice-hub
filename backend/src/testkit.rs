//! Test fixtures: a fully wired App over temp dirs, servable on a real port.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::{Method, Request, header};
use tower_http::services::ServeDir;

use crate::auth::connpass::{self, Ttl};
use crate::auth::limiter::AuthLimiter;
use crate::auth::{Role, WsRegistry, password, session};
use crate::config::Config;
use crate::filestore;
use crate::http::server::{self, App};
use crate::http::{Resp, full};
use crate::presence::Hub;
use crate::ratelimit;
use crate::sfu::{Room, RoomConfig, net::Mux};
use crate::util::{hex, random_bytes};

pub const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";
pub const ADMIN_PASS: &str = "admin-password-for-tests";

pub struct Env {
    pub app: Arc<App>,
    pub dir: PathBuf,
    pub user_pass: String,
    pub user_entry: String,
}

impl Drop for Env {
    fn drop(&mut self) {
        self.app.files.close();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn admin_hash() -> &'static str {
    static H: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    H.get_or_init(|| password::hash(ADMIN_PASS))
}

pub async fn env_with(files_cfg: impl FnOnce(PathBuf) -> filestore::Config) -> Env {
    let dir = std::env::temp_dir().join(format!("vh-http-{}", hex(&random_bytes::<8>())));
    std::fs::create_dir_all(dir.join("web")).unwrap();
    std::fs::create_dir_all(dir.join("updates")).unwrap();
    std::fs::write(dir.join("web/index.html"), "<html>app</html>").unwrap();
    std::fs::write(dir.join("web/login.html"), "<html>login</html>").unwrap();
    let mut cfg = Config::load().unwrap();
    cfg.web_dir = dir.join("web");
    cfg.updates_dir = dir.join("updates");
    cfg.admin_password_hash = admin_hash().to_string();
    cfg.cookie_secure = false;
    cfg.app_hostname = "vh.example".into();
    let conn_pass = connpass::Store::load(&dir.join("data")).unwrap();
    let (entry, user_pass) = conn_pass.create("friends", Ttl::Never).unwrap();
    let files = filestore::Store::new(files_cfg(dir.join("tmp"))).unwrap();
    let mux = Mux::bind("127.0.0.1".parse().unwrap(), 0, 0).await.unwrap();
    let presence = Arc::new(Hub::default());
    let rooms: BTreeMap<String, Arc<Room>> = ["room1", "room2"]
        .iter()
        .map(|s| {
            let r = Room::new(RoomConfig {
                room_id: s.to_string(),
                app_hostname: cfg.app_hostname.clone(),
                mux: mux.clone(),
                presence: presence.clone(),
                files: files.clone(),
            });
            (s.to_string(), r)
        })
        .collect();
    let app = Arc::new(App {
        static_files: ServeDir::new(&cfg.web_dir),
        api_limiter: ratelimit::Limiter::new(ratelimit::Config::new(5.0, 60)),
        limiter: AuthLimiter::new(10, Duration::from_secs(900)),
        ws_registry: WsRegistry::default(),
        session_secret: SECRET.to_vec(),
        turn_secret: "turn".into(),
        admin_ver: session::admin_password_version(SECRET, admin_hash()),
        version: "v1".into(),
        conn_pass,
        rooms,
        presence,
        files,
        stun_url: "stun:1.2.3.4:3478".into(),
        turn_url: "turn:1.2.3.4:3478?transport=udp".into(),
        cfg,
    });
    Env { app, dir, user_pass, user_entry: entry.id }
}

pub async fn env() -> Env {
    env_with(|tmp| filestore::Config {
        temp_dir: tmp,
        ttl: Duration::from_secs(300),
        ttl_hard_cap: Duration::from_secs(1800),
        max_upload_bytes: 1 << 20,
        max_total_bytes: 1 << 20,
        janitor_interval: Duration::from_secs(3600),
    })
    .await
}

impl Env {
    pub fn admin_cookie(&self) -> String {
        let v = session::encode(SECRET, Role::Admin, 0, &self.app.admin_ver, "", Duration::from_secs(3600));
        format!("{}={v}", session::COOKIE_NAME)
    }

    pub fn user_cookie(&self, generation: u64) -> String {
        let v = session::encode(SECRET, Role::User, generation, "", &self.user_entry, Duration::from_secs(3600));
        format!("{}={v}", session::COOKIE_NAME)
    }

    pub async fn call(&self, method: Method, uri: &str, cookie: Option<&str>, headers: &[(&str, &str)], body: impl Into<Bytes>) -> Resp {
        let mut b = Request::builder().method(method).uri(uri).header(header::HOST, "vh.example");
        if let Some(c) = cookie {
            b = b.header(header::COOKIE, c);
        }
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        let peer: SocketAddr = "127.0.0.1:40000".parse().unwrap();
        server::handle(self.app.clone(), b.body(full(body.into())).unwrap(), peer).await
    }

    pub async fn get(&self, uri: &str, cookie: Option<&str>) -> Resp {
        self.call(Method::GET, uri, cookie, &[], Bytes::new()).await
    }
}

impl Env {
    /// Serves the app on an ephemeral port; returns the port.
    pub async fn serve(&self) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = self.app.clone();
        tokio::spawn(async move {
            loop {
                let Ok((tcp, peer)) = listener.accept().await else { return };
                let app = app.clone();
                tokio::spawn(async move {
                    let svc = hyper::service::service_fn(move |req| {
                        let app = app.clone();
                        async move { Ok::<_, std::convert::Infallible>(server::handle(app, crate::http::boxed_request(req), peer).await) }
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(hyper_util::rt::TokioIo::new(tcp), svc)
                        .with_upgrades()
                        .await;
                });
            }
        });
        port
    }
}
