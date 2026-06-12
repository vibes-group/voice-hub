import { useEffect } from 'react';
import { ChevronLeft, ChevronRight, Download, Trash2, X } from 'lucide-react';
import type { Attachment } from '../sfu/protocol';
import { useAttachmentUrl, downloadAttachment } from '../hooks/useAttachmentUrl';

type Props = {
  images: Attachment[];
  index: number | null;
  roomId: string;
  onClose: () => void;
  onNavigate: (index: number) => void;
  onDelete: (att: Attachment) => void;
};

// Fullscreen image viewer (modeled on ScreenShareFocused). ←/→ page across
// every image in the visible chat, not just one message's album.
export function ImageLightbox({ images, index, roomId, onClose, onNavigate, onDelete }: Props) {
  const open = index !== null && index >= 0 && index < images.length;

  useEffect(() => {
    if (!open) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === 'Escape') {
        onClose();
      } else if (e.key === 'ArrowRight' && images.length > 1) {
        e.preventDefault();
        onNavigate((index! + 1) % images.length);
      } else if (e.key === 'ArrowLeft' && images.length > 1) {
        e.preventDefault();
        onNavigate((index! - 1 + images.length) % images.length);
      }
    }
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open, index, images.length, onClose, onNavigate]);

  if (!open) return null;

  return (
    <Viewer
      images={images}
      index={index!}
      roomId={roomId}
      onClose={onClose}
      onNavigate={onNavigate}
      onDelete={onDelete}
    />
  );
}

function Viewer({
  images,
  index,
  roomId,
  onClose,
  onNavigate,
  onDelete,
}: {
  images: Attachment[];
  index: number;
  roomId: string;
  onClose: () => void;
  onNavigate: (index: number) => void;
  onDelete: (att: Attachment) => void;
}) {
  const current = images[index];
  const { url, status } = useAttachmentUrl(current.uploadId, roomId);
  const multiple = images.length > 1;

  return (
    <div className="fixed inset-0 z-50 flex flex-col bg-black/95" onClick={onClose}>
      <header
        className="flex items-center justify-between px-4 py-2 text-zinc-200"
        onClick={(e) => e.stopPropagation()}
      >
        <span className="truncate text-sm font-medium">
          {current.name}
          {multiple && (
            <span className="ml-2 text-zinc-400">
              {index + 1}/{images.length}
            </span>
          )}
        </span>
        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={() => void downloadAttachment(current, roomId)}
            aria-label="Скачать"
            className="rounded p-1 hover:bg-white/10"
          >
            <Download size={18} strokeWidth={2.25} />
          </button>
          <button
            type="button"
            onClick={() => {
              onDelete(current);
              onClose();
            }}
            aria-label="Удалить"
            title="Удалить (освободить место)"
            className="rounded p-1 hover:bg-white/10 hover:text-danger"
          >
            <Trash2 size={18} strokeWidth={2.25} />
          </button>
          <button
            type="button"
            onClick={onClose}
            aria-label="Закрыть"
            className="rounded p-1 hover:bg-white/10"
          >
            <X size={18} strokeWidth={2.25} />
          </button>
        </div>
      </header>

      <div className="relative grid min-h-0 flex-1 place-items-center px-4 pb-4">
        {url ? (
          <img
            src={url}
            alt={current.name}
            className="max-h-full max-w-full object-contain"
            onClick={(e) => e.stopPropagation()}
          />
        ) : (
          <div className="text-sm text-zinc-400">
            {status === 'loading' ? 'Загрузка…' : 'Файл недоступен'}
          </div>
        )}

        {multiple && (
          <>
            <button
              type="button"
              aria-label="Предыдущее"
              onClick={(e) => {
                e.stopPropagation();
                onNavigate((index - 1 + images.length) % images.length);
              }}
              className="absolute left-2 rounded-full bg-black/40 p-2 text-zinc-200 hover:bg-black/70"
            >
              <ChevronLeft size={28} />
            </button>
            <button
              type="button"
              aria-label="Следующее"
              onClick={(e) => {
                e.stopPropagation();
                onNavigate((index + 1) % images.length);
              }}
              className="absolute right-2 rounded-full bg-black/40 p-2 text-zinc-200 hover:bg-black/70"
            >
              <ChevronRight size={28} />
            </button>
          </>
        )}
      </div>
    </div>
  );
}
