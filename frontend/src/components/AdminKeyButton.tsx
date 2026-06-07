import { useEffect, useState, useCallback } from 'react';
import { Key, Plus } from 'lucide-react';
import { useStore } from '../store/useStore';
import { useConnPassApi } from '../hooks/useConnPassApi';
import type { PlaintextResponse } from '../hooks/useConnPassApi';
import { AdminConnPassEntryRow, TTL_PRESETS } from './AdminConnPassEntryRow';

type Mode = 'list' | 'plaintext';

const MAX_ENTRIES = 16;

export function AdminKeyButton() {
  const isAdmin = useStore((s) => s.role === 'admin');
  const { entries, refresh, create, rotate, rename, revoke, setTTL, disconnectUsers } = useConnPassApi();
  const [open, setOpen] = useState(false);
  const [mode, setMode] = useState<Mode>('list');
  const [plaintext, setPlaintext] = useState<PlaintextResponse | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const [newLabel, setNewLabel] = useState('');
  const [newTTL, setNewTTL] = useState<number>(0);

  // Seed the entry list once isAdmin flips true so the badge in the trigger
  // can later show counts without opening the modal.
  useEffect(() => {
    if (!isAdmin) return;
    void refresh();
  }, [isAdmin, refresh]);

  const handleOpen = useCallback(() => {
    setMode('list');
    setPlaintext(null);
    setError(null);
    setNewLabel('');
    setOpen(true);
    void refresh();
  }, [refresh]);

  const handleClose = useCallback(() => {
    setOpen(false);
    setPlaintext(null);
    setError(null);
  }, []);

  const withBusy = useCallback(async (fn: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await fn();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }, []);

  const handleCreate = useCallback(
    () =>
      withBusy(async () => {
        const data = await create(newLabel, newTTL);
        if (data) {
          setPlaintext(data);
          setMode('plaintext');
          setNewLabel('');
          setNewTTL(0);
          await refresh();
        }
      }),
    [withBusy, create, newLabel, newTTL, refresh],
  );

  const handleSetTTL = useCallback(
    (id: string, ttlSeconds: number) =>
      withBusy(async () => {
        await setTTL(id, ttlSeconds);
        await refresh();
      }),
    [withBusy, setTTL, refresh],
  );

  const handleRotate = useCallback(
    async (id: string, label: string) => {
      const what = label ? `«${label}»` : 'этот пароль';
      if (!window.confirm(`Перегенерировать ${what}? Старый пароль перестанет работать.`)) return;
      await withBusy(async () => {
        const data = await rotate(id);
        setPlaintext(data);
        setMode('plaintext');
        await refresh();
      });
    },
    [withBusy, rotate, refresh],
  );

  const handleRevoke = useCallback(
    async (id: string, label: string) => {
      const what = label ? `«${label}»` : 'этот пароль';
      if (!window.confirm(`Удалить ${what}?`)) return;
      await withBusy(async () => {
        await revoke(id);
        await refresh();
      });
    },
    [withBusy, revoke, refresh],
  );

  const handleRename = useCallback(
    (id: string, label: string) =>
      withBusy(async () => {
        await rename(id, label);
        await refresh();
      }),
    [withBusy, rename, refresh],
  );

  const handleDisconnectUsers = useCallback(async () => {
    if (!window.confirm('Отключить пользователей?')) return;
    await withBusy(async () => {
      await disconnectUsers();
    });
  }, [withBusy, disconnectUsers]);

  const copy = useCallback(async (label: string, text: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(label);
      window.setTimeout(() => setCopied(null), 1500);
    } catch {
      // Clipboard API can fail in non-secure contexts; user can select+copy manually.
    }
  }, []);

  if (!isAdmin) return null;

  const shareBlock = plaintext ? `Сервер: ${plaintext.host}\nПароль: ${plaintext.password}` : '';
  const atLimit = entries.length >= MAX_ENTRIES;

  return (
    <>
      <button
        type="button"
        onClick={handleOpen}
        title="Управление паролями подключения"
        aria-label="Управление паролями подключения"
        className="inline-flex items-center justify-center w-9 h-9 bg-bg-0 border border-line text-muted-2 hover:text-accent hover:border-accent transition-colors"
      >
        <Key size={18} />
      </button>

      {open && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-[rgba(0,0,0,0.6)] p-4"
          onClick={handleClose}
        >
          <div
            className="card card-lg w-[min(520px,100%)] p-6"
            onClick={(e) => e.stopPropagation()}
          >
            {mode === 'list' && (
              <>
                <h2 className="text-[14px] font-extrabold uppercase tracking-[0.18em] text-text m-0 mb-2">
                  Пароли подключения
                </h2>
                <div className="text-muted text-[12px] mb-4">
                  {entries.length === 0
                    ? 'Нет активных паролей — пользователи не могут войти.'
                    : `Активных: ${entries.length} из ${MAX_ENTRIES}`}
                </div>

                {entries.length > 0 && (
                  <div className="flex flex-col gap-2 mb-4">
                    {entries.map((entry) => (
                      <AdminConnPassEntryRow
                        key={entry.id}
                        entry={entry}
                        busy={busy}
                        onRotate={(id, label) => void handleRotate(id, label)}
                        onRename={(id, label) => void handleRename(id, label)}
                        onRevoke={(id, label) => void handleRevoke(id, label)}
                        onSetTTL={(id, ttlSeconds) => void handleSetTTL(id, ttlSeconds)}
                      />
                    ))}
                  </div>
                )}

                <div className="flex items-center gap-2 mb-4">
                  <input
                    type="text"
                    value={newLabel}
                    onChange={(e) => setNewLabel(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter' && !atLimit) void handleCreate();
                    }}
                    placeholder="Название (необязательно)"
                    maxLength={64}
                    disabled={busy || atLimit}
                    className="flex-1 bg-bg-0 border border-line px-3 py-2 text-[13px] text-text placeholder:text-muted-2 focus:border-accent outline-none disabled:opacity-50"
                  />
                  <select
                    value={newTTL}
                    onChange={(e) => setNewTTL(Number(e.target.value))}
                    disabled={busy || atLimit}
                    className="bg-bg-0 border border-line px-2 py-2 text-[13px] text-text focus:border-accent outline-none disabled:opacity-50"
                    title="Срок действия"
                  >
                    {TTL_PRESETS.map((p) => (
                      <option key={p.value} value={p.value}>
                        {p.label}
                      </option>
                    ))}
                  </select>
                  <button
                    type="button"
                    onClick={handleCreate}
                    disabled={busy || atLimit}
                    className="btn btn-primary inline-flex items-center gap-1.5"
                    title={atLimit ? 'Достигнут лимит паролей' : 'Создать пароль'}
                  >
                    <Plus size={14} />
                    Создать
                  </button>
                </div>

                <div className="flex flex-col gap-2">
                  <button
                    type="button"
                    onClick={handleDisconnectUsers}
                    disabled={busy}
                    className="btn w-full justify-center text-danger border-[rgba(248,113,113,0.3)] hover:bg-[rgba(248,113,113,0.08)]"
                  >
                    Отключить пользователей
                  </button>
                  <button type="button" onClick={handleClose} className="btn w-full justify-center">
                    Закрыть
                  </button>
                </div>
                {error && (
                  <div className="mt-3 px-3 py-2 text-[12px] text-danger bg-[rgba(248,113,113,0.08)] border border-[rgba(248,113,113,0.3)]">
                    {error}
                  </div>
                )}
              </>
            )}

            {mode === 'plaintext' && plaintext && (
              <>
                <h2 className="text-[14px] font-extrabold uppercase tracking-[0.18em] text-accent m-0 mb-2">
                  Новый пароль{plaintext.label ? ` · ${plaintext.label}` : ''}
                </h2>
                <div className="text-muted text-[12px] mb-4">
                  Скопируйте сейчас — больше не будет показан.
                </div>
                <pre className="px-3 py-2.5 mb-3 bg-bg-0 border border-line text-[12px] font-mono whitespace-pre-wrap break-all text-accent">
                  {shareBlock}
                </pre>
                <div className="flex gap-2 mb-2">
                  <button
                    type="button"
                    className="btn flex-1 justify-center"
                    onClick={() => copy('share', shareBlock)}
                  >
                    {copied === 'share' ? '✓ Скопировано' : 'Копировать всё'}
                  </button>
                  <button
                    type="button"
                    className="btn"
                    onClick={() => copy('pw', plaintext.password)}
                  >
                    {copied === 'pw' ? '✓' : 'Только пароль'}
                  </button>
                </div>
                <button
                  type="button"
                  onClick={() => setMode('list')}
                  className="btn btn-primary w-full justify-center mt-3"
                >
                  Готово
                </button>
              </>
            )}
          </div>
        </div>
      )}
    </>
  );
}
