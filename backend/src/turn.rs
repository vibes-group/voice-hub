//! Embedded STUN+TURN server (webrtc-rs `turn`, a port of pion/turn).
//! Authentication uses the TURN REST API HMAC scheme, so short-term
//! credentials minted by /api/config are validated from one shared secret.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use hmac::{Hmac, KeyInit, Mac};
use sha1::Sha1;
use turn::auth::{AuthHandler, generate_auth_key};
use turn::relay::relay_range::RelayAddressGeneratorRanges;
use turn::server::Server;
use turn::server::config::{ConnConfig, ServerConfig};
use webrtc_util::vnet::net::Net;

use crate::log;
use crate::util::{b64_std, unix_secs};

pub struct Config {
    /// Usually the public hostname.
    pub realm: String,
    pub shared_secret: String,
    /// Address advertised in relay candidates.
    pub public_ip: String,
    pub listen_addr: String,
    pub min_relay_port: u16,
    pub max_relay_port: u16,
}

/// A long call must not be torn down at a library-default 10-minute mark.
const CHANNEL_BIND_TIMEOUT: Duration = Duration::from_secs(8 * 3600);

/// base64(HMAC-SHA1(secret, username)).
pub fn credential(secret: &str, username: &str) -> String {
    let mut mac = Hmac::<Sha1>::new_from_slice(secret.as_bytes()).expect("any key length");
    mac.update(username.as_bytes());
    b64_std(&mac.finalize().into_bytes())
}

/// Accepts `<unix_expiry>:<id>` usernames that have not expired.
struct RestAuth {
    secret: String,
}

impl AuthHandler for RestAuth {
    fn auth_handle(&self, username: &str, realm: &str, src: SocketAddr) -> Result<Vec<u8>, turn::Error> {
        let expiry = username.split(':').next().and_then(|t| t.parse::<i64>().ok());
        let now = unix_secs(SystemTime::now());
        match expiry {
            Some(t) if t >= now => Ok(generate_auth_key(username, realm, &credential(&self.secret, username))),
            _ => {
                log!("turn: rejected credentials username={username:?} src={src}");
                Err(turn::Error::Other(format!("expired or malformed username {username}")))
            }
        }
    }
}

pub async fn start(cfg: Config) -> Result<Server, String> {
    if cfg.shared_secret.is_empty() {
        return Err("turn: shared secret required".into());
    }
    let relay_ip: IpAddr = cfg.public_ip.parse().map_err(|_| "turn: public IP required".to_string())?;
    if cfg.min_relay_port == 0 || cfg.max_relay_port < cfg.min_relay_port {
        return Err(format!("turn: relay port range invalid ({}-{})", cfg.min_relay_port, cfg.max_relay_port));
    }
    let socket = tokio::net::UdpSocket::bind(&cfg.listen_addr).await.map_err(|e| format!("turn: listen {}: {e}", cfg.listen_addr))?;
    let server = Server::new(ServerConfig {
        conn_configs: vec![ConnConfig {
            conn: Arc::new(socket),
            relay_addr_generator: Box::new(RelayAddressGeneratorRanges {
                relay_address: relay_ip,
                min_port: cfg.min_relay_port,
                max_port: cfg.max_relay_port,
                max_retries: 0,
                address: "0.0.0.0".into(),
                net: Arc::new(Net::new(None)),
            }),
        }],
        realm: cfg.realm.clone(),
        auth_handler: Arc::new(RestAuth { secret: cfg.shared_secret }),
        channel_bind_timeout: CHANNEL_BIND_TIMEOUT,
        alloc_close_notify: None,
    })
    .await
    .map_err(|e| format!("turn: server: {e}"))?;
    log!(
        "turn: listening on {}, relay {}:{}-{}, realm={}",
        cfg.listen_addr,
        cfg.public_ip,
        cfg.min_relay_port,
        cfg.max_relay_port,
        cfg.realm
    );
    Ok(server)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_matches_go() {
        // Go: base64(HMAC-SHA1("secret", "1700000000:u")).
        assert_eq!(credential("secret", "1700000000:u"), "XG6B680Mf7Xc6AI7xZ5TSz0EUb4=");
    }

    #[test]
    fn auth_expiry() {
        let a = RestAuth { secret: "s".into() };
        let src: SocketAddr = "1.2.3.4:5".parse().unwrap();
        assert!(a.auth_handle("1:u", "r", src).is_err());
        assert!(a.auth_handle("bogus", "r", src).is_err());
        assert!(a.auth_handle("99999999999:u", "r", src).is_ok());
    }
}
