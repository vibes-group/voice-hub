// Characterization tests for the onopen → hello paths in client.ts.
//
// Purpose: pin the exact hello envelope sent on WebSocket open for both
// createSFUClient and createChatClient so the upcoming removal of the ws!
// non-null assertions (lines ~244 and ~980) cannot silently break the wire
// protocol.
//
// Environment: vitest/node. WebSocket and RTCPeerConnection do not exist in
// node, so minimal stubs are installed on globalThis before each test.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

// useScreenShareSettingsStore touches localStorage at module-init, which
// is undefined in vitest/node. vi.hoisted runs before any ESM import in
// this file, so the shim is in place when the store module is evaluated.
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

import { createSFUClient, createChatClient } from './client';

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

class StubRTCPeerConnection {
  ontrack: unknown = null;
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

beforeEach(() => {
  lastWs = null;
  (globalThis as Record<string, unknown>).WebSocket = TrackingWebSocket;
  (globalThis as Record<string, unknown>).RTCPeerConnection = StubRTCPeerConnection;
  (globalThis as Record<string, unknown>).MediaStream = StubMediaStream;
});

afterEach(() => {
  delete (globalThis as Record<string, unknown>).WebSocket;
  delete (globalThis as Record<string, unknown>).RTCPeerConnection;
  delete (globalThis as Record<string, unknown>).MediaStream;
  lastWs = null;
});

const WELCOME_PAYLOAD = { id: 'server-assigned-id', peers: [] };

async function connectAndWelcome(connectPromise: Promise<void>): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
  lastWs?.injectMessage('welcome', WELCOME_PAYLOAD);
  await connectPromise;
}

describe('createSFUClient.connect() onopen', () => {
  it('sends hello with displayName and clientId, no chatOnly field', async () => {
    const client = createSFUClient();
    const p = client.connect({
      wsUrl: 'ws://localhost:8080/sfu',
      iceServers: [],
      localStream: new StubMediaStream() as unknown as MediaStream,
      displayName: 'TestUser',
      clientId: 'cid-1',
    });

    await connectAndWelcome(p);

    const hello = lastWs?.sentMessages.find((m) => m.event === 'hello');
    expect(hello).toBeDefined();
    expect(hello!.data).toMatchObject({ displayName: 'TestUser', clientId: 'cid-1' });
    expect((hello!.data as Record<string, unknown>).chatOnly).toBeUndefined();

    client.disconnect();
  });
});

describe('createChatClient.connect() onopen', () => {
  it('sends hello with displayName, clientId, and chatOnly: true', async () => {
    const client = createChatClient();
    const p = client.connect({
      wsUrl: 'ws://localhost:8080/sfu',
      displayName: 'ChatUser',
      clientId: 'cid-2',
    });

    await connectAndWelcome(p);

    const hello = lastWs?.sentMessages.find((m) => m.event === 'hello');
    expect(hello).toBeDefined();
    expect(hello!.data).toMatchObject({
      displayName: 'ChatUser',
      clientId: 'cid-2',
      chatOnly: true,
    });

    client.disconnect();
  });
});
