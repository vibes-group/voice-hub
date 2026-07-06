package sfu

import (
	"context"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"errors"
	"log"
	mathrand "math/rand/v2"
	"sync"
	"sync/atomic"
	"time"

	"github.com/pion/rtcp"
	"github.com/pion/rtp"
	"github.com/pion/webrtc/v4"

	"voice-hub/backend/internal/sfu/codec"
	"voice-hub/backend/internal/sfu/protocol"
)

// Non-cryptographic; collision-avoidance only.
func seqSeed() uint32 { return mathrand.Uint32() & 0xffff }

// Grace window during which a publisher's session survives a WS disconnect for resume.
const screenShareGracePeriod = 5 * time.Second

// Passed to the pause/resume helpers to stop or restart the entire screen
// stream (no subscribers, or publisher gone). Do not mutate; treat as
// immutable shared slice.
var allScreenEncodeLayers = []int{0, 1, 2}

// autoDowngradePollInterval is the cadence at which the decision loop wakes
// up to evaluate every subscriber's loss state. Tied to the RTCP RR cadence
// (~1s): sampling faster than RRs arrive would add noise without info.
const autoDowngradePollInterval = 2 * time.Second

// modePolicy bundles the loss thresholds, sustain windows, and layer floor
// for one ScreenShareMode. Sharp protects readability — it drops FPS at the
// first sign of trouble (low threshold, short sustain) and can fall all the
// way to T0 (~3-7 fps). Motion protects smoothness — it tolerates more loss
// before dropping FPS (high threshold, long sustain) and stops at T1 so the
// stream never falls below half-FPS, where motion content becomes choppy.
type modePolicy struct {
	highLossPM uint32        // ≥ this per-mille triggers a downstep streak
	lowLossPM  uint32        // ≤ this per-mille triggers an upstep streak
	highWindow time.Duration // sustain duration before stepping down
	lowWindow  time.Duration // sustain duration before stepping back up
	floorTemp  int32         // minimum target temporal layer (inclusive)
}

var (
	sharpPolicy = modePolicy{
		highLossPM: 50,
		lowLossPM:  10,
		highWindow: 2 * time.Second,
		lowWindow:  4 * time.Second,
		floorTemp:  0,
	}
	motionPolicy = modePolicy{
		highLossPM: 120,
		lowLossPM:  30,
		highWindow: 3 * time.Second,
		lowWindow:  7 * time.Second,
		floorTemp:  1,
	}
)

func policyForMode(m protocol.ScreenShareMode) modePolicy {
	if m == protocol.ScreenShareModeMotion {
		return motionPolicy
	}
	return sharpPolicy
}

// ScreenShareSession owns one publisher's screen-share state. Lifecycle:
//
//	create  →ScreenShareSession.start (in screen-share-start handler)
//	idle    →publisher OnTrack fires; forwardLoop drains remote → VideoTrack
//	subscribe→ScreenShareSession.addSubscriber adds the track to a new sub PC
//	stop    →ScreenShareSession.close cancels ctx, closes publisher PC,
//	         broadcasts screen-share-ended, drops the session off peer.screen.
//
// Concurrency: most fields are set at start() and read-only thereafter.
// Mutable state (subscribers, graceCancel) is guarded by mu. forwardLoop
// snapshots subscribers under mu.RLock to avoid pion-internal lock crossover.
type ScreenShareSession struct {
	PublisherID    string
	SessionToken   string
	HasSystemAudio bool

	// videoCodec is the negotiated video codec capability captured at start.
	// Per-subscriber video tracks are minted from this template at subscribe
	// time so each subscriber gets an independent fan-out lane the SFU can
	// filter on (temporal-layer dropping + per-sub seqno rewrite). MimeType
	// empty until SetRemoteDescription populated the publisher's transceivers.
	videoCodec webrtc.RTPCodecCapability
	VideoCodec protocol.ScreenVideoCodec
	AudioTrack *webrtc.TrackLocalStaticRTP // shared across subs; nil when HasSystemAudio=false

	publisherPC *webrtc.PeerConnection
	room        *Room

	// codecAdapter abstracts per-codec RTP parsing (AV1 DD vs VP9 payload
	// header). Created at handleScreenShareStart from the negotiated codec;
	// OnTrackNegotiated runs once on the first OnTrack to capture any per-PC
	// state (e.g. AV1's DD extension ID). Used serially by forwardVideo.
	codecAdapter codec.Adapter

	// mode stores the publisher-selected ScreenShareMode encoded as a small
	// integer so it can be swapped lock-free from a mid-share mode-change
	// without disturbing the forward path. Read by the auto-downgrade loop;
	// the forward goroutine itself doesn't branch on mode.
	mode atomic.Uint32

	// publisherVideoSSRC is the SSRC of the publisher's inbound video track.
	// Set once in the OnTrack callback from remote.SSRC(); zero until OnTrack
	// fires. Cached to avoid GetTransceivers() scans on every PLI/FIR packet.
	publisherVideoSSRC atomic.Uint32

	mu          sync.RWMutex
	subscribers map[string]*screenSubscriber // key = subscriber peer ID

	// graceCancel cancels the in-flight grace timer when the publisher
	// reattaches via screen-share-resume. Replaced atomically each time a
	// new disconnect→reconnect cycle starts. nil when no timer is armed.
	graceCancel context.CancelFunc

	ctx    context.Context
	cancel context.CancelFunc
	closed bool
}

// screenSubscriber holds the per-publisher subscriber state on the SFU side.
//
// videoTrack is per-subscriber (not shared from session) so the forward loop
// can drop temporal layers for one viewer without starving others.
// seqCounter is a monotonically incrementing RTP seqno generator: every
// packet handed to videoTrack.WriteRTP has its SequenceNumber rewritten so
// the subscriber never sees gaps from dropped frames. NACK responder caches
// by SSRC+seq, so contiguous seqs keep the responder useful for genuine
// packet loss on the wire.
//
// targetTemp is the highest temporal layer this subscriber accepts (0..2 for
// L1T3). Atomic so layer-select handlers can update it without taking the
// session-wide lock the forwarder holds while iterating subscribers.
//
// chain tracks DD chain integrity for the chosen target. It's owned by the
// forward goroutine — no concurrent access from layer-select. SetChain on
// targetTemp updates is dispatched via a flag the forwarder checks.
type screenSubscriber struct {
	peerID      string
	pc          *webrtc.PeerConnection
	videoTrack  *webrtc.TrackLocalStaticRTP
	videoSender *webrtc.RTPSender
	audioSender *webrtc.RTPSender // nil when session has no audio

	targetTemp atomic.Int32
	chainGen   atomic.Int32 // bumped by SetTargetTemp; forwarder reads to detect re-arm
	lastGen    int32        // forwarder-local mirror of chainGen
	seqCounter atomic.Uint32
	chain      *ChainTracker

	// outPkt is a reusable scratch buffer for the forwarded copy of each inbound
	// RTP packet. maybeForward overwrites all fields before use; this avoids a
	// heap allocation per packet on the hot forward path.
	outPkt rtp.Packet

	// Auto-downgrade signal. lossPerMille is updated from the last ReceiverReport
	// seen on this subscriber's video sender (FractionLost is a 0..255 fraction,
	// we store ‰ for cheaper int comparisons). The two "since" timestamps are
	// unix-nano hysteresis windows — zero means "no streak yet". The decision
	// loop runs every 2s and reads these atomics without locks.
	lossPerMille  atomic.Uint32
	highLossSince atomic.Int64
	lowLossSince  atomic.Int64
}

// SetTargetTemp updates the subscriber's allowed temporal layer and signals
// the forwarder to re-arm the chain tracker on the next packet. Called from
// the layer-select handler on the WS goroutine, so we cannot mutate the chain
// tracker here directly (it's not concurrency-safe). Bumping the generation
// counter is the rendezvous: forwarder sees the change and calls
// chain.SetChain(layer) before evaluating the next packet.
func (s *screenSubscriber) SetTargetTemp(layer int32) {
	s.targetTemp.Store(layer)
	s.chainGen.Add(1)
}

// newSessionToken returns a base64 (no padding) string of 32 random bytes.
// Used as the opaque server-issued token publishers echo back to resume.
func newSessionToken() (string, error) {
	var b [32]byte
	if _, err := rand.Read(b[:]); err != nil {
		return "", err
	}
	return base64.RawStdEncoding.EncodeToString(b[:]), nil
}

func screenVideoCodecFromCapability(codec webrtc.RTPCodecCapability) protocol.ScreenVideoCodec {
	switch codec.MimeType {
	case webrtc.MimeTypeAV1:
		return protocol.ScreenVideoCodecAV1
	case webrtc.MimeTypeVP9:
		return protocol.ScreenVideoCodecVP9
	default:
		return ""
	}
}

func (s *ScreenShareSession) supportsTemporalFiltering() bool {
	return s.codecAdapter != nil && s.codecAdapter.SupportsTemporalFiltering()
}

// modeEncoding maps protocol.ScreenShareMode to the uint32 representation
// stored on ScreenShareSession.mode. Sharp is the default fallback for
// unknown / empty values, mirroring the client-side default.
const (
	modeSharp  uint32 = 0
	modeMotion uint32 = 1
)

func encodeMode(m protocol.ScreenShareMode) uint32 {
	if m == protocol.ScreenShareModeMotion {
		return modeMotion
	}
	return modeSharp
}

func decodeMode(v uint32) protocol.ScreenShareMode {
	if v == modeMotion {
		return protocol.ScreenShareModeMotion
	}
	return protocol.ScreenShareModeSharp
}

// Mode returns the active ScreenShareMode for the session. Safe for
// concurrent reads — backed by an atomic.
func (s *ScreenShareSession) Mode() protocol.ScreenShareMode {
	return decodeMode(s.mode.Load())
}

// subscribersSnapshot returns the current subscribers under RLock so callers
// can iterate without holding s.mu.
func (s *ScreenShareSession) subscribersSnapshot() []*screenSubscriber {
	s.mu.RLock()
	defer s.mu.RUnlock()
	subs := make([]*screenSubscriber, 0, len(s.subscribers))
	for _, sub := range s.subscribers {
		subs = append(subs, sub)
	}
	return subs
}

// setupScreenPubPC creates the publisher PC, performs the SDP exchange, and
// builds the ScreenShareSession. Returns the session (which owns the PC and
// cancel func) together with the negotiated answer SDP on success. On any
// error the PC is closed and a screen-share-error is sent; the caller must
// return immediately.
func (r *Room) setupScreenPubPC(p *peer, data protocol.ScreenShareStartData) (session *ScreenShareSession, answer webrtc.SessionDescription, err error) {
	token, err := newSessionToken()
	if err != nil {
		log.Printf("sfu: screen-share-start (%s): token: %v", p.id, err)
		r.sendScreenShareError(p, "", protocol.ReasonInternal)
		return nil, webrtc.SessionDescription{}, err
	}

	r.pcCreateMu.Lock()
	r.pendingBWE = nil
	pc, err := r.api.NewPeerConnection(webrtc.Configuration{ICEServers: r.cfg.ICEServers})
	// Drain pendingBWE so subsequent NewPeerConnection callers don't pick up
	// this screen-share PC's estimator by accident.
	r.pendingBWE = nil
	r.pcCreateMu.Unlock()
	if err != nil {
		log.Printf("sfu: screen-share-start (%s) new pc: %v", p.id, err)
		r.sendScreenShareError(p, "", protocol.ReasonInternal)
		return nil, webrtc.SessionDescription{}, err
	}

	ctx, cancel := context.WithCancel(p.ctx)

	defer func() {
		if err != nil {
			pc.Close()
			cancel()
			r.sendScreenShareError(p, "", protocol.ReasonInternal)
		}
	}()

	initialMode := data.Mode
	if !initialMode.IsValid() {
		initialMode = protocol.ScreenShareModeSharp
	}

	session = &ScreenShareSession{
		PublisherID:    p.id,
		SessionToken:   token,
		HasSystemAudio: data.HasSystemAudio,
		publisherPC:    pc,
		room:           r,
		subscribers:    make(map[string]*screenSubscriber),
		ctx:            ctx,
		cancel:         cancel,
	}
	session.mode.Store(encodeMode(initialMode))

	offer := webrtc.SessionDescription{Type: webrtc.SDPTypeOffer, SDP: data.SDP}
	if err = pc.SetRemoteDescription(offer); err != nil {
		log.Printf("sfu: screen-share-start (%s) set remote: %v", p.id, err)
		return nil, webrtc.SessionDescription{}, err
	}

	answer, err = pc.CreateAnswer(nil)
	if err != nil {
		log.Printf("sfu: screen-share-start (%s) create answer: %v", p.id, err)
		return nil, webrtc.SessionDescription{}, err
	}
	if err = pc.SetLocalDescription(answer); err != nil {
		log.Printf("sfu: screen-share-start (%s) set local: %v", p.id, err)
		return nil, webrtc.SessionDescription{}, err
	}

	// Capture negotiated codecs. Video: snapshot capability only — the actual
	// fan-out track is per-subscriber so the SFU can drop temporal layers
	// independently for each viewer and rewrite RTP sequence numbers to
	// avoid NACK storms over deliberate gaps. Audio: shared track minted now,
	// added directly to every subscriber PC (no layer dropping for audio).
	for _, tr := range pc.GetTransceivers() {
		recv := tr.Receiver()
		if recv == nil {
			continue
		}
		track := recv.Track()
		if track == nil {
			continue
		}
		params := recv.GetParameters()
		if len(params.Codecs) == 0 {
			continue
		}
		c := params.Codecs[0].RTPCodecCapability
		switch track.Kind() {
		case webrtc.RTPCodecTypeVideo:
			session.videoCodec = c
		case webrtc.RTPCodecTypeAudio:
			if !data.HasSystemAudio {
				continue
			}
			var at *webrtc.TrackLocalStaticRTP
			at, err = webrtc.NewTrackLocalStaticRTP(c, "screen-audio", p.id)
			if err != nil {
				log.Printf("sfu: screen-share-start (%s) new audio track: %v", p.id, err)
				return nil, webrtc.SessionDescription{}, err
			}
			session.AudioTrack = at
		}
	}
	if session.videoCodec.MimeType == "" {
		log.Printf("sfu: screen-share-start (%s) no video transceiver in offer", p.id)
		err = errors.New("no video transceiver")
		return nil, webrtc.SessionDescription{}, err
	}
	session.VideoCodec = screenVideoCodecFromCapability(session.videoCodec)
	if session.VideoCodec == "" {
		log.Printf("sfu: screen-share-start (%s) unsupported video codec: %s", p.id, session.videoCodec.MimeType)
		err = errors.New("unsupported video codec")
		return nil, webrtc.SessionDescription{}, err
	}
	session.codecAdapter = codec.New(session.VideoCodec)

	return session, answer, nil
}

// wireScreenPubCallbacks registers the three publisher PC event callbacks
// (OnConnectionStateChange, OnICECandidate, OnTrack) on pc.
func (r *Room) wireScreenPubCallbacks(p *peer, pc *webrtc.PeerConnection, session *ScreenShareSession) {
	pc.OnConnectionStateChange(func(s webrtc.PeerConnectionState) {
		switch s {
		case webrtc.PeerConnectionStateFailed, webrtc.PeerConnectionStateClosed:
			r.endScreenShareSession(session, "publisher pc closed")
		}
	})

	pc.OnICECandidate(func(c *webrtc.ICECandidate) {
		if c == nil {
			return
		}
		env := protocol.CandidateEnvelope{
			PC:               protocol.PCScreenPub,
			ICECandidateInit: c.ToJSON(),
		}
		b, err := json.Marshal(env)
		if err != nil {
			return
		}
		_ = p.write(protocol.Envelope{Event: "candidate", Data: b})
	})

	pc.OnTrack(func(remote *webrtc.TrackRemote, receiver *webrtc.RTPReceiver) {
		switch remote.Kind() {
		case webrtc.RTPCodecTypeVideo:
			// codecAdapter captures any per-PC negotiated state here (e.g.
			// AV1's DD extension ID). Subscriber PCs negotiate their own
			// extension IDs independently — we strip the publisher's
			// extension on forward and let pion re-insert the right ID on
			// each subscriber side.
			session.publisherVideoSSRC.Store(uint32(remote.SSRC()))
			session.codecAdapter.OnTrackNegotiated(receiver)
			r.firstScreenVideoReady(p, session)
			session.forwardVideo(remote)
		case webrtc.RTPCodecTypeAudio:
			session.forwardAudio(remote)
		}
	})
}

// handleScreenShareStart is the entry point for the publisher's first
// screen-share-start message. It creates a new PC dedicated to the screen
// share, parses the publisher's offer, creates AV1-capable forwarder
// tracks, answers, and on first OnTrack broadcasts screen-share-available
// to the room.
func (r *Room) handleScreenShareStart(p *peer, data protocol.ScreenShareStartData) {
	r.mu.Lock()
	if p.screenSession != nil {
		r.mu.Unlock()
		log.Printf("sfu: screen-share-start (%s): already publishing", p.id)
		r.sendScreenShareError(p, "", protocol.ReasonAlreadyPublishing)
		return
	}
	r.mu.Unlock()

	session, answer, err := r.setupScreenPubPC(p, data)
	if err != nil {
		return
	}

	r.wireScreenPubCallbacks(p, session.publisherPC, session)

	r.mu.Lock()
	p.screenSession = session
	p.screenSharing = true
	p.screenSharingHasAudio = data.HasSystemAudio
	p.screenSharingVideoCodec = session.VideoCodec
	r.screenSessionsByToken[session.SessionToken] = session
	r.mu.Unlock()

	if session.supportsTemporalFiltering() {
		go r.autoDowngradeLoop(session)
	}

	// Order matters: -started carries the resume token, the answer carries
	// the SDP. Both go through the same FIFO writeLoop, so writing -started
	// first guarantees the publisher reads the token before processing the
	// answer (and so can recover it for screen-share-resume on reconnect).
	startedData, _ := json.Marshal(protocol.ScreenShareStartedData{SessionToken: session.SessionToken})
	_ = p.write(protocol.Envelope{Event: "screen-share-started", Data: startedData})

	answerEnv := protocol.AnswerEnvelope{
		PC:                 protocol.PCScreenPub,
		SessionDescription: answer,
	}
	answerData, err := json.Marshal(answerEnv)
	if err != nil {
		log.Printf("sfu: screen-share-start (%s) marshal answer: %v", p.id, err)
		r.endScreenShareSession(session, "marshal answer failed")
		return
	}
	_ = p.write(protocol.Envelope{Event: "answer", Data: answerData})
}

// firstScreenVideoReady runs exactly once per session, on the first video
// OnTrack. It updates the publisher's PeerInfo and broadcasts both peer-info
// (for late joiners) and screen-share-available (for active subscribers'
// gallery refresh). Done here, not at screen-share-start, so subscribers
// only render a tile when the SFU actually has media to forward.
func (r *Room) firstScreenVideoReady(p *peer, session *ScreenShareSession) {
	session.mu.Lock()
	if session.closed {
		session.mu.Unlock()
		return
	}
	session.mu.Unlock()

	r.mu.Lock()
	others := r.othersLocked(p.id)
	// peerInfo reads p.displayName / p.selfMuted / etc — fields guarded by
	// r.mu. Snapshot here, not after Unlock.
	info := peerInfo(p)
	r.mu.Unlock()

	infoData, _ := json.Marshal(info)
	infoEnv, _ := json.Marshal(protocol.Envelope{Event: "peer-info", Data: infoData})

	availableData, _ := json.Marshal(protocol.ScreenShareAvailableData{
		PublisherID:    session.PublisherID,
		HasSystemAudio: session.HasSystemAudio,
		VideoCodec:     session.VideoCodec,
		Mode:           session.Mode(),
	})
	availableEnv, _ := json.Marshal(protocol.Envelope{Event: "screen-share-available", Data: availableData})

	for _, op := range others {
		_ = op.writeRaw(infoEnv)
		_ = op.writeRaw(availableEnv)
	}

	if r.cfg.OnPeerUpdated != nil {
		r.cfg.OnPeerUpdated(info)
	}

	// Dynacast initial state: until the first subscriber clicks the tile,
	// the encoder has no audience — pause it so a publisher in a quiet room
	// doesn't burn CPU on encoded frames the SFU drops on the floor.
	session.mu.RLock()
	idle := len(session.subscribers) == 0
	session.mu.RUnlock()
	if idle {
		r.sendScreenEncodeEnvelope(session.PublisherID, "screen-share-encode-pause",
			protocol.ScreenShareEncodePauseData{Layers: allScreenEncodeLayers})
	}
}

// sendScreenEncodeEnvelope tells the publisher to pause or resume encoding
// the listed temporal layers. Layers=[0,1,2] means full pause/resume. No-op
// if the publisher peer is no longer in the room.
func (r *Room) sendScreenEncodeEnvelope(publisherID, event string, payload any) {
	r.mu.Lock()
	pubPeer := r.peers[publisherID]
	r.mu.Unlock()
	if pubPeer == nil {
		return
	}
	raw, err := json.Marshal(payload)
	if err != nil {
		log.Printf("sfu: %s marshal (%s): %v", event, publisherID, err)
		return
	}
	_ = pubPeer.write(protocol.Envelope{Event: event, Data: raw})
	log.Printf("sfu: dynacast %s pub=%s", event, publisherID)
}

// requestKeyframe sends a PLI to the publisher so the next RTP packet carries
// an intra frame. Called on dynacast resume and explicit layer-select so the
// new viewer state doesn't wait up to a full GOP for decodable video. No-op
// when the publisher's video receiver has no SSRC yet (pre-OnTrack).
func (s *ScreenShareSession) requestKeyframe() {
	pubSSRC := s.publisherVideoSSRC.Load()
	if pubSSRC == 0 {
		return
	}
	_ = s.publisherPC.WriteRTCP([]rtcp.Packet{
		&rtcp.PictureLossIndication{MediaSSRC: pubSSRC},
	})
	log.Printf("sfu: screen PLI pub=%s ssrc=%d", s.PublisherID, pubSSRC)
}

// handleScreenShareModeChange swaps the active ScreenShareMode mid-share.
// Updates session.mode (atomic), broadcasts screen-share-mode-changed to
// every other peer in the room so viewers can refresh any mode-derived UI
// state, and PLIs the publisher so the encoder hands out a fresh keyframe
// under the new contentHint.
//
// Invalid / unchanged mode is a silent no-op.
func (r *Room) handleScreenShareModeChange(p *peer, data protocol.ScreenShareModeChangeData) {
	if !data.Mode.IsValid() {
		return
	}
	r.mu.Lock()
	session := p.screenSession
	r.mu.Unlock()
	if session == nil {
		r.sendScreenShareError(p, "", protocol.ReasonNotFound)
		return
	}

	newEnc := encodeMode(data.Mode)
	if session.mode.Swap(newEnc) == newEnc {
		return
	}

	session.requestKeyframe()

	r.mu.Lock()
	others := r.othersLocked(p.id)
	r.mu.Unlock()

	payload, _ := json.Marshal(protocol.ScreenShareModeChangedData{
		PublisherID: session.PublisherID,
		Mode:        data.Mode,
	})
	envBytes, _ := json.Marshal(protocol.Envelope{Event: "screen-share-mode-changed", Data: payload})
	for _, op := range others {
		_ = op.writeRaw(envBytes)
	}
	log.Printf("sfu: screen-share mode-change pub=%s mode=%s", session.PublisherID, data.Mode)
}

// handleScreenShareUnsubscribe is the client-initiated path. It defers
// to removeScreenSubscriber, which is also the cleanup path for the
// connection-state-failed branch.
func (r *Room) handleScreenShareUnsubscribe(sub *peer, data protocol.ScreenShareUnsubscribeData) {
	r.removeScreenSubscriber(sub, data.PublisherID, "client requested")
}

// removeScreenSubscriber tears down the subscriber's per-publisher PC.
// Idempotent: safe to call from both the unsubscribe handler and the
// OnConnectionStateChange callback.
func (r *Room) removeScreenSubscriber(sub *peer, publisherID, reason string) {
	r.mu.Lock()
	var subPC *webrtc.PeerConnection
	if sub.screenSubs != nil {
		subPC = sub.screenSubs[publisherID]
		delete(sub.screenSubs, publisherID)
	}
	// Snapshot the publisher's session pointer under r.mu — reading
	// pubPeer.screenSession outside the lock would race with
	// endScreenShareSession's nil-out and deref to panic.
	var session *ScreenShareSession
	if pubPeer := r.peers[publisherID]; pubPeer != nil {
		session = pubPeer.screenSession
	}
	r.mu.Unlock()

	if subPC != nil {
		_ = subPC.Close()
	}
	wentIdle := false
	if session != nil {
		session.mu.Lock()
		if _, ok := session.subscribers[sub.id]; ok {
			delete(session.subscribers, sub.id)
			wentIdle = len(session.subscribers) == 0
		}
		session.mu.Unlock()
	}
	if wentIdle {
		r.sendScreenEncodeEnvelope(publisherID, "screen-share-encode-pause",
			protocol.ScreenShareEncodePauseData{Layers: allScreenEncodeLayers})
	}

	log.Printf("sfu: screen unsubscribe (%s→%s) %s", sub.id, publisherID, reason)
}

// handleScreenShareStop is the publisher-initiated tear-down path. It uses
// endScreenShareSession to handle the broadcast + state cleanup.
func (r *Room) handleScreenShareStop(p *peer) {
	r.mu.Lock()
	session := p.screenSession
	r.mu.Unlock()
	if session == nil {
		return
	}
	r.endScreenShareSession(session, "publisher requested stop")
}

// endScreenShareSession closes the publisher PC, closes all subscriber PCs
// for this session, broadcasts screen-share-ended, and clears the publisher's
// screenSharing peer-info flags. Idempotent via session.closed guard.
func (r *Room) endScreenShareSession(session *ScreenShareSession, reason string) {
	session.mu.Lock()
	if session.closed {
		session.mu.Unlock()
		return
	}
	session.closed = true
	subs := make([]*screenSubscriber, 0, len(session.subscribers))
	for _, s := range session.subscribers {
		subs = append(subs, s)
	}
	session.subscribers = nil
	graceCancel := session.graceCancel
	session.graceCancel = nil
	session.mu.Unlock()

	if graceCancel != nil {
		graceCancel()
	}

	_ = session.publisherPC.Close()
	session.cancel()

	for _, s := range subs {
		_ = s.pc.Close()
		r.mu.Lock()
		if subPeer, ok := r.peers[s.peerID]; ok && subPeer.screenSubs != nil {
			delete(subPeer.screenSubs, session.PublisherID)
		}
		r.mu.Unlock()
	}

	r.mu.Lock()
	delete(r.screenSessionsByToken, session.SessionToken)
	pubPeer := r.peers[session.PublisherID]
	var info protocol.PeerInfo
	var havePubInfo bool
	if pubPeer != nil && pubPeer.screenSession == session {
		pubPeer.screenSession = nil
		pubPeer.screenSharing = false
		pubPeer.screenSharingHasAudio = false
		pubPeer.screenSharingVideoCodec = ""
	}
	if pubPeer != nil {
		// Snapshot peerInfo while still under r.mu — fields it reads
		// (displayName, selfMuted, etc.) are guarded by this lock.
		info = peerInfo(pubPeer)
		havePubInfo = true
	}
	others := make([]*peer, 0, len(r.peers))
	for _, op := range r.peers {
		others = append(others, op)
	}
	r.mu.Unlock()

	endedData, _ := json.Marshal(protocol.ScreenShareEndedData{PublisherID: session.PublisherID})
	endedEnv, _ := json.Marshal(protocol.Envelope{Event: "screen-share-ended", Data: endedData})

	if havePubInfo {
		infoData, _ := json.Marshal(info)
		infoEnv, _ := json.Marshal(protocol.Envelope{Event: "peer-info", Data: infoData})
		for _, op := range others {
			_ = op.writeRaw(endedEnv)
			if op.id != info.ID {
				_ = op.writeRaw(infoEnv)
			}
		}
		if r.cfg.OnPeerUpdated != nil {
			r.cfg.OnPeerUpdated(info)
		}
	} else {
		for _, op := range others {
			_ = op.writeRaw(endedEnv)
		}
	}

	log.Printf("sfu: screen-share ended publisher=%s reason=%s", session.PublisherID, reason)
}

// handleClientOffer routes a client-initiated offer (currently only used for
// screen-share publisher ICE restart on resume) to the right PC. The offer
// envelope carries the discriminator field "pc"; for screen-pub we feed it
// into the publisher PC's SetRemoteDescription / CreateAnswer cycle and reply
// with an answer carrying pc=screen-pub.
//
// Other discriminators are protocol errors: audio uses SFU-as-offerer, and
// screen-sub never sees a client offer (clients always answer those).
func (r *Room) handleClientOffer(p *peer, env protocol.OfferEnvelope) {
	if env.PC != protocol.PCScreenPub {
		log.Printf("sfu: client offer with unsupported pc=%q from %s", env.PC, p.id)
		return
	}
	r.mu.Lock()
	session := p.screenSession
	r.mu.Unlock()
	if session == nil {
		log.Printf("sfu: client offer screen-pub from %s with no session", p.id)
		r.sendScreenShareError(p, "", protocol.ReasonNotFound)
		return
	}

	if err := session.publisherPC.SetRemoteDescription(env.SessionDescription); err != nil {
		log.Printf("sfu: client offer screen-pub (%s) set remote: %v", p.id, err)
		r.sendScreenShareError(p, "", protocol.ReasonInternal)
		return
	}
	answer, err := session.publisherPC.CreateAnswer(nil)
	if err != nil {
		log.Printf("sfu: client offer screen-pub (%s) create answer: %v", p.id, err)
		r.sendScreenShareError(p, "", protocol.ReasonInternal)
		return
	}
	if err := session.publisherPC.SetLocalDescription(answer); err != nil {
		log.Printf("sfu: client offer screen-pub (%s) set local: %v", p.id, err)
		r.sendScreenShareError(p, "", protocol.ReasonInternal)
		return
	}
	answerEnv := protocol.AnswerEnvelope{
		PC:                 protocol.PCScreenPub,
		SessionDescription: answer,
	}
	data, err := json.Marshal(answerEnv)
	if err != nil {
		return
	}
	_ = p.write(protocol.Envelope{Event: "answer", Data: data})
}

// sendScreenShareError marshals and writes a screen-share-error envelope.
// Errors are advisory: the client uses them to revert UI state, but the
// server has already torn the relevant state down.
func (r *Room) sendScreenShareError(p *peer, publisherID string, reason protocol.ScreenShareReason) {
	payload := protocol.ScreenShareErrorData{PublisherID: publisherID, Reason: reason}
	data, _ := json.Marshal(payload)
	_ = p.write(protocol.Envelope{Event: "screen-share-error", Data: data})
}
