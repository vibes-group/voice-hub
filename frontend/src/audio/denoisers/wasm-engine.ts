// Instantiates a denoiser wasm built by wasm/build.sh (ABI: wasm/denoiser.h).
// Runs inside the AudioWorklet, and in node for tests.

type Exports = {
  memory: WebAssembly.Memory;
  _initialize?: () => void;
  malloc(size: number): number;
  free(ptr: number): void;
  dn_create(weights: number, len: number): number;
  dn_frame_size(state: number): number;
  dn_process(state: number, input: number, output: number): number;
  dn_destroy(state: number): void;
};

export type WasmEngine = {
  frameSize: number;
  // Denoises `frame` (frameSize samples in [-1, 1]) in place.
  process(frame: Float32Array): void;
  destroy(): void;
};

export async function instantiateEngine(
  wasm: BufferSource,
  weights: ArrayBuffer | null,
): Promise<WasmEngine> {
  const module = await WebAssembly.compile(wasm);
  // Standalone builds only import emscripten's memory-growth hook.
  const imports: Record<string, Record<string, () => void>> = {};
  for (const imp of WebAssembly.Module.imports(module)) {
    (imports[imp.module] ??= {})[imp.name] = () => undefined;
  }
  const e = (await WebAssembly.instantiate(module, imports)).exports as unknown as Exports;
  e._initialize?.();

  const len = weights?.byteLength ?? 0;
  const ptr = weights ? e.malloc(len) : 0;
  if (weights) new Uint8Array(e.memory.buffer, ptr, len).set(new Uint8Array(weights));
  const state = e.dn_create(ptr, len);
  if (ptr) e.free(ptr);
  if (!state) throw new Error('dn_create failed');

  const frameSize = e.dn_frame_size(state);
  const buf = e.malloc(frameSize * 4);
  // dn_process never allocates, so memory cannot grow and detach this view.
  const heap = new Float32Array(e.memory.buffer, buf, frameSize);

  return {
    frameSize,
    process(frame) {
      heap.set(frame);
      e.dn_process(state, buf, buf);
      frame.set(heap);
    },
    destroy() {
      e.dn_destroy(state);
      e.free(buf);
    },
  };
}
