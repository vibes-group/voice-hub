// RNNoise — Shiguredo vendor + our processor pre-bundled at build time by
// scripts/bundle-rnnoise.mjs into a single self-contained worklet file.

import { createWorkletDenoiser } from '../worklet-denoiser';

// Caddy serves /vendor/* immutable, so bump this when the bundled worklet
// content changes (processor name, ring sizing, ABI). Stale cached copies
// register the wrong processor name and silently time out on init.
const WORKLET_VERSION = '5';
const WORKLET_URL = `/vendor/rnnoise/worklet.js?v=${WORKLET_VERSION}`;

export const rnnoise = createWorkletDenoiser({
  id: 'rnnoise',
  label: 'RNNoise',
  processorName: 'rnnoise-processor',
  loadModule: async () => {
    // Warms the HTTP cache so addModule doesn't wait on the network later.
    const r = await fetch(WORKLET_URL, { cache: 'force-cache' });
    if (!r.ok) throw new Error(`rnnoise worklet fetch ${r.status}`);
    return WORKLET_URL;
  },
});
