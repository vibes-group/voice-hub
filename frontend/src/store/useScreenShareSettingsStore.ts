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

function loadResolution(): ScreenResolution {
  const raw = localStorage.getItem(KEYS.screenResolution);
  return isScreenResolution(raw) ? raw : DEFAULT_SCREEN_RESOLUTION;
}

function loadFps(): ScreenFps {
  const raw = Number(localStorage.getItem(KEYS.screenFps));
  return isScreenFps(raw) ? raw : DEFAULT_SCREEN_FPS;
}

function loadCodec(): ScreenCodecPref {
  const raw = localStorage.getItem(KEYS.screenCodec);
  return isScreenCodecPref(raw) ? raw : DEFAULT_SCREEN_CODEC;
}

function loadMode(): ScreenMode {
  const raw = localStorage.getItem(KEYS.screenMode);
  return isScreenMode(raw) ? raw : DEFAULT_SCREEN_MODE;
}

function loadShareMode(): ShareMode {
  const raw = localStorage.getItem(KEYS.screenShareMode);
  return isShareMode(raw) ? raw : DEFAULT_SHARE_MODE;
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
  mode: loadMode(),
  shareMode: loadShareMode(),
  codec: loadCodec(),
  customResolution: loadResolution(),
  customFps: loadFps(),
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
