//! In-process, disk-backed transient store for chat attachments. An upload
//! lands in a temp file with a short TTL, lives just long enough for the peers
//! present at send time to download it, and is then swept. Eviction is purely
//! TTL- and budget-driven.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use crate::util::{hex, random_bytes, ulid};

#[derive(Clone, Debug)]
pub struct Entry {
    pub upload_id: String,
    pub room_id: String,
    pub name: String,
    pub mime: String,
    pub size: u64,
    pub temp_path: PathBuf,
    pub created_at: SystemTime,
    pub expires_at: SystemTime,
}

pub struct Config {
    pub temp_dir: PathBuf,
    /// How long a fresh upload survives without a download.
    pub ttl: Duration,
    /// Lifetime bound from creation regardless of how often Touch extends it.
    pub ttl_hard_cap: Duration,
    /// Per-upload ceiling, enforced by the upload handler.
    pub max_upload_bytes: u64,
    /// 0 means unbounded.
    pub max_total_bytes: u64,
    pub janitor_interval: Duration,
}

#[derive(Debug, PartialEq, Eq)]
pub struct BudgetExceeded;

pub struct Store {
    cfg: Config,
    work_dir: PathBuf,
    state: Mutex<State>,
    janitor: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

#[derive(Default)]
struct State {
    entries: HashMap<String, Entry>,
    total: u64,
}

impl Store {
    /// Creates the working directory and starts the background janitor.
    pub fn new(cfg: Config) -> std::io::Result<Arc<Store>> {
        std::fs::create_dir_all(&cfg.temp_dir)?;
        let work_dir = cfg.temp_dir.join(format!("voice-hub-uploads-{}", hex(&random_bytes::<6>())));
        std::fs::create_dir(&work_dir)?;
        set_mode(&work_dir, 0o700)?;
        let store = Arc::new(Store { cfg, work_dir, state: Mutex::default(), janitor: Mutex::new(None) });
        let weak = Arc::downgrade(&store);
        let interval = if store.cfg.janitor_interval.is_zero() { Duration::from_secs(60) } else { store.cfg.janitor_interval };
        let handle = tokio::spawn(async move {
            let mut tick = tokio::time::interval(interval);
            tick.tick().await;
            loop {
                tick.tick().await;
                let Some(store) = weak.upgrade() else { return };
                store.sweep(SystemTime::now());
            }
        });
        *store.janitor.lock().unwrap() = Some(handle);
        Ok(store)
    }

    pub fn max_upload_bytes(&self) -> u64 {
        self.cfg.max_upload_bytes
    }

    /// A fresh temp path inside the working directory for the caller to stream
    /// an upload into.
    pub fn temp_path(&self) -> PathBuf {
        self.work_dir.join(format!("up-{}", hex(&random_bytes::<8>())))
    }

    /// Registers a fully written temp file. On `BudgetExceeded` the caller
    /// still owns (and must remove) the file.
    pub fn store(&self, room_id: &str, name: &str, mime: &str, size: u64, temp_path: PathBuf) -> Result<Entry, BudgetExceeded> {
        self.store_at(room_id, name, mime, size, temp_path, SystemTime::now())
    }

    fn store_at(
        &self,
        room_id: &str,
        name: &str,
        mime: &str,
        size: u64,
        temp_path: PathBuf,
        now: SystemTime,
    ) -> Result<Entry, BudgetExceeded> {
        let mut st = self.state.lock().unwrap();
        if self.cfg.max_total_bytes > 0 && st.total + size > self.cfg.max_total_bytes {
            return Err(BudgetExceeded);
        }
        let entry = Entry {
            upload_id: ulid(now),
            room_id: room_id.to_string(),
            name: name.to_string(),
            mime: mime.to_string(),
            size,
            temp_path,
            created_at: now,
            expires_at: now + self.cfg.ttl,
        };
        st.entries.insert(entry.upload_id.clone(), entry.clone());
        st.total += size;
        Ok(entry)
    }

    /// An expired-but-unswept entry reads as absent so downloads 404 the
    /// instant the TTL lapses.
    pub fn get(&self, id: &str) -> Option<Entry> {
        self.get_at(id, SystemTime::now())
    }

    fn get_at(&self, id: &str, now: SystemTime) -> Option<Entry> {
        let st = self.state.lock().unwrap();
        st.entries.get(id).filter(|e| now <= e.expires_at).cloned()
    }

    /// Extends the TTL on download, never past CreatedAt+TTLHardCap and never
    /// shortening it.
    pub fn touch(&self, id: &str) {
        self.touch_at(id, SystemTime::now())
    }

    fn touch_at(&self, id: &str, now: SystemTime) {
        let mut st = self.state.lock().unwrap();
        let Some(e) = st.entries.get_mut(id) else { return };
        if now > e.expires_at {
            return;
        }
        let extended = (now + self.cfg.ttl).min(e.created_at + self.cfg.ttl_hard_cap);
        if extended > e.expires_at {
            e.expires_at = extended;
        }
    }

    fn sweep(&self, now: SystemTime) {
        let stale: Vec<PathBuf> = {
            let mut st = self.state.lock().unwrap();
            let mut stale = Vec::new();
            let mut freed = 0;
            st.entries.retain(|_, e| {
                if now > e.expires_at {
                    stale.push(e.temp_path.clone());
                    freed += e.size;
                    false
                } else {
                    true
                }
            });
            st.total -= freed;
            stale
        };
        for p in stale {
            let _ = std::fs::remove_file(p);
        }
    }

    /// Stops the janitor and removes the working directory.
    pub fn close(&self) {
        if let Some(h) = self.janitor.lock().unwrap().take() {
            h.abort();
        }
        let _ = std::fs::remove_dir_all(&self.work_dir);
    }
}

fn set_mode(p: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(total: u64) -> Arc<Store> {
        Store::new(Config {
            temp_dir: std::env::temp_dir(),
            ttl: Duration::from_secs(300),
            ttl_hard_cap: Duration::from_secs(1800),
            max_upload_bytes: 100,
            max_total_bytes: total,
            janitor_interval: Duration::from_secs(3600),
        })
        .unwrap()
    }

    #[tokio::test]
    async fn ttl_touch_and_sweep() {
        let s = store(0);
        let t0 = SystemTime::now();
        let path = s.temp_path();
        std::fs::write(&path, b"x").unwrap();
        let e = s.store_at("room1", "a.txt", "text/plain", 1, path.clone(), t0).unwrap();
        assert!(s.get_at(&e.upload_id, t0 + Duration::from_secs(299)).is_some());
        assert!(s.get_at(&e.upload_id, t0 + Duration::from_secs(301)).is_none());

        s.touch_at(&e.upload_id, t0 + Duration::from_secs(200));
        assert!(s.get_at(&e.upload_id, t0 + Duration::from_secs(450)).is_some());
        // Hard cap: never past created+30min.
        for m in 1..60 {
            s.touch_at(&e.upload_id, t0 + Duration::from_secs(m * 60).min(Duration::from_secs(1700)));
        }
        assert!(s.get_at(&e.upload_id, t0 + Duration::from_secs(1801)).is_none());

        s.sweep(t0 + Duration::from_secs(1801));
        assert!(!path.exists());
        assert_eq!(s.state.lock().unwrap().total, 0);
        s.close();
    }

    #[tokio::test]
    async fn budget() {
        let s = store(10);
        assert!(s.store("r", "a", "m", 6, s.temp_path()).is_ok());
        assert_eq!(s.store("r", "b", "m", 6, s.temp_path()).unwrap_err(), BudgetExceeded);
        s.close();
    }
}
