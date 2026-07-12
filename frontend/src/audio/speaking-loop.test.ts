// @vitest-environment jsdom

import { afterEach, describe, expect, it, vi } from 'vitest';
import { createSpeakingLoop } from './speaking-loop';

describe('createSpeakingLoop', () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it('does not read analyser buffers while the document is hidden', () => {
    vi.useFakeTimers();
    let visibility: DocumentVisibilityState = 'visible';
    vi.spyOn(document, 'visibilityState', 'get').mockImplementation(() => visibility);

    const getFloatTimeDomainData = vi.fn((data: Float32Array) => data.fill(0));
    const loop = createSpeakingLoop();
    loop.register('peer', {
      analyser: { getFloatTimeDomainData } as unknown as AnalyserNode,
      data: new Float32Array(16) as Float32Array<ArrayBuffer>,
      onChange: vi.fn(),
    });

    vi.advanceTimersByTime(150);
    expect(getFloatTimeDomainData).toHaveBeenCalledTimes(3);

    visibility = 'hidden';
    vi.advanceTimersByTime(1_000);
    expect(getFloatTimeDomainData).toHaveBeenCalledTimes(3);

    visibility = 'visible';
    vi.advanceTimersByTime(50);
    expect(getFloatTimeDomainData).toHaveBeenCalledTimes(4);
    loop.unregister('peer');
  });
});
