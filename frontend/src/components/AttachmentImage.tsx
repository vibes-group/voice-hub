import { useEffect, useState, type CSSProperties } from 'react';
import { ImageOff, Trash2 } from 'lucide-react';
import type { Attachment } from '../sfu/protocol';
import { useAttachmentUrl } from '../hooks/useAttachmentUrl';

type Props = {
  attachment: Attachment;
  roomId: string;
  onClick?: () => void;
  onDelete?: () => void;
  deleted?: boolean;
  // When true the image fills its parent cell (album grid) instead of sizing
  // itself from its intrinsic aspect ratio.
  fill?: boolean;
};

const MAX_WIDTH = 320;

export function AttachmentImage({
  attachment,
  roomId,
  onClick,
  onDelete,
  deleted = false,
  fill = false,
}: Props) {
  const { url, status, reload } = useAttachmentUrl(attachment.uploadId, roomId, !deleted);
  const [loaded, setLoaded] = useState(false);

  // Restart the fade-in when the resolved URL changes (e.g. after a retry).
  useEffect(() => {
    setLoaded(false);
  }, [url]);

  const aspectRatio =
    attachment.width && attachment.height
      ? `${attachment.width} / ${attachment.height}`
      : undefined;
  const containerStyle: CSSProperties = {
    maxWidth: fill ? undefined : MAX_WIDTH,
    aspectRatio: fill ? undefined : aspectRatio,
    minHeight: fill || aspectRatio ? undefined : 80,
  };
  const containerClass = `group relative overflow-hidden bg-bg-2 ${fill ? 'h-full w-full' : 'block w-full'}`;

  if (deleted) {
    return (
      <div className={containerClass} style={containerStyle}>
        <div className="absolute inset-0 grid place-items-center gap-1 text-muted-2">
          <ImageOff size={22} />
          <span className="text-[12px]">Файл удалён</span>
        </div>
      </div>
    );
  }

  return (
    <div className={containerClass} style={containerStyle}>
      <button
        type="button"
        onClick={onClick}
        className="block h-full w-full"
        aria-label={attachment.name}
      >
        {attachment.blurThumb ? (
          <img
            src={attachment.blurThumb}
            aria-hidden
            alt=""
            className="absolute inset-0 h-full w-full object-cover"
            style={{ filter: 'blur(16px)', transform: 'scale(1.1)' }}
          />
        ) : (
          status === 'loading' && <div className="absolute inset-0 animate-pulse bg-bg-3" />
        )}

        {url && (
          <img
            src={url}
            alt={attachment.name}
            onLoad={() => setLoaded(true)}
            className="relative h-full w-full object-cover transition-opacity duration-300"
            style={{ opacity: loaded ? 1 : 0 }}
          />
        )}

        {status === 'unavailable' && (
          <div className="absolute inset-0 grid place-items-center bg-bg-2 px-2 text-center text-[12px] text-muted-2">
            Файл недоступен
          </div>
        )}
        {status === 'error' && (
          <div
            className="absolute inset-0 grid place-items-center bg-bg-2 px-2 text-center text-[12px] text-muted-2"
            onClick={(e) => {
              e.stopPropagation();
              reload();
            }}
          >
            Ошибка загрузки · повторить
          </div>
        )}
      </button>

      {onDelete && (
        <button
          type="button"
          aria-label="Удалить файл"
          title="Удалить (освободить место)"
          onClick={(e) => {
            e.stopPropagation();
            onDelete();
          }}
          className="absolute right-1 top-1 hidden place-items-center bg-black/60 p-1 text-white transition-colors hover:bg-black/80 group-hover:grid"
        >
          <Trash2 size={16} />
        </button>
      )}
    </div>
  );
}
