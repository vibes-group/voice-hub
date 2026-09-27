import { useSyncExternalStore } from 'react';

// Touch-first phones only: narrow desktop windows and tablets keep the desktop
// layout.
const MOBILE_QUERY =
  '(pointer: coarse) and (max-width: 767px), (pointer: coarse) and (max-height: 500px)';

const mql = typeof window !== 'undefined' ? window.matchMedia?.(MOBILE_QUERY) : undefined;

// The root class drives the `mobile:` CSS variant, so CSS and JS share one query.
function syncRootClass() {
  document.documentElement.classList.toggle('mobile', mql?.matches ?? false);
}
if (mql) {
  syncRootClass();
  mql.addEventListener('change', syncRootClass);
}

function subscribe(onChange: () => void) {
  mql?.addEventListener('change', onChange);
  return () => mql?.removeEventListener('change', onChange);
}

export function useIsMobile(): boolean {
  return useSyncExternalStore(subscribe, () => mql?.matches ?? false);
}
