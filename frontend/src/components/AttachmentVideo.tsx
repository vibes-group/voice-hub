import { Play } from 'lucide-react';
import type { Attachment } from '../sfu/protocol';
import { mediaBoxStyle } from '../utils/mediaBox';
import { formatFileSize } from './AttachmentFileCard';
import { MediaActions } from './MediaActions';

type Props = {
  attachment: Attachment;
  roomId: string;
  onClick: () => void;
  onDelete?: () => void;
};

// Video poster in the message list; it plays in the lightbox. Unlike images the
// bytes are not fetched here: a video can be up to the upload limit in size.
export function AttachmentVideo({ attachment, roomId, onClick, onDelete }: Props) {
  return (
    <div className="group relative overflow-hidden bg-black" style={mediaBoxStyle(attachment)}>
      {attachment.blurThumb && (
        <img
          src={attachment.blurThumb}
          aria-hidden
          alt=""
          className="absolute inset-0 h-full w-full object-cover"
          style={{ filter: 'blur(16px)', transform: 'scale(1.1)' }}
        />
      )}

      <button
        type="button"
        onClick={onClick}
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

      <MediaActions attachment={attachment} roomId={roomId} onDelete={onDelete} />
    </div>
  );
}
