import 'fake-indexeddb/auto';
import { IDBFactory } from 'fake-indexeddb';
import { beforeEach, describe, expect, it, vi } from 'vitest';

// Each test gets a fresh in-memory IndexedDB and a fresh module instance (so the
// module-level db promise is rebuilt against the new factory).
async function freshCache(): Promise<typeof import('./blobCache')> {
  globalThis.indexedDB = new IDBFactory();
  vi.resetModules();
  return import('./blobCache');
}

function blobOf(size: number): Blob {
  return new Blob([new Uint8Array(size)]);
}

describe('blobCache', () => {
  let cache: typeof import('./blobCache');

  beforeEach(async () => {
    cache = await freshCache();
  });

  it('round-trips a blob through put → get', async () => {
    await cache.putBlob('a', new Blob(['hello']));
    const got = await cache.getBlob('a');
    expect(got).not.toBeNull();
    expect(await got!.text()).toBe('hello');
  });

  it('reports has() true after put and false on miss', async () => {
    await cache.putBlob('present', new Blob(['x']));
    expect(await cache.hasBlob('present')).toBe(true);
    expect(await cache.hasBlob('absent')).toBe(false);
    expect(await cache.getBlob('absent')).toBeNull();
  });

  it('rekeys a blob from a temp id to the real id', async () => {
    await cache.putBlob('temp:1', new Blob(['bytes']));
    await cache.rekeyBlob('temp:1', 'real-1');
    expect(await cache.hasBlob('temp:1')).toBe(false);
    expect(await (await cache.getBlob('real-1'))!.text()).toBe('bytes');
  });

  it('evicts oldest-first (LRU) once over the size cap, returning evicted ids', async () => {
    const now = 1000;
    // Three 100-byte blobs (300 total); a 250 cap drops the single oldest.
    await cache.putBlob('o1', blobOf(100), now - 30);
    await cache.putBlob('o2', blobOf(100), now - 20);
    await cache.putBlob('o3', blobOf(100), now - 10);

    const evicted = await cache.pruneBlobs(250);

    expect(evicted).toEqual(['o1']);
    expect(await cache.hasBlob('o1')).toBe(false);
    expect(await cache.hasBlob('o2')).toBe(true);
    expect(await cache.hasBlob('o3')).toBe(true);
  });
});
