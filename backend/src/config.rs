use std::env;
use std::net::IpAddr;
use std::path::PathBuf;

use crate::auth;
use crate::log;

/// CIDR prefix, e.g. `10.0.0.0/8`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prefix {
    addr: IpAddr,
    bits: u8,
}

impl Prefix {
    pub fn parse(s: &str) -> Option<Prefix> {
        let (ip, bits) = s.split_once('/')?;
        let addr: IpAddr = ip.parse().ok()?;
        // Go's netip.ParsePrefix rejects leading zeros and signs in the bit count.
        if bits.is_empty() || (bits.len() > 1 && bits.starts_with('0')) || !bits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let bits: u8 = bits.parse().ok()?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        (bits <= max).then_some(Prefix { addr, bits })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = if self.bits == 0 { 0 } else { u32::MAX << (32 - self.bits) };
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = if self.bits == 0 { 0 } else { u128::MAX << (128 - self.bits) };
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

impl std::fmt::Display for Prefix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.addr, self.bits)
    }
}

/// Parses a CSV of CIDR prefixes. Empty input yields loopback only (direct dev
/// connections); anything else, private RFC1918 ranges included, must be
/// opted in. A malformed entry is an error — silently dropping it would let a
/// typo bypass the trust check.
pub fn parse_trusted_proxies(csv: &str) -> Result<Vec<Prefix>, String> {
    let mut out = Vec::new();
    for raw in csv.trim().split(',') {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        out.push(Prefix::parse(raw).ok_or_else(|| format!("APP_TRUSTED_PROXIES: {raw:?}: invalid CIDR prefix"))?);
    }
    if out.is_empty() {
        return Ok(vec![Prefix::parse("127.0.0.0/8").unwrap(), Prefix::parse("::1/128").unwrap()]);
    }
    Ok(out)
}

#[derive(Clone, Debug)]
pub struct Config {
    pub addr: String,
    pub web_dir: PathBuf,
    pub updates_dir: PathBuf,
    /// Also the TURN realm.
    pub app_hostname: String,
    pub public_ip: String,
    /// argon2id hash from `hashpass`; the plaintext never reaches the host.
    pub admin_password_hash: String,
    pub cookie_secure: bool,
    pub udp_port_min: u16,
    pub udp_port_max: u16,
    pub turn_relay_min: u16,
    pub turn_relay_max: u16,
    /// CIDR prefixes whose peer address may set X-Forwarded-For.
    pub trusted_proxies: Vec<Prefix>,
}

impl Config {
    pub fn load() -> Result<Config, String> {
        let trusted = parse_trusted_proxies(&env::var("APP_TRUSTED_PROXIES").unwrap_or_default())?;
        Ok(Config {
            addr: env_or("APP_ADDR", ":8080"),
            web_dir: env_or("APP_WEB_DIR", "../frontend/dist").into(),
            updates_dir: env_or("APP_UPDATES_DIR", "../updates").into(),
            app_hostname: env_or("APP_HOSTNAME", "localhost"),
            public_ip: env::var("PUBLIC_IP").unwrap_or_default(),
            admin_password_hash: env::var("APP_ADMIN_PASSWORD_HASH").unwrap_or_default(),
            cookie_secure: env_bool("APP_COOKIE_SECURE", true),
            udp_port_min: env_u16("UDP_PORT_MIN", 10101),
            udp_port_max: env_u16("UDP_PORT_MAX", 10200),
            turn_relay_min: env_u16("TURN_RELAY_PORT_MIN", 49160),
            turn_relay_max: env_u16("TURN_RELAY_PORT_MAX", 49199),
            trusted_proxies: trusted,
        })
    }
}

/// Refuses combinations only acceptable in a local dev container, so a pure
/// env mistake can't silently produce an insecure public deployment.
pub fn validate_insecure_config(cfg: &Config, allow_insecure: bool) -> Result<(), String> {
    if !cfg.cookie_secure && !allow_insecure {
        return Err("APP_COOKIE_SECURE=false requires APP_ALLOW_INSECURE=1".into());
    }
    if !allow_insecure && auth::password::verify(&cfg.admin_password_hash, "dev") {
        return Err("dev admin password requires APP_ALLOW_INSECURE=1".into());
    }
    Ok(())
}

/// Go's `net.Listen(":8080")` form: an empty host means all interfaces.
pub fn listen_addr(addr: &str) -> String {
    if addr.starts_with(':') { format!("0.0.0.0{addr}") } else { addr.to_string() }
}

fn env_or(key: &str, fallback: &str) -> String {
    match env::var(key) {
        Ok(v) if !v.is_empty() => v,
        _ => fallback.to_string(),
    }
}

/// strconv.ParseBool semantics.
fn parse_bool(v: &str) -> Option<bool> {
    match v {
        "1" | "t" | "T" | "TRUE" | "true" | "True" => Some(true),
        "0" | "f" | "F" | "FALSE" | "false" | "False" => Some(false),
        _ => None,
    }
}

pub fn env_flag(key: &str) -> bool {
    env::var(key).ok().and_then(|v| parse_bool(&v)).unwrap_or(false)
}

fn env_bool(key: &str, fallback: bool) -> bool {
    env_parsed(key, fallback, parse_bool)
}

fn env_u16(key: &str, fallback: u16) -> u16 {
    env_parsed(key, fallback, |v| v.parse().ok())
}

fn env_parsed<T: std::fmt::Display>(key: &str, fallback: T, parse: impl Fn(&str) -> Option<T>) -> T {
    let v = env::var(key).unwrap_or_default();
    if v.is_empty() {
        return fallback;
    }
    parse(&v).unwrap_or_else(|| {
        log!("config: bad {key}={v:?}, using default {fallback}");
        fallback
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusted_proxies() {
        let d = parse_trusted_proxies("").unwrap();
        assert_eq!(d.iter().map(ToString::to_string).collect::<Vec<_>>(), ["127.0.0.0/8", "::1/128"]);
        assert_eq!(parse_trusted_proxies("10.0.0.0/8").unwrap()[0].to_string(), "10.0.0.0/8");
        assert_eq!(parse_trusted_proxies("10.0.0.0/8 , 192.168.0.0/16,fd00::/8").unwrap().len(), 3);
        assert!(parse_trusted_proxies("10.0.0.0/8,notacidr").is_err());
        assert!(parse_trusted_proxies("10.0.0.5").is_err());
    }

    #[test]
    fn prefix_contains() {
        let p = Prefix::parse("10.0.0.0/8").unwrap();
        assert!(p.contains("10.1.2.3".parse().unwrap()));
        assert!(!p.contains("11.1.2.3".parse().unwrap()));
        assert!(!p.contains("::1".parse().unwrap()));
    }

    #[test]
    fn insecure_config_guard() {
        let real = auth::password::hash("real-password");
        let dev = auth::password::hash("dev");
        let cfg =
            |secure: bool, hash: &str| Config { cookie_secure: secure, admin_password_hash: hash.to_string(), ..Config::load().unwrap() };
        assert!(validate_insecure_config(&cfg(true, &real), false).is_ok());
        assert!(validate_insecure_config(&cfg(false, &real), false).is_err());
        assert!(validate_insecure_config(&cfg(false, &real), true).is_ok());
        assert!(validate_insecure_config(&cfg(true, &dev), false).is_err());
        assert!(validate_insecure_config(&cfg(true, &dev), true).is_ok());
        assert!(validate_insecure_config(&cfg(false, &dev), true).is_ok());
    }
}
