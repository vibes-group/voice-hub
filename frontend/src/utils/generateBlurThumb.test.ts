import { afterEach, describe, expect, it, vi } from 'vitest';
import { generateBlurThumb } from './uploadFile';

class FakeOffscreenCanvas {
  drawCalls = 0;
  constructor(
    public width: number,
    public height: number,
  ) {}
  getContext() {
    return {
      drawImage: () => {
        this.drawCalls++;
      },
    };
  }
  convertToBlob() {
    return Promise.resolve(new Blob([new Uint8Array([1, 2, 3, 4])], { type: 'image/jpeg' }));
  }
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('generateBlurThumb', () => {
  it('produces a base64 JPEG data URL via OffscreenCanvas', async () => {
    vi.stubGlobal('OffscreenCanvas', FakeOffscreenCanvas);

    const url = await generateBlurThumb({} as CanvasImageSource);

    expect(url.startsWith('data:image/jpeg;base64,')).toBe(true);
    // base64 of bytes [1,2,3,4]
    expect(url).toBe(`data:image/jpeg;base64,${btoa('\x01\x02\x03\x04')}`);
  });

  it('returns empty string when no canvas backend is available', async () => {
    vi.stubGlobal('OffscreenCanvas', undefined);
    vi.stubGlobal('document', undefined);

    const url = await generateBlurThumb({} as CanvasImageSource);
    expect(url).toBe('');
  });
});
