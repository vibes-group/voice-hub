// RNNoise AudioWorkletProcessor.
//
/* global AudioWorkletProcessor, Rnnoise, registerProcessor */

// At build time scripts/bundle-rnnoise.mjs prepends the Shiguredo vendor
// (with the ESM export stripped and `var Rnnoise = ...;` exposed in scope),
// so this file can use `Rnnoise` directly. Do not import anything here —
// AudioWorkletGlobalScope rejects ESM in WebView2.

// Jitsi-style LCM ring: lcm(128 quantum, 480 frame) = 1920. Frame slots
// {0,480,960,1440} and quantum slots {0,128,...,1792} both tile 1920 exactly,
// so neither subarray ever crosses the wrap boundary. No copyWithin, no shifts.
const QUANTUM = 128;
const FRAME = 480;
const RING_CAPACITY = 1920;
const Q15 = 32768;
const INV_Q15 = 1 / 32768;

class RnnoiseProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
    this.ready = false;
    this.primed = false;
    this.state = null;
    this.frame = null;
    this.inBuf = new Float32Array(RING_CAPACITY);
    this.outBuf = new Float32Array(RING_CAPACITY);
    this.writeHead = 0; // next input write position
    this.denoisedUpTo = 0; // next frame slot to denoise (also outBuf write slot)
    this.readHead = 0; // next output read position
    this.buffered = 0; // unprocessed input samples in inBuf
    this.pending = 0; // denoised samples ready in outBuf

    this.port.onmessage = (e) => {
      if (e.data && e.data.type === 'destroy') {
        try {
          this.state && this.state.destroy();
        } catch {
          /* ignore */
        }
        this.state = null;
        this.ready = false;
      }
    };

    this._init();
  }

  async _init() {
    try {
      const Rn = await Rnnoise.load();
      // LCM ring sizing assumes 480 — guard against future vendor changes.
      if (Rn.frameSize !== FRAME) {
        throw new Error(`rnnoise frameSize ${Rn.frameSize} != expected ${FRAME}`);
      }
      this.state = Rn.createDenoiseState();
      this.frame = new Float32Array(FRAME);
      this.ready = true;
      this.port.postMessage({ type: 'ready' });
    } catch (err) {
      this.port.postMessage({
        type: 'error',
        message: String((err && err.message) || err),
      });
    }
  }

  process(inputs, outputs) {
    const input = inputs[0] && inputs[0][0];
    const output = outputs[0] && outputs[0][0];
    if (!output) return true;

    if (!this.ready || !input) {
      if (input) output.set(input);
      else output.fill(0);
      return true;
    }

    const frame = this.frame;
    const inBuf = this.inBuf;
    const outBuf = this.outBuf;

    // 1. Write input quantum into inBuf. AudioWorklet always delivers exactly
    // QUANTUM (128) samples per render, and 1920 % 128 === 0 so this never
    // crosses the wrap boundary — single set() suffices.
    inBuf.set(input, this.writeHead);
    this.writeHead = (this.writeHead + QUANTUM) % RING_CAPACITY;
    this.buffered += QUANTUM;

    // 2. Denoise complete frames in place. RNNoise expects Q15-range floats,
    // so scale up before processFrame and back down on the way out.
    // 1920 % 480 === 0 so frame slot never wraps.
    while (this.buffered >= FRAME) {
      const slot = this.denoisedUpTo;
      for (let i = 0; i < FRAME; i++) frame[i] = inBuf[slot + i] * Q15;
      this.state.processFrame(frame);
      for (let i = 0; i < FRAME; i++) outBuf[slot + i] = frame[i] * INV_Q15;
      this.denoisedUpTo = (slot + FRAME) % RING_CAPACITY;
      this.buffered -= FRAME;
      this.pending += FRAME;
    }

    // 3. Hold output silent until pending has FRAME+QUANTUM (608 samples)
    // cushion so the bursty 480/128 production cadence never underruns. The
    // gate opens after the second frame produces (~8 quanta = 21ms first-emit).
    if (!this.primed) {
      if (this.pending >= FRAME + QUANTUM) {
        this.primed = true;
      } else {
        output.fill(0);
        return true;
      }
    }

    // 4. Emit output quantum from outBuf.
    if (this.pending >= QUANTUM) {
      output.set(outBuf.subarray(this.readHead, this.readHead + QUANTUM));
      this.readHead = (this.readHead + QUANTUM) % RING_CAPACITY;
      this.pending -= QUANTUM;
    } else {
      output.fill(0);
    }

    return true;
  }
}

registerProcessor('rnnoise-processor', RnnoiseProcessor);
