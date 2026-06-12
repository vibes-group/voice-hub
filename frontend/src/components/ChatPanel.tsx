import {
  memo,
  useRef,
  useMemo,
  useEffect,
  useState,
  useCallback,
  type ReactNode,
  type DragEvent,
  type ClipboardEvent,
} from 'react';
import { selectSelfPeerId, useStore, type ChatMessage } from '../store/useStore';
import {
  CHAT_MAX_BYTES,
  CHAT_MAX_ATTACHMENTS,
  type Attachment,
  type AttachmentKind,
} from '../sfu/protocol';
import { loadOrCreateClientId } from '../utils/storage';
import { Send, Info, Paperclip, X, Trash2, WifiOff } from 'lucide-react';
import { isTauri } from '../utils/tauri';
import { uploadFile, imageMeta, MAX_UPLOAD_BYTES, TEMP_UPLOAD_PREFIX } from '../utils/uploadFile';
import { putBlob, getBlob, rekeyBlob, pruneBlobs, deleteBlob } from '../utils/blobCache';
import { AttachmentImage } from './AttachmentImage';
import { AttachmentAlbum } from './AttachmentAlbum';
import { AttachmentFileCard, formatFileSize } from './AttachmentFileCard';
import { ImageLightbox } from './ImageLightbox';

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

// Stable empty ref for the chat selector. A fresh `[]` fallback would change
// identity every render — and since history now loads async (the entry stays
// undefined until IndexedDB resolves), useSyncExternalStore would loop.
const EMPTY_MESSAGES: ChatMessage[] = [];

const CHAT_HINT =
  'Сообщения и файлы видны всем в комнате.\nСервер их не хранит — только пересылает.\nИстория хранится локально на устройстве: 7 дней или 1к сообщений (файлы — до 1 ГБ).';

function byteLength(s: string): number {
  return TEXT_ENCODER.encode(s).length;
}

function formatTime(ts: number): string {
  const d = new Date(ts);
  return d.toLocaleTimeString('ru-RU', { hour: '2-digit', minute: '2-digit' });
}

interface Props {
  roomId: string;
  onSend: (text: string, clientMsgId: string, attachments?: Attachment[]) => void;
  // Returns false when the request couldn't be sent (socket closed) — the
  // message stays and we surface that, since deletion only takes effect on the
  // server's echo.
  onDelete: (id: string) => boolean;
}

type VisibleMessage = {
  msg: ChatMessage;
  isSelf: boolean;
  senderName: string;
  showName: boolean;
  showTime: boolean;
  renderedText: ReactNode;
};

export function ChatPanel({ roomId, onSend, onDelete }: Props) {
  const messages = useStore((s) => s.chatByRoom[roomId] ?? EMPTY_MESSAGES);
  const participants = useStore((s) => s.participants);
  const chatSendOptimistic = useStore((s) => s.chatSendOptimistic);
  const chatUpdateUploadProgress = useStore((s) => s.chatUpdateUploadProgress);
  const chatMarkUploadFailed = useStore((s) => s.chatMarkUploadFailed);
  const chatSetAttachments = useStore((s) => s.chatSetAttachments);
  const markAttachmentsDeleted = useStore((s) => s.markAttachmentsDeleted);
  const persistChat = useStore((s) => s.persistChat);
  const loadChatRoom = useStore((s) => s.loadChatRoom);
  const setChatLightboxOpen = useStore((s) => s.setChatLightboxOpen);

  // Hydrate persisted history on mount so chat is visible before joining.
  useEffect(() => {
    loadChatRoom(roomId);
  }, [roomId, loadChatRoom]);

  // Evict over-budget cached blobs once per mount, marking the evicted
  // attachments deleted. (Age-out is handled by chat-history pruning.)
  useEffect(() => {
    void pruneBlobs().then(markAttachmentsDeleted);
  }, [markAttachmentsDeleted]);

  const deleteAttachment = useCallback(
    (uploadId: string) => {
      void deleteBlob(uploadId);
      markAttachmentsDeleted([uploadId]);
    },
    [markAttachmentsDeleted],
  );

  const selfPeerId = useStore(selectSelfPeerId);
  // Stable per-install identity — independent of join state. Lets MessageRow
  // recognise own messages even after we leave the room (when selfPeerId is null).
  const selfClientId = useRef(loadOrCreateClientId()).current;

  const [text, setText] = useState('');
  const [files, setFiles] = useState<File[]>([]);
  const [fileError, setFileError] = useState<string | null>(null);
  const [dragging, setDragging] = useState(false);
  const dragDepth = useRef(0);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const bytes = byteLength(text);
  const overLimit = bytes > CHAT_MAX_BYTES;
  const canSend = (text.trim().length > 0 || files.length > 0) && !overLimit;

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

  const addFiles = useCallback((incoming: File[]) => {
    if (incoming.length === 0) return;
    const tooBig = incoming.some((f) => f.size > MAX_UPLOAD_BYTES);
    const ok = incoming.filter((f) => f.size <= MAX_UPLOAD_BYTES);
    setFileError(
      tooBig
        ? `Файл больше ${Math.round(MAX_UPLOAD_BYTES / (1024 * 1024))} МБ — не добавлен`
        : null,
    );
    setFiles((prev) => [...prev, ...ok].slice(0, CHAT_MAX_ATTACHMENTS));
  }, []);

  const removeFile = useCallback((index: number) => {
    setFiles((prev) => prev.filter((_, i) => i !== index));
  }, []);

  // Uploads each temp-keyed attachment (skipping ones already on a real id),
  // rekeys its cached blob to the server id, then sends the chat message.
  // blobFor supplies each attachment's bytes — the original File on first send,
  // the cached temp blob on retry. On failure the temp blobs are left intact so
  // a retry can re-upload from cache.
  const uploadAttachmentsAndSend = useCallback(
    async (
      clientMsgId: string,
      text: string,
      attachments: Attachment[],
      blobFor: (att: Attachment, index: number) => Promise<Blob | null>,
    ) => {
      try {
        const progress = new Array(attachments.length).fill(0);
        const real = await Promise.all(
          attachments.map(async (att, i) => {
            if (!att.uploadId.startsWith(TEMP_UPLOAD_PREFIX)) {
              progress[i] = 1;
              return att;
            }
            const blob = await blobFor(att, i);
            if (!blob) throw new Error('attachment blob missing');
            const uploadId = await uploadFile(blob, roomId, {
              name: att.name,
              onProgress: (fraction) => {
                progress[i] = fraction;
                const agg = progress.reduce((a, b) => a + b, 0) / attachments.length;
                chatUpdateUploadProgress(roomId, clientMsgId, agg);
              },
            });
            await rekeyBlob(att.uploadId, uploadId);
            return { ...att, uploadId };
          }),
        );
        chatSetAttachments(roomId, clientMsgId, real);
        chatUpdateUploadProgress(roomId, clientMsgId, 1);
        onSend(text, clientMsgId, real);
        persistChat(roomId);
      } catch (err) {
        if ((err as DOMException)?.name === 'AbortError') return;
        chatMarkUploadFailed(roomId, clientMsgId);
        persistChat(roomId);
      }
    },
    [
      roomId,
      onSend,
      chatUpdateUploadProgress,
      chatSetAttachments,
      chatMarkUploadFailed,
      persistChat,
    ],
  );

  const sendMessage = useCallback(
    async (trimmed: string, outgoing: File[]) => {
      if (!selfPeerId) return;
      const clientMsgId = crypto.randomUUID();
      const now = Date.now();
      const selfEntry = participants[selfPeerId];

      const tempAttachments: Attachment[] = await Promise.all(
        outgoing.map(async (file, i) => {
          const uploadId = `${TEMP_UPLOAD_PREFIX}${clientMsgId}:${i}`;
          await putBlob(uploadId, file);
          const kind: AttachmentKind = file.type.startsWith('image/') ? 'image' : 'file';
          if (kind === 'image') {
            const meta = await imageMeta(file);
            return {
              uploadId,
              kind,
              name: file.name,
              mime: file.type,
              size: file.size,
              width: meta?.width,
              height: meta?.height,
              blurThumb: meta?.blurThumb,
            };
          }
          return { uploadId, kind, name: file.name, mime: file.type, size: file.size };
        }),
      );

      chatSendOptimistic(roomId, {
        id: clientMsgId,
        from: selfPeerId,
        text: trimmed,
        ts: now,
        clientMsgId,
        pending: true,
        senderName: selfEntry?.display,
        senderClientId: selfEntry?.clientId,
        attachments: tempAttachments.length ? tempAttachments : undefined,
        uploadProgress: outgoing.length ? 0 : undefined,
      });
      persistChat(roomId);

      if (outgoing.length === 0) {
        onSend(trimmed, clientMsgId);
        return;
      }
      // First send uploads the original Files directly (no cache dependency).
      await uploadAttachmentsAndSend(clientMsgId, trimmed, tempAttachments, (_att, i) =>
        Promise.resolve(outgoing[i]),
      );
    },
    [
      selfPeerId,
      participants,
      roomId,
      chatSendOptimistic,
      persistChat,
      onSend,
      uploadAttachmentsAndSend,
    ],
  );

  // Re-send a failed message, re-uploading its attachments from their cached
  // temp blobs (already-uploaded ones are reused as-is).
  const retryMessage = useCallback(
    async (msg: ChatMessage) => {
      if (!msg.clientMsgId) return;
      chatUpdateUploadProgress(roomId, msg.clientMsgId, 0);
      await uploadAttachmentsAndSend(msg.clientMsgId, msg.text, msg.attachments ?? [], (att) =>
        getBlob(att.uploadId),
      );
    },
    [roomId, chatUpdateUploadProgress, uploadAttachmentsAndSend],
  );

  const handleSend = useCallback(() => {
    const trimmed = text.trim();
    if ((!trimmed && files.length === 0) || overLimit || !selfPeerId) return;
    setText('');
    setFiles([]);
    setFileError(null);
    void sendMessage(trimmed, files);
  }, [text, files, overLimit, selfPeerId, sendMessage]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        handleSend();
      }
    },
    [handleSend],
  );

  const handlePaste = useCallback(
    (e: ClipboardEvent<HTMLTextAreaElement>) => {
      const pasted = Array.from(e.clipboardData.items)
        .filter((item) => item.kind === 'file' && item.type.startsWith('image/'))
        .map((item) => item.getAsFile())
        .filter((f): f is File => f !== null);
      if (pasted.length) {
        e.preventDefault();
        addFiles(pasted);
      }
    },
    [addFiles],
  );

  const onDragEnter = useCallback((e: DragEvent) => {
    if (!Array.from(e.dataTransfer.types).includes('Files')) return;
    dragDepth.current++;
    setDragging(true);
  }, []);
  const onDragLeave = useCallback(() => {
    dragDepth.current = Math.max(0, dragDepth.current - 1);
    if (dragDepth.current === 0) setDragging(false);
  }, []);
  const onDragOver = useCallback((e: DragEvent) => {
    if (Array.from(e.dataTransfer.types).includes('Files')) e.preventDefault();
  }, []);
  const onDrop = useCallback(
    (e: DragEvent) => {
      e.preventDefault();
      dragDepth.current = 0;
      setDragging(false);
      addFiles(Array.from(e.dataTransfer.files));
    },
    [addFiles],
  );

  const [lightboxIndex, setLightboxIndex] = useState<number | null>(null);

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

  // Flat list of every live image in the visible chat, so the lightbox can page
  // across all of them with ←/→ regardless of which message they belong to.
  // Deleted images are excluded — they have no bytes to show.
  const imageList = useMemo<Attachment[]>(
    () =>
      visible.flatMap((v) =>
        (v.msg.attachments ?? []).filter(
          (a) => a.kind === 'image' && !v.msg.deletedUploadIds?.includes(a.uploadId),
        ),
      ),
    [visible],
  );
  const imageListRef = useRef(imageList);
  imageListRef.current = imageList;

  const openLightbox = useCallback((att: Attachment) => {
    const idx = imageListRef.current.findIndex((a) => a.uploadId === att.uploadId);
    if (idx >= 0) setLightboxIndex(idx);
  }, []);
  const closeLightbox = useCallback(() => setLightboxIndex(null), []);

  // Keep the global flag in sync so App's arrow-key handler yields while open.
  useEffect(() => {
    setChatLightboxOpen(lightboxIndex !== null);
    return () => setChatLightboxOpen(false);
  }, [lightboxIndex, setChatLightboxOpen]);

  // Not optimistic — deletion rides the server's chat-deleted echo, one path
  // for us and every online peer.
  const [menu, setMenu] = useState<{ x: number; y: number; id: string } | null>(null);
  const openMessageMenu = useCallback((x: number, y: number, id: string) => {
    setMenu({ x, y, id });
  }, []);
  const closeMessageMenu = useCallback(() => setMenu(null), []);

  // Transient bottom-right notice, auto-dismissed (mirrors PingToast).
  const [notice, setNotice] = useState<string | null>(null);
  const noticeTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(
    () => () => {
      if (noticeTimerRef.current) clearTimeout(noticeTimerRef.current);
    },
    [],
  );
  const showNotice = useCallback((text: string) => {
    setNotice(text);
    if (noticeTimerRef.current) clearTimeout(noticeTimerRef.current);
    noticeTimerRef.current = setTimeout(() => setNotice(null), 3500);
  }, []);

  const confirmDeleteMessage = useCallback(() => {
    if (menu && !onDelete(menu.id)) showNotice('Нет соединения — сообщение не удалено');
    setMenu(null);
  }, [menu, onDelete, showNotice]);

  return (
    <section
      className="card relative p-0! flex flex-col flex-1 min-h-0"
      onDragEnter={onDragEnter}
      onDragLeave={onDragLeave}
      onDragOver={onDragOver}
      onDrop={onDrop}
    >
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
          <MessageRow
            key={row.msg.id}
            row={row}
            roomId={roomId}
            onImageClick={openLightbox}
            onRetry={retryMessage}
            onDeleteAttachment={deleteAttachment}
            onRequestMenu={openMessageMenu}
          />
        ))}
      </div>

      <div className="px-4 pb-4 pt-3 border-t border-line shrink-0">
        {files.length > 0 && (
          <div className="mb-2 flex flex-wrap gap-2">
            {files.map((file, i) => (
              <TrayItem key={`${file.name}-${i}`} file={file} onRemove={() => removeFile(i)} />
            ))}
          </div>
        )}
        <div
          className={`flex gap-1.5 items-end p-1.5 border ${overLimit ? 'border-danger' : 'border-line'} bg-bg-input focus-within:border-accent transition-[border-color] duration-150`}
        >
          <button
            onClick={() => fileInputRef.current?.click()}
            disabled={!selfPeerId || files.length >= CHAT_MAX_ATTACHMENTS}
            className="btn btn-secondary shrink-0 grid place-items-center p-0! border-0"
            style={{ width: 40, height: 40 }}
            aria-label="Прикрепить файл"
            title="Прикрепить файл"
          >
            <Paperclip size={22} />
          </button>
          <input
            ref={fileInputRef}
            type="file"
            multiple
            hidden
            onChange={(e) => {
              addFiles(Array.from(e.target.files ?? []));
              e.target.value = '';
            }}
          />
          <textarea
            ref={textareaRef}
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={handleKeyDown}
            onPaste={handlePaste}
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
        {fileError && <div className="mt-1 text-[11px] text-danger">{fileError}</div>}
        {bytes > CHAT_MAX_BYTES * 0.8 && (
          <div
            className={`mt-1 text-right text-[11px] tabular-nums ${overLimit ? 'text-danger' : 'text-muted-2'}`}
          >
            {bytes}/{CHAT_MAX_BYTES}
          </div>
        )}
      </div>

      {dragging && (
        <div className="pointer-events-none absolute inset-0 z-30 grid place-items-center border-2 border-dashed border-accent bg-bg-0/80 text-accent text-[14px] uppercase tracking-[0.14em]">
          Отпустите файлы, чтобы прикрепить
        </div>
      )}

      <ImageLightbox
        images={imageList}
        index={lightboxIndex}
        roomId={roomId}
        onClose={closeLightbox}
        onNavigate={setLightboxIndex}
        onDelete={(att) => deleteAttachment(att.uploadId)}
      />

      {menu && (
        <MessageContextMenu
          x={menu.x}
          y={menu.y}
          onDelete={confirmDeleteMessage}
          onClose={closeMessageMenu}
        />
      )}

      {notice && (
        <div
          className="fixed bottom-4 right-4 z-50 flex items-center gap-2 px-4 py-2.5 bg-bg-1 border border-danger text-danger text-[13px] shadow-lg pointer-events-none animate-[fadeIn_0.15s_ease-out]"
          role="status"
          aria-live="polite"
        >
          <WifiOff size={16} aria-hidden />
          <span>{notice}</span>
        </div>
      )}
    </section>
  );
}

const TrayItem = memo(function TrayItem({ file, onRemove }: { file: File; onRemove: () => void }) {
  const isImage = file.type.startsWith('image/');
  const [previewUrl, setPreviewUrl] = useState<string | null>(null);
  useEffect(() => {
    if (!isImage) return;
    const url = URL.createObjectURL(file);
    setPreviewUrl(url);
    return () => URL.revokeObjectURL(url);
  }, [file, isImage]);

  return (
    <div className="relative h-16 w-16 shrink-0 overflow-hidden border border-line bg-bg-2">
      {isImage && previewUrl ? (
        <img src={previewUrl} alt={file.name} className="h-full w-full object-cover" />
      ) : (
        <div className="flex h-full w-full flex-col items-center justify-center px-1 text-center">
          <span className="truncate text-[10px] text-body w-full">{file.name}</span>
          <span className="text-[9px] text-muted-2">{formatFileSize(file.size)}</span>
        </div>
      )}
      <button
        onClick={onRemove}
        aria-label="Убрать"
        className="absolute right-0.5 top-0.5 grid h-5 w-5 place-items-center bg-black/70 text-white hover:bg-black"
      >
        <X size={12} />
      </button>
    </div>
  );
});

const MessageRow = memo(function MessageRow({
  row,
  roomId,
  onImageClick,
  onRetry,
  onDeleteAttachment,
  onRequestMenu,
}: {
  row: VisibleMessage;
  roomId: string;
  onImageClick: (att: Attachment) => void;
  onRetry: (msg: ChatMessage) => void;
  onDeleteAttachment: (uploadId: string) => void;
  onRequestMenu: (x: number, y: number, id: string) => void;
}) {
  const { msg, isSelf, senderName, showName, showTime, renderedText } = row;
  const attachments = msg.attachments ?? [];
  const images = attachments.filter((a) => a.kind === 'image');
  const fileCards = attachments.filter((a) => a.kind === 'file');
  const isDeleted = (uploadId: string) => msg.deletedUploadIds?.includes(uploadId) ?? false;
  const uploading =
    msg.pending && !msg.uploadFailed && msg.uploadProgress !== undefined && msg.uploadProgress < 1;

  // Only confirmed own messages (server id assigned) can be retracted.
  const canDelete = isSelf && !msg.pending;
  const longPressRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const cancelLongPress = () => {
    if (longPressRef.current) {
      clearTimeout(longPressRef.current);
      longPressRef.current = null;
    }
  };
  useEffect(() => cancelLongPress, []);

  const menuHandlers = canDelete
    ? {
        onContextMenu: (e: React.MouseEvent) => {
          e.preventDefault();
          onRequestMenu(e.clientX, e.clientY, msg.id);
        },
        onTouchStart: (e: React.TouchEvent) => {
          const { clientX, clientY } = e.touches[0];
          cancelLongPress();
          longPressRef.current = setTimeout(() => {
            longPressRef.current = null;
            onRequestMenu(clientX, clientY, msg.id);
          }, 500);
        },
        onTouchEnd: cancelLongPress,
        onTouchMove: cancelLongPress,
        onTouchCancel: cancelLongPress,
      }
    : undefined;

  return (
    <div
      {...menuHandlers}
      className={`px-2 ${showName ? 'pt-2' : ''} ${msg.pending ? 'opacity-50' : ''}`}
    >
      {showName && (
        <div
          className={`text-[11px] font-bold uppercase tracking-[0.14em] truncate mb-0.5 ${isSelf ? 'text-accent' : 'text-muted'}`}
        >
          {senderName}
        </div>
      )}

      {attachments.length > 0 && (
        <div className="mb-1 flex flex-col gap-1.5">
          {images.length === 1 && (
            <AttachmentImage
              attachment={images[0]}
              roomId={roomId}
              deleted={isDeleted(images[0].uploadId)}
              onClick={() => onImageClick(images[0])}
              onDelete={() => onDeleteAttachment(images[0].uploadId)}
            />
          )}
          {images.length > 1 && (
            <AttachmentAlbum
              images={images}
              roomId={roomId}
              onOpen={onImageClick}
              onDelete={(att) => onDeleteAttachment(att.uploadId)}
              isDeleted={isDeleted}
            />
          )}
          {fileCards.map((att) => (
            <AttachmentFileCard
              key={att.uploadId}
              attachment={att}
              roomId={roomId}
              deleted={isDeleted(att.uploadId)}
              onDelete={() => onDeleteAttachment(att.uploadId)}
            />
          ))}
        </div>
      )}

      {(msg.text || showTime) && (
        <div className="grid grid-cols-[minmax(0,1fr)_auto] gap-x-3 items-baseline">
          {msg.text ? (
            <p className="m-0 text-[17px] text-body break-words whitespace-pre-wrap">
              {renderedText}
            </p>
          ) : (
            <span />
          )}
          {showTime && (
            <span className="text-[11px] text-muted-2 tabular-nums shrink-0">
              {formatTime(msg.ts)}
            </span>
          )}
        </div>
      )}

      {uploading && (
        <div className="mt-1 h-1 overflow-hidden bg-bg-3">
          <div
            className="h-full bg-accent transition-[width] duration-150"
            style={{ width: `${Math.round((msg.uploadProgress ?? 0) * 100)}%` }}
          />
        </div>
      )}
      {msg.uploadFailed && (
        <div className="mt-1 flex items-center gap-2 text-[12px] text-danger">
          <span>Не удалось загрузить</span>
          <button onClick={() => onRetry(msg)} className="underline hover:opacity-80">
            Повторить
          </button>
        </div>
      )}
    </div>
  );
});

function MessageContextMenu({
  x,
  y,
  onDelete,
  onClose,
}: {
  x: number;
  y: number;
  onDelete: () => void;
  onClose: () => void;
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    window.addEventListener('resize', onClose);
    // Capture-phase so the chat list's own scroll dismisses the menu too.
    window.addEventListener('scroll', onClose, true);
    return () => {
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('resize', onClose);
      window.removeEventListener('scroll', onClose, true);
    };
  }, [onClose]);

  const left = Math.min(x, window.innerWidth - 200);
  const top = Math.min(y, window.innerHeight - 64);

  return (
    <>
      <div
        className="fixed inset-0 z-40"
        onPointerDown={onClose}
        onContextMenu={(e) => {
          e.preventDefault();
          onClose();
        }}
      />
      <div
        role="menu"
        className="fixed z-50 min-w-[180px] border border-line-strong bg-bg-3 py-1 shadow-lg"
        style={{ left, top }}
      >
        <button
          role="menuitem"
          onClick={onDelete}
          className="flex w-full items-center gap-2 px-3 py-2 text-left text-[14px] text-danger hover:bg-bg-2"
        >
          <Trash2 size={15} />
          Удалить
        </button>
      </div>
    </>
  );
}
