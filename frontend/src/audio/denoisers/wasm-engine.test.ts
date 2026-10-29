import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { instantiateEngine } from './wasm-engine';

const asset = (name: string) => {
  const b = readFileSync(new URL(`../wasm/${name}`, import.meta.url));
  return b.buffer.slice(b.byteOffset, b.byteOffset + b.byteLength);
};

const rms = (x: Float32Array) => Math.sqrt(x.reduce((s, v) => s + v * v, 0) / x.length);

// Deterministic white noise at -20 dBFS: every engine should attenuate it hard.
function noise(n: number): Float32Array {
  let seed = 1;
  return Float32Array.from({ length: n }, () => {
    seed = (seed * 1103515245 + 12345) >>> 0;
    return (seed / 2 ** 32 - 0.5) * 0.35;
  });
}

describe.each([
  ['rnnoise', null, 480],
  ['fastenhancer', 'fastenhancer-t.bin', 512],
  ['fastenhancer', 'fastenhancer-b.bin', 512],
])('%s %s', (wasm, weights, frameSize) => {
  it('suppresses stationary noise', async () => {
    const engine = await instantiateEngine(asset(`${wasm}.wasm`), weights ? asset(weights) : null);
    expect(engine.frameSize).toBe(frameSize);

    const input = noise(frameSize * 200);
    const output = input.slice();
    for (let off = 0; off < output.length; off += frameSize) {
      engine.process(output.subarray(off, off + frameSize));
    }
    engine.destroy();

    const tail = output.subarray(output.length / 2);
    expect(tail.every(Number.isFinite)).toBe(true);
    expect(rms(tail) / rms(input)).toBeLessThan(0.1);
  });
});

// Not FastEnhancer weights at all, and a valid "FE48" v1 header with every dimension 0.
const zeroDims = new Int32Array(18);
zeroDims.set([0x38344546, 1]);

it.each([
  ['garbage', new ArrayBuffer(1024)],
  ['zero dimensions', zeroDims.buffer],
])('fastenhancer rejects %s weights', async (_, weights) => {
  await expect(instantiateEngine(asset('fastenhancer.wasm'), weights)).rejects.toThrow(
    'dn_create failed',
  );
});
