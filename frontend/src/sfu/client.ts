import {
  parseServerMessage,
  type ServerMessage,
  type WelcomePayload,
  type PeerInfo,
  type PeerLeftPayload,
  type PeerStatePayload,
  type ChatPayload,
  type ChatSendPayload,
  type ChatDeletedPayload,
  type PingPayload,
  type ScreenShareAvailablePayload,
  type ScreenShareEndedPayload,
  type ScreenShareErrorPayload,
  type ScreenVideoCodec,
} from './protocol';
export type {
  ChatOnlyHandlers,
  ChatOnlyConnectOptions,
  ChatOnlyClient,
} from './chat-client';
export { createChatClient } from './chat-client';
import { closeWebSocket } from './chat-client';
import {
  applyScreenCodecPreferences,
  canReceiveScreenCodec,
  chooseScreenCodec,
  isScreenVideoCodec,
  primeScreenCodecProfile,
} from '../screenshare/codec';
import {
  getCurrentScreenCodecPref,
  getCurrentScreenContentHint,
  getCurrentScreenParams,
  getCurrentShareMode,
} from '../store/useScreenShareSettingsStore';
import type { ScreenParams, ShareMode } from '../screenshare/params';
import { buildScreenParams, shareModeToContentHint } from '../screenshare/params';

export const SCREEN_SHARE_NO_CODEC = 'SCREEN_SHARE_NO_CODEC';

export type SFUHandlers = {
  onState: (state: string) => void;
  onWelcome: (data: WelcomePayload) => void;
  onPeerJoined: (data: PeerInfo) => void;
  onPeerLeft: (data: PeerLeftPayload) => void;
  onPeerInfo: (data: PeerInfo) => void;
  onPeerState: (data: PeerStatePayload) => void;
  onChat: (data: ChatPayload) => void;
  onChatDeleted: (data: ChatDeletedPayload) => void;
  onPing: (data: PingPayload) => void;
  onTrack: (data: { track: MediaStreamTrack; stream: MediaStream; peerId: string | null }) => void;
  onScreenShareAvailable: (data: ScreenShareAvailablePayload) => void;
  onScreenShareEnded: (data: ScreenShareEndedPayload) => void;
  onScreenShareError: (data: ScreenShareErrorPayload) => void;
  onScreenShareTrack: (data: {
    publisherId: string;
    track: MediaStreamTrack;
    stream: MediaStream;
    kind: 'video' | 'audio';
  }) => void;
  onScreenShareSelfStarted: (data: { stream: MediaStream; videoCodec: ScreenVideoCodec }) => void;
  onScreenShareSelfStopped: () => void;
  onScreenShareSystemAudioWarning: (data: { reason: 'monitor-feedback-risk' }) => void;
  onError: (err: unknown) => void;
};

export type ConnectOptions = {
  wsUrl: string;
  iceServers: RTCIceServer[];
  localStream: MediaStream;
  displayName: string;
  clientId: string;
};

export type SFUClient = {
  connect(opts: ConnectOptions): Promise<void>;
  reconnect(opts: ConnectOptions): Promise<void>;
  disconnect(): void;
  setDisplayName(name: string): void;
  sendSetState(selfMuted: boolean, deafened: boolean): void;
  sendChat(payload: ChatSendPayload): void;
  sendChatDelete(id: string): boolean;
  sendPing(targetId: string): void;
  getPeerConnection(): RTCPeerConnection | null;
  startScreenShare(): Promise<void>;
  stopScreenShare(): void;
  updateScreenShareParams(): Promise<void>;
  changeScreenShareMode(mode: ShareMode): Promise<void>;
  subscribeScreenShare(publisherId: string): void;
  unsubscribeScreenShare(publisherId: string): void;
  isPublishingScreenShare(): boolean;
  getScreenShareToken(): string | null;
  resumeScreenShare(token: string): Promise<void>;
};

function isFullLayerSet(layers: number[]): boolean {
  return layers.length === 3 && layers.includes(0) && layers.includes(1) && layers.includes(2);
}

function noop(): void {}

export function createSFUClient(handlers: Partial<SFUHandlers> = {}): SFUClient {
  const on: SFUHandlers = {
    onState: handlers.onState ?? noop,
    onWelcome: handlers.onWelcome ?? noop,
    onPeerJoined: handlers.onPeerJoined ?? noop,
    onPeerLeft: handlers.onPeerLeft ?? noop,
    onPeerInfo: handlers.onPeerInfo ?? noop,
    onPeerState: handlers.onPeerState ?? noop,
    onChat: handlers.onChat ?? noop,
    onChatDeleted: handlers.onChatDeleted ?? noop,
    onPing: handlers.onPing ?? noop,
    onTrack: handlers.onTrack ?? noop,
    onScreenShareAvailable: handlers.onScreenShareAvailable ?? noop,
    onScreenShareEnded: handlers.onScreenShareEnded ?? noop,
    onScreenShareError: handlers.onScreenShareError ?? noop,
    onScreenShareTrack: handlers.onScreenShareTrack ?? noop,
    onScreenShareSelfStarted: handlers.onScreenShareSelfStarted ?? noop,
    onScreenShareSelfStopped: handlers.onScreenShareSelfStopped ?? noop,
    onScreenShareSystemAudioWarning: handlers.onScreenShareSystemAudioWarning ?? noop,
    onError: handlers.onError ?? noop,
  };

  let ws: WebSocket | null = null;
  let pc: RTCPeerConnection | null = null;
  let stopped = false;
  let iceServers: RTCIceServer[] = [];

  let screenPubPC: RTCPeerConnection | null = null;
  let screenPubStream: MediaStream | null = null;
  let screenPubStopped = false;
  let screenPubToken: string | null = null;
  let screenPubVideoSender: RTCRtpSender | null = null;
  let screenPubInitialParams: Promise<void> | null = null;
  let resumeContinuation: {
    resolve: () => void;
    reject: (err: Error) => void;
  } | null = null;
  const screenSubs = new Map<string, RTCPeerConnection>();
  const screenShareCodecs = new Map<string, ScreenVideoCodec>();

  void primeScreenCodecProfile();

  // Returns whether the frame was actually written — callers that need delivery
  // feedback (e.g. chat-delete) check this; a closed socket is a no-op.
  function send(event: string, data: unknown): boolean {
    if (!ws || ws.readyState !== WebSocket.OPEN) return false;
    ws.send(JSON.stringify({ event, data }));
    return true;
  }

  function setupAudioAndWS(opts: ConnectOptions): Promise<void> {
    pc = new RTCPeerConnection({ iceServers });

    pc.ontrack = (event) => {
      const stream = event.streams?.[0] ?? null;
      const peerId = stream ? stream.id : null;
      if (stream) {
        on.onTrack({ track: event.track, stream, peerId });
      }
    };

    pc.onicecandidate = (event) => {
      if (!event.candidate) return;
      const cand = event.candidate.toJSON ? event.candidate.toJSON() : event.candidate;
      send('candidate', { pc: 'audio', ...cand });
    };

    pc.onconnectionstatechange = () => {
      if (!pc) return;
      on.onState(pc.connectionState);
    };

    for (const track of opts.localStream.getTracks()) {
      pc.addTrack(track, opts.localStream);
    }

    return new Promise<void>((resolve, reject) => {
      let resolved = false;
      const timer = setTimeout(() => {
        if (!resolved) {
          resolved = true;
          reject(new Error('sfu-client: welcome timeout'));
        }
      }, 10000);

      const socket = new WebSocket(opts.wsUrl);
      ws = socket;

      socket.onopen = () => {
        on.onState('connecting');
        socket.send(
          JSON.stringify({
            event: 'hello',
            data: { displayName: opts.displayName ?? '', clientId: opts.clientId },
          }),
        );
      };

      socket.onerror = (event) => {
        on.onError(event);
        if (!resolved) {
          resolved = true;
          clearTimeout(timer);
          reject(new Error('sfu-client: websocket error'));
        }
      };

      socket.onclose = () => {
        if (!stopped) on.onState('closed');
      };

      socket.onmessage = async (event) => {
        const msg = parseServerMessage(event.data as string);
        if (!msg) return;
        try {
          await handleServerMessage(msg);
        } catch (err) {
          on.onError(err);
        }
        if (msg.event === 'welcome' && !resolved) {
          resolved = true;
          clearTimeout(timer);
          resolve();
        }
      };
    });
  }

  function detachAudioAndWS(): void {
    if (ws) {
      ws.onopen = null;
      ws.onerror = null;
      ws.onclose = null;
      ws.onmessage = null;
      try {
        ws.close();
      } catch {
        /* ignore */
      }
      ws = null;
    }
    if (pc) {
      pc.ontrack = null;
      pc.onicecandidate = null;
      pc.onconnectionstatechange = null;
      try {
        pc.close();
      } catch {
        /* ignore */
      }
      pc = null;
    }
  }

  function connect(opts: ConnectOptions): Promise<void> {
    if (ws || pc) throw new Error('sfu-client: already connected');
    stopped = false;
    iceServers = opts.iceServers ?? [];
    return setupAudioAndWS(opts).catch((err) => {
      disconnect();
      throw err;
    });
  }

  function reconnect(opts: ConnectOptions): Promise<void> {
    if (stopped) throw new Error('sfu-client: cannot reconnect after disconnect');

    detachAudioAndWS();

    for (const id of Array.from(screenSubs.keys())) {
      teardownScreenSub(id);
    }
    screenShareCodecs.clear();
    if (resumeContinuation) {
      const cont = resumeContinuation;
      resumeContinuation = null;
      cont.reject(new Error('sfu-client: reconnect interrupted resume'));
    }

    iceServers = opts.iceServers ?? iceServers;
    return setupAudioAndWS(opts).catch((err) => {
      detachAudioAndWS();
      throw err;
    });
  }

  async function handleServerMessage(msg: ServerMessage): Promise<void> {
    switch (msg.event) {
      case 'welcome':
        screenShareCodecs.clear();
        for (const peer of msg.data.peers) {
          if (peer.screenSharing && isScreenVideoCodec(peer.screenSharingVideoCodec)) {
            screenShareCodecs.set(peer.id, peer.screenSharingVideoCodec);
          }
        }
        on.onWelcome(msg.data);
        break;
      case 'peer-joined':
        on.onPeerJoined(msg.data);
        break;
      case 'peer-left':
        screenShareCodecs.delete(msg.data.id);
        on.onPeerLeft(msg.data);
        break;
      case 'peer-info':
        if (msg.data.screenSharing && isScreenVideoCodec(msg.data.screenSharingVideoCodec)) {
          screenShareCodecs.set(msg.data.id, msg.data.screenSharingVideoCodec);
        } else if (!msg.data.screenSharing) {
          screenShareCodecs.delete(msg.data.id);
        }
        on.onPeerInfo(msg.data);
        break;
      case 'peer-state':
        on.onPeerState(msg.data);
        break;
      case 'chat':
        on.onChat(msg.data);
        break;
      case 'chat-deleted':
        on.onChatDeleted(msg.data);
        break;
      case 'ping':
        on.onPing(msg.data);
        break;
      case 'offer':
        await handleOffer(msg.data);
        break;
      case 'answer':
        await handleAnswer(msg.data);
        break;
      case 'candidate':
        await handleCandidate(msg.data);
        break;
      case 'screen-share-started':
        screenPubToken = msg.data.sessionToken;
        if (resumeContinuation) {
          const cont = resumeContinuation;
          resumeContinuation = null;
          cont.resolve();
        }
        break;
      case 'screen-share-available':
        if (isScreenVideoCodec(msg.data.videoCodec)) {
          screenShareCodecs.set(msg.data.publisherId, msg.data.videoCodec);
        }
        on.onScreenShareAvailable(msg.data);
        break;
      case 'screen-share-ended':
        // Tear down our local subscriber PC for that publisher, if any.
        screenShareCodecs.delete(msg.data.publisherId);
        teardownScreenSub(msg.data.publisherId);
        on.onScreenShareEnded(msg.data);
        break;
      case 'screen-share-error':
        // Best-effort cleanup of the relevant local state. The handler may
        // also revert UI state.
        if (msg.data.publisherId) teardownScreenSub(msg.data.publisherId);
        if (resumeContinuation) {
          const cont = resumeContinuation;
          resumeContinuation = null;
          cont.reject(new Error(`screen-share-error: ${msg.data.reason}`));
        }
        on.onScreenShareError(msg.data);
        break;
      case 'screen-share-encode-pause':
        if (!isFullLayerSet(msg.data.layers)) {
          console.warn('[sfu] partial encode-pause not supported, layers=', msg.data.layers);
          break;
        }
        void applyScreenEncodeActive(false);
        break;
      case 'screen-share-encode-resume':
        if (!isFullLayerSet(msg.data.layers)) {
          console.warn('[sfu] partial encode-resume not supported, layers=', msg.data.layers);
          break;
        }
        void applyScreenEncodeActive(true);
        break;
    }
  }

  async function handleOffer(data: Extract<ServerMessage, { event: 'offer' }>['data']): Promise<void> {
    switch (data.pc) {
      case 'audio': {
        if (!pc) return;
        await pc.setRemoteDescription({ type: data.type, sdp: data.sdp });
        const answer = await pc.createAnswer();
        await pc.setLocalDescription(answer);
        send('answer', { pc: 'audio', type: answer.type, sdp: answer.sdp });
        return;
      }
      case 'screen-sub': {
        const publisherId = data.publisherId;
        if (!publisherId) {
          console.warn('[sfu] screen-sub offer without publisherId');
          return;
        }
        const subPC = screenSubs.get(publisherId);
        if (!subPC) {
          console.warn(`[sfu] screen-sub offer for unknown publisher=${publisherId}`);
          return;
        }
        await subPC.setRemoteDescription({ type: data.type, sdp: data.sdp });
        const answer = await subPC.createAnswer();
        await subPC.setLocalDescription(answer);
        send('answer', {
          pc: 'screen-sub',
          publisherId,
          type: answer.type,
          sdp: answer.sdp,
        });
        return;
      }
      case 'screen-pub':
        // SFU never offers to a screen-pub PC — it always answers.
        console.warn('[sfu] unexpected offer with pc=screen-pub');
        return;
    }
  }

  async function handleAnswer(
    data: Extract<ServerMessage, { event: 'answer' }>['data'],
  ): Promise<void> {
    // The SFU answers the publisher's screen-pub offer. Other pc kinds
    // arriving as 'answer' are a protocol mistake; log and drop.
    if (data.pc !== 'screen-pub') {
      console.warn(`[sfu] unexpected answer with pc=${data.pc}`);
      return;
    }
    if (!screenPubPC) {
      console.warn('[sfu] screen-pub answer with no active publisher PC');
      return;
    }
    await screenPubPC.setRemoteDescription({ type: data.type, sdp: data.sdp });
  }

  async function handleCandidate(
    data: Extract<ServerMessage, { event: 'candidate' }>['data'],
  ): Promise<void> {
    const { pc: kind, publisherId, ...cand } = data;
    try {
      switch (kind) {
        case 'audio':
          if (!pc) return;
          await pc.addIceCandidate(cand);
          return;
        case 'screen-pub':
          if (!screenPubPC) return;
          await screenPubPC.addIceCandidate(cand);
          return;
        case 'screen-sub': {
          if (!publisherId) return;
          const subPC = screenSubs.get(publisherId);
          if (!subPC) return;
          await subPC.addIceCandidate(cand);
          return;
        }
      }
    } catch {
      // stale or invalid candidate; ignore.
    }
  }

  // ---- Screen share: publisher ----

  function isTabOrWindowSurface(track: MediaStreamTrack): boolean {
    const surface = track.getSettings().displaySurface;
    return surface === 'browser' || surface === 'window';
  }

  function tabSourceParams(params: ScreenParams, track?: MediaStreamTrack): ScreenParams {
    if (params.resolution !== 'source' || track?.getSettings().displaySurface !== 'browser') {
      return params;
    }
    // Tabs can expose a backing surface larger than the monitor.
    const dpr = window.devicePixelRatio || 1;
    const width = Math.round(window.screen.width * dpr);
    const height = Math.round(window.screen.height * dpr);
    if (!width || !height) return params;
    const maxPixels = Math.min(width * height, params.width * params.height);
    const basePixels = params.width * params.height;
    return {
      ...params,
      width,
      height,
      maxBitrate: Math.round(params.maxBitrate * (maxPixels / basePixels)),
    };
  }

  function screenCaptureConstraints(
    params: ScreenParams,
    track?: MediaStreamTrack,
  ): MediaTrackConstraints {
    params = tabSourceParams(params, track);
    const constraints: MediaTrackConstraints = {
      frameRate: { ideal: params.fps, max: params.fps },
    };
    const settings = track?.getSettings();
    const actualW = settings?.width;
    const actualH = settings?.height;
    if (!actualW || !actualH) {
      constraints.height = { max: params.height };
      return constraints;
    }

    const maxPixels = params.width * params.height;
    const actualPixels = actualW * actualH;
    if (actualPixels <= maxPixels) {
      constraints.height = { max: params.height };
      return constraints;
    }

    const scale = Math.sqrt(maxPixels / actualPixels);
    const maxW = Math.max(1, Math.floor(actualW * scale));
    const maxH = Math.max(1, Math.floor(actualH * scale));
    constraints.width = { ideal: maxW, max: maxW };
    constraints.height = { ideal: maxH, max: maxH };
    return constraints;
  }

  async function applyScreenCaptureConstraints(
    track: MediaStreamTrack,
    params: ScreenParams,
  ): Promise<void> {
    try {
      await track.applyConstraints(screenCaptureConstraints(params, track));
    } catch (err) {
      console.warn('[sfu] applyConstraints on screen video failed', err);
    }
  }

  async function applyScreenSenderParams(
    sender: RTCRtpSender,
    track: MediaStreamTrack | undefined,
    params: ScreenParams,
  ): Promise<void> {
    params = track ? tabSourceParams(params, track) : params;
    const senderParams = sender.getParameters();
    if (!senderParams.encodings || senderParams.encodings.length === 0) {
      senderParams.encodings = [{}];
    }
    senderParams.encodings[0] = {
      ...senderParams.encodings[0],
      maxBitrate: scaledBitrate(track, params),
      maxFramerate: params.fps,
    } as RTCRtpEncodingParameters;
    try {
      await sender.setParameters(senderParams);
    } catch (err) {
      console.warn('[sfu] setParameters(update) on screen video failed', err);
    }
  }

  function finishTabOrWindowMotionBootstrap(
    videoTrack: MediaStreamTrack,
    videoSender: RTCRtpSender,
    targetParams: ScreenParams,
    targetMode: ShareMode,
  ): void {
    if (targetMode !== 'motion' || !isTabOrWindowSurface(videoTrack)) return;
    // Chrome tab/window capture can start thumbnail-sized when initialized as
    // motion. Starting as sharp/source first matches the manual recovery path;
    // restore motion after capture and the publisher sender have settled.
    window.setTimeout(() => {
      if (screenPubStopped || screenPubVideoSender !== videoSender) return;
      const current = getCurrentScreenParams();
      if (
        getCurrentShareMode() !== targetMode ||
        current.resolution !== targetParams.resolution ||
        current.fps !== targetParams.fps
      ) {
        return;
      }
      videoTrack.contentHint = shareModeToContentHint(targetMode);
      void applyScreenCaptureConstraints(videoTrack, targetParams).then(() =>
        applyScreenSenderParams(videoSender, videoTrack, targetParams),
      );
    }, 500);
  }

  async function applyInitialEncoderParams(
    newPC: RTCPeerConnection,
    videoSender: RTCRtpSender,
    videoTrack: MediaStreamTrack,
    selectedCodec: ScreenVideoCodec,
    pickedParams: ReturnType<typeof getCurrentScreenParams>,
  ): Promise<void> {
    await new Promise<void>((resolve, reject) => {
      let settled = false;
      const t = setTimeout(() => {
        if (!settled) {
          settled = true;
          reject(new Error('sfu-client: screen-share answer timeout'));
        }
      }, 10000);
      const watcher = () => {
        if (newPC.signalingState !== 'stable' || settled) return;
        settled = true;
        clearTimeout(t);
        newPC.removeEventListener('signalingstatechange', watcher);
        try {
          const effectiveParams = tabSourceParams(pickedParams, videoTrack);
          const params = videoSender.getParameters();
          if (!params.encodings || params.encodings.length === 0) {
            params.encodings = [{}];
          }
          params.encodings[0] = {
            ...params.encodings[0],
            ...(selectedCodec === 'av1' ? { scalabilityMode: 'L1T3' } : {}),
            maxBitrate: scaledBitrate(videoTrack, effectiveParams),
            maxFramerate: effectiveParams.fps,
            priority: 'high',
          } as RTCRtpEncodingParameters;
          screenPubInitialParams = videoSender.setParameters(params).catch((err: unknown) => {
            console.warn('[sfu] setParameters on screen video failed', err);
          });
        } catch (err) {
          console.warn('[sfu] setParameters on screen video failed', err);
        }
        resolve();
      };
      newPC.addEventListener('signalingstatechange', watcher);
    });
  }

  function teardownNewPubPC(newPC: RTCPeerConnection, stream: MediaStream): void {
    try {
      stream.getTracks().forEach((t) => t.stop());
    } catch {
      /* ignore */
    }
    try {
      newPC.close();
    } catch {
      /* ignore */
    }
    if (screenPubPC === newPC) {
      screenPubPC = null;
      screenPubStream = null;
      screenPubVideoSender = null;
      screenPubInitialParams = null;
      screenPubStopped = false;
    }
    on.onScreenShareSelfStopped();
  }

  async function startScreenShare(): Promise<void> {
    if (screenPubPC) throw new Error('sfu-client: already publishing screen share');

    const caps = RTCRtpSender.getCapabilities('video');
    const selectedCodec = chooseScreenCodec(getCurrentScreenCodecPref());
    if (!caps || !selectedCodec) {
      throw new Error(SCREEN_SHARE_NO_CODEC);
    }

    const pickedParams = getCurrentScreenParams();
    const requestedMode = getCurrentShareMode();
    const bootstrapParams = buildScreenParams('source', pickedParams.fps, 'sharp');

    // Must stay sync from here to getDisplayMedia — any await breaks the gesture context.
    // windowAudio: 'window' asks Chromium for per-window (per-process) audio
    // on window surfaces, which avoids picking up the speaker mix and stops
    // viewers from hearing themselves. Monitor shares still capture the full
    // system mix — we warn the user in that case.
    const stream = await navigator.mediaDevices.getDisplayMedia({
      video: {
        frameRate: { ideal: pickedParams.fps, max: pickedParams.fps },
        height: { max: pickedParams.height },
      },
      audio: true,
      systemAudio: 'include',
      windowAudio: 'window',
      selfBrowserSurface: 'exclude',
      surfaceSwitching: 'include',
    } as DisplayMediaStreamOptions);

    const videoTrack = stream.getVideoTracks()[0];
    if (!videoTrack) {
      stream.getTracks().forEach((t) => t.stop());
      throw new Error('sfu-client: getDisplayMedia returned no video track');
    }
    const pickedSurface = videoTrack.getSettings().displaySurface;

    if (pickedSurface === 'monitor' && stream.getAudioTracks().length > 0) {
      on.onScreenShareSystemAudioWarning({ reason: 'monitor-feedback-risk' });
    }
    const shouldBootstrapBrowserMotion =
      requestedMode === 'motion' && isTabOrWindowSurface(videoTrack);
    videoTrack.contentHint = shouldBootstrapBrowserMotion ? 'text' : getCurrentScreenContentHint();
    await applyScreenCaptureConstraints(
      videoTrack,
      shouldBootstrapBrowserMotion ? bootstrapParams : pickedParams,
    );

    const audioTrack = stream.getAudioTracks()[0];
    const hasSystemAudio = !!audioTrack;

    const newPC = new RTCPeerConnection({ iceServers });
    screenPubPC = newPC;
    screenPubStream = stream;
    screenPubStopped = false;
    on.onScreenShareSelfStarted({ stream, videoCodec: selectedCodec });

    const videoSender = newPC.addTrack(videoTrack, stream);
    screenPubVideoSender = videoSender;
    if (audioTrack) newPC.addTrack(audioTrack, stream);

    const tx = newPC.getTransceivers().find((t) => t.sender === videoSender);
    if (tx) {
      applyScreenCodecPreferences(tx, caps, selectedCodec);
    }

    newPC.addEventListener('icecandidate', (ev) => {
      if (!ev.candidate || screenPubStopped) return;
      const cand = ev.candidate.toJSON ? ev.candidate.toJSON() : ev.candidate;
      send('candidate', { pc: 'screen-pub', ...cand });
    });

    newPC.addEventListener('connectionstatechange', () => {
      if (newPC.connectionState === 'failed' || newPC.connectionState === 'closed') {
        if (!screenPubStopped) stopScreenShare();
      }
    });

    videoTrack.addEventListener('ended', () => {
      if (!screenPubStopped) stopScreenShare();
    });

    const offer = await newPC.createOffer();
    await newPC.setLocalDescription(offer);

    send('screen-share-start', {
      sdp: offer.sdp ?? '',
      hasSystemAudio,
      mode: requestedMode,
    });

    try {
      await applyInitialEncoderParams(newPC, videoSender, videoTrack, selectedCodec, pickedParams);
      finishTabOrWindowMotionBootstrap(videoTrack, videoSender, pickedParams, requestedMode);
    } catch (err) {
      teardownNewPubPC(newPC, stream);
      throw err;
    }
  }

  function stopScreenShare(): void {
    if (!screenPubPC) return;
    screenPubStopped = true;
    send('screen-share-stop', {});
    try {
      screenPubStream?.getTracks().forEach((t) => t.stop());
    } catch {
      /* ignore */
    }
    try {
      screenPubPC.close();
    } catch {
      /* ignore */
    }
    screenPubPC = null;
    screenPubStream = null;
    screenPubToken = null;
    screenPubVideoSender = null;
    screenPubInitialParams = null;
    on.onScreenShareSelfStopped();
  }

  function scaledBitrate(track: MediaStreamTrack | undefined, p: ScreenParams): number {
    const actualW = track?.getSettings().width;
    if (!actualW || actualW <= p.width) return p.maxBitrate;
    const scale = Math.min(actualW / p.width, 2);
    return Math.round(p.maxBitrate * scale);
  }

  async function updateScreenShareParams(): Promise<void> {
    const sender = screenPubVideoSender;
    const stream = screenPubStream;
    if (!sender || !stream) return;
    if (screenPubInitialParams) {
      try {
        await screenPubInitialParams;
      } catch {
        // already logged in the watcher
      }
    }
    const next = getCurrentScreenParams();

    const track = stream.getVideoTracks()[0];
    if (track) {
      track.contentHint = getCurrentScreenContentHint();
      await applyScreenCaptureConstraints(track, next);
    }
    await applyScreenSenderParams(sender, track, next);
  }

  async function changeScreenShareMode(mode: ShareMode): Promise<void> {
    if (!screenPubVideoSender || !screenPubStream) return;
    const track = screenPubStream.getVideoTracks()[0];
    if (track) {
      track.contentHint = shareModeToContentHint(mode);
    }
    const eff = getCurrentScreenParams();
    const next = buildScreenParams(eff.resolution, eff.fps, mode);
    const sender = screenPubVideoSender;
    const params = sender.getParameters();
    if (!params.encodings || params.encodings.length === 0) {
      params.encodings = [{}];
    }
    params.encodings[0] = {
      ...params.encodings[0],
      maxBitrate: scaledBitrate(track, next),
      maxFramerate: next.fps,
    } as RTCRtpEncodingParameters;
    try {
      await sender.setParameters(params);
    } catch (err) {
      console.warn('[sfu] setParameters(mode-change) on screen video failed', err);
    }
    send('screen-share-mode-change', { mode });
  }

  async function applyScreenEncodeActive(active: boolean): Promise<void> {
    if (screenPubInitialParams) {
      try {
        await screenPubInitialParams;
      } catch {
        // already logged in the watcher
      }
    }
    const sender = screenPubVideoSender;
    if (!sender) return;
    const params = sender.getParameters();
    if (!params.encodings || params.encodings.length === 0) {
      params.encodings = [{}];
    }
    params.encodings[0] = { ...params.encodings[0], active };
    try {
      await sender.setParameters(params);
    } catch (err) {
      console.warn('[sfu] setParameters(active) on screen video failed', err);
    }
  }

  function getScreenShareToken(): string | null {
    return screenPubToken;
  }

  async function resumeScreenShare(token: string): Promise<void> {
    if (!screenPubPC) {
      throw new Error('sfu-client: no live publisher PC to resume');
    }
    if (resumeContinuation) {
      throw new Error('sfu-client: resume already in flight');
    }

    const settled = new Promise<void>((resolve, reject) => {
      resumeContinuation = { resolve, reject };
    });
    send('screen-share-resume', { sessionToken: token });

    // Server-side: validates token → re-broadcasts -ended/-available → sends
    // screen-share-started back to us. On success the message handler resolves
    // settled; on screen-share-error it rejects.
    const timeoutId = setTimeout(() => {
      if (resumeContinuation) {
        const cont = resumeContinuation;
        resumeContinuation = null;
        cont.reject(new Error('sfu-client: screen-share-resume timeout'));
      }
    }, 10000);

    try {
      await settled;
    } finally {
      clearTimeout(timeoutId);
    }

    const pcRef = screenPubPC;
    if (!pcRef) {
      throw new Error('sfu-client: publisher PC vanished mid-resume');
    }
    const offer = await pcRef.createOffer({ iceRestart: true });
    await pcRef.setLocalDescription(offer);
    send('offer', { pc: 'screen-pub', type: offer.type, sdp: offer.sdp ?? '' });
  }

  function isPublishingScreenShare(): boolean {
    return screenPubPC !== null && !screenPubStopped;
  }

  // ---- Screen share: subscriber ----

  function subscribeScreenShare(publisherId: string): void {
    if (screenSubs.has(publisherId)) return;
    const codec = screenShareCodecs.get(publisherId);
    if (codec && !canReceiveScreenCodec(codec)) {
      on.onScreenShareError({ publisherId, reason: 'internal' });
      return;
    }
    const subPC = new RTCPeerConnection({ iceServers });
    screenSubs.set(publisherId, subPC);

    subPC.addEventListener('track', (ev) => {
      const stream = ev.streams[0] ?? new MediaStream([ev.track]);
      on.onScreenShareTrack({
        publisherId,
        track: ev.track,
        stream,
        kind: ev.track.kind as 'video' | 'audio',
      });
    });

    subPC.addEventListener('icecandidate', (ev) => {
      if (!ev.candidate) return;
      const cand = ev.candidate.toJSON ? ev.candidate.toJSON() : ev.candidate;
      send('candidate', { pc: 'screen-sub', publisherId, ...cand });
    });

    subPC.addEventListener('connectionstatechange', () => {
      if (subPC.connectionState === 'failed' || subPC.connectionState === 'closed') {
        teardownScreenSub(publisherId);
      }
    });

    send('screen-share-subscribe', { publisherId, preferredTemporalLayer: 2 });
  }

  function unsubscribeScreenShare(publisherId: string): void {
    if (!screenSubs.has(publisherId)) return;
    send('screen-share-unsubscribe', { publisherId });
    teardownScreenSub(publisherId);
  }

  function teardownScreenSub(publisherId: string): void {
    const subPC = screenSubs.get(publisherId);
    if (!subPC) return;
    screenSubs.delete(publisherId);
    try {
      subPC.close();
    } catch {
      /* ignore */
    }
  }

  function setDisplayName(name: string): void {
    send('set-displayname', { displayName: name });
  }

  function sendSetState(selfMuted: boolean, deafened: boolean): void {
    send('set-state', { selfMuted, deafened });
  }

  function sendChat(payload: ChatSendPayload): void {
    send('chat-send', payload);
  }

  function sendChatDelete(id: string): boolean {
    return send('chat-delete', { id });
  }

  function sendPing(targetId: string): void {
    send('ping', { to: targetId });
  }

  function getPeerConnection(): RTCPeerConnection | null {
    return pc;
  }

  function disconnect(): void {
    stopped = true;
    if (screenPubPC) stopScreenShare();
    for (const id of Array.from(screenSubs.keys())) {
      teardownScreenSub(id);
    }
    if (ws) {
      closeWebSocket(ws);
      ws = null;
    }
    if (pc) {
      try {
        pc.close();
      } catch {
        /* ignore */
      }
      pc = null;
    }
  }

  return {
    connect,
    reconnect,
    disconnect,
    setDisplayName,
    sendSetState,
    sendChat,
    sendChatDelete,
    sendPing,
    getPeerConnection,
    startScreenShare,
    stopScreenShare,
    updateScreenShareParams,
    changeScreenShareMode,
    subscribeScreenShare,
    unsubscribeScreenShare,
    isPublishingScreenShare,
    getScreenShareToken,
    resumeScreenShare,
  };
}
