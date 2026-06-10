import { useEffect, useState } from 'react';
import { isRoomSlug, ROOM_SLUGS, type RoomSlug } from '../rooms';

export type PeerSummary = { id: string; displayName: string; chatOnly: boolean };

function emptyPeers(): Record<RoomSlug, PeerSummary[]> {
  return { room1: [], room2: [], room3: [] };
}

function normalizePeer(raw: ValidPeer): PeerSummary {
  return {
    id: raw.id,
    displayName:
      typeof raw.displayName === 'string' && raw.displayName.trim()
        ? raw.displayName.trim()
        : `peer-${raw.id}`,
    chatOnly: raw.chatOnly === true,
  };
}

type RawPeer = { id?: unknown; displayName?: unknown; chatOnly?: unknown };
type ValidPeer = { id: string; displayName?: unknown; chatOnly?: unknown };

function asValidPeer(p: unknown): ValidPeer | null {
  if (typeof p !== 'object' || p === null) return null;
  const { id } = p as RawPeer;
  if (typeof id !== 'string') return null;
  return p as ValidPeer;
}

function parseSnapshot(data: unknown): Record<RoomSlug, PeerSummary[]> | null {
  if (typeof data !== 'object' || data === null) return null;
  const { rooms } = data as { rooms?: unknown };
  if (typeof rooms !== 'object' || rooms === null) return null;
  const next = emptyPeers();
  for (const slug of ROOM_SLUGS) {
    const room = (rooms as Record<string, unknown>)[slug];
    if (typeof room !== 'object' || room === null) continue;
    const peers = (room as { peers?: unknown }).peers;
    if (!Array.isArray(peers)) continue;
    next[slug] = peers.flatMap((p) => {
      const valid = asValidPeer(p);
      return valid ? [normalizePeer(valid)] : [];
    });
  }
  return next;
}

function parsePeerJoined(data: unknown): { room: RoomSlug; peer: PeerSummary } | null {
  if (typeof data !== 'object' || data === null) return null;
  const { room, peer } = data as { room?: unknown; peer?: unknown };
  if (!isRoomSlug(room)) return null;
  const valid = asValidPeer(peer);
  if (!valid) return null;
  return { room, peer: normalizePeer(valid) };
}

function parsePeerLeft(data: unknown): { room: RoomSlug; id: string } | null {
  if (typeof data !== 'object' || data === null) return null;
  const { room, id } = data as { room?: unknown; id?: unknown };
  if (!isRoomSlug(room)) return null;
  if (typeof id !== 'string') return null;
  return { room, id };
}

// Browsers signal permanent failure (non-2xx) by closing EventSource with
// readyState CLOSED. Probe /api/config to distinguish auth failure from other
// permanent errors; 401 → redirect to login (mirrors loadAppConfig behavior).
async function checkAuthAfterHandshakeFailure(): Promise<void> {
  try {
    const res = await fetch('/api/config', { credentials: 'same-origin', cache: 'no-store' });
    if (res.status === 401) {
      const next = encodeURIComponent(window.location.pathname + window.location.search);
      window.location.replace('/login.html?next=' + next);
    }
  } catch {
    // Network failure during probe — ignore, reconnect will retry.
  }
}

function openEventSource(
  setPeers: React.Dispatch<React.SetStateAction<Record<RoomSlug, PeerSummary[]>>>,
): EventSource {
  const es = new EventSource('/api/presence');
  let hadOpen = false;

  es.onopen = () => {
    hadOpen = true;
  };

  es.addEventListener('presence-snapshot', (e: MessageEvent) => {
    const snapshot = parseSnapshot(JSON.parse(e.data));
    if (snapshot) setPeers(snapshot);
  });

  es.addEventListener('presence-peer-joined', (e: MessageEvent) => {
    const parsed = parsePeerJoined(JSON.parse(e.data));
    if (parsed) {
      const { room, peer } = parsed;
      setPeers((prev) => ({
        ...prev,
        [room]: prev[room].filter((p) => p.id !== peer.id).concat(peer),
      }));
    }
  });

  es.addEventListener('presence-peer-left', (e: MessageEvent) => {
    const parsed = parsePeerLeft(JSON.parse(e.data));
    if (parsed) {
      const { room, id } = parsed;
      setPeers((prev) => ({
        ...prev,
        [room]: prev[room].filter((p) => p.id !== id),
      }));
    }
  });

  es.addEventListener('presence-peer-updated', (e: MessageEvent) => {
    // peer-updated with unknown id is treated as insert (idempotent with joined).
    const parsed = parsePeerJoined(JSON.parse(e.data));
    if (parsed) {
      const { room, peer } = parsed;
      setPeers((prev) => ({
        ...prev,
        [room]: prev[room].filter((p) => p.id !== peer.id).concat(peer),
      }));
    }
  });

  es.onerror = () => {
    // readyState CLOSED means the browser gave up (permanent failure, e.g. 4xx).
    // readyState CONNECTING means a transient error — browser will auto-reconnect.
    if (es.readyState === EventSource.CLOSED && !hadOpen) {
      void checkAuthAfterHandshakeFailure();
    }
  };

  return es;
}

export function useRoomPeers(): Record<RoomSlug, PeerSummary[]> {
  const [peers, setPeers] = useState<Record<RoomSlug, PeerSummary[]>>(emptyPeers);

  useEffect(() => {
    let es: EventSource | null = null;

    const open = () => {
      if (document.visibilityState === 'visible') {
        es = openEventSource(setPeers);
      }
    };

    const onVisibilityChange = () => {
      if (document.visibilityState === 'hidden') {
        es?.close();
        es = null;
      } else {
        if (!es || es.readyState === EventSource.CLOSED) {
          es = openEventSource(setPeers);
        }
      }
    };

    open();
    document.addEventListener('visibilitychange', onVisibilityChange);

    return () => {
      document.removeEventListener('visibilitychange', onVisibilityChange);
      es?.close();
    };
  }, []);

  return peers;
}
