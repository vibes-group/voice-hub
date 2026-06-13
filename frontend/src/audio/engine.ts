// Pure engine helpers — no React dependency.
// Lives in audio/ so non-hook callers can import without pulling a hook module.

import type { EngineKind } from '../types';
import { DENOISERS, getDenoiser } from './denoisers/registry';
import type { DenoiserId } from './denoisers/types';

export type ActiveEngineKind = Exclude<EngineKind, 'off'>;

const CAPTURE_ENGINE_LABELS = {
  browser: 'Браузерное',
} as const satisfies Record<Exclude<EngineKind, 'off' | DenoiserId>, string>;

const CAPTURE_ENGINE_IDS = Object.keys(CAPTURE_ENGINE_LABELS) as Exclude<
  EngineKind,
  'off' | DenoiserId
>[];

export type EngineOption = {
  value: ActiveEngineKind;
  label: string;
};

export const ENGINE_OPTIONS: EngineOption[] = [
  ...CAPTURE_ENGINE_IDS.map((id) => ({ value: id, label: CAPTURE_ENGINE_LABELS[id] })),
  ...Object.values(DENOISERS).map((d) => ({ value: d.id, label: d.label })),
];

export const ENGINE_IDS = ENGINE_OPTIONS.map((opt) => opt.value);

export function isCaptureEngine(engine: EngineKind): boolean {
  return engine in CAPTURE_ENGINE_LABELS;
}

export function formatEngine(engine: string): string {
  if (engine === 'off') return 'Выкл.';
  if (engine in CAPTURE_ENGINE_LABELS) {
    return CAPTURE_ENGINE_LABELS[engine as keyof typeof CAPTURE_ENGINE_LABELS];
  }
  return getDenoiser(engine)?.label ?? engine;
}

export function preloadEngine(engine: EngineKind): Promise<void> {
  if (engine === 'off' || isCaptureEngine(engine)) return Promise.resolve();
  const d = getDenoiser(engine);
  return d ? d.preload() : Promise.resolve();
}

export function isEngineReady(engine: EngineKind): boolean {
  if (engine === 'off' || isCaptureEngine(engine)) return true;
  const d = getDenoiser(engine);
  // Unknown engine id (shouldn't happen — type-checked) is treated as
  // not-ready so a future bad value can't masquerade as ready.
  return d ? d.isReady() : false;
}
