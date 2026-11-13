import { describe, expect, it } from 'vitest';
import { FIT, MAX_SCALE, panBy, zoomAt } from './zoom';

const size = { width: 400, height: 300 };

describe('zoomAt', () => {
  it('keeps the image point under the anchor in place', () => {
    const anchor = { x: 120, y: -50 };
    const before = { scale: 1.5, x: 10, y: 20 };
    const after = zoomAt(before, 3, anchor, size);
    const local = (z: typeof before) => ({
      x: (anchor.x - z.x) / z.scale,
      y: (anchor.y - z.y) / z.scale,
    });
    expect(after.scale).toBe(3);
    expect(local(after).x).toBeCloseTo(local(before).x);
    expect(local(after).y).toBeCloseTo(local(before).y);
  });

  it('snaps back to fit at or below 1×', () => {
    expect(zoomAt({ scale: 2, x: 80, y: -40 }, 0.5, { x: 0, y: 0 }, size)).toEqual(FIT);
    expect(zoomAt({ scale: 1.2, x: 5, y: 5 }, 1.005, { x: 0, y: 0 }, size)).toEqual(FIT);
  });

  it('caps the scale', () => {
    expect(zoomAt(FIT, 100, { x: 0, y: 0 }, size).scale).toBe(MAX_SCALE);
  });
});

describe('panBy', () => {
  it('stops once an edge reaches the fitted centre', () => {
    expect(panBy({ scale: 2, x: 0, y: 0 }, 1000, -1000, size)).toEqual({
      scale: 2,
      x: 400,
      y: -300,
    });
  });
});
