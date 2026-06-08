import { memo, useRef, useMemo, useEffect, useState, useCallback, type ReactNode } from 'react';
import { selectSelfPeerId, useStore, type ChatMessage } from '../store/useStore';
import { CHAT_MAX_BYTES } from '../sfu/protocol';
import { loadOrCreateClientId } from '../utils/storage';
import { Send, Info } from 'lucide-react';
import { isTauri } from '../utils/tauri';

// http(s):// or bare www. — greedy until whitespace/quotes/angle brackets,
// then strip trailing punctuation and unbalanced closers (so `(see https://en.wikipedia.org/wiki/Rust_(programming_language))`
// keeps the inner parens but drops the outer closing one).
const URL_RE = /\b(?:https?:\/\/|www\.)[^\s<>"']+/gi;
const TEXT_ENCODER = new TextEncoder();

function trimUrl(raw: string): string {
  let s = raw.replace(/[.,;:!?]+$/, '');
  const pairs: Record<string, string> = { ')': '(', ']': '[', '}': '{' };
  while (s.length > 0) {
    const last = s[s.length - 1];
    const open = pairs[last];
    if (!open) break;
    let opens = 0;
    let closes = 0;
    for (const ch of s) {
      if (ch === open) opens++;
      else if (ch === last) closes++;
    }
    if (closes <= opens) break;
    s = s.slice(0, -1);
  }
  return s;
}

function handleLinkClick(href: string) {
  if (!isTauri()) return;
  // In Tauri webview default anchor would navigate the app. Route to OS browser.
  void import('@tauri-apps/plugin-opener')
    .then((m) => m.openUrl(href))
    .catch((err) => console.error('openUrl failed', err));
}

function renderText(text: string): ReactNode {
  const parts: ReactNode[] = [];
  let last = 0;
  let key = 0;
  for (const m of text.matchAll(URL_RE)) {
    const start = m.index ?? 0;
    const raw = trimUrl(m[0]);
    if (!raw) continue;
    if (start > last) parts.push(text.slice(last, start));
    const href = raw.startsWith('www.') ? `https://${raw}` : raw;
    parts.push(
      <a
        key={key++}
        href={href}
        target="_blank"
        rel="noreferrer noopener"
        onClick={(e) => {
          if (isTauri()) {
            e.preventDefault();
            handleLinkClick(href);
          }
        }}
        className="text-accent underline underline-offset-2 break-all hover:opacity-80"
      >
        {raw}
      </a>,
    );
    last = start + raw.length;
  }
  if (last < text.length) parts.push(text.slice(last));
  return parts;
}

const MAX_DISPLAY = 200;

const CHAT_HINT =
  'Сообщения видны всем в комнате.\nСервер их не хранит — только пересылает.\nИстория хранится локально на устройстве: 7 дней или 500 сообщений.';

function byteLength(s: string): number {
  return TEXT_ENCODER.encode(s).length;
}

function formatTime(ts: number): string {
  const d = new Date(ts);
  return d.toLocaleTimeString('ru-RU', { hour: '2-digit', minute: '2-digit' });
}

interface Props {
  roomId: string;
  onSend: (text: string, clientMsgId: string) => void;
}

type VisibleMessage = {
  msg: ChatMessage;
  isSelf: boolean;
  senderName: string;
  showName: boolean;
  showTime: boolean;
  renderedText: ReactNode;
};

export function ChatPanel({ roomId, onSend }: Props) {
  const messages = useStore((s) => s.chatByRoom[roomId] ?? []);
  const participants = useStore((s) => s.participants);
  const chatSendOptimistic = useStore((s) => s.chatSendOptimistic);
  const persistChat = useStore((s) => s.persistChat);
  const loadChatRoom = useStore((s) => s.loadChatRoom);

  // Hydrate persisted history on mount so chat is visible before joining.
  useEffect(() => {
    loadChatRoom(roomId);
  }, [roomId, loadChatRoom]);

  const selfPeerId = useStore(selectSelfPeerId);
  // Stable per-install identity — independent of join state. Lets MessageRow
  // recognise own messages even after we leave the room (when selfPeerId is null).
  const selfClientId = useRef(loadOrCreateClientId()).current;

  const [text, setText] = useState('');
  const bytes = byteLength(text);
  const overLimit = bytes > CHAT_MAX_BYTES;
  const canSend = text.trim().length > 0 && !overLimit;

  const listRef = useRef<HTMLDivElement>(null);
  const atBottomRef = useRef(true);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  // Track whether user has scrolled away from bottom.
  const handleScroll = useCallback(() => {
    const el = listRef.current;
    if (!el) return;
    atBottomRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
  }, []);

  // Auto-scroll on new messages only when pinned to bottom.
  const prevLenRef = useRef(messages.length);
  useEffect(() => {
    if (messages.length !== prevLenRef.current) {
      prevLenRef.current = messages.length;
      if (atBottomRef.current) {
        listRef.current?.scrollTo({ top: listRef.current.scrollHeight });
      }
    }
  }, [messages.length]);

  // Auto-resize textarea up to ~5 lines.
  useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight, 140)}px`;
  }, [text]);

  const handleSend = useCallback(() => {
    const trimmed = text.trim();
    if (!trimmed || overLimit || !selfPeerId) return;
    const clientMsgId = crypto.randomUUID();
    const now = Date.now();
    const selfEntry = participants[selfPeerId];
    chatSendOptimistic(roomId, {
      id: clientMsgId,
      from: selfPeerId,
      text: trimmed,
      ts: now,
      clientMsgId,
      pending: true,
      senderName: selfEntry?.display,
      senderClientId: selfEntry?.clientId,
    });
    persistChat(roomId);
    onSend(trimmed, clientMsgId);
    setText('');
  }, [text, overLimit, selfPeerId, roomId, participants, chatSendOptimistic, persistChat, onSend]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        handleSend();
      }
    },
    [handleSend],
  );

  const visible = useMemo<VisibleMessage[]>(() => {
    const tail = messages.slice(-MAX_DISPLAY);
    return tail.map((msg, i) => {
      const prev = i > 0 ? tail[i - 1] : null;
      const sameSender =
        prev !== null &&
        (prev.senderClientId !== undefined && msg.senderClientId !== undefined
          ? prev.senderClientId === msg.senderClientId
          : prev.from === msg.from);
      const showName = !(sameSender && prev !== null && msg.ts - prev.ts < 5 * 60_000);
      const sameMinute =
        prev !== null && Math.floor(prev.ts / 60_000) === Math.floor(msg.ts / 60_000);
      const showTime = showName || !sameMinute;
      const isSelf =
        msg.senderClientId !== undefined
          ? msg.senderClientId === selfClientId
          : msg.from === selfPeerId;
      const senderName =
        msg.senderName ?? participants[msg.from]?.display ?? (isSelf ? 'Вы' : 'Неизвестный');

      return {
        msg,
        isSelf,
        senderName,
        showName,
        showTime,
        renderedText: renderText(msg.text),
      };
    });
  }, [messages, participants, selfClientId, selfPeerId]);

  return (
    <section className="card p-0! flex flex-col flex-1 min-h-0">
      <div className="px-6 pt-5 pb-4 border-b border-line shrink-0">
        <h2 className="card-title flex items-center gap-1.5">
          Чат
          <span
            tabIndex={0}
            className="group relative inline-flex outline-none cursor-help text-muted-2/60 hover:text-muted-2 focus-visible:text-muted-2 transition-colors"
          >
            <Info size={15} strokeWidth={2} aria-label="Как работает чат" />
            <span
              role="tooltip"
              className="pointer-events-none invisible absolute left-0 top-full z-20 mt-2 w-max whitespace-pre border border-line-strong bg-bg-3 px-3 py-2 text-[12px] leading-snug text-muted normal-case tracking-normal font-normal opacity-0 shadow-lg transition-opacity duration-150 group-hover:visible group-hover:opacity-100 group-focus-within:visible group-focus-within:opacity-100"
            >
              {CHAT_HINT}
            </span>
          </span>
        </h2>
      </div>

      <div
        ref={listRef}
        onScroll={handleScroll}
        className="flex-1 min-h-0 overflow-y-auto px-4 py-3 grid content-start gap-0.5"
      >
        {visible.length === 0 && (
          <div className="px-2 py-8 text-center text-muted-2 text-[12px] uppercase tracking-[0.12em]">
            Сообщений пока нет
          </div>
        )}
        {visible.map((row) => (
          <MessageRow key={row.msg.id} row={row} />
        ))}
      </div>

      <div className="px-4 pb-4 pt-3 border-t border-line shrink-0">
        <div
          className={`flex gap-1.5 items-end p-1.5 border ${overLimit ? 'border-danger' : 'border-line'} bg-bg-input focus-within:border-accent transition-[border-color] duration-150`}
        >
          <textarea
            ref={textareaRef}
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder="Сообщение…"
            rows={1}
            className="flex-1 resize-none bg-transparent px-3 py-2 text-[17px] text-text placeholder:text-muted-2 focus:outline-none disabled:opacity-40"
            style={{ minHeight: 40, maxHeight: 140, lineHeight: '1.4' }}
          />
          <button
            onClick={handleSend}
            disabled={!canSend || !selfPeerId}
            className="btn btn-primary shrink-0 grid place-items-center p-0!"
            style={{ width: 40, height: 40 }}
            aria-label="Отправить"
          >
            <Send size={26} />
          </button>
        </div>
        {bytes > CHAT_MAX_BYTES * 0.8 && (
          <div
            className={`mt-1 text-right text-[11px] tabular-nums ${overLimit ? 'text-danger' : 'text-muted-2'}`}
          >
            {bytes}/{CHAT_MAX_BYTES}
          </div>
        )}
      </div>
    </section>
  );
}

const MessageRow = memo(function MessageRow({ row }: { row: VisibleMessage }) {
  const { msg, isSelf, senderName, showName, showTime, renderedText } = row;
  return (
    <div className={`px-2 ${showName ? 'pt-2' : ''} ${msg.pending ? 'opacity-50' : ''}`}>
      {showName && (
        <div
          className={`text-[11px] font-bold uppercase tracking-[0.14em] truncate mb-0.5 ${isSelf ? 'text-accent' : 'text-muted'}`}
        >
          {senderName}
        </div>
      )}
      <div className="grid grid-cols-[minmax(0,1fr)_auto] gap-x-3 items-baseline">
        <p className="m-0 text-[17px] text-body break-words whitespace-pre-wrap">{renderedText}</p>
        {showTime && (
          <span className="text-[11px] text-muted-2 tabular-nums shrink-0">
            {formatTime(msg.ts)}
          </span>
        )}
      </div>
    </div>
  );
});
