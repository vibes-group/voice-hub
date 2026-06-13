// Zustand store: reactive UI state.
// Audio nodes are NOT stored here — they live in imperative refs inside useAudioEngine.

import { create } from 'zustand';
import type { EngineKind, ParticipantUI, Role } from '../types';
import type { InputBinding } from '../utils/binding';
import { loadBinding } from '../utils/binding';
import {
  KEYS,
  loadBoolean,
  loadEngine,
  loadMicDeviceId,
  loadNumber,
  loadRoomSlug,
  saveBoolean,
  saveSendVolume,
  saveOutputVolume,
  saveEngine,
  saveMicDeviceId,
  saveRoomSlug,
  loadChatHistory,
  saveChatHistory,
  deleteDroppedBlobs,
  loadPeerLabel,
  type PersistedChatMessage,
} from '../utils/storage';
import type { RoomSlug } from '../rooms';
import type { Attachment } from '../sfu/protocol';

export type ChatMessage = PersistedChatMessage;

export type JoinState = 'idle' | 'joining' | 'joined';
type StatusState = 'idle' | 'ok' | 'err';

function compareParticipants(a: ParticipantUI, b: ParticipantUI): number {
  if (a.isSelf) return -1;
  if (b.isSelf) return 1;
  // Stable by clientId (per-install) so renames don't reorder. Fallback to
  // peer id for peers without a clientId.
  return (a.clientId ?? a.id).localeCompare(b.clientId ?? b.id);
}

// Patches the pending message matching clientMsgId; returns {} (a no-op slice)
// when it's already been reconciled away.
function patchPendingMessage(
  state: AppState,
  roomId: string,
  clientMsgId: string,
  patch: Partial<ChatMessage>,
): Partial<AppState> {
  const existing = state.chatByRoom[roomId];
  if (!existing) return {};
  const idx = existing.findIndex((m) => m.clientMsgId === clientMsgId && m.pending);
  if (idx < 0) return {};
  const next = [...existing];
  next[idx] = { ...next[idx], ...patch };
  return { chatByRoom: { ...state.chatByRoom, [roomId]: next } };
}

type SortedCache = {
  source: Record<string, ParticipantUI>;
  voice: ParticipantUI[];
  chatOnly: ParticipantUI[];
};

let sortedCache: SortedCache | null = null;

function getSortedCache(participants: Record<string, ParticipantUI>): SortedCache {
  if (sortedCache && sortedCache.source === participants) return sortedCache;
  const all = Object.values(participants);
  sortedCache = {
    source: participants,
    voice: all.filter((p) => !p.chatOnly).sort(compareParticipants),
    chatOnly: all.filter((p) => Boolean(p.chatOnly)).sort(compareParticipants),
  };
  return sortedCache;
}

export const selectVoiceParticipants = (state: AppState): ParticipantUI[] =>
  getSortedCache(state.participants).voice;

export const selectChatOnlyParticipants = (state: AppState): ParticipantUI[] =>
  getSortedCache(state.participants).chatOnly;

export const selectSelfPeerId = (state: AppState): string | null => {
  for (const [id, participant] of Object.entries(state.participants)) {
    if (participant.isSelf) return id;
  }
  return null;
};

export interface AppState {
  joinState: JoinState;
  setJoinState: (s: JoinState) => void;

  // True once /api/config has resolved — gates Join so users can't click
  // before iceServers are known.
  configReady: boolean;

  // Caller's session role from /api/config. null until config resolves.
  // Drives admin-only UI (AdminKeyButton) without a separate probe endpoint.
  role: Role | null;

  // Mute/deafen are persistent (Discord-style — survive reload).
  // There is no separate outputMuted field on purpose: the previous
  // independently-persisted boolean caused an orphan-state trap (commit
  // 9a7c196). Audio code that needs to know whether the local listener is
  // muted reads `deafened` directly.
  selfMuted: boolean;
  setSelfMuted: (v: boolean) => void;
  deafened: boolean;
  setDeafened: (v: boolean) => void;
  preDeafenSelfMuted: boolean;
  // Atomic enter: snapshot current selfMuted, force selfMuted+deafened on
  // in a single set() so subscribers never see partial state.
  enterDeafen: () => void;

  sendVolume: number;
  setSendVolume: (v: number) => void;
  outputVolume: number;
  setOutputVolume: (v: number) => void;

  engine: EngineKind;
  setEngine: (e: EngineKind) => void;

  // Selected microphone deviceId. null = system default (no deviceId constraint).
  micDeviceId: string | null;
  setMicDeviceId: (id: string | null) => void;

  roomSlug: RoomSlug;
  setRoomSlug: (s: RoomSlug) => void;

  shortcut: InputBinding | null;
  setShortcut: (s: InputBinding | null) => void;
  capturingShortcut: boolean;
  setCapturingShortcut: (v: boolean) => void;

  statusText: string;
  statusState: StatusState;
  setStatus: (text: string, isError?: boolean, joined?: boolean) => void;

  participants: Record<string, ParticipantUI>;
  upsertParticipant: (p: Partial<ParticipantUI> & { id: string }) => ParticipantUI;
  removeParticipant: (id: string) => void;
  clearParticipants: () => void;
  updateParticipant: (id: string, patch: Partial<ParticipantUI>) => void;

  // Ping feature
  pingSoundEnabled: boolean;
  setPingSoundEnabled: (v: boolean) => void;
  muteIncomingPings: boolean;
  setMuteIncomingPings: (v: boolean) => void;
  pingWindowFlashEnabled: boolean;
  setPingWindowFlashEnabled: (v: boolean) => void;
  incomingPing: { fromName: string; at: number } | null;
  setIncomingPing: (p: { fromName: string; at: number }) => void;
  clearIncomingPing: () => void;
  lastPingSentByTarget: Map<string, number>;
  markPingSent: (targetId: string) => void;

  // Chat — per-room message history. roomId matches the SFU room / host.
  chatByRoom: Record<string, ChatMessage[]>;
  // Load persisted history for a room on join.
  loadChatRoom: (roomId: string) => void;
  // Append an optimistic (pending) outgoing message before the server echoes it.
  chatSendOptimistic: (roomId: string, msg: ChatMessage) => void;
  // Reconcile server echo: replace pending entry matching clientMsgId, or append.
  chatReceive: (roomId: string, msg: ChatMessage) => void;
  // Update aggregate attachment-upload progress (0..1) on a pending message.
  chatUpdateUploadProgress: (roomId: string, clientMsgId: string, progress: number) => void;
  // Flag a pending message whose attachment upload failed (shows retry).
  chatMarkUploadFailed: (roomId: string, clientMsgId: string) => void;
  // Swap a pending message's attachments for ones keyed by their real,
  // server-assigned uploadIds once uploads complete.
  chatSetAttachments: (roomId: string, clientMsgId: string, attachments: Attachment[]) => void;
  // Mark attachments (by uploadId, across all rooms) as deleted — their bytes
  // are gone locally, so they render as "deleted" and never re-fetch.
  markAttachmentsDeleted: (uploadIds: string[]) => void;
  // Retract a message by server id — our own delete, or a peer's "chat-deleted" echo.
  chatDelete: (roomId: string, id: string) => void;
  // Persist current history to localStorage (debounce externally).
  persistChat: (roomId: string) => void;
  // True while the chat image lightbox is open, so global arrow-key handlers
  // (e.g. screen-share switching) can yield to in-lightbox navigation.
  chatLightboxOpen: boolean;
  setChatLightboxOpen: (open: boolean) => void;
}

export const useStore = create<AppState>((set, get) => ({
  joinState: 'idle',
  setJoinState: (s) => set({ joinState: s }),
  configReady: false,
  role: null,

  selfMuted: loadBoolean(KEYS.selfMuted, false),
  setSelfMuted: (v) => {
    saveBoolean(KEYS.selfMuted, v);
    set({ selfMuted: v });
  },
  deafened: loadBoolean(KEYS.deafened, false),
  setDeafened: (v) => {
    saveBoolean(KEYS.deafened, v);
    set({ deafened: v });
  },
  preDeafenSelfMuted: loadBoolean(KEYS.preDeafenSelfMuted, false),
  enterDeafen: () =>
    set((s) => {
      saveBoolean(KEYS.selfMuted, true);
      saveBoolean(KEYS.deafened, true);
      saveBoolean(KEYS.preDeafenSelfMuted, s.selfMuted);
      return {
        preDeafenSelfMuted: s.selfMuted,
        deafened: true,
        selfMuted: true,
      };
    }),

  sendVolume: loadNumber(KEYS.sendVolume, 100),
  setSendVolume: (v) => {
    saveSendVolume(v);
    set({ sendVolume: v });
  },
  outputVolume: loadNumber(KEYS.outputVolume, 100),
  setOutputVolume: (v) => {
    saveOutputVolume(v);
    set({ outputVolume: v });
  },

  engine: loadEngine(),
  setEngine: (e) => {
    saveEngine(e);
    set({ engine: e });
  },

  micDeviceId: loadMicDeviceId(),
  setMicDeviceId: (id) => {
    saveMicDeviceId(id);
    set({ micDeviceId: id });
  },

  roomSlug: loadRoomSlug(),
  setRoomSlug: (s) => {
    saveRoomSlug(s);
    set({ roomSlug: s });
  },

  shortcut: loadBinding(),
  setShortcut: (s) => set({ shortcut: s }),
  capturingShortcut: false,
  setCapturingShortcut: (v) => set({ capturingShortcut: v }),

  statusText: 'Загрузка…',
  statusState: 'idle',
  setStatus: (text, isError = false, joined) => {
    const currentJoined = joined ?? get().joinState === 'joined';
    set({
      statusText: text,
      statusState: isError ? 'err' : currentJoined ? 'ok' : 'idle',
    });
  },

  pingSoundEnabled: loadBoolean(KEYS.pingSoundEnabled, true),
  setPingSoundEnabled: (v) => {
    saveBoolean(KEYS.pingSoundEnabled, v);
    set({ pingSoundEnabled: v });
  },
  muteIncomingPings: loadBoolean(KEYS.muteIncomingPings, false),
  setMuteIncomingPings: (v) => {
    saveBoolean(KEYS.muteIncomingPings, v);
    set({ muteIncomingPings: v });
  },
  pingWindowFlashEnabled: loadBoolean(KEYS.pingWindowFlashEnabled, false),
  setPingWindowFlashEnabled: (v) => {
    saveBoolean(KEYS.pingWindowFlashEnabled, v);
    set({ pingWindowFlashEnabled: v });
  },
  incomingPing: null,
  setIncomingPing: (p) =>
    set((s) => {
      if (s.muteIncomingPings) return {};
      return { incomingPing: p };
    }),
  clearIncomingPing: () => set({ incomingPing: null }),
  lastPingSentByTarget: new Map(),
  markPingSent: (targetId) =>
    set((s) => {
      const now = Date.now();
      const next = new Map(s.lastPingSentByTarget);
      next.set(targetId, now);
      return { lastPingSentByTarget: next };
    }),

  participants: {},
  upsertParticipant: (partial) => {
    let result!: ParticipantUI;
    set((s) => {
      const existing = s.participants[partial.id];
      const merged: ParticipantUI = existing
        ? { ...existing, ...partial }
        : {
            ...partial,
            display: partial.display ?? `user-${partial.id}`,
            isSelf: Boolean(partial.isSelf),
            selfMuted: partial.selfMuted ?? false,
            speaking: partial.speaking ?? false,
            localMuted: partial.localMuted ?? false,
            localVolume: partial.localVolume ?? 100,
            hasStream: partial.hasStream ?? false,
          };
      if (
        merged.clientId &&
        !merged.isSelf &&
        merged.localLabel === undefined &&
        partial.localLabel === undefined
      ) {
        const stored = loadPeerLabel(merged.clientId);
        if (stored) merged.localLabel = stored;
      }
      const next = { ...s.participants, [partial.id]: merged };
      // Mirror server-side eviction: a peer arriving with a clientId already
      // held by another entry replaces that entry (e.g. voice → lurker
      // transition where peer-joined Y can race peer-left X). Keeps the
      // roster consistent even if broadcast order at the source is loose.
      if (partial.clientId) {
        for (const id of Object.keys(next)) {
          if (id !== partial.id && next[id].clientId === partial.clientId) {
            delete next[id];
          }
        }
      }
      result = merged;
      return { participants: next };
    });
    return result;
  },
  removeParticipant: (id) =>
    set((s) => {
      const rest = { ...s.participants };
      delete rest[id];
      return { participants: rest };
    }),
  clearParticipants: () => set({ participants: {} }),

  updateParticipant: (id, patch) =>
    set((s) => {
      const existing = s.participants[id];
      if (!existing) return {};
      return { participants: { ...s.participants, [id]: { ...existing, ...patch } } };
    }),

  chatByRoom: {},
  loadChatRoom: (roomId) => {
    void loadChatHistory(roomId).then((loaded) => {
      set((s) => {
        const existing = s.chatByRoom[roomId] ?? [];
        if (existing.length === 0) return { chatByRoom: { ...s.chatByRoom, [roomId]: loaded } };
        // Messages may arrive during the async load — keep them. History
        // (older) goes first, live entries after, deduped by id.
        const seen = new Set(existing.map((m) => m.id));
        const merged = [...loaded.filter((m) => !seen.has(m.id)), ...existing];
        return { chatByRoom: { ...s.chatByRoom, [roomId]: merged } };
      });
    });
  },
  chatSendOptimistic: (roomId, msg) =>
    set((s) => ({
      chatByRoom: { ...s.chatByRoom, [roomId]: [...(s.chatByRoom[roomId] ?? []), msg] },
    })),
  chatReceive: (roomId, msg) =>
    set((s) => {
      const existing = s.chatByRoom[roomId] ?? [];
      const idx = msg.clientMsgId
        ? existing.findIndex((m) => m.clientMsgId === msg.clientMsgId && m.pending)
        : -1;
      const next =
        idx >= 0
          ? [...existing.slice(0, idx), msg, ...existing.slice(idx + 1)]
          : [...existing, msg];
      return { chatByRoom: { ...s.chatByRoom, [roomId]: next } };
    }),
  chatUpdateUploadProgress: (roomId, clientMsgId, progress) =>
    set((s) =>
      patchPendingMessage(s, roomId, clientMsgId, {
        uploadProgress: progress,
        uploadFailed: false,
      }),
    ),
  chatMarkUploadFailed: (roomId, clientMsgId) =>
    set((s) => patchPendingMessage(s, roomId, clientMsgId, { uploadFailed: true })),
  chatSetAttachments: (roomId, clientMsgId, attachments) =>
    set((s) => patchPendingMessage(s, roomId, clientMsgId, { attachments })),
  markAttachmentsDeleted: (uploadIds) => {
    if (uploadIds.length === 0) return;
    const gone = new Set(uploadIds);
    const changedRooms: string[] = [];
    set((s) => {
      const chatByRoom = { ...s.chatByRoom };
      for (const [roomId, msgs] of Object.entries(s.chatByRoom)) {
        let roomChanged = false;
        const next = msgs.map((m) => {
          const newly = (m.attachments ?? [])
            .map((a) => a.uploadId)
            .filter((id) => gone.has(id) && !m.deletedUploadIds?.includes(id));
          if (newly.length === 0) return m;
          roomChanged = true;
          return { ...m, deletedUploadIds: [...(m.deletedUploadIds ?? []), ...newly] };
        });
        if (roomChanged) {
          chatByRoom[roomId] = next;
          changedRooms.push(roomId);
        }
      }
      return changedRooms.length ? { chatByRoom } : {};
    });
    for (const roomId of changedRooms)
      void saveChatHistory(roomId, useStore.getState().chatByRoom[roomId]);
  },
  chatDelete: (roomId, id) => {
    let removed: ChatMessage | undefined;
    set((s) => {
      const existing = s.chatByRoom[roomId];
      if (!existing) return {};
      removed = existing.find((m) => m.id === id);
      if (!removed) return {};
      return { chatByRoom: { ...s.chatByRoom, [roomId]: existing.filter((m) => m.id !== id) } };
    });
    if (!removed) return;
    deleteDroppedBlobs([removed]);
    void saveChatHistory(roomId, useStore.getState().chatByRoom[roomId] ?? []);
  },
  persistChat: (roomId) => {
    const msgs = useStore.getState().chatByRoom[roomId];
    if (msgs) void saveChatHistory(roomId, msgs);
  },
  chatLightboxOpen: false,
  setChatLightboxOpen: (open) => set({ chatLightboxOpen: open }),
}));
