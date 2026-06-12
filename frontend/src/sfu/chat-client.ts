import { parseServerMessage } from './protocol';
import type {
  WelcomePayload,
  PeerInfo,
  PeerLeftPayload,
  ChatPayload,
  ChatSendPayload,
  ChatDeletedPayload,
  PingPayload,
} from './protocol';

export type ChatOnlyHandlers = {
  onWelcome: (data: WelcomePayload) => void;
  onPeerJoined: (data: PeerInfo) => void;
  onPeerLeft: (data: PeerLeftPayload) => void;
  onChat: (data: ChatPayload) => void;
  onChatDeleted: (data: ChatDeletedPayload) => void;
  onPing: (data: PingPayload) => void;
  onClose: () => void;
  onError: (err: unknown) => void;
};

export type ChatOnlyConnectOptions = {
  wsUrl: string;
  displayName: string;
  clientId: string;
};

export type ChatOnlyClient = {
  connect(opts: ChatOnlyConnectOptions): Promise<void>;
  disconnect(): void;
  sendChat(payload: ChatSendPayload): void;
  sendChatDelete(id: string): boolean;
  sendPing(targetId: string): void;
};

export function closeWebSocket(ws: WebSocket): void {
  if (ws.readyState === WebSocket.CONNECTING) {
    ws.onopen = () => {
      try {
        ws.close();
      } catch {
        /* ignore */
      }
    };
  } else {
    try {
      ws.close();
    } catch {
      /* ignore */
    }
  }
}

function noop(): void {}

export function createChatClient(handlers: Partial<ChatOnlyHandlers> = {}): ChatOnlyClient {
  const on: ChatOnlyHandlers = {
    onWelcome: handlers.onWelcome ?? noop,
    onPeerJoined: handlers.onPeerJoined ?? noop,
    onPeerLeft: handlers.onPeerLeft ?? noop,
    onChat: handlers.onChat ?? noop,
    onChatDeleted: handlers.onChatDeleted ?? noop,
    onPing: handlers.onPing ?? noop,
    onClose: handlers.onClose ?? noop,
    onError: handlers.onError ?? noop,
  };

  let ws: WebSocket | null = null;
  let stopped = false;

  // Returns whether the frame was actually written — see createSFUClient.send.
  function send(event: string, data: unknown): boolean {
    if (!ws || ws.readyState !== WebSocket.OPEN) return false;
    ws.send(JSON.stringify({ event, data }));
    return true;
  }

  function connect(opts: ChatOnlyConnectOptions): Promise<void> {
    if (ws) throw new Error('chat-client: already connected');
    stopped = false;

    return new Promise<void>((resolve, reject) => {
      let resolved = false;
      const timer = setTimeout(() => {
        if (!resolved) {
          resolved = true;
          reject(new Error('chat-client: welcome timeout'));
          disconnect();
        }
      }, 10000);

      const socket = new WebSocket(opts.wsUrl);
      ws = socket;

      socket.onopen = () => {
        socket.send(
          JSON.stringify({
            event: 'hello',
            data: { displayName: opts.displayName, clientId: opts.clientId, chatOnly: true },
          }),
        );
      };

      socket.onerror = (event) => {
        on.onError(event);
        if (!resolved) {
          resolved = true;
          clearTimeout(timer);
          reject(new Error('chat-client: websocket error'));
        }
      };

      socket.onclose = () => {
        if (!stopped) on.onClose();
      };

      socket.onmessage = (event) => {
        const msg = parseServerMessage(event.data as string);
        if (!msg) return;
        switch (msg.event) {
          case 'welcome':
            on.onWelcome(msg.data);
            break;
          case 'peer-joined':
            on.onPeerJoined(msg.data);
            break;
          case 'peer-left':
            on.onPeerLeft(msg.data);
            break;
          case 'chat':
            on.onChat(msg.data);
            break;
          case 'chat-deleted':
            on.onChatDeleted(msg.data);
            break;
          case 'ping':
            on.onPing(msg.data);
            break;
          default:
            break;
        }
        if (msg.event === 'welcome' && !resolved) {
          resolved = true;
          clearTimeout(timer);
          resolve();
        }
      };
    });
  }

  function disconnect(): void {
    stopped = true;
    if (ws) {
      closeWebSocket(ws);
      ws = null;
    }
  }

  function sendChat(payload: ChatSendPayload): void {
    send('chat-send', payload);
  }

  function sendChatDelete(id: string): boolean {
    return send('chat-delete', { id });
  }

  function sendPing(targetId: string): void {
    send('ping', { to: targetId });
  }

  return { connect, disconnect, sendChat, sendChatDelete, sendPing };
}
