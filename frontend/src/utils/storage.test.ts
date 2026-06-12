// @vitest-environment jsdom
import 'fake-indexeddb/auto';
import { beforeEach, describe, expect, it } from 'vitest';
import {
  saveChatHistory,
  loadChatHistory,
  clearLegacyStorage,
  CHAT_HISTORY_CAP,
  type PersistedChatMessage,
} from './storage';
import { putBlob, getBlob } from './blobCache';

const DAY = 24 * 60 * 60 * 1000;

function fileMsg(id: string, ts: number, uploadId: string): PersistedChatMessage {
  return {
    id,
    from: 'p',
    text: '',
    ts,
    attachments: [{ uploadId, kind: 'file', name: id, mime: 'application/octet-stream', size: 1 }],
  };
}

// deleteBlob runs fire-and-forget inside saveChatHistory; let it settle.
const tick = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => localStorage.clear());

describe('chat history prune → blob cleanup', () => {
  it('drops messages past the 7-day TTL and deletes their blobs, keeping fresh ones', async () => {
    await putBlob('old-bytes', new Blob(['x']));
    await putBlob('fresh-bytes', new Blob(['y']));
    const now = Date.now();

    await saveChatHistory('room1', [
      fileMsg('old', now - 8 * DAY, 'old-bytes'),
      fileMsg('fresh', now, 'fresh-bytes'),
    ]);

    expect((await loadChatHistory('room1')).map((m) => m.id)).toEqual(['fresh']);

    await tick();
    expect(await getBlob('old-bytes')).toBeNull();
    expect(await getBlob('fresh-bytes')).not.toBeNull();
  });

  it('drops the oldest beyond the cap and deletes its blob', async () => {
    await putBlob('cap-bytes', new Blob(['z']));
    const now = Date.now();
    const msgs: PersistedChatMessage[] = [fileMsg('oldest', now - 1000, 'cap-bytes')];
    for (let i = 0; i < CHAT_HISTORY_CAP; i++)
      msgs.push({ id: `m${i}`, from: 'p', text: 'x', ts: now });

    await saveChatHistory('room2', msgs);

    expect((await loadChatHistory('room2')).some((m) => m.id === 'oldest')).toBe(false);
    await tick();
    expect(await getBlob('cap-bytes')).toBeNull();
  });

  it('keeps blobs when nothing is dropped', async () => {
    await putBlob('keep-bytes', new Blob(['k']));
    await saveChatHistory('room3', [fileMsg('m', Date.now(), 'keep-bytes')]);
    await tick();
    expect(await getBlob('keep-bytes')).not.toBeNull();
  });
});

describe('clearLegacyStorage', () => {
  it('removes orphaned voice-hub.chat.* keys, keeps everything else', () => {
    localStorage.setItem('voice-hub.chat.room1', '[]');
    localStorage.setItem('voice-hub.chat.lobby', '[]');
    localStorage.setItem('voice-hub.display-name', 'Alice');

    clearLegacyStorage();

    expect(localStorage.getItem('voice-hub.chat.room1')).toBeNull();
    expect(localStorage.getItem('voice-hub.chat.lobby')).toBeNull();
    expect(localStorage.getItem('voice-hub.display-name')).toBe('Alice');
  });
});
