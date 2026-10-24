//! Signed session cookie: `<exp>:<role>:<gen>:<adminVer>:<entryID>.<sig>`.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use super::password::constant_time_str_eq;
use crate::util::b64_raw_url;

pub const COOKIE_NAME: &str = "vh_session";
pub const SESSION_TTL: Duration = Duration::from_secs(30 * 24 * 3600);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Admin,
    User,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::User => "user",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub role: Role,
    /// ConnPass entry generation at issue time; user sessions die when it drifts.
    pub generation: u64,
    /// Admin-password fingerprint at issue time; empty for user sessions.
    pub admin_version: String,
    /// ConnPass entry id at issue time; empty for admin sessions.
    pub entry_id: String,
}

fn sign(secret: &[u8], payload: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("any key length");
    mac.update(payload.as_bytes());
    b64_raw_url(&mac.finalize().into_bytes())
}

/// base64url HMAC fingerprint of the admin password hash; the hash itself is
/// never stored in the cookie.
pub fn admin_password_version(secret: &[u8], password_hash: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("any key length");
    mac.update(b"admin-password:v2:");
    mac.update(password_hash.as_bytes());
    b64_raw_url(&mac.finalize().into_bytes())
}

pub fn encode(secret: &[u8], role: Role, gen_: u64, admin_ver: &str, entry_id: &str, ttl: Duration) -> String {
    encode_at(secret, role, gen_, admin_ver, entry_id, unix(SystemTime::now() + ttl))
}

/// Encodes with an absolute expiry; also lets tests mint already-expired cookies.
pub fn encode_at(secret: &[u8], role: Role, gen_: u64, admin_ver: &str, entry_id: &str, exp_unix: i64) -> String {
    let payload = format!("{exp_unix}:{}:{gen_}:{admin_ver}:{entry_id}", role.as_str());
    let sig = sign(secret, &payload);
    format!("{payload}.{sig}")
}

fn unix(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    }
}

pub fn decode(secret: &[u8], value: &str) -> Option<Session> {
    let (payload, sig) = value.rsplit_once('.')?;
    if !constant_time_str_eq(sig, &sign(secret, payload)) {
        return None;
    }
    let parts: Vec<&str> = payload.splitn(5, ':').collect();
    if parts.len() != 5 {
        return None;
    }
    let exp: i64 = parts[0].parse().ok()?;
    let role = match parts[1] {
        "admin" => Role::Admin,
        "user" => Role::User,
        _ => return None,
    };
    let generation: u64 = parts[2].parse().ok()?;
    if unix(SystemTime::now()) > exp {
        return None;
    }
    Some(Session { role, generation, admin_version: parts[3].to_string(), entry_id: parts[4].to_string() })
}

/// `Set-Cookie` value for a fresh session.
pub fn session_cookie(secure: bool, secret: &[u8], role: Role, gen_: u64, admin_ver: &str, entry_id: &str) -> String {
    let value = encode(secret, role, gen_, admin_ver, entry_id, SESSION_TTL);
    cookie_header(&value, SESSION_TTL.as_secs() as i64, secure)
}

/// `Set-Cookie` that clears the session.
pub fn clear_cookie(secure: bool) -> String {
    cookie_header("", -1, secure)
}

fn cookie_header(value: &str, max_age: i64, secure: bool) -> String {
    let mut s = format!("{COOKIE_NAME}={value}; Path=/");
    if max_age > 0 {
        let expires = SystemTime::now() + Duration::from_secs(max_age as u64);
        s.push_str(&format!("; Expires={}", http_date(expires)));
        s.push_str(&format!("; Max-Age={max_age}"));
    } else if max_age < 0 {
        s.push_str("; Max-Age=0");
    }
    s.push_str("; HttpOnly");
    if secure {
        s.push_str("; Secure");
    }
    s.push_str("; SameSite=Strict");
    s
}

fn http_date(t: SystemTime) -> String {
    let dt = time::OffsetDateTime::from(t);
    let fmt = time::macros::format_description!("[weekday repr:short], [day] [month repr:short] [year] [hour]:[minute]:[second] GMT");
    dt.format(&fmt).expect("formattable")
}

/// Extracts a cookie value from a raw `Cookie` header list.
pub fn cookie_value<'a>(headers: &'a http::HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(http::header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.trim_matches('"'))
}

pub fn from_headers(secret: &[u8], headers: &http::HeaderMap) -> Option<Session> {
    decode(secret, cookie_value(headers, COOKIE_NAME)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";

    #[test]
    fn round_trip() {
        for role in [Role::Admin, Role::User] {
            let token = encode(SECRET, role, 7, "av-xyz", "entry-1", Duration::from_secs(3600));
            let s = decode(SECRET, &token).unwrap();
            assert_eq!(s.role, role);
            assert_eq!(s.generation, 7);
            assert_eq!(s.admin_version, "av-xyz");
            assert_eq!(s.entry_id, "entry-1");
        }
    }

    #[test]
    fn admin_version_deterministic() {
        let a = admin_password_version(SECRET, "hunter2");
        assert_eq!(a, admin_password_version(SECRET, "hunter2"));
        assert_ne!(a, admin_password_version(SECRET, "hunter3"));
        assert_ne!(a, admin_password_version(b"ffffffffffffffffffffffffffffffff", "hunter2"));
        assert!(!a.is_empty());
    }

    #[test]
    fn rejects_tampering_and_expiry() {
        let token = encode(SECRET, Role::User, 0, "", "", Duration::from_secs(3600));
        assert!(decode(SECRET, &token.replacen(":user:", ":admin:", 1)).is_none());
        let token = encode(SECRET, Role::User, 1, "", "", Duration::from_secs(3600));
        assert!(decode(SECRET, &token.replacen(":1:", ":99:", 1)).is_none());
        let token = encode(SECRET, Role::Admin, 0, "", "", Duration::from_secs(3600));
        assert!(decode(b"ffffffffffffffffffffffffffffffff", &token).is_none());
        let expired = encode_at(SECRET, Role::Admin, 0, "", "", unix(SystemTime::now()) - 1);
        assert!(decode(SECRET, &expired).is_none());
        let payload = "9999999999:root:0:";
        assert!(decode(SECRET, &format!("{payload}.{}", sign(SECRET, payload))).is_none());
    }

    #[test]
    fn cookie_is_strict() {
        let c = session_cookie(true, SECRET, Role::User, 0, "", "");
        assert!(c.contains("SameSite=Strict") && c.contains("HttpOnly") && c.contains("Secure"));
    }

    #[test]
    fn decodes_go_cookie_format() {
        // Format and signature scheme are unchanged from the Go backend.
        let payload = "9999999999:admin:0:av:";
        let token = format!("{payload}.{}", sign(SECRET, payload));
        assert_eq!(decode(SECRET, &token).unwrap().role, Role::Admin);
    }
}
