import { useCallback, useEffect, useRef, useState } from 'react';
import type {
  CSSProperties,
  MouseEvent as ReactMouseEvent,
  PointerEvent as ReactPointerEvent,
  SyntheticEvent,
} from 'react';
import { FIT, panBy, zoomAt, type Size, type Zoom } from '../utils/zoom';

const WHEEL_SPEED = 0.002;
// Trackpad pinch arrives as ctrl+wheel with small deltas.
const PINCH_SPEED = 0.01;

// Wheel/button zoom and drag-to-pan for the lightbox image. `stageRef` goes on
// the always-mounted container (it takes the wheel), `imgRef` on the <img>.
// Zoom resets when `id` changes.
export function useImageZoom(id: string) {
  const [state, setState] = useState({ id, ...FIT });
  const zoom: Zoom = state.id === id ? state : FIT;
  const stageRef = useRef<HTMLDivElement>(null);
  const imgRef = useRef<HTMLImageElement>(null);
  const [viewport, setViewport] = useState({ width: 0, height: 0, x: 0, y: 0 });
  const [natural, setNatural] = useState({ id, width: 0, height: 0 });
  const drag = useRef<{ x: number; y: number } | null>(null);
  const dragged = useRef(false);

  useEffect(() => {
    const stage = stageRef.current;
    if (!stage) return;
    const observer = new ResizeObserver(([entry]) => {
      const { width, height, x, y } = entry.contentRect;
      setViewport({ width, height, x, y });
    });
    observer.observe(stage);
    return () => observer.disconnect();
  }, []);

  // Keep the fitted size independent of the enlarged image's layout.
  const fitScale =
    natural.id === id && natural.width && natural.height
      ? Math.min(1, viewport.width / natural.width, viewport.height / natural.height)
      : 0;
  const width = natural.width * fitScale;
  const height = natural.height * fitScale;

  const update = useCallback(
    (fn: (z: Zoom, size: Size) => Zoom) => {
      if (!imgRef.current || !width || !height) return;
      setState((prev) => ({ id, ...fn(prev.id === id ? prev : FIT, { width, height }) }));
    },
    [id, width, height],
  );

  const zoomAround = useCallback(
    (factor: number, clientX: number, clientY: number) => {
      const stage = stageRef.current;
      if (!stage) return;
      const r = stage.getBoundingClientRect();
      const anchor = {
        x: clientX - (r.left + stage.clientLeft + viewport.x + viewport.width / 2),
        y: clientY - (r.top + stage.clientTop + viewport.y + viewport.height / 2),
      };
      update((z, size) => zoomAt(z, z.scale * factor, anchor, size));
    },
    [update, viewport],
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
      const stage = stageRef.current;
      if (r && stage)
        zoomAround(
          factor,
          r.left + stage.clientLeft + viewport.x + viewport.width / 2,
          r.top + stage.clientTop + viewport.y + viewport.height / 2,
        );
    },
    [zoomAround, viewport],
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
  // Resize the raster itself instead of scaling a composited texture.
  const renderedWidth = Math.round(width * zoom.scale);
  const renderedHeight = Math.round(height * zoom.scale);
  const style: CSSProperties = {
    position: 'absolute',
    width: renderedWidth,
    height: renderedHeight,
    left: Math.round(viewport.x + (viewport.width - renderedWidth) / 2 + zoom.x),
    top: Math.round(viewport.y + (viewport.height - renderedHeight) / 2 + zoom.y),
    visibility: fitScale ? undefined : 'hidden',
    touchAction: zoomed ? 'none' : undefined,
  };
  const imgProps = {
    ref: imgRef,
    draggable: false,
    style,
    onLoad(e: SyntheticEvent<HTMLImageElement>) {
      const img = e.currentTarget;
      setNatural({ id, width: img.naturalWidth, height: img.naturalHeight });
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
      update((z, size) => panBy(z, dx, dy, size));
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
