import { memo, useEffect, useRef, useState } from 'react';
import { Bell, EarOff, MicOff, ScreenShare, Tag, Volume2, VolumeX } from 'lucide-react';
import { useStore } from '../store/useStore';
import { PEER_LABEL_MAX, savePeerLabel, savePeerVolume } from '../utils/storage';
import type { ParticipantUI } from '../types';

type MetaTone = 'good' | 'danger' | 'muted' | 'connecting';

const TONE_STYLES: Record<MetaTone, { dot: string; text: string }> = {
  good: { dot: 'bg-good', text: 'text-good' },
  danger: { dot: 'bg-danger', text: 'text-danger' },
  connecting: {
    dot: 'bg-accent animate-[vh-pulse_1.4s_ease-in-out_infinite]',
    text: 'text-muted-2',
  },
  muted: { dot: 'bg-muted-2', text: 'text-muted-2' },
};

interface Props {
  participant: ParticipantUI;
  onRemoteGainChange: () => void;
  onPing: (targetId: string) => void;
}

function ParticipantRowImpl({ participant, onRemoteGainChange, onPing }: Props) {
  const updateParticipant = useStore((s) => s.updateParticipant);
  const inVoice = useStore((s) => s.joinState === 'joined');
  const lastPingSentAt = useStore((s) => s.lastPingSentByTarget.get(participant.id) ?? 0);
  const [, forceTick] = useState(0);
  const [editingLabel, setEditingLabel] = useState(false);
  const [labelDraft, setLabelDraft] = useState('');
  // Set true by Esc to make the upcoming onBlur skip its commit.
  const skipLabelCommitRef = useRef(false);
  const canLabel = !participant.isSelf && Boolean(participant.clientId);

  function beginEditLabel() {
    if (!canLabel) return;
    setLabelDraft(participant.localLabel ?? '');
    setEditingLabel(true);
  }

  function commitLabel() {
    if (skipLabelCommitRef.current) {
      skipLabelCommitRef.current = false;
      return;
    }
    const next = labelDraft.trim().slice(0, PEER_LABEL_MAX);
    if (participant.clientId) savePeerLabel(participant.clientId, next);
    updateParticipant(participant.id, { localLabel: next || undefined });
    setEditingLabel(false);
  }

  function cancelLabel() {
    skipLabelCommitRef.current = true;
    setEditingLabel(false);
  }
  const pingCoolingDown = Date.now() - lastPingSentAt < 10000;
  useEffect(() => {
    if (!pingCoolingDown) return;
    const remaining = 10000 - (Date.now() - lastPingSentAt);
    const t = window.setTimeout(() => forceTick((v) => v + 1), remaining + 50);
    return () => clearTimeout(t);
  }, [lastPingSentAt, pingCoolingDown]);

  const isLurker = Boolean(participant.chatOnly);
  const isMuted = participant.isSelf ? participant.selfMuted : participant.localMuted;
  // Chat-only viewers never receive remote media, so hasStream is always false
  // for voice peers — treat them as ready to avoid a permanent dimmed row.
  const isReady = participant.isSelf || participant.hasStream || !inVoice;

  function statusMeta(): { text: string; tone: MetaTone } {
    if (isLurker) return { text: 'только чат', tone: 'muted' };
    if (participant.isSelf) {
      if (participant.selfMuted) return { text: 'микрофон выключен', tone: 'danger' };
      if (participant.speaking) return { text: 'говорит', tone: 'good' };
      return { text: 'в эфире', tone: 'muted' };
    }
    if (participant.hasStream) {
      if (participant.localMuted) return { text: 'заглушён вами', tone: 'danger' };
      if (participant.remoteDeafened) return { text: 'не слышит', tone: 'danger' };
      if (participant.remoteMuted) return { text: 'микрофон выключен', tone: 'danger' };
      if (participant.speaking) return { text: 'говорит', tone: 'good' };
      return { text: 'слышно', tone: 'muted' };
    }
    // No remote stream. In voice that's a genuine (transient) connecting state;
    // chat-only viewers never receive media, so hasStream stays false forever —
    // reflect the join-time snapshot (matches the indicator icons) instead.
    if (inVoice) return { text: 'подключается', tone: 'connecting' };
    if (participant.remoteDeafened) return { text: 'не слышит', tone: 'danger' };
    if (participant.remoteMuted) return { text: 'микрофон выключен', tone: 'danger' };
    return { text: 'в голосе', tone: 'muted' };
  }
  const { text: metaText, tone: metaTone } = statusMeta();

  const initial = (participant.display || '?').trim().charAt(0).toUpperCase() || '?';

  function handleToggleMute() {
    updateParticipant(participant.id, { localMuted: !participant.localMuted });
    onRemoteGainChange();
  }

  function handleVolumeChange(e: React.ChangeEvent<HTMLInputElement>) {
    const volume = Number(e.target.value);
    updateParticipant(participant.id, { localVolume: volume });
    if (participant.clientId) {
      savePeerVolume(participant.clientId, volume);
    }
    onRemoteGainChange();
  }

  // Voice activity ring: solid green border on speaker. Border width fixed at 2px so toggling speaking doesn't shift layout.
  const rowClass = isLurker
    ? 'border-2 border-line bg-bg-0 opacity-50'
    : participant.speaking
      ? 'border-2 border-accent bg-bg-0'
      : isMuted
        ? 'border-2 border-line bg-bg-0'
        : !isReady
          ? 'border-2 border-line bg-bg-0 opacity-70'
          : 'border-2 border-line bg-bg-0 hover:border-line-strong';

  const { dot: metaDotClass, text: metaTextClass } = TONE_STYLES[metaTone];

  const avatarRing = participant.speaking ? 'ring-2 ring-accent ring-offset-0' : '';

  const showIndicators =
    !participant.isSelf &&
    !isLurker &&
    (participant.remoteMuted || participant.remoteDeafened || participant.screenSharing);

  return (
    <div
      className={`grid gap-3 px-4 ${participant.isSelf ? 'h-[72px] items-center' : 'py-3'} transition-[border-color,background] duration-75 ${rowClass}`}
    >
      <div className="grid grid-cols-[40px_minmax(0,1fr)_auto] gap-3 items-center">
        {!participant.isSelf ? (
          <button
            type="button"
            disabled={pingCoolingDown}
            onClick={() => onPing(participant.id)}
            aria-label={`Пингануть ${participant.display}`}
            title={pingCoolingDown ? 'Подождите 10 с' : `Пингануть ${participant.display}`}
            className={`group relative grid place-items-center ${isLurker ? 'bg-bg-3 text-muted' : 'bg-accent text-accent-ink'} font-extrabold text-[20px] uppercase shrink-0 border-2 border-transparent transition-[border-color] duration-150 ${avatarRing} ${pingCoolingDown ? 'opacity-40 cursor-not-allowed' : 'hover:border-accent cursor-pointer'}`}
            style={{ width: 40, height: 40 }}
          >
            <span
              className={`absolute inset-0 flex items-center justify-center transition-opacity duration-150 ${pingCoolingDown ? '' : 'group-hover:opacity-0'}`}
            >
              {initial}
            </span>
            {!pingCoolingDown && (
              <span
                className={`absolute inset-0 flex items-center justify-center opacity-0 transition-opacity duration-150 group-hover:opacity-100 ${isLurker ? 'text-accent' : 'text-accent-ink'}`}
              >
                <Bell size={20} />
              </span>
            )}
          </button>
        ) : (
          <div
            className={`grid place-items-center ${isLurker ? 'bg-bg-3 text-muted' : 'bg-accent text-accent-ink'} font-extrabold text-[20px] uppercase shrink-0 ${avatarRing}`}
            style={{ width: 40, height: 40 }}
          >
            {initial}
          </div>
        )}
        <div className="min-w-0 flex flex-col justify-between" style={{ height: 40 }}>
          <div className="group/label text-[18px] font-bold text-body tracking-tight leading-tight flex items-center gap-1.5 min-w-0">
            <span className="truncate">{participant.display}</span>
            {participant.isSelf ? (
              <span className="text-muted-2 font-normal shrink-0">[вы]</span>
            ) : editingLabel ? (
              <input
                type="text"
                autoFocus
                value={labelDraft}
                maxLength={PEER_LABEL_MAX}
                placeholder="метка"
                size={Math.max(4, labelDraft.length + 1)}
                onChange={(e) => setLabelDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') {
                    e.preventDefault();
                    e.currentTarget.blur();
                  } else if (e.key === 'Escape') {
                    e.preventDefault();
                    cancelLabel();
                  }
                }}
                onBlur={commitLabel}
                className="shrink-0 bg-bg-2 border border-accent text-body text-[18px] font-bold px-1.5 py-0 outline-none tracking-tight"
              />
            ) : participant.localLabel ? (
              <button
                type="button"
                onClick={beginEditLabel}
                title="Изменить метку"
                aria-label="Изменить метку"
                className="text-muted-2 font-normal shrink-0 hover:text-accent cursor-pointer"
              >
                [{participant.localLabel}]
              </button>
            ) : canLabel ? (
              <button
                type="button"
                onClick={beginEditLabel}
                title="Добавить метку"
                aria-label="Добавить метку"
                className="shrink-0 text-muted-2 hover:text-accent cursor-pointer opacity-0 group-hover/label:opacity-100 transition-opacity grid place-items-center"
              >
                <Tag size={16} />
              </button>
            ) : null}
          </div>
          <div
            className={`text-[11px] uppercase tracking-[0.18em] inline-flex items-center gap-1.5 leading-none ${metaTextClass}`}
          >
            <span className={`w-1.5 h-1.5 ${metaDotClass}`} />
            {metaText}
          </div>
        </div>
        {showIndicators && (
          <div className="flex gap-1 items-center shrink-0">
            {participant.screenSharing && (
              <span
                aria-label="Делится экраном"
                title="Делится экраном"
                className="grid place-items-center w-9 h-9 text-accent border border-accent/40"
              >
                <ScreenShare size={18} />
              </span>
            )}
            {participant.remoteMuted && (
              <span
                aria-label="Микрофон выключен"
                title="Микрофон выключен"
                className="grid place-items-center w-9 h-9 text-danger border border-danger/40"
              >
                <MicOff size={18} />
              </span>
            )}
            {participant.remoteDeafened && (
              <span
                aria-label="В наушниках"
                title="В наушниках"
                className="grid place-items-center w-9 h-9 text-danger border border-danger/40"
              >
                <EarOff size={18} />
              </span>
            )}
          </div>
        )}
      </div>

      {!participant.isSelf && !isLurker && (
        <div className="flex gap-2 items-center">
          <button
            type="button"
            onClick={handleToggleMute}
            aria-pressed={participant.localMuted}
            aria-label={participant.localMuted ? 'Слушать' : 'Заглушить'}
            title={participant.localMuted ? 'Слушать' : 'Заглушить'}
            className={`grid place-items-center w-9 h-9 border transition-colors shrink-0 ${
              participant.localMuted
                ? 'border-danger text-danger bg-[rgba(248,113,113,0.08)] hover:bg-danger hover:text-accent-ink'
                : 'border-line text-muted hover:border-accent hover:text-accent'
            }`}
          >
            {participant.localMuted ? <VolumeX size={18} /> : <Volume2 size={18} />}
          </button>
          <label className="grid gap-1 flex-1 min-w-0">
            <span className="whitespace-nowrap tabular-nums text-[11px] font-bold uppercase tracking-[0.18em] text-muted-2">
              Громкость {participant.localVolume}%
            </span>
            <input
              type="range"
              min="0"
              max="300"
              step="5"
              value={participant.localVolume}
              onChange={handleVolumeChange}
              className="vh-range vh-range-sm"
              style={{ '--fill-pct': `${participant.localVolume / 3}%` } as React.CSSProperties}
            />
          </label>
        </div>
      )}
    </div>
  );
}

export const ParticipantRow = memo(ParticipantRowImpl);
