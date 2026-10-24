import type { Denoiser, DenoiserId } from './types';
import { rnnoise } from './rnnoise';

export const DENOISERS: Record<DenoiserId, Denoiser> = {
  rnnoise,
};

export function getDenoiser(id: string): Denoiser | null {
  return id in DENOISERS ? DENOISERS[id as DenoiserId] : null;
}
