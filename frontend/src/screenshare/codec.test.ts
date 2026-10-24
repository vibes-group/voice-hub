import { describe, expect, it, vi } from 'vitest';
import {
  applyScreenCodecPreferences,
  chooseScreenCodec,
  orderScreenCodecs,
  type ScreenCodecSupport,
} from './codec';

function support(send: ('av1' | 'vp9')[]): ScreenCodecSupport {
  return {
    send: new Set(send),
    receive: new Set(send),
  };
}

const codecs = [
  { mimeType: 'video/VP8', clockRate: 90000 },
  { mimeType: 'video/VP9', clockRate: 90000 },
  { mimeType: 'video/AV1', clockRate: 90000 },
] as RTCRtpCodec[];

describe('screen codec policy', () => {
  it('returns the requested codec when it is supported', () => {
    expect(chooseScreenCodec('av1', support(['av1', 'vp9']))).toBe('av1');
    expect(chooseScreenCodec('vp9', support(['av1', 'vp9']))).toBe('vp9');
  });

  it('falls back to AV1 > VP9 order when the requested codec is unsupported', () => {
    expect(chooseScreenCodec('av1', support(['vp9']))).toBe('vp9');
    expect(chooseScreenCodec('vp9', support(['av1']))).toBe('av1');
  });

  it('orders AV1 before VP9 while retaining VP9 as negotiation fallback', () => {
    expect(orderScreenCodecs(codecs, 'av1').map((c) => c.mimeType)).toEqual([
      'video/AV1',
      'video/VP9',
      'video/VP8',
    ]);
  });

  it('forces VP9-only codec preferences when VP9 was selected', () => {
    const tx = { setCodecPreferences: vi.fn() } as unknown as RTCRtpTransceiver;
    applyScreenCodecPreferences(tx, { codecs, headerExtensions: [] }, 'vp9');
    expect(tx.setCodecPreferences).toHaveBeenCalledWith([
      { mimeType: 'video/VP9', clockRate: 90000 },
    ]);
  });
});
