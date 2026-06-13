export type ScreenResolution = 'source' | '720' | '1080' | '1440' | '2160';
export type ScreenFps = 15 | 30 | 60;
export type ScreenCodecPref = 'av1' | 'vp9';

export type ShareMode = 'sharp' | 'motion';

export const SCREEN_RESOLUTIONS: readonly ScreenResolution[] = [
  'source',
  '720',
  '1080',
  '1440',
  '2160',
] as const;
export const SCREEN_FPS_OPTIONS: readonly ScreenFps[] = [15, 30, 60] as const;
export const SCREEN_CODEC_OPTIONS: readonly ScreenCodecPref[] = ['av1', 'vp9'] as const;
export const SHARE_MODES: readonly ShareMode[] = ['sharp', 'motion'] as const;

export const DEFAULT_SCREEN_RESOLUTION: ScreenResolution = '1440';
export const DEFAULT_SCREEN_FPS: ScreenFps = 60;
export const DEFAULT_SCREEN_CODEC: ScreenCodecPref = 'av1';
export const DEFAULT_SHARE_MODE: ShareMode = 'motion';

export function isScreenResolution(v: unknown): v is ScreenResolution {
  return v === 'source' || v === '720' || v === '1080' || v === '1440' || v === '2160';
}

export function isScreenFps(v: unknown): v is ScreenFps {
  return v === 15 || v === 30 || v === 60;
}

export function isScreenCodecPref(v: unknown): v is ScreenCodecPref {
  return v === 'av1' || v === 'vp9';
}

export function isShareMode(v: unknown): v is ShareMode {
  return v === 'sharp' || v === 'motion';
}

const DIMENSIONS: Record<ScreenResolution, { width: number; height: number }> = {
  source: { width: 3840, height: 2160 },
  '720': { width: 1280, height: 720 },
  '1080': { width: 1920, height: 1080 },
  '1440': { width: 2560, height: 1440 },
  '2160': { width: 3840, height: 2160 },
};

// Bitrate ceilings tuned for AV1/VP9 desktop capture with L1T3 SVC.
// Pre-shareMode-modifier values — sharp scales these down to favour
// cleaner static frames at lower CPU/network cost, motion keeps them
// at full to let motion content render without smear.
const BITRATES: Record<ScreenResolution, Record<ScreenFps, number>> = {
  source: { 15: 8_000_000, 30: 14_000_000, 60: 20_000_000 },
  '720': { 15: 1_000_000, 30: 2_000_000, 60: 3_500_000 },
  '1080': { 15: 2_000_000, 30: 4_000_000, 60: 6_500_000 },
  '1440': { 15: 4_000_000, 30: 8_000_000, 60: 12_000_000 },
  '2160': { 15: 8_000_000, 30: 14_000_000, 60: 20_000_000 },
};

// SHARE_MODE_BITRATE_FACTOR scales the bitrate cap based on the share
// mode. AV1's Screen Content Coding (intraBC + palette) makes static
// text/code/docs encode at a small fraction of the motion budget — we
// halve the cap for sharp and let the encoder coast well below it.
// Motion content (games, videos) gets the full cap.
const SHARE_MODE_BITRATE_FACTOR: Record<ShareMode, number> = {
  sharp: 0.5,
  motion: 1.0,
};

export type ScreenParams = {
  resolution: ScreenResolution;
  fps: ScreenFps;
  width: number;
  height: number;
  maxBitrate: number;
};

export function buildScreenParams(
  resolution: ScreenResolution,
  fps: ScreenFps,
  shareMode: ShareMode = DEFAULT_SHARE_MODE,
): ScreenParams {
  const dim = DIMENSIONS[resolution];
  const base = BITRATES[resolution][fps];
  return {
    resolution,
    fps,
    width: dim.width,
    height: dim.height,
    maxBitrate: Math.round(base * SHARE_MODE_BITRATE_FACTOR[shareMode]),
  };
}

type ScreenPresetId = 'gaming' | 'screenshare';
export type ScreenMode = ScreenPresetId | 'custom';

type ScreenPreset = {
  id: ScreenPresetId;
  label: string;
  resolution: ScreenResolution;
  fps: ScreenFps;
  shareMode: ShareMode;
};

// Codec is intentionally NOT part of presets — it's a one-time per-PC pick
// (depends on hardware encode support) and shouldn't get clobbered when the
// user flips between presets. ShareMode is content-shaped: demos favour
// readable detail, games favour fluid motion. Custom mode lets users override.
export const SCREEN_PRESETS: readonly ScreenPreset[] = [
  { id: 'gaming', label: 'Игры', resolution: '1080', fps: 60, shareMode: 'motion' },
  { id: 'screenshare', label: 'Демонстрация', resolution: 'source', fps: 60, shareMode: 'sharp' },
] as const;

export const DEFAULT_SCREEN_MODE: ScreenMode = 'gaming';

export function isScreenMode(v: unknown): v is ScreenMode {
  return v === 'gaming' || v === 'screenshare' || v === 'custom';
}

export function getPreset(id: ScreenPresetId): ScreenPreset {
  const found = SCREEN_PRESETS.find((p) => p.id === id);
  if (!found) throw new Error(`unknown screen preset: ${id}`);
  return found;
}

// 'text' biases the encoder toward sharp edges and stable text rendering at
// the cost of motion smoothness. 'motion' is the inverse — accept blur on
// frames to keep motion fluid. Mapping mirrors the shareMode contract.
export function shareModeToContentHint(mode: ShareMode): 'text' | 'motion' {
  return mode === 'sharp' ? 'text' : 'motion';
}
