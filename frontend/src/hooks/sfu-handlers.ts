import type { MutableRefObject } from 'react';
import { useStore } from '../store/useStore';
import { useScreenShareStore, type ScreenShare } from '../store/useScreenShareStore';
import type { useAudioEngine } from './useAudioEngine';
import type { useSFU } from './useSFU';
import { loadPeerVolume } from '../utils/storage';
import { retryPendingChats } from '../utils/chat-retry';
import type { ChatPayload, ChatDeletedPayload, PeerInfo, PingPayload } from '../sfu/protocol';
import type { SFUHandlers } from '../sfu/client';
import { screenShareErrorRu } from '../screenshare/errors';
import type { ReconnectScheduler } from '../utils/reconnect';
import type { ParticipantUI } from '../types';

// Roster entry for a remote peer from welcome / peer-joined (voice or lurker
// WS), restoring our saved volume for them by clientId.
export function upsertRemotePeer(p: PeerInfo, extra?: Partial<ParticipantUI>): void {
  const stored = p.clientId ? loadPeerVolume(p.clientId) : null;
  useStore.getState().upsertParticipant({
    id: p.id,
    display: p.displayName ?? `peer-${p.id}`,
    clientId: p.clientId,
    remoteMuted: p.selfMuted ?? false,
    remoteDeafened: p.deafened ?? false,
    chatOnly: p.chatOnly ?? false,
    ...extra,
    ...(stored !== null ? { localVolume: stored } : {}),
  });
}

function shareOf(p: PeerInfo): ScreenShare {
  return {
    publisherId: p.id,
    hasSystemAudio: p.screenSharingHasAudio ?? false,
    videoCodec: p.screenSharingVideoCodec,
  };
}

function addVoicePeer(p: PeerInfo): void {
  upsertRemotePeer(p, { screenSharing: p.screenSharing ?? false });
  if (p.screenSharing) useScreenShareStore.getState().upsertShare(shareOf(p));
}

type SFUHandlerDeps = {
  display: string;
  audio: ReturnType<typeof useAudioEngine>;
  sfu: ReturnType<typeof useSFU>;
  handleChatReceive: (data: ChatPayload) => void;
  handleChatDelete: (data: ChatDeletedPayload) => void;
  handlePingReceive: (data: PingPayload) => void;
  peerIdRef: MutableRefObject<string | null>;
  clientIdRef: MutableRefObject<string>;
  reconnectSchedulerRef: MutableRefObject<ReconnectScheduler>;
  userLeavingRef: MutableRefObject<boolean>;
};

export function buildSFUHandlers(deps: SFUHandlerDeps): Partial<SFUHandlers> {
  const {
    display,
    audio,
    sfu,
    handleChatReceive,
    handleChatDelete,
    handlePingReceive,
    peerIdRef,
    clientIdRef,
    reconnectSchedulerRef,
    userLeavingRef,
  } = deps;
  const getStore = useStore.getState;
  return {
    onState: (s) => {
      if (s === 'connected') {
        reconnectSchedulerRef.current.reset();
        getStore().setStatus('Подключено', false, true);
      } else if (s === 'failed' || s === 'closed') {
        if (getStore().joinState === 'joined' && !userLeavingRef.current) {
          const nextAttempt = reconnectSchedulerRef.current.attemptIndex + 1;
          getStore().setStatus(
            `Соединение оборвалось, переподключаюсь (попытка ${nextAttempt})…`,
            true,
            true,
          );
          reconnectSchedulerRef.current.schedule();
        }
      }
    },
    onWelcome: ({ id, peers }) => {
      peerIdRef.current = id;
      getStore().clearParticipants();
      getStore().upsertParticipant({
        id,
        display,
        isSelf: true,
        clientId: clientIdRef.current,
        chatOnly: false,
      });
      useScreenShareStore.getState().clearShares();
      for (const p of peers ?? []) addVoicePeer(p);
      retryPendingChats((payload) => sfu.getClient()?.sendChat(payload), clientIdRef.current);
    },
    onPeerJoined: addVoicePeer,
    onPeerLeft: ({ id }) => {
      audio.detachRemoteStream(id);
      getStore().removeParticipant(id);
      useScreenShareStore.getState().removeShare(id);
    },
    onPeerInfo: (p) => {
      const patch: Partial<ParticipantUI> = { screenSharing: Boolean(p.screenSharing) };
      if (p.displayName) patch.display = p.displayName;
      if (p.clientId) patch.clientId = p.clientId;
      getStore().updateParticipant(p.id, patch);
      const share = useScreenShareStore.getState();
      if (p.screenSharing) share.upsertShare(shareOf(p));
      else share.removeShare(p.id);
    },
    onPeerState: ({ id, selfMuted, deafened }) => {
      getStore().updateParticipant(id, { remoteMuted: selfMuted, remoteDeafened: deafened });
    },
    onChat: handleChatReceive,
    onChatDeleted: handleChatDelete,
    onPing: handlePingReceive,
    onTrack: ({ track, stream, peerId }) => {
      if (track.kind === 'audio') {
        getStore().upsertParticipant({ id: peerId, hasStream: true });
        audio.attachRemoteStream(peerId, stream);
      }
    },
    onTrackRemoved: ({ peerId }) => {
      audio.detachRemoteStream(peerId);
      if (getStore().participants[peerId])
        getStore().updateParticipant(peerId, { hasStream: false });
    },
    onScreenShareAvailable: ({ publisherId, hasSystemAudio, videoCodec }) => {
      useScreenShareStore.getState().upsertShare({ publisherId, hasSystemAudio, videoCodec });
    },
    onScreenShareEnded: ({ publisherId }) => {
      const store = useScreenShareStore.getState();
      store.removeShare(publisherId);
      if (publisherId === peerIdRef.current && store.myStatus === 'publishing') {
        store.setMyStatus('idle');
        getStore().setStatus('Демонстрация прервана разрывом соединения.', true, true);
      }
    },
    onScreenShareError: ({ publisherId, reason }) => {
      const store = useScreenShareStore.getState();
      if (publisherId) store.removeShare(publisherId);
      if (!publisherId || reason === 'already-publishing' || reason === 'internal') {
        store.setMyStatus('idle');
      }
      const msg = screenShareErrorRu(reason);
      if (msg) getStore().setStatus(msg, true, true);
    },
    onScreenShareTrack: ({ publisherId, stream, kind }) => {
      const store = useScreenShareStore.getState();
      if (kind === 'video') store.attachFocusedVideo(publisherId, stream);
      else store.attachFocusedAudio(publisherId, stream);
    },
    onScreenShareSelfStarted: ({ stream, videoCodec }) => {
      useScreenShareStore.getState().setMyStream(stream, videoCodec);
    },
    onScreenShareSelfStopped: () => {
      const store = useScreenShareStore.getState();
      store.setMyStatus('idle');
      store.setMyStream(null);
    },
    onScreenShareSystemAudioWarning: ({ reason }) => {
      if (reason === 'monitor-feedback-risk') {
        getStore().setStatus(
          'Зрители могут слышать свои голоса. Демонстрируйте окно, а не весь монитор.',
          true,
          true,
        );
      }
    },
    onError: (err) => {
      console.warn('[sfu]', err);
    },
  };
}
