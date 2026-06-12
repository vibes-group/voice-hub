import { afterEach, describe, expect, it, vi } from 'vitest';
import { uploadFile, UploadError } from './uploadFile';

type ProgressEvent = { lengthComputable: boolean; loaded: number; total: number };

class MockXHR {
  static last: MockXHR | null = null;

  upload = { onprogress: null as ((e: ProgressEvent) => void) | null };
  onload: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onabort: (() => void) | null = null;
  status = 0;
  responseText = '';
  withCredentials = false;
  headers: Record<string, string> = {};
  method = '';
  url = '';
  body: unknown = null;

  constructor() {
    MockXHR.last = this;
  }
  open(method: string, url: string) {
    this.method = method;
    this.url = url;
  }
  setRequestHeader(key: string, value: string) {
    this.headers[key] = value;
  }
  send(body: unknown) {
    this.body = body;
  }
  abort() {
    this.onabort?.();
  }
  // test drivers
  emitProgress(loaded: number, total: number) {
    this.upload.onprogress?.({ lengthComputable: true, loaded, total });
  }
  complete(status: number, responseText = '') {
    this.status = status;
    this.responseText = responseText;
    this.onload?.();
  }
}

afterEach(() => {
  vi.unstubAllGlobals();
  MockXHR.last = null;
});

describe('uploadFile', () => {
  it('reports progress and resolves the uploadId on 201', async () => {
    vi.stubGlobal('XMLHttpRequest', MockXHR);
    const progress: number[] = [];
    const p = uploadFile(new Blob(['data'], { type: 'image/png' }), 'room1', {
      name: 'pic.png',
      onProgress: (f) => progress.push(f),
    });

    const xhr = MockXHR.last!;
    expect(xhr.method).toBe('POST');
    expect(xhr.url).toContain('room=room1');
    expect(xhr.withCredentials).toBe(true);

    xhr.emitProgress(50, 100);
    xhr.emitProgress(100, 100);
    xhr.complete(201, JSON.stringify({ uploadId: 'ULID123' }));

    await expect(p).resolves.toBe('ULID123');
    expect(progress).toEqual([0.5, 1]);
  });

  it('rejects with an UploadError carrying the status on non-201', async () => {
    vi.stubGlobal('XMLHttpRequest', MockXHR);
    const p = uploadFile(new Blob(['x']), 'room1', { name: 'big.bin' });
    MockXHR.last!.complete(413);
    await expect(p).rejects.toBeInstanceOf(UploadError);
    await expect(p).rejects.toMatchObject({ status: 413 });
  });

  it('rejects on a malformed 201 response', async () => {
    vi.stubGlobal('XMLHttpRequest', MockXHR);
    const p = uploadFile(new Blob(['x']), 'room1', { name: 'f' });
    MockXHR.last!.complete(201, 'not json');
    await expect(p).rejects.toBeInstanceOf(Error);
  });

  it('rejects with AbortError when the signal aborts', async () => {
    vi.stubGlobal('XMLHttpRequest', MockXHR);
    const controller = new AbortController();
    const p = uploadFile(new Blob(['x']), 'room1', { name: 'f', signal: controller.signal });
    controller.abort();
    await expect(p).rejects.toMatchObject({ name: 'AbortError' });
  });

  it('rejects immediately if the signal is already aborted', async () => {
    vi.stubGlobal('XMLHttpRequest', MockXHR);
    const controller = new AbortController();
    controller.abort();
    const p = uploadFile(new Blob(['x']), 'room1', { name: 'f', signal: controller.signal });
    await expect(p).rejects.toMatchObject({ name: 'AbortError' });
  });
});
