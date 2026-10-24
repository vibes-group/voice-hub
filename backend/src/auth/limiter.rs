use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Tracks failed logins per IP and blocks an IP that exceeds `max` failures
/// within the sliding `window`.
pub struct AuthLimiter {
    inner: Mutex<Inner>,
    max: u32,
    window: Duration,
}

struct Inner {
    attempts: HashMap<String, Attempt>,
    last_sweep: Option<Instant>,
}

struct Attempt {
    count: u32,
    first: Instant,
}

impl AuthLimiter {
    pub fn new(max: u32, window: Duration) -> AuthLimiter {
        AuthLimiter { inner: Mutex::new(Inner { attempts: HashMap::new(), last_sweep: None }), max, window }
    }

    pub fn blocked(&self, ip: &str) -> bool {
        self.blocked_at(ip, Instant::now())
    }

    pub fn fail(&self, ip: &str) {
        self.fail_at(ip, Instant::now())
    }

    pub fn success(&self, ip: &str) {
        self.inner.lock().unwrap().attempts.remove(ip);
    }

    fn blocked_at(&self, ip: &str, now: Instant) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let Some(a) = inner.attempts.get(ip) else {
            return false;
        };
        if now.duration_since(a.first) > self.window {
            inner.attempts.remove(ip);
            return false;
        }
        a.count >= self.max
    }

    fn fail_at(&self, ip: &str, now: Instant) {
        let mut inner = self.inner.lock().unwrap();
        // Evict stale entries at most once per window.
        if inner.last_sweep.is_none_or(|t| now.duration_since(t) >= self.window) {
            inner.last_sweep = Some(now);
            let window = self.window;
            inner.attempts.retain(|_, a| now.duration_since(a.first) <= window);
        }
        match inner.attempts.get_mut(ip) {
            Some(a) if now.duration_since(a.first) <= self.window => a.count += 1,
            _ => {
                inner.attempts.insert(ip.to_string(), Attempt { count: 1, first: now });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(60);

    #[test]
    fn blocks_after_max_and_unblocks_after_window() {
        let l = AuthLimiter::new(3, MIN);
        let t0 = Instant::now();
        for _ in 0..3 {
            l.fail_at("1.1.1.1", t0);
        }
        assert!(l.blocked_at("1.1.1.1", t0));
        assert!(!l.blocked_at("2.2.2.2", t0));
        assert!(!l.blocked_at("1.1.1.1", t0 + 2 * MIN));
    }

    #[test]
    fn success_clears() {
        let l = AuthLimiter::new(2, MIN);
        l.fail("1.1.1.1");
        l.success("1.1.1.1");
        assert!(!l.blocked("1.1.1.1"));
        assert!(l.inner.lock().unwrap().attempts.is_empty());
    }

    #[test]
    fn sweep_evicts_orphans_but_is_throttled() {
        let l = AuthLimiter::new(5, MIN);
        let t0 = Instant::now();
        for i in 0..100 {
            l.fail_at(&format!("10.0.0.{i}"), t0);
        }
        assert_eq!(l.inner.lock().unwrap().attempts.len(), 100);
        l.fail_at("fresh", t0 + MIN / 2);
        assert_eq!(l.inner.lock().unwrap().attempts.len(), 101);
        l.fail_at("fresh2", t0 + 2 * MIN);
        let inner = l.inner.lock().unwrap();
        assert_eq!(inner.attempts.len(), 1);
        assert!(inner.attempts.contains_key("fresh2"));
    }

    #[test]
    fn counter_resets_after_window() {
        let l = AuthLimiter::new(2, MIN);
        let t0 = Instant::now();
        l.fail_at("1.1.1.1", t0);
        l.fail_at("1.1.1.1", t0);
        l.fail_at("1.1.1.1", t0 + 2 * MIN);
        assert!(!l.blocked_at("1.1.1.1", t0 + 2 * MIN));
        assert_eq!(l.inner.lock().unwrap().attempts["1.1.1.1"].count, 1);
    }
}
