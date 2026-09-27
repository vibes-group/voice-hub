// @vitest-environment jsdom
import { beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';

class FakeAudioContext {
  setSinkId(_sinkId: string): Promise<void> {
    return Promise.resolve();
  }
}

// SPEAKER_SELECTABLE is read at import time, so stub before importing.
beforeAll(() => {
  vi.stubGlobal('AudioContext', FakeAudioContext);
});

async function load() {
  const { routeToSpeaker, SPEAKER_SELECTABLE } = await import('./output');
  const { useStore } = await import('../store/useStore');
  return { routeToSpeaker, SPEAKER_SELECTABLE, useStore };
}

beforeEach(() => {
  localStorage.clear();
});

describe('routeToSpeaker', () => {
  it('routes to the chosen speaker', async () => {
    const { routeToSpeaker, SPEAKER_SELECTABLE, useStore } = await load();
    expect(SPEAKER_SELECTABLE).toBe(true);
    useStore.getState().setSpeakerDeviceId('usb-headset');
    const ctx = new FakeAudioContext();
    const spy = vi.spyOn(ctx, 'setSinkId');

    await routeToSpeaker(ctx as unknown as AudioContext);

    expect(spy).toHaveBeenCalledWith('usb-headset');
  });

  it('uses the system default when nothing is chosen', async () => {
    const { routeToSpeaker, useStore } = await load();
    useStore.getState().setSpeakerDeviceId(null);
    const ctx = new FakeAudioContext();
    const spy = vi.spyOn(ctx, 'setSinkId');

    await routeToSpeaker(ctx as unknown as AudioContext);

    expect(spy).toHaveBeenCalledWith('');
  });

  it('falls back to the default and forgets a device that is gone', async () => {
    const { routeToSpeaker, useStore } = await load();
    useStore.getState().setSpeakerDeviceId('unplugged');
    const ctx = new FakeAudioContext();
    const spy = vi
      .spyOn(ctx, 'setSinkId')
      .mockRejectedValueOnce(new DOMException('gone', 'NotFoundError'));
    vi.spyOn(console, 'warn').mockImplementation(() => undefined);

    await routeToSpeaker(ctx as unknown as AudioContext);

    expect(spy).toHaveBeenLastCalledWith('');
    expect(useStore.getState().speakerDeviceId).toBeNull();
    expect(localStorage.getItem('voice-hub.speaker-device-id')).toBeNull();
  });
});
