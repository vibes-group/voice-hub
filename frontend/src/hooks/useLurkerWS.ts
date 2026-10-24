import { useRef, useEffect, useCallback } from 'react';
import { createChatClient, type ChatOnlyClient } from '../sfu/client';
import { buildWsUrl } from '../config';
import { loadOrCreateClientId } from '../utils/storage';
import type {
  ChatPayload,
  ChatDeletedPayload,
  ChatSendPayload,
  PingPayload,
} from '../sfu/protocol';
import { useStore } from '../store/useStore';
import { retryPendingChats } from '../utils/chat-retry';
import { upsertRemotePeer } from './sfu-handlers';

type UseLurkerWSDeps = {
  displayName: string;
  // Same handlers as the voice WS.
  onChat: (data: ChatPayload) => void;
  onChatDeleted: (data: ChatDeletedPayload) => void;
  onPing: (data: PingPayload) => void;
  // True while the voice WS is joining/joined; the lurker WS stays closed then.
  voiceActive: boolean;
};

type UseLurkerWSReturn = {
  // No-ops (sendChatDelete → false) when not connected.
  sendChat: (payload: ChatSendPayload) => void;
  sendChatDelete: (id: string) => boolean;
  sendPing: (targetId: string) => void;
};

// Chat-only WS for when the user isn't in voice, so chat and the roster stay
// live. Never open at the same time as the voice WS.
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
  const latestRef = useRef({ displayName, onChat, onChatDeleted, onPing });
  useEffect(() => {
    latestRef.current = { displayName, onChat, onChatDeleted, onPing };
  });

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
          display: latestRef.current.displayName,
          isSelf: true,
          clientId: clientIdRef.current,
          chatOnly: true,
        });
        for (const p of peers) upsertRemotePeer(p);
        retryPendingChats(clientRef.current?.sendChat, clientIdRef.current);
      },
      onPeerJoined: (p) => upsertRemotePeer(p),
      onPeerLeft: ({ id }) => {
        useStore.getState().removeParticipant(id);
      },
      onChat: (data) => latestRef.current.onChat(data),
      onChatDeleted: (data) => latestRef.current.onChatDeleted(data),
      onPing: (data) => latestRef.current.onPing(data),
      onClose: () => {
        // Server-initiated close (not from our disconnect()). Schedule a
        // reconnect without clearing the roster — the participant list stays
        // visible during the 2-second gap so the UI doesn't flash empty.
        // The next welcome will clear and repopulate authoritatively.
        clientRef.current = null;
        scheduleReconnect();
      },
    });

    clientRef.current = client;
    void client
      .connect({
        wsUrl: buildWsUrl(roomSlugRef.current),
        displayName: latestRef.current.displayName,
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

  const sendChat = useCallback((payload: ChatSendPayload): void => {
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
