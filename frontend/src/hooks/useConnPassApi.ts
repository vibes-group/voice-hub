import { useState, useCallback } from 'react';

export type ConnPassEntry = {
  id: string;
  label: string;
  generation: number;
  created_at: string;
  expires_at?: string;
  expired: boolean;
};

export type PlaintextResponse = {
  host: string;
  id: string;
  label: string;
  password: string;
  generation: number;
  created_at: string;
  expires_at?: string;
  expired: boolean;
};

const ENDPOINT = '/api/admin/connection-passwords';

export function useConnPassApi() {
  const [entries, setEntries] = useState<ConnPassEntry[]>([]);

  const refresh = useCallback(async () => {
    try {
      const res = await fetch(ENDPOINT, { credentials: 'same-origin' });
      if (res.ok) {
        const data = (await res.json()) as { entries: ConnPassEntry[] };
        setEntries(data.entries ?? []);
      }
    } catch {
      // Swallowed: status stays stale, badge shows last known count.
    }
  }, []);

  const create = useCallback(
    async (label: string, ttlSeconds: number): Promise<PlaintextResponse> => {
      const res = await fetch(ENDPOINT, {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ label: label.trim(), ttl_seconds: ttlSeconds }),
      });
      if (!res.ok) {
        throw res.status === 409
          ? new Error('Достигнут лимит паролей')
          : new Error('Не удалось создать пароль');
      }
      return (await res.json()) as PlaintextResponse;
    },
    [],
  );

  const rotate = useCallback(async (id: string): Promise<PlaintextResponse> => {
    const res = await fetch(`${ENDPOINT}/${id}/rotate`, {
      method: 'POST',
      credentials: 'same-origin',
    });
    if (!res.ok) throw new Error('Не удалось перегенерировать');
    return (await res.json()) as PlaintextResponse;
  }, []);

  const rename = useCallback(async (id: string, label: string): Promise<void> => {
    const res = await fetch(`${ENDPOINT}/${id}/rename`, {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ label: label.trim() }),
    });
    if (!res.ok) throw new Error('Не удалось переименовать');
  }, []);

  const revoke = useCallback(async (id: string): Promise<void> => {
    const res = await fetch(`${ENDPOINT}/${id}`, {
      method: 'DELETE',
      credentials: 'same-origin',
    });
    if (!res.ok) throw new Error('Не удалось удалить');
  }, []);

  const setTTL = useCallback(async (id: string, ttlSeconds: number): Promise<void> => {
    const res = await fetch(`${ENDPOINT}/${id}/ttl`, {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ ttl_seconds: ttlSeconds }),
    });
    if (!res.ok) throw new Error('Не удалось обновить срок');
  }, []);

  const disconnectUsers = useCallback(async (): Promise<void> => {
    const res = await fetch(`${ENDPOINT}/disconnect-users`, {
      method: 'POST',
      credentials: 'same-origin',
    });
    if (!res.ok) throw new Error('Не удалось отключить');
  }, []);

  return { entries, refresh, create, rotate, rename, revoke, setTTL, disconnectUsers };
}
