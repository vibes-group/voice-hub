// RNNoise — Shiguredo vendor + our processor pre-bundled at build time by
// scripts/bundle-rnnoise.mjs into a single self-contained worklet file.
// Loading is just addModule + construct, no init payload.

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
  preloadAssets: async () => {
    // HTTP cache warmup — addModule resolves from cache without a second
    // round trip when the user actually starts the engine.
    const r = await fetch(WORKLET_URL, { cache: 'force-cache' });
    if (!r.ok) throw new Error(`rnnoise worklet fetch ${r.status}`);
    return { moduleUrl: WORKLET_URL };
  },
});
