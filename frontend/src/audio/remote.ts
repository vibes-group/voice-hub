export type RemoteParticipantAudio = {
  audioEl: HTMLAudioElement;
  gainNode: GainNode;
  sourceNode: MediaStreamAudioSourceNode | null;
  analyser: AnalyserNode;
  monitorData: Float32Array<ArrayBuffer>;
};

export function createRemoteAudioContext(): AudioContext {
  const ctx = new AudioContext({ sampleRate: 48000 });
  ctx.resume().catch((err: unknown) => console.warn('[remote-audio] ctx resume failed:', err));
  return ctx;
}

export function setupParticipantAudio(
  ctx: AudioContext,
  stream: MediaStream,
): RemoteParticipantAudio {
  const audioEl = document.createElement('audio');
  audioEl.autoplay = true;
  audioEl.muted = true;
  audioEl.srcObject = stream;
  audioEl
    .play()
    .catch((err: unknown) =>
      console.warn(`[remote-audio] play() failed stream=${stream.id}:`, err),
    );

  const gainNode = ctx.createGain();
  gainNode.connect(ctx.destination);

  const analyser = ctx.createAnalyser();
  analyser.fftSize = 512;
  analyser.smoothingTimeConstant = 0;
  const monitorData = new Float32Array(analyser.fftSize) as Float32Array<ArrayBuffer>;

  let sourceNode: MediaStreamAudioSourceNode | null = null;
  try {
    sourceNode = ctx.createMediaStreamSource(stream);
    sourceNode.connect(gainNode);
    sourceNode.connect(analyser);
  } catch (err) {
    console.warn(`[remote-audio] createMediaStreamSource failed stream=${stream.id}:`, err);
  }

  return {
    audioEl,
    gainNode,
    sourceNode,
    analyser,
    monitorData,
  };
}

export function teardownParticipantAudio(audio: RemoteParticipantAudio): void {
  audio.sourceNode?.disconnect();
  audio.gainNode.disconnect();
  audio.analyser.disconnect();
  audio.audioEl.pause();
  audio.audioEl.srcObject = null;
}

export function applyParticipantGain(
  audio: RemoteParticipantAudio,
  outputVolume: number,
  deafened: boolean,
  localMuted: boolean,
  localVolume: number,
): void {
  const gain = deafened || localMuted ? 0 : (outputVolume / 100) * (localVolume / 100);
  audio.gainNode.gain.setTargetAtTime(gain, audio.gainNode.context.currentTime, 0.01);
}
