// Re-blocks 128-sample render quanta into the denoiser's frame size and back.
//
// Output starts after `primeQuanta` silent quanta: the smallest constant delay
// at which the bursty frame cadence never underruns. A frame completes at most
// F - gcd(F, 128) samples after a quantum boundary would need it, so the delay
// is ceil((F - gcd) / 128) quanta: 4 for RNNoise (480), 3 for FastEnhancer (512).

export const QUANTUM = 128;

function gcd(a: number, b: number): number {
  return b === 0 ? a : gcd(b, a % b);
}

export class FrameAdapter {
  readonly primeQuanta: number;
  private readonly frame: Float32Array;
  private filled = 0;
  // Ring of processed samples awaiting output.
  private readonly ring: Float32Array;
  private readPos = 0;
  private pending = 0;
  private quanta = 0;

  constructor(
    readonly frameSize: number,
    // Denoises `frame` in place.
    private readonly processFrame: (frame: Float32Array) => void,
  ) {
    this.primeQuanta = Math.ceil((frameSize - gcd(frameSize, QUANTUM)) / QUANTUM);
    this.frame = new Float32Array(frameSize);
    this.ring = new Float32Array(frameSize + 2 * QUANTUM);
  }

  process(input: Float32Array, output: Float32Array): void {
    let off = 0;
    while (off < QUANTUM) {
      const n = Math.min(QUANTUM - off, this.frameSize - this.filled);
      this.frame.set(input.subarray(off, off + n), this.filled);
      this.filled += n;
      off += n;
      if (this.filled === this.frameSize) {
        this.processFrame(this.frame);
        this.push(this.frame);
        this.filled = 0;
      }
    }

    if (this.quanta < this.primeQuanta) {
      this.quanta++;
      output.fill(0);
      return;
    }
    this.pop(output);
  }

  private push(src: Float32Array): void {
    const cap = this.ring.length;
    const writePos = (this.readPos + this.pending) % cap;
    const head = Math.min(src.length, cap - writePos);
    this.ring.set(src.subarray(0, head), writePos);
    this.ring.set(src.subarray(head), 0);
    this.pending += src.length;
  }

  private pop(dst: Float32Array): void {
    // Unreachable with the prime delay above; silence beats a stale buffer.
    if (this.pending < dst.length) {
      dst.fill(0);
      return;
    }
    const cap = this.ring.length;
    const head = Math.min(dst.length, cap - this.readPos);
    dst.set(this.ring.subarray(this.readPos, this.readPos + head));
    dst.set(this.ring.subarray(0, dst.length - head), head);
    this.readPos = (this.readPos + dst.length) % cap;
    this.pending -= dst.length;
  }
}
