import { Download } from 'lucide-react';
import { isTauri } from '../utils/tauri';
import { isWindowsBrowser } from '../utils/platform';

export function DesktopDownloadButton() {
  if (isTauri() || !isWindowsBrowser()) return null;

  return (
    <a
      href="/desktop/download/windows"
      title="Скачать приложение для Windows"
      aria-label="Скачать приложение для Windows"
      className="inline-flex items-center justify-center w-9 h-9 bg-bg-0 border border-line text-muted-2
        hover:text-accent hover:border-accent transition-colors"
    >
      <Download size={18} />
    </a>
  );
}
