import { useState, type ReactNode } from 'react';
import { Archive, Download, File, FileText, Music, Trash2, Video } from 'lucide-react';
import type { Attachment } from '../sfu/protocol';
import { downloadAttachment } from '../hooks/useAttachmentUrl';

type Props = {
  attachment: Attachment;
  roomId: string;
  onDelete?: () => void;
  deleted?: boolean;
};

export function formatFileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} Б`;
  const units = ['КБ', 'МБ', 'ГБ'];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${value.toFixed(value < 10 ? 1 : 0)} ${units[unit]}`;
}

function fileIcon(mime: string): ReactNode {
  const props = { size: 22, className: 'shrink-0 text-muted' };
  if (mime.startsWith('video/')) return <Video {...props} />;
  if (mime.startsWith('audio/')) return <Music {...props} />;
  if (mime.startsWith('text/') || mime === 'application/pdf') return <FileText {...props} />;
  if (/zip|rar|7z|tar|gzip|compress/.test(mime)) return <Archive {...props} />;
  return <File {...props} />;
}

export function AttachmentFileCard({ attachment, roomId, onDelete, deleted = false }: Props) {
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);

  if (deleted) {
    return (
      <div className="flex w-full max-w-[320px] items-center gap-3 border border-line bg-bg-2 px-3 py-2 text-muted-2">
        <File size={22} className="shrink-0" />
        <span className="min-w-0 flex-1">
          <span className="block truncate text-[14px] line-through" title={attachment.name}>
            {attachment.name}
          </span>
          <span className="block text-[11px]">Файл удалён</span>
        </span>
      </div>
    );
  }

  async function handleDownload() {
    if (busy) return;
    setBusy(true);
    setFailed(false);
    try {
      await downloadAttachment(attachment, roomId);
    } catch {
      setFailed(true);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="flex w-full max-w-[320px] items-stretch border border-line bg-bg-2">
      <button
        type="button"
        onClick={handleDownload}
        disabled={busy}
        className="group/dl flex min-w-0 flex-1 cursor-pointer items-center gap-3 px-3 py-2 text-left transition-colors hover:bg-bg-3 disabled:cursor-default disabled:opacity-60"
      >
        {fileIcon(attachment.mime)}
        <span className="min-w-0 flex-1">
          <span className="block truncate text-[14px] text-body" title={attachment.name}>
            {attachment.name}
          </span>
          <span className="block text-[11px] text-muted-2 tabular-nums">
            {failed ? 'Не удалось скачать' : formatFileSize(attachment.size)}
          </span>
        </span>
        <span className="grid h-8 w-8 shrink-0 place-items-center border border-line text-muted-2 transition-colors group-hover/dl:border-accent group-hover/dl:text-accent">
          <Download size={18} />
        </span>
      </button>
      {onDelete && (
        <button
          type="button"
          onClick={onDelete}
          aria-label="Удалить файл"
          title="Удалить (освободить место)"
          className="flex shrink-0 items-center border-l border-line px-2.5 text-muted-2 transition-colors hover:bg-bg-3 hover:text-danger"
        >
          <Trash2 size={16} />
        </button>
      )}
    </div>
  );
}
