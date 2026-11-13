// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from 'vitest';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import type { Attachment } from '../sfu/protocol';

vi.mock('../utils/blobCache', () => ({
  getBlob: vi.fn(),
  putBlob: vi.fn().mockResolvedValue(undefined),
}));

import { getBlob } from '../utils/blobCache';
import { ImageLightbox } from './ImageLightbox';

const attachment = (uploadId: string, mime = 'image/png'): Attachment => ({
  uploadId,
  kind: 'image',
  name: `${uploadId}.png`,
  mime,
  size: 10,
});

// Preloads by URL; each decode() waits until the test settles it.
const decodes = new Map<string, { resolve: () => void; reject: () => void }>();
class FakeImage {
  src = '';
  naturalWidth = 0;
  naturalHeight = 0;
  complete = false;
  addEventListener() {}
  decode() {
    return new Promise<void>((resolve, reject) => {
      decodes.set(this.src, {
        resolve: () => {
          this.naturalWidth = 400;
          this.naturalHeight = 300;
          resolve();
        },
        reject: () => {
          this.complete = true;
          reject(new Error('EncodingError'));
        },
      });
    });
  }
}

let container: HTMLDivElement;
let root: Root;
let urls = 0;

beforeEach(() => {
  vi.stubGlobal('IS_REACT_ACT_ENVIRONMENT', true);
  vi.stubGlobal('Image', FakeImage);
  vi.stubGlobal(
    'ResizeObserver',
    class {
      constructor(callback: (entries: { contentRect: object }[]) => void) {
        queueMicrotask(() => callback([{ contentRect: { width: 800, height: 600, x: 16, y: 0 } }]));
      }
      observe() {}
      disconnect() {}
    },
  );
  urls = 0;
  (URL as unknown as { createObjectURL: () => string }).createObjectURL = vi.fn(
    () => `blob:${++urls}`,
  );
  (URL as unknown as { revokeObjectURL: () => void }).revokeObjectURL = vi.fn();
  (getBlob as Mock).mockResolvedValue(new Blob(['x'], { type: 'image/png' }));
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  decodes.clear();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  vi.clearAllMocks();
});

async function flush() {
  await act(async () => {
    await new Promise((r) => setTimeout(r, 0));
  });
}

async function show(images: Attachment[], index: number) {
  await act(async () => {
    root.render(
      <ImageLightbox
        images={images}
        index={index}
        roomId="room1"
        onClose={() => {}}
        onNavigate={() => {}}
        onDelete={() => {}}
      />,
    );
  });
  await flush();
}

async function decode(url: string, ok = true) {
  await act(async () => (ok ? decodes.get(url)!.resolve() : decodes.get(url)!.reject()));
  await flush();
}

const img = () => container.querySelector('img');

describe('ImageLightbox paging', () => {
  it('keeps the previous image up until the next one decodes', async () => {
    const images = [attachment('a'), attachment('b')];
    await show(images, 0);
    expect(img()).toBeNull();
    await decode('blob:1');
    const first = img()!;
    expect(first.getAttribute('src')).toBe('blob:1');
    expect(first.style.width).toBe('400px');

    await show(images, 1);
    expect(img()).toBe(first);
    expect(first.getAttribute('src')).toBe('blob:1');
    expect(first.className).toContain('opacity-40');

    await decode('blob:2');
    expect(img()).toBe(first);
    expect(first.getAttribute('src')).toBe('blob:2');
    expect(first.className).not.toContain('opacity-40');
  });

  it("doesn't bring back a left image after a video", async () => {
    vi.spyOn(HTMLMediaElement.prototype, 'canPlayType').mockReturnValue('probably');
    const images = [attachment('a'), attachment('v', 'video/mp4'), attachment('c')];
    await show(images, 0);
    await decode('blob:1');
    await show(images, 1);
    expect(img()).toBeNull();
    await show(images, 2);
    expect(img()).toBeNull();
    await decode('blob:3');
    expect(img()!.getAttribute('src')).toBe('blob:3');
  });

  it('offers the download when the image does not decode', async () => {
    await show([attachment('a')], 0);
    await decode('blob:1', false);
    expect(img()).toBeNull();
    expect(container.textContent).toContain('Не открывается');
  });
});
