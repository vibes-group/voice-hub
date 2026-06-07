import { X } from 'lucide-react';
import { formatBinding } from '../utils/binding';
import { type HotkeyApi } from '../hooks/useKeyboardCapture';
import { useTauriHotkey } from '../hooks/useTauriHotkey';
import { useWebHotkey } from '../hooks/useWebHotkey';
import { isTauri } from '../utils/tauri';

type Props = {
  onStatusMessage: (msg: string) => void;
};

// Web and Tauri use different capture hooks; hooks can't be called
// conditionally, so each platform gets its own wrapper around the shared view.
export function HotkeyCard({ onStatusMessage }: Props) {
  return isTauri() ? (
    <TauriCard onStatusMessage={onStatusMessage} />
  ) : (
    <WebCard onStatusMessage={onStatusMessage} />
  );
}

function WebCard({ onStatusMessage }: Props) {
  return <HotkeyCardView api={useWebHotkey(onStatusMessage)} />;
}

function TauriCard({ onStatusMessage }: Props) {
  return <HotkeyCardView api={useTauriHotkey(onStatusMessage)} />;
}

function HotkeyCardView({ api }: { api: HotkeyApi }) {
  let display: string;
  if (api.capturing) {
    display = api.liveKeys.length > 0 ? api.liveKeys.join(' + ') : 'Жду нажатия…';
  } else {
    display = formatBinding(api.binding) || 'Не задано';
  }

  return (
    <section className="card grid gap-4 p-6">
      <div className="flex items-start justify-between gap-3">
        <div className="grid gap-1 min-w-0">
          <h2 className="card-title">Горячая клавиша</h2>
          <span className="text-[11px] font-bold text-muted-2 uppercase tracking-[0.14em]">
            Микрофон вкл/выкл
          </span>
        </div>
        <button
          id="shortcut-reset"
          type="button"
          onClick={api.reset}
          disabled={api.capturing}
          className="btn btn-secondary btn-mini shrink-0 text-muted hover:border-danger! hover:text-danger! active:translate-y-0! active:bg-danger! active:border-danger! active:text-accent-ink!"
        >
          Сбросить
        </button>
      </div>
      <div className="grid gap-2">
        <span className="section-label">Привязка</span>
        <div className="relative">
          <input
            id="shortcut-input"
            type="text"
            readOnly
            value={display}
            onClick={api.start}
            onBlur={api.cancel}
            className="input-field cursor-pointer uppercase tracking-[0.1em] pr-10 mt-0! text-muted!"
          />
          {(api.capturing || api.binding) && (
            <button
              id="shortcut-clear"
              type="button"
              aria-label={api.capturing ? 'Отмена' : 'Удалить привязку'}
              title={api.capturing ? 'Отмена' : 'Удалить'}
              onMouseDown={(e) => e.preventDefault()}
              onClick={api.capturing ? api.cancel : api.clear}
              className={`absolute right-2 top-1/2 -translate-y-1/2 grid place-items-center w-7 h-7 transition-colors hover:bg-danger/15 ${
                api.capturing ? 'text-danger' : 'text-muted-2 hover:text-danger'
              }`}
            >
              <X size={20} />
            </button>
          )}
        </div>
      </div>
    </section>
  );
}
