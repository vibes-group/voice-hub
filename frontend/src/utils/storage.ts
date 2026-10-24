// Central registry of every voice-hub.* localStorage key the frontend uses.
// All reads and writes go through the typed helpers below — no inline
// localStorage calls in components, hooks, or the store.

import type { EngineKind } from '../types';
import { ENGINE_IDS } from '../audio/engine';
import { isRoomSlug, DEFAULT_ROOM_SLUG, type RoomSlug } from '../rooms';
import type { Attachment } from '../sfu/protocol';
import { deleteBlob, getChatRecord, putChatRecord } from './blobCache';
import { isMobileBrowser } from './platform';

export const KEYS = {
  // Audio / engine
  outputVolume: 'voice-hub.output-volume',
  sendVolume: 'voice-hub.send-volume',
  engine: 'voice-hub.engine',
  // Selected microphone deviceId, or empty string for system default.
  micDeviceId: 'voice-hub.mic-device-id',
  // Selected speaker deviceId, absent for system default.
  speakerDeviceId: 'voice-hub.speaker-device-id',
  // Persistent mute/deafen state — Discord-style, survives reloads.
  selfMuted: 'voice-hub.self-muted',
  deafened: 'voice-hub.deafened',
  preDeafenSelfMuted: 'voice-hub.pre-deafen-self-muted',
  // Identity
  displayName: 'voice-hub.display-name',
  // Stable per-install identifier (UUID) generated once on first launch.
  // Sent to the SFU in `hello` so peers can key per-peer UI prefs by
  // something that survives reconnects (peer IDs are ephemeral per WS).
  clientId: 'voice-hub.client-id',
  // Hotkey binding (JSON-serialised InputBinding | null)
  shortcut: 'voice-hub.shortcut',
  // One-shot flag set before reload so the app can auto-rejoin on startup.
  rejoinOnLoad: 'voice-hub.rejoin-on-load',
  // Ping feature
  pingSoundEnabled: 'voice-hub.ping-sound-enabled',
  muteIncomingPings: 'voice-hub.mute-incoming-pings',
  pingWindowFlashEnabled: 'voice-hub.ping-window-flash-enabled',
  // Selected room slug
  roomSlug: 'voice-hub.room-slug',
  // Screen share capture preferences
  screenResolution: 'voice-hub.screen-resolution',
  screenFps: 'voice-hub.screen-fps',
  screenCodec: 'voice-hub.screen-codec',
  screenMode: 'voice-hub.screen-mode',
  screenShareMode: 'voice-hub.screen-share-mode',
} as const;

// Per-room chat history, persisted in the IndexedDB blob DB (store 'chat',
// keyed by roomId) so it shares the larger IDB quota with attachment bytes
// rather than the ~5 MB localStorage budget. Two retention rules apply together
// (whichever is stricter wins):
//   1. Drop messages older than CHAT_TTL_MS (rolling window).
//   2. Cap to CHAT_HISTORY_CAP entries (FIFO eviction on write).
// Pruning runs on both load and save so stale entries don't linger across
// sessions where the user just reads without writing.
export const CHAT_HISTORY_CAP = 1000;
const CHAT_TTL_MS = 7 * 24 * 60 * 60 * 1000;

export type PersistedChatMessage = {
  id: string;
  from: string;
  text: string;
  ts: number;
  clientMsgId?: string;
  pending?: boolean;
  // Snapshot of sender display name at send/receive time. Survives peer leave
  // and reconnects, where `from` (ephemeral peer id) can no longer be resolved
  // via the participants map.
  senderName?: string;
  // Stable per-install id of the sender. Lets MessageRow identify own messages
  // after our own peerId rotates (leave + rejoin).
  senderClientId?: string;
  // Image/file attachments. Bytes live in the IndexedDB blob cache (and the
  // server's transient buffer until it expires), keyed by attachment.uploadId.
  attachments?: Attachment[];
  // uploadIds whose bytes have been dropped locally (manual delete or the 1GB
  // cache cap). The message stays in history; these attachments render as
  // "deleted" and are never re-fetched. Persisted.
  deletedUploadIds?: string[];
  // --- transient, never persisted (see stripTransient) ---
  // Aggregate upload progress 0..1 while a pending message's attachments upload.
  uploadProgress?: number;
  // True when an attachment upload failed; surfaces a retry affordance.
  uploadFailed?: boolean;
};

// Splits history into the messages to keep and the ones aged out (>7d) or
// FIFO-evicted (>CHAT_HISTORY_CAP). The dropped set drives blob cleanup: a message leaving
// history takes its attachments' bytes with it, so blobCache needs no age limit
// of its own.
function pruneChatHistory(messages: PersistedChatMessage[]): {
  kept: PersistedChatMessage[];
  dropped: PersistedChatMessage[];
} {
  const cutoff = Date.now() - CHAT_TTL_MS;
  const kept: PersistedChatMessage[] = [];
  const dropped: PersistedChatMessage[] = [];
  for (const m of messages) (m.ts >= cutoff ? kept : dropped).push(m);
  if (kept.length > CHAT_HISTORY_CAP) {
    dropped.push(...kept.splice(0, kept.length - CHAT_HISTORY_CAP));
  }
  return { kept, dropped };
}

// Frees the IndexedDB bytes of removed messages' attachments (fire-and-forget).
export function deleteDroppedBlobs(dropped: PersistedChatMessage[]): void {
  for (const m of dropped) {
    for (const a of m.attachments ?? []) void deleteBlob(a.uploadId);
  }
}

// Strips in-flight-only fields (upload progress/failure) so a reload never
// resurrects a half-uploaded or failed send as if it were real history.
function stripTransient(m: PersistedChatMessage): PersistedChatMessage {
  const copy = { ...m };
  delete copy.uploadProgress;
  delete copy.uploadFailed;
  return copy;
}

export async function loadChatHistory(roomId: string): Promise<PersistedChatMessage[]> {
  const stored = await getChatRecord<PersistedChatMessage[]>(roomId);
  if (!Array.isArray(stored)) return [];
  const { kept, dropped } = pruneChatHistory(stored);
  if (dropped.length > 0) {
    deleteDroppedBlobs(dropped);
    // Persist the pruned view so a read-only session still expires stale entries.
    void putChatRecord(roomId, kept);
  }
  return kept;
}

export async function saveChatHistory(
  roomId: string,
  messages: PersistedChatMessage[],
): Promise<void> {
  const { kept, dropped } = pruneChatHistory(messages);
  deleteDroppedBlobs(dropped);
  await putChatRecord(roomId, kept.map(stripTransient));
}

// Prefix for per-peer volume entries: voice-hub.peer-volume.<clientId> = number.
// Keyed by the peer's stable clientId, not the ephemeral SFU peer ID, so the
// setting survives both their reconnects and ours.
const PEER_VOLUME_PREFIX = 'voice-hub.peer-volume.';

// Per-peer screen-share system-audio volume, keyed by the publisher's stable
// clientId. Independent from the voice-mic volume (PEER_VOLUME_PREFIX) so
// muting someone's mic doesn't silence the screen audio they're sharing.
const SCREEN_AUDIO_VOLUME_PREFIX = 'voice-hub.screen-audio-volume.';

// Per-peer custom label keyed by stable clientId. Local-only annotation —
// rendered as `[label]` after the peer's display name. Not synced anywhere.
const PEER_LABEL_PREFIX = 'voice-hub.peer-label.';
export const PEER_LABEL_MAX = 32;

function loadNumberOrNull(key: string): number | null {
  const raw = localStorage.getItem(key);
  if (raw === null || raw === '') return null;
  const parsed = Number(raw);
  return Number.isFinite(parsed) ? parsed : null;
}

export function loadNumber(key: string, fallback: number): number {
  return loadNumberOrNull(key) ?? fallback;
}

export function saveNumber(key: string, v: number): void {
  localStorage.setItem(key, String(v));
}

export function loadBoolean(key: string, fallback: boolean): boolean {
  const raw = localStorage.getItem(key);
  if (raw === null) return fallback;
  return raw === 'true';
}

export function saveBoolean(key: string, v: boolean): void {
  localStorage.setItem(key, String(v));
}

export function saveDisplayName(name: string): void {
  localStorage.setItem(KEYS.displayName, name.trim());
}

// Persistent display name. Generated once on first launch via the supplied
// generator and stored alongside clientId — both act as stable identity that
// survives reconnects, server switches, and reloads. The user can rename
// themselves at any time; rename is just another saveDisplayName.
export function loadOrCreateDisplayName(generate: () => string): string {
  const existing = localStorage.getItem(KEYS.displayName);
  if (existing) return existing;
  const fresh = generate();
  saveDisplayName(fresh);
  return fresh;
}

export function makeGuestName(): string {
  return `~${Math.random().toString(36).slice(2, 7)}`;
}

// Stable client identifier. Generated once on first launch via
// crypto.randomUUID() (available in all Tauri webviews and modern browsers
// over a secure context) and persisted forever. Clearing localStorage =
// new identity, which is the same effect as a fresh install — by design.
export function loadOrCreateClientId(): string {
  const existing = localStorage.getItem(KEYS.clientId);
  if (existing) return existing;
  const fresh = crypto.randomUUID();
  localStorage.setItem(KEYS.clientId, fresh);
  return fresh;
}

// Per-peer volume keyed by the peer's stable clientId. Returns null when no
// preference has been saved so callers can fall back to their own default.
export function loadPeerVolume(clientId: string): number | null {
  return clientId ? loadNumberOrNull(PEER_VOLUME_PREFIX + clientId) : null;
}

export function savePeerVolume(clientId: string, volume: number): void {
  if (!clientId) return;
  localStorage.setItem(PEER_VOLUME_PREFIX + clientId, String(volume));
}

export function loadScreenAudioVolume(clientId: string): number | null {
  return clientId ? loadNumberOrNull(SCREEN_AUDIO_VOLUME_PREFIX + clientId) : null;
}

export function saveScreenAudioVolume(clientId: string, volume: number): void {
  if (!clientId) return;
  localStorage.setItem(SCREEN_AUDIO_VOLUME_PREFIX + clientId, String(volume));
}

export function loadPeerLabel(clientId: string): string {
  if (!clientId) return '';
  return localStorage.getItem(PEER_LABEL_PREFIX + clientId) ?? '';
}

export function savePeerLabel(clientId: string, label: string): void {
  if (!clientId) return;
  const trimmed = label.trim().slice(0, PEER_LABEL_MAX);
  if (!trimmed) {
    localStorage.removeItem(PEER_LABEL_PREFIX + clientId);
    return;
  }
  localStorage.setItem(PEER_LABEL_PREFIX + clientId, trimmed);
}

const ENGINE_VALUES: EngineKind[] = ['off', ...ENGINE_IDS];

export function loadEngine(): EngineKind {
  const raw = localStorage.getItem(KEYS.engine);
  if (ENGINE_VALUES.includes(raw as EngineKind)) return raw as EngineKind;
  // RNNoise runs a WASM frame loop on the audio thread; phone CPUs miss the
  // deadline and the mic stutters, so phones fall back to the OS suppressor.
  return isMobileBrowser() ? 'browser' : 'rnnoise';
}

export function saveEngine(e: EngineKind): void {
  localStorage.setItem(KEYS.engine, e);
}

export function loadRoomSlug(): RoomSlug {
  const raw = localStorage.getItem(KEYS.roomSlug);
  return isRoomSlug(raw) ? raw : DEFAULT_ROOM_SLUG;
}

export function saveRoomSlug(s: RoomSlug): void {
  localStorage.setItem(KEYS.roomSlug, s);
}

// Selected microphone deviceId. null = use system default.
export function loadMicDeviceId(): string | null {
  return localStorage.getItem(KEYS.micDeviceId) || null;
}

export function saveMicDeviceId(id: string | null): void {
  if (id) localStorage.setItem(KEYS.micDeviceId, id);
  else localStorage.removeItem(KEYS.micDeviceId);
}

// Selected speaker deviceId. null = use system default.
export function loadSpeakerDeviceId(): string | null {
  return localStorage.getItem(KEYS.speakerDeviceId) || null;
}

export function saveSpeakerDeviceId(id: string | null): void {
  if (id) localStorage.setItem(KEYS.speakerDeviceId, id);
  else localStorage.removeItem(KEYS.speakerDeviceId);
}

// Shortcut binding (JSON-serialised InputBinding | null); null = key absent.
export function loadShortcutRaw(): string | null {
  return localStorage.getItem(KEYS.shortcut);
}

export function saveShortcutRaw(json: string): void {
  localStorage.setItem(KEYS.shortcut, json);
}

// rejoin-on-load flag: set before a reload so the app auto-rejoins on startup.
export function setRejoinFlag(): void {
  localStorage.setItem(KEYS.rejoinOnLoad, '1');
}

export function consumeRejoinFlag(): boolean {
  if (localStorage.getItem(KEYS.rejoinOnLoad) !== '1') return false;
  localStorage.removeItem(KEYS.rejoinOnLoad);
  return true;
}

// Drops the per-room chat entries (voice-hub.chat.<roomId>) that older
// versions kept in localStorage; history now lives in IndexedDB.
export function clearLegacyStorage(): void {
  try {
    const stale: string[] = [];
    for (let i = 0; i < localStorage.length; i++) {
      const key = localStorage.key(i);
      if (key?.startsWith('voice-hub.chat.')) stale.push(key);
    }
    for (const key of stale) localStorage.removeItem(key);
  } catch {
    /* ignore */
  }
}
