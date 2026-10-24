// Reconnect backoff scheduler, free of React/DOM/store so it can be tested
// with fake timers. The caller supplies what an attempt does via `onAttempt`.

export type ReconnectSchedulerOptions = {
  /** Delay (ms) for attempt 0, 1, 2, … Once exhausted, onExhausted fires. */
  delays: readonly number[];
  /** Resolve = success (counter resets), reject = failure (next attempt is scheduled). */
  onAttempt: (attempt: number) => Promise<void>;
  onExhausted: () => void;
  /** Checked before scheduling and before each attempt fires; true stops the scheduler. */
  isLeaving: () => boolean;
};

export type ReconnectScheduler = {
  /** Schedule the next attempt; no-op while one is already pending. */
  schedule: () => void;
  /** Cancel any pending timer and reset the counter. */
  reset: () => void;
  readonly attemptIndex: number;
};

export function createReconnectScheduler(opts: ReconnectSchedulerOptions): ReconnectScheduler {
  let timerId: ReturnType<typeof setTimeout> | null = null;
  let attempt = 0;

  function reset() {
    if (timerId !== null) clearTimeout(timerId);
    timerId = null;
    attempt = 0;
  }

  function schedule() {
    if (opts.isLeaving() || timerId !== null) return;

    const currentAttempt = attempt;
    if (currentAttempt >= opts.delays.length) {
      opts.onExhausted();
      return;
    }
    attempt = currentAttempt + 1;

    timerId = setTimeout(() => {
      timerId = null;
      if (opts.isLeaving()) return;
      opts.onAttempt(currentAttempt).then(
        () => {
          attempt = 0;
        },
        () => schedule(),
      );
    }, opts.delays[currentAttempt]);
  }

  return {
    schedule,
    reset,
    get attemptIndex() {
      return attempt;
    },
  };
}
