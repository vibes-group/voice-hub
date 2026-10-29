// AudioWorklet entry for every wasm denoiser. Vite bundles it with its imports
// into one classic script (`?worker&url`): WebView2 rejects ESM in worklets.

import { FrameAdapter } from './frame-adapter';
import { PROCESSOR_NAME, type DenoiseProcessorOptions } from './types';
import { instantiateEngine, type WasmEngine } from './wasm-engine';

declare class AudioWorkletProcessor {
  readonly port: MessagePort;
}
declare function registerProcessor(
  name: string,
  ctor: new (options: AudioWorkletNodeOptions) => AudioWorkletProcessor,
): void;

class DenoiseProcessor extends AudioWorkletProcessor {
  private engine: WasmEngine | null = null;
  private adapter: FrameAdapter | null = null;
  private destroyed = false;

  constructor(options: AudioWorkletNodeOptions) {
    super();
    this.port.onmessage = (e: MessageEvent<{ type?: string } | null>) => {
      if (e.data?.type === 'destroy') this.destroy();
    };
    void this.init(options.processorOptions as DenoiseProcessorOptions);
  }

  private async init({ wasm, weights }: DenoiseProcessorOptions): Promise<void> {
    try {
      const engine = await instantiateEngine(wasm, weights);
      // Destroyed (e.g. init timeout) while the module compiled.
      if (this.destroyed) {
        engine.destroy();
        return;
      }
      this.engine = engine;
      this.adapter = new FrameAdapter(engine.frameSize, (frame) => engine.process(frame));
      this.port.postMessage({ type: 'ready' });
    } catch (err) {
      this.port.postMessage({
        type: 'error',
        message: err instanceof Error ? err.message : String(err),
      });
    }
  }

  private destroy(): void {
    this.destroyed = true;
    this.engine?.destroy();
    this.engine = null;
    this.adapter = null;
  }

  process(inputs: Float32Array[][], outputs: Float32Array[][]): boolean {
    // false lets the node be collected once it is disconnected.
    if (this.destroyed) return false;
    const input = inputs[0]?.[0];
    const output = outputs[0]?.[0];
    if (!output) return true;
    if (!input) output.fill(0);
    else if (this.adapter) this.adapter.process(input, output);
    else output.set(input);
    return true;
  }
}

registerProcessor(PROCESSOR_NAME, DenoiseProcessor);
