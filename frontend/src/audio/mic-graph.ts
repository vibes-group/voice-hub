import type { EngineKind } from '../types';
import { getDenoiser } from './denoisers/registry';
import type { DenoiserNode } from './denoisers/types';

export type MicGraph = {
  localAudioContext: AudioContext;
  localSourceNode: MediaStreamAudioSourceNode;
  localHighPassNode: BiquadFilterNode;
  localGainNode: GainNode;
  localDestinationNode: MediaStreamAudioDestinationNode;
  localMonitorAnalyser: AnalyserNode;
  localMonitorData: Float32Array<ArrayBuffer>;
  processedLocalStream: MediaStream;
  denoiser: DenoiserNode | null;
};

export function createLocalAudioContext(): AudioContext {
  try {
    return new AudioContext({ sampleRate: 48000 });
  } catch {
    return new AudioContext();
  }
}

export async function buildMicGraph(
  rawLocalStream: MediaStream,
  engine: EngineKind,
  sendVolumeRef: () => number,
  onStatusMessage: (msg: string, isError?: boolean) => void,
  prebuiltContext?: AudioContext,
): Promise<MicGraph> {
  const localAudioContext = prebuiltContext ?? createLocalAudioContext();
  if (localAudioContext.state !== 'running') {
    await localAudioContext.resume();
  }

  const localSourceNode = localAudioContext.createMediaStreamSource(rawLocalStream);

  const localMonitorAnalyser = localAudioContext.createAnalyser();
  localMonitorAnalyser.fftSize = 512;
  const localMonitorData = new Float32Array(
    localMonitorAnalyser.fftSize,
  ) as Float32Array<ArrayBuffer>;

  const localGainNode = localAudioContext.createGain();
  const localDestinationNode = localAudioContext.createMediaStreamDestination();

  const localHighPassNode = localAudioContext.createBiquadFilter();
  localHighPassNode.type = 'highpass';
  localHighPassNode.frequency.value = 110;
  localHighPassNode.Q.value = 0.707;

  localSourceNode.connect(localHighPassNode);

  let chainTail: AudioNode = localHighPassNode;
  let denoiser: DenoiserNode | null = null;

  const denoiserDef = getDenoiser(engine);
  if (denoiserDef) {
    denoiser = await denoiserDef.create(localAudioContext);
    if (denoiser) {
      localHighPassNode.connect(denoiser.input);
      chainTail = denoiser.output;
    } else {
      onStatusMessage(`${denoiserDef.label} недоступен, отправка без шумоподавления.`, true);
    }
  }

  chainTail.connect(localGainNode);
  localGainNode.connect(localDestinationNode);

  // Tap pre-volume so the speaking threshold doesn't track the user's slider.
  chainTail.connect(localMonitorAnalyser);

  const graph: MicGraph = {
    localAudioContext,
    localSourceNode,
    localHighPassNode,
    localGainNode,
    localDestinationNode,
    localMonitorAnalyser,
    localMonitorData,
    processedLocalStream: localDestinationNode.stream,
    denoiser,
  };

  applySendGain(graph, sendVolumeRef);

  return graph;
}

export function applySendGain(graph: MicGraph, sendVolumeRef: () => number): void {
  graph.localGainNode.gain.value = sendVolumeRef() / 100;
}

export function teardownMicGraph(graph: MicGraph): void {
  if (graph.denoiser) {
    graph.denoiser.dispose();
    graph.denoiser = null;
  }

  graph.localSourceNode.disconnect();
  graph.localHighPassNode.disconnect();
  graph.localGainNode.disconnect();
  graph.localMonitorAnalyser.disconnect();

  graph.processedLocalStream.getTracks().forEach((t) => t.stop());
  void graph.localAudioContext.close().catch(() => undefined);
}
