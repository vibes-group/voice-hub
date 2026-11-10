import type { CSSProperties } from 'react';

// Bounding box of one inline photo/video in the message list. The media is
// scaled down to fit both limits (never up past its natural width), so a tall
// image can't outgrow the viewport; aspect ratios beyond the clamp are
// center-cropped and shown whole only in the lightbox / player.
const MAX_WIDTH = 320;
const MAX_HEIGHT = 360;
// Keeps tiny images a usable click target.
const MIN_WIDTH = 64;
// 1:2 fits a phone screenshot; 3:1 a wide banner.
const MIN_RATIO = 1 / 2;
const MAX_RATIO = 3;
// Until the real dimensions are known.
const FALLBACK_RATIO = 4 / 3;

export type MediaSize = { width: number; height: number };

// `declared` comes with the attachment; `measured` is what the loaded media
// reports, for attachments sent without usable dimensions.
export function mediaBoxStyle(
  declared: Partial<MediaSize>,
  measured?: MediaSize | null,
): CSSProperties {
  const { width, height } = declared.width && declared.height ? declared : (measured ?? {});
  const known = width !== undefined && height !== undefined && width > 0 && height > 0;
  const ratio = +(
    known ? Math.min(MAX_RATIO, Math.max(MIN_RATIO, width / height)) : FALLBACK_RATIO
  ).toFixed(4);
  const natural = known ? `${Math.max(MIN_WIDTH, Math.round(width))}px, ` : '';
  return {
    // Height follows from the width, so capping it means capping the width.
    width: `min(${MAX_WIDTH}px, ${natural}calc(min(${MAX_HEIGHT}px, 50vh) * ${ratio}))`,
    maxWidth: '100%',
    aspectRatio: `${ratio}`,
  };
}

let probe: HTMLVideoElement | undefined;

// Videos travel as kind 'file' (older clients keep rendering a download card);
// ones this browser can decode get an inline player.
export function isInlineVideo(mime: string): boolean {
  if (!mime.startsWith('video/')) return false;
  probe ??= document.createElement('video');
  return probe.canPlayType(mime) !== '';
}
