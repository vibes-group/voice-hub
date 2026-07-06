import { useStore } from '../store/useStore';
import { isTauri } from '../utils/tauri';
import { ToggleRow } from './Toggle';

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
        <ToggleRow
          label="Показывать пинги"
          checked={pingsVisible}
          onChange={() => setMuteIncomingPings(!muteIncomingPings)}
          ariaLabel="Показывать пинги"
        />

        {pingsVisible && (
          <div className="grid gap-3 border-l border-line pl-4 ml-1">
            <ToggleRow
              label="Звук"
              checked={pingSoundEnabled}
              onChange={() => setPingSoundEnabled(!pingSoundEnabled)}
              ariaLabel="Звук пинга"
            />

            {isTauri() && (
              <ToggleRow
                label="Мигание окна"
                checked={pingWindowFlashEnabled}
                onChange={() => setPingWindowFlashEnabled(!pingWindowFlashEnabled)}
                ariaLabel="Мигание окна"
              />
            )}
          </div>
        )}
      </div>
    </section>
  );
}
