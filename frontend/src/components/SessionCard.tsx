import { Mic, Volume2, Wifi, WifiOff, type LucideIcon } from 'lucide-react';
import { useStore } from '../store/useStore';

interface Props {
  onJoin: (displayName: string) => void;
  onLeave: () => void;
  onToggleSelfMute: () => void;
  onToggleDeafen: () => void;
  displayName: string;
  onDisplayNameChange: (value: string) => void;
}

export function SessionCard({
  onJoin,
  onLeave,
  onToggleSelfMute,
  onToggleDeafen,
  displayName,
  onDisplayNameChange,
}: Props) {
  const joinState = useStore((s) => s.joinState);
  const selfMuted = useStore((s) => s.selfMuted);
  const deafened = useStore((s) => s.deafened);
  const configReady = useStore((s) => s.configReady);

  const joining = joinState === 'joining';
  const joined = joinState === 'joined';

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (joined) {
      onLeave();
      return;
    }
    onJoin(displayName);
  }

  const heroLabel = joined ? 'Отключиться' : joining ? 'Подключение…' : 'Подключиться';
  const HeroIcon = joined ? WifiOff : Wifi;

  return (
    <section className="card grid gap-5 p-6">
      <div className="grid gap-3">
        <h2 className="card-title">Подключение</h2>

        <div className="grid gap-2">
          <label htmlFor="display-name" className="section-label">
            Имя
          </label>
          <input
            id="display-name"
            name="displayName"
            type="text"
            placeholder="ИМЯ"
            autoComplete="off"
            value={displayName}
            onChange={(e) => onDisplayNameChange(e.target.value)}
            className="w-full bg-bg-0 border border-line px-4 py-3
            text-accent text-[20px] leading-[28px] font-bold tracking-[0.2em]
            transition-colors duration-150
            placeholder:text-muted-2
            focus:outline-none focus:border-accent"
          />
        </div>

        <form id="join-form" onSubmit={handleSubmit} className="grid gap-3">
          <div className="flex gap-2">
            <ToggleTile
              id="self-mute-button"
              off={selfMuted}
              Icon={Mic}
              label="Микро"
              hint={selfMuted ? 'Включить микрофон' : 'Выключить микрофон'}
              onClick={onToggleSelfMute}
            />
            <ToggleTile
              id="deafen-button"
              off={deafened}
              Icon={Volume2}
              label="Звук"
              hint={deafened ? 'Слушать всех' : 'Заглушить всех'}
              onClick={onToggleDeafen}
            />
          </div>

          <button
            id="join-button"
            type="submit"
            disabled={joining || (!joined && !configReady)}
            className={`btn btn-hero ${joined ? 'btn-danger' : 'btn-primary'}`}
          >
            <HeroIcon size={32} className="shrink-0" />
            <span className="flex-1 text-center">{heroLabel}</span>
          </button>
        </form>
      </div>
    </section>
  );
}

function ToggleTile({
  id,
  off,
  Icon,
  label,
  hint,
  onClick,
}: {
  id: string;
  off: boolean;
  Icon: LucideIcon;
  label: string;
  hint: string;
  onClick: () => void;
}) {
  return (
    <button
      id={id}
      type="button"
      aria-pressed={off}
      aria-label={hint}
      title={hint}
      onClick={onClick}
      className={`flex-1 flex items-center p-4 transition-colors duration-150 bg-bg-0 border ${
        off
          ? 'border-danger text-danger hover:bg-[rgba(248,113,113,0.08)]'
          : 'border-accent text-accent hover:bg-[rgba(75,226,119,0.08)]'
      }`}
    >
      <Icon size={24} className="shrink-0" />
      <span className="flex-1 text-center text-[14px] font-bold uppercase tracking-[0.18em]">
        {off ? 'Выкл' : label}
      </span>
    </button>
  );
}
