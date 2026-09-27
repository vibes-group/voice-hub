import { ScreenShare, Volume2 } from 'lucide-react';
import { useStore } from '../store/useStore';

type Props = {
  publisherId: string;
  hasSystemAudio: boolean;
  onClick: () => void;
};

export function ScreenShareTile({ publisherId, hasSystemAudio, onClick }: Props) {
  const display = useStore(
    (s) => s.participants[publisherId]?.display ?? `user-${publisherId}`,
  );

  return (
    <button
      type="button"
      onClick={onClick}
      className="flex flex-col items-center justify-center gap-2
        rounded-md border border-zinc-700 bg-zinc-800/60 hover:bg-zinc-700/60
        p-4 transition aspect-video text-zinc-200
        mobile:flex-row mobile:justify-start mobile:gap-3 mobile:p-3 mobile:aspect-auto"
    >
      <ScreenShare size={28} strokeWidth={2} className="text-emerald-400" />
      <span className="text-sm font-medium truncate max-w-full">{display}</span>
      <span className="flex items-center gap-1 text-xs text-zinc-400">
        Шарит экран
        {hasSystemAudio && <Volume2 size={12} strokeWidth={2.25} />}
      </span>
    </button>
  );
}
