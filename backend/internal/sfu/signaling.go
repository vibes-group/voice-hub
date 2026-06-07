package sfu

import (
	"context"
	"encoding/json"
	"errors"
	"log"
	"maps"
	"sync"
	"time"

	"github.com/pion/webrtc/v4"

	"voice-hub/backend/internal/sfu/protocol"
)

// signalPeerConnections renegotiates each peer so it has senders for all
// current room tracks (minus its own). Optimistic retry pattern from sfu-ws.
func (r *Room) signalPeerConnections() {
	if r.runSyncAttempts() {
		return
	}
	if !r.resyncPending.CompareAndSwap(false, true) {
		return
	}
	go r.deferredResyncLoop()
}

// runSyncAttempts runs up to maxAttempts inline passes of attemptSync.
// Returns true if any pass settled cleanly (no retry needed). Returns
// false only when all attempts exhausted with attemptSync still wanting
// to retry.
func (r *Room) runSyncAttempts() bool {
	const maxAttempts = 5
	for range maxAttempts {
		if !r.attemptSync() {
			return true
		}
	}
	return false
}

// deferredResyncLoop is the body of the single in-flight retry
// goroutine. It keeps issuing maxAttempts passes every 3s until one
// settles or the room closes. Single-flight is enforced by
// r.resyncPending; this goroutine clears the flag only on exit so any
// concurrent exhaustion correctly folds into the in-flight retry rather
// than spawning a duplicate.
//
// Earlier we recursed into signalPeerConnections() from this goroutine,
// which silently stopped scheduling further retries: the recursive call
// hit CompareAndSwap(false, true) while the outer goroutine still held
// the flag, so it returned without queuing another pass, then the outer
// defer cleared the flag — and nothing else was scheduled. A peer stuck
// for longer than one 3s window would leave the room un-resynced.
func (r *Room) deferredResyncLoop() {
	defer r.resyncPending.Store(false)
	for {
		time.Sleep(3 * time.Second)
		if r.closed.Load() {
			return
		}
		if r.runSyncAttempts() {
			return
		}
	}
}

func (r *Room) attemptSync() (retry bool) {
	r.mu.Lock()
	peers := make([]*peer, 0, len(r.peers))
	for _, p := range r.peers {
		peers = append(peers, p)
	}
	tracks := make(map[string]*webrtc.TrackLocalStaticRTP, len(r.tracks))
	maps.Copy(tracks, r.tracks)
	r.mu.Unlock()

	for _, p := range peers {
		if r.syncOnePeer(p, tracks) {
			return true
		}
	}
	return false
}

// syncBufs holds the per-iteration scratch maps used by syncOnePeer.
// Pooled to avoid 2 map allocations per peer per attemptSync call,
// which is the dominant alloc source during a join/leave storm.
type syncBufs struct {
	want map[string]bool
	have map[string]bool
}

var syncBufsPool = sync.Pool{
	New: func() any {
		return &syncBufs{
			want: make(map[string]bool, 16),
			have: make(map[string]bool, 16),
		}
	},
}

func (r *Room) syncOnePeer(p *peer, tracks map[string]*webrtc.TrackLocalStaticRTP) (retry bool) {
	// Lurker peers have no PeerConnection; nothing to sync.
	if p.pc == nil {
		return false
	}
	p.syncMu.Lock()
	defer p.syncMu.Unlock()
	// Peer context already cancelled (e.g. PC failed, outq full, ws closed):
	// removePeer will fire from ServeWS's defer shortly. Skipping here avoids
	// log spam from CreateOffer/AddTrack on a doomed PC during the brief
	// window before defer runs.
	if p.ctx.Err() != nil {
		return false
	}
	if p.pc.ConnectionState() == webrtc.PeerConnectionStateClosed {
		r.removePeer(p.id)
		return true
	}
	// Offer-in-flight: SetLocalDescription rejects while signaling is
	// have-local-offer. Mark a re-sync request; the answer handler drains
	// it once the remote answer lands.
	if p.pc.SignalingState() != webrtc.SignalingStateStable {
		p.syncPending.Store(true)
		return false
	}

	bufs := syncBufsPool.Get().(*syncBufs)
	defer syncBufsPool.Put(bufs)
	want := bufs.want
	have := bufs.have
	clear(want)
	clear(have)

	for key, t := range tracks {
		owner := ownerOf(key)
		if owner == p.id {
			continue
		}
		want[t.ID()] = true
	}

	for _, sender := range p.pc.GetSenders() {
		t := sender.Track()
		if t == nil {
			continue
		}
		id := t.ID()
		have[id] = true
		if !want[id] {
			if err := p.pc.RemoveTrack(sender); err != nil {
				log.Printf("sfu: syncOnePeer (%s) RemoveTrack: %v", p.id, err)
				return true
			}
		}
	}

	for _, recv := range p.pc.GetReceivers() {
		t := recv.Track()
		if t == nil {
			continue
		}
		have[t.ID()] = true
	}

	for key, t := range tracks {
		owner := ownerOf(key)
		if owner == p.id {
			continue
		}
		if have[t.ID()] {
			continue
		}
		if _, err := p.pc.AddTrack(t); err != nil {
			log.Printf("sfu: syncOnePeer (%s) AddTrack: %v", p.id, err)
			return true
		}
	}

	offer, err := p.pc.CreateOffer(nil)
	if err != nil {
		log.Printf("sfu: syncOnePeer (%s) CreateOffer: %v", p.id, err)
		return true
	}
	if err := p.pc.SetLocalDescription(offer); err != nil {
		log.Printf("sfu: syncOnePeer (%s) SetLocalDescription: %v", p.id, err)
		return true
	}
	sd, err := json.Marshal(protocol.OfferEnvelope{
		PC:                 protocol.PCAudio,
		SessionDescription: offer,
	})
	if err != nil {
		log.Printf("sfu: syncOnePeer (%s) marshal offer: %v", p.id, err)
		return true
	}
	if err := p.write(protocol.Envelope{Event: "offer", Data: sd}); err != nil {
		if !errors.Is(err, context.Canceled) {
			log.Printf("sfu: syncOnePeer (%s) send offer: %v", p.id, err)
		}
		return true
	}
	return false
}
