use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::protocol::{Attachment, ChatMessage, validate_chat};
use crate::settings::Storage;

pub const HISTORY_CAP: usize = 1000;
pub const HISTORY_TTL_MS: u64 = 7 * 24 * 60 * 60 * 1000;
pub const CACHE_CAP: u64 = 1024 * 1024 * 1024;

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub struct History {
    pub messages: Vec<ChatMessage>,
    storage: Storage,
    name: String,
}

impl History {
    pub fn load(storage: Storage, server: &str, room: &str) -> Result<Self, String> {
        let name = format!("chat-{}.json", Storage::namespace(server, room));
        let messages = storage.read(&name)?.unwrap_or_default();
        let mut history = Self {
            messages,
            storage,
            name,
        };
        history.prune(now_ms());
        history.save()?;
        Ok(history)
    }
    pub fn prune(&mut self, now: u64) {
        self.messages
            .retain(|m| m.ts >= now.saturating_sub(HISTORY_TTL_MS));
        if self.messages.len() > HISTORY_CAP {
            self.messages.drain(..self.messages.len() - HISTORY_CAP);
        }
    }
    pub fn save(&mut self) -> Result<(), String> {
        self.prune(now_ms());
        self.storage.write(&self.name, &self.messages)
    }
    pub fn optimistic(
        &mut self,
        text: String,
        attachments: Vec<Attachment>,
        peer_id: String,
        name: String,
        client_id: String,
    ) -> Result<String, String> {
        validate_chat(&text, &attachments)?;
        let id = uuid::Uuid::new_v4().to_string();
        self.messages.push(ChatMessage {
            id: format!("pending:{id}"),
            client_msg_id: id.clone(),
            from: peer_id,
            text: text.trim().into(),
            sender_name: name,
            sender_client_id: client_id,
            ts: now_ms(),
            attachments,
            pending: true,
            ..Default::default()
        });
        self.save()?;
        Ok(id)
    }
    pub fn receive(&mut self, mut message: ChatMessage, self_id: &str, client_id: &str) {
        if message.from == self_id {
            message.sender_client_id = client_id.into();
        }
        let index = self.messages.iter().position(|m| {
            m.id == message.id
                || (!message.client_msg_id.is_empty()
                    && m.pending
                    && m.client_msg_id == message.client_msg_id
                    && m.sender_client_id == client_id
                    && message.from == self_id)
        });
        if let Some(index) = index {
            self.messages[index] = message;
        } else {
            self.messages.push(message);
        }
    }
    pub fn delete(&mut self, id: &str) {
        self.messages.retain(|m| m.id != id);
    }
    pub fn mark_deleted(&mut self, ids: &BTreeSet<String>) {
        for m in &mut self.messages {
            for a in &m.attachments {
                if ids.contains(&a.upload_id) && !m.deleted_upload_ids.contains(&a.upload_id) {
                    m.deleted_upload_ids.push(a.upload_id.clone());
                }
            }
        }
    }
    pub fn pending(&self, client_id: &str) -> impl Iterator<Item = &ChatMessage> {
        self.messages.iter().filter(move |m| {
            m.pending
                && !m.failed
                && m.sender_client_id == client_id
                && m.attachments
                    .iter()
                    .all(|a| !a.upload_id.starts_with("temp:"))
        })
    }
    pub fn may_delete(message: &ChatMessage, client_id: &str, admin: bool) -> bool {
        admin || (!client_id.is_empty() && message.sender_client_id == client_id)
    }
}

#[derive(Clone)]
pub struct BlobCache {
    root: PathBuf,
}
impl BlobCache {
    pub fn new(storage: &Storage, server: &str) -> Result<Self, String> {
        let root = storage
            .root()
            .join(format!("files-{}", Storage::namespace(server, "")));
        std::fs::create_dir_all(&root).map_err(|_| "Не удалось создать кеш файлов")?;
        Ok(Self { root })
    }
    pub fn path(&self, id: &str) -> Result<PathBuf, String> {
        // Never allow a server-supplied upload id to become a filesystem path.
        if id.is_empty()
            || id.len() > 160
            || !id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
        {
            return Err("Некорректный идентификатор файла".into());
        }
        Ok(self.root.join(id))
    }
    pub fn contains(&self, id: &str) -> bool {
        self.path(id).is_ok_and(|p| p.is_file())
    }
    pub fn delete(&self, id: &str) -> Result<(), String> {
        let path = self.path(id)?;
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err("Не удалось удалить файл из кеша".into()),
        }
    }
    pub async fn put(&self, id: &str, source: &Path) -> Result<(), String> {
        let dest = self.path(id)?;
        tokio::fs::copy(source, dest)
            .await
            .map_err(|_| "Не удалось сохранить файл в кеше")?;
        Ok(())
    }
    pub fn trim(&self) -> Result<BTreeSet<String>, String> {
        let entries = std::fs::read_dir(&self.root).map_err(|_| "Не удалось прочитать кеш")?;
        let mut files = Vec::new();
        let mut total = 0;
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            total += meta.len();
            files.push((meta.modified().ok(), meta.len(), entry.path()));
        }
        files.sort_by_key(|f| f.0);
        let mut removed = BTreeSet::new();
        for (_, size, path) in files {
            if total <= CACHE_CAP {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(size);
                if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                    removed.insert(name.to_owned());
                }
            }
        }
        Ok(removed)
    }
}

#[derive(Default)]
pub struct PingLimiter {
    sent: BTreeMap<String, std::time::Instant>,
}
impl PingLimiter {
    pub fn remaining(&self, target: &str) -> u64 {
        self.sent
            .get(target)
            .map_or(0, |at| 10u64.saturating_sub(at.elapsed().as_secs()))
    }
    pub fn mark(&mut self, target: String) {
        self.sent.insert(target, std::time::Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reconciles_only_own_echo_and_keeps_history_isolated() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let storage = Storage::at(root.clone()).unwrap();
        let mut history = History::load(storage.clone(), "https://one.example", "room1").unwrap();
        let id = history
            .optimistic(
                "hello".into(),
                vec![],
                "old-peer".into(),
                "Tester".into(),
                "installation".into(),
            )
            .unwrap();
        let echo = ChatMessage {
            id: "server-id".into(),
            from: "new-peer".into(),
            text: "hello".into(),
            client_msg_id: id,
            ts: now_ms(),
            ..Default::default()
        };
        history.receive(echo, "new-peer", "installation");
        assert_eq!(history.messages.len(), 1);
        assert!(!history.messages[0].pending);
        assert!(History::may_delete(
            &history.messages[0],
            "installation",
            false
        ));
        history.save().unwrap();
        assert!(
            History::load(storage, "https://two.example", "room1")
                .unwrap()
                .messages
                .is_empty()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn validates_utf8_byte_limit_and_untrusted_file_ids() {
        assert!(validate_chat(&"я".repeat(1001), &[]).is_err());
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let storage = Storage::at(root.clone()).unwrap();
        let cache = BlobCache::new(&storage, "https://one.example").unwrap();
        for bad in ["../settings.json", "a/b", "..", "", "C:\\x", "a%2fb"] {
            assert!(cache.path(bad).is_err());
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
