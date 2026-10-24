pub mod connpass;
pub mod limiter;
pub mod password;
pub mod secrets;
pub mod session;

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio_util::sync::CancellationToken;

pub use session::{Role, Session};

/// Reports whether the request carries a valid, non-stale session. User
/// sessions die when their ConnPass entry is gone or its generation bumped;
/// admin sessions die when APP_ADMIN_PASSWORD_HASH changed.
pub fn authenticated(secret: &[u8], conn_pass: &connpass::Store, admin_ver: &str, headers: &http::HeaderMap) -> bool {
    let Some(sess) = session::from_headers(secret, headers) else {
        return false;
    };
    match sess.role {
        Role::User => conn_pass.entry_generation(&sess.entry_id) == Some(sess.generation),
        Role::Admin => password::constant_time_str_eq(&sess.admin_version, admin_ver),
    }
}

/// Tracks live /ws and /api/presence connections with their role so the admin
/// endpoint can cut user connections selectively.
#[derive(Default)]
pub struct WsRegistry {
    next: AtomicU64,
    entries: Mutex<HashMap<u64, (Role, CancellationToken)>>,
}

impl WsRegistry {
    pub fn add(&self, role: Role, cancel: CancellationToken) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        self.entries.lock().unwrap().insert(id, (role, cancel));
        id
    }

    pub fn remove(&self, id: u64) {
        self.entries.lock().unwrap().remove(&id);
    }

    /// Cancels every connection (shutdown: SSE streams would otherwise hold
    /// the graceful drain open).
    pub fn disconnect_all(&self) {
        let cancels: Vec<CancellationToken> = self.entries.lock().unwrap().values().map(|(_, c)| c.clone()).collect();
        for c in cancels {
            c.cancel();
        }
    }

    /// Cancels every user-role connection. Cancels run outside the lock since
    /// connection cleanup calls `remove`.
    pub fn disconnect_users(&self) -> usize {
        let cancels: Vec<CancellationToken> =
            self.entries.lock().unwrap().values().filter(|(role, _)| *role == Role::User).map(|(_, c)| c.clone()).collect();
        for c in &cancels {
            c.cancel();
        }
        cancels.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnect_users_only_cancels_users() {
        let r = WsRegistry::default();
        let u1 = CancellationToken::new();
        let u2 = CancellationToken::new();
        let a = CancellationToken::new();
        r.add(Role::User, u1.clone());
        let id = r.add(Role::User, u2.clone());
        r.add(Role::Admin, a.clone());
        assert_eq!(r.disconnect_users(), 2);
        assert!(u1.is_cancelled() && u2.is_cancelled() && !a.is_cancelled());
        r.remove(id);
        assert_eq!(r.disconnect_users(), 1);
    }
}
