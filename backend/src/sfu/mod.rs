//! WebRTC SFU embedded in the HTTP process. A room fans each participant's
//! Opus track out to every other participant over one audio PeerConnection
//! per peer, plus separate PeerConnections for screen share (see `screen`).

mod audio;
pub mod codec;
pub mod dd;
pub mod net;
pub mod protocol;
pub mod rtc;
pub mod screen;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime};

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::Utf8Bytes;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_util::sync::CancellationToken;

use crate::filestore;
use crate::http::ws::WebSocket;
use crate::log;
use crate::presence::Hub;
use crate::util::{hex, random_bytes, ulid, unix_millis};
use audio::{AudioPeer, AudioTrack};
use net::{Mux, Pc};
use protocol::{Envelope, PeerInfo, ScreenVideoCodec, envelope};
use screen::ScreenSession;

/// Per-peer outbound queue depth. Broadcasts never wait on a slow socket: a
/// peer whose queue is full is treated as dead and torn down.
const PEER_OUT_BUF_LEN: usize = 1024;
/// After a ping reaches a target, more pings to it within this window drop.
const PING_COOLDOWN: Duration = Duration::from_secs(10);
/// Each renegotiate costs an offer/answer round trip per peer.
const RENEGOTIATE_COOLDOWN: Duration = Duration::from_millis(250);
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
/// Keeps idle connections (lurkers especially) alive through proxy timeouts.
const KEEPALIVE: Duration = Duration::from_secs(25);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

pub struct RoomConfig {
    pub room_id: String,
    /// "localhost" enables dev Origin wildcards; otherwise only this host.
    pub app_hostname: String,
    pub mux: Arc<Mux>,
    pub presence: Option<Arc<Hub>>,
    /// Backs chat attachments; None disables them.
    pub files: Option<Arc<filestore::Store>>,
}

pub struct Room {
    cfg: RoomConfig,
    state: Mutex<State>,
    closed: AtomicBool,
    me: Weak<Room>,
}

#[derive(Default)]
struct State {
    peers: HashMap<String, Entry>,
    /// Keyed by `track_key(owner, kind)`.
    tracks: HashMap<String, Arc<AudioTrack>>,
    /// Live screen sessions by resume token (opaque secret = ownership proof).
    sessions_by_token: HashMap<String, Arc<ScreenSession>>,
}

/// Mutable per-peer room state, guarded by `Room::state`.
struct Entry {
    peer: Arc<Peer>,
    display_name: String,
    self_muted: bool,
    deafened: bool,
    last_ping_received: Option<Instant>,
    last_renegotiate: Option<Instant>,
    /// The publisher session this peer owns.
    screen_session: Option<Arc<ScreenSession>>,
    /// This peer's subscriber PCs, keyed by publisher peer ID.
    screen_subs: HashMap<String, Arc<screen::ScreenSub>>,
    screen_sharing: bool,
    screen_sharing_has_audio: bool,
    screen_sharing_video_codec: Option<ScreenVideoCodec>,
}

impl Entry {
    fn info(&self) -> PeerInfo {
        PeerInfo {
            id: self.peer.id.clone(),
            display_name: self.display_name.clone(),
            client_id: self.peer.client_id.clone(),
            self_muted: self.self_muted,
            deafened: self.deafened,
            chat_only: self.peer.chat_only,
            screen_sharing: self.screen_sharing,
            screen_sharing_has_audio: self.screen_sharing_has_audio,
            screen_sharing_video_codec: self.screen_sharing_video_codec,
        }
    }
}

pub struct Peer {
    pub id: String,
    client_id: String,
    chat_only: bool,
    out: mpsc::Sender<Utf8Bytes>,
    cancel: CancellationToken,
    /// None for chat-only (lurker) peers.
    audio: Option<Arc<AudioPeer>>,
}

impl Peer {
    /// Queues a message. Non-blocking: a full queue cancels the peer so it is
    /// torn down without stalling room-wide broadcasts.
    fn send(&self, msg: &Utf8Bytes) -> bool {
        if self.cancel.is_cancelled() {
            return false;
        }
        match self.out.try_send(msg.clone()) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                log!("sfu: peer {} outq full (cap={PEER_OUT_BUF_LEN}), dropping", self.id);
                self.cancel.cancel();
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }

    fn send_event<T: serde::Serialize>(&self, event: &str, data: &T) {
        self.send(&Utf8Bytes::from(envelope(event, data)));
    }
}

fn msg<T: serde::Serialize>(event: &str, data: &T) -> Utf8Bytes {
    Utf8Bytes::from(envelope(event, data))
}

fn broadcast(peers: &[Arc<Peer>], m: &Utf8Bytes) {
    for p in peers {
        p.send(m);
    }
}

/// Doubles as map key; the kind suffix tells a publisher's tracks apart.
fn track_key(owner: &str, kind: &str) -> String {
    format!("{owner}:{kind}")
}

fn new_peer_id() -> String {
    hex(&random_bytes::<8>())
}

impl Room {
    pub fn new(cfg: RoomConfig) -> Arc<Room> {
        Arc::new_cyclic(|me| Room { cfg, state: Mutex::default(), closed: AtomicBool::new(false), me: me.clone() })
    }

    fn arc(&self) -> Arc<Room> {
        self.me.upgrade().expect("room alive while in use")
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    pub fn origin_patterns(&self) -> Vec<String> {
        match self.cfg.app_hostname.as_str() {
            "" | "localhost" => vec!["localhost:*".into(), "127.0.0.1:*".into()],
            h => vec![h.to_string()],
        }
    }

    pub fn peers(&self) -> Vec<PeerInfo> {
        self.state.lock().unwrap().peers.values().map(Entry::info).collect()
    }

    fn others(st: &State, exclude: &str) -> Vec<Arc<Peer>> {
        st.peers.values().filter(|e| e.peer.id != exclude).map(|e| e.peer.clone()).collect()
    }

    fn all(st: &State) -> Vec<Arc<Peer>> {
        st.peers.values().map(|e| e.peer.clone()).collect()
    }

    fn presence_joined(&self, info: &PeerInfo) {
        if let Some(h) = &self.cfg.presence {
            h.peer_joined(&self.cfg.room_id, info);
        }
    }

    fn presence_left(&self, id: &str) {
        if let Some(h) = &self.cfg.presence {
            h.peer_left(&self.cfg.room_id, id);
        }
    }

    fn presence_updated(&self, info: &PeerInfo) {
        if let Some(h) = &self.cfg.presence {
            h.peer_updated(&self.cfg.room_id, info);
        }
    }

    /// Runs one peer session on an upgraded WebSocket.
    pub async fn serve_ws(self: Arc<Self>, ws: WebSocket, conn_cancel: CancellationToken) {
        let (mut sink, mut stream) = ws.split();
        let cancel = conn_cancel.child_token();

        // The first message must be `hello`, so the peer joins with its name
        // and the join broadcast doesn't race a follow-up rename.
        let first = tokio::time::timeout(HELLO_TIMEOUT, next_data(&mut stream)).await;
        let raw = match first {
            Ok(Some(raw)) => raw,
            Ok(None) => {
                log!("sfu: ws read hello: connection closed");
                return;
            }
            Err(_) => {
                log!("sfu: ws read hello: timeout");
                let _ = sink.send(Message::Close(None)).await;
                return;
            }
        };
        let hello = match serde_json::from_str::<Envelope>(&raw) {
            Ok(env) if env.event == "hello" => match env.data.map(|d| serde_json::from_str::<protocol::Hello>(d.get())) {
                Some(Ok(h)) => h,
                Some(Err(e)) => {
                    log!("sfu: hello payload unmarshal: {e}");
                    protocol::Hello::default()
                }
                None => protocol::Hello::default(),
            },
            Ok(env) => {
                log!("sfu: expected hello, got {:?}", env.event);
                let _ = sink.send(Message::Close(None)).await;
                return;
            }
            Err(_) => {
                log!("sfu: expected hello, got \"\"");
                let _ = sink.send(Message::Close(None)).await;
                return;
            }
        };

        let (tx, rx) = mpsc::channel(PEER_OUT_BUF_LEN);
        let pong = Arc::new(Mutex::new(Instant::now()));
        let writer = tokio::spawn(write_loop(sink, rx, cancel.clone(), pong.clone(), self.me.clone()));

        let id = new_peer_id();
        let audio = (!hello.chat_only).then(|| AudioPeer::new(&self, id.clone(), cancel.clone()));
        let peer = Arc::new(Peer { id, client_id: hello.client_id, chat_only: hello.chat_only, out: tx, cancel: cancel.clone(), audio });
        self.add_peer(&peer, hello.display_name);
        if !peer.chat_only {
            // Initial sync: subscribe this peer to existing tracks.
            self.signal_all();
        }

        loop {
            let raw = tokio::select! {
                _ = cancel.cancelled() => break,
                m = next_frame(&mut stream, &pong) => match m {
                    Some(raw) => raw,
                    None => {
                        cancel.cancel();
                        break;
                    }
                },
            };
            let Ok(env) = serde_json::from_str::<Envelope>(&raw) else {
                log!("sfu: bad json from {}", peer.id);
                continue;
            };
            self.handle_client_message(&peer, &env);
        }

        self.remove_peer(&peer.id);
        if let Some(a) = &peer.audio {
            a.close();
        }
        let _ = writer.await;
    }

    fn add_peer(&self, p: &Arc<Peer>, display_name: String) {
        let (evicted, others, existing, count, info) = {
            let mut st = self.state.lock().unwrap();
            // Reconnects (e.g. network switch) replace the stale session
            // immediately instead of leaving a phantom self until ICE times out.
            let mut evicted = Vec::new();
            if !p.client_id.is_empty() {
                let ids: Vec<String> = st.peers.values().filter(|e| e.peer.client_id == p.client_id).map(|e| e.peer.id.clone()).collect();
                for id in ids {
                    if let Some(e) = st.peers.remove(&id) {
                        st.tracks.retain(|k, _| owner_of(k) != id);
                        evicted.push(e);
                    }
                }
            }
            let existing: Vec<PeerInfo> = st.peers.values().map(Entry::info).collect();
            let others = Self::all(&st);
            let entry = Entry {
                peer: p.clone(),
                display_name,
                self_muted: false,
                deafened: false,
                last_ping_received: None,
                last_renegotiate: None,
                screen_session: None,
                screen_subs: HashMap::new(),
                screen_sharing: false,
                screen_sharing_has_audio: false,
                screen_sharing_video_codec: None,
            };
            let info = entry.info();
            st.peers.insert(p.id.clone(), entry);
            (evicted, others, existing, st.peers.len(), info)
        };

        for ev in &evicted {
            log!("sfu: evicting prior session id={} clientId={:?} (replaced)", ev.peer.id, ev.peer.client_id);
            ev.peer.cancel.cancel();
            // Its subscriber PCs are dead on the client side already.
            for s in ev.screen_subs.values() {
                s.pc.close();
            }
            broadcast(&others, &msg("peer-left", &protocol::PeerLeft { id: &ev.peer.id }));
        }
        log!("sfu: peer joined id={} name={:?} clientId={:?} chatOnly={} peers={count}", p.id, info.display_name, p.client_id, p.chat_only);
        p.send_event("welcome", &protocol::Welcome { id: &p.id, peers: &existing });
        broadcast(&others, &msg("peer-joined", &info));
        if !evicted.is_empty() {
            self.signal_all();
        }
        for ev in &evicted {
            self.presence_left(&ev.peer.id);
        }
        self.presence_joined(&info);
    }

    fn remove_peer(&self, id: &str) {
        let (entry, others, count) = {
            let mut st = self.state.lock().unwrap();
            let Some(e) = st.peers.remove(id) else { return };
            st.tracks.retain(|k, _| owner_of(k) != id);
            (e, Self::all(&st), st.peers.len())
        };
        // Close our subscriber PCs outside the room lock.
        for s in entry.screen_subs.values() {
            s.pc.close();
        }
        if let Some(session) = &entry.screen_session {
            // The publisher is gone: arm the resume grace window.
            self.start_screen_share_grace(session);
        }
        entry.peer.cancel.cancel();
        log!("sfu: peer left id={id} peers={count}");
        broadcast(&others, &msg("peer-left", &protocol::PeerLeft { id }));
        if !entry.peer.chat_only {
            self.signal_all();
        }
        self.presence_left(id);
    }

    /// Stops accepting peers and tears down every session.
    pub fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let peers = Self::all(&self.state.lock().unwrap());
        for p in peers {
            p.cancel.cancel();
        }
    }

    fn handle_client_message(&self, p: &Arc<Peer>, env: &Envelope) {
        let data = env.data.map(|d| d.get()).unwrap_or("null");
        fn parse<'a, T: serde::Deserialize<'a>>(data: &'a str) -> Option<T> {
            serde_json::from_str(data).ok()
        }
        if env.event == "ping" {
            if let Some(pc) = parse::<protocol::PingClient>(data) {
                self.handle_ping(p, &pc.to);
            }
            return;
        }
        // Lurkers may only send and retract chat; everything else drops.
        if p.chat_only && env.event != "chat-send" && env.event != "chat-delete" {
            return;
        }
        match env.event {
            "chat-send" => {
                if let Some(cs) = parse::<protocol::ChatSend>(data) {
                    self.broadcast_chat(p, cs);
                }
            }
            "chat-delete" => {
                if let Some(cd) = parse::<protocol::ChatDelete>(data) {
                    self.broadcast_chat_delete(&cd.id);
                }
            }
            "answer" => {
                if let Some(e) = parse::<protocol::SdpEnvelope>(data) {
                    self.handle_answer(p, e);
                }
            }
            "candidate" => {
                if let Some(e) = parse::<protocol::CandidateEnvelope>(data) {
                    self.handle_candidate(p, e);
                }
            }
            "set-displayname" => {
                if let Some(d) = parse::<protocol::SetDisplayName>(data) {
                    self.set_display_name(&p.id, d.display_name);
                }
            }
            "set-state" => {
                if let Some(s) = parse::<protocol::SetState>(data) {
                    self.set_state(&p.id, s.self_muted, s.deafened);
                }
            }
            "renegotiate" => {
                {
                    let mut st = self.state.lock().unwrap();
                    let Some(e) = st.peers.get_mut(&p.id) else { return };
                    if e.last_renegotiate.is_some_and(|t| t.elapsed() < RENEGOTIATE_COOLDOWN) {
                        return;
                    }
                    e.last_renegotiate = Some(Instant::now());
                }
                self.signal_all();
            }
            // Only screen-share publisher ICE restart after resume; audio
            // always uses the SFU-as-offerer path.
            "offer" => {
                if let Some(e) = parse::<protocol::SdpEnvelope>(data) {
                    self.handle_client_offer(p, e);
                }
            }
            "screen-share-start" => {
                if let Some(d) = parse::<protocol::ScreenShareStart>(data) {
                    self.handle_screen_share_start(p, d);
                }
            }
            "screen-share-stop" => self.handle_screen_share_stop(p),
            "screen-share-resume" => {
                if let Some(d) = parse::<protocol::ScreenShareResume>(data) {
                    self.handle_screen_share_resume(p, &d.session_token);
                }
            }
            "screen-share-subscribe" => {
                if let Some(d) = parse::<protocol::ScreenShareSubscribe>(data) {
                    self.handle_screen_share_subscribe(p, d);
                }
            }
            "screen-share-unsubscribe" => {
                if let Some(d) = parse::<protocol::ScreenShareUnsubscribe>(data) {
                    self.remove_screen_sub(&p.id, &d.publisher_id, None, "client requested");
                }
            }
            "screen-share-mode-change" => {
                if let Some(d) = parse::<protocol::ScreenShareModeChange>(data) {
                    self.handle_screen_share_mode_change(p, d);
                }
            }
            _ => {}
        }
    }

    fn handle_answer(&self, p: &Arc<Peer>, e: protocol::SdpEnvelope) {
        match e.pc {
            protocol::PcKind::Audio => {
                if let Some(a) = &p.audio {
                    a.accept_answer(&e.sdp);
                }
            }
            protocol::PcKind::ScreenSub => self.screen_sub_answer(p, &e.publisher_id, &e.sdp),
            // The SFU answers screen-pub; it never offers it.
            protocol::PcKind::ScreenPub => log!("sfu: unexpected answer pc=screen-pub from {}", p.id),
        }
    }

    fn handle_candidate(&self, p: &Arc<Peer>, e: protocol::CandidateEnvelope) {
        let pc = match e.pc {
            protocol::PcKind::Audio => p.audio.as_ref().map(|a| a.pc.clone()),
            protocol::PcKind::ScreenPub => {
                let s = self.state.lock().unwrap().peers.get(&p.id).and_then(|e| e.screen_session.clone());
                if s.is_none() {
                    log!("sfu: screen-pub candidate ({}) no session", p.id);
                }
                s.map(|s| s.publisher_pc.clone())
            }
            protocol::PcKind::ScreenSub => {
                let pc =
                    self.state.lock().unwrap().peers.get(&p.id).and_then(|en| en.screen_subs.get(&e.publisher_id).map(|s| s.pc.clone()));
                if pc.is_none() {
                    log!("sfu: screen-sub candidate ({}→{}) no PC", p.id, e.publisher_id);
                }
                pc
            }
        };
        if let Some(pc) = pc {
            add_remote_candidate(&pc, &e.candidate);
        }
    }

    fn set_display_name(&self, id: &str, name: String) {
        let (info, others) = {
            let mut st = self.state.lock().unwrap();
            let Some(e) = st.peers.get_mut(id) else { return };
            e.display_name = name;
            let info = e.info();
            (info, Self::others(&st, id))
        };
        broadcast(&others, &msg("peer-info", &info));
        self.presence_updated(&info);
    }

    fn set_state(&self, id: &str, self_muted: bool, deafened: bool) {
        let (info, others) = {
            let mut st = self.state.lock().unwrap();
            let Some(e) = st.peers.get_mut(id) else { return };
            e.self_muted = self_muted;
            e.deafened = deafened;
            let info = e.info();
            (info, Self::others(&st, id))
        };
        broadcast(&others, &msg("peer-state", &protocol::PeerState { id, self_muted, deafened }));
        self.presence_updated(&info);
    }

    fn broadcast_chat(&self, sender: &Arc<Peer>, cs: protocol::ChatSend) {
        let (name, all) = {
            let st = self.state.lock().unwrap();
            let Some(e) = st.peers.get(&sender.id) else { return };
            (e.display_name.clone(), Self::all(&st))
        };
        // Display name doubles as the "hello received" marker.
        if name.is_empty() {
            return;
        }
        let text = cs.text.trim();
        let atts = cs.attachments.unwrap_or_default();
        if text.is_empty() && atts.is_empty() {
            return;
        }
        if text.len() > protocol::CHAT_MAX_BYTES {
            return;
        }
        if !self.attachments_valid(&atts) {
            return;
        }
        let now = SystemTime::now();
        let id = ulid(now);
        let m = msg(
            "chat",
            &protocol::Chat {
                id: &id,
                from: &sender.id,
                text,
                ts: unix_millis(now),
                client_msg_id: &cs.client_msg_id,
                sender_name: &name,
                attachments: &atts,
            },
        );
        broadcast(&all, &m);
    }

    /// Echoed to everyone including the requester. Authorship is not checked:
    /// chat is never persisted, so the server can't know who sent an ID.
    fn broadcast_chat_delete(&self, id: &str) {
        if id.is_empty() {
            return;
        }
        let all = Self::all(&self.state.lock().unwrap());
        broadcast(&all, &msg("chat-deleted", &protocol::ChatDeleted { id }));
    }

    /// Every attachment must be a live upload in this room.
    fn attachments_valid(&self, atts: &[protocol::Attachment]) -> bool {
        if atts.is_empty() {
            return true;
        }
        if atts.len() > protocol::CHAT_MAX_ATTACHMENTS {
            return false;
        }
        let Some(files) = &self.cfg.files else { return false };
        atts.iter().all(|a| {
            !a.upload_id.is_empty() && !a.mime.is_empty() && files.get(&a.upload_id).is_some_and(|e| e.room_id == self.cfg.room_id)
        })
    }

    fn handle_ping(&self, p: &Arc<Peer>, to: &str) {
        if to.is_empty() || to == p.id {
            return;
        }
        let (target, from_name) = {
            let mut st = self.state.lock().unwrap();
            let from_name = st.peers.get(&p.id).map(|e| e.display_name.clone()).unwrap_or_default();
            let Some(t) = st.peers.get_mut(to) else { return };
            if t.last_ping_received.is_some_and(|at| at.elapsed() < PING_COOLDOWN) {
                return;
            }
            t.last_ping_received = Some(Instant::now());
            (t.peer.clone(), from_name)
        };
        target.send_event("ping", &protocol::PingServer { from: &p.id, from_name: &from_name });
    }

    /// Points every audio PC's slots at the current room tracks (all but its
    /// own).
    fn signal_all(&self) {
        let peers: Vec<Arc<AudioPeer>> = self.state.lock().unwrap().peers.values().filter_map(|e| e.peer.audio.clone()).collect();
        for a in peers {
            a.sync();
        }
    }

    /// Same as `signal_all`, but safe to call from a PC event handler (which
    /// must not drive other PCs synchronously).
    fn signal_all_async(&self) {
        let room = self.arc();
        tokio::spawn(async move { room.signal_all() });
    }

    fn publish_track(&self, track: Arc<AudioTrack>) {
        self.state.lock().unwrap().tracks.insert(track.key.clone(), track);
        self.signal_all_async();
    }

    fn unpublish_track(&self, key: &str, track: &Arc<AudioTrack>) {
        let removed = {
            let mut st = self.state.lock().unwrap();
            match st.tracks.get(key) {
                Some(t) if Arc::ptr_eq(t, track) => st.tracks.remove(key).is_some(),
                _ => false,
            }
        };
        if removed {
            self.signal_all_async();
        }
    }
}

fn owner_of(key: &str) -> &str {
    key.split_once(':').map(|(o, _)| o).filter(|o| !o.is_empty()).unwrap_or(key)
}

fn add_remote_candidate(pc: &Arc<Pc>, candidate: &str) {
    let c = candidate.trim().trim_start_matches("a=");
    if c.is_empty() {
        return;
    }
    // mDNS (.local) hosts can't be resolved here; peer-reflexive discovery
    // from the client's own checks covers them.
    if let Ok(c) = str0m::Candidate::from_sdp_string(c) {
        pc.with(|rtc| rtc.add_remote_candidate(c));
    }
}

/// Next data message (text or binary), skipping control frames.
async fn next_data<S>(stream: &mut S) -> Option<String>
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    while let Some(m) = stream.next().await {
        match m.ok()? {
            Message::Text(t) => return Some(t.to_string()),
            Message::Binary(b) => return Some(String::from_utf8_lossy(&b).into_owned()),
            Message::Close(_) => return None,
            _ => {}
        }
    }
    None
}

async fn next_frame<S>(stream: &mut S, pong: &Mutex<Instant>) -> Option<String>
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    while let Some(m) = stream.next().await {
        match m {
            Ok(Message::Text(t)) => return Some(t.to_string()),
            Ok(Message::Binary(b)) => return Some(String::from_utf8_lossy(&b).into_owned()),
            Ok(Message::Pong(_)) => *pong.lock().unwrap() = Instant::now(),
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => {}
        }
    }
    None
}

/// Serialises WS writes for one peer and drives keepalive pings; a ping not
/// answered within the write timeout (or a stalled write) kills the peer.
async fn write_loop<S>(
    mut sink: S,
    mut rx: mpsc::Receiver<Utf8Bytes>,
    cancel: CancellationToken,
    pong: Arc<Mutex<Instant>>,
    room: Weak<Room>,
) where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + KEEPALIVE, KEEPALIVE);
    let mut pong_deadline: Option<(tokio::time::Instant, Instant)> = None;
    loop {
        let deadline = pong_deadline.map(|(d, _)| d);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            m = rx.recv() => {
                let Some(m) = m else { break };
                if !matches!(tokio::time::timeout(WRITE_TIMEOUT, sink.send(Message::Text(m))).await, Ok(Ok(()))) {
                    log!("sfu: ws write stalled or failed, dropping peer");
                    cancel.cancel();
                    return;
                }
            }
            _ = ping.tick() => {
                // Stamped before sending: a pong can arrive before send() returns.
                let sent = Instant::now();
                if !matches!(tokio::time::timeout(WRITE_TIMEOUT, sink.send(Message::Ping(Default::default()))).await, Ok(Ok(()))) {
                    cancel.cancel();
                    return;
                }
                pong_deadline = Some((tokio::time::Instant::now() + WRITE_TIMEOUT, sent));
            }
            _ = async { tokio::time::sleep_until(deadline.unwrap()).await }, if deadline.is_some() => {
                let sent = pong_deadline.take().map(|(_, s)| s).unwrap();
                if *pong.lock().unwrap() < sent {
                    log!("sfu: ws pong missing, dropping peer");
                    cancel.cancel();
                    return;
                }
            }
        }
    }
    let shutting_down = room.upgrade().is_some_and(|r| r.is_closed());
    let frame = if shutting_down {
        CloseFrame { code: CloseCode::Away, reason: "server shutting down".into() }
    } else {
        CloseFrame { code: CloseCode::Normal, reason: "".into() }
    };
    let _ = tokio::time::timeout(WRITE_TIMEOUT, sink.send(Message::Close(Some(frame)))).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        assert_eq!(owner_of(&track_key("abc", "audio")), "abc");
        assert_eq!(owner_of("plain"), "plain");
    }
}

#[cfg(test)]
mod room_tests;
