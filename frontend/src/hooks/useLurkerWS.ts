import { useRef, useEffect, useCallback } from 'react';
import { createChatClient, type ChatOnlyClient } from '../sfu/client';
import { buildWsUrl } from '../config';
import { loadOrCreateClientId, loadPeerVolume } from '../utils/storage';
import type { ChatPayload, ChatDeletedPayload, PingPayload } from '../sfu/protocol';
import { useStore } from '../store/useStore';
import { retryPendingChats } from '../utils/chat-retry';

export type UseLurkerWSDeps = {
  /**
   * Display name to send in the lurker hello. Should stay in sync with the
   * value the user is editing (same source as the voice hello).
   */
  displayName: string;
  /** Fired on every received chat message — same handler as voice WS. */
  onChat: (data: ChatPayload) => void;
  /** Fired on every received chat retraction — same handler as voice WS. */
  onChatDeleted: (data: ChatDeletedPayload) => void;
  /** Fired on every received ping — same handler as voice WS. */
  onPing: (data: PingPayload) => void;
  /**
   * When true the lurker connection must NOT run — the voice WS is active.
   * Caller derives this from joinState === 'joined'.
   */
  voiceActive: boolean;
};

export type UseLurkerWSReturn = {
  /** Send a chat message via the lurker WS. No-op when not connected. */
  sendChat: (payload: import('../sfu/protocol').ChatSendPayload) => void;
  /** Retract a message by id via the lurker WS. Returns false when not connected. */
  sendChatDelete: (id: string) => boolean;
  /** Send a ping via the lurker WS. No-op when not connected. */
  sendPing: (targetId: string) => void;
};

/**
 * Manages the lurker (chat-only) WS connection lifecycle.
 *
 * Open when voiceActive=false, closed when voiceActive=true.
 * There is never a concurrent voice + lurker connection — caller ensures
 * this by toggling voiceActive before join and after leave.
 */
export function useLurkerWS({
  displayName,
  onChat,
  onChatDeleted,
  onPing,
  voiceActive,
}: UseLurkerWSDeps): UseLurkerWSReturn {
  const roomSlug = useStore((s) => s.roomSlug);
  const roomSlugRef = useRef(roomSlug);
  useEffect(() => {
    roomSlugRef.current = roomSlug;
  }, [roomSlug]);

  const clientRef = useRef<ChatOnlyClient | null>(null);
  const clientIdRef = useRef(loadOrCreateClientId());
  const reconnectTimerRef = useRef<number | null>(null);
  // False while voiceActive=true; prevents reconnect from racing a voice-WS handshake.
  const wantOpenRef = useRef(false);
  const onChatRef = useRef(onChat);
  useEffect(() => {
    onChatRef.current = onChat;
  }, [onChat]);

  const onChatDeletedRef = useRef(onChatDeleted);
  useEffect(() => {
    onChatDeletedRef.current = onChatDeleted;
  }, [onChatDeleted]);

  const onPingRef = useRef(onPing);
  useEffect(() => {
    onPingRef.current = onPing;
  }, [onPing]);

  const displayNameRef = useRef(displayName);
  useEffect(() => {
    displayNameRef.current = displayName;
  }, [displayName]);

  // Stable ref so scheduleReconnect can call open() without being listed as a
  // dep of open's useCallback (which would create a circular dep chain).
  const openRef = useRef<() => void>(() => undefined);

  const scheduleReconnect = useCallback((): void => {
    if (!wantOpenRef.current) return;
    if (reconnectTimerRef.current !== null) return;
    reconnectTimerRef.current = window.setTimeout(() => {
      reconnectTimerRef.current = null;
      if (wantOpenRef.current) openRef.current();
    }, 2000);
  }, []);

  const open = useCallback((): void => {
    if (clientRef.current) return;

    const client = createChatClient({
      onWelcome: ({ id, peers }) => {
        // Welcome is authoritative — drop any stale roster entries from the
        // previous connection (incl. our own with the old peer id).
        useStore.getState().clearParticipants();
        useStore.getState().upsertParticipant({
          id,
          display: displayNameRef.current,
          isSelf: true,
          clientId: clientIdRef.current,
          chatOnly: true,
        });
        for (const p of peers) {
          const stored = p.clientId ? loadPeerVolume(p.clientId) : null;
          useStore.getState().upsertParticipant({
            id: p.id,
            display: p.displayName ?? `peer-${p.id}`,
            clientId: p.clientId,
            remoteMuted: p.selfMuted ?? false,
            remoteDeafened: p.deafened ?? false,
            chatOnly: p.chatOnly ?? false,
            ...(stored !== null ? { localVolume: stored } : {}),
          });
        }
        // Re-send our own pending chat messages whose echo we never received
        // (lost during a prior WS rotation). Cap at 5 min so we don't keep
        // retrying ancient failures. Server idempotency: clientMsgId match
        // on reconcile, or duplicate id from another retry → server treats
        // each as a new message (worst case duplicate). Keep window tight.
        retryPendingChats(clientRef.current?.sendChat, clientIdRef.current);
      },
      onPeerJoined: (p) => {
        const stored = p.clientId ? loadPeerVolume(p.clientId) : null;
        useStore.getState().upsertParticipant({
          id: p.id,
          display: p.displayName ?? `peer-${p.id}`,
          clientId: p.clientId,
          remoteMuted: p.selfMuted ?? false,
          remoteDeafened: p.deafened ?? false,
          chatOnly: p.chatOnly ?? false,
          ...(stored !== null ? { localVolume: stored } : {}),
        });
      },
      onPeerLeft: ({ id }) => {
        useStore.getState().removeParticipant(id);
      },
      onChat: (data) => {
        onChatRef.current(data);
      },
      onChatDeleted: (data) => {
        onChatDeletedRef.current(data);
      },
      onPing: (data) => {
        onPingRef.current(data);
      },
      onClose: () => {
        // Server-initiated close (not from our disconnect()). Schedule a
        // reconnect without clearing the roster — the participant list stays
        // visible during the 2-second gap so the UI doesn't flash empty.
        // The next welcome will clear and repopulate authoritatively.
        clientRef.current = null;
        scheduleReconnect();
      },
      onError: () => {
        // onClose fires after onerror; cleanup happens there.
      },
    });

    clientRef.current = client;
    void client
      .connect({
        wsUrl: buildWsUrl(roomSlugRef.current),
        displayName: displayNameRef.current,
        clientId: clientIdRef.current,
      })
      .catch(() => {
        // connect() rejected before welcome arrived. onClose may also fire;
        // scheduleReconnect is idempotent (guards on reconnectTimerRef).
        clientRef.current = null;
        scheduleReconnect();
      });
  }, [scheduleReconnect]);

  useEffect(() => {
    openRef.current = open;
  }, [open]);

  const close = useCallback((): void => {
    if (reconnectTimerRef.current !== null) {
      clearTimeout(reconnectTimerRef.current);
      reconnectTimerRef.current = null;
    }
    clientRef.current?.disconnect();
    clientRef.current = null;
  }, []);

  useEffect(() => {
    if (voiceActive) {
      wantOpenRef.current = false;
      close();
    } else {
      wantOpenRef.current = true;
      open();
    }
    return () => {
      wantOpenRef.current = false;
      close();
    };
  }, [voiceActive, roomSlug, open, close]);

  const sendChat = useCallback((payload: import('../sfu/protocol').ChatSendPayload): void => {
    clientRef.current?.sendChat(payload);
  }, []);

  const sendChatDelete = useCallback(
    (id: string): boolean => clientRef.current?.sendChatDelete(id) ?? false,
    [],
  );

  const sendPing = useCallback((targetId: string): void => {
    clientRef.current?.sendPing(targetId);
  }, []);

  return { sendChat, sendChatDelete, sendPing };
}
