import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import { clearLegacyStorage } from './utils/storage';

const rootEl = document.getElementById('root');
if (!rootEl) throw new Error('No #root element');

clearLegacyStorage();

createRoot(rootEl).render(
  <StrictMode>
    <App />
  </StrictMode>,
);

// PWA: register the service worker in production only (dev is served by Vite
// without one, and stale SW caching would fight HMR).
if (import.meta.env.PROD && 'serviceWorker' in navigator) {
  window.addEventListener('load', () => {
    navigator.serviceWorker.register('/sw.js').catch(() => {});
  });
}
