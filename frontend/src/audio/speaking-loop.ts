import { detectLevel, SPEAKING_THRESHOLD } from './level-detect';

const TICK_MS = 50;
const HOLD_MS = 200;

export type SpeakingTarget = {
  analyser: AnalyserNode;
  data: Float32Array<ArrayBuffer>;
  // Optional gate: when true, target is forced silent without applying hold.
  getMuted?: () => boolean;
  onChange: (speaking: boolean) => void;
};

type TargetState = SpeakingTarget & {
  speaking: boolean;
  speakingHoldUntil: number;
};

export type SpeakingLoop = {
  register(id: string, target: SpeakingTarget): void;
  unregister(id: string): void;
};

export function createSpeakingLoop(): SpeakingLoop {
  const targets = new Map<string, TargetState>();
  let timerId: number | null = null;

  const tick = (): void => {
    // Speaking indicators are invisible in background tabs; skip expensive analyser reads.
    if (document.visibilityState === 'hidden') return;
    const now = performance.now();
    for (const t of targets.values()) {
      const muted = t.getMuted?.() ?? false;
      if (muted) {
        t.speakingHoldUntil = 0;
        if (t.speaking) {
          t.speaking = false;
          t.onChange(false);
        }
        continue;
      }
      const level = detectLevel(t.analyser, t.data);
      if (level > SPEAKING_THRESHOLD) {
        t.speakingHoldUntil = now + HOLD_MS;
      }
      const speakingNow = t.speakingHoldUntil > now;
      if (speakingNow !== t.speaking) {
        t.speaking = speakingNow;
        t.onChange(speakingNow);
      }
    }
  };

  const ensureTimer = (): void => {
    if (timerId === null && targets.size > 0) {
      timerId = window.setInterval(tick, TICK_MS);
    }
  };

  const maybeStopTimer = (): void => {
    if (targets.size === 0 && timerId !== null) {
      window.clearInterval(timerId);
      timerId = null;
    }
  };

  return {
    register(id, target) {
      const prev = targets.get(id);
      targets.set(id, {
        ...target,
        speaking: prev?.speaking ?? false,
        speakingHoldUntil: 0,
      });
      ensureTimer();
    },
    unregister(id) {
      const t = targets.get(id);
      if (!t) return;
      targets.delete(id);
      if (t.speaking) t.onChange(false);
      maybeStopTimer();
    },
  };
}
