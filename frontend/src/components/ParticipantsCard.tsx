import { useShallow } from 'zustand/react/shallow';
import { selectChatOnlyParticipants, selectVoiceParticipants, useStore } from '../store/useStore';
import { ParticipantRow } from './ParticipantRow';
import { useRoomPeers } from '../hooks/useRoomPeers';
import { ROOM_SLUGS, ROOM_LABELS, type RoomSlug } from '../rooms';

interface Props {
  onRemoteGainChange: () => void;
  onPingUser?: (targetId: string) => void;
  onRoomSelect: (slug: RoomSlug) => void;
}

export function ParticipantsCard({ onRemoteGainChange, onPingUser, onRoomSelect }: Props) {
  const participants = useStore(useShallow(selectVoiceParticipants));
  const lurkers = useStore(useShallow(selectChatOnlyParticipants));
  const joinState = useStore((s) => s.joinState);
  const roomSlug = useStore((s) => s.roomSlug);
  const roomPeers = useRoomPeers();

  const liveCount = participants.length;

  const isJoining = joinState === 'joining';
  const isJoined = joinState === 'joined';

  const tileOn = 'bg-bg-0 border border-accent text-accent hover:bg-[rgba(75,226,119,0.08)]';

  return (
    <section className="card flex flex-col p-6 flex-1 min-h-0 overflow-y-auto gap-5">
      <div className="grid gap-3">
        <h2 className="card-title">Комната</h2>
        <div className="grid gap-1.5">
          {ROOM_SLUGS.map((slug: RoomSlug) => {
            const active = slug === roomSlug;
            const peers = roomPeers[slug];
            const total = peers.length;
            const voice = peers.filter((p) => !p.chatOnly).length;
            const text = total - voice;
            const nameLine =
              total > 0
                ? peers
                    .slice()
                    .sort((a, b) => {
                      if (a.chatOnly !== b.chatOnly) return a.chatOnly ? 1 : -1;
                      return a.id.localeCompare(b.id);
                    })
                    .map((p) => p.displayName)
                    .join(', ')
                : null;

            const switchable = isJoined && !active;
            const clickable = !isJoining && (!isJoined || switchable);

            let title: string | undefined;
            if (isJoining) title = 'Подождите…';
            else if (switchable) title = 'Перейти в эту комнату';

            return (
              <button
                key={slug}
                type="button"
                onClick={clickable ? () => onRoomSelect(slug) : undefined}
                title={title}
                className={`flex items-center gap-3 px-3 py-2.5 transition-colors duration-150 ${
                  active
                    ? tileOn
                    : switchable
                      ? 'bg-bg-0 border border-line text-muted hover:border-line-strong hover:border-accent/50'
                      : 'bg-bg-0 border border-line text-muted hover:border-line-strong'
                } ${isJoining ? 'opacity-50 cursor-not-allowed' : active && isJoined ? 'cursor-default' : 'cursor-pointer'}`}
              >
                <span className="text-[13px] font-bold uppercase tracking-[0.15em] shrink-0">
                  {ROOM_LABELS[slug]}
                </span>
                <span
                  className={`text-[12px] tabular-nums tracking-[0.1em] px-2 py-0.5 border shrink-0 ${
                    voice > 0 ? 'border-accent' : 'border-line'
                  }`}
                  title={voice > 0 && text > 0 ? `${voice} в голосе, ${text} в чате` : undefined}
                >
                  {voice > 0 ? (
                    <>
                      <span className="text-accent">{voice}</span>
                      {text > 0 && <span className="text-muted-2">/{text}</span>}
                    </>
                  ) : (
                    <span className="text-muted-2">{total}</span>
                  )}
                </span>
                {nameLine && (
                  <span className="flex-1 min-w-0 text-left text-muted-2 text-[11px] tracking-[0.05em] truncate">
                    {nameLine}
                  </span>
                )}
              </button>
            );
          })}
        </div>
      </div>

      <div className="grid gap-3">
        <div className="flex items-center justify-between gap-3">
          <div className="flex items-center gap-2 min-w-0">
            <span className="section-label">Участники</span>
            {liveCount > 0 && (
              <span className="w-1.5 h-1.5 bg-accent animate-[vh-pulse_1.4s_ease-in-out_infinite] shrink-0" />
            )}
          </div>
          {liveCount > 0 && (
            <span className="text-[11px] font-bold uppercase tracking-[0.18em] text-muted-2 tabular-nums shrink-0">
              {liveCount}
            </span>
          )}
        </div>
        <div id="participants" className="grid gap-2">
          {liveCount === 0 && (
            <div className="px-4 py-6 text-center text-muted-2 border border-dashed border-line bg-bg-0 text-[12px] uppercase tracking-[0.12em]">
              Пока никого нет
            </div>
          )}
          {participants.map((p) => (
            <ParticipantRow
              key={p.id}
              participant={p}
              onRemoteGainChange={onRemoteGainChange}
              onPing={onPingUser}
            />
          ))}
        </div>
      </div>

      {lurkers.length > 0 && (
        <div className="grid gap-3">
          <div className="flex items-center justify-between gap-3">
            <span className="section-label">Только чат</span>
            <span className="text-[11px] font-bold uppercase tracking-[0.18em] text-muted-2 tabular-nums shrink-0">
              {lurkers.length}
            </span>
          </div>
          <div className="grid gap-2">
            {lurkers.map((p) => (
              <ParticipantRow
                key={p.id}
                participant={p}
                onRemoteGainChange={onRemoteGainChange}
                onPing={onPingUser}
              />
            ))}
          </div>
        </div>
      )}
    </section>
  );
}
