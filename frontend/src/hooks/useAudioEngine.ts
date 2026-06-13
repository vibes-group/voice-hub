// Imperative facade over the local audio graph.
// Owns refs to AudioContext, MicGraph, and remote audio nodes.
// Does NOT put AudioNode instances into zustand.

import { useRef, useCallback } from 'react';
import { useStore } from '../store/useStore';
import type { EngineKind } from '../types';
import {
  buildMicGraph,
  teardownMicGraph,
  applySendGain,
  createLocalAudioContext,
  type MicGraph,
} from '../audio/mic-graph';
import {
  createRemoteAudioContext,
  setupParticipantAudio,
  teardownParticipantAudio,
  applyParticipantGain,
  type RemoteParticipantAudio,
} from '../audio/remote';
import { createSpeakingLoop, type SpeakingLoop } from '../audio/speaking-loop';
import { isCaptureEngine, preloadEngine } from '../audio/engine';

const LOCAL_SPEAKING_ID = 'local';

function getAudioSender(pc: RTCPeerConnection | null): RTCRtpSender | null {
  return pc?.getSenders().find((s) => s.track?.kind === 'audio') ?? null;
}

type AudioEngineRef = {
  rawLocalStream: MediaStream | null;
  rawLocalStreamUsesBrowserNs: boolean | null;
  micGraph: MicGraph | null;
  remoteAudio: Map<string, RemoteParticipantAudio>;
  remoteAudioCtx: AudioContext | null;
  speakingLoop: SpeakingLoop;
};

export function useAudioEngine() {
  const refs = useRef<AudioEngineRef>({
    rawLocalStream: null,
    rawLocalStreamUsesBrowserNs: null,
    micGraph: null,
    remoteAudio: new Map(),
    remoteAudioCtx: null,
    speakingLoop: createSpeakingLoop(),
  });

  // ---- Mic graph ----

  const openMicStream = useCallback(async (engine: EngineKind): Promise<MediaStream> => {
    const deviceId = useStore.getState().micDeviceId;
    const useBrowserNs = isCaptureEngine(engine);
    const baseConstraints: MediaTrackConstraints = {
      channelCount: 1,
      sampleRate: 48000,
      echoCancellation: true,
      noiseSuppression: useBrowserNs,
      autoGainControl: true,
    };
    try {
      const audio: MediaTrackConstraints = deviceId
        ? { ...baseConstraints, deviceId: { exact: deviceId } }
        : baseConstraints;
      return await navigator.mediaDevices.getUserMedia({ audio, video: false });
    } catch (err) {
      // Saved deviceId may refer to an unplugged/revoked device. Drop the
      // pinned id and retry with system default rather than failing the join.
      if (deviceId && err instanceof Error && err.name === 'OverconstrainedError') {
        useStore.getState().setMicDeviceId(null);
        return await navigator.mediaDevices.getUserMedia({
          audio: baseConstraints,
          video: false,
        });
      }
      throw err;
    }
  }, []);

  const acquireMic = useCallback(
    async (engine: EngineKind) => {
      const r = refs.current;
      const useBrowserNs = isCaptureEngine(engine);
      const haveLiveMic = r.rawLocalStream?.getAudioTracks().some((t) => t.readyState === 'live');
      if (haveLiveMic && r.rawLocalStreamUsesBrowserNs === useBrowserNs) return;
      r.rawLocalStream?.getTracks().forEach((t) => t.stop());
      r.rawLocalStream = await openMicStream(engine);
      r.rawLocalStreamUsesBrowserNs = useBrowserNs;
    },
    [openMicStream],
  );

  const buildGraph = useCallback(
    async (engine: EngineKind, prebuiltContext?: AudioContext, rawStream?: MediaStream) => {
      const r = refs.current;
      const stream = rawStream ?? r.rawLocalStream;
      if (!stream) throw new Error('No mic stream');
      const graph = await buildMicGraph(
        stream,
        engine,
        () => useStore.getState().sendVolume,
        (msg, isError) => useStore.getState().setStatus(msg, isError),
        prebuiltContext,
      );
      r.micGraph = graph;
      return graph;
    },
    [],
  );

  const clearLocalGraph = useCallback(() => {
    const r = refs.current;
    r.speakingLoop.unregister(LOCAL_SPEAKING_ID);
    if (r.micGraph) {
      teardownMicGraph(r.micGraph);
      r.micGraph = null;
    }
  }, []);

  const clearRawMic = useCallback(() => {
    const r = refs.current;
    r.rawLocalStream?.getTracks().forEach((t) => t.stop());
    r.rawLocalStream = null;
    r.rawLocalStreamUsesBrowserNs = null;
  }, []);

  const prepareRawMicForEngine = useCallback(
    async (engine: EngineKind): Promise<{ stream: MediaStream; captureModeChanged: boolean }> => {
      const r = refs.current;
      const useBrowserNs = isCaptureEngine(engine);
      const captureModeChanged = r.rawLocalStreamUsesBrowserNs !== useBrowserNs;
      const haveLiveMic = r.rawLocalStream?.getAudioTracks().some((t) => t.readyState === 'live');
      if (haveLiveMic && !captureModeChanged && r.rawLocalStream) {
        return { stream: r.rawLocalStream, captureModeChanged: false };
      }

      if (captureModeChanged) {
        clearLocalGraph();
      }
      clearRawMic();

      const stream = await openMicStream(engine);
      r.rawLocalStream = stream;
      r.rawLocalStreamUsesBrowserNs = useBrowserNs;
      return { stream, captureModeChanged };
    },
    [clearLocalGraph, clearRawMic, openMicStream],
  );

  const prepareLocalAudio = useCallback(
    async (engine: EngineKind, onProgress?: (stage: 'mic-ready') => void) => {
      void preloadEngine(engine);
      const ctxPromise = (async () => {
        const ctx = createLocalAudioContext();
        await ctx.resume();
        return ctx;
      })();
      const [, ctx] = await Promise.all([acquireMic(engine), ctxPromise]);
      onProgress?.('mic-ready');
      return buildGraph(engine, ctx);
    },
    [acquireMic, buildGraph],
  );

  const rebuildLocalAudio = useCallback(
    async (
      engine: EngineKind,
      selfMuted: boolean,
      getSFUPeerConnection: () => RTCPeerConnection | null,
    ) => {
      const r = refs.current;
      const oldGraph = r.micGraph;
      const { stream: rawStream, captureModeChanged } = await prepareRawMicForEngine(engine);
      r.micGraph = null;
      let graph: MicGraph;
      try {
        graph = await buildGraph(engine, undefined, rawStream);
      } catch (err) {
        if (captureModeChanged) {
          clearRawMic();
        } else {
          r.micGraph = oldGraph;
        }
        throw err;
      }
      const newTrack = graph.processedLocalStream.getAudioTracks()[0];
      if (!newTrack) {
        teardownMicGraph(graph);
        if (captureModeChanged) {
          clearRawMic();
        } else {
          r.micGraph = oldGraph;
        }
        throw new Error('No audio track after rebuild');
      }
      newTrack.enabled = !selfMuted;
      const sender = getAudioSender(getSFUPeerConnection());
      if (sender) {
        try {
          await sender.replaceTrack(newTrack);
        } catch (err) {
          teardownMicGraph(graph);
          if (captureModeChanged) {
            clearRawMic();
          } else {
            r.micGraph = oldGraph;
          }
          throw err;
        }
      }
      if (!captureModeChanged && oldGraph) {
        r.speakingLoop.unregister(LOCAL_SPEAKING_ID);
        teardownMicGraph(oldGraph);
      }
      return graph;
    },
    [buildGraph, clearRawMic, prepareRawMicForEngine],
  );

  const switchMicDevice = useCallback(
    async (
      engine: EngineKind,
      selfMuted: boolean,
      getSFUPeerConnection: () => RTCPeerConnection | null,
    ) => {
      clearLocalGraph();
      clearRawMic();
      await acquireMic(engine);
      const graph = await buildGraph(engine);
      const newTrack = graph.processedLocalStream.getAudioTracks()[0];
      if (!newTrack) throw new Error('No audio track after device switch');
      newTrack.enabled = !selfMuted;
      const sender = getAudioSender(getSFUPeerConnection());
      if (sender) await sender.replaceTrack(newTrack);
      return graph;
    },
    [acquireMic, buildGraph, clearRawMic, clearLocalGraph],
  );

  const updateSendGain = useCallback(() => {
    const r = refs.current;
    if (!r.micGraph) return;
    // Read store directly so same-turn slider updates apply immediately.
    applySendGain(r.micGraph, () => useStore.getState().sendVolume);
  }, []);

  const startSpeaking = useCallback(
    (
      graph: MicGraph,
      getSelfMuted: () => boolean,
      onSpeakingChange: (speaking: boolean) => void,
    ) => {
      refs.current.speakingLoop.register(LOCAL_SPEAKING_ID, {
        analyser: graph.localMonitorAnalyser,
        data: graph.localMonitorData,
        getMuted: getSelfMuted,
        onChange: onSpeakingChange,
      });
    },
    [],
  );

  // ---- Remote audio ----

  const applyAllRemoteGains = useCallback(() => {
    const r = refs.current;
    const { outputVolume, deafened, participants } = useStore.getState();
    for (const [id, audio] of r.remoteAudio.entries()) {
      const p = participants[id];
      applyParticipantGain(
        audio,
        outputVolume,
        deafened,
        p?.localMuted ?? false,
        p?.localVolume ?? 100,
      );
    }
  }, []);

  const attachRemoteStream = useCallback(
    (participantId: string, stream: MediaStream) => {
      const r = refs.current;
      // Tear down existing audio for this participant if present.
      const existing = r.remoteAudio.get(participantId);
      if (existing) {
        r.speakingLoop.unregister(participantId);
        teardownParticipantAudio(existing);
      }
      // Create the shared remote AudioContext lazily on first attach.
      if (!r.remoteAudioCtx) {
        r.remoteAudioCtx = createRemoteAudioContext();
      }
      const audio = setupParticipantAudio(r.remoteAudioCtx, stream);
      r.remoteAudio.set(participantId, audio);
      applyAllRemoteGains();
      r.speakingLoop.register(participantId, {
        analyser: audio.analyser,
        data: audio.monitorData,
        onChange: (speaking) => {
          const current = useStore.getState().participants[participantId];
          if (current && current.speaking !== speaking) {
            useStore.getState().updateParticipant(participantId, { speaking });
          }
        },
      });
    },
    [applyAllRemoteGains],
  );

  const detachRemoteStream = useCallback((participantId: string) => {
    const r = refs.current;
    const audio = r.remoteAudio.get(participantId);
    if (audio) {
      r.speakingLoop.unregister(participantId);
      teardownParticipantAudio(audio);
      r.remoteAudio.delete(participantId);
    }
  }, []);

  const cleanupAllRemote = useCallback(() => {
    const r = refs.current;
    for (const id of r.remoteAudio.keys()) {
      r.speakingLoop.unregister(id);
    }
    for (const audio of r.remoteAudio.values()) {
      teardownParticipantAudio(audio);
    }
    r.remoteAudio.clear();
    void r.remoteAudioCtx?.close().catch(() => undefined);
    r.remoteAudioCtx = null;
  }, []);

  const fullCleanup = useCallback(() => {
    clearLocalGraph();
    cleanupAllRemote();
    clearRawMic();
  }, [clearLocalGraph, cleanupAllRemote, clearRawMic]);

  return {
    prepareLocalAudio,
    rebuildLocalAudio,
    switchMicDevice,
    updateSendGain,
    startSpeaking,
    attachRemoteStream,
    detachRemoteStream,
    applyAllRemoteGains,
    cleanupAllRemote,
    fullCleanup,
  };
}
