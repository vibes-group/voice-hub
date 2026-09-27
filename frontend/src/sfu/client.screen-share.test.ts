// The local publisher PC must follow the server: whenever our own share ends
// or fails, a new start has to work instead of throwing "already publishing"
// while the UI shows the share as off.

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
  primeScreenCodecProfile: () => Promise.resolve(),
  chooseScreenCodec: () => 'av1',
  canReceiveScreenCodec: () => true,
  applyScreenCodecPreferences: () => undefined,
  isScreenVideoCodec: (v: unknown) => v === 'av1' || v === 'vp9',
}));

import { createSFUClient } from './client';

type WsMessage = { event: string; data: unknown };

class StubWebSocket {
  static readonly OPEN = 1;
  readyState = StubWebSocket.OPEN;
  readonly sent: WsMessage[] = [];
  onopen: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onmessage: ((ev: { data: string }) => void) | null = null;
  constructor() {
    queueMicrotask(() => this.onopen?.());
  }
  send(raw: string): void {
    this.sent.push(JSON.parse(raw) as WsMessage);
  }
  close(): void {}
  inject(event: string, data: unknown): void {
    this.onmessage?.({ data: JSON.stringify({ event, data }) });
  }
}

let failNextOffer = false;

class StubRTCPeerConnection {
  connectionState = 'new';
  signalingState = 'stable';
  addTrack(): object {
    return { setParameters: () => Promise.resolve(), getParameters: () => ({ encodings: [] }) };
  }
  getTransceivers(): [] {
    return [];
  }
  getSenders(): [] {
    return [];
  }
  addEventListener(): void {}
  removeEventListener(): void {}
  close(): void {}
  createOffer(): Promise<RTCSessionDescriptionInit> {
    if (failNextOffer) {
      failNextOffer = false;
      return Promise.reject(new Error('offer failed'));
    }
    return Promise.resolve({ type: 'offer', sdp: '' });
  }
  setLocalDescription(): Promise<void> {
    this.signalingState = 'have-local-offer';
    return Promise.resolve();
  }
  setRemoteDescription(): Promise<void> {
    return Promise.resolve();
  }
  addIceCandidate(): Promise<void> {
    return Promise.resolve();
  }
}

function fakeScreenStream() {
  const track = {
    kind: 'video',
    contentHint: '',
    stop: vi.fn(),
    getSettings: () => ({ displaySurface: 'window', width: 1920, height: 1080 }),
    applyConstraints: () => Promise.resolve(),
    addEventListener: () => undefined,
  };
  return {
    track,
    stream: {
      getVideoTracks: () => [track],
      getAudioTracks: () => [],
      getTracks: () => [track],
    },
  };
}

let ws: StubWebSocket;

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  failNextOffer = false;
  const g = globalThis as Record<string, unknown>;
  g.WebSocket = Object.assign(
    function () {
      ws = new StubWebSocket();
      return ws;
    },
    { OPEN: StubWebSocket.OPEN },
  );
  g.RTCPeerConnection = StubRTCPeerConnection;
  g.RTCRtpSender = { getCapabilities: () => ({ codecs: [] }) };
  g.MediaStream = class {
    getTracks(): [] {
      return [];
    }
  };
  vi.stubGlobal('navigator', {
    mediaDevices: { getDisplayMedia: () => Promise.resolve(fakeScreenStream().stream) },
  });
});

afterEach(() => {
  vi.useRealTimers();
  const g = globalThis as Record<string, unknown>;
  for (const k of ['WebSocket', 'RTCPeerConnection', 'RTCRtpSender', 'MediaStream']) delete g[k];
  vi.unstubAllGlobals();
});

async function joined() {
  const client = createSFUClient();
  const connected = client.connect({
    wsUrl: 'ws://test/sfu',
    iceServers: [],
    localStream: new (
      globalThis as unknown as { MediaStream: new () => MediaStream }
    ).MediaStream(),
    displayName: 'me',
    clientId: 'cid',
  });
  await Promise.resolve();
  await Promise.resolve();
  ws.inject('welcome', { id: 'me', peers: [] });
  await connected;
  return client;
}

const stopsSent = () => ws.sent.filter((m) => m.event === 'screen-share-stop').length;

// Start without awaiting: the answer never comes in these tests.
async function startPublishing(client: Awaited<ReturnType<typeof joined>>) {
  void client.startScreenShare().catch(() => undefined);
  for (let i = 0; i < 10 && !client.isPublishingScreenShare(); i++) await Promise.resolve();
  expect(client.isPublishingScreenShare()).toBe(true);
}

describe('screen share publisher state', () => {
  it('drops the local publisher when the server ends our current share', async () => {
    const client = await joined();
    await startPublishing(client);
    ws.inject('screen-share-started', { sessionToken: 't1' });

    ws.inject('screen-share-ended', { publisherId: 'me' });

    expect(client.isPublishingScreenShare()).toBe(false);
    await startPublishing(client);
    client.disconnect();
  });

  it('ignores the echo of our own stop when a new share already started', async () => {
    const client = await joined();
    await startPublishing(client);
    ws.inject('screen-share-started', { sessionToken: 't1' });
    client.stopScreenShare();
    await startPublishing(client);

    ws.inject('screen-share-ended', { publisherId: 'me' });

    expect(client.isPublishingScreenShare()).toBe(true);
    client.disconnect();
  });

  it("keeps publishing when someone else's share ends", async () => {
    const client = await joined();
    await startPublishing(client);
    ws.inject('screen-share-started', { sessionToken: 't1' });

    ws.inject('screen-share-ended', { publisherId: 'other' });

    expect(client.isPublishingScreenShare()).toBe(true);
    client.disconnect();
  });

  it('drops the local publisher when the server rejects our start', async () => {
    const client = await joined();
    await startPublishing(client);

    ws.inject('screen-share-error', { publisherId: '', reason: 'internal' });

    expect(client.isPublishingScreenShare()).toBe(false);
    await startPublishing(client);
    client.disconnect();
  });

  it("a rejected start's answer timeout doesn't touch the next share", async () => {
    const client = await joined();
    await startPublishing(client);
    await vi.advanceTimersByTimeAsync(6000);
    ws.inject('screen-share-error', { publisherId: '', reason: 'internal' });
    await startPublishing(client);
    const stops = stopsSent();

    // Past the first start's 10 s answer timeout, before the second's.
    await vi.advanceTimersByTimeAsync(5000);

    expect(client.isPublishingScreenShare()).toBe(true);
    expect(stopsSent()).toBe(stops);
    client.disconnect();
  });

  it('a stop settles the pending start at once, not after the answer timeout', async () => {
    const client = await joined();
    const pending = client.startScreenShare();
    for (let i = 0; i < 10 && !client.isPublishingScreenShare(); i++) await Promise.resolve();

    ws.inject('screen-share-error', { publisherId: '', reason: 'internal' });

    // The UI treats AbortError as "nothing to report"; a late timeout error
    // would instead reset a share the user started in the meantime.
    await expect(pending).rejects.toMatchObject({ name: 'AbortError' });
    client.disconnect();
  });

  it('releases the capture when the start fails midway', async () => {
    const client = await joined();
    const { stream, track } = fakeScreenStream();
    vi.stubGlobal('navigator', {
      mediaDevices: { getDisplayMedia: () => Promise.resolve(stream) },
    });
    failNextOffer = true;

    await expect(client.startScreenShare()).rejects.toThrow('offer failed');

    expect(track.stop).toHaveBeenCalled();
    expect(client.isPublishingScreenShare()).toBe(false);
    await startPublishing(client);
    client.disconnect();
  });
});
