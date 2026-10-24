//! Screen share. The publisher opens a second PeerConnection (the SFU
//! answers); each viewer gets its own subscriber PC per publisher (the SFU
//! offers), so temporal layers can be dropped per viewer.
//!
//! Lifecycle: screen-share-start → screen-share-started + answer → first
//! video packet → screen-share-available to the room. On publisher WS loss
//! the session survives a 5 s grace window for screen-share-resume.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};
use std::time::{Duration, SystemTime};

use str0m::Event;
use str0m::change::{SdpAnswer, SdpOffer, SdpPendingOffer};
use str0m::format::Codec;
use str0m::media::{Direction, KeyframeRequestKind, MediaKind, Mid, Pt};
use str0m::rtp::{RtpPacket, RtpWrite, SeqNo};
use tokio_util::sync::CancellationToken;

use super::codec::{Adapter, ChainTracker};
use super::dd::Descriptor;
use super::net::{Pc, PcHandler};
use super::protocol::{self, PcKind, ScreenShareMode, ScreenShareReason, ScreenVideoCodec, SdpEnvelope};
use super::rtc::{self, DdBytes, PT_AV1, PT_OPUS, PT_VP9};
use super::{Peer, Room, broadcast, msg};
use crate::log;
use crate::util::{b64_raw_std, random_bytes, random_u32};

const GRACE_PERIOD: Duration = Duration::from_secs(5);
/// Pause/resume payload meaning "the whole stream".
const ALL_LAYERS: &[i32] = &[0, 1, 2];
/// Matches the RTCP RR cadence; faster sampling adds noise, not information.
const AUTO_DOWNGRADE_POLL: Duration = Duration::from_secs(2);

/// Loss thresholds (per mille), sustain windows and layer floor for a mode.
/// Sharp protects readability (drops FPS early, down to T0); motion
/// tolerates more loss and never goes below T1.
#[derive(Clone, Copy)]
pub struct ModePolicy {
    high_loss_pm: u32,
    low_loss_pm: u32,
    high_window: Duration,
    low_window: Duration,
    floor_temp: i32,
}

const SHARP: ModePolicy = ModePolicy {
    high_loss_pm: 50,
    low_loss_pm: 10,
    high_window: Duration::from_secs(2),
    low_window: Duration::from_secs(4),
    floor_temp: 0,
};
const MOTION: ModePolicy = ModePolicy {
    high_loss_pm: 120,
    low_loss_pm: 30,
    high_window: Duration::from_secs(3),
    low_window: Duration::from_secs(7),
    floor_temp: 1,
};

fn policy(m: ScreenShareMode) -> ModePolicy {
    match m {
        ScreenShareMode::Sharp => SHARP,
        ScreenShareMode::Motion => MOTION,
    }
}

fn encode_mode(m: ScreenShareMode) -> u8 {
    m as u8
}

fn decode_mode(v: u8) -> ScreenShareMode {
    if v == ScreenShareMode::Motion as u8 { ScreenShareMode::Motion } else { ScreenShareMode::Sharp }
}

pub struct ScreenSession {
    /// Changes when a reconnected publisher resumes the session.
    publisher_id: Mutex<String>,
    pub token: String,
    pub has_system_audio: bool,
    video_codec: OnceLock<ScreenVideoCodec>,
    mode: AtomicU8,
    pub publisher_pc: Arc<Pc>,
    room: Weak<Room>,
    inner: Mutex<SessionInner>,
    /// Copy-on-write view of `inner.subscribers` for the RTP hot path.
    view: RwLock<Arc<Vec<Arc<ScreenSub>>>>,
    /// Publisher-side negotiation results; set once before the PC can deliver media.
    media: Mutex<PubMedia>,
    adapter: Mutex<Adapter>,
    first_video: AtomicBool,
    cancel: CancellationToken,
}

#[derive(Default)]
struct PubMedia {
    video_mid: Option<Mid>,
    video_pts: Vec<Pt>,
    audio_pts: Vec<Pt>,
}

#[derive(Default)]
struct SessionInner {
    subscribers: HashMap<String, Arc<ScreenSub>>,
    grace: Option<CancellationToken>,
    closed: bool,
}

pub struct ScreenSub {
    peer_id: String,
    pub pc: Arc<Pc>,
    session: Weak<ScreenSession>,
    video_mid: Mid,
    audio_mid: Option<Mid>,
    video_pt: Pt,
    pending: Mutex<Option<SdpPendingOffer>>,
    layer: LayerCtl,
    fwd: Mutex<SubForward>,
    audio_roc_base: OnceLock<u64>,
}

/// Per-viewer temporal-layer target plus the loss hysteresis driving it.
pub struct LayerCtl {
    target_temp: AtomicI32,
    /// Bumped by `set_target_temp`; the forwarder re-arms its chain on change.
    chain_gen: AtomicI32,
    loss_per_mille: AtomicU32,
    high_loss_since: AtomicI64,
    low_loss_since: AtomicI64,
}

impl LayerCtl {
    pub fn new(target: i32) -> LayerCtl {
        LayerCtl {
            target_temp: AtomicI32::new(target),
            chain_gen: AtomicI32::new(0),
            loss_per_mille: AtomicU32::new(0),
            high_loss_since: AtomicI64::new(0),
            low_loss_since: AtomicI64::new(0),
        }
    }

    fn set_target_temp(&self, layer: i32) {
        self.target_temp.store(layer, Ordering::Release);
        self.chain_gen.fetch_add(1, Ordering::AcqRel);
    }
}

struct SubForward {
    last_gen: i32,
    chain: ChainTracker,
    /// Rewritten per subscriber so deliberate drops never look like loss.
    seq: u64,
}

impl ScreenSub {
    fn forward_video(&self, pkt: &RtpPacket, desc: Option<&Descriptor>) {
        let seq = {
            let mut f = self.fwd.lock().unwrap();
            let g = self.layer.chain_gen.load(Ordering::Acquire);
            if g != f.last_gen {
                f.last_gen = g;
                f.chain.set_chain(self.layer.target_temp.load(Ordering::Acquire) as usize);
            }
            if let Some(d) = desc {
                if d.temporal_layer as i32 > self.layer.target_temp.load(Ordering::Acquire) {
                    return;
                }
                if !f.chain.allow(Some(d)) {
                    return;
                }
            }
            f.seq += 1;
            f.seq
        };
        let (mid, pt) = (self.video_mid, self.video_pt);
        self.pc.write(|rtc| {
            if let Some(tx) = rtc.direct_api().stream_tx_by_mid(mid, None) {
                tx.write_rtp(
                    RtpWrite::new(pt, SeqNo::from(seq), pkt.header.timestamp, pkt.timestamp, pkt.payload.clone())
                        .marker(pkt.header.marker)
                        .nackable(true),
                );
            }
        });
    }

    fn forward_audio(&self, pkt: &RtpPacket) {
        let Some(mid) = self.audio_mid else { return };
        let seq = rebase_roc(&self.audio_roc_base, pkt.seq_no);
        self.pc.write(|rtc| {
            if let Some(tx) = rtc.direct_api().stream_tx_by_mid(mid, None) {
                tx.write_rtp(
                    RtpWrite::new(Pt::new_with_value(PT_OPUS), seq, pkt.header.timestamp, pkt.timestamp, pkt.payload.clone())
                        .marker(pkt.header.marker),
                );
            }
        });
    }
}

impl PcHandler for ScreenSub {
    fn on_event(&self, _pc: &Arc<Pc>, ev: Event) {
        match ev {
            // Relay viewer PLIs to the publisher.
            Event::KeyframeRequest(_) => {
                if let Some(s) = self.session.upgrade() {
                    s.request_keyframe();
                }
            }
            Event::MediaEgressStats(st) if st.mid == self.video_mid => {
                if let Some(loss) = st.loss {
                    self.layer.loss_per_mille.store((loss.clamp(0.0, 1.0) * 1000.0) as u32, Ordering::Release);
                }
            }
            _ => {}
        }
    }

    fn on_closed(&self, pc: &Arc<Pc>) {
        let Some(s) = self.session.upgrade() else { return };
        let Some(room) = s.room.upgrade() else { return };
        let pub_id = s.publisher_id();
        room.remove_screen_sub(&self.peer_id, &pub_id, Some(pc), "subscriber pc closed");
    }
}

impl ScreenSession {
    pub fn publisher_id(&self) -> String {
        self.publisher_id.lock().unwrap().clone()
    }

    /// Negotiated at start; set before the session is visible to anyone.
    pub fn video_codec(&self) -> ScreenVideoCodec {
        *self.video_codec.get().expect("codec set at setup")
    }

    pub fn mode(&self) -> ScreenShareMode {
        decode_mode(self.mode.load(Ordering::Acquire))
    }

    fn subscribers(&self) -> Arc<Vec<Arc<ScreenSub>>> {
        self.view.read().unwrap().clone()
    }

    fn refresh_view(&self, inner: &SessionInner) {
        *self.view.write().unwrap() = Arc::new(inner.subscribers.values().cloned().collect());
    }

    /// PLI to the publisher so the next frame is a keyframe.
    fn request_keyframe(&self) {
        let Some(mid) = self.media.lock().unwrap().video_mid else { return };
        self.publisher_pc.write(|rtc| {
            if let Some(rx) = rtc.direct_api().stream_rx_by_mid(mid, None) {
                rx.request_keyframe(KeyframeRequestKind::Pli);
            }
        });
        log!("sfu: screen PLI pub={}", self.publisher_id());
    }

    fn on_video(&self, pkt: &RtpPacket) {
        if !self.first_video.swap(true, Ordering::AcqRel) {
            // The SFU renumbers by arrival and never asks the publisher for
            // retransmissions.
            let ssrc = pkt.header.ssrc;
            self.publisher_pc.write(|rtc| {
                if let Some(rx) = rtc.direct_api().stream_rx(&ssrc) {
                    rx.suppress_nack(true);
                }
            });
            if let Some(room) = self.room.upgrade() {
                room.first_screen_video_ready(self);
            }
        }
        let desc = {
            let mut a = self.adapter.lock().unwrap();
            let dd = pkt.header.ext_vals.user_values.get::<DdBytes>().map(|d| d.0.as_slice());
            match a.parse(&pkt.payload, dd) {
                Ok(d) => d,
                Err(e) => {
                    log!("sfu: screen forwardVideo ({}) {} parse: {e}", self.publisher_id(), a.name());
                    None
                }
            }
        };
        for sub in self.subscribers().iter() {
            sub.forward_video(pkt, desc.as_ref());
        }
    }
}

impl PcHandler for ScreenSession {
    fn on_event(&self, _pc: &Arc<Pc>, ev: Event) {
        let Event::RtpPacket(pkt) = ev else { return };
        if pkt.payload.is_empty() {
            return; // padding-only probe
        }
        let pt = pkt.header.payload_type;
        let (video, audio) = {
            let m = self.media.lock().unwrap();
            (m.video_pts.contains(&pt), m.audio_pts.contains(&pt))
        };
        if video {
            self.on_video(&pkt);
        } else if audio && self.has_system_audio {
            for sub in self.subscribers().iter() {
                sub.forward_audio(&pkt);
            }
        }
    }

    fn on_closed(&self, _pc: &Arc<Pc>) {
        // Ending the session closes the subscriber PCs; a PC handler must not
        // drive other PCs synchronously.
        let (Some(room), token) = (self.room.upgrade(), self.token.clone()) else { return };
        tokio::spawn(async move {
            if let Some(me) = room.find_session(&token) {
                room.end_screen_share_session(&me, "publisher pc closed");
            }
        });
    }
}

/// One m-section of an SDP: kind, mid, payload types in preference order.
struct Section {
    kind: String,
    mid: String,
    pts: Vec<u8>,
    rejected: bool,
}

fn sections(sdp: &str) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    for line in sdp.lines() {
        if let Some(m) = line.strip_prefix("m=") {
            let mut f = m.split_whitespace();
            let kind = f.next().unwrap_or("").to_string();
            let rejected = f.next() == Some("0");
            let pts = f.skip(1).filter_map(|p| p.parse().ok()).collect();
            out.push(Section { kind, mid: String::new(), pts, rejected });
        } else if let (Some(mid), Some(s)) = (line.strip_prefix("a=mid:"), out.last_mut()) {
            s.mid = mid.trim().to_string();
        }
    }
    out
}

fn video_codec(c: Codec) -> Option<ScreenVideoCodec> {
    match c {
        Codec::Av1 => Some(ScreenVideoCodec::Av1),
        Codec::Vp9 => Some(ScreenVideoCodec::Vp9),
        _ => None,
    }
}

fn sub_video_pt(c: ScreenVideoCodec) -> Pt {
    Pt::new_with_value(match c {
        ScreenVideoCodec::Av1 => PT_AV1,
        ScreenVideoCodec::Vp9 => PT_VP9,
    })
}

fn sdp_msg(pc: PcKind, publisher_id: &str, kind: &str, sdp: String) -> SdpEnvelope {
    SdpEnvelope { pc, publisher_id: publisher_id.to_string(), kind: kind.into(), sdp }
}

impl Room {
    fn send_screen_error(&self, p: &Peer, publisher_id: &str, reason: ScreenShareReason) {
        p.send_event("screen-share-error", &protocol::ScreenShareError { publisher_id, reason });
    }

    fn peer_handle(&self, id: &str) -> Option<Arc<Peer>> {
        self.state.lock().unwrap().peers.get(id).map(|e| e.peer.clone())
    }

    pub(super) fn find_session(&self, token: &str) -> Option<Arc<ScreenSession>> {
        self.state.lock().unwrap().sessions_by_token.get(token).cloned()
    }

    fn send_encode(&self, publisher_id: &str, event: &str) {
        if let Some(p) = self.peer_handle(publisher_id) {
            p.send_event(event, &protocol::ScreenShareEncodeLayers { layers: ALL_LAYERS });
            log!("sfu: dynacast {event} pub={publisher_id}");
        }
    }

    pub(super) fn handle_screen_share_start(&self, p: &Arc<Peer>, data: protocol::ScreenShareStart) {
        if self.state.lock().unwrap().peers.get(&p.id).is_some_and(|e| e.screen_session.is_some()) {
            log!("sfu: screen-share-start ({}): already publishing", p.id);
            self.send_screen_error(p, "", ScreenShareReason::AlreadyPublishing);
            return;
        }
        let Some((session, answer)) = self.setup_screen_pub(p, data) else {
            self.send_screen_error(p, "", ScreenShareReason::Internal);
            return;
        };
        let registered = {
            let mut st = self.state.lock().unwrap();
            match st.peers.get_mut(&p.id) {
                Some(e) if e.screen_session.is_none() => {
                    e.screen_session = Some(session.clone());
                    e.screen_sharing = true;
                    e.screen_sharing_has_audio = session.has_system_audio;
                    e.screen_sharing_video_codec = Some(session.video_codec());
                    st.sessions_by_token.insert(session.token.clone(), session.clone());
                    true
                }
                _ => false,
            }
        };
        if !registered {
            session.publisher_pc.close();
            return;
        }
        tokio::spawn(auto_downgrade_loop(session.clone()));
        // -started carries the resume token; the FIFO writer delivers it
        // before the answer, so the publisher has it before media flows.
        p.send_event("screen-share-started", &protocol::ScreenShareStarted { session_token: &session.token });
        p.send_event("answer", &sdp_msg(PcKind::ScreenPub, "", "answer", answer));
    }

    /// Creates the publisher PC, answers the offer and captures the
    /// negotiated codecs. Video is required; system audio is optional.
    fn setup_screen_pub(&self, p: &Arc<Peer>, data: protocol::ScreenShareStart) -> Option<(Arc<ScreenSession>, String)> {
        let token = b64_raw_std(&random_bytes::<32>());
        let socket = self.cfg.mux.pick();
        let (rtc, ufrag) = rtc::build(&socket, false);
        let offer = match SdpOffer::from_sdp_string(&data.sdp) {
            Ok(o) => o,
            Err(e) => {
                log!("sfu: screen-share-start ({}) set remote: {e}", p.id);
                return None;
            }
        };
        let mode = data.mode.unwrap_or_default();
        let room = self.me.clone();
        let mut codec = None;
        let session = Arc::new_cyclic(|me: &Weak<ScreenSession>| {
            let handler: Weak<dyn PcHandler> = me.clone();
            ScreenSession {
                publisher_id: Mutex::new(p.id.clone()),
                token,
                has_system_audio: data.has_system_audio,
                video_codec: OnceLock::new(),
                mode: AtomicU8::new(encode_mode(mode)),
                publisher_pc: Pc::spawn(rtc, socket, ufrag, handler),
                room,
                inner: Mutex::default(),
                view: RwLock::new(Arc::new(Vec::new())),
                media: Mutex::default(),
                adapter: Mutex::new(Adapter::Vp9),
                first_video: AtomicBool::new(false),
                cancel: CancellationToken::new(),
            }
        });
        let answer = session.publisher_pc.with(|rtc| {
            let answer = rtc.sdp_api().accept_offer(offer)?;
            let sdp = answer.to_sdp_string();
            let mut media = session.media.lock().unwrap();
            for s in sections(&sdp).into_iter().filter(|s| !s.rejected) {
                let pts: Vec<Pt> = s.pts.iter().map(|&v| Pt::new_with_value(v)).collect();
                match s.kind.as_str() {
                    "video" if media.video_mid.is_none() => {
                        codec = pts.first().and_then(|&pt| rtc::codec_of_pt(rtc, pt));
                        media.video_mid = Some(Mid::from(s.mid.as_str()));
                        media.video_pts = pts.into_iter().filter(|&pt| rtc::codec_of_pt(rtc, pt).and_then(video_codec).is_some()).collect();
                    }
                    "audio" => media.audio_pts.extend(pts.into_iter().filter(|&pt| rtc::codec_of_pt(rtc, pt) == Some(Codec::Opus))),
                    _ => {}
                }
            }
            Ok::<_, str0m::RtcError>(sdp)
        });
        let answer = match answer {
            Ok(a) => a,
            Err(e) => {
                log!("sfu: screen-share-start ({}) set remote: {e}", p.id);
                session.publisher_pc.close();
                return None;
            }
        };
        let Some(codec) = codec else {
            log!("sfu: screen-share-start ({}) no video transceiver in offer", p.id);
            session.publisher_pc.close();
            return None;
        };
        let Some(vc) = video_codec(codec) else {
            log!("sfu: screen-share-start ({}) unsupported video codec: {codec:?}", p.id);
            session.publisher_pc.close();
            return None;
        };
        let _ = session.video_codec.set(vc);
        *session.adapter.lock().unwrap() = Adapter::new(vc);
        Some((session, answer))
    }

    /// Once per session, on the first video packet: tell the room media is
    /// actually flowing, and pause the encoder until somebody watches.
    pub(super) fn first_screen_video_ready(&self, session: &ScreenSession) {
        if session.inner.lock().unwrap().closed {
            return;
        }
        let pub_id = session.publisher_id();
        let (info, others) = {
            let st = self.state.lock().unwrap();
            let Some(e) = st.peers.get(&pub_id) else { return };
            (e.info(), Self::others(&st, &pub_id))
        };
        let info_msg = msg("peer-info", &info);
        let avail = msg(
            "screen-share-available",
            &protocol::ScreenShareAvailable {
                publisher_id: &pub_id,
                has_system_audio: session.has_system_audio,
                video_codec: Some(session.video_codec()),
                mode: Some(session.mode()),
            },
        );
        for op in &others {
            op.send(&info_msg);
            op.send(&avail);
        }
        self.presence_updated(&info);
        if session.inner.lock().unwrap().subscribers.is_empty() {
            self.send_encode(&pub_id, "screen-share-encode-pause");
        }
    }

    pub(super) fn handle_screen_share_mode_change(&self, p: &Arc<Peer>, data: protocol::ScreenShareModeChange) {
        let Some(mode) = data.mode else { return };
        let Some(session) = self.state.lock().unwrap().peers.get(&p.id).and_then(|e| e.screen_session.clone()) else {
            self.send_screen_error(p, "", ScreenShareReason::NotFound);
            return;
        };
        if session.mode.swap(encode_mode(mode), Ordering::AcqRel) == encode_mode(mode) {
            return;
        }
        session.request_keyframe();
        let pub_id = session.publisher_id();
        let others = Self::others(&self.state.lock().unwrap(), &p.id);
        broadcast(&others, &msg("screen-share-mode-changed", &protocol::ScreenShareModeChanged { publisher_id: &pub_id, mode }));
        log!("sfu: screen-share mode-change pub={pub_id} mode={}", if mode == ScreenShareMode::Motion { "motion" } else { "sharp" });
    }

    pub(super) fn handle_screen_share_stop(&self, p: &Arc<Peer>) {
        let session = self.state.lock().unwrap().peers.get(&p.id).and_then(|e| e.screen_session.clone());
        if let Some(s) = session {
            self.end_screen_share_session(&s, "publisher requested stop");
        }
    }

    /// Closes the publisher and every subscriber PC, broadcasts
    /// screen-share-ended and clears the publisher's flags. Idempotent.
    pub(super) fn end_screen_share_session(&self, session: &Arc<ScreenSession>, reason: &str) {
        let (subs, grace) = {
            let mut inner = session.inner.lock().unwrap();
            if inner.closed {
                return;
            }
            inner.closed = true;
            let subs: Vec<Arc<ScreenSub>> = inner.subscribers.drain().map(|(_, s)| s).collect();
            session.refresh_view(&inner);
            (subs, inner.grace.take())
        };
        if let Some(g) = grace {
            g.cancel();
        }
        session.publisher_pc.close();
        session.cancel.cancel();
        let pub_id = session.publisher_id();
        for s in &subs {
            s.pc.close();
            let mut st = self.state.lock().unwrap();
            if let Some(e) = st.peers.get_mut(&s.peer_id) {
                e.screen_subs.remove(&pub_id);
            }
        }
        let (info, all) = {
            let mut st = self.state.lock().unwrap();
            st.sessions_by_token.remove(&session.token);
            let info = st.peers.get_mut(&pub_id).map(|e| {
                if e.screen_session.as_ref().is_some_and(|s| Arc::ptr_eq(s, session)) {
                    e.screen_session = None;
                    e.screen_sharing = false;
                    e.screen_sharing_has_audio = false;
                    e.screen_sharing_video_codec = None;
                }
                e.info()
            });
            (info, Self::all(&st))
        };
        let ended = msg("screen-share-ended", &protocol::ScreenShareEnded { publisher_id: &pub_id });
        match info {
            Some(info) => {
                let info_msg = msg("peer-info", &info);
                for op in &all {
                    op.send(&ended);
                    if op.id != info.id {
                        op.send(&info_msg);
                    }
                }
                self.presence_updated(&info);
            }
            None => broadcast(&all, &ended),
        }
        log!("sfu: screen-share ended publisher={pub_id} reason={reason}");
    }

    /// Client-initiated offer: only screen-pub ICE restart after resume.
    pub(super) fn handle_client_offer(&self, p: &Arc<Peer>, env: SdpEnvelope) {
        if env.pc != PcKind::ScreenPub {
            log!("sfu: client offer with unsupported pc={:?} from {}", env.pc, p.id);
            return;
        }
        let Some(session) = self.state.lock().unwrap().peers.get(&p.id).and_then(|e| e.screen_session.clone()) else {
            log!("sfu: client offer screen-pub from {} with no session", p.id);
            self.send_screen_error(p, "", ScreenShareReason::NotFound);
            return;
        };
        let answer = SdpOffer::from_sdp_string(&env.sdp)
            .map_err(|e| e.to_string())
            .and_then(|offer| session.publisher_pc.with(|rtc| rtc.sdp_api().accept_offer(offer)).map_err(|e| e.to_string()));
        match answer {
            Ok(a) => {
                session.publisher_pc.refresh_ufrag();
                p.send_event("answer", &sdp_msg(PcKind::ScreenPub, "", "answer", a.to_sdp_string()))
            }
            Err(e) => {
                log!("sfu: client offer screen-pub ({}) set remote: {e}", p.id);
                self.send_screen_error(p, "", ScreenShareReason::Internal);
            }
        }
    }

    pub(super) fn handle_screen_share_subscribe(&self, sub: &Arc<Peer>, data: protocol::ScreenShareSubscribe) {
        let session = {
            let st = self.state.lock().unwrap();
            let session = st.peers.get(&data.publisher_id).and_then(|e| e.screen_session.clone());
            if session.is_some() && st.peers.get(&sub.id).is_some_and(|e| e.screen_subs.contains_key(&data.publisher_id)) {
                log!("sfu: screen-share-subscribe ({}→{}) duplicate, ignoring", sub.id, data.publisher_id);
                return;
            }
            session
        };
        let Some(session) = session else {
            self.send_screen_error(sub, &data.publisher_id, ScreenShareReason::NotFound);
            return;
        };
        let pub_id = session.publisher_id();
        // Without temporal filtering every layer is forwarded.
        let target = (data.preferred_temporal_layer.min(2)) as i32;

        let socket = self.cfg.mux.pick();
        let (rtc, ufrag) = rtc::build(&socket, true);
        let sess_weak = Arc::downgrade(&session);
        let mut offer = None;
        let entry = Arc::new_cyclic(|me: &Weak<ScreenSub>| {
            let handler: Weak<dyn PcHandler> = me.clone();
            let pc = Pc::spawn(rtc, socket, ufrag, handler);
            let (video_mid, audio_mid, o) = pc.with(|rtc| {
                let mut api = rtc.sdp_api();
                let v = api.add_media(MediaKind::Video, Direction::SendOnly, Some(pub_id.clone()), Some("screen-video".into()), None);
                let a = session
                    .has_system_audio
                    .then(|| api.add_media(MediaKind::Audio, Direction::SendOnly, Some(pub_id.clone()), Some("screen-audio".into()), None));
                (v, a, api.apply())
            });
            let pending = o.map(|(offer_sdp, pending)| {
                offer = Some(offer_sdp.to_sdp_string());
                pending
            });
            ScreenSub {
                peer_id: sub.id.clone(),
                pc,
                session: sess_weak,
                video_mid,
                audio_mid,
                video_pt: sub_video_pt(session.video_codec()),
                pending: Mutex::new(pending),
                layer: LayerCtl::new(target),
                audio_roc_base: OnceLock::new(),
                fwd: Mutex::new(SubForward { last_gen: 0, chain: ChainTracker::new(target as usize), seq: (random_u32() & 0xffff) as u64 }),
            }
        });
        let Some(offer) = offer else {
            log!("sfu: screen subscribe ({}→{pub_id}) create offer failed", sub.id);
            entry.pc.close();
            self.send_screen_error(sub, &pub_id, ScreenShareReason::Internal);
            return;
        };

        let first = {
            let mut inner = session.inner.lock().unwrap();
            if inner.closed {
                drop(inner);
                entry.pc.close();
                self.send_screen_error(sub, &pub_id, ScreenShareReason::NotFound);
                return;
            }
            inner.subscribers.insert(sub.id.clone(), entry.clone());
            session.refresh_view(&inner);
            inner.subscribers.len() == 1
        };
        let attached = {
            let mut st = self.state.lock().unwrap();
            match st.peers.get_mut(&sub.id) {
                Some(e) => {
                    e.screen_subs.insert(pub_id.clone(), entry.clone());
                    true
                }
                None => false,
            }
        };
        if !attached {
            self.remove_screen_sub(&sub.id, &pub_id, Some(&entry.pc), "subscriber gone");
            return;
        }
        sub.send_event("offer", &sdp_msg(PcKind::ScreenSub, &pub_id, "offer", offer));
        log!("sfu: screen subscribe sub={} pub={pub_id} temp={target} firstSub={first}", sub.id);
        if first {
            self.send_encode(&pub_id, "screen-share-encode-resume");
            session.request_keyframe();
        }
    }

    pub(super) fn screen_sub_answer(&self, p: &Arc<Peer>, publisher_id: &str, sdp: &str) {
        let Some(sub) = self.state.lock().unwrap().peers.get(&p.id).and_then(|e| e.screen_subs.get(publisher_id).cloned()) else {
            log!("sfu: screen-sub answer ({}→{publisher_id}) no PC", p.id);
            return;
        };
        let Some(pending) = sub.pending.lock().unwrap().take() else {
            log!("sfu: screen-sub set remote ({}→{publisher_id}): no offer pending", p.id);
            return;
        };
        let r = SdpAnswer::from_sdp_string(sdp)
            .map_err(|e| e.to_string())
            .and_then(|a| sub.pc.with(|rtc| rtc.sdp_api().accept_answer(pending, a)).map_err(|e| e.to_string()));
        if let Err(e) = r {
            log!("sfu: screen-sub set remote ({}→{publisher_id}): {e}", p.id);
        }
    }

    /// Tears down a viewer's PC for one publisher. Idempotent; `only` limits
    /// the teardown to that exact PC so a stale close can't kill a newer
    /// subscription.
    pub(super) fn remove_screen_sub(&self, sub_id: &str, publisher_id: &str, only: Option<&Arc<Pc>>, reason: &str) {
        let matches = |s: &Arc<ScreenSub>| only.is_none_or(|pc| Arc::ptr_eq(&s.pc, pc));
        let (sub, session) = {
            let mut st = self.state.lock().unwrap();
            let sub = st
                .peers
                .get_mut(sub_id)
                .and_then(|e| if e.screen_subs.get(publisher_id).is_some_and(matches) { e.screen_subs.remove(publisher_id) } else { None });
            let session = st.peers.get(publisher_id).and_then(|e| e.screen_session.clone());
            (sub, session)
        };
        if let Some(s) = &sub {
            s.pc.close();
        }
        let mut went_idle = false;
        if let Some(session) = &session {
            let mut inner = session.inner.lock().unwrap();
            let removed = if inner.subscribers.get(sub_id).is_some_and(matches) { inner.subscribers.remove(sub_id) } else { None };
            if removed.is_some() {
                session.refresh_view(&inner);
                went_idle = inner.subscribers.is_empty();
            }
            // Closing re-enters this function via the PC's on_closed.
            drop(inner);
            if let Some(s) = removed {
                s.pc.close();
            }
        }
        if went_idle {
            self.send_encode(publisher_id, "screen-share-encode-pause");
        }
        log!("sfu: screen unsubscribe ({sub_id}→{publisher_id}) {reason}");
    }

    /// Arms the reattach window after the publisher's WS closed. Resume
    /// cancels it; expiry ends the session.
    pub(super) fn start_screen_share_grace(&self, session: &Arc<ScreenSession>) {
        let token = CancellationToken::new();
        {
            let mut inner = session.inner.lock().unwrap();
            if inner.closed {
                return;
            }
            if let Some(prev) = inner.grace.replace(token.clone()) {
                prev.cancel();
            }
        }
        let room = self.arc();
        let session = session.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = token.cancelled() => {}
                _ = tokio::time::sleep(GRACE_PERIOD) => room.end_screen_share_session(&session, "grace expired"),
            }
        });
    }

    /// Rebinds a session orphaned by a WS drop (grace still armed) to the
    /// reconnected publisher. The token is the ownership proof.
    pub(super) fn handle_screen_share_resume(&self, p: &Arc<Peer>, token: &str) {
        let claimed = {
            let mut st = self.state.lock().unwrap();
            let Some(session) = st.sessions_by_token.get(token).cloned() else {
                drop(st);
                log!("sfu: screen-share-resume ({}): unknown token", p.id);
                self.send_screen_error(p, "", ScreenShareReason::InvalidToken);
                return;
            };
            let old = session.publisher_id();
            if st.peers.get(&old).is_some_and(|e| !Arc::ptr_eq(&e.peer, p)) {
                drop(st);
                log!("sfu: screen-share-resume ({}): token in use by {old}", p.id);
                self.send_screen_error(p, "", ScreenShareReason::InvalidToken);
                return;
            }
            let Some(entry) = st.peers.get_mut(&p.id) else { return };
            if entry.screen_session.as_ref().is_some_and(|s| !Arc::ptr_eq(s, &session)) {
                drop(st);
                log!("sfu: screen-share-resume ({}): peer already owns another session", p.id);
                self.send_screen_error(p, "", ScreenShareReason::AlreadyPublishing);
                return;
            }
            let mut inner = session.inner.lock().unwrap();
            if inner.closed {
                drop(inner);
                drop(st);
                self.send_screen_error(p, "", ScreenShareReason::InvalidToken);
                return;
            }
            // Claim before releasing the room lock so concurrent resumes see the new owner.
            *session.publisher_id.lock().unwrap() = p.id.clone();
            let grace = inner.grace.take();
            let stale: Vec<Arc<ScreenSub>> = inner.subscribers.drain().map(|(_, s)| s).collect();
            session.refresh_view(&inner);
            drop(inner);
            entry.screen_session = Some(session.clone());
            entry.screen_sharing = true;
            entry.screen_sharing_has_audio = session.has_system_audio;
            entry.screen_sharing_video_codec = Some(session.video_codec());
            (session, old, stale, grace)
        };
        let (session, old, stale, grace) = claimed;
        if let Some(g) = grace {
            g.cancel();
        }
        // Stale subscriber PCs were keyed by the old publisher ID; viewers
        // re-subscribe under the new one.
        for s in &stale {
            s.pc.close();
            let mut st = self.state.lock().unwrap();
            if let Some(e) = st.peers.get_mut(&s.peer_id) {
                e.screen_subs.remove(&old);
            }
        }
        let (info, others) = {
            let st = self.state.lock().unwrap();
            let Some(e) = st.peers.get(&p.id) else { return };
            (e.info(), Self::others(&st, &p.id))
        };
        let info_msg = msg("peer-info", &info);
        let ended = msg("screen-share-ended", &protocol::ScreenShareEnded { publisher_id: &old });
        let avail = msg(
            "screen-share-available",
            &protocol::ScreenShareAvailable {
                publisher_id: &p.id,
                has_system_audio: session.has_system_audio,
                video_codec: Some(session.video_codec()),
                mode: Some(session.mode()),
            },
        );
        for op in &others {
            op.send(&info_msg);
            op.send(&ended);
            op.send(&avail);
        }
        self.presence_updated(&info);
        p.send_event("screen-share-started", &protocol::ScreenShareStarted { session_token: &session.token });
        log!("sfu: screen-share-resume {old}→{} subs-teardown={}", p.id, stale.len());
    }
}

/// Polls every viewer's loss and steps temporal layers with hysteresis; one
/// PLI per tick even if several viewers changed.
async fn auto_downgrade_loop(session: Arc<ScreenSession>) {
    let mut tick = tokio::time::interval(AUTO_DOWNGRADE_POLL);
    tick.tick().await;
    loop {
        tokio::select! {
            _ = session.cancel.cancelled() => return,
            _ = tick.tick() => {}
        }
        let pol = policy(session.mode());
        let pub_id = session.publisher_id();
        let now = SystemTime::now();
        let mut changed = false;
        for sub in session.subscribers().iter() {
            changed |= eval_auto_downgrade(&sub.layer, now, &sub.peer_id, &pub_id, pol);
        }
        if changed {
            session.request_keyframe();
        }
    }
}

/// Advances the hysteresis windows from the last loss sample; returns
/// whether the target temporal layer changed.
pub fn eval_auto_downgrade(sub: &LayerCtl, now: SystemTime, sub_id: &str, pub_id: &str, p: ModePolicy) -> bool {
    let loss = sub.loss_per_mille.load(Ordering::Acquire);
    let target = sub.target_temp.load(Ordering::Acquire);
    let now_ns = now.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_nanos() as i64;
    if loss >= p.high_loss_pm {
        sub.low_loss_since.store(0, Ordering::Release);
        if target <= p.floor_temp {
            sub.high_loss_since.store(0, Ordering::Release);
            return false;
        }
        let since = sub.high_loss_since.load(Ordering::Acquire);
        if since == 0 {
            sub.high_loss_since.store(now_ns, Ordering::Release);
            return false;
        }
        if Duration::from_nanos((now_ns - since).max(0) as u64) < p.high_window {
            return false;
        }
        sub.set_target_temp(target - 1);
        sub.high_loss_since.store(0, Ordering::Release);
        log!("sfu: auto-downgrade sub={sub_id} pub={pub_id} temp={target}→{} loss={loss}‰", target - 1);
        return true;
    }
    if loss <= p.low_loss_pm {
        sub.high_loss_since.store(0, Ordering::Release);
        if target >= 2 {
            sub.low_loss_since.store(0, Ordering::Release);
            return false;
        }
        let since = sub.low_loss_since.load(Ordering::Acquire);
        if since == 0 {
            sub.low_loss_since.store(now_ns, Ordering::Release);
            return false;
        }
        if Duration::from_nanos((now_ns - since).max(0) as u64) < p.low_window {
            return false;
        }
        sub.set_target_temp(target + 1);
        sub.low_loss_since.store(0, Ordering::Release);
        log!("sfu: auto-upgrade sub={sub_id} pub={pub_id} temp={target}→{} loss={loss}‰", target + 1);
        return true;
    }
    // Mid band: both streaks reset so the next excursion must re-prove itself.
    sub.high_loss_since.store(0, Ordering::Release);
    sub.low_loss_since.store(0, Ordering::Release);
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(target: i32, loss: u32) -> LayerCtl {
        let l = LayerCtl::new(target);
        l.loss_per_mille.store(loss, Ordering::Release);
        l
    }

    fn eval(l: &LayerCtl, t: SystemTime, p: ModePolicy) -> bool {
        eval_auto_downgrade(l, t, "s", "p", p)
    }

    fn temp(l: &LayerCtl) -> i32 {
        l.target_temp.load(Ordering::Acquire)
    }

    #[test]
    fn downgrade_needs_sustained_loss() {
        let l = sub(2, 120);
        let t0 = SystemTime::now();
        assert!(!eval(&l, t0, SHARP));
        assert_ne!(l.high_loss_since.load(Ordering::Acquire), 0);
        assert!(!eval(&l, t0 + Duration::from_secs(1), SHARP));
        assert_eq!(temp(&l), 2);
        assert!(eval(&l, t0 + SHARP.high_window + Duration::from_millis(10), SHARP));
        assert_eq!(temp(&l), 1);
        assert_eq!(l.high_loss_since.load(Ordering::Acquire), 0);
        assert_eq!(l.chain_gen.load(Ordering::Acquire), 1, "forwarder re-arms its chain");
    }

    #[test]
    fn floors() {
        let l = sub(0, 200);
        let t0 = SystemTime::now();
        assert!(!eval(&l, t0, SHARP));
        assert!(!eval(&l, t0 + SHARP.high_window + Duration::from_secs(1), SHARP));
        assert_eq!(l.high_loss_since.load(Ordering::Acquire), 0);

        let m = sub(1, 300);
        eval(&m, t0, MOTION);
        assert!(!eval(&m, t0 + MOTION.high_window + Duration::from_secs(1), MOTION));
        assert_eq!(temp(&m), 1, "motion never drops below T1");
    }

    #[test]
    fn upgrade_after_calm() {
        let l = sub(1, 10);
        let t0 = SystemTime::now();
        eval(&l, t0, SHARP);
        assert_ne!(l.low_loss_since.load(Ordering::Acquire), 0);
        assert!(!eval(&l, t0 + Duration::from_secs(2), SHARP));
        assert!(eval(&l, t0 + SHARP.low_window + Duration::from_millis(10), SHARP));
        assert_eq!(temp(&l), 2);
    }

    #[test]
    fn mid_band_and_policy_differences() {
        let l = sub(2, 120);
        let t0 = SystemTime::now();
        eval(&l, t0, SHARP);
        l.loss_per_mille.store(25, Ordering::Release);
        eval(&l, t0 + Duration::from_secs(1), SHARP);
        assert_eq!((l.high_loss_since.load(Ordering::Acquire), l.low_loss_since.load(Ordering::Acquire)), (0, 0));

        // 6% is high for sharp, mid-band for motion.
        let m = sub(2, 60);
        eval(&m, t0, MOTION);
        assert!(!eval(&m, t0 + MOTION.high_window + Duration::from_secs(1), MOTION));
        assert_eq!(temp(&m), 2);

        // Flapping across the threshold never completes a window.
        let f = sub(2, 120);
        for i in 0..10 {
            f.loss_per_mille.store(if i % 2 == 0 { 120 } else { 20 }, Ordering::Release);
            assert!(!eval(&f, t0 + Duration::from_secs(i), SHARP));
        }
    }

    #[test]
    fn sdp_sections() {
        let s = sections(
            "v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=mid:0\r\nm=video 9 UDP/TLS/RTP/SAVPF 45 46 98\r\na=mid:1\r\nm=video 0 UDP/TLS/RTP/SAVPF 98\r\na=mid:2\r\n",
        );
        assert_eq!(s.len(), 3);
        assert_eq!((s[1].kind.as_str(), s[1].mid.as_str(), s[1].pts.as_slice(), s[1].rejected), ("video", "1", &[45u8, 46, 98][..], false));
        assert!(s[2].rejected);
    }
}

/// Maps a publisher's extended sequence number onto a subscriber SSRC that
/// started at rollover zero (as the receiver assumes); `base` is fixed by the
/// first forwarded packet.
fn rebase_roc(base: &OnceLock<u64>, seq: SeqNo) -> SeqNo {
    let base = *base.get_or_init(|| *seq & !0xffff);
    SeqNo::from(seq.saturating_sub(base))
}

#[cfg(test)]
mod roc_tests {
    use super::*;

    #[test]
    fn rebase_keeps_wire_seq_and_drops_rollover() {
        let base = OnceLock::new();
        assert_eq!(*rebase_roc(&base, SeqNo::from(3 * 65536 + 65535)), 65535);
        assert_eq!(*rebase_roc(&base, SeqNo::from(4 * 65536 + 2)), 65536 + 2, "rolls over once on the subscriber side");
    }
}
