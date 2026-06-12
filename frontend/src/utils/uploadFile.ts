// Attachment upload (XHR, for progress events) plus the image-side helpers that
// derive dimensions and a tiny blurred placeholder before upload starts.

// Prefix for the placeholder uploadId an optimistic attachment carries until
// its real, server-assigned id is known. Recognisable so the retry path can
// avoid re-sending a chat message whose bytes haven't finished uploading.
export const TEMP_UPLOAD_PREFIX = 'temp:';

/** Client-side ceiling mirroring the server's UPLOAD_MAX_BYTES default. */
export const MAX_UPLOAD_BYTES = 100 * 1024 * 1024;

/** Carries the HTTP status so callers can distinguish 413/507 from generic failures. */
export class UploadError extends Error {
  constructor(public readonly status: number) {
    super(`upload failed with status ${status}`);
    this.name = 'UploadError';
  }
}

export type UploadOptions = {
  name: string;
  onProgress?: (fraction: number) => void;
  signal?: AbortSignal;
};

/**
 * Uploads a blob to POST /api/upload?room=… as a raw body, resolving the
 * server-assigned uploadId. Uses XHR rather than fetch so upload progress is
 * observable; the filename rides in Content-Disposition (RFC 5987).
 */
export function uploadFile(file: Blob, roomId: string, opts: UploadOptions): Promise<string> {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open('POST', `/api/upload?room=${encodeURIComponent(roomId)}`);
    xhr.withCredentials = true;
    if (file.type) xhr.setRequestHeader('Content-Type', file.type);
    xhr.setRequestHeader(
      'Content-Disposition',
      `attachment; filename*=UTF-8''${encodeURIComponent(opts.name)}`,
    );

    if (opts.onProgress) {
      xhr.upload.onprogress = (e) => {
        if (e.lengthComputable) opts.onProgress?.(e.loaded / e.total);
      };
    }
    xhr.onload = () => {
      if (xhr.status !== 201) {
        reject(new UploadError(xhr.status));
        return;
      }
      try {
        const resp = JSON.parse(xhr.responseText) as { uploadId?: unknown };
        if (typeof resp.uploadId === 'string' && resp.uploadId) {
          resolve(resp.uploadId);
        } else {
          reject(new Error('upload: response missing uploadId'));
        }
      } catch {
        reject(new Error('upload: malformed response'));
      }
    };
    xhr.onerror = () => reject(new Error('upload: network error'));
    xhr.onabort = () => reject(new DOMException('upload aborted', 'AbortError'));

    if (opts.signal) {
      if (opts.signal.aborted) {
        reject(new DOMException('upload aborted', 'AbortError'));
        return;
      }
      opts.signal.addEventListener('abort', () => xhr.abort(), { once: true });
    }
    xhr.send(file);
  });
}

export type ImageMeta = {
  width: number;
  height: number;
  blurThumb: string;
};

// 20×20 is enough detail for a blurred placeholder yet keeps the base64 JPEG
// well under 1KB.
const THUMB_SIZE = 20;

/**
 * Decodes an image and returns its natural dimensions plus a blurred-thumbnail
 * data URL. Returns null if the blob isn't a decodable image.
 */
export async function imageMeta(file: Blob): Promise<ImageMeta | null> {
  if (typeof createImageBitmap === 'undefined') return null;
  let bitmap: ImageBitmap;
  try {
    bitmap = await createImageBitmap(file);
  } catch {
    return null;
  }
  try {
    const blurThumb = await generateBlurThumb(bitmap);
    return { width: bitmap.width, height: bitmap.height, blurThumb };
  } finally {
    bitmap.close();
  }
}

/**
 * Renders source down to a THUMB_SIZE square JPEG data URL. Prefers
 * OffscreenCanvas, falling back to a detached <canvas>. Returns '' if no 2D
 * canvas is available.
 */
export async function generateBlurThumb(source: CanvasImageSource): Promise<string> {
  if (typeof OffscreenCanvas !== 'undefined') {
    const canvas = new OffscreenCanvas(THUMB_SIZE, THUMB_SIZE);
    const ctx = canvas.getContext('2d');
    if (!ctx) return '';
    ctx.drawImage(source, 0, 0, THUMB_SIZE, THUMB_SIZE);
    const blob = await canvas.convertToBlob({ type: 'image/jpeg', quality: 0.5 });
    return blobToDataURL(blob);
  }
  if (typeof document !== 'undefined') {
    const canvas = document.createElement('canvas');
    canvas.width = THUMB_SIZE;
    canvas.height = THUMB_SIZE;
    const ctx = canvas.getContext('2d');
    if (!ctx) return '';
    ctx.drawImage(source, 0, 0, THUMB_SIZE, THUMB_SIZE);
    return canvas.toDataURL('image/jpeg', 0.5);
  }
  return '';
}

// Reads a blob into a base64 data URL without FileReader, so it works in the
// node test environment as well as the browser.
async function blobToDataURL(blob: Blob): Promise<string> {
  const bytes = new Uint8Array(await blob.arrayBuffer());
  let binary = '';
  for (let i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i]);
  return `data:${blob.type || 'image/jpeg'};base64,${btoa(binary)}`;
}
