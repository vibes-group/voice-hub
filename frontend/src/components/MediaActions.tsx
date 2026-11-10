import { Download, Trash2 } from 'lucide-react';
import type { Attachment } from '../sfu/protocol';
import { useDownload } from '../hooks/useAttachmentUrl';

type Props = {
  attachment: Attachment;
  roomId: string;
  onDelete?: () => void;
};

const BUTTON = 'grid place-items-center bg-black/60 p-1 transition-colors hover:bg-black/80';

// Download / delete buttons in the top-right corner of a photo or video tile.
// The tile must be `group relative`: they show on hover or keyboard focus.
// Without hover (touch screens) download is always shown and delete is left to
// the lightbox: it is irreversible and too easy to hit on a small tile.
export function MediaActions({ attachment, roomId, onDelete }: Props) {
  const { state, start } = useDownload([attachment], roomId);

  return (
    <div className="absolute right-1 top-1 hidden gap-1 group-hover:flex group-has-[:focus-visible]:flex [@media(hover:none)]:flex">
      <button
        type="button"
        aria-label="Скачать"
        title={state === 'failed' ? 'Не удалось скачать · повторить' : 'Скачать'}
        disabled={state === 'busy'}
        onClick={start}
        className={`${BUTTON} disabled:opacity-60 ${state === 'failed' ? 'text-danger' : 'text-white'}`}
      >
        <Download size={16} />
      </button>
      {onDelete && (
        <button
          type="button"
          aria-label="Удалить файл"
          title="Удалить (освободить место)"
          onClick={onDelete}
          className={`${BUTTON} text-white [@media(hover:none)]:hidden`}
        >
          <Trash2 size={16} />
        </button>
      )}
    </div>
  );
}
