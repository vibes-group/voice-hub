// Golden-fixture tests for frontend/src/sfu/protocol.ts.
//
// Fixtures live in backend/internal/sfu/protocol/testdata/*.json.
// Go writes them; TS reads them. A Go field rename changes a fixture,
// which causes these tests to fail at CI — that is the intended behaviour.

import { readFileSync } from 'fs';
import { resolve } from 'path';
import { describe, it, expect, vi } from 'vitest';
import {
  parseServerMessage,
  type WelcomePayload,
  type PeerInfo,
  type PeerLeftPayload,
  type HelloPayload,
  type SetDisplayNamePayload,
  type PeerStatePayload,
  type SetStatePayload,
} from './protocol';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const FIXTURE_DIR = resolve(__dirname, '../../../backend/internal/sfu/protocol/testdata');

function readFixture(name: string): unknown {
  const raw = readFileSync(resolve(FIXTURE_DIR, name), 'utf-8');
  return JSON.parse(raw);
}

/** Wraps a payload in a wire envelope and serialises it back to a string. */
function envelope(event: string, data: unknown): string {
  return JSON.stringify({ event, data });
}

// ---------------------------------------------------------------------------
// Fixture: welcome.json  →  WelcomePayload
// ---------------------------------------------------------------------------

describe('welcome fixture', () => {
  it('has required WelcomePayload shape', () => {
    const data = readFixture('welcome.json') as WelcomePayload;
    expect(typeof data.id).toBe('string');
    expect(Array.isArray(data.peers)).toBe(true);
    for (const peer of data.peers) {
      expect(typeof peer.id).toBe('string');
      // displayName is optional; when present it must be a string
      if ('displayName' in peer) {
        expect(typeof peer.displayName).toBe('string');
      }
    }
  });

  it('parseServerMessage accepts welcome envelope', () => {
    const data = readFixture('welcome.json');
    const msg = parseServerMessage(envelope('welcome', data));
    expect(msg).not.toBeNull();
    expect(msg!.event).toBe('welcome');
    const welcome = msg as { event: 'welcome'; data: WelcomePayload };
    expect(typeof welcome.data.id).toBe('string');
    expect(Array.isArray(welcome.data.peers)).toBe(true);
  });

  it('matches exact fixture values', () => {
    const data = readFixture('welcome.json') as WelcomePayload;
    expect(data.id).toBe('abc12345def56789');
    expect(data.peers).toHaveLength(2);
    expect(data.peers[0].id).toBe('11223344aabbccdd');
    expect(data.peers[0].displayName).toBe('Alice');
    expect(data.peers[1].id).toBe('99887766ffeeddcc');
    expect(data.peers[1].displayName).toBeUndefined();
  });
});

// ---------------------------------------------------------------------------
// Fixture: peer-joined.json  →  PeerInfo
// ---------------------------------------------------------------------------

describe('peer-joined fixture', () => {
  it('has required PeerInfo shape', () => {
    const data = readFixture('peer-joined.json') as PeerInfo;
    expect(typeof data.id).toBe('string');
    if ('displayName' in data) {
      expect(typeof data.displayName).toBe('string');
    }
  });

  it('parseServerMessage accepts peer-joined envelope', () => {
    const data = readFixture('peer-joined.json');
    const msg = parseServerMessage(envelope('peer-joined', data));
    expect(msg).not.toBeNull();
    expect(msg!.event).toBe('peer-joined');
  });

  it('matches exact fixture values', () => {
    const data = readFixture('peer-joined.json') as PeerInfo;
    expect(data.id).toBe('11223344aabbccdd');
    expect(data.displayName).toBe('Alice');
  });
});

// ---------------------------------------------------------------------------
// Fixture: peer-left.json  →  PeerLeftPayload
// ---------------------------------------------------------------------------

describe('peer-left fixture', () => {
  it('has required PeerLeftPayload shape', () => {
    const data = readFixture('peer-left.json') as PeerLeftPayload;
    expect(typeof data.id).toBe('string');
  });

  it('parseServerMessage accepts peer-left envelope', () => {
    const data = readFixture('peer-left.json');
    const msg = parseServerMessage(envelope('peer-left', data));
    expect(msg).not.toBeNull();
    expect(msg!.event).toBe('peer-left');
  });

  it('matches exact fixture values', () => {
    const data = readFixture('peer-left.json') as PeerLeftPayload;
    expect(data.id).toBe('11223344aabbccdd');
    expect(Object.keys(data)).not.toContain('displayName');
  });
});

// ---------------------------------------------------------------------------
// Fixture: peer-info.json  →  PeerInfo
// ---------------------------------------------------------------------------

describe('peer-info fixture', () => {
  it('has required PeerInfo shape', () => {
    const data = readFixture('peer-info.json') as PeerInfo;
    expect(typeof data.id).toBe('string');
    if ('displayName' in data) {
      expect(typeof data.displayName).toBe('string');
    }
  });

  it('parseServerMessage accepts peer-info envelope', () => {
    const data = readFixture('peer-info.json');
    const msg = parseServerMessage(envelope('peer-info', data));
    expect(msg).not.toBeNull();
    expect(msg!.event).toBe('peer-info');
  });

  it('matches exact fixture values', () => {
    const data = readFixture('peer-info.json') as PeerInfo;
    expect(data.id).toBe('11223344aabbccdd');
    expect(data.displayName).toBe('Bob');
  });
});

// ---------------------------------------------------------------------------
// Fixture: hello.json  →  HelloPayload (client→server, shape check only)
// ---------------------------------------------------------------------------

describe('hello fixture', () => {
  it('has required HelloPayload shape', () => {
    const data = readFixture('hello.json') as HelloPayload;
    expect(typeof data.displayName).toBe('string');
  });

  it('matches exact fixture values', () => {
    const data = readFixture('hello.json') as HelloPayload;
    expect(data.displayName).toBe('Alice');
  });
});

// ---------------------------------------------------------------------------
// Fixture: set-displayname.json  →  SetDisplayNamePayload (client→server)
// ---------------------------------------------------------------------------

describe('set-displayname fixture', () => {
  it('has required SetDisplayNamePayload shape', () => {
    const data = readFixture('set-displayname.json') as SetDisplayNamePayload;
    expect(typeof data.displayName).toBe('string');
  });

  it('matches exact fixture values', () => {
    const data = readFixture('set-displayname.json') as SetDisplayNamePayload;
    expect(data.displayName).toBe('Bob');
  });
});

// ---------------------------------------------------------------------------
// Fixture: peer-state.json  →  PeerStatePayload (server→client)
// ---------------------------------------------------------------------------

describe('peer-state fixture', () => {
  it('has required PeerStatePayload shape', () => {
    const data = readFixture('peer-state.json') as PeerStatePayload;
    expect(typeof data.id).toBe('string');
    expect(typeof data.selfMuted).toBe('boolean');
    expect(typeof data.deafened).toBe('boolean');
  });

  it('parseServerMessage accepts peer-state envelope', () => {
    const data = readFixture('peer-state.json');
    const msg = parseServerMessage(envelope('peer-state', data));
    expect(msg).not.toBeNull();
    expect(msg!.event).toBe('peer-state');
    const ps = msg as { event: 'peer-state'; data: PeerStatePayload };
    expect(typeof ps.data.id).toBe('string');
    expect(typeof ps.data.selfMuted).toBe('boolean');
    expect(typeof ps.data.deafened).toBe('boolean');
  });

  it('matches exact fixture values', () => {
    const data = readFixture('peer-state.json') as PeerStatePayload;
    expect(data.id).toBe('11223344aabbccdd');
    expect(data.selfMuted).toBe(true);
    expect(data.deafened).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// Fixture: set-state.json  →  SetStatePayload (client→server, shape check only)
// ---------------------------------------------------------------------------

describe('set-state fixture', () => {
  it('has required SetStatePayload shape', () => {
    const data = readFixture('set-state.json') as SetStatePayload;
    expect(typeof data.selfMuted).toBe('boolean');
    expect(typeof data.deafened).toBe('boolean');
  });

  it('matches exact fixture values', () => {
    const data = readFixture('set-state.json') as SetStatePayload;
    expect(data.selfMuted).toBe(true);
    expect(data.deafened).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// parseServerMessage — unit tests
// ---------------------------------------------------------------------------

describe('parseServerMessage', () => {
  it('returns null and warns on malformed JSON', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    expect(parseServerMessage('not json {{{')).toBeNull();
    expect(warn).toHaveBeenCalledWith(
      expect.stringContaining('failed to JSON.parse'),
      expect.any(String),
    );
    warn.mockRestore();
  });

  it("returns null and warns when 'event' is missing", () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    expect(parseServerMessage(JSON.stringify({ data: {} }))).toBeNull();
    expect(warn).toHaveBeenCalled();
    warn.mockRestore();
  });

  it('returns null and warns on unknown event', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    expect(parseServerMessage(JSON.stringify({ event: 'mute-state', data: {} }))).toBeNull();
    expect(warn).toHaveBeenCalledWith(
      expect.stringContaining('unknown server event'),
      'mute-state',
    );
    warn.mockRestore();
  });

  it("returns null and warns when welcome is missing 'peers'", () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    const raw = JSON.stringify({ event: 'welcome', data: { id: 'abc' } });
    expect(parseServerMessage(raw)).toBeNull();
    expect(warn).toHaveBeenCalledWith(
      expect.stringContaining("malformed 'welcome'"),
      expect.anything(),
    );
    warn.mockRestore();
  });

  it("returns null and warns when peer-joined is missing 'id'", () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    const raw = JSON.stringify({ event: 'peer-joined', data: { displayName: 'X' } });
    expect(parseServerMessage(raw)).toBeNull();
    expect(warn).toHaveBeenCalled();
    warn.mockRestore();
  });

  it('accepts a well-formed offer (audio pc)', () => {
    const raw = JSON.stringify({
      event: 'offer',
      data: { pc: 'audio', type: 'offer', sdp: 'v=0\r\n...' },
    });
    const msg = parseServerMessage(raw);
    expect(msg).not.toBeNull();
    expect(msg!.event).toBe('offer');
  });

  it('accepts a well-formed candidate (screen-pub pc)', () => {
    const raw = JSON.stringify({
      event: 'candidate',
      data: {
        pc: 'screen-pub',
        candidate: 'candidate:1 1 UDP 2122252543 192.168.1.1 54321 typ host',
      },
    });
    const msg = parseServerMessage(raw);
    expect(msg).not.toBeNull();
    expect(msg!.event).toBe('candidate');
  });

  it('accepts a well-formed chat-deleted', () => {
    const raw = JSON.stringify({
      event: 'chat-deleted',
      data: { id: '01HXXXXXXXXXXXXXXXXXXXXXXX' },
    });
    const msg = parseServerMessage(raw);
    expect(msg).not.toBeNull();
    expect(msg!.event).toBe('chat-deleted');
    expect((msg as { event: 'chat-deleted'; data: { id: string } }).data.id).toBe(
      '01HXXXXXXXXXXXXXXXXXXXXXXX',
    );
  });

  it("returns null and warns when chat-deleted is missing 'id'", () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    expect(parseServerMessage(JSON.stringify({ event: 'chat-deleted', data: {} }))).toBeNull();
    expect(warn).toHaveBeenCalledWith(
      expect.stringContaining("malformed 'chat-deleted'"),
      expect.anything(),
    );
    warn.mockRestore();
  });

  it("returns null and warns when offer is missing 'pc'", () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    const raw = JSON.stringify({
      event: 'offer',
      data: { type: 'offer', sdp: 'v=0\r\n...' },
    });
    expect(parseServerMessage(raw)).toBeNull();
    expect(warn).toHaveBeenCalled();
    warn.mockRestore();
  });

  it("returns null and warns when offer is missing 'sdp'", () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    const raw = JSON.stringify({ event: 'offer', data: { pc: 'audio', type: 'offer' } });
    expect(parseServerMessage(raw)).toBeNull();
    expect(warn).toHaveBeenCalled();
    warn.mockRestore();
  });

  it('parses candidate-screen-pub.json fixture', () => {
    const data = readFixture('candidate-screen-pub.json');
    const msg = parseServerMessage(envelope('candidate', data));
    expect(msg).not.toBeNull();
    if (msg && msg.event === 'candidate') {
      expect(msg.data.pc).toBe('screen-pub');
    }
  });

  it('parses offer-audio.json fixture', () => {
    const data = readFixture('offer-audio.json');
    const msg = parseServerMessage(envelope('offer', data));
    expect(msg).not.toBeNull();
    expect(msg!.event).toBe('offer');
    if (msg && msg.event === 'offer') {
      expect(msg.data.pc).toBe('audio');
      expect(msg.data.type).toBe('offer');
    }
  });

  it('parses answer-screen-sub.json fixture into a CandidateEnvelope-like answer envelope', () => {
    const data = readFixture('answer-screen-sub.json') as Record<string, unknown>;
    expect(data.pc).toBe('screen-sub');
    expect(data.publisherId).toBe('11223344aabbccdd');
    expect(data.type).toBe('answer');
  });

  it('parses screen-share-available fixture', () => {
    const data = readFixture('screen-share-available.json');
    const msg = parseServerMessage(envelope('screen-share-available', data));
    expect(msg).not.toBeNull();
    expect(msg!.event).toBe('screen-share-available');
    if (msg && msg.event === 'screen-share-available') {
      expect(msg.data.videoCodec).toBe('av1');
    }
  });

  it('parses screen-share-error fixture', () => {
    const data = readFixture('screen-share-error.json');
    const msg = parseServerMessage(envelope('screen-share-error', data));
    expect(msg).not.toBeNull();
    if (msg && msg.event === 'screen-share-error') {
      expect(msg.data.reason).toBe('already-publishing');
    }
  });

  it('parses screen-share-encode-pause fixture', () => {
    const data = readFixture('screen-share-encode-pause.json');
    const msg = parseServerMessage(envelope('screen-share-encode-pause', data));
    expect(msg).not.toBeNull();
    if (msg && msg.event === 'screen-share-encode-pause') {
      expect(msg.data.layers).toEqual([0, 1, 2]);
    }
  });
});

// ---------------------------------------------------------------------------
// chat attachments
// ---------------------------------------------------------------------------

describe('chat attachments', () => {
  const base = { id: 'ULID', from: 'peer-1', text: 'look', ts: 1 };

  it('accepts a chat payload with a valid attachment array', () => {
    const attachment = {
      uploadId: 'up-1',
      kind: 'image',
      name: 'pic.png',
      mime: 'image/png',
      size: 1234,
      width: 800,
      height: 600,
      blurThumb: 'data:image/jpeg;base64,AAAA',
    };
    const msg = parseServerMessage(envelope('chat', { ...base, attachments: [attachment] }));
    expect(msg).not.toBeNull();
    if (msg && msg.event === 'chat') {
      expect(msg.data.attachments).toHaveLength(1);
      expect(msg.data.attachments![0].uploadId).toBe('up-1');
      expect(msg.data.attachments![0].kind).toBe('image');
    }
  });

  it('accepts a chat payload with empty text but attachments', () => {
    const attachment = {
      uploadId: 'up-2',
      kind: 'file',
      name: 'a.bin',
      mime: 'application/octet-stream',
      size: 9,
    };
    const msg = parseServerMessage(
      envelope('chat', { ...base, text: '', attachments: [attachment] }),
    );
    expect(msg).not.toBeNull();
  });

  it('rejects a chat payload whose attachments is not an array', () => {
    const msg = parseServerMessage(envelope('chat', { ...base, attachments: 'nope' }));
    expect(msg).toBeNull();
  });

  it('rejects a chat payload with a malformed attachment (bad kind)', () => {
    const bad = { uploadId: 'up-3', kind: 'gif', name: 'x', mime: 'image/gif', size: 1 };
    const msg = parseServerMessage(envelope('chat', { ...base, attachments: [bad] }));
    expect(msg).toBeNull();
  });

  it('rejects a chat payload with an attachment missing required fields', () => {
    const bad = { uploadId: 'up-4', kind: 'image' };
    const msg = parseServerMessage(envelope('chat', { ...base, attachments: [bad] }));
    expect(msg).toBeNull();
  });

  it('still parses a chat payload without attachments', () => {
    const msg = parseServerMessage(envelope('chat', base));
    expect(msg).not.toBeNull();
    if (msg && msg.event === 'chat') {
      expect(msg.data.attachments).toBeUndefined();
    }
  });
});
