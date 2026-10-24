//! Fans out presence deltas (and an initial snapshot) to /api/presence
//! subscribers over Server-Sent Events.
//!
//! Deltas are not sequenced: a subscriber that falls behind (queue full) has
//! its queue replaced by a fresh snapshot, and clients merge idempotently.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use serde::Serialize;
use tokio::sync::Notify;

use crate::sfu::protocol::{self, PeerInfo, PresencePeer, PresencePeerLeft, PresenceRoom, PresenceSnapshot};

const OUT_BUF_LEN: usize = 8;
pub const HEARTBEAT: Duration = Duration::from_secs(25);

pub type RoomLister = Box<dyn Fn() -> Vec<(String, Vec<PeerInfo>)> + Send + Sync>;

#[derive(Default)]
pub struct Hub {
    rooms: OnceLock<RoomLister>,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    next_id: u64,
    subs: HashMap<u64, Arc<Subscriber>>,
}

pub struct Subscriber {
    queue: Mutex<VecDeque<Bytes>>,
    notify: Notify,
}

impl Subscriber {
    /// Next frame to write, waiting if none is queued.
    pub async fn next(&self) -> Bytes {
        loop {
            let notified = self.notify.notified();
            if let Some(f) = self.queue.lock().unwrap().pop_front() {
                return f;
            }
            notified.await;
        }
    }
}

/// Unregisters the subscriber when the SSE stream is dropped.
pub struct Subscription {
    hub: Arc<Hub>,
    id: u64,
    pub sub: Arc<Subscriber>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.hub.inner.lock().unwrap().subs.remove(&self.id);
    }
}

fn frame(event: &str, payload: &impl Serialize) -> Bytes {
    let data = serde_json::to_string(payload).expect("serializable payload");
    Bytes::from(format!("event: {event}\ndata: {data}\n\n"))
}

impl Hub {
    pub fn set_rooms(&self, rooms: RoomLister) {
        let _ = self.rooms.set(rooms);
    }

    pub fn peer_joined(&self, room: &str, peer: &PeerInfo) {
        self.push(frame(protocol::PRESENCE_PEER_JOINED_EVENT, &PresencePeer { room, peer }));
    }

    pub fn peer_left(&self, room: &str, id: &str) {
        self.push(frame(protocol::PRESENCE_PEER_LEFT_EVENT, &PresencePeerLeft { room, id }));
    }

    pub fn peer_updated(&self, room: &str, peer: &PeerInfo) {
        self.push(frame(protocol::PRESENCE_PEER_UPDATED_EVENT, &PresencePeer { room, peer }));
    }

    fn push(&self, f: Bytes) {
        let inner = self.inner.lock().unwrap();
        let mut snapshot: Option<Bytes> = None;
        for sub in inner.subs.values() {
            {
                let mut q = sub.queue.lock().unwrap();
                if q.len() < OUT_BUF_LEN {
                    q.push_back(f.clone());
                } else {
                    // Slow subscriber: drop its backlog and resync from a snapshot.
                    let snap = snapshot.get_or_insert_with(|| self.snapshot()).clone();
                    q.clear();
                    q.push_back(snap);
                }
            }
            sub.notify.notify_one();
        }
    }

    /// Registers a subscriber whose first frame is the current snapshot. Taken
    /// under the hub lock, so no delta can slip between snapshot and registration.
    pub fn subscribe(self: &Arc<Self>) -> Subscription {
        let mut inner = self.inner.lock().unwrap();
        let sub = Arc::new(Subscriber { queue: Mutex::new(VecDeque::from([self.snapshot()])), notify: Notify::new() });
        inner.next_id += 1;
        let id = inner.next_id;
        inner.subs.insert(id, sub.clone());
        Subscription { hub: self.clone(), id, sub }
    }

    fn snapshot(&self) -> Bytes {
        let rooms: BTreeMap<String, PresenceRoom> =
            self.rooms.get().map(|f| f()).unwrap_or_default().into_iter().map(|(slug, peers)| (slug, PresenceRoom { peers })).collect();
        frame(protocol::PRESENCE_SNAPSHOT_EVENT, &PresenceSnapshot { rooms })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hub() -> Arc<Hub> {
        let h = Arc::new(Hub::default());
        h.set_rooms(Box::new(|| vec![("room1".to_string(), vec![PeerInfo { id: "p".into(), ..Default::default() }])]));
        h
    }

    #[tokio::test]
    async fn snapshot_first_then_deltas() {
        let h = hub();
        let s = h.subscribe();
        h.peer_left("room1", "x");
        let first = String::from_utf8(s.sub.next().await.to_vec()).unwrap();
        assert_eq!(first, "event: presence-snapshot\ndata: {\"rooms\":{\"room1\":{\"peers\":[{\"id\":\"p\"}]}}}\n\n");
        let second = String::from_utf8(s.sub.next().await.to_vec()).unwrap();
        assert_eq!(second, "event: presence-peer-left\ndata: {\"room\":\"room1\",\"id\":\"x\"}\n\n");
    }

    #[tokio::test]
    async fn overflow_resyncs_with_snapshot() {
        let h = hub();
        let s = h.subscribe();
        for _ in 0..20 {
            h.peer_left("room1", "x");
        }
        let q = s.sub.queue.lock().unwrap();
        assert!(q.len() <= OUT_BUF_LEN);
        assert!(q.iter().any(|f| f.starts_with(b"event: presence-snapshot")));
    }

    #[tokio::test]
    async fn drop_unsubscribes() {
        let h = hub();
        drop(h.subscribe());
        assert!(h.inner.lock().unwrap().subs.is_empty());
    }
}
