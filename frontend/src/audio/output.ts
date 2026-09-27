import { useStore } from '../store/useStore';

// TS's DOM lib types setSinkId on HTMLMediaElement only.
type SinkTarget = { setSinkId?: (sinkId: string) => Promise<void> };

// Voices play through an AudioContext, so the picker needs AudioContext.setSinkId
// (Chromium, incl. the desktop app's WebView2). Elsewhere audio stays on the
// system default and the picker is hidden.
export const SPEAKER_SELECTABLE =
  typeof AudioContext !== 'undefined' && 'setSinkId' in AudioContext.prototype;

// Route an AudioContext or media element to the chosen speaker. A device that
// is gone falls back to the system default and the choice is forgotten.
export async function routeToSpeaker(target: AudioContext | HTMLMediaElement): Promise<void> {
  const sink = target as SinkTarget;
  if (!SPEAKER_SELECTABLE || !sink.setSinkId) return;
  const id = useStore.getState().speakerDeviceId;
  try {
    await sink.setSinkId(id ?? '');
  } catch (err) {
    if (!id) return;
    console.warn('[output] setSinkId failed, using system default:', err);
    useStore.getState().setSpeakerDeviceId(null);
    await sink.setSinkId('').catch(() => undefined);
  }
}
