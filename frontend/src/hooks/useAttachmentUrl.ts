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
    { credentials: 'include', signal },
  );
  if (res.status === 404) return null;
  if (!res.ok) throw new Error(`download failed: ${res.status}`);
  const blob = await res.blob();
  await putBlob(uploadId, blob);
  return blob;
}

// Triggers a browser/WebView download of the attachment via a synthetic anchor.
// In Tauri's WebView2 this opens the OS save dialog; no plugin required.
// Throws when the bytes can't be had, including an upload the server evicted.
async function downloadAttachment(att: Attachment, roomId: string): Promise<void> {
  const blob = await resolveAttachmentBlob(att.uploadId, roomId);
  if (!blob) throw new Error('attachment unavailable');
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = att.name || 'file';
  document.body.appendChild(a);
  a.click();
  a.remove();
  URL.revokeObjectURL(url);
}

// Saves the attachments one after another; a failure doesn't stop the rest.
// Resolves to the ones that failed.
export async function downloadAttachments(
  attachments: Attachment[],
  roomId: string,
): Promise<Attachment[]> {
  const failed: Attachment[] = [];
  for (const att of attachments) {
    try {
      await downloadAttachment(att, roomId);
    } catch {
      failed.push(att);
    }
  }
  return failed;
}

type DownloadState = 'idle' | 'busy' | 'failed';

// Download button state for one or several attachments. The next start
// retries only the ones that failed.
export function useDownload(
  attachments: Attachment[],
  roomId: string,
): { state: DownloadState; start: () => void } {
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState<Attachment[]>([]);
  // Guards against a second click landing before the disabled state renders.
  const running = useRef(false);

  async function run() {
    if (running.current) return;
    running.current = true;
    setBusy(true);
    setFailed(await downloadAttachments(failed.length ? failed : attachments, roomId));
    setBusy(false);
    running.current = false;
  }

  return { state: busy ? 'busy' : failed.length ? 'failed' : 'idle', start: () => void run() };
}

type AttachmentUrlStatus = 'loading' | 'ready' | 'unavailable' | 'error';

// Resolves an attachment to an object URL for rendering, owning the URL's
// lifecycle (revoked on unmount or when the source changes) so callers never
// leak. 'unavailable' is the terminal 404 state; 'error' is retryable. Pass
// enabled=false to skip resolution entirely (e.g. a deleted attachment).
export function useAttachmentUrl(
  uploadId: string,
  roomId: string,
  enabled = true,
): { url: string | null; status: AttachmentUrlStatus; reload: () => void } {
  const [status, setStatus] = useState<AttachmentUrlStatus>('loading');
  const [url, setUrl] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  const reload = useCallback(() => setAttempt((n) => n + 1), []);

  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    let objectUrl: string | null = null;
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
        objectUrl = URL.createObjectURL(blob);
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
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [uploadId, roomId, attempt, enabled]);

  return { url, status, reload };
}
