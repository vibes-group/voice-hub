import { LogOut } from 'lucide-react';

// POST /api/logout expires the session cookie; the server has no session
// table so nothing else needs cleaning. The redirect lands on /login.html.
async function handleLogout() {
  try {
    await fetch('/api/logout', { method: 'POST', credentials: 'same-origin' });
  } catch {
    // Ignore network errors — cookie may already be invalid; fall through to redirect.
  }
  window.location.replace('/login.html');
}

export function LogoutButton() {
  return (
    <button
      type="button"
      onClick={handleLogout}
      title="Выйти"
      aria-label="Выйти"
      className="inline-flex items-center justify-center w-9 h-9 bg-bg-0 border border-line text-muted-2
        hover:text-danger hover:border-danger transition-colors"
    >
      <LogOut size={18} />
    </button>
  );
}
