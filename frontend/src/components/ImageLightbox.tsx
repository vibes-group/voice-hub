import { useEffect, useState } from 'react';
import { ChevronLeft, ChevronRight, Download, Trash2, X, ZoomIn, ZoomOut } from 'lucide-react';
import type { Attachment } from '../sfu/protocol';
import { useAttachmentUrl, useDownload } from '../hooks/useAttachmentUrl';
import { useImageZoom } from '../hooks/useImageZoom';
import { MAX_SCALE } from '../utils/zoom';
import { isInlineVideo } from '../utils/mediaBox';

const ZOOM_STEP = 1.5;

type Props = {
  images: Attachment[];
  index: number | null;
  roomId: string;
  onClose: () => void;
  onNavigate: (index: number) => void;
  onDelete: (att: Attachment) => void;
};

// Fullscreen image and video viewer (modeled on ScreenShareFocused). ←/→ page
// across every image and video in the visible chat, not just one message's album.
// Images zoom with the wheel, the magnifier buttons or a double click, and pan by drag.
export function ImageLightbox({ images, index, roomId, onClose, onNavigate, onDelete }: Props) {
  const open = index !== null && index >= 0 && index < images.length;

  useEffect(() => {
    if (!open) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === 'Escape') {
        onClose();
        return;
      }
      // A focused player seeks with the arrows.
      if (e.target instanceof HTMLVideoElement) return;
      if (e.key === 'ArrowRight' && images.length > 1) {
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

  // The page behind is taller than the viewport; keep it from scrolling.
  useEffect(() => {
    const root = document.documentElement;
    const previous = root.style.overflow;
    root.style.overflow = 'hidden';
    return () => {
      root.style.overflow = previous;
    };
  }, []);
  // The container is supported but its codec doesn't decode here.
  const [undecodableId, setUndecodableId] = useState<string | null>(null);
  const { zoom, stageRef, imgProps, zoomStep, reset, wasDragged } = useImageZoom(current.uploadId);
  const isImage = !!url && !isInlineVideo(current.mime) && undecodableId !== current.uploadId;

  return (
    <div
      className="fixed inset-0 z-50 flex flex-col bg-black/95"
      onClick={() => {
        if (!wasDragged()) onClose();
      }}
    >
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
          {isImage && (
            <div className="flex items-center">
              <button
                type="button"
                onClick={() => zoomStep(1 / ZOOM_STEP)}
                disabled={zoom.scale <= 1}
                aria-label="Уменьшить"
                title="Уменьшить"
                className="rounded p-1 mobile:p-3.5 hover:bg-white/10 disabled:opacity-40 disabled:hover:bg-transparent"
              >
                <ZoomOut size={18} strokeWidth={2.25} />
              </button>
              <button
                type="button"
                onClick={reset}
                title="Вписать в экран"
                className="w-12 rounded py-1 text-center text-xs tabular-nums hover:bg-white/10 mobile:hidden"
              >
                {Math.round(zoom.scale * 100)}%
              </button>
              <button
                type="button"
                onClick={() => zoomStep(ZOOM_STEP)}
                disabled={zoom.scale >= MAX_SCALE}
                aria-label="Увеличить"
                title="Увеличить (или колёсико мыши)"
                className="rounded p-1 mobile:p-3.5 hover:bg-white/10 disabled:opacity-40 disabled:hover:bg-transparent"
              >
                <ZoomIn size={18} strokeWidth={2.25} />
              </button>
            </div>
          )}
          <DownloadButton key={current.uploadId} attachment={current} roomId={roomId} />
          <button
            type="button"
            onClick={() => {
              onDelete(current);
              onClose();
            }}
            aria-label="Удалить"
            title="Удалить (освободить место)"
            className="rounded p-1 mobile:p-3.5 hover:bg-white/10 hover:text-danger"
          >
            <Trash2 size={18} strokeWidth={2.25} />
          </button>
          <button
            type="button"
            onClick={onClose}
            aria-label="Закрыть"
            className="rounded p-1 mobile:p-3.5 hover:bg-white/10"
          >
            <X size={18} strokeWidth={2.25} />
          </button>
        </div>
      </header>

      <div
        ref={stageRef}
        className="relative grid min-h-0 flex-1 overflow-hidden grid-cols-[minmax(0,1fr)] grid-rows-[minmax(0,1fr)] place-items-center px-4 pb-4"
      >
        {undecodableId === current.uploadId ? (
          <div className="text-sm text-zinc-400">Не воспроизводится — скачайте файл</div>
        ) : url && isInlineVideo(current.mime) ? (
          <video
            key={current.uploadId}
            src={url}
            controls
            autoPlay
            playsInline
            className="max-h-full max-w-full"
            onClick={(e) => e.stopPropagation()}
            onError={() => setUndecodableId(current.uploadId)}
          />
        ) : url ? (
          <img
            {...imgProps}
            src={url}
            alt={current.name}
            className={`max-h-full max-w-full object-contain select-none ${zoom.scale > 1 ? 'cursor-grab active:cursor-grabbing' : ''}`}
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

function DownloadButton({ attachment, roomId }: { attachment: Attachment; roomId: string }) {
  const { state, start } = useDownload([attachment], roomId);
  return (
    <button
      type="button"
      onClick={start}
      disabled={state === 'busy'}
      aria-label="Скачать"
      title={state === 'failed' ? 'Не удалось скачать · повторить' : 'Скачать'}
      className={`rounded p-1 mobile:p-3.5 hover:bg-white/10 disabled:opacity-60 ${state === 'failed' ? 'text-danger' : ''}`}
    >
      <Download size={18} strokeWidth={2.25} />
    </button>
  );
}
