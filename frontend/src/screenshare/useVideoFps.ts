import { useEffect, useState } from 'react';

/**
 * Attach the stream to the <video> (autoplay, muted — share audio is routed
 * through a sibling <audio>) and track the intrinsic video dimensions.
 */
export function useVideoStream(
  videoRef: React.RefObject<HTMLVideoElement | null>,
  stream: MediaStream | null,
) {
  const [size, setSize] = useState<{ w: number; h: number } | null>(null);

  useEffect(() => {
    const el = videoRef.current;
    if (!el) return;
    el.srcObject = stream;
    el.muted = true;
    el.play().catch(() => {});

    setSize(null);
    const update = () => {
      if (!el.videoWidth || !el.videoHeight) return;
      setSize({ w: el.videoWidth, h: el.videoHeight });
    };
    update();
    el.addEventListener('loadedmetadata', update);
    el.addEventListener('resize', update);
    return () => {
      el.removeEventListener('loadedmetadata', update);
      el.removeEventListener('resize', update);
    };
  }, [videoRef, stream]);

  return size;
}

export function useVideoFps(
  videoRef: React.RefObject<HTMLVideoElement | null>,
  stream: MediaStream | null,
) {
  const [fps, setFps] = useState<number | null>(null);

  useEffect(() => {
    const el = videoRef.current;
    if (!el || !('requestVideoFrameCallback' in el)) return;
    let stopped = false;
    let handle = 0;
    let lastTime: number | null = null;
    let frames = 0;
    const onFrame: VideoFrameRequestCallback = (_now, metadata) => {
      if (stopped) return;
      if (lastTime === null) {
        lastTime = metadata.mediaTime;
      } else {
        frames += 1;
        const elapsed = metadata.mediaTime - lastTime;
        if (elapsed >= 1) {
          setFps(frames / elapsed);
          frames = 0;
          lastTime = metadata.mediaTime;
        }
      }
      handle = el.requestVideoFrameCallback(onFrame);
    };
    handle = el.requestVideoFrameCallback(onFrame);
    return () => {
      stopped = true;
      el.cancelVideoFrameCallback(handle);
      setFps(null);
    };
  }, [videoRef, stream]);

  return fps;
}
