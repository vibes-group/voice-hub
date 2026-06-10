import { useScreenShareStore } from '../store/useScreenShareStore';
import { ScreenShareTile } from './ScreenShareTile';
import { selectSelfPeerId, useStore } from '../store/useStore';

type Props = {
  onTileClick: (publisherId: string) => void;
};

export function ScreenShareGallery({ onTileClick }: Props) {
  const shares = useScreenShareStore((s) => s.shares);
  const selfId = useStore(selectSelfPeerId);

  const list = Array.from(shares.values()).filter((share) => share.publisherId !== selfId);
  if (list.length === 0) return null;

  return (
    <section className="rounded-lg border border-zinc-800 bg-zinc-900/50 p-3 shrink-0 max-h-[200px] overflow-y-auto">
      <h3 className="text-xs font-semibold text-zinc-400 uppercase tracking-wider mb-2 px-1">
        Демонстрации экрана
      </h3>
      <div className="grid gap-3 grid-cols-[repeat(auto-fill,minmax(180px,1fr))]">
        {list.map((share) => (
          <ScreenShareTile
            key={share.publisherId}
            publisherId={share.publisherId}
            hasSystemAudio={share.hasSystemAudio}
            onClick={() => onTileClick(share.publisherId)}
          />
        ))}
      </div>
    </section>
  );
}
