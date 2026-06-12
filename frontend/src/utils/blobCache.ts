// IndexedDB cache for downloaded attachment blobs, keyed by uploadId. The
// server only holds a file transiently, so caching the bytes locally is what
// lets images survive reloads and stay visible after the server has evicted
// the upload. All operations fail soft: a missing IndexedDB (or any error)
// degrades to "not cached" rather than throwing into the render path.

const DB_NAME = 'voice-hub-blobs';
const STORE = 'blobs';
// Per-room chat history shares this DB (one connection, related data). Value is
// the room's message array, keyed out-of-line by roomId.
const CHAT_STORE = 'chat';
const DB_VERSION = 2;

// Total-bytes ceiling for cached blobs; oldest-first (LRU) eviction past this.
// Age-based cleanup is driven by chat-history retention (a dropped message
// deletes its blobs), so this size cap is the only blob-level limit.
const MAX_TOTAL_BYTES = 1024 * 1024 * 1024;

export type CachedBlob = {
  uploadId: string;
  blob: Blob;
  savedAt: number;
  size: number;
};

function supported(): boolean {
  return typeof indexedDB !== 'undefined';
}

let dbPromise: Promise<IDBDatabase> | null = null;

function openDB(): Promise<IDBDatabase> {
  if (dbPromise) return dbPromise;
  dbPromise = new Promise((resolve, reject) => {
    const req = indexedDB.open(DB_NAME, DB_VERSION);
    req.onupgradeneeded = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains(STORE)) {
        db.createObjectStore(STORE, { keyPath: 'uploadId' });
      }
      if (!db.objectStoreNames.contains(CHAT_STORE)) {
        db.createObjectStore(CHAT_STORE);
      }
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
  return dbPromise;
}

function request<T>(
  mode: IDBTransactionMode,
  run: (store: IDBObjectStore) => IDBRequest<T>,
  storeName: string = STORE,
): Promise<T> {
  return openDB().then(
    (db) =>
      new Promise<T>((resolve, reject) => {
        const tx = db.transaction(storeName, mode);
        const req = run(tx.objectStore(storeName));
        req.onsuccess = () => resolve(req.result);
        req.onerror = () => reject(req.error);
      }),
  );
}

/** Stores (or overwrites) a blob. savedAt is overridable for deterministic tests. */
export async function putBlob(
  uploadId: string,
  blob: Blob,
  savedAt: number = Date.now(),
): Promise<void> {
  if (!supported()) return;
  try {
    const record: CachedBlob = { uploadId, blob, savedAt, size: blob.size };
    await request('readwrite', (s) => s.put(record));
  } catch {
    /* best effort */
  }
}

export async function getBlob(uploadId: string): Promise<Blob | null> {
  if (!supported()) return null;
  try {
    const record = await request<CachedBlob | undefined>('readonly', (s) => s.get(uploadId));
    return record ? record.blob : null;
  } catch {
    return null;
  }
}

export async function hasBlob(uploadId: string): Promise<boolean> {
  if (!supported()) return false;
  try {
    const key = await request<IDBValidKey | undefined>('readonly', (s) => s.getKey(uploadId));
    return key !== undefined;
  } catch {
    return false;
  }
}

export async function deleteBlob(uploadId: string): Promise<void> {
  if (!supported()) return;
  try {
    await request('readwrite', (s) => s.delete(uploadId));
  } catch {
    /* best effort */
  }
}

/**
 * Moves a blob from a temporary key to its real uploadId once the upload
 * resolves, avoiding a duplicate copy of large bytes. No-op if the source is
 * already gone.
 */
export async function rekeyBlob(fromId: string, toId: string): Promise<void> {
  if (fromId === toId) return;
  const blob = await getBlob(fromId);
  if (blob) await putBlob(toId, blob);
  await deleteBlob(fromId);
}

/**
 * Evicts cached blobs oldest-first until the total is under maxTotalBytes,
 * returning the evicted uploadIds so the caller can mark those attachments
 * deleted in chat history. maxTotalBytes is overridable for tests.
 */
export async function pruneBlobs(maxTotalBytes: number = MAX_TOTAL_BYTES): Promise<string[]> {
  if (!supported()) return [];
  try {
    const all = await request<CachedBlob[]>(
      'readonly',
      (s) => s.getAll() as IDBRequest<CachedBlob[]>,
    );
    all.sort((a, b) => a.savedAt - b.savedAt); // oldest first
    let total = all.reduce((sum, r) => sum + r.size, 0);
    const evicted: string[] = [];
    for (let i = 0; i < all.length && total > maxTotalBytes; i++) {
      evicted.push(all[i].uploadId);
      total -= all[i].size;
    }

    if (evicted.length === 0) return [];
    const db = await openDB();
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction(STORE, 'readwrite');
      const store = tx.objectStore(STORE);
      for (const id of evicted) store.delete(id);
      tx.oncomplete = () => resolve();
      tx.onerror = () => reject(tx.error);
    });
    return evicted;
  } catch {
    return [];
  }
}

// --- Chat history records (store keyed by roomId) ---

/** Reads a room's stored value, or undefined when absent / IndexedDB is off. */
export async function getChatRecord<T>(roomId: string): Promise<T | undefined> {
  if (!supported()) return undefined;
  try {
    return await request<T | undefined>('readonly', (s) => s.get(roomId), CHAT_STORE);
  } catch {
    return undefined;
  }
}

/** Overwrites a room's stored value. Fails soft. */
export async function putChatRecord(roomId: string, value: unknown): Promise<void> {
  if (!supported()) return;
  try {
    await request('readwrite', (s) => s.put(value, roomId), CHAT_STORE);
  } catch {
    /* best effort */
  }
}
