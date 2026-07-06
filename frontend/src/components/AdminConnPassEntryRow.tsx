import { useState } from 'react';
import { RefreshCw, Trash2, Edit2, Check, X, Clock } from 'lucide-react';
import type { ConnPassEntry } from '../hooks/useConnPassApi';

export const TTL_PRESETS: readonly { value: number; label: string }[] = [
  { value: 0, label: 'без срока' },
  { value: 60 * 60, label: '1 час' },
  { value: 24 * 60 * 60, label: '1 день' },
  { value: 7 * 24 * 60 * 60, label: '7 дней' },
  { value: 30 * 24 * 60 * 60, label: '30 дней' },
  { value: -1, label: 'отключить сейчас' },
];

function expiryLabel(entry: ConnPassEntry): string {
  if (!entry.expires_at || entry.expires_at.startsWith('0001-')) return 'без срока';
  if (entry.expired) return 'просрочен';
  const ms = new Date(entry.expires_at).getTime() - Date.now();
  if (ms <= 0) return 'просрочен';
  const mins = Math.floor(ms / 60000);
  if (mins < 60) return `< ${mins} мин`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `< ${hours} ч`;
  const days = Math.floor(hours / 24);
  return `< ${days} дн`;
}

function relativeTime(iso: string): string {
  if (!iso || iso.startsWith('0001-')) return 'никогда';
  const t = new Date(iso).getTime();
  const diff = Date.now() - t;
  if (diff < 0) return new Date(iso).toLocaleString();
  const mins = Math.floor(diff / 60000);
  if (mins < 1) return 'только что';
  if (mins < 60) return `${mins} мин назад`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours} ч назад`;
  const days = Math.floor(hours / 24);
  return `${days} дн назад`;
}

interface Props {
  entry: ConnPassEntry;
  busy: boolean;
  onRotate: (id: string, label: string) => void;
  onRename: (id: string, label: string) => void;
  onRevoke: (id: string, label: string) => void;
  onSetTTL: (id: string, ttlSeconds: number) => void;
}

export function AdminConnPassEntryRow({ entry, busy, onRotate, onRename, onRevoke, onSetTTL }: Props) {
  const [renamingID, setRenamingID] = useState<string | null>(null);
  const [renameValue, setRenameValue] = useState('');
  const [ttlEditingID, setTtlEditingID] = useState<string | null>(null);

  const commitRename = () => {
    onRename(entry.id, renameValue);
    setRenamingID(null);
  };

  return (
    <div
      className={`flex items-center gap-2 px-3 py-2 bg-bg-0 border ${entry.expired ? 'border-[rgba(248,113,113,0.3)] opacity-60' : 'border-line'}`}
    >
      <div className="flex-1 min-w-0">
        {renamingID === entry.id ? (
          <div className="flex items-center gap-1">
            <input
              type="text"
              value={renameValue}
              onChange={(e) => setRenameValue(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') commitRename();
                if (e.key === 'Escape') setRenamingID(null);
              }}
              maxLength={64}
              className="flex-1 bg-bg-1 border border-line px-2 py-1 text-[12px] text-text focus:border-accent outline-none"
              autoFocus
            />
            <button
              type="button"
              onClick={commitRename}
              disabled={busy}
              className="p-1 text-accent hover:bg-[rgba(255,255,255,0.04)]"
              title="Сохранить"
            >
              <Check size={14} />
            </button>
            <button
              type="button"
              onClick={() => setRenamingID(null)}
              className="p-1 text-muted hover:bg-[rgba(255,255,255,0.04)]"
              title="Отмена"
            >
              <X size={14} />
            </button>
          </div>
        ) : (
          <>
            <div className="text-text text-[13px] font-medium truncate">
              {entry.label || <span className="text-muted-2 italic">без названия</span>}
            </div>
            <div className="text-muted-2 text-[11px] flex items-center gap-1.5">
              <span>{relativeTime(entry.created_at)} · #{entry.generation}</span>
              <span className={entry.expired ? 'text-danger' : ''}>· {expiryLabel(entry)}</span>
            </div>
          </>
        )}
      </div>
      {renamingID !== entry.id && ttlEditingID !== entry.id && (
        <div className="flex items-center gap-1 shrink-0">
          <button
            type="button"
            onClick={() => setTtlEditingID(entry.id)}
            disabled={busy}
            className="p-1.5 text-muted-2 hover:text-text hover:bg-[rgba(255,255,255,0.04)]"
            title="Изменить срок"
          >
            <Clock size={14} />
          </button>
          <button
            type="button"
            onClick={() => {
              setRenamingID(entry.id);
              setRenameValue(entry.label);
            }}
            disabled={busy}
            className="p-1.5 text-muted-2 hover:text-text hover:bg-[rgba(255,255,255,0.04)]"
            title="Переименовать"
          >
            <Edit2 size={14} />
          </button>
          <button
            type="button"
            onClick={() => onRotate(entry.id, entry.label)}
            disabled={busy}
            className="p-1.5 text-muted-2 hover:text-accent hover:bg-[rgba(255,255,255,0.04)]"
            title="Перегенерировать"
          >
            <RefreshCw size={14} />
          </button>
          <button
            type="button"
            onClick={() => onRevoke(entry.id, entry.label)}
            disabled={busy}
            className="p-1.5 text-muted-2 hover:text-danger hover:bg-[rgba(248,113,113,0.08)]"
            title="Удалить"
          >
            <Trash2 size={14} />
          </button>
        </div>
      )}
      {ttlEditingID === entry.id && (
        <div className="flex items-center gap-1 shrink-0">
          <select
            defaultValue=""
            disabled={busy}
            onChange={(e) => {
              const v = Number(e.target.value);
              if (!Number.isNaN(v)) {
                onSetTTL(entry.id, v);
                setTtlEditingID(null);
              }
            }}
            className="bg-bg-1 border border-line px-2 py-1 text-[12px] text-text focus:border-accent outline-none"
            autoFocus
          >
            <option value="" disabled>Срок…</option>
            {TTL_PRESETS.map((p) => (
              <option key={p.value} value={p.value}>
                {p.label}
              </option>
            ))}
          </select>
          <button
            type="button"
            onClick={() => setTtlEditingID(null)}
            className="p-1 text-muted hover:bg-[rgba(255,255,255,0.04)]"
            title="Отмена"
          >
            <X size={14} />
          </button>
        </div>
      )}
    </div>
  );
}
