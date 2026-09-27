import { useEffect, useState, type ReactNode } from 'react';
import {
  AudioLines,
  MessageSquare,
  Mic,
  MicOff,
  SlidersHorizontal,
  Volume2,
  VolumeX,
  WifiOff,
  type LucideIcon,
} from 'lucide-react';
import { useStore, type ChatMessage } from '../store/useStore';
import { ROOM_LABELS } from '../rooms';
import { loadOrCreateClientId } from '../utils/storage';
import { AdminKeyButton } from './AdminKeyButton';
import { LogoutButton } from './LogoutButton';
import { StatusPill } from './StatusPill';

type Tab = 'voice' | 'chat' | 'settings';

const TABS: { id: Tab; label: string; Icon: LucideIcon }[] = [
  { id: 'voice', label: 'Голос', Icon: AudioLines },
  { id: 'chat', label: 'Чат', Icon: MessageSquare },
  { id: 'settings', label: 'Настройки', Icon: SlidersHorizontal },
];

// Block flow, not flex/grid: desktop cards (min-h-0 + overflow) would shrink
// and scroll inside themselves.
const CARD_STACK = 'overflow-y-auto overscroll-contain p-3 space-y-3 [&_.card]:p-4';

interface Props {
  banner: ReactNode;
  voice: ReactNode;
  chat: ReactNode;
  settings: ReactNode;
  onToggleSelfMute: () => void;
  onToggleDeafen: () => void;
  onLeave: () => void;
}

// Phone layout: one screen per tab. Inactive panels are invisible rather than
// display:none, so the chat draft and each tab's scroll position survive.
export function MobileLayout({
  banner,
  voice,
  chat,
  settings,
  onToggleSelfMute,
  onToggleDeafen,
  onLeave,
}: Props) {
  const [tab, setTab] = useState<Tab>('voice');
  const inVoice = useStore((s) => s.joinState === 'joined');
  const unread = useUnreadCount(tab === 'chat');

  const panel = (id: Tab, layout: string) =>
    `absolute inset-0 ${layout} ${id === tab ? '' : 'invisible'}`;

  return (
    <div
      className="flex flex-col h-dvh overflow-hidden
        pt-[env(safe-area-inset-top)] pl-[env(safe-area-inset-left)] pr-[env(safe-area-inset-right)]"
    >
      <header className="flex items-center gap-2 h-14 landscape:h-12 shrink-0 pl-3 pr-2 bg-bg-1 border-b border-line">
        <img src="/favicon.svg" alt="" className="w-6 h-6 shrink-0" />
        <div className="text-[14px] font-extrabold uppercase tracking-[0.14em] text-accent shrink-0 max-[374px]:hidden">
          Voice&nbsp;Hub
        </div>
        <div className="flex-1 min-w-0 flex justify-end">
          <StatusPill />
        </div>
        <AdminKeyButton />
        <LogoutButton />
      </header>

      <div className="shrink-0 px-3 pt-3 empty:hidden">{banner}</div>

      <div className="relative flex-1 min-h-0">
        <div className={panel('voice', CARD_STACK)}>{voice}</div>
        <div className={panel('chat', 'flex flex-col p-3 landscape:p-2')}>{chat}</div>
        <div className={panel('settings', CARD_STACK)}>{settings}</div>
      </div>

      <div
        className="flex flex-col-reverse landscape:flex-row shrink-0 bg-bg-1 border-t border-line
          pb-[env(safe-area-inset-bottom)] select-none"
      >
        <nav className="grid grid-cols-3 landscape:flex-1" aria-label="Разделы">
          {TABS.map(({ id, label, Icon }) => {
            const active = id === tab;
            const badge = id === 'chat' && unread > 0 ? unread : null;
            return (
              <button
                key={id}
                type="button"
                onClick={() => setTab(id)}
                aria-current={active ? 'page' : undefined}
                className={`relative flex flex-col landscape:flex-row items-center justify-center gap-1 landscape:gap-2 h-16 landscape:h-12
                text-[11px] font-bold uppercase tracking-[0.14em] transition-colors
                ${active ? 'text-accent' : 'text-muted-2'}`}
              >
                {active && <span className="absolute top-0 inset-x-6 h-0.5 bg-accent" />}
                <span className="relative">
                  <Icon size={24} />
                  {id === 'voice' && inVoice && (
                    <span className="absolute -top-0.5 -right-1.5 w-2 h-2 bg-accent border-2 border-bg-1 box-content" />
                  )}
                  {badge !== null && (
                    <span
                      className="absolute -top-1.5 left-4 min-w-5 h-5 px-1 grid place-items-center
                      bg-accent text-accent-ink text-[11px] font-extrabold tracking-normal tabular-nums"
                      aria-label={`Непрочитанных: ${badge}`}
                    >
                      {badge > 99 ? '99+' : badge}
                    </span>
                  )}
                </span>
                {label}
              </button>
            );
          })}
        </nav>
        {inVoice && tab !== 'voice' && (
          <VoiceBar
            onToggleSelfMute={onToggleSelfMute}
            onToggleDeafen={onToggleDeafen}
            onLeave={onLeave}
          />
        )}
      </div>
    </div>
  );
}

// Voice controls reachable from every tab besides "Голос", which has the full card.
function VoiceBar({
  onToggleSelfMute,
  onToggleDeafen,
  onLeave,
}: Pick<Props, 'onToggleSelfMute' | 'onToggleDeafen' | 'onLeave'>) {
  const selfMuted = useStore((s) => s.selfMuted);
  const deafened = useStore((s) => s.deafened);
  const roomSlug = useStore((s) => s.roomSlug);

  const on = 'border-accent text-accent';
  const off = 'border-danger text-danger bg-[rgba(248,113,113,0.08)]';
  const btn = 'grid place-items-center w-11 h-11 border bg-bg-0 transition-colors';

  return (
    <div className="flex items-center gap-2 px-3 py-2 landscape:py-0.5 border-b landscape:border-b-0 landscape:border-l border-line">
      <span className="w-1.5 h-1.5 shrink-0 bg-accent animate-[vh-pulse_1.4s_ease-in-out_infinite]" />
      <span className="flex-1 min-w-0 truncate text-[12px] font-bold uppercase tracking-[0.18em] text-muted">
        Комната {ROOM_LABELS[roomSlug]}
      </span>
      <button
        type="button"
        onClick={onToggleSelfMute}
        aria-pressed={selfMuted}
        aria-label={selfMuted ? 'Включить микрофон' : 'Выключить микрофон'}
        className={`${btn} ${selfMuted ? off : on}`}
      >
        {selfMuted ? <MicOff size={20} /> : <Mic size={20} />}
      </button>
      <button
        type="button"
        onClick={onToggleDeafen}
        aria-pressed={deafened}
        aria-label={deafened ? 'Слушать всех' : 'Заглушить всех'}
        className={`${btn} ${deafened ? off : on}`}
      >
        {deafened ? <VolumeX size={20} /> : <Volume2 size={20} />}
      </button>
      <button
        type="button"
        onClick={onLeave}
        aria-label="Отключиться"
        className={`${btn} border-danger text-danger`}
      >
        <WifiOff size={20} />
      </button>
    </div>
  );
}

const EMPTY_MESSAGES: ChatMessage[] = [];

// Others' messages in the current room newer than the chat tab last showed
// there. Watermarks are server timestamps, per room; an unviewed room counts
// from mount.
function useUnreadCount(chatVisible: boolean): number {
  const roomSlug = useStore((s) => s.roomSlug);
  const messages = useStore((s) => s.chatByRoom[roomSlug] ?? EMPTY_MESSAGES);
  const [selfClientId] = useState(loadOrCreateClientId);
  const [mountedAt] = useState(Date.now);
  const [seenByRoom, setSeenByRoom] = useState<Record<string, number>>({});

  // Pending messages carry the local clock; only server-stamped ones count.
  const confirmed = messages.filter((m) => !m.pending);
  const latest = confirmed.reduce((max, m) => Math.max(max, m.ts), 0);

  useEffect(() => {
    if (!chatVisible || latest === 0) return;
    setSeenByRoom((prev) => (prev[roomSlug] === latest ? prev : { ...prev, [roomSlug]: latest }));
  }, [chatVisible, roomSlug, latest]);

  if (chatVisible) return 0;
  const seen = seenByRoom[roomSlug] ?? mountedAt;
  return confirmed.filter((m) => m.ts > seen && m.senderClientId !== selfClientId).length;
}
