//! Router-level tests: requests go through `server::handle` exactly as the
//! listener would send them, minus the socket.

use std::time::Duration;

use bytes::Bytes;
use http::{Method, Request, StatusCode, header};
use http_body_util::BodyExt;

use super::server;
use super::{Resp, full};
use crate::auth::connpass;
use crate::auth::{Role, session};
use crate::filestore;
use crate::testkit::{ADMIN_PASS, SECRET, env, env_with};

async fn text(r: Resp) -> String {
    String::from_utf8(r.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap()
}

fn set_cookie(r: &Resp) -> String {
    r.headers().get(header::SET_COOKIE).unwrap().to_str().unwrap().split(';').next().unwrap().to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn admin_gate() {
    let e = env().await;
    let path = "/api/admin/connection-passwords";
    assert_eq!(e.get(path, None).await.status(), StatusCode::FORBIDDEN);
    assert_eq!(e.get(path, Some(&e.user_cookie(1))).await.status(), StatusCode::FORBIDDEN);
    let r = e.get(path, Some(&e.admin_cookie())).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert!(text(r).await.contains("\"label\":\"friends\""));
    let stale = format!("{}={}", session::COOKIE_NAME, session::encode(SECRET, Role::Admin, 0, "old-version", "", Duration::from_secs(60)));
    assert_eq!(e.get(path, Some(&stale)).await.status(), StatusCode::FORBIDDEN);
    let forged = format!(
        "{}={}",
        session::COOKIE_NAME,
        session::encode(b"ffffffffffffffffffffffffffffffff", Role::Admin, 0, &e.app.admin_ver, "", Duration::from_secs(60))
    );
    assert_eq!(e.get(path, Some(&forged)).await.status(), StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread")]
async fn login_roles() {
    let e = env().await;
    let form = [(header::CONTENT_TYPE.as_str(), "application/x-www-form-urlencoded")];
    let r = e.call(Method::POST, "/api/login", None, &form, format!("password={ADMIN_PASS}")).await;
    assert_eq!(r.status(), StatusCode::NO_CONTENT);
    let admin = set_cookie(&r);
    let r = e.get("/api/config", Some(&admin)).await;
    assert!(text(r).await.contains("\"role\":\"admin\""));

    let pass = super::path_escape(&e.user_pass).replace('&', "%26").replace('+', "%2B").replace('=', "%3D");
    let r = e.call(Method::POST, "/api/login", None, &form, format!("password={pass}")).await;
    assert_eq!(r.status(), StatusCode::NO_CONTENT);
    let user = set_cookie(&r);
    let r = e.get("/api/config", Some(&user)).await;
    let body = text(r).await;
    assert!(body.contains("\"role\":\"user\""), "{body}");
    assert!(body.contains("\"urls\":[\"turn:1.2.3.4:3478?transport=udp\"],\"username\":"), "{body}");
    assert_eq!(e.get("/api/admin/connection-passwords", Some(&user)).await.status(), StatusCode::FORBIDDEN);

    let r = e.call(Method::POST, "/api/login", None, &form, "password=guess").await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(e.call(Method::POST, "/api/login", None, &form, "").await.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread")]
async fn login_lockout() {
    let e = env().await;
    let form = [(header::CONTENT_TYPE.as_str(), "application/x-www-form-urlencoded")];
    for _ in 0..10 {
        assert_eq!(e.call(Method::POST, "/api/login", None, &form, "password=x").await.status(), StatusCode::UNAUTHORIZED);
    }
    let r = e.call(Method::POST, "/api/login", None, &form, format!("password={ADMIN_PASS}")).await;
    assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(r.headers()[header::RETRY_AFTER], "900");
}

#[tokio::test(flavor = "multi_thread")]
async fn session_staleness() {
    let e = env().await;
    let user = e.user_cookie(1);
    assert_eq!(e.get("/api/config", Some(&user)).await.status(), StatusCode::OK);
    e.app.conn_pass.rotate(&e.user_entry).unwrap();
    assert_eq!(e.get("/api/config", Some(&user)).await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(e.get("/api/config", Some(&e.user_cookie(2))).await.status(), StatusCode::OK);
    assert_eq!(e.get("/api/config", Some(&e.admin_cookie())).await.status(), StatusCode::OK, "admin unaffected by rotate");
    e.app.conn_pass.revoke(&e.user_entry).unwrap();
    assert_eq!(e.get("/api/config", Some(&e.user_cookie(2))).await.status(), StatusCode::UNAUTHORIZED);
    let stale = format!("{}={}", session::COOKIE_NAME, session::encode(SECRET, Role::Admin, 0, "old", "", Duration::from_secs(60)));
    assert_eq!(e.get("/api/config", Some(&stale)).await.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread")]
async fn admin_crud() {
    let e = env().await;
    let a = e.admin_cookie();
    let json = [(header::CONTENT_TYPE.as_str(), "application/json")];
    let r = e.call(Method::POST, "/api/admin/connection-passwords", Some(&a), &json, r#"{"label":"x","ttl_seconds":60}"#).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.headers()[header::CACHE_CONTROL], "no-store");
    let v: serde_json::Value = serde_json::from_str(&text(r).await).unwrap();
    assert_eq!(v["host"], "vh.example");
    assert_eq!(v["password"].as_str().unwrap().len(), connpass::CONN_PASS_LENGTH);
    let id = v["id"].as_str().unwrap().to_string();
    let r = e.call(Method::POST, &format!("/api/admin/connection-passwords/{id}/ttl"), Some(&a), &json, r#"{"ttl_seconds":-1}"#).await;
    assert!(text(r).await.contains("\"expired\":true"));
    let r = e.call(Method::POST, &format!("/api/admin/connection-passwords/{id}/rename"), Some(&a), &json, r#"{"label":"y"}"#).await;
    assert_eq!(r.status(), StatusCode::NO_CONTENT);
    let r = e.call(Method::POST, &format!("/api/admin/connection-passwords/{id}/rotate"), Some(&a), &[], Bytes::new()).await;
    assert!(text(r).await.contains("\"generation\":2"));
    assert_eq!(
        e.call(Method::DELETE, &format!("/api/admin/connection-passwords/{id}"), Some(&a), &[], Bytes::new()).await.status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        e.call(Method::DELETE, &format!("/api/admin/connection-passwords/{id}"), Some(&a), &[], Bytes::new()).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        e.call(Method::POST, "/api/admin/connection-passwords/disconnect-users", Some(&a), &[], Bytes::new()).await.status(),
        StatusCode::NO_CONTENT
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn upload_and_download() {
    let e = env().await;
    let u = e.user_cookie(1);
    assert_eq!(e.call(Method::POST, "/api/upload?room=room1", None, &[], "x").await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(e.call(Method::POST, "/api/upload?room=nope", Some(&u), &[], "x").await.status(), StatusCode::NOT_FOUND);
    let too_big = vec![b'a'; (1 << 20) + 2];
    assert_eq!(e.call(Method::POST, "/api/upload?room=room1", Some(&u), &[], too_big).await.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let cd = [(header::CONTENT_DISPOSITION.as_str(), "attachment; filename*=UTF-8''%D1%84%D0%BE%D1%82%D0%BE.png")];
    let png = b"\x89PNG\x0D\x0A\x1A\x0A0123456789".to_vec();
    let r = e.call(Method::POST, "/api/upload?room=room1", Some(&u), &cd, png).await;
    assert_eq!(r.status(), StatusCode::CREATED);
    let v: serde_json::Value = serde_json::from_str(&text(r).await).unwrap();
    let id = v["uploadId"].as_str().unwrap().to_string();

    assert_eq!(e.get(&format!("/api/file/{id}?room=room1"), None).await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(e.get("/api/file/nope?room=room1", Some(&u)).await.status(), StatusCode::NOT_FOUND);
    assert_eq!(e.get(&format!("/api/file/{id}?room=room2"), Some(&u)).await.status(), StatusCode::NOT_FOUND, "cross-room");
    let r = e.get(&format!("/api/file/{id}?room=room1"), Some(&u)).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.headers()[header::CONTENT_TYPE], "image/png");
    assert_eq!(r.headers()[header::CONTENT_DISPOSITION], "inline; filename=\"____.png\"; filename*=UTF-8''%D1%84%D0%BE%D1%82%D0%BE.png");
    let r = e.call(Method::GET, &format!("/api/file/{id}?room=room1"), Some(&u), &[("range", "bytes=8-11")], Bytes::new()).await;
    assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(text(r).await, "0123");

    let mp = [(header::CONTENT_TYPE.as_str(), "multipart/form-data; boundary=XX")];
    let body = "--XX\r\nContent-Disposition: form-data; name=\"note\"\r\n\r\nhi\r\n--XX\r\nContent-Disposition: form-data; name=\"file\"; filename=\"../a.pdf\"\r\nContent-Type: application/pdf\r\n\r\n%PDF-1.7 body\r\n--XX--\r\n";
    let r = e.call(Method::POST, "/api/upload?room=room1", Some(&u), &mp, body).await;
    assert_eq!(r.status(), StatusCode::CREATED);
    let v: serde_json::Value = serde_json::from_str(&text(r).await).unwrap();
    let entry = e.app.files.get(v["uploadId"].as_str().unwrap()).unwrap();
    assert_eq!((entry.name.as_str(), entry.mime.as_str(), entry.size), ("a.pdf", "application/pdf", 13));
    let r = e.get(&format!("/api/file/{}?room=room1", entry.upload_id), Some(&u)).await;
    assert!(r.headers()[header::CONTENT_DISPOSITION].to_str().unwrap().starts_with("attachment"));
}

#[tokio::test(flavor = "multi_thread")]
async fn upload_budget_and_eviction() {
    let e = env_with(|tmp| filestore::Config {
        temp_dir: tmp,
        ttl: Duration::ZERO,
        ttl_hard_cap: Duration::from_secs(1800),
        max_upload_bytes: 100,
        max_total_bytes: 10,
        janitor_interval: Duration::from_secs(3600),
    })
    .await;
    let u = e.user_cookie(1);
    assert_eq!(
        e.call(Method::POST, "/api/upload?room=room1", Some(&u), &[], "x".repeat(11)).await.status(),
        StatusCode::INSUFFICIENT_STORAGE
    );
    let r = e.call(Method::POST, "/api/upload?room=room1", Some(&u), &[], "hello").await;
    let v: serde_json::Value = serde_json::from_str(&text(r).await).unwrap();
    tokio::time::sleep(Duration::from_millis(5)).await;
    assert_eq!(e.get(&format!("/api/file/{}?room=room1", v["uploadId"].as_str().unwrap()), Some(&u)).await.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread")]
async fn desktop_updates() {
    let e = env().await;
    std::fs::write(
        e.dir.join("updates/latest.json"),
        r#"{"version":"0.4.6","notes":"n","platforms":{"windows-x86_64":{"signature":"sig","url":"https://github.com/o/r/releases/download/v0.4.6/Voice%20Hub_0.4.6_x64-setup.exe"}}}"#,
    )
    .unwrap();
    std::fs::write(e.dir.join("updates/Voice Hub_0.4.6_x64-setup.exe"), "exe").unwrap();
    let r = e.call(Method::GET, "/desktop/latest.json", None, &[("x-forwarded-proto", "https")], Bytes::new()).await;
    assert_eq!(r.headers()[header::CACHE_CONTROL], "no-cache");
    assert_eq!(
        text(r).await,
        "{\"version\":\"0.4.6\",\"notes\":\"n\",\"platforms\":{\"windows-x86_64\":{\"signature\":\"sig\",\"url\":\"https://vh.example/desktop/files/Voice%20Hub_0.4.6_x64-setup.exe\"}}}\n"
    );
    let r = e.get("/desktop/download/windows", None).await;
    assert_eq!(r.status(), StatusCode::FOUND);
    assert_eq!(r.headers()[header::LOCATION], "/desktop/files/Voice%20Hub_0.4.6_x64-setup.exe");
    let r = e.get("/desktop/files/Voice%20Hub_0.4.6_x64-setup.exe", None).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.headers()[header::CONTENT_DISPOSITION], "attachment; filename=\"Voice Hub_0.4.6_x64-setup.exe\"");
    assert_eq!(text(r).await, "exe");
    assert_eq!(e.get("/desktop/files/..%2Flatest.json", None).await.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread")]
async fn static_gate_and_misc() {
    let e = env().await;
    let r = e.get("/", None).await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(r.headers()[header::LOCATION], "/login.html");
    assert_eq!(r.headers()["permissions-policy"], "geolocation=(), payment=()");
    assert_eq!(e.get("/login.html", None).await.status(), StatusCode::OK);
    assert_eq!(e.get("/connect.html", Some(&e.admin_cookie())).await.status(), StatusCode::NOT_FOUND);
    let r = e.get("/", Some(&e.user_cookie(1))).await;
    assert_eq!(text(r).await, "<html>app</html>");
    assert_eq!(e.get("/index.html", Some(&e.user_cookie(1))).await.headers()[header::LOCATION], "./");
    assert_eq!(text(e.get("/healthz", None).await).await, "{\"status\":\"ok\"}\n");
    assert_eq!(text(e.get("/api/version", None).await).await, "{\"version\":\"v1\"}\n");
    assert_eq!(e.get("/api/presence", None).await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(e.get("/ws/room1", None).await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(e.get("/ws/nope", Some(&e.user_cookie(1))).await.status(), StatusCode::NOT_FOUND);
    let r = server::handle_internal(e.app.clone(), Request::get("/internal/call-status").body(full(Bytes::new())).unwrap()).await;
    assert_eq!(text(r).await, "idle\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn api_rate_limit() {
    let e = env().await;
    let mut limited = None;
    for i in 0..80 {
        let r = e.get("/api/version", None).await;
        if r.status() == StatusCode::TOO_MANY_REQUESTS {
            limited = Some((i, r));
            break;
        }
    }
    let (i, r) = limited.expect("burst of 60 then throttled");
    assert!(i >= 60);
    assert_eq!(r.headers()[header::RETRY_AFTER], "1");
    // Non-API paths are exempt.
    assert_eq!(e.get("/healthz", None).await.status(), StatusCode::OK);
}
