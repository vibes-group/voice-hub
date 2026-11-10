import { useEffect, useState } from 'react';
import { ImageOff } from 'lucide-react';
import type { Attachment } from '../sfu/protocol';
import { useAttachmentUrl } from '../hooks/useAttachmentUrl';
import { MediaActions } from './MediaActions';
import { mediaBoxStyle, type MediaSize } from '../utils/mediaBox';

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

  // Dimensions measured on load, for attachments sent without them.
  const [natural, setNatural] = useState<MediaSize | null>(null);
  const containerStyle = fill ? undefined : mediaBoxStyle(attachment, natural);
  const containerClass = `group overflow-hidden bg-bg-2 ${fill ? 'absolute inset-0' : 'relative'}`;

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
        className="absolute inset-0"
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
            onLoad={(e) => {
              const { naturalWidth: width, naturalHeight: height } = e.currentTarget;
              setNatural({ width, height });
              setLoaded(true);
            }}
            className="absolute inset-0 h-full w-full object-cover transition-opacity duration-300"
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

      <MediaActions attachment={attachment} roomId={roomId} onDelete={onDelete} />
    </div>
  );
}
