// Incoming audio arrives on fixed "slot" streams; audio-slots names the
// owner. The client must bind a slot to its owner whatever arrives first and
// unbind the previous owner when a slot changes hands.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

vi.hoisted(() => {
  const memoryStore = new Map<string, string>();
  (globalThis as { localStorage?: unknown }).localStorage = {
    getItem: (k: string) => memoryStore.get(k) ?? null,
    setItem: (k: string, v: string) => {
      memoryStore.set(k, v);
    },
    removeItem: (k: string) => {
      memoryStore.delete(k);
    },
    clear: () => memoryStore.clear(),
    key: (i: number) => Array.from(memoryStore.keys())[i] ?? null,
    get length() {
      return memoryStore.size;
    },
  };
});

vi.mock('../screenshare/codec', () => ({
  chooseScreenCodec: () => 'av1',
  canReceiveScreenCodec: () => true,
  applyScreenCodecPreferences: () => undefined,
}));

import { createSFUClient } from './client';

type WsMessage = { event: string; data: unknown };

class StubWebSocket {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSING = 2;
  static readonly CLOSED = 3;

  readyState = StubWebSocket.OPEN;
  readonly sentMessages: WsMessage[] = [];

  onopen: ((this: StubWebSocket) => void) | null = null;
  onerror: ((this: StubWebSocket, ev: unknown) => void) | null = null;
  onclose: ((this: StubWebSocket) => void) | null = null;
  onmessage: ((this: StubWebSocket, ev: { data: string }) => void) | null = null;

  constructor(_url: string) {
    queueMicrotask(() => {
      this.onopen?.call(this);
    });
  }

  send(raw: string): void {
    this.sentMessages.push(JSON.parse(raw) as WsMessage);
  }

  close(): void {
    this.readyState = StubWebSocket.CLOSED;
  }

  injectMessage(event: string, data: unknown): void {
    this.onmessage?.call(this, { data: JSON.stringify({ event, data }) });
  }
}

let lastPc: StubRTCPeerConnection | null = null;

class StubRTCPeerConnection {
  ontrack: ((ev: { track: unknown; streams: { id: string }[] }) => void) | null = null;
  onicecandidate: unknown = null;
  onconnectionstatechange: unknown = null;
  connectionState = 'new';

  addTrack(): void {}
  getTracks(): [] {
    return [];
  }
  close(): void {}
  createOffer(): Promise<RTCSessionDescriptionInit> {
    return Promise.resolve({ type: 'offer', sdp: '' });
  }
  setLocalDescription(): Promise<void> {
    return Promise.resolve();
  }
  setRemoteDescription(): Promise<void> {
    return Promise.resolve();
  }
  addIceCandidate(): Promise<void> {
    return Promise.resolve();
  }
  getTransceivers(): [] {
    return [];
  }
}

class StubMediaStream {
  getTracks(): [] {
    return [];
  }
}

let lastWs: StubWebSocket | null = null;

function TrackingWebSocket(url: string): StubWebSocket {
  const instance = new StubWebSocket(url);
  lastWs = instance;
  return instance;
}
TrackingWebSocket.CONNECTING = StubWebSocket.CONNECTING;
TrackingWebSocket.OPEN = StubWebSocket.OPEN;
TrackingWebSocket.CLOSING = StubWebSocket.CLOSING;
TrackingWebSocket.CLOSED = StubWebSocket.CLOSED;

function TrackingRTCPeerConnection(): StubRTCPeerConnection {
  lastPc = new StubRTCPeerConnection();
  return lastPc;
}

beforeEach(() => {
  lastWs = null;
  (globalThis as Record<string, unknown>).WebSocket = TrackingWebSocket;
  (globalThis as Record<string, unknown>).RTCPeerConnection = TrackingRTCPeerConnection;
  (globalThis as Record<string, unknown>).MediaStream = StubMediaStream;
});

afterEach(() => {
  delete (globalThis as Record<string, unknown>).WebSocket;
  delete (globalThis as Record<string, unknown>).RTCPeerConnection;
  delete (globalThis as Record<string, unknown>).MediaStream;
  lastWs = null;
});

async function connected() {
  const tracks: { peerId: string; stream: string }[] = [];
  const removed: string[] = [];
  const client = createSFUClient({
    onTrack: ({ stream, peerId }) => tracks.push({ peerId, stream: stream.id }),
    onTrackRemoved: ({ peerId }) => removed.push(peerId),
  });
  const p = client.connect({
    wsUrl: 'ws://localhost:8080/ws/room1',
    iceServers: [],
    localStream: new StubMediaStream() as unknown as MediaStream,
    displayName: 'me',
    clientId: 'cid',
  });
  await Promise.resolve();
  await Promise.resolve();
  lastWs!.injectMessage('welcome', { id: 'me', peers: [] });
  await p;
  const track = (slot: string) =>
    lastPc!.ontrack!({ track: { kind: 'audio' }, streams: [{ id: slot }] });
  const assign = (slots: { slot: string; peerId: string | null }[]) =>
    lastWs!.injectMessage('audio-slots', { slots });
  return { client, tracks, removed, track, assign };
}

describe('audio slots', () => {
  it('binds a slot once both its stream and owner are known, in either order', async () => {
    const c = await connected();
    c.track('slot-0');
    expect(c.tracks).toEqual([]);
    c.assign([{ slot: 'slot-0', peerId: 'alice' }]);
    c.assign([{ slot: 'slot-1', peerId: 'bob' }]);
    expect(c.tracks).toEqual([{ peerId: 'alice', stream: 'slot-0' }]);
    c.track('slot-1');
    expect(c.tracks).toEqual([
      { peerId: 'alice', stream: 'slot-0' },
      { peerId: 'bob', stream: 'slot-1' },
    ]);
    c.client.disconnect();
  });

  it('unbinds the previous owner when a slot is freed or changes hands', async () => {
    const c = await connected();
    c.track('slot-0');
    c.assign([{ slot: 'slot-0', peerId: 'alice' }]);
    c.assign([{ slot: 'slot-0', peerId: null }]);
    expect(c.removed).toEqual(['alice']);
    c.assign([
      { slot: 'slot-0', peerId: 'carol' },
      { slot: 'slot-0', peerId: 'dave' },
    ]);
    expect(c.removed).toEqual(['alice', 'carol']);
    expect(c.tracks.map((t) => t.peerId)).toEqual(['alice', 'carol', 'dave']);
    // A repeated announcement changes nothing.
    c.assign([{ slot: 'slot-0', peerId: 'dave' }]);
    expect(c.tracks).toHaveLength(3);
    c.client.disconnect();
  });
});
