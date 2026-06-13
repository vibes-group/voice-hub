// Session lifecycle hook.
//
// Owns: join, leave, reconnect state machine (scheduleReconnect +
// RECONNECT_DELAYS_MS), Tauri "toggle-mute" event bridge, config loading,
// auto-rejoin-on-reload, mic track enable/disable, engine switch,
// display-name sync to SFU.
//
// Does NOT own: mute/deafen boolean state (App), output mute/deafen UX (App),
// status messages for engine switch (App wraps switchEngine in try/catch).

import { useRef, useEffect, useCallback } from 'react';
import { useStore } from '../store/useStore';
import { useScreenShareStore } from '../store/useScreenShareStore';
import { useAudioEngine } from './useAudioEngine';
import { preloadEngine, isEngineReady, formatEngine } from '../audio/engine';
import { useSFU } from './useSFU';
import {
  loadOrCreateDisplayName,
  makeGuestName,
  saveDisplayName,
  consumeRejoinFlag,
  loadOrCreateClientId,
} from '../utils/storage';
import type { Attachment, ChatPayload, ChatDeletedPayload, PingPayload } from '../sfu/protocol';
import type { ShareMode } from '../screenshare/params';
import { SCREEN_SHARE_NO_CODEC } from '../sfu/client';
import { playPing } from '../audio/feedback-sounds';
import { flashAttention } from '../utils/tray';
import { flashFavicon } from '../utils/favicon';
import { loadAppConfig, buildWsUrl } from '../config';
import { isTauri } from '../utils/tauri';
import { createReconnectScheduler } from '../utils/reconnect';
import { buildSFUHandlers } from './sfu-handlers';
import type { EngineKind, ParticipantUI } from '../types';
import type { RoomSlug } from '../rooms';
import type { MicGraph } from '../audio/mic-graph';

type UseSessionManagerDeps = {
  audio: ReturnType<typeof useAudioEngine>;
  sfu: ReturnType<typeof useSFU>;
  onTauriToggleMute: () => void;
};

type UseSessionManagerReturn = {
  join: (name: string) => Promise<void>;
  leave: () => void;
  switchRoom: (slug: RoomSlug) => Promise<void>;
  getPeerId: () => string | null;
  setMicEnabled: (enabled: boolean) => void;
  switchEngine: (engine: EngineKind) => Promise<void>;
  switchMicDevice: () => Promise<void>;
  setRemoteDisplayName: (name: string) => void;
  sendSetState: (selfMuted: boolean, deafened: boolean) => void;
  sendChat: (text: string, clientMsgId: string, attachments?: Attachment[]) => void;
  sendChatDelete: (id: string) => boolean;
  sendPing: (targetId: string) => void;
  startScreenShare: () => Promise<void>;
  stopScreenShare: () => void;
  updateScreenShareParams: () => Promise<void>;
  changeScreenShareMode: (mode: ShareMode) => Promise<void>;
  subscribeScreenShare: (publisherId: string) => void;
  unsubscribeScreenShare: (publisherId: string) => void;
  getRoomId: () => string;
  handleChatReceive: (data: ChatPayload) => void;
  handleChatDelete: (data: ChatDeletedPayload) => void;
  handlePingReceive: (data: PingPayload) => void;
};

const RECONNECT_DELAYS_MS = [1000, 2000, 4000, 8000, 15000, 30000, 30000] as const;

export function useSessionManager({
  audio,
  sfu,
  onTauriToggleMute,
}: UseSessionManagerDeps): UseSessionManagerReturn {
  const getStore = useStore.getState;

  // Imperative refs — internal only, not exposed to callers.
  const micGraphRef = useRef<MicGraph | null>(null);
  const peerIdRef = useRef<string | null>(null);

  // Stable per-install id. Created once on first launch, persisted in
  // localStorage. Survives reconnects, deploys, and OS restarts; only
  // clearing browser storage / reinstalling resets it. Sent in every
  // `hello` so peers can key per-peer UI prefs (e.g. volume) by it.
  const clientIdRef = useRef<string>(loadOrCreateClientId());

  // Reconnect state.
  const userLeavingRef = useRef<boolean>(false);
  const lastDisplayNameRef = useRef<string>('');

  // Config loaded at mount — needed by connectSfu.
  const configRef = useRef<{ iceServers: RTCIceServer[] } | null>(null);

  // Forward ref for handleJoin so the auto-rejoin effect can call it after
  // handleJoin is defined (avoids a chicken-and-egg dep cycle).
  const handleJoinRef = useRef<((name: string) => Promise<void>) | null>(null);

  // Stable ref for the latest onTauriToggleMute callback — the Tauri effect
  // runs only once (empty deps) so it must not close over a stale value.
  const onTauriToggleMuteRef = useRef(onTauriToggleMute);
  useEffect(() => {
    onTauriToggleMuteRef.current = onTauriToggleMute;
  }, [onTauriToggleMute]);

  // connectSfu is defined below but the scheduler's onAttempt callback needs
  // to call it. A ref breaks the forward-reference cycle.
  const connectSfuRef = useRef<(graph: MicGraph, display: string) => Promise<void>>(async () => {
    throw new Error('connectSfu not yet initialised');
  });

  // ---- Action surface helpers (stable, ref-based) ----

  const getPeerId = useCallback((): string | null => peerIdRef.current, []);

  const setMicEnabled = useCallback(
    (enabled: boolean): void => {
      const graph = micGraphRef.current;
      if (!graph) return;
      for (const track of graph.processedLocalStream.getAudioTracks()) {
        track.enabled = enabled;
      }
      const pid = peerIdRef.current;
      if (pid) {
        getStore().updateParticipant(pid, {
          selfMuted: !enabled,
          speaking: !enabled ? false : undefined,
        });
      }
    },
    [getStore],
  );

  // Single attach point for the local-mic speaking-detect RAF loop.
  // Used by both initial join and engine hot-swap; keeps the speaking-state
  // write path identical and avoids two RAF loops racing on the same graph.
  const attachSpeakingLoop = useCallback(
    (graph: MicGraph): void => {
      audio.startSpeaking(
        graph,
        () => useStore.getState().selfMuted,
        (speaking) => {
          const pid = peerIdRef.current;
          if (!pid) return;
          const current = useStore.getState().participants[pid];
          if (current && current.speaking !== speaking) {
            getStore().updateParticipant(pid, { speaking });
          }
        },
      );
    },
    [audio, getStore],
  );

  const switchEngine = useCallback(
    async (engine: EngineKind): Promise<void> => {
      const graph = await audio.rebuildLocalAudio(engine, useStore.getState().selfMuted, () =>
        sfu.getPeerConnection(),
      );
      micGraphRef.current = graph;
      attachSpeakingLoop(graph);
    },
    [audio, sfu, attachSpeakingLoop],
  );

  const switchMicDevice = useCallback(async (): Promise<void> => {
    const s = useStore.getState();
    const graph = await audio.switchMicDevice(s.engine, s.selfMuted, () => sfu.getPeerConnection());
    micGraphRef.current = graph;
    attachSpeakingLoop(graph);
  }, [audio, sfu, attachSpeakingLoop]);

  const setRemoteDisplayName = useCallback(
    (name: string): void => {
      // Track latest name so reconnect scheduler uses the up-to-date value,
      // not whatever was passed to the original join.
      lastDisplayNameRef.current = name;
      if (name.trim()) saveDisplayName(name.trim());
      sfu.getClient()?.setDisplayName(name);
      const pid = peerIdRef.current;
      if (pid) {
        getStore().updateParticipant(pid, { display: name });
      }
    },
    [sfu, getStore],
  );

  const sendSetState = useCallback(
    (selfMuted: boolean, deafened: boolean): void => {
      sfu.getClient()?.sendSetState(selfMuted, deafened);
    },
    [sfu],
  );

  // Tracks the currently selected room slug. Picker is disabled while joined,
  // so this only changes between sessions — chat history key won't shift mid-call.
  // Kept in sync with the store via subscription below so idle lurker chats land
  // in the right bucket.
  const roomSlugRef = useRef<string>(useStore.getState().roomSlug);
  useEffect(() => {
    return useStore.subscribe((state, prev) => {
      if (state.roomSlug !== prev.roomSlug) {
        roomSlugRef.current = state.roomSlug;
      }
    });
  }, []);

  const getRoomId = useCallback(() => roomSlugRef.current, []);

  const pingClearRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const persistDebounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const schedulePersist = useCallback((): void => {
    if (persistDebounceRef.current !== null) clearTimeout(persistDebounceRef.current);
    persistDebounceRef.current = setTimeout(() => {
      persistDebounceRef.current = null;
      useStore.getState().persistChat(roomSlugRef.current);
    }, 250);
  }, []);

  const sendChat = useCallback(
    (text: string, clientMsgId: string, attachments?: Attachment[]): void => {
      sfu.getClient()?.sendChat({ text, clientMsgId, attachments });
    },
    [sfu],
  );

  const sendChatDelete = useCallback(
    (id: string): boolean => sfu.getClient()?.sendChatDelete(id) ?? false,
    [sfu],
  );

  const sendPing = useCallback(
    (targetId: string): void => {
      sfu.getClient()?.sendPing(targetId);
    },
    [sfu],
  );

  const handleChatDelete = useCallback((data: ChatDeletedPayload): void => {
    useStore.getState().chatDelete(roomSlugRef.current, data.id);
  }, []);

  const handleChatReceive = useCallback(
    (data: ChatPayload): void => {
      const sender = useStore.getState().participants[data.from];
      // Prefer server-snapshotted name (survives lurker / post-leave lookup).
      // Fall back to participants map entry for older server versions.
      useStore.getState().chatReceive(roomSlugRef.current, {
        ...data,
        pending: false,
        senderName: data.senderName ?? sender?.display,
        senderClientId: sender?.clientId,
      });
      schedulePersist();
    },
    [schedulePersist],
  );

  const handlePingReceive = useCallback(({ fromName }: PingPayload): void => {
    const s = useStore.getState(); // snapshot read, not subscription
    if (s.muteIncomingPings) return;
    s.setIncomingPing({ fromName, at: Date.now() });
    if (pingClearRef.current !== null) clearTimeout(pingClearRef.current);
    pingClearRef.current = setTimeout(() => {
      pingClearRef.current = null;
      useStore.getState().clearIncomingPing();
    }, 4000);
    if (s.pingSoundEnabled) playPing();
    flashFavicon();
    void flashAttention({ tray: true, window: s.pingWindowFlashEnabled });
  }, []);

  // ---- Reconnect scheduler ----
  // Created once; options that need freshness use refs (isLeaving, onAttempt).

  const reconnectSchedulerRef = useRef(
    createReconnectScheduler({
      delays: RECONNECT_DELAYS_MS,
      isLeaving: () => userLeavingRef.current,
      onExhausted: () => {
        getStore().setStatus('Не удалось переподключиться. Перезайдите вручную.', true, true);
      },
      onAttempt: async () => {
        const graph = micGraphRef.current;
        if (!graph) {
          // Mic gone — treat as terminal; reset so manual re-join starts fresh.
          reconnectSchedulerRef.current.reset();
          throw new Error('mic graph gone');
        }
        const cfg = configRef.current;
        if (!cfg) throw new Error('Config not loaded');

        const liveClient = sfu.getClient();
        const screenToken = liveClient?.getScreenShareToken() ?? null;
        const hadActiveShare = liveClient?.isPublishingScreenShare() ?? false;
        if (liveClient && screenToken && hadActiveShare) {
          audio.cleanupAllRemote();
          getStore().clearParticipants();
          peerIdRef.current = null;
          try {
            await liveClient.reconnect({
              wsUrl: buildWsUrl(roomSlugRef.current),
              iceServers: cfg.iceServers,
              localStream: graph.processedLocalStream,
              displayName: lastDisplayNameRef.current,
              clientId: clientIdRef.current,
            });
            await liveClient.resumeScreenShare(screenToken);
            const s = useStore.getState();
            if (s.selfMuted || s.deafened) {
              liveClient.sendSetState(s.selfMuted, s.deafened);
            }
            return;
          } catch (err) {
            console.warn(
              '[session] screen-share resume failed; falling back to cold reconnect',
              err,
            );
            useScreenShareStore.getState().setMyStatus('idle');
            // fall through
          }
        }

        sfu.disconnect();
        audio.cleanupAllRemote();
        getStore().clearParticipants();
        peerIdRef.current = null;
        await connectSfuRef.current(graph, lastDisplayNameRef.current);
      },
    }),
  );

  // ---- SFU connection helper ----

  const connectSfu = useCallback(
    async (graph: MicGraph, display: string): Promise<void> => {
      const cfg = configRef.current;
      if (!cfg) throw new Error('Config not loaded');

      const client = sfu.createClient(
        buildSFUHandlers({
          display,
          audio,
          sfu,
          getStore,
          handleChatReceive,
          handleChatDelete,
          handlePingReceive,
          peerIdRef,
          clientIdRef,
          reconnectSchedulerRef,
          userLeavingRef,
        }),
      );

      await client.connect({
        wsUrl: buildWsUrl(roomSlugRef.current),
        iceServers: cfg.iceServers,
        localStream: graph.processedLocalStream,
        displayName: display,
        clientId: clientIdRef.current,
      });

      const s = useStore.getState();
      const track = graph.processedLocalStream.getAudioTracks()[0];
      if (track) track.enabled = !s.selfMuted;
      if (s.selfMuted || s.deafened) {
        client.sendSetState(s.selfMuted, s.deafened);
      }
    },
    [audio, sfu, handleChatReceive, handleChatDelete, handlePingReceive, getStore],
  );

  useEffect(() => {
    connectSfuRef.current = connectSfu;
  }, [connectSfu]);

  // ---- Leave ----

  const handleLeave = useCallback(() => {
    userLeavingRef.current = true;
    reconnectSchedulerRef.current.reset();
    sfu.disconnect();
    audio.fullCleanup();
    micGraphRef.current = null;
    peerIdRef.current = null;
    useScreenShareStore.getState().clearShares();
    useScreenShareStore.getState().setMyStatus('idle');
    const prev = useStore.getState().participants;
    const nextRec: Record<string, ParticipantUI> = {};
    for (const [id, p] of Object.entries(prev)) {
      if (p.isSelf) {
        nextRec[id] = { ...p, chatOnly: true, speaking: false, hasStream: false };
      }
    }
    useStore.setState({ participants: nextRec });
    getStore().setJoinState('idle');
    getStore().setStatus('Отключено');
  }, [sfu, audio, getStore]);

  // ---- Join ----

  const handleJoin = useCallback(
    async (name: string) => {
      if (getStore().joinState === 'joined') return;
      const cfg = configRef.current;
      if (!cfg) {
        getStore().setStatus('Конфигурация не загружена', true);
        return;
      }

      // Empty input falls back to the persisted identity (auto-generated on
      // first launch, then stable across reconnects/reloads/server switches).
      // Typed names overwrite it; renaming is just another saveDisplayName.
      const display = name.trim() || loadOrCreateDisplayName(makeGuestName);
      saveDisplayName(display);

      // Capture the slug at join time — chat history bucket stays stable even
      // if the user picks a different room between sessions (shouldn't be
      // possible while joined, but defend anyway).
      roomSlugRef.current = useStore.getState().roomSlug; // snapshot read, not subscription

      userLeavingRef.current = false;
      reconnectSchedulerRef.current.reset();
      lastDisplayNameRef.current = display;

      getStore().setJoinState('joining');
      getStore().loadChatRoom(roomSlugRef.current);
      getStore().setStatus('Запрашиваю микрофон…');

      // Hot-swap path: join with engine=off if WASM not ready yet, rebuild in
      // background so users enter the room without waiting for the vendor fetch.
      const targetEngine = getStore().engine;
      const denoiserReady = isEngineReady(targetEngine);
      const initialEngine: EngineKind = denoiserReady ? targetEngine : 'off';
      if (!denoiserReady) {
        void preloadEngine(targetEngine);
      }

      try {
        const graph = await audio.prepareLocalAudio(initialEngine, (stage) => {
          if (stage === 'mic-ready' && initialEngine !== 'off') {
            getStore().setStatus('Загружаю шумоподавление…');
          }
        });
        micGraphRef.current = graph;

        getStore().setStatus('Подключаюсь…');
        await connectSfu(graph, display);

        getStore().setJoinState('joined');
        if (!denoiserReady) {
          getStore().setStatus('Подключено. Шумоподавление загружается…', false, true);
        } else {
          getStore().setStatus('Подключено', false, true);
        }

        if (!denoiserReady) {
          // Capture the graph identity so leave-during-load (or leave+rejoin
          // before WASM resolves) cannot rebuild on a torn-down / replaced graph.
          const pendingGraph = graph;
          void preloadEngine(targetEngine).then(async () => {
            if (micGraphRef.current !== pendingGraph) return;
            const s = useStore.getState();
            if (s.joinState !== 'joined') return;
            if (s.engine !== targetEngine) return;
            try {
              await switchEngine(targetEngine);
              getStore().setStatus(`Шумоподавление: ${formatEngine(targetEngine)}`, false, true);
            } catch (err) {
              getStore().setStatus(
                `Не удалось включить ${formatEngine(targetEngine)}: ${err instanceof Error ? err.message : String(err)}`,
                true,
                true,
              );
            }
          });
        }

        attachSpeakingLoop(graph);
      } catch (error) {
        handleLeave();
        getStore().setStatus(error instanceof Error ? error.message : String(error), true);
      }
    },
    [audio, connectSfu, handleLeave, switchEngine, attachSpeakingLoop, getStore],
  );

  useEffect(() => {
    handleJoinRef.current = handleJoin;
  }, [handleJoin]);

  // ---- Config load + auto-rejoin on mount ----

  useEffect(() => {
    const shouldRejoin = consumeRejoinFlag();
    loadAppConfig()
      .then((cfg) => {
        configRef.current = cfg;
        useStore.setState({ role: cfg.role, configReady: true });
        getStore().setStatus('Готово');
        if (shouldRejoin) {
          void handleJoinRef.current?.(loadOrCreateDisplayName(makeGuestName));
        }
      })
      .catch((err: unknown) => {
        getStore().setStatus(err instanceof Error ? err.message : String(err), true);
      });
    // Warm up the selected engine while the user fills in their name.
    preloadEngine(useStore.getState().engine);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // ---- Tauri global hotkey bridge ----
  // OS-level rdev listener emits "toggle-mute" when the window is unfocused.
  // Checks joinState as a session-level gate; delegates to the callback for
  // the actual mute logic (owned by App).

  useEffect(() => {
    if (!isTauri()) return;
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void import('@tauri-apps/api/event')
      .then(({ listen }) =>
        listen('toggle-mute', () => {
          if (useStore.getState().joinState !== 'joined') return;
          onTauriToggleMuteRef.current();
        }).then((off) => {
          if (cancelled) off();
          else unlisten = off;
        }),
      )
      .catch((err: unknown) => {
        console.error('[hotkey-bridge] toggle-mute listen failed:', err);
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  const rotateRoomKeepingMic = useCallback(
    async (slug: RoomSlug, display: string, mic: MicGraph): Promise<void> => {
      reconnectSchedulerRef.current.reset();
      sfu.disconnect();
      audio.cleanupAllRemote();
      useStore.getState().clearParticipants();
      peerIdRef.current = null;

      useStore.getState().setRoomSlug(slug);
      useStore.getState().loadChatRoom(slug);
      useStore.getState().setJoinState('joining');
      useStore.getState().setStatus('Подключаюсь…');
      lastDisplayNameRef.current = display;

      try {
        await connectSfu(mic, display);
        useStore.getState().setJoinState('joined');
        useStore.getState().setStatus('Подключено', false, true);
      } catch (error) {
        handleLeave();
        useStore.getState().setStatus(error instanceof Error ? error.message : String(error), true);
      }
    },
    [audio, connectSfu, handleLeave, sfu],
  );

  const switchRoom = useCallback(
    async (slug: RoomSlug): Promise<void> => {
      const s = useStore.getState(); // snapshot read, not subscription
      if (slug === s.roomSlug) return;
      if (s.joinState === 'joining') return;
      if (s.joinState === 'idle') {
        s.setRoomSlug(slug);
        return;
      }
      const display = lastDisplayNameRef.current || loadOrCreateDisplayName(makeGuestName);
      const liveMic = micGraphRef.current;
      if (liveMic) {
        await rotateRoomKeepingMic(slug, display, liveMic);
        return;
      }
      handleLeave();
      useStore.getState().setRoomSlug(slug);
      await handleJoin(display);
    },
    [handleLeave, handleJoin, rotateRoomKeepingMic],
  );

  const startScreenShare = useCallback(async (): Promise<void> => {
    const client = sfu.getClient();
    if (!client) throw new Error('Не подключён');
    const share = useScreenShareStore.getState();
    if (share.myStatus !== 'idle') return;
    share.setMyStatus('starting');
    try {
      await client.startScreenShare();
      useScreenShareStore.getState().setMyStatus('publishing');
    } catch (err) {
      const store = useScreenShareStore.getState();
      store.setMyStatus('idle');
      store.setMyStream(null);
      const name = err instanceof DOMException ? err.name : '';
      if (name === 'NotAllowedError' || name === 'AbortError') return;
      if (err instanceof Error && err.message === SCREEN_SHARE_NO_CODEC) {
        getStore().setStatus(
          'Браузер не поддерживает кодеки AV1/VP9 для демонстрации экрана.',
          true,
          true,
        );
        return;
      }
      throw err;
    }
  }, [sfu, getStore]);

  const stopScreenShare = useCallback((): void => {
    const client = sfu.getClient();
    if (!client) return;
    useScreenShareStore.getState().setMyStatus('stopping');
    client.stopScreenShare();
  }, [sfu]);

  const updateScreenShareParams = useCallback(async (): Promise<void> => {
    const client = sfu.getClient();
    if (!client) return;
    if (!client.isPublishingScreenShare()) return;
    await client.updateScreenShareParams();
  }, [sfu]);

  const changeScreenShareMode = useCallback(
    async (mode: ShareMode): Promise<void> => {
      const client = sfu.getClient();
      if (!client) return;
      if (!client.isPublishingScreenShare()) return;
      await client.changeScreenShareMode(mode);
    },
    [sfu],
  );

  const subscribeScreenShare = useCallback(
    (publisherId: string): void => {
      sfu.getClient()?.subscribeScreenShare(publisherId);
    },
    [sfu],
  );

  const unsubscribeScreenShare = useCallback(
    (publisherId: string): void => {
      sfu.getClient()?.unsubscribeScreenShare(publisherId);
    },
    [sfu],
  );

  return {
    join: handleJoin,
    leave: handleLeave,
    switchRoom,
    getPeerId,
    setMicEnabled,
    switchEngine,
    switchMicDevice,
    setRemoteDisplayName,
    sendSetState,
    sendChat,
    sendChatDelete,
    sendPing,
    startScreenShare,
    stopScreenShare,
    updateScreenShareParams,
    changeScreenShareMode,
    subscribeScreenShare,
    unsubscribeScreenShare,
    getRoomId,
    handleChatReceive,
    handleChatDelete,
    handlePingReceive,
  };
}
