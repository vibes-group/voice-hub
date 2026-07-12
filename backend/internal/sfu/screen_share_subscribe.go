package sfu

import (
	"encoding/json"
	"log"

	"github.com/pion/webrtc/v4"

	"voice-hub/backend/internal/sfu/protocol"
)

// setupScreenSubPC creates a subscriber PeerConnection for session, adds the
// video (and optionally audio) tracks, wires OnICECandidate and
// OnConnectionStateChange, and generates the SDP offer. Returns the PC, the
// subscriber entry (without seqCounter/chain seeded), the audio sender (may be
// nil), and the offer — or an error on any failure. On error the partial PC is
// already closed and a screen-share-error has been sent to sub.
func (r *Room) setupScreenSubPC(
	sub *peer,
	session *ScreenShareSession,
) (pc *webrtc.PeerConnection, subEntry *screenSubscriber, offer webrtc.SessionDescription, err error) {
	r.pcCreateMu.Lock()
	r.pendingBWE = nil
	pc, err = r.api.NewPeerConnection(webrtc.Configuration{ICEServers: r.cfg.ICEServers})
	r.pendingBWE = nil
	r.pcCreateMu.Unlock()
	if err != nil {
		log.Printf("sfu: screen subscribe (%s→%s) new pc: %v", sub.id, session.PublisherID, err)
		r.sendScreenShareError(sub, session.PublisherID, protocol.ReasonInternal)
		return nil, nil, webrtc.SessionDescription{}, err
	}

	videoTrack, err := webrtc.NewTrackLocalStaticRTP(session.videoCodec, "screen-video", session.PublisherID)
	if err != nil {
		pc.Close()
		log.Printf("sfu: screen subscribe (%s→%s) new video track: %v", sub.id, session.PublisherID, err)
		r.sendScreenShareError(sub, session.PublisherID, protocol.ReasonInternal)
		return nil, nil, webrtc.SessionDescription{}, err
	}
	videoSender, err := pc.AddTrack(videoTrack)
	if err != nil {
		pc.Close()
		log.Printf("sfu: screen subscribe (%s→%s) add video: %v", sub.id, session.PublisherID, err)
		r.sendScreenShareError(sub, session.PublisherID, protocol.ReasonInternal)
		return nil, nil, webrtc.SessionDescription{}, err
	}
	var audioSender *webrtc.RTPSender
	if session.AudioTrack != nil {
		audioSender, err = pc.AddTrack(session.AudioTrack)
		if err != nil {
			pc.Close()
			log.Printf("sfu: screen subscribe (%s→%s) add audio: %v", sub.id, session.PublisherID, err)
			r.sendScreenShareError(sub, session.PublisherID, protocol.ReasonInternal)
			return nil, nil, webrtc.SessionDescription{}, err
		}
	}

	pc.OnICECandidate(func(c *webrtc.ICECandidate) {
		if c == nil {
			return
		}
		env := protocol.CandidateEnvelope{
			PC:               protocol.PCScreenSub,
			PublisherID:      session.PublisherID,
			ICECandidateInit: c.ToJSON(),
		}
		b, err := json.Marshal(env)
		if err != nil {
			return
		}
		_ = sub.write(protocol.Envelope{Event: "candidate", Data: b})
	})

	pc.OnConnectionStateChange(func(s webrtc.PeerConnectionState) {
		switch s {
		case webrtc.PeerConnectionStateFailed, webrtc.PeerConnectionStateClosed:
			r.removeScreenSubscriber(sub, session.PublisherID, "subscriber pc closed")
		}
	})

	offer, err = pc.CreateOffer(nil)
	if err != nil {
		pc.Close()
		log.Printf("sfu: screen subscribe (%s→%s) create offer: %v", sub.id, session.PublisherID, err)
		r.sendScreenShareError(sub, session.PublisherID, protocol.ReasonInternal)
		return nil, nil, webrtc.SessionDescription{}, err
	}
	if err = pc.SetLocalDescription(offer); err != nil {
		pc.Close()
		log.Printf("sfu: screen subscribe (%s→%s) set local: %v", sub.id, session.PublisherID, err)
		r.sendScreenShareError(sub, session.PublisherID, protocol.ReasonInternal)
		return nil, nil, webrtc.SessionDescription{}, err
	}

	subEntry = &screenSubscriber{
		peerID:      sub.id,
		pc:          pc,
		videoTrack:  videoTrack,
		videoSender: videoSender,
		audioSender: audioSender,
	}
	return pc, subEntry, offer, nil
}

// finishScreenSubSetup commits subEntry into the session and room maps, starts
// RTCP forwarding goroutines, writes the offer to sub, and triggers dynacast
// on the first subscriber. Called after setupScreenSubPC succeeds and the
// target temporal layer has been stored on subEntry.
func (r *Room) finishScreenSubSetup(
	sub *peer,
	session *ScreenShareSession,
	pc *webrtc.PeerConnection,
	subEntry *screenSubscriber,
	offer webrtc.SessionDescription,
) {
	session.mu.Lock()
	if session.closed {
		session.mu.Unlock()
		pc.Close()
		r.sendScreenShareError(sub, session.PublisherID, protocol.ReasonNotFound)
		return
	}
	session.subscribers[sub.id] = subEntry
	session.refreshSubscriberViewLocked()
	firstSubscriber := len(session.subscribers) == 1
	session.mu.Unlock()

	r.mu.Lock()
	sub.screenSubs[session.PublisherID] = pc
	r.mu.Unlock()

	go r.forwardScreenVideoRTCPToPublisher(session, subEntry, subEntry.videoSender)
	if subEntry.audioSender != nil {
		// nil onReport drops audio RRs so they don't clobber the video-side
		// lossPerMille that drives the auto-downgrade loop.
		go r.forwardRTCPToPublisher(session, subEntry.audioSender, nil)
	}

	offerEnv := protocol.OfferEnvelope{
		PC:                 protocol.PCScreenSub,
		PublisherID:        session.PublisherID,
		SessionDescription: offer,
	}
	b, err := json.Marshal(offerEnv)
	if err != nil {
		log.Printf("sfu: screen subscribe (%s→%s) marshal offer: %v", sub.id, session.PublisherID, err)
		r.removeScreenSubscriber(sub, session.PublisherID, "marshal offer failed")
		return
	}
	_ = sub.write(protocol.Envelope{Event: "offer", Data: b})
	log.Printf("sfu: screen subscribe sub=%s pub=%s temp=%d firstSub=%v",
		sub.id, session.PublisherID, subEntry.targetTemp.Load(), firstSubscriber)

	if firstSubscriber {
		r.sendScreenEncodeEnvelope(session.PublisherID, "screen-share-encode-resume",
			protocol.ScreenShareEncodeResumeData{Layers: allScreenEncodeLayers})
		session.requestKeyframe()
	}
}

// handleScreenShareSubscribe creates a subscriber PC for the given peer,
// attaches the session's fan-out tracks, generates an offer, and writes it
// back.
func (r *Room) handleScreenShareSubscribe(sub *peer, data protocol.ScreenShareSubscribeData) {
	r.mu.Lock()
	pubPeer, ok := r.peers[data.PublisherID]
	var session *ScreenShareSession
	if ok && pubPeer.screenSession != nil {
		session = pubPeer.screenSession
	}
	if session == nil {
		r.mu.Unlock()
		r.sendScreenShareError(sub, data.PublisherID, protocol.ReasonNotFound)
		return
	}
	if sub.screenSubs == nil {
		sub.screenSubs = make(map[string]*webrtc.PeerConnection)
	}
	if _, dup := sub.screenSubs[data.PublisherID]; dup {
		r.mu.Unlock()
		log.Printf("sfu: screen-share-subscribe (%s→%s) duplicate, ignoring", sub.id, data.PublisherID)
		return
	}
	r.mu.Unlock()

	pc, subEntry, offer, err := r.setupScreenSubPC(sub, session)
	if err != nil {
		return
	}

	target := max(min(int32(data.PreferredTemporalLayer), 2), 0)
	if !session.supportsTemporalFiltering() {
		target = 2
	}
	subEntry.chain = NewChainTracker(int(target))
	subEntry.targetTemp.Store(target)
	subEntry.seqCounter.Store(seqSeed())

	r.finishScreenSubSetup(sub, session, pc, subEntry, offer)
}
