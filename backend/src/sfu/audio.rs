//! Audio PeerConnection per voice peer. The SFU is always the offerer: the
//! first offer carries one recvonly m-line for the peer's microphone, later
//! offers add a sendonly m-line per other peer's track and stop the m-lines
//! of tracks that went away.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};

use str0m::Event;
use str0m::change::{SdpAnswer, SdpPendingOffer};
use str0m::media::{Direction, MediaKind, Mid, Pt};
use str0m::rtp::{RtpPacket, RtpWrite, SeqNo};
use tokio_util::sync::CancellationToken;

use super::net::{Pc, PcHandler};
use super::protocol::{PcKind, SdpEnvelope};
use super::rtc::{self, PT_OPUS};
use super::{Room, track_key};
use crate::log;

/// One published audio track and the subscriber m-lines it fans out to.
pub struct AudioTrack {
    pub key: String,
    owner: String,
    /// Copy-on-write so the forward path never holds the lock while it
    /// writes into a subscriber PC.
    subs: RwLock<Arc<Vec<Arc<Sub>>>>,
}

struct Sub {
    pc: Arc<Pc>,
    mid: Mid,
    /// See `rebase_roc`.
    roc_base: OnceLock<u64>,
}

impl AudioTrack {
    fn new(owner: &str, kind: &str) -> Arc<AudioTrack> {
        Arc::new(AudioTrack { key: track_key(owner, kind), owner: owner.to_string(), subs: RwLock::default() })
    }

    fn update(&self, f: impl FnOnce(&mut Vec<Arc<Sub>>)) {
        let mut guard = self.subs.write().unwrap();
        let mut subs = guard.as_ref().clone();
        f(&mut subs);
        *guard = Arc::new(subs);
    }

    pub fn add_sub(&self, pc: &Arc<Pc>, mid: Mid) {
        self.update(|subs| {
            if !subs.iter().any(|s| Arc::ptr_eq(&s.pc, pc) && s.mid == mid) {
                subs.push(Arc::new(Sub { pc: pc.clone(), mid, roc_base: OnceLock::new() }));
            }
        });
    }

    pub fn remove_sub(&self, pc: &Arc<Pc>) {
        self.update(|subs| subs.retain(|s| !Arc::ptr_eq(&s.pc, pc)));
    }

    /// Writes one packet to every subscriber. Sequence numbers (rebased to
    /// rollover zero) and timestamps pass through; publisher header
    /// extensions are dropped since their IDs were negotiated with the
    /// publisher, not the subscriber.
    pub fn forward(&self, pkt: &RtpPacket, pt: Pt) {
        let subs = self.subs.read().unwrap().clone();
        let mut dead = false;
        for sub in subs.iter() {
            if sub.pc.is_closed() {
                dead = true;
                continue;
            }
            let seq = rebase_roc(&sub.roc_base, pkt.seq_no);
            sub.pc.write(|rtc| {
                if let Some(tx) = rtc.direct_api().stream_tx_by_mid(sub.mid, None) {
                    tx.write_rtp(
                        RtpWrite::new(pt, seq, pkt.header.timestamp, pkt.timestamp, pkt.payload.clone()).marker(pkt.header.marker),
                    );
                }
            });
        }
        if dead {
            self.update(|subs| subs.retain(|s| !s.pc.is_closed()));
        }
    }
}

/// Maps a publisher's extended sequence number onto a subscriber SSRC that
/// started at rollover zero; `base` is fixed by the first forwarded packet.
pub(super) fn rebase_roc(base: &OnceLock<u64>, seq: SeqNo) -> SeqNo {
    let base = *base.get_or_init(|| *seq & !0xffff);
    SeqNo::from(seq.saturating_sub(base))
}

struct OutTrack {
    track: Arc<AudioTrack>,
    mid: Mid,
    open: bool,
}

#[derive(Default)]
struct SyncState {
    pending: Option<SdpPendingOffer>,
    out: HashMap<String, OutTrack>,
    initialized: bool,
    /// A sync was skipped mid-negotiation; rerun once the answer lands.
    resync: bool,
}

pub struct AudioPeer {
    pub pc: Arc<Pc>,
    peer_id: String,
    room: Weak<Room>,
    cancel: CancellationToken,
    sync: Mutex<SyncState>,
    /// This peer's own microphone track, published on its first packet.
    track: OnceLock<Arc<AudioTrack>>,
}

impl AudioPeer {
    pub fn new(room: &Arc<Room>, peer_id: String, cancel: CancellationToken) -> Arc<AudioPeer> {
        let socket = room.cfg.mux.pick();
        let (rtc, ufrag) = rtc::build(&socket, false);
        Arc::new_cyclic(|me: &Weak<AudioPeer>| {
            let handler: Weak<dyn PcHandler> = me.clone();
            AudioPeer {
                pc: Pc::spawn(rtc, socket, ufrag, handler),
                peer_id,
                room: Arc::downgrade(room),
                cancel,
                sync: Mutex::default(),
                track: OnceLock::new(),
            }
        })
    }

    pub fn close(&self) {
        self.pc.close();
        if let (Some(t), Some(room)) = (self.track.get(), self.room.upgrade()) {
            room.unpublish_track(&t.key, t);
        }
        for o in self.sync.lock().unwrap().out.values() {
            o.track.remove_sub(&self.pc);
        }
    }

    /// Reconciles this PC's outgoing m-lines with `tracks` (all room tracks
    /// but our own) and sends an offer if anything changed.
    pub fn sync(&self, tracks: &HashMap<String, Arc<AudioTrack>>) {
        if self.pc.is_closed() || self.cancel.is_cancelled() {
            return;
        }
        let mut guard = self.sync.lock().unwrap();
        // Offer in flight: str0m (like any JSEP stack) can't start another.
        if guard.pending.is_some() {
            guard.resync = true;
            return;
        }
        let want: HashMap<&String, &Arc<AudioTrack>> = tracks.iter().filter(|(_, t)| t.owner != self.peer_id).collect();
        let st = &mut *guard;
        // Unsubscribed after the Rtc lock is released: the forward path takes
        // the track's lock, then this PC's.
        let mut stopped = Vec::new();
        let offer = self.pc.with(|rtc| {
            let mut api = rtc.sdp_api();
            if !st.initialized {
                api.add_media(MediaKind::Audio, Direction::RecvOnly, None, None, None);
            }
            let mut changed = false;
            st.out.retain(|key, o| {
                let keep = want.get(key).is_some_and(|t| Arc::ptr_eq(t, &o.track));
                if !keep {
                    api.stop_media(o.mid);
                    stopped.push(o.track.clone());
                    changed = true;
                }
                keep
            });
            for (key, t) in &want {
                if st.out.contains_key(*key) {
                    continue;
                }
                // StreamID = owner peer ID, so the client groups tracks by publisher.
                let mid =
                    api.add_media(MediaKind::Audio, Direction::SendOnly, Some(t.owner.clone()), Some(format!("{}-audio", t.owner)), None);
                st.out.insert((*key).clone(), OutTrack { track: (*t).clone(), mid, open: false });
                changed = true;
            }
            if st.initialized && !changed {
                return None;
            }
            api.apply()
        });
        for t in stopped {
            t.remove_sub(&self.pc);
        }
        let Some((offer, pending)) = offer else { return };
        st.pending = Some(pending);
        st.initialized = true;
        drop(guard);
        if let Some(p) = self.room.upgrade().and_then(|r| r.state.lock().unwrap().peers.get(&self.peer_id).map(|e| e.peer.clone())) {
            p.send_event(
                "offer",
                &SdpEnvelope { pc: PcKind::Audio, publisher_id: String::new(), kind: "offer".into(), sdp: offer.to_sdp_string() },
            );
        }
    }

    /// Applies the client's answer. Returns whether a skipped sync is due.
    pub fn accept_answer(&self, sdp: &str) -> bool {
        let mut st = self.sync.lock().unwrap();
        let Some(pending) = st.pending.take() else {
            log!("sfu: set remote audio ({}): no offer pending", self.peer_id);
            return false;
        };
        let answer = match SdpAnswer::from_sdp_string(sdp) {
            Ok(a) => a,
            Err(e) => {
                log!("sfu: set remote audio ({}): {e}", self.peer_id);
                return std::mem::take(&mut st.resync);
            }
        };
        if let Err(e) = self.pc.with(|rtc| rtc.sdp_api().accept_answer(pending, answer)) {
            log!("sfu: set remote audio ({}): {e}", self.peer_id);
            return std::mem::take(&mut st.resync);
        }
        for o in st.out.values_mut().filter(|o| !o.open) {
            o.open = true;
            o.track.add_sub(&self.pc, o.mid);
        }
        std::mem::take(&mut st.resync)
    }
}

impl PcHandler for AudioPeer {
    fn on_event(&self, _pc: &Arc<Pc>, ev: Event) {
        if let Event::RtpPacket(pkt) = ev {
            // Forwarded as-is, padding included: seq numbers pass through,
            // so dropping any packet would show up as loss downstream.
            let track = self.track.get_or_init(|| {
                let t = AudioTrack::new(&self.peer_id, "audio");
                if let Some(room) = self.room.upgrade() {
                    room.publish_track(t.clone());
                }
                t
            });
            track.forward(&pkt, Pt::new_with_value(PT_OPUS));
        }
    }

    fn on_closed(&self, _pc: &Arc<Pc>) {
        self.cancel.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebase_keeps_wire_seq_and_drops_rollover() {
        let base = OnceLock::new();
        let first = rebase_roc(&base, SeqNo::from(3 * 65536 + 65535));
        assert_eq!(*first, 65535);
        assert_eq!(*rebase_roc(&base, SeqNo::from(4 * 65536 + 2)), 65536 + 2, "rolls over once on the subscriber side");
    }
}
