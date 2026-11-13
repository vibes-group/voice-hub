import { useCallback, useEffect, useRef, useState } from 'react';
import type { MouseEvent as ReactMouseEvent, PointerEvent as ReactPointerEvent } from 'react';
import { FIT, panBy, zoomAt, type Zoom } from '../utils/zoom';

const WHEEL_SPEED = 0.002;
// Trackpad pinch arrives as ctrl+wheel with small deltas.
const PINCH_SPEED = 0.01;

function sizeOf(img: HTMLImageElement) {
  return { width: img.offsetWidth, height: img.offsetHeight };
}

// Client point relative to the fitted (untransformed) image centre.
function fromCentre(img: HTMLImageElement, clientX: number, clientY: number) {
  const parent = img.offsetParent!.getBoundingClientRect();
  return {
    x: clientX - (parent.left + img.offsetLeft + img.offsetWidth / 2),
    y: clientY - (parent.top + img.offsetTop + img.offsetHeight / 2),
  };
}

// Wheel/button zoom and drag-to-pan for the lightbox image. `stageRef` goes on
// the always-mounted container (it takes the wheel), `imgRef` on the <img>.
// Zoom resets when `id` changes.
export function useImageZoom(id: string) {
  const [state, setState] = useState({ id, ...FIT });
  const zoom: Zoom = state.id === id ? state : FIT;
  const stageRef = useRef<HTMLDivElement>(null);
  const imgRef = useRef<HTMLImageElement>(null);
  const drag = useRef<{ x: number; y: number } | null>(null);
  const dragged = useRef(false);

  const update = useCallback(
    (fn: (z: Zoom, img: HTMLImageElement) => Zoom) => {
      const img = imgRef.current;
      if (!img) return;
      setState((prev) => ({ id, ...fn(prev.id === id ? prev : FIT, img) }));
    },
    [id],
  );

  const zoomAround = useCallback(
    (factor: number, clientX: number, clientY: number) =>
      update((z, img) =>
        zoomAt(z, z.scale * factor, fromCentre(img, clientX, clientY), sizeOf(img)),
      ),
    [update],
  );

  useEffect(() => {
    const stage = stageRef.current;
    if (!stage) return;
    function onWheel(e: WheelEvent) {
      if (!imgRef.current) return;
      // Also keeps ctrl+wheel from zooming the whole page.
      e.preventDefault();
      const px = e.deltaY * (e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? 800 : 1);
      zoomAround(Math.exp(-px * (e.ctrlKey ? PINCH_SPEED : WHEEL_SPEED)), e.clientX, e.clientY);
    }
    stage.addEventListener('wheel', onWheel, { passive: false });
    return () => stage.removeEventListener('wheel', onWheel);
  }, [zoomAround]);

  const zoomStep = useCallback(
    (factor: number) => {
      const r = stageRef.current?.getBoundingClientRect();
      if (r) zoomAround(factor, r.left + r.width / 2, r.top + r.height / 2);
    },
    [zoomAround],
  );

  const reset = useCallback(() => setState({ id, ...FIT }), [id]);

  // True once after a pan, so the click that ends it doesn't close the viewer.
  const wasDragged = useCallback(() => {
    const was = dragged.current;
    dragged.current = false;
    return was;
  }, []);

  const zoomed = zoom.scale > 1;
  const endDrag = () => {
    drag.current = null;
  };
  const imgProps = {
    ref: imgRef,
    draggable: false,
    style: {
      transform: `translate(${zoom.x}px, ${zoom.y}px) scale(${zoom.scale})`,
      touchAction: zoomed ? 'none' : undefined,
    },
    onPointerDown(e: ReactPointerEvent<HTMLImageElement>) {
      dragged.current = false;
      if (!zoomed || e.button !== 0) return;
      e.currentTarget.setPointerCapture(e.pointerId);
      drag.current = { x: e.clientX, y: e.clientY };
    },
    onPointerMove(e: ReactPointerEvent<HTMLImageElement>) {
      const d = drag.current;
      if (!d) return;
      const dx = e.clientX - d.x;
      const dy = e.clientY - d.y;
      if (!dx && !dy) return;
      drag.current = { x: e.clientX, y: e.clientY };
      dragged.current = true;
      update((z, img) => panBy(z, dx, dy, sizeOf(img)));
    },
    onPointerUp: endDrag,
    onPointerCancel: endDrag,
    onClick(e: ReactMouseEvent) {
      e.stopPropagation();
      dragged.current = false;
    },
    onDoubleClick(e: ReactMouseEvent) {
      if (zoomed) reset();
      else zoomAround(2.5, e.clientX, e.clientY);
    },
  };

  return { zoom, stageRef, imgProps, zoomStep, reset, wasDragged };
}
