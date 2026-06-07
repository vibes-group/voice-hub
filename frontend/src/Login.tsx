import { useState } from 'react';
import { ArrowLeftRight, LogIn } from 'lucide-react';
import { isTauri } from './utils/tauri';
import './styles/main.css';

export function Login() {
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Desktop only: an unauthenticated user with a wrong/expired password can't
  // reach the in-app TopBar button, so expose a way out from the login screen.
  // Same Tauri command as the TopBar / tray entry.
  async function handleChangeServer() {
    try {
      const { invoke } = await import('@tauri-apps/api/core');
      await invoke('change_server');
    } catch (err) {
      console.error('[login] change_server failed:', err);
    }
  }

  async function handleSubmit(e: React.FormEvent<HTMLFormElement>) {
    e.preventDefault();
    setError(null);
    setSubmitting(true);

    const form = e.currentTarget;
    const passwordInput = form.elements.namedItem('password') as HTMLInputElement;
    const body = new URLSearchParams();
    body.set('password', passwordInput.value);

    try {
      const res = await fetch('/api/login', {
        method: 'POST',
        body,
        credentials: 'same-origin',
        headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
      });
      if (res.status === 204) {
        window.location.replace('/');
        return;
      }
      if (res.status === 429) {
        setError('Слишком много попыток. Подождите 15 минут.');
        return;
      }
      setError('Неверный пароль.');
    } catch (err) {
      setError('Сетевая ошибка: ' + (err instanceof Error ? err.message : String(err)));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <>
      {isTauri() && (
        <button
          type="button"
          onClick={handleChangeServer}
          title="Сменить сервер"
          aria-label="Сменить сервер"
          className="fixed top-4 right-4 z-10 inline-flex items-center justify-center w-9 h-9 bg-bg-0 border border-line text-muted-2 hover:text-accent hover:border-accent transition-colors"
        >
          <ArrowLeftRight size={18} />
        </button>
      )}
      <main className="card card-lg w-[min(400px,100%)] p-8 mx-auto mt-[max(18vh,60px)]">
        <div className="flex items-center gap-2.5 mb-6">
          <img src="/favicon.svg" alt="" width={22} height={22} className="block" />
          <span className="font-extrabold text-[16px] uppercase tracking-[0.2em] text-accent">
            Voice&nbsp;Hub
          </span>
        </div>
        <h1 className="text-[10px] font-bold uppercase tracking-[0.2em] text-muted m-0 mb-2">
          Вход
        </h1>
        <div className="text-muted-2 text-[12px] mb-6">Введите пароль для входа.</div>
        <form
          id="login-form"
          method="post"
          action="/api/login"
          onSubmit={handleSubmit}
        >
          <div className="grid gap-2">
            <label htmlFor="password" className="section-label">
              Пароль
            </label>
            <input
              id="password"
              name="password"
              type="password"
              autoComplete="current-password"
              required
              autoFocus
              className="input-field mt-0!"
            />
          </div>
          <button
            type="submit"
            id="submit"
            disabled={submitting}
            className="btn btn-primary btn-hero mt-6 disabled:cursor-progress"
          >
            Войти
            <LogIn size={18} />
          </button>
          {error && (
            <div className="mt-3 px-3 py-2 text-[12px] text-danger bg-[rgba(248,113,113,0.08)] border border-[rgba(248,113,113,0.3)]">
              {error}
            </div>
          )}
        </form>
      </main>
    </>
  );
}
