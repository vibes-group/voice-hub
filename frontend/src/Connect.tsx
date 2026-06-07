import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { ConnectionState } from './ipc';
import { AudioWaveform, Wifi } from 'lucide-react';
import { isTauri } from './utils/tauri';
import './styles/main.css';

export function Connect() {
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [host, setHost] = useState('');

  // Pre-fill the saved host so "Change server" doesn't make the user retype.
  useEffect(() => {
    if (!isTauri()) return;
    void invoke<ConnectionState>('get_state').then((state) => {
      if (state.host) setHost(state.host);
    });
  }, []);

  async function handleSubmit(e: React.FormEvent<HTMLFormElement>) {
    e.preventDefault();
    setError(null);
    setSubmitting(true);
    try {
      if (!isTauri()) throw new Error('Эта страница работает только в десктопном приложении.');
      await invoke('set_host', { host });
      // Rust navigated the webview; nothing else to do here.
    } catch (err) {
      setError(typeof err === 'string' ? err : (err as Error).message);
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <main className="card card-lg w-[min(400px,100%)] p-8 mx-auto mt-[max(18vh,24px)]">
      <div className="flex items-center gap-2.5 mb-6">
        <AudioWaveform size={22} className="text-accent" />
        <span className="font-extrabold text-[16px] uppercase tracking-[0.2em] text-accent">
          Voice&nbsp;Hub
        </span>
      </div>
      <h1 className="text-[10px] font-bold uppercase tracking-[0.2em] text-muted m-0 mb-2">
        Подключение
      </h1>
      <div className="text-muted-2 text-[12px] mb-6">
        Введите адрес сервера, который дал администратор. Пароль попросят на следующем шаге.
      </div>
      <form onSubmit={handleSubmit}>
        <div className="grid gap-2">
          <label htmlFor="host" className="section-label">
            Сервер
          </label>
          <input
            id="host"
            name="host"
            value={host}
            onChange={(e) => setHost(e.target.value)}
            placeholder="vh.example.com"
            autoComplete="off"
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
            required
            autoFocus
            className="input-field mt-0!"
          />
        </div>
        <button
          type="submit"
          disabled={submitting || !host.trim()}
          className="btn btn-primary btn-hero mt-6 disabled:cursor-progress"
        >
          Подключиться
          <Wifi size={18} />
        </button>
        {error && (
          <div className="mt-3 px-3 py-2 text-[12px] text-danger bg-[rgba(248,113,113,0.08)] border border-[rgba(248,113,113,0.3)]">
            {error}
          </div>
        )}
      </form>
    </main>
  );
}
