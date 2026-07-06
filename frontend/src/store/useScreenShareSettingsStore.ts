import { create } from 'zustand';
import { useShallow } from 'zustand/react/shallow';
import {
  DEFAULT_SCREEN_CODEC,
  DEFAULT_SCREEN_FPS,
  DEFAULT_SCREEN_MODE,
  DEFAULT_SCREEN_RESOLUTION,
  DEFAULT_SHARE_MODE,
  buildScreenParams,
  getPreset,
  isScreenCodecPref,
  isScreenFps,
  isScreenMode,
  isScreenResolution,
  isShareMode,
  shareModeToContentHint,
  type ScreenCodecPref,
  type ScreenFps,
  type ScreenMode,
  type ScreenParams,
  type ScreenResolution,
  type ShareMode,
} from '../screenshare/params';
import { KEYS } from '../utils/storage';

// Read a persisted setting, validating it against its type guard and falling
// back to the default. parse lets fps coerce the stored string to a number.
function loadValidated<T>(
  key: string,
  isValid: (v: unknown) => v is T,
  fallback: T,
  parse?: (raw: string | null) => unknown,
): T {
  const stored = localStorage.getItem(key);
  const raw = parse ? parse(stored) : stored;
  return isValid(raw) ? raw : fallback;
}

type State = {
  mode: ScreenMode;
  shareMode: ShareMode;
  codec: ScreenCodecPref;
  customResolution: ScreenResolution;
  customFps: ScreenFps;
  setMode: (m: ScreenMode) => void;
  setShareMode: (m: ShareMode) => void;
  setCodec: (c: ScreenCodecPref) => void;
  setResolution: (r: ScreenResolution) => void;
  setFps: (f: ScreenFps) => void;
};

export const useScreenShareSettingsStore = create<State>((set) => ({
  mode: loadValidated(KEYS.screenMode, isScreenMode, DEFAULT_SCREEN_MODE),
  shareMode: loadValidated(KEYS.screenShareMode, isShareMode, DEFAULT_SHARE_MODE),
  codec: loadValidated(KEYS.screenCodec, isScreenCodecPref, DEFAULT_SCREEN_CODEC),
  customResolution: loadValidated(
    KEYS.screenResolution,
    isScreenResolution,
    DEFAULT_SCREEN_RESOLUTION,
  ),
  customFps: loadValidated(KEYS.screenFps, isScreenFps, DEFAULT_SCREEN_FPS, Number),
  setMode: (m) => {
    localStorage.setItem(KEYS.screenMode, m);
    set({ mode: m });
  },
  setShareMode: (m) => {
    localStorage.setItem(KEYS.screenShareMode, m);
    set({ shareMode: m });
  },
  setCodec: (c) => {
    localStorage.setItem(KEYS.screenCodec, c);
    set({ codec: c });
  },
  setResolution: (r) => {
    localStorage.setItem(KEYS.screenResolution, r);
    set({ customResolution: r });
  },
  setFps: (f) => {
    localStorage.setItem(KEYS.screenFps, String(f));
    set({ customFps: f });
  },
}));

type EffectiveSettings = {
  resolution: ScreenResolution;
  fps: ScreenFps;
  codec: ScreenCodecPref;
  shareMode: ShareMode;
};

function pickEffectiveSettings(
  s: Pick<State, 'mode' | 'codec' | 'shareMode' | 'customResolution' | 'customFps'>,
): EffectiveSettings {
  if (s.mode === 'custom') {
    return {
      resolution: s.customResolution,
      fps: s.customFps,
      codec: s.codec,
      shareMode: s.shareMode,
    };
  }
  const p = getPreset(s.mode);
  return { resolution: p.resolution, fps: p.fps, codec: s.codec, shareMode: p.shareMode };
}

function getEffectiveSettings(): EffectiveSettings {
  return pickEffectiveSettings(useScreenShareSettingsStore.getState());
}

export function useEffectiveScreenSettings(): EffectiveSettings {
  return useScreenShareSettingsStore(useShallow(pickEffectiveSettings));
}

export function getCurrentScreenParams(): ScreenParams {
  const { resolution, fps, shareMode } = getEffectiveSettings();
  return buildScreenParams(resolution, fps, shareMode);
}

export function getCurrentScreenCodecPref(): ScreenCodecPref {
  return getEffectiveSettings().codec;
}

export function getCurrentShareMode(): ShareMode {
  return getEffectiveSettings().shareMode;
}

export function getCurrentScreenContentHint(): 'text' | 'motion' {
  return shareModeToContentHint(getCurrentShareMode());
}
