import { useStore } from '../store/useStore';
import { isTauri } from '../utils/tauri';
import { Toggle } from './Toggle';

export function PingCard() {
  const pingSoundEnabled = useStore((s) => s.pingSoundEnabled);
  const muteIncomingPings = useStore((s) => s.muteIncomingPings);
  const pingWindowFlashEnabled = useStore((s) => s.pingWindowFlashEnabled);
  const setPingSoundEnabled = useStore((s) => s.setPingSoundEnabled);
  const setMuteIncomingPings = useStore((s) => s.setMuteIncomingPings);
  const setPingWindowFlashEnabled = useStore((s) => s.setPingWindowFlashEnabled);

  const pingsVisible = !muteIncomingPings;

  const handleReset = () => {
    setMuteIncomingPings(false);
    setPingSoundEnabled(true);
    setPingWindowFlashEnabled(false);
  };

  return (
    <section className="card grid gap-5 p-6">
      <div className="flex items-center justify-between gap-3">
        <h2 className="card-title">Пинг</h2>
        <button
          type="button"
          onClick={handleReset}
          className="btn btn-secondary btn-mini text-muted hover:border-danger! hover:text-danger! active:translate-y-0! active:bg-danger! active:border-danger! active:text-accent-ink!"
        >
          Сбросить
        </button>
      </div>

      <div className="grid gap-3">
        <div className="flex items-center justify-between gap-3">
          <span className="section-label">Показывать пинги</span>
          <Toggle
            checked={pingsVisible}
            onChange={() => setMuteIncomingPings(!muteIncomingPings)}
            ariaLabel="Показывать пинги"
          />
        </div>

        {pingsVisible && (
          <div className="grid gap-3 border-l border-line pl-4 ml-1">
            <div className="flex items-center justify-between gap-3">
              <span className="section-label">Звук</span>
              <Toggle
                checked={pingSoundEnabled}
                onChange={() => setPingSoundEnabled(!pingSoundEnabled)}
                ariaLabel="Звук пинга"
              />
            </div>

            {isTauri() && (
              <div className="flex items-center justify-between gap-3">
                <span className="section-label">Мигание окна</span>
                <Toggle
                  checked={pingWindowFlashEnabled}
                  onChange={() => setPingWindowFlashEnabled(!pingWindowFlashEnabled)}
                  ariaLabel="Мигание окна"
                />
              </div>
            )}
          </div>
        )}
      </div>
    </section>
  );
}
