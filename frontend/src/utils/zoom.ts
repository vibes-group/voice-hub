// Lightbox zoom: `scale` over the fitted image, `x`/`y` its offset from the
// fitted position in CSS px. Points are relative to the fitted image's centre.
export type Zoom = { scale: number; x: number; y: number };
export type Size = { width: number; height: number };
export type Point = { x: number; y: number };

export const FIT: Zoom = { scale: 1, x: 0, y: 0 };
export const MAX_SCALE = 8;

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

// Panning stops once an edge of the image reaches the fitted centre.
function clampPan(z: Zoom, size: Size): Zoom {
  const mx = (size.width * z.scale) / 2;
  const my = (size.height * z.scale) / 2;
  return { scale: z.scale, x: clamp(z.x, -mx, mx), y: clamp(z.y, -my, my) };
}

// Rescales so the image point under `anchor` stays under it.
export function zoomAt(z: Zoom, scale: number, anchor: Point, size: Size): Zoom {
  const next = clamp(scale, 1, MAX_SCALE);
  if (next < 1.01) return FIT;
  const k = next / z.scale;
  return clampPan(
    { scale: next, x: anchor.x - (anchor.x - z.x) * k, y: anchor.y - (anchor.y - z.y) * k },
    size,
  );
}

export function panBy(z: Zoom, dx: number, dy: number, size: Size): Zoom {
  return clampPan({ scale: z.scale, x: z.x + dx, y: z.y + dy }, size);
}
