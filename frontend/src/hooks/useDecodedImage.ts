import { useEffect, useState } from 'react';

type DecodedImage = { id: string; url: string; width: number; height: number };

// The image to put on the stage, with its natural size: the last one that
// finished decoding, so paging keeps the previous image up instead of blanking.
// Its object URL is revoked once its attachment is left, so it may only linger
// on an <img> that is already mounted: `show` = false drops it. `onError` gets
// the id of an image that can't be decoded.
export function useDecodedImage(
  id: string,
  url: string | null,
  show: boolean,
  onError: (id: string) => void,
): DecodedImage | null {
  const [decoded, setDecoded] = useState<DecodedImage | null>(null);
  if (!show && decoded) setDecoded(null);
  const target = show ? url : null;

  useEffect(() => {
    if (!target) return;
    let cancelled = false;
    const img = new Image();
    img.src = target;
    const settle = () => {
      if (cancelled) return;
      // decode() rejects spuriously in some engines; a sized image is usable.
      if (img.naturalWidth && img.naturalHeight)
        setDecoded({ id, url: target, width: img.naturalWidth, height: img.naturalHeight });
      else onError(id);
    };
    img.decode().then(settle, () => {
      if (img.complete) settle();
      else {
        img.addEventListener('load', settle);
        img.addEventListener('error', settle);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [id, target, onError]);

  return show ? decoded : null;
}
