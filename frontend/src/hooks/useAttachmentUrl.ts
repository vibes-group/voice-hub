import { useEffect, useRef, useState, useCallback } from 'react';
import type { Attachment } from '../sfu/protocol';
import { getBlob, putBlob } from '../utils/blobCache';

// Resolves an attachment's bytes: local IndexedDB cache first, then the
// transient download endpoint. A 404 means the server already evicted the
// upload (returns null); any other non-OK status throws so the caller can
// retry. Successful downloads are written back to the cache so the next render
// — and a reload — hit locally.
async function resolveAttachmentBlob(
  uploadId: string,
  roomId: string,
  signal?: AbortSignal,
): Promise<Blob | null> {
  const cached = await getBlob(uploadId);
  if (cached) return cached;
  const res = await fetch(
    `/api/file/${encodeURIComponent(uploadId)}?room=${encodeURIComponent(roomId)}`,
    {
      credentials: 'include',
      signal,
    },
  );
  if (res.status === 404) return null;
  if (!res.ok) throw new Error(`download failed: ${res.status}`);
  const blob = await res.blob();
  await putBlob(uploadId, blob);
  return blob;
}

// Triggers a browser/WebView download of the attachment via a synthetic anchor.
// In Tauri's WebView2 this opens the OS save dialog; no plugin required.
export async function downloadAttachment(att: Attachment, roomId: string): Promise<void> {
  const blob = await resolveAttachmentBlob(att.uploadId, roomId);
  if (!blob) return;
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = att.name || 'file';
  document.body.appendChild(a);
  a.click();
  a.remove();
  URL.revokeObjectURL(url);
}

export type AttachmentUrlStatus = 'loading' | 'ready' | 'unavailable' | 'error';

export type AttachmentUrlState = {
  url: string | null;
  status: AttachmentUrlStatus;
  reload: () => void;
};

// Resolves an attachment to an object URL for rendering, owning the URL's
// lifecycle (revoked on unmount or when the source changes) so callers never
// leak. 'unavailable' is the terminal 404 state; 'error' is retryable. Pass
// enabled=false to skip resolution entirely (e.g. a deleted attachment).
export function useAttachmentUrl(
  uploadId: string,
  roomId: string,
  enabled = true,
): AttachmentUrlState {
  const [status, setStatus] = useState<AttachmentUrlStatus>('loading');
  const [url, setUrl] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const urlRef = useRef<string | null>(null);

  const reload = useCallback(() => setAttempt((n) => n + 1), []);

  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    const controller = new AbortController();
    setStatus('loading');
    setUrl(null);

    resolveAttachmentBlob(uploadId, roomId, controller.signal)
      .then((blob) => {
        if (cancelled) return;
        if (!blob) {
          setStatus('unavailable');
          return;
        }
        const objectUrl = URL.createObjectURL(blob);
        urlRef.current = objectUrl;
        setUrl(objectUrl);
        setStatus('ready');
      })
      .catch((err) => {
        if (cancelled || (err as DOMException)?.name === 'AbortError') return;
        setStatus('error');
      });

    return () => {
      cancelled = true;
      controller.abort();
      if (urlRef.current) {
        URL.revokeObjectURL(urlRef.current);
        urlRef.current = null;
      }
    };
  }, [uploadId, roomId, attempt, enabled]);

  return { url, status, reload };
}
