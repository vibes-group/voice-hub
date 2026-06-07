import { useCallback, useEffect, useRef, useState } from 'react';
import './styles/main.css';
import { useStore, selectSelfPeerId } from './store/useStore';
import { useAudioEngine } from './hooks/useAudioEngine';
import { formatEngine, preloadEngine } from './audio/engine';
import { useSFU } from './hooks/useSFU';
import { useSessionManager } from './hooks/useSessionManager';
import { useGlobalShortcut } from './hooks/useShortcut';
import { loadOrCreateDisplayName, makeGuestName, saveDisplayName } from './utils/storage';
import { playMuteSound, playUnmuteSound } from './audio/feedback-sounds';
import type { EngineKind } from './types';

import { TopBar } from './components/TopBar';
import { SessionCard } from './components/SessionCard';
import { AudioCard } from './components/AudioCard';
import { HotkeyCard } from './components/HotkeyCard';
import { ParticipantsCard } from './components/ParticipantsCard';
import { ChatPanel } from './components/ChatPanel';
import { UpdateBanner } from './components/UpdateBanner';
import { Footer } from './components/Footer';
import { PingToast } from './components/PingToast';
import { PingCard } from './components/PingCard';
import { ScreenShareButton } from './components/ScreenShareButton';
import { ScreenShareGallery } from './components/ScreenShareGallery';
import { ScreenShareFocused } from './components/ScreenShareFocused';
import { useScreenShareStore } from './store/useScreenShareStore';
import { useAppVersion } from './hooks/useAppVersion';
import { useLurkerWS } from './hooks/useLurkerWS';

export function App() {
  // App does not render any store field directly — children subscribe per-card.
  // Callbacks read fresh state via useStore.getState() to avoid re-rendering App
  // (and recreating every handler) on every speaking/participant update.
  const audio = useAudioEngine();
  const sfu = useSFU();
  const { bootVersion, update, reload, applyDesktopUpdate, desktopApplyState } = useAppVersion();

  // Display name local state (synced to localStorage).
  const [displayName, setDisplayName] = useState<string>(() =>
    loadOrCreateDisplayName(makeGuestName),
  );

  // ---- Toggle handlers ----

  // Single dedup gate shared between in-window keyboard listener and the Tauri
  // OS-level event bridge. Each path has its own short-circuit; this is the
  // authoritative gate against a focus-race where both paths fire together.
  const lastToggleAtRef = useRef(0);
  const TOGGLE_COOLDOWN_MS = 60;

  // triggerToggleSelfMute needs session, but session is defined below.
  // We break the cycle with a stable ref, same pattern as the hook itself.
  const handleToggleSelfMuteRef = useRef<() => void>(() => undefined);

  const triggerToggleSelfMute = useCallback(() => {
    const now = performance.now();
    if (now - lastToggleAtRef.current < TOGGLE_COOLDOWN_MS) return;
    lastToggleAtRef.current = now;
    handleToggleSelfMuteRef.current();
  }, []);

  useGlobalShortcut(triggerToggleSelfMute);

  // ---- Session manager ----
  // Owns join/leave/reconnect/config/Tauri event subscription/mic actions.

  const session = useSessionManager({
    audio,
    sfu,
    onTauriToggleMute: triggerToggleSelfMute,
  });

  const handleToggleSelfMute = useCallback(() => {
    const s = useStore.getState();
    const joined = session.getPeerId() !== null;
    if (s.deafened) {
      // Exit deafen as a side effect of unmuting (matches Discord).
      s.setDeafened(false);
      audio.applyAllRemoteGains();
    }
    const nextMuted = !s.selfMuted;
    s.setSelfMuted(nextMuted);
    if (joined) {
      session.setMicEnabled(!nextMuted);
      session.sendSetState(nextMuted, false);
    }
    if (nextMuted) playMuteSound();
    else playUnmuteSound();
  }, [audio, session]);

  // Keep the ref in sync so triggerToggleSelfMute (defined before session) can
  // call the latest version without capturing session in its own dep array.
  handleToggleSelfMuteRef.current = handleToggleSelfMute;

  const handleToggleDeafen = useCallback(() => {
    const s = useStore.getState();
    const joined = session.getPeerId() !== null;
    if (s.deafened) {
      s.setDeafened(false);
      s.setSelfMuted(s.preDeafenSelfMuted);
      if (joined) {
        session.setMicEnabled(!s.preDeafenSelfMuted);
        session.sendSetState(s.preDeafenSelfMuted, false);
      }
    } else {
      s.enterDeafen();
      if (joined) {
        session.setMicEnabled(false);
        session.sendSetState(true, true);
      }
    }
    audio.applyAllRemoteGains();
  }, [audio, session]);

  // ---- Engine switch ----

  const handleEngineSelect = useCallback(
    async (engine: EngineKind) => {
      const s = useStore.getState();
      if (engine === s.engine) return;
      s.setEngine(engine);
      preloadEngine(engine);
      if (s.joinState !== 'joined') {
        s.setStatus(`Шумоподавление: ${formatEngine(engine)}`);
        return;
      }
      s.setStatus(`Переключаюсь на ${formatEngine(engine)}…`);
      try {
        await session.switchEngine(engine);
        useStore.getState().setStatus(`Шумоподавление: ${formatEngine(engine)}`, false, true);
      } catch (err) {
        useStore
          .getState()
          .setStatus(
            `Не удалось переключить шумоподавление: ${err instanceof Error ? err.message : String(err)}`,
            true,
            true,
          );
      }
    },
    [session],
  );

  // ---- Mic device switch ----

  const handleMicDeviceSelect = useCallback(
    async (deviceId: string | null) => {
      const s = useStore.getState();
      if (deviceId === s.micDeviceId) return;
      s.setMicDeviceId(deviceId);
      if (s.joinState !== 'joined') return;
      s.setStatus('Переключаю микрофон…');
      try {
        await session.switchMicDevice();
        useStore.getState().setStatus('Микрофон переключён.', false, true);
      } catch (err) {
        useStore
          .getState()
          .setStatus(
            `Не удалось переключить микрофон: ${err instanceof Error ? err.message : String(err)}`,
            true,
            true,
          );
      }
    },
    [session],
  );

  // ---- Audio controls ----

  const handleSendVolumeChange = useCallback(
    (v: number) => {
      useStore.getState().setSendVolume(v);
      audio.updateSendGain();
    },
    [audio],
  );

  const handleOutputVolumeChange = useCallback(
    (v: number) => {
      useStore.getState().setOutputVolume(v);
      audio.applyAllRemoteGains();
    },
    [audio],
  );

  const handleAudioReset = useCallback(() => {
    const s = useStore.getState();
    s.setSendVolume(100);
    s.setOutputVolume(100);
    audio.updateSendGain();
    audio.applyAllRemoteGains();

    if (s.engine !== 'rnnoise') {
      void handleEngineSelect('rnnoise');
    }
    if (s.micDeviceId !== null) {
      void handleMicDeviceSelect(null);
    }

    s.setStatus('Настройки звука сброшены.', false, s.joinState === 'joined');
  }, [audio, handleEngineSelect, handleMicDeviceSelect]);

  const handleStatusMessage = useCallback((msg: string) => {
    const s = useStore.getState();
    s.setStatus(msg, false, s.joinState === 'joined');
  }, []);

  // ---- Display name sync back to SFU ----

  const handleDisplayNameChange = useCallback(
    (value: string) => {
      setDisplayName(value);
      saveDisplayName(value);
      if (useStore.getState().joinState === 'joined' && value.trim()) {
        session.setRemoteDisplayName(value.trim());
      }
    },
    [session],
  );

  const joinState = useStore((s) => s.joinState);
  const voiceActive = joinState === 'joined' || joinState === 'joining';
  const roomSlug = useStore((s) => s.roomSlug);

  const lurker = useLurkerWS({
    displayName,
    onChat: session.handleChatReceive,
    onPing: session.handlePingReceive,
    voiceActive,
  });

  const handleChatSend = useCallback(
    (text: string, clientMsgId: string) => {
      if (voiceActive) {
        session.sendChat(text, clientMsgId);
      } else {
        lurker.sendChat({ text, clientMsgId });
      }
    },
    [voiceActive, session, lurker],
  );

  const handlePingUser = useCallback(
    (targetId: string): void => {
      const s = useStore.getState(); // snapshot read, not subscription
      const last = s.lastPingSentByTarget.get(targetId) ?? 0;
      if (Date.now() - last < 10000) return;
      if (voiceActive) session.sendPing(targetId);
      else lurker.sendPing(targetId);
      s.markPingSent(targetId);
    },
    [voiceActive, session, lurker],
  );

  const handleTileClick = useCallback(
    (publisherId: string) => {
      const share = useScreenShareStore.getState();
      const prev = share.focusedId;
      if (prev === publisherId) return;
      if (prev) session.unsubscribeScreenShare(prev);
      share.setFocused(publisherId);
      session.subscribeScreenShare(publisherId);
    },
    [session],
  );

  const handleFocusedClose = useCallback(() => {
    const share = useScreenShareStore.getState();
    const focused = share.focusedId;
    if (focused) session.unsubscribeScreenShare(focused);
    share.setFocused(null);
  }, [session]);

  const focusedId = useScreenShareStore((s) => s.focusedId);

  useEffect(() => {
    if (!focusedId) return;
    const current = focusedId;
    const selfId = selectSelfPeerId(useStore.getState());
    function onKey(e: KeyboardEvent) {
      if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
      // Don't hijack arrows while the user is typing (chat composer etc.).
      const a = document.activeElement as HTMLElement | null;
      if (a && (a.tagName === 'INPUT' || a.tagName === 'TEXTAREA' || a.isContentEditable)) {
        return;
      }
      const ids = Array.from(useScreenShareStore.getState().shares.keys()).filter(
        (id) => id !== selfId,
      );
      if (ids.length < 2) return;
      const idx = ids.indexOf(current);
      if (idx < 0) return;
      const next =
        e.key === 'ArrowRight'
          ? ids[(idx + 1) % ids.length]
          : ids[(idx - 1 + ids.length) % ids.length];
      handleTileClick(next);
    }
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [focusedId, handleTileClick]);

  return (
    <>
      <PingToast />
      <ScreenShareFocused onClose={handleFocusedClose} />
      <main
        className="grid gap-4 mx-auto
          w-[min(1560px,100%)] px-5 pt-5 pb-5
          max-[640px]:px-3 max-[640px]:pt-3 max-[640px]:pb-3"
      >
        <TopBar />
        <UpdateBanner
          update={update}
          reload={reload}
          applyDesktopUpdate={applyDesktopUpdate}
          desktopApplyState={desktopApplyState}
        />
        <div className="grid gap-4 grid-cols-[400px_minmax(0,1fr)_400px] max-[1340px]:grid-cols-1 items-start">
          <div className="flex flex-col gap-4 min-h-0" style={{ height: 840 }}>
            <SessionCard
              onJoin={session.join}
              onLeave={session.leave}
              onToggleSelfMute={handleToggleSelfMute}
              onToggleDeafen={handleToggleDeafen}
              displayName={displayName}
              onDisplayNameChange={handleDisplayNameChange}
            />
            <ParticipantsCard
              onRemoteGainChange={audio.applyAllRemoteGains}
              onPingUser={handlePingUser}
              onRoomSelect={(slug) => void session.switchRoom(slug)}
            />
          </div>
          <div className="flex flex-col gap-4 min-h-0" style={{ height: 840 }}>
            {voiceActive && (
              <ScreenShareButton
                onStart={session.startScreenShare}
                onStop={session.stopScreenShare}
                onUpdateParams={session.updateScreenShareParams}
                onShareModeChange={session.changeScreenShareMode}
              />
            )}
            <ScreenShareGallery onTileClick={handleTileClick} />
            <ChatPanel roomId={roomSlug} onSend={handleChatSend} />
          </div>
          <div className="grid gap-4 content-start">
            <AudioCard
              onEngineSelect={handleEngineSelect}
              onMicDeviceSelect={handleMicDeviceSelect}
              onSendVolumeChange={handleSendVolumeChange}
              onOutputVolumeChange={handleOutputVolumeChange}
              onReset={handleAudioReset}
            />
            <PingCard />
            <HotkeyCard onStatusMessage={handleStatusMessage} />
          </div>
        </div>
        <Footer uiVersion={bootVersion} />
      </main>
    </>
  );
}
