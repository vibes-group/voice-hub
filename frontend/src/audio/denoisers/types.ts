export type DenoiserId = 'rnnoise';

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
