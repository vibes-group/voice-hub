// Wasm denoiser on the shared AudioWorklet: fetch the module (+ weights) once,
// register the worklet per AudioContext, construct a mono node and wait for
// its {ready|error} reply.

import workletUrl from './processor.worklet.ts?worker&url';
import {
  PROCESSOR_NAME,
  type DenoiseProcessorOptions,
  type Denoiser,
  type DenoiserId,
  type DenoiserNode,
} from './types';

const SAMPLE_RATE = 48000;
const INIT_TIMEOUT_MS = 3000;

type WasmDenoiserConfig = {
  id: DenoiserId;
  label: string;
  wasmUrl: string;
  weightsUrl?: string;
};

// addModule is per AudioContext; every denoiser shares the one processor.
const registered = new WeakMap<BaseAudioContext, Promise<void>>();

function ensureRegistered(ctx: BaseAudioContext): Promise<void> {
  let p = registered.get(ctx);
  if (p) return p;
  p = ctx.audioWorklet.addModule(workletUrl).catch((err: unknown) => {
    registered.delete(ctx);
    throw err;
  });
  registered.set(ctx, p);
  return p;
}

async function fetchBytes(url: string): Promise<ArrayBuffer> {
  const r = await fetch(url);
  if (!r.ok) throw new Error(`${url}: HTTP ${r.status}`);
  return r.arrayBuffer();
}

export function createWasmDenoiser(cfg: WasmDenoiserConfig): Denoiser {
  const tag = `[${cfg.id}]`;

  let assets: DenoiseProcessorOptions | null = null;
  let loading: Promise<DenoiseProcessorOptions> | null = null;

  function load(): Promise<DenoiseProcessorOptions> {
    loading ??= Promise.all([
      fetchBytes(cfg.wasmUrl),
      cfg.weightsUrl ? fetchBytes(cfg.weightsUrl) : null,
    ]).then(
      ([wasm, weights]) => (assets = { wasm, weights }),
      (err: unknown) => {
        loading = null;
        throw err;
      },
    );
    return loading;
  }

  async function create(ctx: AudioContext): Promise<DenoiserNode | null> {
    if (ctx.sampleRate !== SAMPLE_RATE) {
      console.warn(`${tag} disabled: sampleRate=${ctx.sampleRate} (need ${SAMPLE_RATE})`);
      return null;
    }

    let processorOptions: DenoiseProcessorOptions;
    try {
      processorOptions = await load();
      await ensureRegistered(ctx);
    } catch (err) {
      console.error(`${tag} load failed:`, err);
      return null;
    }

    let node: AudioWorkletNode;
    try {
      // Explicit mono: a stereo mic is downmixed instead of dropping a channel.
      node = new AudioWorkletNode(ctx, PROCESSOR_NAME, {
        numberOfInputs: 1,
        numberOfOutputs: 1,
        outputChannelCount: [1],
        channelCount: 1,
        channelCountMode: 'explicit',
        channelInterpretation: 'speakers',
        processorOptions,
      });
    } catch (err) {
      console.error(`${tag} node construction failed:`, err);
      return null;
    }

    const dispose = () => {
      node.port.postMessage({ type: 'destroy' });
      node.disconnect();
    };

    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      await new Promise<void>((resolve, reject) => {
        timer = setTimeout(() => reject(new Error('init timeout')), INIT_TIMEOUT_MS);
        node.port.onmessage = (e: MessageEvent<{ type?: string; message?: string } | null>) => {
          if (e.data?.type === 'ready') resolve();
          else if (e.data?.type === 'error') reject(new Error(e.data.message ?? 'init failed'));
        };
      });
    } catch (err) {
      console.warn(`${tag} init failed:`, err);
      dispose();
      return null;
    } finally {
      clearTimeout(timer);
      node.port.onmessage = null;
    }

    return { input: node, output: node, dispose };
  }

  return {
    id: cfg.id,
    label: cfg.label,
    preload: () => load().then(() => undefined),
    isReady: () => assets !== null,
    create,
  };
}
