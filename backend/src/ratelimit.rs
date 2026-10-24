//! In-memory per-key token bucket (one bucket per client IP). Process-local:
//! counters reset on restart, which is fine for throttling. Memory is bounded
//! by evicting idle keys on access instead of a background sweeper.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const DEFAULT_TTL: Duration = Duration::from_secs(3600);
const DEFAULT_MAX_KEYS: usize = 1 << 16;

pub struct Config {
    /// Tokens per second.
    pub rate: f64,
    pub burst: f64,
    pub ttl: Duration,
    pub max_keys: usize,
}

impl Config {
    pub fn new(rate: f64, burst: u32) -> Config {
        Config { rate, burst: burst as f64, ttl: DEFAULT_TTL, max_keys: DEFAULT_MAX_KEYS }
    }
}

pub struct Limiter {
    cfg: Config,
    buckets: Mutex<HashMap<String, Bucket>>,
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

impl Limiter {
    pub fn new(cfg: Config) -> Limiter {
        Limiter { cfg, buckets: Mutex::new(HashMap::new()) }
    }

    /// Whether an event for `key` is allowed now; when not, how long until a
    /// retry would succeed (for Retry-After).
    pub fn allow(&self, key: &str) -> Result<(), Duration> {
        self.allow_at(key, Instant::now())
    }

    fn allow_at(&self, key: &str, now: Instant) -> Result<(), Duration> {
        let mut buckets = self.buckets.lock().unwrap();
        if !buckets.contains_key(key) && buckets.len() >= self.cfg.max_keys {
            evict(&mut buckets, self.cfg.ttl, now);
        }
        let b = buckets.entry(key.to_string()).or_insert(Bucket { tokens: self.cfg.burst, last: now });
        let elapsed = now.saturating_duration_since(b.last).as_secs_f64();
        b.tokens = (b.tokens + elapsed * self.cfg.rate).min(self.cfg.burst);
        b.last = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            return Ok(());
        }
        if self.cfg.rate <= 0.0 {
            return Err(Duration::ZERO);
        }
        Err(Duration::from_secs_f64((1.0 - b.tokens) / self.cfg.rate))
    }
}

/// Drops every bucket idle past `ttl`; if that freed nothing, drops the single
/// oldest one so there is room for one more under the cap.
fn evict(m: &mut HashMap<String, Bucket>, ttl: Duration, now: Instant) {
    let before = m.len();
    m.retain(|_, b| now.saturating_duration_since(b.last) <= ttl);
    if m.len() < before {
        return;
    }
    if let Some(oldest) = m.iter().min_by_key(|(_, b)| b.last).map(|(k, _)| k.clone()) {
        m.remove(&oldest);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_then_throttle_then_refill() {
        let l = Limiter::new(Config::new(5.0, 3));
        let t0 = Instant::now();
        for _ in 0..3 {
            assert!(l.allow_at("a", t0).is_ok());
        }
        let retry = l.allow_at("a", t0).unwrap_err();
        assert!(retry > Duration::from_millis(150) && retry <= Duration::from_millis(200));
        assert!(l.allow_at("b", t0).is_ok(), "keys are independent");
        assert!(l.allow_at("a", t0 + Duration::from_millis(200)).is_ok());
    }

    #[test]
    fn evicts_oldest_at_cap() {
        let l = Limiter::new(Config { max_keys: 2, ..Config::new(1.0, 1) });
        let t0 = Instant::now();
        let _ = l.allow_at("a", t0);
        let _ = l.allow_at("b", t0 + Duration::from_millis(1));
        let _ = l.allow_at("c", t0 + Duration::from_millis(2));
        let m = l.buckets.lock().unwrap();
        assert_eq!(m.len(), 2);
        assert!(!m.contains_key("a"));
    }
}
