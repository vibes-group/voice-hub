import { describe, expect, it } from 'vitest';
import { mediaBoxStyle } from './mediaBox';

describe('mediaBoxStyle', () => {
  it('keeps the natural ratio and width of ordinary images', () => {
    const s = mediaBoxStyle(200, 100);
    expect(s.aspectRatio).toBe('2');
    expect(s.width).toBe('min(320px, 200px, calc(min(360px, 50vh) * 2))');
  });

  it('clamps a very tall image so it cannot outgrow the height limit', () => {
    const s = mediaBoxStyle(1080, 10000);
    expect(s.aspectRatio).toBe('0.5');
    expect(s.width).toBe('min(320px, 1080px, calc(min(360px, 50vh) * 0.5))');
  });

  it('clamps a very wide image and enforces a minimum width', () => {
    expect(mediaBoxStyle(5000, 100).aspectRatio).toBe('3');
    expect(mediaBoxStyle(16, 16).width).toContain('64px');
  });

  it('falls back to 4:3 when dimensions are unknown', () => {
    const s = mediaBoxStyle(undefined, 0);
    expect(s.aspectRatio).toBe('1.3333');
    expect(s.width).toBe('min(320px, calc(min(360px, 50vh) * 1.3333))');
  });
});
