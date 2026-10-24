//! Audio PeerConnection per voice peer. The SFU is always the offerer: one
//! recvonly m-line for the peer's microphone plus a pool of sendonly "slot"
//! m-lines with fixed msids (`slot-N`). Other peers' voices are assigned to
//! slots and announced with `audio-slots`, so joins and leaves don't touch
//! the SDP; the pool only grows, in steps, when every slot is taken.

use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};
use std::time::Instant;

use str0m::Event;
use str0m::change::{SdpAnswer, SdpPendingOffer};
use str0m::media::{Direction, MediaKind, Mid, Pt};
use str0m::rtp::{RtpPacket, RtpWrite, SeqNo};
use tokio_util::sync::CancellationToken;

use super::Room;
use super::net::{Pc, PcHandler};
use super::protocol::{AudioSlot, AudioSlots, PcKind, SdpEnvelope};
use super::rtc::{self, PT_OPUS};
use crate::log;

/// Slots in the first offer; enough that typical rooms never renegotiate.
const INITIAL_SLOTS: usize = 8;
/// Slots added at once when the pool runs out.
const SLOT_GROWTH: usize = 4;
/// Opus at 48 kHz, 20 ms frames.
const SAMPLES_PER_FRAME: u32 = 960;

/// One published audio track and the slots it is forwarded into.
pub struct AudioTrack {
    pub owner: String,
    /// Copy-on-write so the forward path never holds the lock while it
    /// writes into a subscriber PC.
    subs: RwLock<Arc<Vec<Arc<Sub>>>>,
}

/// This track's place in one subscriber's slot.
struct Sub {
    pc: Arc<Pc>,
    mid: Mid,
    out: Arc<SlotStream>,
    base: OnceLock<SlotBase>,
}

/// Last packet written into a slot, whoever owned it: (seq, timestamp, when).
/// A new owner continues from here, so the receiver's jitter buffer sees one
/// uninterrupted stream per slot.
#[derive(Default)]
struct SlotStream {
    last: Mutex<Option<(u64, u32, Instant)>>,
}

/// Maps the owner's stream onto the slot's, fixed by its first packet there.
#[derive(Clone, Copy)]
struct SlotBase {
    in_seq: u64,
    in_ts: u32,
    out_seq: u64,
    out_ts: u32,
}

impl Sub {
    /// (seq, timestamp, marker) in the slot's stream for an owner packet;
    /// None for one that predates this owner's first packet in the slot.
    fn map(&self, in_seq: u64, in_ts: u32, marker: bool, now: Instant) -> Option<(u64, u32, bool)> {
        let mut last = self.out.last.lock().unwrap();
        let mut first = false;
        let b = *self.base.get_or_init(|| {
            first = true;
            // One frame (or the real silence) after the previous owner.
            let (out_seq, out_ts) = match *last {
                Some((seq, ts, at)) => {
                    let gap = (now.duration_since(at).as_millis() as u32).saturating_mul(48).max(SAMPLES_PER_FRAME);
                    (seq + 1, ts.wrapping_add(gap))
                }
                None => (1, 0),
            };
            SlotBase { in_seq, in_ts, out_seq, out_ts }
        });
        let seq = b.out_seq + in_seq.checked_sub(b.in_seq)?;
        let ts = b.out_ts.wrapping_add(in_ts.wrapping_sub(b.in_ts));
        if last.is_none_or(|(s, _, _)| seq > s) {
            *last = Some((seq, ts, now));
        }
        // A new owner starts a talkspurt for the receiver's jitter buffer.
        Some((seq, ts, first || marker))
    }
}

impl AudioTrack {
    fn new(owner: &str) -> Arc<AudioTrack> {
        Arc::new(AudioTrack { owner: owner.to_string(), subs: RwLock::default() })
    }

    fn update(&self, f: impl FnOnce(&mut Vec<Arc<Sub>>)) {
        let mut guard = self.subs.write().unwrap();
        let mut subs = guard.as_ref().clone();
        f(&mut subs);
        *guard = Arc::new(subs);
    }

    fn add_sub(&self, pc: &Arc<Pc>, mid: Mid, out: &Arc<SlotStream>) {
        self.update(|subs| subs.push(Arc::new(Sub { pc: pc.clone(), mid, out: out.clone(), base: OnceLock::new() })));
    }

    fn remove_sub(&self, pc: &Arc<Pc>) {
        self.update(|subs| subs.retain(|s| !Arc::ptr_eq(&s.pc, pc)));
    }

    /// Writes one packet into every subscriber's slot. Publisher header
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
            let Some((seq, ts, marker)) = sub.map(*pkt.seq_no, pkt.header.timestamp, pkt.header.marker, Instant::now()) else { continue };
            sub.pc.write(|rtc| {
                if let Some(tx) = rtc.direct_api().stream_tx_by_mid(sub.mid, None) {
                    tx.write_rtp(RtpWrite::new(pt, SeqNo::from(seq), ts, pkt.timestamp, pkt.payload.clone()).marker(marker));
                }
            });
        }
        if dead {
            self.update(|subs| subs.retain(|s| !s.pc.is_closed()));
        }
    }
}

struct Slot {
    mid: Mid,
    stream_id: String,
    track: Option<Arc<AudioTrack>>,
    /// The client has answered the offer that added it.
    open: bool,
    out: Arc<SlotStream>,
}

#[derive(Default)]
struct SyncState {
    pending: Option<SdpPendingOffer>,
    slots: Vec<Slot>,
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
            room.unpublish_track(t);
        }
        for t in self.sync.lock().unwrap().slots.iter().filter_map(|s| s.track.as_ref()) {
            t.remove_sub(&self.pc);
        }
    }

    /// Points the slots at the room's current tracks (all but our own).
    /// While an offer is in flight this waits for the answer, which syncs.
    pub fn sync(&self) {
        let mut st = self.sync.lock().unwrap();
        if st.pending.is_none() {
            self.reconcile(&mut st);
        }
    }

    /// Applies the client's answer, then syncs with the slots it opened.
    pub fn accept_answer(&self, sdp: &str) {
        let mut st = self.sync.lock().unwrap();
        let Some(pending) = st.pending.take() else {
            log!("sfu: set remote audio ({}): no offer pending", self.peer_id);
            return;
        };
        let accepted = SdpAnswer::from_sdp_string(sdp)
            .map_err(|e| e.to_string())
            .and_then(|answer| self.pc.with(|rtc| rtc.sdp_api().accept_answer(pending, answer)).map_err(|e| e.to_string()));
        match accepted {
            Ok(()) => st.slots.iter_mut().for_each(|s| s.open = true),
            Err(e) => {
                log!("sfu: set remote audio ({}): {e}", self.peer_id);
                // The offer's m-lines never reached the session.
                st.slots.retain(|s| s.open);
            }
        }
        self.reconcile(&mut st);
    }

    /// Frees the slots of departed tracks, puts new tracks into open slots and
    /// offers more slots when they run out. Runs under the sync lock, so the
    /// client sees `audio-slots` in the order the pool changed.
    fn reconcile(&self, st: &mut SyncState) {
        let Some(room) = self.room.upgrade() else { return };
        if self.pc.is_closed() || self.cancel.is_cancelled() {
            return;
        }
        let tracks = room.state.lock().unwrap().tracks.clone();
        let want: Vec<&Arc<AudioTrack>> = tracks.values().filter(|t| t.owner != self.peer_id).collect();
        // (slot stream ID, owner peer ID or None when freed).
        let mut assignments: Vec<(String, Option<String>)> = Vec::new();
        for s in &mut st.slots {
            let Some(t) = &s.track else { continue };
            if !want.iter().any(|w| Arc::ptr_eq(w, t)) {
                t.remove_sub(&self.pc);
                assignments.push((s.stream_id.clone(), None));
                s.track = None;
            }
        }
        let mut new: Vec<&Arc<AudioTrack>> =
            want.into_iter().filter(|t| !st.slots.iter().any(|s| s.track.as_ref().is_some_and(|a| Arc::ptr_eq(a, t)))).collect();
        new.sort_by(|a, b| a.owner.cmp(&b.owner));

        let mut new = new.into_iter();
        let mut subscribe = Vec::new();
        for s in st.slots.iter_mut().filter(|s| s.track.is_none()) {
            let Some(t) = new.next() else { break };
            s.track = Some(t.clone());
            assignments.push((s.stream_id.clone(), Some(t.owner.clone())));
            subscribe.push((t.clone(), s.mid, s.out.clone()));
        }
        // Owner announcements go out before forwarding starts, so the client
        // knows whose voice a slot carries by the time it arrives.
        if !assignments.is_empty() {
            let slots: Vec<AudioSlot> = assignments.iter().map(|(slot, peer)| AudioSlot { slot, peer_id: peer.as_deref() }).collect();
            self.send("audio-slots", &AudioSlots { slots: &slots });
        }
        for (t, mid, out) in subscribe {
            t.add_sub(&self.pc, mid, &out);
        }

        let waiting = new.len();
        let grow = if st.slots.is_empty() { INITIAL_SLOTS.max(waiting) } else { waiting.div_ceil(SLOT_GROWTH) * SLOT_GROWTH };
        if grow == 0 {
            return;
        }
        let first_offer = st.slots.is_empty();
        let offer = self.pc.with(|rtc| {
            let mut api = rtc.sdp_api();
            if first_offer {
                api.add_media(MediaKind::Audio, Direction::RecvOnly, None, None, None);
            }
            for _ in 0..grow {
                let stream_id = format!("slot-{}", st.slots.len());
                let mid =
                    api.add_media(MediaKind::Audio, Direction::SendOnly, Some(stream_id.clone()), Some(format!("{stream_id}-audio")), None);
                st.slots.push(Slot { mid, stream_id, track: None, open: false, out: Arc::default() });
            }
            api.apply()
        });
        let Some((offer, pending)) = offer else {
            st.slots.retain(|s| s.open);
            return;
        };
        st.pending = Some(pending);
        self.send(
            "offer",
            &SdpEnvelope { pc: PcKind::Audio, publisher_id: String::new(), kind: "offer".into(), sdp: offer.to_sdp_string() },
        );
    }

    fn send<T: serde::Serialize>(&self, event: &str, data: &T) {
        if let Some(p) = self.room.upgrade().and_then(|r| r.peer_handle(&self.peer_id)) {
            p.send_event(event, data);
        }
    }
}

impl PcHandler for AudioPeer {
    fn on_event(&self, _pc: &Arc<Pc>, ev: Event) {
        if let Event::RtpPacket(pkt) = ev {
            // Forwarded as-is, padding included: dropping any packet would
            // show up as loss downstream.
            let track = self.track.get_or_init(|| {
                let t = AudioTrack::new(&self.peer_id);
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
    use std::time::Duration;

    use super::*;

    fn sub(out: &Arc<SlotStream>, pc: &Arc<Pc>) -> Sub {
        Sub { pc: pc.clone(), mid: Mid::from("a"), out: out.clone(), base: OnceLock::new() }
    }

    #[tokio::test]
    async fn slot_stream_continues_across_owners() {
        let mux = super::super::net::Mux::bind("127.0.0.1".parse().unwrap(), 0, 0).await.unwrap();
        let socket = mux.pick();
        let (rtc, ufrag) = rtc::build(&socket, false);
        let handler: Weak<dyn PcHandler> = Weak::<AudioPeer>::new();
        let pc = Pc::spawn(rtc, socket, ufrag, handler);
        let out = Arc::new(SlotStream::default());
        let t0 = Instant::now();

        // First owner: its own numbering, anywhere in u64/u32 space.
        let a = sub(&out, &pc);
        assert_eq!(a.map(70_000, 4_000_000_000, false, t0), Some((1, 0, true)), "slot starts at 1 / 0, marked");
        assert_eq!(a.map(70_001, 4_000_000_960, false, t0), Some((2, 960, false)));
        assert_eq!(a.map(69_999, 3_999_999_040, false, t0), None, "older than the owner's start");

        // Next owner, 100 ms later: seq continues, timestamp skips the silence.
        let b = sub(&out, &pc);
        let t1 = t0 + Duration::from_millis(100);
        assert_eq!(b.map(5, 123, false, t1), Some((3, 960 + 4800, true)));
        assert_eq!(b.map(6, 123 + 960, true, t1), Some((4, 960 + 4800 + 960, true)));
        // Reordered packet of the new owner doesn't move the slot backwards.
        assert_eq!(b.map(8, 123 + 3 * 960, false, t1), Some((6, 960 + 4800 + 3 * 960, false)));
        assert_eq!(b.map(7, 123 + 2 * 960, false, t1), Some((5, 960 + 4800 + 2 * 960, false)));
        assert_eq!(out.last.lock().unwrap().unwrap().0, 6);
    }
}
