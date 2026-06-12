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
import { AttachmentImage } from './AttachmentImage';

const imageAttachment: Attachment = {
  uploadId: 'up-1',
  kind: 'image',
  name: 'pic.png',
  mime: 'image/png',
  size: 10,
  width: 100,
  height: 80,
  blurThumb: 'data:image/jpeg;base64,AAAA',
};

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  (URL as unknown as { createObjectURL: () => string }).createObjectURL = vi.fn(() => 'blob:mock');
  (URL as unknown as { revokeObjectURL: () => void }).revokeObjectURL = vi.fn();
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

// Lets the cache/fetch promises and their effects settle.
async function flush() {
  await act(async () => {
    await new Promise((r) => setTimeout(r, 0));
  });
}

describe('AttachmentImage', () => {
  it('renders the cached blob as an image that fades in on load', async () => {
    (getBlob as Mock).mockResolvedValue(new Blob(['x'], { type: 'image/png' }));

    await act(async () => {
      root.render(<AttachmentImage attachment={imageAttachment} roomId="room1" />);
    });
    await flush();

    const img = container.querySelector('img[src="blob:mock"]') as HTMLImageElement | null;
    expect(img).not.toBeNull();
    // Starts transparent (fade-in begins once the image loads).
    expect(img!.style.opacity).toBe('0');

    await act(async () => {
      img!.dispatchEvent(new Event('load'));
    });
    expect(img!.style.opacity).toBe('1');
  });

  it('shows "файл недоступен" when the download 404s', async () => {
    (getBlob as Mock).mockResolvedValue(null);
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ status: 404, ok: false }));

    await act(async () => {
      root.render(<AttachmentImage attachment={imageAttachment} roomId="room1" />);
    });
    await flush();

    expect(container.textContent).toContain('Файл недоступен');
  });
});
