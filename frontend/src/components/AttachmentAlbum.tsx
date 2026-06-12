import type { Attachment } from '../sfu/protocol';
import { AttachmentImage } from './AttachmentImage';

type Props = {
  images: Attachment[];
  roomId: string;
  onOpen: (att: Attachment) => void;
  onDelete: (att: Attachment) => void;
  isDeleted: (uploadId: string) => boolean;
};

const WRAP = 'grid gap-0.5 max-w-[320px] overflow-hidden';

// Telegram-style album grids for 2 / 3 / 4+ images.
export function AttachmentAlbum({ images, roomId, onOpen, onDelete, isDeleted }: Props) {
  const tile = (att: Attachment) => (
    <AttachmentImage
      attachment={att}
      roomId={roomId}
      fill
      deleted={isDeleted(att.uploadId)}
      onClick={() => onOpen(att)}
      onDelete={() => onDelete(att)}
    />
  );

  const cell = (att: Attachment, overlay?: number) => (
    <div key={att.uploadId} className="relative">
      {tile(att)}
      {overlay !== undefined && overlay > 0 && (
        <div className="pointer-events-none absolute inset-0 grid place-items-center bg-black/60 text-2xl font-bold text-white">
          +{overlay}
        </div>
      )}
    </div>
  );

  if (images.length === 2) {
    return (
      <div className={`${WRAP} grid-cols-2`}>
        {images.map((img) => (
          <div key={img.uploadId} className="aspect-square">
            {tile(img)}
          </div>
        ))}
      </div>
    );
  }

  if (images.length === 3) {
    return (
      <div className={`${WRAP} grid-cols-2 grid-rows-2 aspect-[3/2]`}>
        <div className="row-span-2">{cell(images[0])}</div>
        {cell(images[1])}
        {cell(images[2])}
      </div>
    );
  }

  const shown = images.slice(0, 4);
  const extra = images.length - 4;
  return (
    <div className={`${WRAP} grid-cols-2 grid-rows-2 aspect-square`}>
      {shown.map((img, i) => cell(img, i === 3 ? extra : undefined))}
    </div>
  );
}
