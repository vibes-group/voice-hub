// AudioWorklet-backed denoiser: load the module once, register it per
// AudioContext, construct a mono node and wait for its {ready|error} reply.

import type { Denoiser, DenoiserId, DenoiserNode } from './denoisers/types';

const SAMPLE_RATE = 48000;
const INIT_TIMEOUT_MS = 3000;

type WorkletDenoiserConfig = {
  id: DenoiserId;
  label: string;
  processorName: string;
  // Fetches the worklet module; resolves to the URL for addModule().
  loadModule: () => Promise<string>;
};

export function createWorkletDenoiser(cfg: WorkletDenoiserConfig): Denoiser {
  const tag = `[${cfg.id}]`;

  let moduleUrl: string | null = null;
  let loading: Promise<string> | null = null;

  function load(): Promise<string> {
    if (!loading) {
      loading = cfg.loadModule().then(
        (url) => (moduleUrl = url),
        (err: unknown) => {
          loading = null;
          throw err;
        },
      );
    }
    return loading;
  }

  // addModule is per AudioContext; every node in a context shares one registration.
  const registered = new WeakMap<AudioContext, Promise<void>>();

  function ensureRegistered(ctx: AudioContext, url: string): Promise<void> {
    let p = registered.get(ctx);
    if (p) return p;
    p = ctx.audioWorklet.addModule(url).catch((err: unknown) => {
      registered.delete(ctx);
      throw err;
    });
    registered.set(ctx, p);
    return p;
  }

  async function create(ctx: AudioContext): Promise<DenoiserNode | null> {
    if (ctx.sampleRate !== SAMPLE_RATE) {
      console.warn(`${tag} disabled: sampleRate=${ctx.sampleRate} (need ${SAMPLE_RATE})`);
      return null;
    }

    let url: string;
    try {
      url = await load();
    } catch (err) {
      console.error(`${tag} preload failed:`, err);
      return null;
    }

    try {
      await ensureRegistered(ctx, url);
    } catch (err) {
      console.error(`${tag} addModule failed:`, err);
      return null;
    }

    let node: AudioWorkletNode;
    try {
      node = new AudioWorkletNode(ctx, cfg.processorName, {
        numberOfInputs: 1,
        numberOfOutputs: 1,
        channelCount: 1,
      });
    } catch (err) {
      console.error(`${tag} node construction failed:`, err);
      return null;
    }

    const ready = new Promise<void>((resolve, reject) => {
      const onMessage = (e: MessageEvent) => {
        const data = e.data as { type?: string; message?: string } | null;
        if (data?.type === 'ready') {
          node.port.removeEventListener('message', onMessage);
          resolve();
        } else if (data?.type === 'error') {
          node.port.removeEventListener('message', onMessage);
          reject(new Error(data.message ?? `${cfg.id} init failed`));
        }
      };
      node.port.addEventListener('message', onMessage);
      node.port.start();
    });

    const timeout = new Promise<void>((_, reject) => {
      setTimeout(() => reject(new Error(`${cfg.id} init timeout`)), INIT_TIMEOUT_MS);
    });

    try {
      await Promise.race([ready, timeout]);
    } catch (err) {
      console.warn(`${tag} init failed:`, err);
      node.disconnect();
      return null;
    }

    return {
      input: node,
      output: node,
      dispose() {
        node.port.postMessage({ type: 'destroy' });
        node.disconnect();
      },
    };
  }

  return {
    id: cfg.id,
    label: cfg.label,
    preload: () => load().then(() => undefined),
    isReady: () => moduleUrl !== null,
    create,
  };
}
