// Two distinct mute/unmute audio cues, synthesised via Web Audio (no asset pipeline).
// The pair contrasts on two axes at once so the two are never confused:
// Mute (going off): dull triangle, descending 520 → 300 Hz.
// Unmute (going on): bright sine, wide ascending sweep 392 → 880 Hz.

const DURATION = 0.12;

function playGlide(from: number, to: number, type: OscillatorType, peak: number): void {
  let ctx: AudioContext;
  try {
    ctx = new AudioContext({ sampleRate: 48000 });
  } catch {
    return;
  }
  const t0 = ctx.currentTime;
  const osc = ctx.createOscillator();
  const gain = ctx.createGain();
  osc.type = type;
  osc.frequency.setValueAtTime(from, t0);
  osc.frequency.exponentialRampToValueAtTime(to, t0 + DURATION);
  gain.gain.setValueAtTime(0, t0);
  gain.gain.linearRampToValueAtTime(peak, t0 + 0.005);
  gain.gain.linearRampToValueAtTime(0, t0 + DURATION);
  osc.connect(gain);
  gain.connect(ctx.destination);
  osc.onended = () => void ctx.close().catch(() => undefined);
  osc.start(t0);
  osc.stop(t0 + DURATION + 0.02);
}

export function playMuteSound(): void {
  playGlide(520, 300, 'triangle', 0.2);
}

export function playUnmuteSound(): void {
  playGlide(392, 880, 'sine', 0.16);
}

export function playPing(): void {
  let ctx: AudioContext;
  try {
    ctx = new AudioContext({ sampleRate: 48000 });
  } catch {
    return;
  }

  // Soft bell tap: two stacked sines (587 Hz + 880 Hz, fifth interval) with
  // exponential decay. Pleasant, salient, no buzz.
  const PEAK_LOW = 0.28;
  const PEAK_HIGH = 0.18;
  const DURATION = 0.42;
  const t0 = ctx.currentTime;
  const end = t0 + DURATION;

  function tone(freq: number, peak: number): void {
    const osc = ctx.createOscillator();
    const gain = ctx.createGain();
    osc.type = 'sine';
    osc.frequency.setValueAtTime(freq, t0);
    gain.gain.setValueAtTime(0, t0);
    gain.gain.linearRampToValueAtTime(peak, t0 + 0.008);
    gain.gain.exponentialRampToValueAtTime(0.0001, end);
    osc.connect(gain);
    gain.connect(ctx.destination);
    osc.start(t0);
    osc.stop(end + 0.02);
    osc.onended = () => void ctx.close().catch(() => undefined);
  }

  tone(587, PEAK_LOW);
  tone(880, PEAK_HIGH);
}
