import { useEffect, useMemo, useRef, useState } from 'react';
import { Volume2, VolumeX, X } from 'lucide-react';
import { useScreenShareStore } from '../store/useScreenShareStore';
import { useStore } from '../store/useStore';
import { loadScreenAudioVolume, saveScreenAudioVolume } from '../utils/storage';
import { useVideoFps, useVideoStream } from '../screenshare/useVideoFps';

type Props = {
  onClose: () => void;
};

const ENDED_GRACE_MS = 1500;

export function ScreenShareFocused({ onClose }: Props) {
  const focusedId = useScreenShareStore((s) => s.focusedId);
  const videoStream = useScreenShareStore((s) => s.focusedStream);
  const audioStream = useScreenShareStore((s) => s.focusedAudioStream);
  const hasSystemAudio = useScreenShareStore((s) =>
    focusedId ? (s.shares.get(focusedId)?.hasSystemAudio ?? false) : false,
  );
  const videoCodec = useScreenShareStore((s) =>
    focusedId ? (s.shares.get(focusedId)?.videoCodec ?? null) : null,
  );
  const shareStillLive = useScreenShareStore((s) =>
    s.focusedId ? s.shares.has(s.focusedId) : false,
  );
  const publisher = useStore((s) => (focusedId ? s.participants[focusedId] : undefined));
  const display = publisher?.display ?? (focusedId ? `user-${focusedId}` : '');
  const publisherClientId = publisher?.clientId ?? '';

  const videoRef = useRef<HTMLVideoElement | null>(null);
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const [audioMuted, setAudioMuted] = useState(false);
  const videoSize = useVideoStream(videoRef, videoStream);
  const fps = useVideoFps(videoRef, videoStream);

  const initialVolume = useMemo(() => {
    if (!publisherClientId) return 1;
    const saved = loadScreenAudioVolume(publisherClientId);
    return saved !== null ? Math.max(0, Math.min(1, saved)) : 1;
  }, [publisherClientId]);
  const [volume, setVolume] = useState(initialVolume);

  useEffect(() => {
    setVolume(initialVolume);
  }, [initialVolume]);

  useEffect(() => {
    const el = audioRef.current;
    if (!el) return;
    el.srcObject = audioStream;
    el.volume = volume;
    el.muted = audioMuted;
    el.play().catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [audioStream]);

  useEffect(() => {
    if (audioRef.current) audioRef.current.volume = volume;
  }, [volume]);

  function toggleAudioMute() {
    const next = !audioMuted;
    setAudioMuted(next);
    if (audioRef.current) audioRef.current.muted = next;
  }

  function onVolumeInput(e: React.ChangeEvent<HTMLInputElement>) {
    const next = Number(e.target.value) / 100;
    setVolume(next);
    if (publisherClientId) saveScreenAudioVolume(publisherClientId, next);
    // Scrubbing the slider implicitly unmutes — matches Discord / desktop UX.
    if (audioMuted && next > 0) {
      setAudioMuted(false);
      if (audioRef.current) audioRef.current.muted = false;
    }
  }

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === 'Escape') onClose();
    }
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  useEffect(() => {
    if (!focusedId || shareStillLive) return;
    const t = setTimeout(onClose, ENDED_GRACE_MS);
    return () => clearTimeout(t);
  }, [focusedId, shareStillLive, onClose]);

  if (!focusedId) return null;

  const qualityLabel = videoSize ? `${videoSize.w}×${videoSize.h}` : null;
  const fpsLabel = fps !== null ? `${Math.round(fps)}fps` : null;
  const ended = !shareStillLive;

  return (
    <div className="fixed inset-0 z-50 bg-black/95 flex flex-col">
      <header className="flex items-center justify-between px-4 py-2 text-zinc-200">
        <span className="text-sm font-medium truncate flex items-center gap-2">
          Экран · {display}
          {videoCodec && (
            <span className="text-xs font-normal text-zinc-400 px-1.5 py-0.5 rounded bg-zinc-800/80">
              {videoCodec.toUpperCase()}
            </span>
          )}
          {qualityLabel && (
            <span className="text-xs font-normal text-zinc-400 px-1.5 py-0.5 rounded bg-zinc-800/80">
              {qualityLabel}
            </span>
          )}
          {fpsLabel && (
            <span className="text-xs font-normal text-zinc-400 px-1.5 py-0.5 rounded bg-zinc-800/80">
              {fpsLabel}
            </span>
          )}
        </span>
        <div className="flex items-center gap-2">
          {hasSystemAudio && !ended && (
            <>
              <button
                type="button"
                onClick={toggleAudioMute}
                aria-label={audioMuted ? 'Включить звук' : 'Выключить звук'}
                className="rounded p-1 hover:bg-white/10"
              >
                {audioMuted ? (
                  <VolumeX size={18} strokeWidth={2.25} />
                ) : (
                  <Volume2 size={18} strokeWidth={2.25} />
                )}
              </button>
              <input
                type="range"
                min={0}
                max={100}
                step={1}
                value={Math.round((audioMuted ? 0 : volume) * 100)}
                onChange={onVolumeInput}
                aria-label="Громкость звука с экрана"
                className="w-24 accent-zinc-300"
              />
            </>
          )}
          <button
            type="button"
            onClick={onClose}
            aria-label="Закрыть"
            className="rounded p-1 hover:bg-white/10"
          >
            <X size={18} strokeWidth={2.25} />
          </button>
        </div>
      </header>
      <div className="flex-1 min-h-0 grid place-items-center px-4 pb-4 relative">
        {videoStream ? (
          <video
            ref={videoRef}
            autoPlay
            playsInline
            className="w-full h-full max-w-full max-h-full object-contain bg-black"
          />
        ) : (
          <div className="text-zinc-400 text-sm">Подключаюсь к потоку…</div>
        )}
        {ended && (
          <div className="absolute inset-0 grid place-items-center bg-black/60 text-zinc-200 text-sm">
            Стрим завершён
          </div>
        )}
      </div>
      <audio ref={audioRef} />
    </div>
  );
}
