//! Connection passwords: up to 16 labelled, individually rotatable and
//! expirable passwords, persisted as argon2id hashes (plaintext never stored).

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use super::password;
use super::secrets::atomic_write;
use crate::util::{go_time, hex, random_bytes};

const CONN_PASS_FILE: &str = "connection-password.json";
pub const MAX_ENTRIES: usize = 16;
pub const MAX_LABEL_LEN: usize = 64;
const FILE_VERSION: u32 = 2;

/// 22 chars over an 88-char alphabet ≈ 142 bits of entropy.
pub const CONN_PASS_LENGTH: usize = 22;
/// Printable ASCII minus visually confusable characters (0/O, 1/l/I, |).
const ALPHABET: &[u8] = b"!\"#$%&'()*+,-./23456789:;<=>?@ABCDEFGHJKLMNPQRSTUVWXYZ[\\]^_`abcdefghijkmnopqrstuvwxyz{}~";

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    NotFound,
    TooManyEntries,
    Io(String),
}

/// Burned on no-match so timing doesn't reveal "no entries" vs "wrong password".
static DUMMY_HASH: LazyLock<String> = LazyLock::new(|| password::hash("dummy-for-constant-time-verify"));

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    id: String,
    label: String,
    hash: String,
    generation: u64,
    #[serde(with = "go_time")]
    created_at: SystemTime,
    #[serde(with = "go_time::opt", default, skip_serializing_if = "Option::is_none")]
    expires_at: Option<SystemTime>,
}

impl Entry {
    fn is_expired(&self, now: SystemTime) -> bool {
        self.expires_at.is_some_and(|e| now >= e)
    }

    fn status(&self, now: SystemTime) -> EntryStatus {
        EntryStatus {
            id: self.id.clone(),
            label: self.label.clone(),
            generation: self.generation,
            created_at: self.created_at,
            expires_at: self.expires_at,
            expired: self.is_expired(now),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct FileFormat {
    version: u32,
    #[serde(deserialize_with = "null_as_empty")]
    entries: Vec<Entry>,
}

fn null_as_empty<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Entry>, D::Error> {
    Ok(Option::<Vec<Entry>>::deserialize(d)?.unwrap_or_default())
}

/// One row of the read-only snapshot returned to the admin UI.
#[derive(Clone, Debug, Serialize)]
pub struct EntryStatus {
    pub id: String,
    pub label: String,
    pub generation: u64,
    #[serde(with = "go_time")]
    pub created_at: SystemTime,
    #[serde(with = "go_time::opt", skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<SystemTime>,
    pub expired: bool,
}

#[derive(Debug, Serialize)]
pub struct Status {
    pub entries: Vec<EntryStatus>,
}

pub struct Store {
    path: PathBuf,
    state: RwLock<Vec<Entry>>,
}

impl Store {
    pub fn load(dir: &Path) -> Result<Store, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("create data dir: {e}"))?;
        let path = dir.join(CONN_PASS_FILE);
        let entries = match std::fs::read(&path) {
            Ok(data) => serde_json::from_slice::<FileFormat>(&data).map_err(|e| format!("parse {CONN_PASS_FILE}: {e}"))?.entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(format!("read {CONN_PASS_FILE}: {e}")),
        };
        Ok(Store { path, state: RwLock::new(entries) })
    }

    pub fn status(&self) -> Status {
        let now = SystemTime::now();
        let entries = self.state.read().unwrap();
        Status { entries: entries.iter().map(|e| e.status(now)).collect() }
    }

    /// Generation of the entry, or None when it is gone or expired — so
    /// sessions tied to it are rejected on the next request.
    pub fn entry_generation(&self, id: &str) -> Option<u64> {
        if id.is_empty() {
            return None;
        }
        let now = SystemTime::now();
        let entries = self.state.read().unwrap();
        entries.iter().find(|e| e.id == id).filter(|e| !e.is_expired(now)).map(|e| e.generation)
    }

    /// Checks `plain` against every live entry; returns (id, generation) on match.
    pub fn verify(&self, plain: &str) -> Option<(String, u64)> {
        let now = SystemTime::now();
        let entries = self.state.read().unwrap().clone();
        for e in entries.iter().filter(|e| !e.is_expired(now)) {
            if password::verify(&e.hash, plain) {
                return Some((e.id.clone(), e.generation));
            }
        }
        let _ = password::verify(&DUMMY_HASH, plain);
        None
    }

    /// Adds an entry and returns its status plus the one-time plaintext.
    /// `ttl` = None never expires; see `expiry_for` for negative TTLs.
    pub fn create(&self, label: &str, ttl: Ttl) -> Result<(EntryStatus, String), Error> {
        let label = normalize_label(label);
        let plain = generate();
        let hash = password::hash(&plain);
        let id = hex(&random_bytes::<6>());
        let now = SystemTime::now();
        let mut entries = self.state.write().unwrap();
        if entries.len() >= MAX_ENTRIES {
            return Err(Error::TooManyEntries);
        }
        let entry = Entry { id, label, hash, generation: 1, created_at: now, expires_at: expiry_for(now, ttl) };
        entries.push(entry.clone());
        self.persist(&entries)?;
        Ok((entry.status(now), plain))
    }

    /// Updates the expiry without rotating the plaintext.
    pub fn set_ttl(&self, id: &str, ttl: Ttl) -> Result<EntryStatus, Error> {
        let now = SystemTime::now();
        let entry = self.mutate(id, |e| e.expires_at = expiry_for(now, ttl))?;
        Ok(entry.status(now))
    }

    /// Replaces the plaintext and bumps the generation, invalidating sessions
    /// issued against the old one. Expiry is preserved.
    pub fn rotate(&self, id: &str) -> Result<(EntryStatus, String), Error> {
        let plain = generate();
        let hash = password::hash(&plain);
        let now = SystemTime::now();
        let entry = self.mutate(id, |e| {
            e.hash = hash;
            e.generation += 1;
            e.created_at = now;
        })?;
        Ok((entry.status(now), plain))
    }

    pub fn revoke(&self, id: &str) -> Result<(), Error> {
        let mut entries = self.state.write().unwrap();
        let idx = entries.iter().position(|e| e.id == id).ok_or(Error::NotFound)?;
        entries.remove(idx);
        self.persist(&entries)
    }

    pub fn rename(&self, id: &str, label: &str) -> Result<(), Error> {
        let label = normalize_label(label);
        self.mutate(id, |e| e.label = label).map(drop)
    }

    fn mutate(&self, id: &str, f: impl FnOnce(&mut Entry)) -> Result<Entry, Error> {
        let mut entries = self.state.write().unwrap();
        let e = entries.iter_mut().find(|e| e.id == id).ok_or(Error::NotFound)?;
        f(e);
        let entry = e.clone();
        self.persist(&entries)?;
        Ok(entry)
    }

    /// Called with the state lock held, so files land in mutation order.
    fn persist(&self, entries: &[Entry]) -> Result<(), Error> {
        let data = serde_json::to_vec_pretty(&FileFormat { version: FILE_VERSION, entries: entries.to_vec() })
            .map_err(|e| Error::Io(e.to_string()))?;
        atomic_write(&self.path, &data, 0o600).map_err(|e| Error::Io(e.to_string()))
    }
}

/// Entry lifetime as requested by the admin API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ttl {
    Never,
    /// Disabled now but kept on disk so the admin can renew it later.
    Disabled,
    For(Duration),
}

fn expiry_for(now: SystemTime, ttl: Ttl) -> Option<SystemTime> {
    match ttl {
        Ttl::Never => None,
        Ttl::Disabled => Some(now - Duration::from_secs(1)),
        Ttl::For(d) => Some(now + d),
    }
}

fn normalize_label(label: &str) -> String {
    let label = label.trim();
    // Byte-level cut like Go's label[:64]; back off to a char boundary so the
    // result stays valid UTF-8.
    let mut end = label.len().min(MAX_LABEL_LEN);
    while !label.is_char_boundary(end) {
        end -= 1;
    }
    label[..end].to_string()
}

/// Rejection sampling over the OS random source avoids modulo bias.
pub fn generate() -> String {
    let n = ALPHABET.len();
    let cutoff = 256 - (256 % n);
    let mut out = String::with_capacity(CONN_PASS_LENGTH);
    while out.len() < CONN_PASS_LENGTH {
        for b in random_bytes::<32>() {
            if (b as usize) < cutoff && out.len() < CONN_PASS_LENGTH {
                out.push(ALPHABET[b as usize % n] as char);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("vh-connpass-{}", hex(&random_bytes::<8>())));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn lifecycle() {
        let dir = tmp();
        let s = Store::load(&dir).unwrap();
        let (st, plain) = s.create("  friends  ", Ttl::Never).unwrap();
        assert_eq!(st.label, "friends");
        assert_eq!(plain.len(), CONN_PASS_LENGTH);
        assert_eq!(s.verify(&plain), Some((st.id.clone(), 1)));
        assert_eq!(s.verify("nope"), None);

        let (st2, plain2) = s.rotate(&st.id).unwrap();
        assert_eq!(st2.generation, 2);
        assert_eq!(s.verify(&plain), None);
        assert_eq!(s.entry_generation(&st.id), Some(2));

        s.set_ttl(&st.id, Ttl::Disabled).unwrap();
        assert_eq!(s.verify(&plain2), None);
        assert_eq!(s.entry_generation(&st.id), None);
        assert!(s.status().entries[0].expired);

        s.rename(&st.id, "x").unwrap();
        let reloaded = Store::load(&dir).unwrap();
        assert_eq!(reloaded.status().entries[0].label, "x");
        reloaded.revoke(&st.id).unwrap();
        assert_eq!(reloaded.revoke(&st.id), Err(Error::NotFound));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cap() {
        let dir = tmp();
        let s = Store::load(&dir).unwrap();
        for _ in 0..MAX_ENTRIES {
            s.create("", Ttl::Never).unwrap();
        }
        assert_eq!(s.create("", Ttl::Never).unwrap_err(), Error::TooManyEntries);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reads_go_file() {
        let dir = tmp();
        std::fs::write(
            dir.join(CONN_PASS_FILE),
            r#"{"version":2,"entries":[{"id":"abc","label":"l","hash":"h","generation":3,"created_at":"2026-01-02T03:04:05.123456789Z","expires_at":"2030-01-01T00:00:00Z"}]}"#,
        )
        .unwrap();
        let s = Store::load(&dir).unwrap();
        assert_eq!(s.entry_generation("abc"), Some(3));
        let json = serde_json::to_string(&s.status()).unwrap();
        assert!(json.contains(r#""created_at":"2026-01-02T03:04:05.123456789Z""#), "{json}");
        assert!(json.contains(r#""expires_at":"2030-01-01T00:00:00Z""#), "{json}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn alphabet_has_no_confusables() {
        assert_eq!(ALPHABET.len(), 88);
        for c in b"0O1lI|" {
            assert!(!ALPHABET.contains(c));
        }
    }
}
