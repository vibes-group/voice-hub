import { useStore } from '../store/useStore';
import type { ChatSendPayload } from '../sfu/protocol';
import { TEMP_UPLOAD_PREFIX } from './uploadFile';

const RETRY_WINDOW_MS = 5 * 60 * 1000;

// Re-send our own pending chat messages whose echo we never received (lost
// during a prior WS rotation, e.g. lurker → voice transition). Capped at the
// retry window so ancient failures don't keep getting retried.
//
// Server doesn't dedup by clientMsgId, so a duplicate retry could land twice
// in the worst case. Tight window keeps that rare. The receive path uses
// clientMsgId to reconcile the optimistic entry, so the first echo back wins
// (subsequent echo-of-duplicate appends as a separate message — acceptable
// trade-off vs. losing the message entirely).
export function retryPendingChats(
  send: ((payload: ChatSendPayload) => void) | undefined,
  ourClientId: string,
): void {
  if (!send) return;
  const roomId = useStore.getState().roomSlug ?? '';
  const msgs = useStore.getState().chatByRoom[roomId] ?? [];
  const cutoff = Date.now() - RETRY_WINDOW_MS;
  for (const m of msgs) {
    if (!m.pending || m.senderClientId !== ourClientId || m.ts < cutoff || !m.clientMsgId) continue;
    // A failed upload is not retried — re-sending would reference bytes the
    // server never received.
    if (m.uploadFailed) continue;
    // Skip while attachments are still uploading (placeholder ids the server
    // would reject); the normal send path fires once uploads finish.
    if (m.attachments?.some((a) => a.uploadId.startsWith(TEMP_UPLOAD_PREFIX))) continue;
    send({ text: m.text, clientMsgId: m.clientMsgId, attachments: m.attachments });
  }
}
