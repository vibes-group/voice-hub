import type { Denoiser, DenoiserId } from './types';
import { createWasmDenoiser } from './worklet-denoiser';
import rnnoiseWasm from '../wasm/rnnoise.wasm?url';
import fastenhancerWasm from '../wasm/fastenhancer.wasm?url';
import fastenhancerTiny from '../wasm/fastenhancer-t.bin?url';
import fastenhancerBase from '../wasm/fastenhancer-b.bin?url';

export const DENOISERS: Record<DenoiserId, Denoiser> = {
  rnnoise: createWasmDenoiser({ id: 'rnnoise', label: 'RNNoise', wasmUrl: rnnoiseWasm }),
  'fastenhancer-t': createWasmDenoiser({
    id: 'fastenhancer-t',
    label: 'FastEnhancer Tiny',
    wasmUrl: fastenhancerWasm,
    weightsUrl: fastenhancerTiny,
  }),
  // ~3.5x the CPU of Tiny.
  'fastenhancer-b': createWasmDenoiser({
    id: 'fastenhancer-b',
    label: 'FastEnhancer Base',
    wasmUrl: fastenhancerWasm,
    weightsUrl: fastenhancerBase,
  }),
};

export function getDenoiser(id: string): Denoiser | null {
  return id in DENOISERS ? DENOISERS[id as DenoiserId] : null;
}
