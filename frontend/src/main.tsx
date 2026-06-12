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
