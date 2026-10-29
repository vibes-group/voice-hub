import { describe, expect, it } from 'vitest';
import { FrameAdapter, QUANTUM } from './frame-adapter';

function run(frameSize: number, quanta: number) {
  const adapter = new FrameAdapter(frameSize, () => undefined);
  const input = Float32Array.from({ length: quanta * QUANTUM }, (_, i) => i + 1);
  const output = new Float32Array(input.length);
  for (let q = 0; q < quanta; q++) {
    adapter.process(
      input.subarray(q * QUANTUM, (q + 1) * QUANTUM),
      output.subarray(q * QUANTUM, (q + 1) * QUANTUM),
    );
  }
  return { adapter, input, output };
}

describe('FrameAdapter', () => {
  it.each([
    [480, 4],
    [512, 3],
    [128, 0],
    [441, 4],
  ])('frame %i: delays by %i quanta without gaps', (frameSize, primeQuanta) => {
    const { adapter, input, output } = run(frameSize, 1000);
    expect(adapter.primeQuanta).toBe(primeQuanta);
    const delay = primeQuanta * QUANTUM;
    expect(Array.from(output.subarray(0, delay))).toEqual(new Array(delay).fill(0));
    expect(output.subarray(delay)).toEqual(input.subarray(0, input.length - delay));
  });

  it('hands each complete frame to processFrame', () => {
    const seen: number[] = [];
    const adapter = new FrameAdapter(480, (frame) => {
      seen.push(frame[0]);
      frame.fill(-1);
    });
    const input = Float32Array.from({ length: 15 * QUANTUM }, (_, i) => i);
    const out = new Float32Array(QUANTUM);
    for (let q = 0; q < 15; q++)
      adapter.process(input.subarray(q * QUANTUM, (q + 1) * QUANTUM), out);
    expect(seen).toEqual([0, 480, 960, 1440]);
    expect(Array.from(out)).toEqual(new Array(QUANTUM).fill(-1));
  });
});
