// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { useImageZoom } from './useImageZoom';

let container: HTMLDivElement;
let root: Root;
let resize: (size: { width: number; height: number; x: number; y: number }) => void;
const disconnect = vi.fn();

function Viewer({ id }: { id: string }) {
  const { stageRef, imgProps, zoom, reset, zoomStep } = useImageZoom(id);
  return (
    <div ref={stageRef}>
      <img {...imgProps} alt="test" />
      <button onClick={reset}>reset</button>
      <button onClick={() => zoomStep(1.5)}>zoom</button>
      <output>{zoom.scale}</output>
    </div>
  );
}

function image() {
  return container.querySelector('img')!;
}

function load(width: number, height: number) {
  Object.defineProperties(image(), {
    naturalWidth: { configurable: true, value: width },
    naturalHeight: { configurable: true, value: height },
  });
  act(() => image().dispatchEvent(new Event('load')));
}

function wheel(factor: number) {
  const e = new WheelEvent('wheel', {
    bubbles: true,
    cancelable: true,
    deltaY: -Math.log(factor) / 0.002,
    clientX: 716,
    clientY: 250,
  });
  act(() => image().dispatchEvent(e));
  expect(e.defaultPrevented).toBe(true);
}

beforeEach(() => {
  vi.stubGlobal('IS_REACT_ACT_ENVIRONMENT', true);
  vi.stubGlobal(
    'ResizeObserver',
    class {
      constructor(callback: (entries: { contentRect: object }[]) => void) {
        resize = (contentRect) => callback([{ contentRect }]);
      }
      observe() {}
      disconnect = disconnect;
    },
  );
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root.render(<Viewer id="first" />));
  const stage = image().parentElement!;
  vi.spyOn(stage, 'getBoundingClientRect').mockReturnValue({ left: 100, top: 50 } as DOMRect);
  act(() => resize({ width: 800, height: 600, x: 16, y: 0 }));
  load(1600, 1200);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe('useImageZoom layout', () => {
  it('resizes the image while keeping the cursor anchor stable over successive wheel events', () => {
    expect(image().style.width).toBe('800px');
    expect(image().style.height).toBe('600px');
    expect(image().style.left).toBe('16px');
    expect(image().style.top).toBe('0px');
    for (const factor of [2, 2, 0.5]) {
      wheel(factor);
      const style = image().style;
      const width = parseFloat(style.width);
      const height = parseFloat(style.height);
      expect((716 - 100 - parseFloat(style.left)) / width).toBeCloseTo(0.75);
      expect((250 - 50 - parseFloat(style.top)) / height).toBeCloseTo(1 / 3);
      expect(style.transform).toBe('');
    }
    expect(image().style.width).toBe('1600px');
    expect(image().style.height).toBe('1200px');
  });

  it('centres button zoom and refits when the viewport or image changes', () => {
    act(() => container.querySelectorAll('button')[1].click());
    expect(image().style.width).toBe('1200px');
    expect(image().style.left).toBe('-184px');
    expect(image().style.top).toBe('-150px');
    act(() => container.querySelector('button')!.click());
    act(() => resize({ width: 400, height: 300, x: 16, y: 0 }));
    expect(image().style.width).toBe('400px');
    expect(image().style.height).toBe('300px');
    wheel(2);
    act(() => root.render(<Viewer id="second" />));
    expect(container.querySelector('output')!.textContent).toBe('1');
    expect(image().style.visibility).toBe('hidden');
    load(100, 200);
    expect(image().style.width).toBe('100px');
    expect(image().style.height).toBe('200px');
    expect(image().style.left).toBe('166px');
    expect(image().style.top).toBe('50px');
    expect(image().style.visibility).toBe('');
  });

  it('pans the enlarged image without changing its dimensions', () => {
    wheel(2);
    image().setPointerCapture = vi.fn();
    act(() =>
      image().dispatchEvent(
        new MouseEvent('pointerdown', { bubbles: true, clientX: 716, clientY: 250 }),
      ),
    );
    act(() =>
      image().dispatchEvent(
        new MouseEvent('pointermove', { bubbles: true, clientX: 766, clientY: 275 }),
      ),
    );
    expect(image().style.left).toBe('-534px');
    expect(image().style.top).toBe('-175px');
    expect(image().style.width).toBe('1600px');
    act(() => image().dispatchEvent(new MouseEvent('pointerup', { bubbles: true })));
    act(() =>
      image().dispatchEvent(
        new MouseEvent('pointermove', { bubbles: true, clientX: 800, clientY: 300 }),
      ),
    );
    expect(image().style.left).toBe('-534px');
  });
});
