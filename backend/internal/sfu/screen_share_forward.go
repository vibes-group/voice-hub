package sfu

import (
	"errors"
	"io"
	"log"

	"github.com/pion/rtp"
	"github.com/pion/webrtc/v4"

	"voice-hub/backend/internal/sfu/dd"
)

// forwardVideo reads RTP from the publisher's remote video track and writes
// to every active subscriber's per-sub video track. Layer-dropping and chain
// integrity gate each write per subscriber. A parse error is non-fatal — the
// loop falls back to permissive forwarding so video doesn't blackhole if a
// publisher sends bytes the parser cannot make sense of (a re-bootstrap
// frame will recover the parser later).
//
// ReadRTP is blocking and respects ctx via remote close: when the publisher
// PC closes (graceful or failure), Read returns io.EOF and the loop exits.
// We don't select on session.ctx — pion does not surface ctx through Read.
//
// The subscriber slice is snapshotted under s.mu.RLock to keep pion-internal
// locks in WriteRTP from crossing s.mu, which would deadlock on teardown
// because OnConnectionStateChange acquires s.mu while holding pc internals.
func (s *ScreenShareSession) forwardVideo(remote *webrtc.TrackRemote) {
	for {
		pkt, _, err := remote.ReadRTP()
		if err != nil {
			if !errors.Is(err, io.EOF) {
				log.Printf("sfu: screen forwardVideo (%s) read: %v", s.PublisherID, err)
			}
			return
		}
		desc, parseErr := s.codecAdapter.Parse(pkt)
		if parseErr != nil {
			log.Printf("sfu: screen forwardVideo (%s) %s parse: %v", s.PublisherID, s.codecAdapter.Name(), parseErr)
			desc = nil
		}

		subs := s.subscribersSnapshot()

		for _, sub := range subs {
			sub.maybeForward(pkt, desc, s.PublisherID)
		}
	}
}

// maybeForward gates one inbound RTP packet against the subscriber's target
// temporal layer and chain integrity. Forwarded packets carry rewritten
// SequenceNumber so the subscriber never sees gaps from deliberate drops —
// real wire loss still produces gaps, which is what NACK is for.
func (sub *screenSubscriber) maybeForward(pkt *rtp.Packet, desc *dd.Descriptor, pubID string) {
	if g := sub.chainGen.Load(); g != sub.lastGen {
		sub.lastGen = g
		sub.chain.SetChain(int(sub.targetTemp.Load()))
	}

	if desc != nil {
		if int32(desc.TemporalLayer) > sub.targetTemp.Load() {
			return
		}
		if !sub.chain.Allow(desc) {
			return
		}
	}

	sub.outPkt = *pkt
	// Strip RTP extensions: the publisher negotiated extension IDs that
	// subscribers did not. DD info we needed was already extracted upstream.
	sub.outPkt.Extension = false
	sub.outPkt.Extensions = nil
	// Rewrite SequenceNumber per-subscriber so dropped packets don't surface
	// as gaps — gaps would trigger NACK storms that the responder cache can
	// never satisfy.
	sub.outPkt.SequenceNumber = uint16(sub.seqCounter.Add(1))
	if err := sub.videoTrack.WriteRTP(&sub.outPkt); err != nil {
		if !errors.Is(err, io.ErrClosedPipe) {
			log.Printf("sfu: screen forwardVideo (%s→%s) write: %v", pubID, sub.peerID, err)
		}
	}
}

func (s *ScreenShareSession) forwardAudio(remote *webrtc.TrackRemote) {
	if s.AudioTrack == nil {
		return
	}
	for {
		pkt, _, err := remote.ReadRTP()
		if err != nil {
			if !errors.Is(err, io.EOF) {
				log.Printf("sfu: screen forwardAudio (%s) read: %v", s.PublisherID, err)
			}
			return
		}
		out := *pkt
		out.Extension = false
		out.Extensions = nil
		if err := s.AudioTrack.WriteRTP(&out); err != nil {
			return
		}
	}
}
