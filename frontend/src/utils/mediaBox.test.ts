import { describe, expect, it } from 'vitest';
import { mediaBoxStyle } from './mediaBox';

describe('mediaBoxStyle', () => {
  it('keeps the natural ratio and width of ordinary images', () => {
    const s = mediaBoxStyle({ width: 200, height: 100 });
    expect(s.aspectRatio).toBe('2');
    expect(s.width).toBe('min(320px, 200px, calc(min(360px, 50vh) * 2))');
  });

  it('clamps a very tall image so it cannot outgrow the height limit', () => {
    const s = mediaBoxStyle({ width: 1080, height: 10000 });
    expect(s.aspectRatio).toBe('0.5');
    expect(s.width).toBe('min(320px, 1080px, calc(min(360px, 50vh) * 0.5))');
  });

  it('clamps a very wide image and enforces a minimum width', () => {
    expect(mediaBoxStyle({ width: 5000, height: 100 }).aspectRatio).toBe('3');
    expect(mediaBoxStyle({ width: 16, height: 16 }).width).toContain('64px');
  });

  it('falls back to 4:3 until dimensions are known, then to the measured ones', () => {
    const s = mediaBoxStyle({ height: 0 });
    expect(s.aspectRatio).toBe('1.3333');
    expect(s.width).toBe('min(320px, calc(min(360px, 50vh) * 1.3333))');
    expect(mediaBoxStyle({ width: 0 }, { width: 200, height: 100 }).aspectRatio).toBe('2');
  });
});
