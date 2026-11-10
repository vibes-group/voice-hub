import { useState } from 'react';
import { Download, Play, Trash2 } from 'lucide-react';
import type { Attachment } from '../sfu/protocol';
import { downloadAttachment, useAttachmentUrl } from '../hooks/useAttachmentUrl';
import { mediaBoxStyle } from '../utils/mediaBox';
import { formatFileSize } from './AttachmentFileCard';

type Props = {
  attachment: Attachment;
  roomId: string;
  onDelete?: () => void;
};

const OVERLAY_BUTTON =
  'grid place-items-center bg-black/60 p-1 text-white transition-colors hover:bg-black/80';
const NOTICE =
  'absolute inset-0 grid place-items-center bg-bg-2 px-2 text-center text-[12px] text-muted-2';

// Inline video player. Unlike images the bytes are fetched only on play: a
// video can be up to the upload limit in size.
export function AttachmentVideo({ attachment, roomId, onDelete }: Props) {
  const [active, setActive] = useState(false);
  const { url, status, reload } = useAttachmentUrl(attachment.uploadId, roomId, active);
  // Dimensions measured by the player, for attachments sent without them.
  const [natural, setNatural] = useState<{ width: number; height: number } | null>(null);
  // The container is supported but its codec doesn't decode here.
  const [undecodable, setUndecodable] = useState(false);

  return (
    <div
      className="group relative overflow-hidden bg-black"
      style={mediaBoxStyle(
        attachment.width || natural?.width,
        attachment.height || natural?.height,
      )}
    >
      {attachment.blurThumb && !url && (
        <img
          src={attachment.blurThumb}
          aria-hidden
          alt=""
          className="absolute inset-0 h-full w-full object-cover"
          style={{ filter: 'blur(16px)', transform: 'scale(1.1)' }}
        />
      )}

      {!active && (
        <button
          type="button"
          onClick={() => setActive(true)}
          aria-label={`Воспроизвести ${attachment.name}`}
          className="absolute inset-0 grid place-items-center"
        >
          <span className="grid h-12 w-12 place-items-center rounded-full bg-black/60 text-white">
            <Play size={22} fill="currentColor" />
          </span>
          <span className="absolute bottom-1 left-1 bg-black/60 px-1.5 py-0.5 text-[11px] text-white tabular-nums">
            {formatFileSize(attachment.size)}
          </span>
        </button>
      )}

      {active && status === 'loading' && (
        <div className="absolute inset-0 grid animate-pulse place-items-center text-[12px] text-white">
          Загрузка…
        </div>
      )}
      {active && status === 'unavailable' && <div className={NOTICE}>Файл недоступен</div>}
      {active && status === 'error' && (
        <button type="button" className={NOTICE} onClick={reload}>
          Ошибка загрузки · повторить
        </button>
      )}

      {url && !undecodable && (
        <video
          src={url}
          controls
          autoPlay
          playsInline
          onError={() => setUndecodable(true)}
          onLoadedMetadata={(e) => {
            const { videoWidth: width, videoHeight: height } = e.currentTarget;
            setNatural({ width, height });
          }}
          className="absolute inset-0 h-full w-full object-contain"
        />
      )}

      {undecodable && (
        <button
          type="button"
          className={NOTICE}
          onClick={() => void downloadAttachment(attachment, roomId)}
        >
          Не воспроизводится · скачать
        </button>
      )}

      <div className="absolute right-1 top-1 hidden gap-1 group-hover:flex">
        <button
          type="button"
          aria-label="Скачать"
          title="Скачать"
          onClick={() => void downloadAttachment(attachment, roomId)}
          className={OVERLAY_BUTTON}
        >
          <Download size={16} />
        </button>
        {onDelete && (
          <button
            type="button"
            aria-label="Удалить файл"
            title="Удалить (освободить место)"
            onClick={onDelete}
            className={OVERLAY_BUTTON}
          >
            <Trash2 size={16} />
          </button>
        )}
      </div>
    </div>
  );
}
