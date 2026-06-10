// Reactive state for screen-share UI.
//
// Lives in its own Zustand store (not useStore.ts) so the existing participants
// / chat / audio state isn't touched by every screen-share UI re-render and
// vice versa. Audio nodes follow the same pattern (kept imperative outside the
// store); MediaStreams here are exceptions — the focused video tile reads
// `focusedStream` directly to feed `<video>.srcObject`.

import { create } from 'zustand';

import type { ScreenVideoCodec } from '../sfu/protocol';

/** One active screen share announced by the server. */
export type ScreenShare = {
  publisherId: string;
  hasSystemAudio: boolean;
  videoCodec?: ScreenVideoCodec;
};

/** Local publisher state — independent from the shares map (which lists what's
 * happening in the room, not what we're publishing). */
export type MyShareStatus = 'idle' | 'starting' | 'publishing' | 'stopping';

export type ScreenShareState = {
  shares: Map<string, ScreenShare>;
  upsertShare: (share: ScreenShare) => void;
  removeShare: (publisherId: string) => void;
  clearShares: () => void;

  focusedId: string | null;
  focusedStream: MediaStream | null;
  focusedAudioStream: MediaStream | null;
  setFocused: (publisherId: string | null) => void;
  attachFocusedVideo: (publisherId: string, stream: MediaStream) => void;
  attachFocusedAudio: (publisherId: string, stream: MediaStream) => void;

  myStatus: MyShareStatus;
  setMyStatus: (s: MyShareStatus) => void;

  myStream: MediaStream | null;
  myVideoCodec: ScreenVideoCodec | null;
  setMyStream: (stream: MediaStream | null, codec?: ScreenVideoCodec | null) => void;
};

export const useScreenShareStore = create<ScreenShareState>((set) => ({
  shares: new Map(),
  upsertShare: (share) =>
    set((s) => {
      const m = new Map(s.shares);
      m.set(share.publisherId, share);
      return { shares: m };
    }),
  removeShare: (publisherId) =>
    set((s) => {
      if (!s.shares.has(publisherId)) return {};
      const m = new Map(s.shares);
      m.delete(publisherId);
      // focusedId / streams intentionally NOT cleared here — the focused
      // overlay shows a "Стрим завершён" placeholder briefly before the
      // grace-close timer in ScreenShareFocused dismisses the view. The
      // <video> keeps its last frame because the MediaStream reference
      // is still attached.
      return { shares: m };
    }),
  clearShares: () =>
    set({
      shares: new Map(),
      focusedId: null,
      focusedStream: null,
      focusedAudioStream: null,
    }),

  focusedId: null,
  focusedStream: null,
  focusedAudioStream: null,
  setFocused: (publisherId) =>
    set((s) => {
      // Switching focus invalidates any previously-attached stream — the
      // new subscriber's tracks haven't arrived yet. UI shows a loading
      // placeholder until attachFocusedVideo fires.
      if (publisherId === s.focusedId) return {};
      return {
        focusedId: publisherId,
        focusedStream: null,
        focusedAudioStream: null,
      };
    }),
  attachFocusedVideo: (publisherId, stream) =>
    set((s) => {
      if (s.focusedId !== publisherId) return {};
      return { focusedStream: stream };
    }),
  attachFocusedAudio: (publisherId, stream) =>
    set((s) => {
      if (s.focusedId !== publisherId) return {};
      return { focusedAudioStream: stream };
    }),

  myStatus: 'idle',
  setMyStatus: (s) => set({ myStatus: s }),

  myStream: null,
  myVideoCodec: null,
  setMyStream: (stream, codec = null) => set({ myStream: stream, myVideoCodec: codec }),
}));
