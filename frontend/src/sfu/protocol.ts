// Signaling protocol types — TS mirror of backend/internal/sfu/protocol/messages.go.
// Go is the source of truth. Field renames in Go break the golden-JSON fixtures
// (backend/internal/sfu/protocol/testdata/*.json), which the Vitest suite reads.
//
// Wire format: { "event": "<name>", "data": <payload> }
//
// For offer/answer/candidate the data fields are browser-native types:
//   offer/answer  → RTCSessionDescriptionInit  (W3C spec)
//   candidate     → RTCIceCandidateInit         (W3C spec)
// No custom mirrors needed for those.

// Shared

/**
 * PCKind discriminates the RTCPeerConnection role every offer / answer /
 * candidate message targets. Required on every signaling message — a missing
 * value is a wire-format bug, not a default. Mirror of protocol.PCKind in Go.
 *
 *  - audio       → the single audio PC each peer maintains
 *  - screen-pub  → the publisher's screen-share PC (one per active share)
 *  - screen-sub  → a subscriber PC scoped to one publisher (publisherId required)
 */
export type PCKind = 'audio' | 'screen-pub' | 'screen-sub';

/**
 * Wire shape of "offer" / "answer" data: SessionDescription fields hoisted
 * to the top level, plus the PC discriminator. PublisherID is present iff
 * pc='screen-sub'.
 */
export type OfferEnvelope = RTCSessionDescriptionInit & {
  pc: PCKind;
  publisherId?: string;
};

export type AnswerEnvelope = RTCSessionDescriptionInit & {
  pc: PCKind;
  publisherId?: string;
};

export type CandidateEnvelope = RTCIceCandidateInit & {
  pc: PCKind;
  publisherId?: string;
};

/** Canonical peer descriptor, used in welcome / peer-joined / peer-info. */
export type PeerInfo = {
  id: string;
  displayName?: string;
  /**
   * Stable per-install identifier the peer reported in its `hello`. Survives
   * reconnects (unlike `id`, which is regenerated per WS connection). Use
   * this to key per-peer UI prefs (e.g. local volume slider). May be absent
   * for older clients.
   */
  clientId?: string;
  selfMuted?: boolean;
  deafened?: boolean;
  /**
   * True for lurker (chat-only) peers. Lurkers are included in the room
   * roster (welcome.peers, peer-joined, peer-left) and should be rendered
   * visually distinct and sorted below voice peers in the participants list.
   * Absent / false for normal voice peers.
   */
  chatOnly?: boolean;
  /**
   * Server-driven "is currently screen-sharing" flag. Lurkers rely on it
   * entirely; voice peers can use it before the video track has arrived.
   */
  screenSharing?: boolean;
  /**
   * Server-driven flag: true iff the active screen share also carries a
   * system-audio Opus track. Meaningful only when screenSharing=true.
   */
  screenSharingHasAudio?: boolean;
  screenSharingVideoCodec?: ScreenVideoCodec;
};

export type ScreenVideoCodec = 'av1' | 'vp9';

/**
 * Publisher's adaptation mode for a screen share. Picked at start and
 * swappable mid-share via the screen-share-mode-change message.
 *
 *  - sharp  → biases the encoder ('text' contentHint), uses a tighter
 *             bitrate cap, and tells the SFU to drop FPS first when
 *             bandwidth tightens. Floor is the lowest temporal layer.
 *  - motion → biases the encoder ('motion' contentHint), uses the full
 *             bitrate cap, and tells the SFU to tolerate more loss before
 *             dropping FPS. Floor is the middle temporal layer so the
 *             stream never falls below half-FPS.
 */
export type ScreenShareMode = 'sharp' | 'motion';

// Server → Client payloads

/** Data field of the "welcome" message. Sent once, on connect. */
export type WelcomePayload = {
  id: string;
  peers: PeerInfo[];
};

/** Data field of "peer-left". displayName intentionally absent. */
export type PeerLeftPayload = {
  id: string;
};

// peer-joined and peer-info reuse PeerInfo directly (no wrapper).

/** Data field of "peer-state" — broadcast when a peer's mic/deafen state changes. */
export type PeerStatePayload = {
  id: string;
  selfMuted: boolean;
  deafened: boolean;
};

// Client → Server payloads

/** Data field of "hello" — first message after WS open. */
export type HelloPayload = {
  displayName: string;
  /**
   * Stable per-install identifier generated once on first launch and stored
   * in localStorage. Echoed back by the server in PeerInfo so peers can key
   * per-peer UI prefs by something that survives reconnects.
   */
  clientId: string;
  /**
   * When true, places this connection in lurker (chat-only) mode:
   * - Server includes the lurker in the room roster with PeerInfo.chatOnly=true.
   * - "peer-joined" (chatOnly=true) is broadcast to all peers on connect;
   *   "peer-left" is broadcast to all peers on disconnect.
   * - Lurker is included in welcome.peers for all clients (symmetric visibility).
   * - welcome.peers sent to the lurker includes all current peers (voice + lurker).
   * - Lurker may send "chat-send" and receives all "chat" broadcasts.
   * - Lurker must NOT send offer / answer / candidate / set-state / set-displayname
   *   (server silently drops them).
   * Omit or set false for normal voice-participant behaviour (default).
   */
  chatOnly?: boolean;
};

/** Data field of "set-displayname" — mid-session display name update. */
export type SetDisplayNamePayload = {
  displayName: string;
};

/** Data field of "set-state" — sent when local mic mute or deafen state changes. */
export type SetStatePayload = {
  selfMuted: boolean;
  deafened: boolean;
};

// Text chat

/**
 * Maximum UTF-8 byte length the server accepts for a chat message.
 * Mirror of protocol.ChatMaxBytes in messages.go.
 * Validate with: new TextEncoder().encode(text).length <= CHAT_MAX_BYTES
 */
export const CHAT_MAX_BYTES = 2000;

/** Max attachments per chat message; mirrors protocol.ChatMaxAttachments. */
export const CHAT_MAX_ATTACHMENTS = 10;

/** Renders as an inline image preview ('image') or a download card ('file'). */
export type AttachmentKind = 'image' | 'file';

/**
 * Lightweight metadata for one chat attachment. The bytes travel over HTTP
 * (upload/download endpoints, keyed by uploadId), never over the WS. Mirrors
 * protocol.Attachment in the Go backend.
 *
 * width/height/blurThumb are image-only hints: dimensions reserve layout space
 * without a reflow, blurThumb is a tiny base64 JPEG data URL placeholder shown
 * until the full image resolves.
 */
export type Attachment = {
  uploadId: string;
  kind: AttachmentKind;
  name: string;
  mime: string;
  size: number;
  width?: number;
  height?: number;
  blurThumb?: string;
};

/**
 * Data field of "chat-send" (C→S).
 *
 * Rejected by server if: peer has not sent "hello" yet; text is empty after
 * trim AND there are no attachments; text UTF-8 byte length exceeds
 * CHAT_MAX_BYTES; more than CHAT_MAX_ATTACHMENTS attachments; any attachment
 * references an upload not live in this room.
 *
 * clientMsgId is a client-generated dedup key (UUIDv4 recommended). Echoed
 * back in the ChatPayload broadcast so the sender can reconcile its optimistic
 * local entry with the canonical server-assigned id and ts.
 */
export type ChatSendPayload = {
  text: string;
  clientMsgId: string;
  attachments?: Attachment[];
};

/**
 * Data field of "chat" (S→C). Broadcast to all peers including the sender.
 *
 * id — ULID assigned by the server; lexicographically sortable by creation
 *      time. Use as the stable local key for persisted chat history.
 * from — peer id of the sender (matches PeerInfo.id).
 * ts — server Unix timestamp in milliseconds; use for display only. Redundant
 *      with the timestamp in `id` but avoids parsing the ULID in JS.
 * clientMsgId — echoed from ChatSendPayload; present on all broadcasts.
 *               Sender uses it to find and replace its optimistic local entry.
 *               Other peers may ignore it.
 * senderName — display name of the sender at broadcast time, set by the server
 *              from the sender's hello'd displayName. Present when non-empty.
 *              Use this when the participants map lookup on `from` misses —
 *              two cases: (1) sender is a lurker (chat-only peer, never in
 *              participants map); (2) sender left before the message is rendered.
 *              Prefer this over a stale participants-map entry when both exist.
 */
export type ChatPayload = {
  id: string;
  from: string;
  text: string;
  ts: number;
  clientMsgId?: string;
  senderName?: string;
  attachments?: Attachment[];
};

// Screen share — mirror of protocol/messages.go Screen* types.
// See backend file for the lifecycle / state-machine notes; this file only
// names the wire shapes.

export type ScreenShareReason = 'not-found' | 'invalid-token' | 'already-publishing' | 'internal';

export type ScreenShareStartPayload = {
  sdp: string;
  hasSystemAudio: boolean;
  mode?: ScreenShareMode;
};

/** C→S — swap the active mode on a live share without renegotiating. */
export type ScreenShareModeChangePayload = {
  mode: ScreenShareMode;
};

/** S→all — broadcast after a successful mode change. */
export type ScreenShareModeChangedPayload = {
  publisherId: string;
  mode: ScreenShareMode;
};

/** Server-issued ack carrying the resume token. Delivered BEFORE the matching answer. */
export type ScreenShareStartedPayload = {
  sessionToken: string;
};

export type ScreenShareResumePayload = {
  sessionToken: string;
};

export type ScreenShareAvailablePayload = {
  publisherId: string;
  hasSystemAudio: boolean;
  videoCodec?: ScreenVideoCodec;
  mode?: ScreenShareMode;
};

export type ScreenShareEndedPayload = {
  publisherId: string;
};

export type ScreenShareErrorPayload = {
  publisherId?: string;
  reason: ScreenShareReason;
};

export type ScreenShareSubscribePayload = {
  publisherId: string;
  preferredTemporalLayer: number;
};

export type ScreenShareUnsubscribePayload = {
  publisherId: string;
};

export type ScreenShareEncodeLayersPayload = {
  layers: number[];
};

// Discriminated union — all server→client variants

export type PingPayload = {
  from: string;
  fromName: string;
};

export type ServerMessage =
  | { event: 'welcome'; data: WelcomePayload }
  | { event: 'peer-joined'; data: PeerInfo }
  | { event: 'peer-left'; data: PeerLeftPayload }
  | { event: 'peer-info'; data: PeerInfo }
  | { event: 'peer-state'; data: PeerStatePayload }
  | { event: 'chat'; data: ChatPayload }
  | { event: 'ping'; data: PingPayload }
  | { event: 'offer'; data: OfferEnvelope }
  // The SFU answers the publisher's screen-pub offer — that arrives here as
  // 'answer'. (Audio uses the inverse direction; SFU offers, peer answers.)
  | { event: 'answer'; data: AnswerEnvelope }
  | { event: 'candidate'; data: CandidateEnvelope }
  | { event: 'screen-share-started'; data: ScreenShareStartedPayload }
  | { event: 'screen-share-available'; data: ScreenShareAvailablePayload }
  | { event: 'screen-share-ended'; data: ScreenShareEndedPayload }
  | { event: 'screen-share-error'; data: ScreenShareErrorPayload }
  | { event: 'screen-share-encode-pause'; data: ScreenShareEncodeLayersPayload }
  | { event: 'screen-share-encode-resume'; data: ScreenShareEncodeLayersPayload }
  | { event: 'screen-share-mode-changed'; data: ScreenShareModeChangedPayload };

// ClientMessage union (used in send helpers)

export type ClientMessage =
  | { event: 'hello'; data: HelloPayload }
  /**
   * Client-initiated offer. Used only for screen-share ICE restart on resume
   * (pc='screen-pub'). Audio uses the SFU-as-offerer path; clients never send
   * 'offer' for audio.
   */
  | { event: 'offer'; data: OfferEnvelope }
  | { event: 'answer'; data: AnswerEnvelope }
  | { event: 'candidate'; data: CandidateEnvelope }
  | { event: 'set-displayname'; data: SetDisplayNamePayload }
  | { event: 'set-state'; data: SetStatePayload }
  | { event: 'chat-send'; data: ChatSendPayload }
  | { event: 'ping'; data: { to: string } }
  /** Asks the server for a fresh offer after a publisher-side track change. */
  | { event: 'renegotiate'; data?: undefined }
  | { event: 'screen-share-start'; data: ScreenShareStartPayload }
  | { event: 'screen-share-stop'; data: Record<string, never> }
  | { event: 'screen-share-resume'; data: ScreenShareResumePayload }
  | { event: 'screen-share-subscribe'; data: ScreenShareSubscribePayload }
  | { event: 'screen-share-unsubscribe'; data: ScreenShareUnsubscribePayload }
  | { event: 'screen-share-mode-change'; data: ScreenShareModeChangePayload };

// Runtime guard — parses raw WS text into a typed ServerMessage

/**
 * Parses a raw WebSocket message string into a typed ServerMessage.
 *
 * Returns null (never throws) on:
 *   - malformed JSON
 *   - missing/wrong-typed envelope fields
 *   - unknown event name (protocol may grow — unknown events are ignored)
 *   - missing required payload fields
 *
 * A console.warn is emitted on every failure path to aid debugging.
 */
function isPCKind(v: unknown): v is PCKind {
  return v === 'audio' || v === 'screen-pub' || v === 'screen-sub';
}

function isScreenVideoCodec(v: unknown): v is ScreenVideoCodec {
  return v === 'av1' || v === 'vp9';
}

function isScreenShareMode(v: unknown): v is ScreenShareMode {
  return v === 'sharp' || v === 'motion';
}

function isValidAttachment(v: unknown): v is Attachment {
  if (typeof v !== 'object' || v === null) return false;
  const a = v as Record<string, unknown>;
  return (
    typeof a.uploadId === 'string' &&
    (a.kind === 'image' || a.kind === 'file') &&
    typeof a.name === 'string' &&
    typeof a.mime === 'string' &&
    typeof a.size === 'number'
  );
}

export function parseServerMessage(raw: string): ServerMessage | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    console.warn('[protocol] failed to JSON.parse message:', raw);
    return null;
  }

  if (
    typeof parsed !== 'object' ||
    parsed === null ||
    !('event' in parsed) ||
    typeof (parsed as Record<string, unknown>).event !== 'string'
  ) {
    console.warn("[protocol] message missing string 'event':", parsed);
    return null;
  }

  const envelope = parsed as { event: string; data: unknown };
  const { event, data } = envelope;

  switch (event) {
    case 'welcome': {
      if (
        typeof data !== 'object' ||
        data === null ||
        typeof (data as Record<string, unknown>).id !== 'string' ||
        !Array.isArray((data as Record<string, unknown>).peers)
      ) {
        console.warn("[protocol] malformed 'welcome' payload:", data);
        return null;
      }
      return { event, data: data as WelcomePayload };
    }

    case 'peer-joined':
    case 'peer-info': {
      if (
        typeof data !== 'object' ||
        data === null ||
        typeof (data as Record<string, unknown>).id !== 'string'
      ) {
        console.warn('[protocol] malformed peer payload:', event, data);
        return null;
      }
      return { event, data: data as PeerInfo };
    }

    case 'peer-state': {
      if (
        typeof data !== 'object' ||
        data === null ||
        typeof (data as Record<string, unknown>).id !== 'string' ||
        typeof (data as Record<string, unknown>).selfMuted !== 'boolean' ||
        typeof (data as Record<string, unknown>).deafened !== 'boolean'
      ) {
        console.warn("[protocol] malformed 'peer-state' payload:", data);
        return null;
      }
      return { event, data: data as PeerStatePayload };
    }

    case 'peer-left': {
      if (
        typeof data !== 'object' ||
        data === null ||
        typeof (data as Record<string, unknown>).id !== 'string'
      ) {
        console.warn("[protocol] malformed 'peer-left' payload:", data);
        return null;
      }
      return { event, data: data as PeerLeftPayload };
    }

    case 'offer': {
      const d = data as Record<string, unknown>;
      if (
        typeof data !== 'object' ||
        data === null ||
        typeof d.type !== 'string' ||
        typeof d.sdp !== 'string' ||
        !isPCKind(d.pc)
      ) {
        console.warn("[protocol] malformed 'offer' payload:", data);
        return null;
      }
      return { event, data: data as OfferEnvelope };
    }

    case 'answer': {
      const d = data as Record<string, unknown>;
      if (
        typeof data !== 'object' ||
        data === null ||
        typeof d.type !== 'string' ||
        typeof d.sdp !== 'string' ||
        !isPCKind(d.pc)
      ) {
        console.warn("[protocol] malformed 'answer' payload:", data);
        return null;
      }
      return { event, data: data as AnswerEnvelope };
    }

    case 'candidate': {
      const d = data as Record<string, unknown>;
      if (
        typeof data !== 'object' ||
        data === null ||
        typeof d.candidate !== 'string' ||
        !isPCKind(d.pc)
      ) {
        console.warn("[protocol] malformed 'candidate' payload:", data);
        return null;
      }
      return { event, data: data as CandidateEnvelope };
    }

    case 'screen-share-started': {
      const d = data as Record<string, unknown>;
      if (typeof data !== 'object' || data === null || typeof d.sessionToken !== 'string') {
        console.warn("[protocol] malformed 'screen-share-started' payload:", data);
        return null;
      }
      return { event, data: data as ScreenShareStartedPayload };
    }

    case 'screen-share-available': {
      const d = data as Record<string, unknown>;
      if (
        typeof data !== 'object' ||
        data === null ||
        typeof d.publisherId !== 'string' ||
        typeof d.hasSystemAudio !== 'boolean' ||
        ('videoCodec' in d && !isScreenVideoCodec(d.videoCodec)) ||
        ('mode' in d && !isScreenShareMode(d.mode))
      ) {
        console.warn("[protocol] malformed 'screen-share-available' payload:", data);
        return null;
      }
      return { event, data: data as ScreenShareAvailablePayload };
    }

    case 'screen-share-mode-changed': {
      const d = data as Record<string, unknown>;
      if (
        typeof data !== 'object' ||
        data === null ||
        typeof d.publisherId !== 'string' ||
        !isScreenShareMode(d.mode)
      ) {
        console.warn("[protocol] malformed 'screen-share-mode-changed' payload:", data);
        return null;
      }
      return { event, data: data as ScreenShareModeChangedPayload };
    }

    case 'screen-share-ended': {
      const d = data as Record<string, unknown>;
      if (typeof data !== 'object' || data === null || typeof d.publisherId !== 'string') {
        console.warn("[protocol] malformed 'screen-share-ended' payload:", data);
        return null;
      }
      return { event, data: data as ScreenShareEndedPayload };
    }

    case 'screen-share-error': {
      const d = data as Record<string, unknown>;
      if (typeof data !== 'object' || data === null || typeof d.reason !== 'string') {
        console.warn("[protocol] malformed 'screen-share-error' payload:", data);
        return null;
      }
      return { event, data: data as ScreenShareErrorPayload };
    }

    case 'screen-share-encode-pause':
    case 'screen-share-encode-resume': {
      const d = data as Record<string, unknown>;
      if (typeof data !== 'object' || data === null || !Array.isArray(d.layers)) {
        console.warn('[protocol] malformed payload:', event, data);
        return null;
      }
      return { event, data: data as ScreenShareEncodeLayersPayload };
    }

    case 'chat': {
      const d = data as Record<string, unknown>;
      if (
        typeof data !== 'object' ||
        data === null ||
        typeof d.id !== 'string' ||
        typeof d.from !== 'string' ||
        typeof d.text !== 'string' ||
        typeof d.ts !== 'number'
      ) {
        console.warn("[protocol] malformed 'chat' payload:", data);
        return null;
      }
      if (d.attachments !== undefined) {
        if (!Array.isArray(d.attachments) || !d.attachments.every(isValidAttachment)) {
          console.warn("[protocol] malformed 'chat' attachments:", data);
          return null;
        }
      }
      return { event, data: data as ChatPayload };
    }

    case 'ping': {
      const d = data as Record<string, unknown>;
      if (
        typeof data !== 'object' ||
        data === null ||
        typeof d.from !== 'string' ||
        typeof d.fromName !== 'string'
      ) {
        console.warn("[protocol] malformed 'ping' payload:", data);
        return null;
      }
      return { event, data: data as PingPayload };
    }

    default:
      console.warn('[protocol] unknown server event:', event);
      return null;
  }
}
