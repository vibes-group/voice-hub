package sfu

import (
	"context"
	"encoding/json"
	"errors"
	"log"
	"time"

	"voice-hub/backend/internal/sfu/protocol"
)

// startScreenShareGrace arms the 5s reattach window when the publisher's WS
// closes. If screen-share-resume validates the token before the timer fires,
// graceCancel is invoked and the session lives on. Otherwise endScreenShareSession
// runs and the session is torn down.
//
// Called from removePeer (publisher disconnect). Safe to call multiple times:
// each call cancels any prior pending timer and arms a fresh one.
func (r *Room) startScreenShareGrace(session *ScreenShareSession) {
	graceCtx, cancel := context.WithCancel(context.Background())

	session.mu.Lock()
	if session.closed {
		session.mu.Unlock()
		cancel()
		return
	}
	prev := session.graceCancel
	session.graceCancel = cancel
	session.mu.Unlock()
	if prev != nil {
		prev()
	}

	go func() {
		t := time.NewTimer(screenShareGracePeriod)
		defer t.Stop()
		select {
		case <-graceCtx.Done():
			return
		case <-t.C:
			r.endScreenShareSession(session, "grace expired")
		}
	}()
}

// claimScreenShareSession validates the resume token and performs the
// lock-phase of a screen-share-resume: it looks up the session, checks for
// concurrent claims, cancels the grace timer, tears down stale subscribers,
// and migrates the session onto the new peer p. Returns the claimed session,
// the old publisher ID, and the stale subscribers to tear down. Any armed
// grace timer is cancelled internally before returning.
//
// On success r.mu has been released. On error an appropriate screen-share-error
// has been sent and both locks are released.
func (r *Room) claimScreenShareSession(p *peer, token string) (
	session *ScreenShareSession,
	oldPubID string,
	staleSubs []*screenSubscriber,
	err error,
) {
	r.mu.Lock()
	session, ok := r.screenSessionsByToken[token]
	if !ok {
		r.mu.Unlock()
		log.Printf("sfu: screen-share-resume (%s): unknown token", p.id)
		r.sendScreenShareError(p, "", protocol.ReasonInvalidToken)
		return nil, "", nil, errResumeRejected
	}
	// Refuse if some other live peer claims this session.
	if existing, ok := r.peers[session.PublisherID]; ok && existing != p {
		r.mu.Unlock()
		log.Printf("sfu: screen-share-resume (%s): token in use by %s", p.id, session.PublisherID)
		r.sendScreenShareError(p, "", protocol.ReasonInvalidToken)
		return nil, "", nil, errResumeRejected
	}
	if p.screenSession != nil && p.screenSession != session {
		r.mu.Unlock()
		log.Printf("sfu: screen-share-resume (%s): peer already owns another session", p.id)
		r.sendScreenShareError(p, "", protocol.ReasonAlreadyPublishing)
		return nil, "", nil, errResumeRejected
	}

	session.mu.Lock()
	if session.closed {
		session.mu.Unlock()
		r.mu.Unlock()
		r.sendScreenShareError(p, "", protocol.ReasonInvalidToken)
		return nil, "", nil, errResumeRejected
	}
	oldPubID = session.PublisherID
	// Claim BEFORE releasing r.mu so concurrent resumes see the new owner.
	session.PublisherID = p.id
	graceCancel := session.graceCancel
	session.graceCancel = nil
	staleSubs = make([]*screenSubscriber, 0, len(session.subscribers))
	for _, s := range session.subscribers {
		staleSubs = append(staleSubs, s)
	}
	session.subscribers = make(map[string]*screenSubscriber)
	session.mu.Unlock()

	p.screenSession = session
	p.screenSharing = true
	p.screenSharingHasAudio = session.HasSystemAudio
	p.screenSharingVideoCodec = session.VideoCodec
	r.mu.Unlock()

	if graceCancel != nil {
		graceCancel()
	}
	return session, oldPubID, staleSubs, nil
}

// errResumeRejected is returned by claimScreenShareSession when the resume is
// rejected and the error has already been sent to the peer.
var errResumeRejected = errors.New("resume rejected")

// handleScreenShareResume rebinds an orphaned screen-share session (publisher
// WS died, grace timer still armed) to the freshly reconnected publisher peer p.
// Token is the auth check: it was issued at the original start and only the
// legitimate publisher has it.
func (r *Room) handleScreenShareResume(p *peer, data protocol.ScreenShareResumeData) {
	session, oldPubID, staleSubs, err := r.claimScreenShareSession(p, data.SessionToken)
	if err != nil {
		return
	}

	// Snapshot the broadcast list now that r.mu is released.
	r.mu.Lock()
	others := make([]*peer, 0, len(r.peers))
	for _, op := range r.peers {
		if op.id != p.id {
			others = append(others, op)
		}
	}
	info := peerInfo(p)
	r.mu.Unlock()

	// Tear down stale subscriber PCs. Each sub's screenSubs map entry was
	// keyed by oldPubID; clean both ends so a re-subscribe under the new ID
	// builds fresh state.
	for _, s := range staleSubs {
		_ = s.pc.Close()
		r.mu.Lock()
		if subPeer, ok := r.peers[s.peerID]; ok && subPeer.screenSubs != nil {
			delete(subPeer.screenSubs, oldPubID)
		}
		r.mu.Unlock()
	}

	infoData, _ := json.Marshal(info)
	infoEnv, _ := json.Marshal(protocol.Envelope{Event: "peer-info", Data: infoData})
	endedData, _ := json.Marshal(protocol.ScreenShareEndedData{PublisherID: oldPubID})
	endedEnv, _ := json.Marshal(protocol.Envelope{Event: "screen-share-ended", Data: endedData})
	availData, _ := json.Marshal(protocol.ScreenShareAvailableData{
		PublisherID:    p.id,
		HasSystemAudio: session.HasSystemAudio,
		VideoCodec:     session.VideoCodec,
		Mode:           session.Mode(),
	})
	availEnv, _ := json.Marshal(protocol.Envelope{Event: "screen-share-available", Data: availData})

	for _, op := range others {
		_ = op.writeRaw(infoEnv)
		_ = op.writeRaw(endedEnv)
		_ = op.writeRaw(availEnv)
	}

	if r.cfg.OnPeerUpdated != nil {
		r.cfg.OnPeerUpdated(info)
	}

	startedData, _ := json.Marshal(protocol.ScreenShareStartedData{SessionToken: session.SessionToken})
	_ = p.write(protocol.Envelope{Event: "screen-share-started", Data: startedData})

	log.Printf("sfu: screen-share-resume %s→%s subs-teardown=%d", oldPubID, p.id, len(staleSubs))
}
