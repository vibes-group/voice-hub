export type DenoiserId = 'rnnoise' | 'fastenhancer-t' | 'fastenhancer-b';

export type DenoiserNode = {
  input: AudioNode;
  output: AudioNode;
  dispose(): void;
};

export type Denoiser = {
  id: DenoiserId;
  label: string;
  preload(): Promise<void>;
  isReady(): boolean;
  // null when the engine can't start; the caller sends without denoising.
  create(ctx: AudioContext): Promise<DenoiserNode | null>;
};

// Shared by processor.worklet.ts and the main thread.
export const PROCESSOR_NAME = 'denoise-processor';

export type DenoiseProcessorOptions = {
  wasm: ArrayBuffer;
  weights: ArrayBuffer | null;
};
