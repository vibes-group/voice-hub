// Package protocol defines the named types for the voice-hub signaling
// protocol. Go is the source of truth; the TypeScript mirror lives in
// frontend/src/sfu/protocol.ts and is updated by hand.
//
// Wire format: every message is wrapped in an Envelope:
//
//	{ "event": "<name>", "data": <payload> }
//
// Payload types in this file cover all custom server↔client messages.
// The offer/answer/candidate messages use pion-native types directly:
//   - offer/answer: webrtc.SessionDescription   (pion/webrtc/v4)
//   - candidate:    webrtc.ICECandidateInit      (pion/webrtc/v4)
//
// Those pion types map to the browser's RTCSessionDescriptionInit and
// RTCIceCandidateInit respectively; TS does not need custom mirrors for them.
package protocol

import (
	"encoding/json"

	"github.com/pion/webrtc/v4"
)

// PCKind discriminates the PeerConnection role each offer/answer/candidate
// message targets. Required (no omitempty) — router on either side must be
// able to dispatch on it; a missing value is a wire-format bug, not a default.
type PCKind string

const (
	PCAudio     PCKind = "audio"
	PCScreenPub PCKind = "screen-pub"
	PCScreenSub PCKind = "screen-sub"
)

// ScreenVideoCodec is the normalized video codec name used by screen-share
// signaling. SDP still carries exact codec parameters; this value is a small
// UI/subscription hint for clients before they create a subscriber PC.
type ScreenVideoCodec string

const (
	ScreenVideoCodecAV1 ScreenVideoCodec = "av1"
	ScreenVideoCodecVP9 ScreenVideoCodec = "vp9"
)

// ScreenShareMode lets the streamer pick the adaptation policy the SFU
// applies for their share. Sharp protects readability — when bandwidth
// tightens the SFU drops FPS first, keeping resolution and bitrate where
// they are. Motion protects smoothness — the SFU is more tolerant of
// loss before dropping FPS, and the floor is one notch higher.
//
// Mode is orthogonal to the resolution/FPS preset. The publisher sets
// both at start; mode can be changed mid-share via screen-share-mode-change
// without renegotiating.
type ScreenShareMode string

const (
	ScreenShareModeSharp  ScreenShareMode = "sharp"
	ScreenShareModeMotion ScreenShareMode = "motion"
)

// IsValid reports whether m is one of the defined ScreenShareMode constants.
// Empty / unknown values fall back to sharp at the call site.
func (m ScreenShareMode) IsValid() bool {
	return m == ScreenShareModeSharp || m == ScreenShareModeMotion
}

// OfferEnvelope is the data field of every "offer" message. PC discriminates
// the target PeerConnection. PublisherID is set only when PC=screen-sub and
// names the publisher whose stream this subscriber PC is being offered.
//
// Embedded SessionDescription means {type, sdp} hoist to the top-level JSON
// object — preserving the wire format the browser already produces from
// RTCSessionDescription.toJSON().
type OfferEnvelope struct {
	PC                        PCKind `json:"pc"`
	PublisherID               string `json:"publisherId,omitempty"`
	webrtc.SessionDescription        // {type, sdp}
}

// AnswerEnvelope mirrors OfferEnvelope for the "answer" event.
type AnswerEnvelope struct {
	PC                        PCKind `json:"pc"`
	PublisherID               string `json:"publisherId,omitempty"`
	webrtc.SessionDescription        // {type, sdp}
}

// CandidateEnvelope wraps a trickle ICE candidate. The pion ICECandidateInit
// shape ({candidate, sdpMid, sdpMLineIndex, usernameFragment}) is preserved
// at the top level via embedding.
type CandidateEnvelope struct {
	PC                      PCKind `json:"pc"`
	PublisherID             string `json:"publisherId,omitempty"`
	webrtc.ICECandidateInit        // {candidate, sdpMid, sdpMLineIndex, usernameFragment}
}

// Envelope is the top-level JSON wrapper for every signaling message.
// It replaces the legacy sfu.Message type; the wire format is identical.
type Envelope struct {
	Event string          `json:"event"`
	Data  json.RawMessage `json:"data,omitempty"`
}

// PeerInfo is the canonical peer descriptor used in welcome, peer-joined,
// and peer-info messages. DisplayName is omitted from JSON when empty so
// that peer-left (which intentionally carries no display name) does not
// emit a spurious empty string. ClientID is the stable per-install
// identifier reported by the client in HelloPayload; it is echoed in
// every peer descriptor so other clients can key per-peer UI prefs
// (e.g. volume sliders) by something that survives reconnects.
// SelfMuted and Deafened default to false on join and reflect the peer's
// last reported audio state as set via set-state.
// ChatOnly is true for lurker (chat-only) peers. Lurker peers are included
// in the room roster (welcome.peers and peer-joined/peer-left broadcasts)
// so all clients can display them; they should be rendered visually distinct
// and sorted below voice peers. Omitted from JSON when false (voice peers).
//
// ScreenSharing flips on/off as the server starts/stops forwarding a video
// track from this peer. Lurkers never receive media, so this flag is their
// only signal that a share is active.
type PeerInfo struct {
	ID                      string           `json:"id"`
	DisplayName             string           `json:"displayName,omitempty"`
	ClientID                string           `json:"clientId,omitempty"`
	SelfMuted               bool             `json:"selfMuted,omitempty"`
	Deafened                bool             `json:"deafened,omitempty"`
	ChatOnly                bool             `json:"chatOnly,omitempty"`
	ScreenSharing           bool             `json:"screenSharing,omitempty"`
	ScreenSharingHasAudio   bool             `json:"screenSharingHasAudio,omitempty"`
	ScreenSharingVideoCodec ScreenVideoCodec `json:"screenSharingVideoCodec,omitempty"`
}

// --- Server → Client payloads ---

// WelcomePayload is the data field of the "welcome" message, sent to a
// newly connected peer. Peers is the room snapshot at the moment of join
// (excludes the joining peer itself).
type WelcomePayload struct {
	ID    string     `json:"id"`
	Peers []PeerInfo `json:"peers"`
}

// PeerLeftPayload is the data field of the "peer-left" message. Only the
// peer ID is carried; display name is intentionally absent.
type PeerLeftPayload struct {
	ID string `json:"id"`
}

// --- Client → Server payloads ---

// HelloPayload is the data field of the "hello" message. It must be the
// first message sent by the client after the WebSocket handshake.
// A 10-second server timeout applies.
//
// ClientID is a stable opaque identifier the client generates once on first
// launch and persists locally (e.g. localStorage). It survives reconnects
// and is echoed back to all peers in PeerInfo so they can key per-peer UI
// state (volume, mute, etc.) by it instead of the ephemeral per-connection
// peer ID. May be empty for older clients; consumers must treat absence as
// "no stable identity available".
//
// ChatOnly, when true, places the connecting client in lurker mode:
//   - The server allocates a peer ID and includes the lurker in the room roster
//     with PeerInfo.ChatOnly=true. Lurkers are visible to all participants.
//   - A "peer-joined" broadcast (PeerInfo.ChatOnly=true) is sent to all peers
//     (voice and lurker) when a lurker connects.
//   - A "peer-left" broadcast is sent to all peers when a lurker disconnects.
//   - Lurkers are included in welcome.peers (with ChatOnly=true) for all clients
//     (voice peers and other lurkers). Symmetric: lurkers see the full roster too.
//   - The server sends "welcome" to the lurker with its assigned peer ID;
//     welcome.peers includes all currently connected peers (voice and lurker).
//   - The lurker MAY send "chat-send" and will receive all "chat" broadcasts.
//   - If the lurker sends "offer", "answer", "candidate", "set-state", or
//     "set-displayname", the server silently drops the message (matches the
//     existing silent-drop pattern for unexpected messages).
//   - Lurker peer IDs use the same opaque format as voice peer IDs.
//   - Default false; omitted from JSON when false (backward compatible).
type HelloPayload struct {
	DisplayName string `json:"displayName"`
	ClientID    string `json:"clientId,omitempty"`
	ChatOnly    bool   `json:"chatOnly,omitempty"`
}

// SetDisplayNamePayload is the data field of the "set-displayname" message,
// sent mid-session when the user changes their display name. Structurally
// identical to HelloPayload but kept separate: hello is session-init,
// set-displayname is a mid-session update with different server-side handling.
type SetDisplayNamePayload struct {
	DisplayName string `json:"displayName"`
}

// SetStatePayload is the data field of the "set-state" message, sent
// mid-session whenever the peer toggles mic mute or self-deafen. Server
// updates the peer's stored state and broadcasts "peer-state" to others.
type SetStatePayload struct {
	SelfMuted bool `json:"selfMuted"`
	Deafened  bool `json:"deafened"`
}

// PeerStatePayload is the data field of the "peer-state" message,
// broadcast when a peer toggles mic mute or self-deafen.
type PeerStatePayload struct {
	ID        string `json:"id"`
	SelfMuted bool   `json:"selfMuted"`
	Deafened  bool   `json:"deafened"`
}

// --- Presence (cross-room) ---
//
// /ws/presence carries four event types:
//   - "presence-snapshot": full rooms map. Sent once on connect, and again
//     after reconnect. Clients replace their local roster verbatim.
//   - "presence-peer-joined": single peer added to one room.
//   - "presence-peer-left":   single peer removed from one room.
//   - "presence-peer-updated": existing peer's fields changed (display name,
//     selfMuted, deafened).
//
// Deltas are NOT sequenced/resumable. On reconnect, clients discard local
// state and wait for the next snapshot — server sends it as the first frame.
//
// Deltas MUST be applied idempotently: a peer-joined for an already-known ID
// replaces in place; peer-left for an unknown ID is a no-op; peer-updated
// replaces by ID, inserting if absent. The reason: a delta can race a fresh
// subscriber's snapshot — the snapshot may already reflect the event, or the
// delta may arrive describing state already-superseded. Idempotent merge keeps
// the client converged either way.

const (
	PresenceSnapshotEvent    = "presence-snapshot"
	PresencePeerJoinedEvent  = "presence-peer-joined"
	PresencePeerLeftEvent    = "presence-peer-left"
	PresencePeerUpdatedEvent = "presence-peer-updated"
)

type PresenceRoom struct {
	Peers []PeerInfo `json:"peers"`
}

// PresenceSnapshotPayload is the data field of "presence-snapshot". Map key
// is the room slug.
type PresenceSnapshotPayload struct {
	Rooms map[string]PresenceRoom `json:"rooms"`
}

// PresencePeerJoinedPayload is the data field of "presence-peer-joined".
type PresencePeerJoinedPayload struct {
	Room string   `json:"room"`
	Peer PeerInfo `json:"peer"`
}

// PresencePeerLeftPayload is the data field of "presence-peer-left".
type PresencePeerLeftPayload struct {
	Room string `json:"room"`
	ID   string `json:"id"`
}

// PresencePeerUpdatedPayload is the data field of "presence-peer-updated".
// The full PeerInfo is sent; clients replace the peer entry by ID.
type PresencePeerUpdatedPayload struct {
	Room string   `json:"room"`
	Peer PeerInfo `json:"peer"`
}

// --- Text chat ---

// MsgTypePing is the event name for the ping signaling message (both C→S and S→C).
const MsgTypePing = "ping"

// PingClient is the payload of the C→S "ping" message.
type PingClient struct {
	To string `json:"to"`
}

// PingServer is the payload of the S→C "ping" broadcast sent to all peers
// except the sender.
type PingServer struct {
	From     string `json:"from"`
	FromName string `json:"fromName"`
}

// ChatMaxBytes is the maximum allowed UTF-8 byte length for a chat message
// text field. 2000 bytes matches Discord's well-understood limit, fits
// comfortably in a single WebSocket frame, and keeps server logs readable.
// Note: 2000 bytes is tighter than 2000 chars for multi-byte scripts (e.g.
// ~667 CJK characters), which is intentional — the server validates
// len([]byte(text)) not len([]rune(text)).
const ChatMaxBytes = 2000

// ChatSendPayload is the data field of the "chat-send" C→S message.
// It is rejected by the server if:
//   - the peer has not yet sent "hello" (hello is required session-init)
//   - Text is empty after trimming whitespace
//   - len([]byte(Text)) > ChatMaxBytes
//
// ClientMsgID is a client-generated opaque dedup key (recommended: UUIDv4 or
// similar random string). The server echoes it unchanged in the ChatPayload
// broadcast so the sender can reconcile its optimistic local entry with the
// canonical server-assigned ID and timestamp. No uniqueness guarantee is
// enforced server-side; clients must treat it as advisory only.
type ChatSendPayload struct {
	Text        string `json:"text"`
	ClientMsgID string `json:"clientMsgId"`
}

// ChatPayload is the data field of the "chat" S→C message. The server
// broadcasts this to ALL peers in the room including the original sender.
// Echo-to-sender is intentional: it delivers the canonical server-assigned ID
// and timestamp so the sender can reconcile its optimistic local entry. The
// sender matches on ClientMsgID; all other peers can ignore that field.
//
// ID is a ULID (https://github.com/oklog/ulid): 26-char base32, lexicographically
// sortable by creation time, no central coordination required. Using ULID rather
// than a plain integer avoids ordering gaps if multiple rooms run concurrently.
//
// Ts is the server-assigned Unix timestamp in milliseconds. It is redundant
// with the timestamp encoded in ID but is kept as a plain int64 for easy
// construction of a JS Date without parsing the ULID.
//
// ClientMsgID echoes the value from ChatSendPayload unchanged. All clients
// receive it; non-senders may ignore it. Keeping it unconditional simplifies
// the broadcast path (no per-connection filtering required).
//
// SenderName is the display name of the sender at the moment the message was
// broadcast, taken from the sender's hello'd DisplayName. It is included so
// that receiving clients can render the correct name in two cases where the
// participants map lookup on From would fail:
//   - The sender is a lurker (chat-only peer): voice peers never receive a
//     "peer-joined" for lurkers, so From is not in their participants map.
//   - The sender has left the room before the recipient renders the message
//     (e.g. on reconnect with a local chat history replay).
//
// Clients SHOULD prefer SenderName over a stale participants-map entry when
// both are available, since the server snapshot is authoritative at send time.
// Omitted from JSON when empty (older server versions or empty display names).
type ChatPayload struct {
	ID          string `json:"id"`
	From        string `json:"from"`
	Text        string `json:"text"`
	Ts          int64  `json:"ts"`
	ClientMsgID string `json:"clientMsgId,omitempty"`
	SenderName  string `json:"senderName,omitempty"`
}

// --- Screen share ---
//
// Lifecycle events (see tmp/screen-share-plan.md for the full state machine):
//
//   C→S "screen-share-start"      (sdp+hasSystemAudio; publisher creates a 2nd PC)
//   S→C "screen-share-started"    (sessionToken; FIFO writer guarantees this
//                                  is delivered BEFORE the matching answer)
//   S→all "screen-share-available" (publisherId; sent after OnTrack — i.e.
//                                  the first RTP packet has arrived and the
//                                  TrackLocalStaticRTP is wired up)
//   C→S "screen-share-stop"       (no payload; publisher's own peerID is
//                                  implicit in the WS connection)
//   S→all "screen-share-ended"    (publisherId; SFU broadcasts on stop,
//                                  grace-timer expiry, or publisher disconnect)
//   C→S "screen-share-resume"     (sessionToken; reattach to live session
//                                  after a WS reconnect within the 5s grace)
//
//   C→S "screen-share-subscribe"  (publisherId + preferredTemporalLayer)
//   C→S "screen-share-unsubscribe"(publisherId)
//
//   S→C "screen-share-encode-pause" / "screen-share-encode-resume"
//     (publisher-side dynacast: SFU tells the publisher which layers to
//     stop / restart encoding. Distinct from subscriber-side layer-select.)
//
//   S→C "screen-share-error"      (publisherId? + reason; any failure path)
//
// sessionToken is a 32-byte cryptorandom value, base64-encoded. Treated as
// opaque by the client; only the server validates it on resume.

// ScreenShareReason names the discrete failure modes ScreenShareErrorData
// can carry. New constants must stay in sync with the TS literal union in
// frontend/src/sfu/protocol.ts.
type ScreenShareReason string

const (
	ReasonNotFound          ScreenShareReason = "not-found"
	ReasonInvalidToken      ScreenShareReason = "invalid-token"
	ReasonAlreadyPublishing ScreenShareReason = "already-publishing"
	ReasonInternal          ScreenShareReason = "internal"
)

// ScreenShareStartData — C→S. SDP is the publisher's offer.sdp. HasSystemAudio
// tells the server whether the second m-section in the offer carries the
// system-audio Opus track; the server uses this to decide whether to pre-
// create an audio TrackLocalStaticRTP for fan-out.
type ScreenShareStartData struct {
	SDP            string          `json:"sdp"`
	HasSystemAudio bool            `json:"hasSystemAudio"`
	Mode           ScreenShareMode `json:"mode,omitempty"`
}

// ScreenShareResumeData — C→S. Sent after a WS reconnect to reattach to a
// publisher session that is still alive in the server's grace window.
type ScreenShareResumeData struct {
	SessionToken string `json:"sessionToken"`
}

// ScreenShareStartedData — S→C. Ack for screen-share-start. The server writes
// it onto the publisher's outbound queue immediately before the answer, and
// the FIFO writeLoop preserves that order on the wire.
type ScreenShareStartedData struct {
	SessionToken string `json:"sessionToken"`
}

// ScreenShareAvailableData — S→all. Broadcast on first OnTrack (not on
// receipt of -start), so subscribers only render a tile once media is
// actually wired up and ready to forward.
type ScreenShareAvailableData struct {
	PublisherID    string           `json:"publisherId"`
	HasSystemAudio bool             `json:"hasSystemAudio"`
	VideoCodec     ScreenVideoCodec `json:"videoCodec,omitempty"`
	Mode           ScreenShareMode  `json:"mode,omitempty"`
}

// ScreenShareEndedData — S→all. Sent on publisher stop, grace expiry, or
// disconnect. Subscribers must close their subscriber PC on receipt.
type ScreenShareEndedData struct {
	PublisherID string `json:"publisherId"`
}

// ScreenShareErrorData — S→C. PublisherID is empty for errors not tied to a
// specific publisher session (e.g. an invalid-token resume that no longer
// has a session to reference).
type ScreenShareErrorData struct {
	PublisherID string            `json:"publisherId,omitempty"`
	Reason      ScreenShareReason `json:"reason"`
}

// ScreenShareSubscribeData — C→S. preferredTemporalLayer is a hint, not a
// guarantee: the server may downgrade it based on subscriber BWE.
type ScreenShareSubscribeData struct {
	PublisherID            string `json:"publisherId"`
	PreferredTemporalLayer uint8  `json:"preferredTemporalLayer"`
}

// ScreenShareUnsubscribeData — C→S. Server closes the subscriber PC and frees
// the per-subscriber forward state.
type ScreenShareUnsubscribeData struct {
	PublisherID string `json:"publisherId"`
}

// ScreenShareModeChangeData — C→S. Sent by the publisher mid-share to swap
// the adaptation mode without renegotiating. Server updates the session's
// mode, broadcasts screen-share-mode-changed to viewers, and sends a PLI to
// the publisher so the encoder hands out a fresh keyframe under the new hint.
type ScreenShareModeChangeData struct {
	Mode ScreenShareMode `json:"mode"`
}

// ScreenShareModeChangedData — S→all. Broadcast after a successful
// ScreenShareModeChange so viewers can update any mode-derived UI state.
type ScreenShareModeChangedData struct {
	PublisherID string          `json:"publisherId"`
	Mode        ScreenShareMode `json:"mode"`
}

// ScreenShareEncodePauseData — S→C (to publisher). Dynacast: tells the
// publisher which encodings can stop. Empty / missing Layers means "all".
//
// Layers is []int (not []uint8) so encoding/json emits a JSON array of
// numbers; []uint8 would be base64-encoded as a byte slice.
type ScreenShareEncodePauseData struct {
	Layers []int `json:"layers"`
}

// ScreenShareEncodeResumeData — S→C (to publisher). Counterpart to pause.
type ScreenShareEncodeResumeData struct {
	Layers []int `json:"layers"`
}
