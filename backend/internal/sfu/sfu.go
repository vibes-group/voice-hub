// Package sfu is a minimal WebRTC SFU embedded in the same Go process as
// the HTTP/auth backend. Single permanent room, fan-out of every
// participant's Opus audio track to every other participant.
// Forked from pion/example-webrtc-applications/sfu-ws.
package sfu

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"log"
	"log/slog"
	"net/http"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/coder/websocket"
	"github.com/oklog/ulid/v2"
	"github.com/pion/interceptor/pkg/cc"
	"github.com/pion/rtp"
	"github.com/pion/webrtc/v4"

	"voice-hub/backend/internal/filestore"
	"voice-hub/backend/internal/sfu/protocol"
)

// Config holds room-level configuration.
type Config struct {
	ICEServers []webrtc.ICEServer
	// NAT1To1IPs lists public IPs to advertise as srflx candidates when the
	// process runs behind 1:1 NAT (typical Docker bridge or NAT'd VPS).
	NAT1To1IPs []string
	// UDPPortRange limits ICE host candidates to this UDP port range so the
	// container's exposed UDP ports match.
	UDPPortMin uint16
	UDPPortMax uint16
	// AppHostname is used to derive the allowed WebSocket Origin patterns.
	// When "localhost", dev wildcard patterns are added so local frontends on
	// any port are accepted. For any other value, only that exact host is
	// allowed (scheme-independent, since coder/websocket matches host:port).
	AppHostname string
	// Callers must release r.mu before these callbacks are invoked; they are
	// allowed to re-enter Room methods that acquire r.mu (e.g. Peers()).
	OnPeerJoined  func(protocol.PeerInfo)
	OnPeerLeft    func(id string)
	OnPeerUpdated func(protocol.PeerInfo)
	// RoomID is the room slug, used to scope attachment uploads to this room.
	RoomID string
	// FileStore backs chat attachments. When nil, attachment uploads are a
	// disabled feature and chat messages carrying attachments are dropped.
	FileStore *filestore.Store
}

func (cfg Config) originPatterns() []string {
	if cfg.AppHostname == "" || cfg.AppHostname == "localhost" {
		return []string{"localhost:*", "127.0.0.1:*"}
	}
	return []string{cfg.AppHostname}
}

type peer struct {
	id          string
	displayName string
	// clientID is the stable per-install identifier reported by the client
	// in HelloPayload. Echoed in PeerInfo broadcasts so other peers can key
	// per-peer UI state by something that survives reconnects. Empty for
	// older clients that don't send it.
	clientID  string
	selfMuted bool
	deafened  bool
	// chatOnly is true for lurker peers. pc is nil for lurker peers; any
	// code path that touches pc must guard on this field first.
	chatOnly bool

	pc *webrtc.PeerConnection
	ws *websocket.Conn

	// bwe is nil for lurkers; its OnTargetBitrateChange feeds bwCapTID.
	bwe cc.BandwidthEstimator
	// bwCapTID is read lock-free on the RTP forward path; bwCapNone = no clamp.
	bwCapTID atomic.Uint32

	// lastPingReceivedAt is the timestamp of the last ping this peer received,
	// from any sender. Guarded by Room.mu. Used to rate-limit incoming pings
	// per target (one alert / 10 s) so a target doesn't get spammed by many
	// senders simultaneously.
	lastPingReceivedAt time.Time

	// lastRenegotiateAt rate-limits inbound renegotiate so one client
	// can't trigger a room-wide offer storm. Guarded by Room.mu.
	lastRenegotiateAt time.Time

	// out is the per-peer outbound queue, drained by writeLoop. Broadcast
	// loops enqueue here non-blocking and never wait on a slow socket; if
	// the queue is full, the peer is treated as dead and its context is
	// cancelled so it can be torn down without holding up other peers.
	out    chan []byte
	ctx    context.Context
	cancel context.CancelFunc

	// syncMu serialises syncOnePeer for this peer; caller must not hold r.mu.
	syncMu sync.Mutex
	// syncPending is set when syncOnePeer was skipped because the PC was
	// mid-negotiation; the answer handler drains it once signaling settles.
	syncPending atomic.Bool

	// Screen share state. Both fields guarded by Room.mu.
	//
	//  - screenSession is the active publisher session this peer owns
	//    (nil when not sharing).
	//  - screenSubs holds this peer's subscriber-side PCs, keyed by the
	//    PUBLISHER's peer ID (one entry per publisher we're focused on).
	//  - screenSharing / screenSharingHasAudio / screenSharingVideoCodec mirror the matching PeerInfo
	//    fields so peer-info broadcasts stay in sync without re-reading
	//    screenSession (avoids touching session under r.mu).
	screenSession           *ScreenShareSession
	screenSubs              map[string]*webrtc.PeerConnection
	screenSharing           bool
	screenSharingHasAudio   bool
	screenSharingVideoCodec protocol.ScreenVideoCodec
}

// peerOutBufLen bounds per-peer outbound queue depth. Sized for the
// pathological synthetic case: every peer in a large room toggling state
// in a tight loop produces a burst of N×K messages per recipient before
// any writeLoop drain. Real human-driven toggle frequency is ~1 Hz, so
// queues stay near zero in practice; this bound only matters when a
// subscriber's TCP socket is genuinely stalled, in which case we cancel
// the peer instead of blocking room-wide broadcasts. ~1024 × ~200B per
// message ≈ 200 KB max buffer per stuck peer — acceptable.
const peerOutBufLen = 1024

// pingCooldown rate-limits incoming pings per target. After a ping reaches a
// target, further pings to that same target from any sender within this window
// are silently dropped, so a target doesn't get spammed when several peers
// ping them at once.
const pingCooldown = 10 * time.Second

// renegotiateCooldown caps inbound renegotiate frequency per peer. Each
// renegotiate triggers an offer/answer round-trip on every other peer, so
// without this a single tight-looping client amplifies into N peers of work.
const renegotiateCooldown = 250 * time.Millisecond

// bwCapNone: sentinel above the TID range (uint32, since atomic.Uint8 doesn't exist).
const bwCapNone uint32 = 255

const (
	bweInitialBitrate   = 3_000_000
	bweMidThresholdBps  = 1_500_000
	bweHighThresholdBps = 3_000_000
)

func (p *peer) write(msg protocol.Envelope) error {
	raw, err := json.Marshal(msg)
	if err != nil {
		return err
	}
	return p.writeRaw(raw)
}

// writeRaw enqueues a pre-marshaled envelope onto the peer's outbound
// queue. Non-blocking: if the queue is full the peer is cancelled and an
// error is returned so the broadcast loop can move on. Caller must not
// rely on synchronous delivery — successful return means "queued", not
// "sent".
func (p *peer) writeRaw(raw []byte) error {
	select {
	case <-p.ctx.Done():
		return p.ctx.Err()
	default:
	}
	select {
	case p.out <- raw:
		return nil
	case <-p.ctx.Done():
		return p.ctx.Err()
	default:
		log.Printf("sfu: peer %s outq full (cap=%d), dropping", p.id, peerOutBufLen)
		p.cancel()
		return errPeerOutqFull
	}
}

var errPeerOutqFull = errors.New("peer outq full")

// writeLoop drains p.out and serializes WS writes for this peer. Started
// once per peer in ServeWS before addPeer so the welcome message has a
// reader. Returns on ctx cancellation or write failure.
//
// Also drives application-level keepalive: a 25 s ticker fires a Ping so
// idle connections (especially lurker peers, which can sit silent for
// minutes) survive proxy idle timeouts (typical defaults are 30–60 s).
func (p *peer) writeLoop() {
	ping := time.NewTicker(25 * time.Second)
	defer ping.Stop()
	for {
		select {
		case <-p.ctx.Done():
			return
		case raw := <-p.out:
			wctx, cancel := context.WithTimeout(p.ctx, 5*time.Second)
			err := p.ws.Write(wctx, websocket.MessageText, raw)
			cancel()
			if err != nil {
				p.cancel()
				return
			}
		case <-ping.C:
			pctx, cancel := context.WithTimeout(p.ctx, 5*time.Second)
			err := p.ws.Ping(pctx)
			cancel()
			if err != nil {
				p.cancel()
				return
			}
		}
	}
}

// Room holds the live state of all peers and forwarded tracks.
type Room struct {
	mu    sync.Mutex
	peers map[string]*peer
	// tracks is keyed by trackKey(ownerID, kind) so one peer can
	// publish both audio and screen-share video concurrently.
	tracks map[string]*webrtc.TrackLocalStaticRTP
	cfg    Config
	api    *webrtc.API
	closed atomic.Bool

	// pcCreateMu serialises NewPeerConnection so the cc OnNewPeerConnection
	// callback (sync, during construction) can deposit into pendingBWE safely.
	pcCreateMu sync.Mutex
	pendingBWE cc.BandwidthEstimator

	// resyncPending guards the deferred-retry goroutine spawn in
	// signalPeerConnections so that a storm of join/leave/track events
	// cannot accumulate concurrent retry goroutines. At most one retry is
	// pending at a time; further exhaustions while pending are no-ops
	// (the in-flight retry already covers the latest state).
	resyncPending atomic.Bool

	// screenSessionsByToken indexes every live ScreenShareSession by its
	// server-issued resume token. Lookup happens on screen-share-resume
	// against a freshly reconnected publisher, whose peer ID changed due to
	// clientId eviction. Token is opaque-and-secret, so it doubles as the
	// auth check that this peer owns the session. Guarded by r.mu.
	screenSessionsByToken map[string]*ScreenShareSession
}

// ServeWS upgrades the request to a WebSocket and runs one peer session.
func (r *Room) ServeWS(w http.ResponseWriter, req *http.Request) {
	if r.closed.Load() {
		http.Error(w, "shutting down", http.StatusServiceUnavailable)
		return
	}
	ws, err := websocket.Accept(w, req, &websocket.AcceptOptions{
		OriginPatterns: r.cfg.originPatterns(),
	})
	if err != nil {
		log.Printf("sfu: ws accept: %v", err)
		return
	}
	defer ws.Close(websocket.StatusNormalClosure, "")

	ctx, cancel := context.WithCancel(req.Context())
	defer cancel()

	// Expect the first message to be `hello { displayName }` so the peer is
	// added to the room with the correct name and peer-joined broadcasts
	// don't race a follow-up set-displayname round trip. 10s timeout keeps
	// idle/probe connections from sticking around.
	helloCtx, cancelHello := context.WithTimeout(ctx, 10*time.Second)
	_, raw, err := ws.Read(helloCtx)
	cancelHello()
	if err != nil {
		log.Printf("sfu: ws read hello: %v", err)
		return
	}
	var helloMsg protocol.Envelope
	if err := json.Unmarshal(raw, &helloMsg); err != nil || helloMsg.Event != "hello" {
		log.Printf("sfu: expected hello, got %q", helloMsg.Event)
		return
	}
	var hello protocol.HelloPayload
	if err := json.Unmarshal(helloMsg.Data, &hello); err != nil {
		log.Printf("sfu: hello payload unmarshal: %v", err)
	}

	if hello.ChatOnly {
		r.serveWSlurker(ctx, cancel, ws, hello)
		return
	}

	r.pcCreateMu.Lock()
	r.pendingBWE = nil
	pc, err := r.api.NewPeerConnection(webrtc.Configuration{ICEServers: r.cfg.ICEServers})
	bwe := r.pendingBWE
	r.pendingBWE = nil
	r.pcCreateMu.Unlock()
	if err != nil {
		log.Printf("sfu: new pc: %v", err)
		return
	}
	defer pc.Close()

	if _, err := pc.AddTransceiverFromKind(webrtc.RTPCodecTypeAudio, webrtc.RTPTransceiverInit{
		Direction: webrtc.RTPTransceiverDirectionRecvonly,
	}); err != nil {
		log.Printf("sfu: add audio transceiver: %v", err)
		return
	}

	p := &peer{
		id:          newPeerID(),
		displayName: hello.DisplayName,
		clientID:    hello.ClientID,
		pc:          pc,
		ws:          ws,
		out:         make(chan []byte, peerOutBufLen),
		ctx:         ctx,
		cancel:      cancel,
		bwe:         bwe,
	}
	p.bwCapTID.Store(bwCapNone)
	if bwe != nil {
		bwe.OnTargetBitrateChange(func(bitrate int) {
			p.bwCapTID.Store(bitrateToTIDCap(bitrate))
		})
	}
	go p.writeLoop()

	r.addPeer(p)
	defer r.removePeer(p.id)

	pc.OnICECandidate(func(c *webrtc.ICECandidate) {
		if c == nil {
			return
		}
		b, err := json.Marshal(protocol.CandidateEnvelope{
			PC:               protocol.PCAudio,
			ICECandidateInit: c.ToJSON(),
		})
		if err != nil {
			log.Printf("sfu: marshal ICE candidate (%s): %v", p.id, err)
			return
		}
		if err := p.write(protocol.Envelope{Event: "candidate", Data: b}); err != nil && !errors.Is(err, context.Canceled) {
			log.Printf("sfu: send candidate (%s): %v", p.id, err)
		}
	})

	pc.OnConnectionStateChange(func(s webrtc.PeerConnectionState) {
		switch s {
		case webrtc.PeerConnectionStateFailed, webrtc.PeerConnectionStateClosed:
			cancel()
		}
	})

	pc.OnTrack(func(t *webrtc.TrackRemote, _ *webrtc.RTPReceiver) {
		trackID := trackKey(p.id, t.Kind().String())
		// StreamID = peer id so the receiving client groups audio + video by publisher.
		local, err := webrtc.NewTrackLocalStaticRTP(t.Codec().RTPCodecCapability, trackID, p.id)
		if err != nil {
			log.Printf("sfu: new local track: %v", err)
			return
		}

		r.publishTrack(trackID, local)
		defer r.unpublishTrack(trackID)

		buf := make([]byte, 1500)
		pkt := &rtp.Packet{}
		for {
			n, _, err := t.Read(buf)
			if err != nil {
				return
			}
			if err := pkt.Unmarshal(buf[:n]); err != nil {
				return
			}
			// Strip RTP header extensions: the publisher negotiated extension IDs
			// that subscribers did not, so forwarding them would cause subscribers
			// to misparse the extension block.
			pkt.Extension = false
			pkt.Extensions = nil

			if err := local.WriteRTP(pkt); err != nil {
				return
			}
		}
	})

	// Initial sync: subscribe this peer to existing tracks.
	r.signalPeerConnections()

	for {
		_, raw, err := ws.Read(ctx)
		if err != nil {
			if !errors.Is(err, context.Canceled) {
				log.Printf("sfu: ws read (%s): %v", p.id, err)
			}
			return
		}
		var msg protocol.Envelope
		if err := json.Unmarshal(raw, &msg); err != nil {
			log.Printf("sfu: bad json from %s: %v", p.id, err)
			continue
		}
		r.handleClientMessage(p, msg)
	}
}

// serveWSlurker runs a chat-only (lurker) peer session. No PeerConnection is
// created; the peer is added to the room roster and may only send chat-send.
func (r *Room) serveWSlurker(ctx context.Context, cancel context.CancelFunc, ws *websocket.Conn, hello protocol.HelloPayload) {
	p := &peer{
		id:          newPeerID(),
		displayName: hello.DisplayName,
		clientID:    hello.ClientID,
		chatOnly:    true,
		ws:          ws,
		out:         make(chan []byte, peerOutBufLen),
		ctx:         ctx,
		cancel:      cancel,
	}
	go p.writeLoop()

	r.addPeer(p)
	defer r.removePeer(p.id)

	for {
		_, raw, err := ws.Read(ctx)
		if err != nil {
			if !errors.Is(err, context.Canceled) {
				log.Printf("sfu: ws read lurker (%s): %v", p.id, err)
			}
			return
		}
		var msg protocol.Envelope
		if err := json.Unmarshal(raw, &msg); err != nil {
			log.Printf("sfu: bad json from lurker %s: %v", p.id, err)
			continue
		}
		r.handleClientMessage(p, msg)
	}
}

func (r *Room) handleAnswer(p *peer, env protocol.AnswerEnvelope) {
	switch env.PC {
	case protocol.PCAudio:
		if err := p.pc.SetRemoteDescription(env.SessionDescription); err != nil {
			log.Printf("sfu: set remote audio (%s): %v", p.id, err)
			return
		}
		// Drain a sync skipped while this peer was mid-negotiation.
		// Conditional: unconditional re-sync creates an offer ping-pong
		// (every answer triggers fresh offers for all peers).
		if p.syncPending.Swap(false) {
			r.signalPeerConnections()
		}
	case protocol.PCScreenSub:
		r.mu.Lock()
		subPC := p.screenSubs[env.PublisherID]
		r.mu.Unlock()
		if subPC == nil {
			log.Printf("sfu: screen-sub answer (%s→%s) no PC", p.id, env.PublisherID)
			return
		}
		if err := subPC.SetRemoteDescription(env.SessionDescription); err != nil {
			log.Printf("sfu: screen-sub set remote (%s→%s): %v", p.id, env.PublisherID, err)
		}
	case protocol.PCScreenPub:
		// SFU is the answerer on screen-pub, never the offerer. A client
		// "answer" with pc=screen-pub is a protocol misuse; log and drop.
		log.Printf("sfu: unexpected answer pc=screen-pub from %s", p.id)
	default:
		log.Printf("sfu: answer with unknown pc=%q from %s", env.PC, p.id)
	}
}

func (r *Room) handleCandidate(p *peer, env protocol.CandidateEnvelope) {
	switch env.PC {
	case protocol.PCAudio:
		if err := p.pc.AddICECandidate(env.ICECandidateInit); err != nil {
			log.Printf("sfu: add audio candidate (%s): %v", p.id, err)
		}
	case protocol.PCScreenPub:
		r.mu.Lock()
		session := p.screenSession
		r.mu.Unlock()
		if session == nil {
			log.Printf("sfu: screen-pub candidate (%s) no session", p.id)
			return
		}
		if err := session.publisherPC.AddICECandidate(env.ICECandidateInit); err != nil {
			log.Printf("sfu: screen-pub add candidate (%s): %v", p.id, err)
		}
	case protocol.PCScreenSub:
		r.mu.Lock()
		subPC := p.screenSubs[env.PublisherID]
		r.mu.Unlock()
		if subPC == nil {
			log.Printf("sfu: screen-sub candidate (%s→%s) no PC", p.id, env.PublisherID)
			return
		}
		if err := subPC.AddICECandidate(env.ICECandidateInit); err != nil {
			log.Printf("sfu: screen-sub add candidate (%s→%s): %v", p.id, env.PublisherID, err)
		}
	default:
		log.Printf("sfu: candidate with unknown pc=%q from %s", env.PC, p.id)
	}
}

func (r *Room) handleClientMessage(p *peer, msg protocol.Envelope) {
	if msg.Event == protocol.MsgTypePing {
		r.handlePing(p, msg)
		return
	}

	// Lurkers may only send/retract chat. Silently drop all other message types.
	if p.chatOnly {
		switch msg.Event {
		case "chat-send":
			var cs protocol.ChatSendPayload
			if err := json.Unmarshal(msg.Data, &cs); err != nil {
				return
			}
			r.broadcastChat(p, cs)
		case "chat-delete":
			var cd protocol.ChatDeletePayload
			if err := json.Unmarshal(msg.Data, &cd); err != nil {
				return
			}
			r.broadcastChatDelete(p, cd.ID)
		}
		return
	}

	switch msg.Event {
	case "answer":
		var env protocol.AnswerEnvelope
		if err := json.Unmarshal(msg.Data, &env); err != nil {
			return
		}
		r.handleAnswer(p, env)
	case "candidate":
		var env protocol.CandidateEnvelope
		if err := json.Unmarshal(msg.Data, &env); err != nil {
			return
		}
		r.handleCandidate(p, env)
	case "set-displayname":
		var dn protocol.SetDisplayNamePayload
		if err := json.Unmarshal(msg.Data, &dn); err != nil {
			return
		}
		r.setDisplayName(p.id, dn.DisplayName)
	case "set-state":
		var ss protocol.SetStatePayload
		if err := json.Unmarshal(msg.Data, &ss); err != nil {
			return
		}
		r.setState(p.id, ss.SelfMuted, ss.Deafened)
	case "chat-send":
		var cs protocol.ChatSendPayload
		if err := json.Unmarshal(msg.Data, &cs); err != nil {
			return
		}
		r.broadcastChat(p, cs)
	case "chat-delete":
		var cd protocol.ChatDeletePayload
		if err := json.Unmarshal(msg.Data, &cd); err != nil {
			return
		}
		r.broadcastChatDelete(p, cd.ID)
	case "renegotiate":
		r.mu.Lock()
		if time.Since(p.lastRenegotiateAt) < renegotiateCooldown {
			r.mu.Unlock()
			return
		}
		p.lastRenegotiateAt = time.Now()
		r.mu.Unlock()
		r.signalPeerConnections()
	case "offer":
		// Currently only used for screen-share ICE restart after resume —
		// the publisher's PC stayed alive across WS reconnect, and they
		// renegotiate transport against the rebound session. Audio uses the
		// SFU-as-offerer path; clients never send "offer" for audio.
		var env protocol.OfferEnvelope
		if err := json.Unmarshal(msg.Data, &env); err != nil {
			return
		}
		r.handleClientOffer(p, env)
	case "screen-share-start":
		var d protocol.ScreenShareStartData
		if err := json.Unmarshal(msg.Data, &d); err != nil {
			return
		}
		r.handleScreenShareStart(p, d)
	case "screen-share-stop":
		r.handleScreenShareStop(p)
	case "screen-share-resume":
		var d protocol.ScreenShareResumeData
		if err := json.Unmarshal(msg.Data, &d); err != nil {
			return
		}
		r.handleScreenShareResume(p, d)
	case "screen-share-subscribe":
		var d protocol.ScreenShareSubscribeData
		if err := json.Unmarshal(msg.Data, &d); err != nil {
			return
		}
		r.handleScreenShareSubscribe(p, d)
	case "screen-share-unsubscribe":
		var d protocol.ScreenShareUnsubscribeData
		if err := json.Unmarshal(msg.Data, &d); err != nil {
			return
		}
		r.handleScreenShareUnsubscribe(p, d)
	case "screen-share-mode-change":
		var d protocol.ScreenShareModeChangeData
		if err := json.Unmarshal(msg.Data, &d); err != nil {
			return
		}
		r.handleScreenShareModeChange(p, d)
	}
}

// peerInfo builds a PeerInfo from p's current state. Caller must not hold r.mu
// since this function does not access room state directly.
func peerInfo(p *peer) protocol.PeerInfo {
	return protocol.PeerInfo{
		ID:                      p.id,
		DisplayName:             p.displayName,
		ClientID:                p.clientID,
		SelfMuted:               p.selfMuted,
		Deafened:                p.deafened,
		ChatOnly:                p.chatOnly,
		ScreenSharing:           p.screenSharing,
		ScreenSharingHasAudio:   p.screenSharingHasAudio,
		ScreenSharingVideoCodec: p.screenSharingVideoCodec,
	}
}

// Peers returns a snapshot of the current peers for read-only consumers
// (e.g. the lobby/preview HTTP endpoint).
func (r *Room) Peers() []protocol.PeerInfo {
	r.mu.Lock()
	defer r.mu.Unlock()
	out := make([]protocol.PeerInfo, 0, len(r.peers))
	for _, p := range r.peers {
		out = append(out, peerInfo(p))
	}
	return out
}

func (r *Room) addPeer(p *peer) {
	r.mu.Lock()
	// Evict prior sessions sharing this clientID so reconnects (e.g. network
	// switch) replace the stale peer immediately instead of waiting for ICE
	// timeout, which would otherwise show a phantom self in the room.
	var evicted []*peer
	if p.clientID != "" {
		for id, op := range r.peers {
			if op.clientID == p.clientID {
				delete(r.peers, id)
				r.dropTracksForPeer(id)
				evicted = append(evicted, op)
			}
		}
	}
	existing := make([]protocol.PeerInfo, 0, len(r.peers))
	others := make([]*peer, 0, len(r.peers))
	for _, op := range r.peers {
		existing = append(existing, peerInfo(op))
		others = append(others, op)
	}
	r.peers[p.id] = p
	count := len(r.peers)
	r.mu.Unlock()

	for _, ev := range evicted {
		log.Printf("sfu: evicting prior session id=%s clientId=%q (replaced)", ev.id, ev.clientID)
		if ev.cancel != nil {
			ev.cancel()
		}
		left, _ := json.Marshal(protocol.PeerLeftPayload{ID: ev.id})
		leftEnv, _ := json.Marshal(protocol.Envelope{Event: "peer-left", Data: left})
		for _, op := range others {
			_ = op.writeRaw(leftEnv)
		}
	}

	log.Printf("sfu: peer joined id=%s name=%q clientId=%q chatOnly=%v peers=%d", p.id, p.displayName, p.clientID, p.chatOnly, count)

	welcome, _ := json.Marshal(protocol.WelcomePayload{ID: p.id, Peers: existing})
	_ = p.write(protocol.Envelope{Event: "welcome", Data: welcome})

	joined, _ := json.Marshal(peerInfo(p))
	joinedEnv, _ := json.Marshal(protocol.Envelope{Event: "peer-joined", Data: joined})
	for _, op := range others {
		_ = op.writeRaw(joinedEnv)
	}

	if len(evicted) > 0 {
		r.signalPeerConnections()
	}
	if r.cfg.OnPeerLeft != nil {
		for _, ev := range evicted {
			r.cfg.OnPeerLeft(ev.id)
		}
	}
	if r.cfg.OnPeerJoined != nil {
		r.cfg.OnPeerJoined(peerInfo(p))
	}
}

func (r *Room) removePeer(id string) {
	r.mu.Lock()
	p, ok := r.peers[id]
	if !ok {
		r.mu.Unlock()
		return
	}
	delete(r.peers, id)
	r.dropTracksForPeer(id)
	others := make([]*peer, 0, len(r.peers))
	for _, op := range r.peers {
		others = append(others, op)
	}
	count := len(r.peers)
	// Snapshot screen-share state so we can arm grace timer / close
	// outbound subscriber PCs OUTSIDE the room lock (each pc.Close acquires
	// pion-internal locks we don't want crossing with r.mu).
	session := p.screenSession
	subs := make([]*webrtc.PeerConnection, 0, len(p.screenSubs))
	for _, s := range p.screenSubs {
		subs = append(subs, s)
	}
	p.screenSubs = nil
	r.mu.Unlock()

	// If this peer was subscribed to someone else's screen share, close
	// each subscriber PC. The session.subscribers entry is cleaned up
	// inside removeScreenSubscriber, which we don't need here since the
	// peer is gone — but we still close the PC for clean teardown.
	for _, s := range subs {
		_ = s.Close()
	}
	if session != nil {
		// Publisher is gone. Arm the grace window; reattach must happen
		// from a peer with the same clientId-evicted reconnect, which will
		// land in screen-share-resume.
		r.startScreenShareGrace(session)
	}

	if p.cancel != nil {
		p.cancel()
	}

	log.Printf("sfu: peer left id=%s peers=%d", id, count)

	left, _ := json.Marshal(protocol.PeerLeftPayload{ID: id})
	leftEnv, _ := json.Marshal(protocol.Envelope{Event: "peer-left", Data: left})
	for _, op := range others {
		_ = op.writeRaw(leftEnv)
	}

	if !p.chatOnly {
		r.signalPeerConnections()
	}
	if r.cfg.OnPeerLeft != nil {
		r.cfg.OnPeerLeft(id)
	}
}

// Close stops accepting new peers and tears down all active sessions.
// Safe to call multiple times.
func (r *Room) Close() {
	if !r.closed.CompareAndSwap(false, true) {
		return
	}
	r.mu.Lock()
	peers := make([]*peer, 0, len(r.peers))
	for _, p := range r.peers {
		peers = append(peers, p)
	}
	r.mu.Unlock()

	for _, p := range peers {
		if p.cancel != nil {
			p.cancel()
		}
		_ = p.ws.Close(websocket.StatusGoingAway, "server shutting down")
	}
}

func (r *Room) setDisplayName(id, name string) {
	r.mu.Lock()
	p, ok := r.peers[id]
	if !ok {
		r.mu.Unlock()
		return
	}
	p.displayName = name
	info := peerInfo(p)
	others := r.othersLocked(id)
	r.mu.Unlock()

	infoData, _ := json.Marshal(info)
	infoEnv, _ := json.Marshal(protocol.Envelope{Event: "peer-info", Data: infoData})
	for _, op := range others {
		_ = op.writeRaw(infoEnv)
	}
	if r.cfg.OnPeerUpdated != nil {
		r.cfg.OnPeerUpdated(info)
	}
}

func (r *Room) setState(id string, selfMuted, deafened bool) {
	r.mu.Lock()
	p, ok := r.peers[id]
	if !ok {
		r.mu.Unlock()
		return
	}
	p.selfMuted = selfMuted
	p.deafened = deafened
	info := peerInfo(p)
	others := r.othersLocked(id)
	r.mu.Unlock()

	state, _ := json.Marshal(protocol.PeerStatePayload{ID: id, SelfMuted: selfMuted, Deafened: deafened})
	stateEnv, _ := json.Marshal(protocol.Envelope{Event: "peer-state", Data: state})
	for _, op := range others {
		_ = op.writeRaw(stateEnv)
	}
	if r.cfg.OnPeerUpdated != nil {
		r.cfg.OnPeerUpdated(info)
	}
}

func (r *Room) broadcastChat(sender *peer, cs protocol.ChatSendPayload) {
	if sender.displayName == "" {
		slog.Debug("sfu: chat-send before hello, dropping", "peer", sender.id)
		return
	}

	text := strings.TrimSpace(cs.Text)
	if text == "" && len(cs.Attachments) == 0 {
		slog.Debug("sfu: chat-send empty, dropping", "peer", sender.id)
		return
	}
	if len([]byte(text)) > protocol.ChatMaxBytes {
		slog.Debug("sfu: chat-send oversized, dropping", "peer", sender.id, "bytes", len([]byte(text)))
		return
	}
	if !r.attachmentsValid(sender, cs.Attachments) {
		return
	}

	now := time.Now()
	id := newChatID(now)

	slog.Debug("sfu: chat", "id", id, "from", sender.id, "bytes", len([]byte(text)), "attachments", len(cs.Attachments))

	payload, _ := json.Marshal(protocol.ChatPayload{
		ID:          id,
		From:        sender.id,
		Text:        text,
		Ts:          now.UnixMilli(),
		ClientMsgID: cs.ClientMsgID,
		SenderName:  sender.displayName,
		Attachments: cs.Attachments,
	})
	env, _ := json.Marshal(protocol.Envelope{Event: "chat", Data: payload})

	for _, p := range r.snapshotPeers() {
		_ = p.writeRaw(env)
	}
}

// snapshotPeers returns a copy of the current peer set, safe to range over
// without holding r.mu.
func (r *Room) snapshotPeers() []*peer {
	r.mu.Lock()
	defer r.mu.Unlock()
	all := make([]*peer, 0, len(r.peers))
	for _, p := range r.peers {
		all = append(all, p)
	}
	return all
}

// broadcastChatDelete echoes a retraction to all peers including the requester,
// whose own removal rides this echo. Authorship is not verified — chat is never
// persisted, so the server can't know who sent a given ID; the affordance is
// gated to own messages client-side, like display names and mute state already are.
func (r *Room) broadcastChatDelete(sender *peer, id string) {
	if id == "" {
		slog.Debug("sfu: chat-delete empty id, dropping", "peer", sender.id)
		return
	}

	slog.Debug("sfu: chat-delete", "id", id, "from", sender.id)

	payload, _ := json.Marshal(protocol.ChatDeletedPayload{ID: id})
	env, _ := json.Marshal(protocol.Envelope{Event: "chat-deleted", Data: payload})

	for _, p := range r.snapshotPeers() {
		_ = p.writeRaw(env)
	}
}

// attachmentsValid reports whether every attachment refers to a live upload in
// this room. An empty list is trivially valid. When attachments are present but
// no file store is configured the message is rejected (feature disabled).
func (r *Room) attachmentsValid(sender *peer, atts []protocol.Attachment) bool {
	if len(atts) == 0 {
		return true
	}
	if len(atts) > protocol.ChatMaxAttachments {
		slog.Debug("sfu: chat-send too many attachments, dropping", "peer", sender.id, "count", len(atts))
		return false
	}
	if r.cfg.FileStore == nil {
		slog.Debug("sfu: chat-send with attachments but no file store, dropping", "peer", sender.id)
		return false
	}
	for _, a := range atts {
		if a.UploadID == "" || a.MIME == "" {
			slog.Debug("sfu: chat-send attachment missing fields, dropping", "peer", sender.id)
			return false
		}
		entry, ok := r.cfg.FileStore.Get(a.UploadID)
		if !ok || entry.RoomID != r.cfg.RoomID {
			slog.Debug("sfu: chat-send attachment not in room, dropping", "peer", sender.id, "uploadId", a.UploadID)
			return false
		}
	}
	return true
}

func (r *Room) handlePing(p *peer, msg protocol.Envelope) {
	var pc protocol.PingClient
	if err := json.Unmarshal(msg.Data, &pc); err != nil || pc.To == "" {
		return
	}
	if pc.To == p.id {
		return
	}

	r.mu.Lock()
	target := r.peers[pc.To]
	if target == nil {
		r.mu.Unlock()
		return
	}
	if time.Since(target.lastPingReceivedAt) < pingCooldown {
		r.mu.Unlock()
		return
	}
	target.lastPingReceivedAt = time.Now()
	r.mu.Unlock()

	payload, _ := json.Marshal(protocol.PingServer{From: p.id, FromName: p.displayName})
	env, _ := json.Marshal(protocol.Envelope{Event: protocol.MsgTypePing, Data: payload})
	_ = target.writeRaw(env)
}

func newChatID(t time.Time) string {
	return ulid.MustNew(ulid.Timestamp(t), rand.Reader).String()
}

// trackKey doubles as map key and wire track ID. The kind suffix lets a
// receiving client tell a publisher's audio and video tracks apart even
// though they share the same StreamID.
func trackKey(ownerID, kind string) string {
	return ownerID + ":" + kind
}

func ownerOf(key string) string {
	if i := strings.IndexByte(key, ':'); i > 0 {
		return key[:i]
	}
	return key
}

func (r *Room) publishTrack(key string, t *webrtc.TrackLocalStaticRTP) {
	r.mu.Lock()
	r.tracks[key] = t
	r.mu.Unlock()
	r.signalPeerConnections()
}

func (r *Room) unpublishTrack(key string) {
	r.mu.Lock()
	delete(r.tracks, key)
	r.mu.Unlock()
	r.signalPeerConnections()
}

// dropTracksForPeer clears all tracks for ownerID. Caller must hold r.mu.
func (r *Room) dropTracksForPeer(ownerID string) {
	for k := range r.tracks {
		if ownerOf(k) == ownerID {
			delete(r.tracks, k)
		}
	}
}

// othersLocked snapshots the peers other than excludeID. Caller must hold r.mu.
func (r *Room) othersLocked(excludeID string) []*peer {
	others := make([]*peer, 0, len(r.peers))
	for _, op := range r.peers {
		if op.id != excludeID {
			others = append(others, op)
		}
	}
	return others
}

// bitrateToTIDCap returns bwCapNone above the high threshold so a healthy
// link respects the user's chosen quality.
func bitrateToTIDCap(bitrate int) uint32 {
	switch {
	case bitrate >= bweHighThresholdBps:
		return bwCapNone
	case bitrate >= bweMidThresholdBps:
		return 1
	default:
		return 0
	}
}

func newPeerID() string {
	var b [8]byte
	if _, err := rand.Read(b[:]); err != nil {
		return time.Now().Format("150405.000000000")
	}
	return hex.EncodeToString(b[:])
}
