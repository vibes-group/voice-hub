import { useRef, useState } from 'react';
import { ChevronDown, ScreenShare, ScreenShareOff } from 'lucide-react';
import { useScreenShareStore } from '../store/useScreenShareStore';
import { useStore } from '../store/useStore';
import { useVideoFps, useVideoStream } from '../screenshare/useVideoFps';
import { ScreenShareSettings } from './ScreenShareSettings';
import type { ShareMode } from '../screenshare/params';

interface Props {
  onStart: () => void | Promise<void>;
  onStop: () => void;
  onUpdateParams: () => void | Promise<void>;
  onShareModeChange: (mode: ShareMode) => void | Promise<void>;
}

// Android Chrome and a few mobile browsers ship MediaDevices without
// getDisplayMedia. Calling it throws "not a function"; hide the entry point
// entirely so the user doesn't see a control that can't deliver.
const screenCaptureSupported =
  typeof navigator !== 'undefined' && typeof navigator.mediaDevices?.getDisplayMedia === 'function';

/**
 * Single in-bar button that toggles screen-share publishing. Greyed out
 * outside the joined state. Label flips on `myStatus`; the transient
 * "starting" / "stopping" states disable the button to prevent double-fire
 * during getDisplayMedia / WS round-trip.
 */
export function ScreenShareButton({ onStart, onStop, onUpdateParams, onShareModeChange }: Props) {
  const joinState = useStore((s) => s.joinState);
  const myStatus = useScreenShareStore((s) => s.myStatus);
  const myStream = useScreenShareStore((s) => s.myStream);
  const myVideoCodec = useScreenShareStore((s) => s.myVideoCodec);
  const [menuOpen, setMenuOpen] = useState(false);

  if (!screenCaptureSupported) return null;

  const joined = joinState === 'joined';
  const busy = myStatus === 'starting' || myStatus === 'stopping';
  const publishing = myStatus === 'publishing';

  function handleClick() {
    if (!joined || busy) return;
    if (publishing) onStop();
    else void onStart();
  }

  let label: string;
  if (myStatus === 'starting') label = 'Запуск…';
  else if (myStatus === 'stopping') label = 'Останавливаю…';
  else if (publishing) label = 'Остановить демонстрацию';
  else label = 'Поделиться экраном';

  const Icon = publishing ? ScreenShareOff : ScreenShare;
  const mainBtnClass = publishing ? 'btn-danger' : 'btn-secondary';

  return (
    <>
      <div className="grid gap-2 shrink-0">
        <div className="flex gap-1">
          <button
            type="button"
            onClick={handleClick}
            disabled={!joined || busy}
            className={`btn flex-1 ${mainBtnClass}`}
          >
            <Icon size={16} strokeWidth={2.25} />
            <span>{label}</span>
          </button>
          <button
            type="button"
            onClick={() => setMenuOpen((v) => !v)}
            aria-label="Настройки демонстрации"
            aria-expanded={menuOpen}
            className="btn btn-secondary px-2!"
          >
            <ChevronDown
              size={16}
              strokeWidth={2.25}
              className={`transition-transform ${menuOpen ? 'rotate-180' : ''}`}
            />
          </button>
        </div>
        {menuOpen && (
          <div className="border border-line/60 bg-bg-input/30 p-2">
            <ScreenShareSettings
              onLiveUpdate={onUpdateParams}
              onShareModeChange={onShareModeChange}
            />
          </div>
        )}
      </div>
      {myStream && <SelfPreview stream={myStream} videoCodec={myVideoCodec} />}
    </>
  );
}

function SelfPreview({
  stream,
  videoCodec,
}: {
  stream: MediaStream;
  videoCodec: 'av1' | 'vp9' | null;
}) {
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const videoSize = useVideoStream(videoRef, stream);
  const fps = useVideoFps(videoRef, stream);

  const qualityLabel = videoSize ? `${videoSize.w}×${videoSize.h}` : null;
  const fpsLabel = fps !== null ? `${Math.round(fps)}fps` : null;

  return (
    <div className="relative">
      <video
        ref={videoRef}
        autoPlay
        playsInline
        muted
        className="w-full aspect-video rounded bg-black object-contain"
      />
      {(videoCodec || qualityLabel || fpsLabel) && (
        <div className="absolute left-2 top-2 flex items-center gap-1">
          {videoCodec && (
            <span className="text-xs text-zinc-300 px-1.5 py-0.5 rounded bg-zinc-900/80">
              {videoCodec.toUpperCase()}
            </span>
          )}
          {qualityLabel && (
            <span className="text-xs text-zinc-300 px-1.5 py-0.5 rounded bg-zinc-900/80">
              {qualityLabel}
            </span>
          )}
          {fpsLabel && (
            <span className="text-xs text-zinc-300 px-1.5 py-0.5 rounded bg-zinc-900/80">
              {fpsLabel}
            </span>
          )}
        </div>
      )}
    </div>
  );
}
