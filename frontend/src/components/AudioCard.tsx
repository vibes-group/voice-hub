import { useEffect, useState } from 'react';
import { ChevronDown } from 'lucide-react';
import { useStore } from '../store/useStore';
import type { EngineKind } from '../types';
import { ENGINE_OPTIONS, type ActiveEngineKind } from '../audio/engine';
import { ToggleRow } from './Toggle';

interface Props {
  onEngineSelect: (engine: EngineKind) => void;
  onMicDeviceSelect: (deviceId: string | null) => void;
  onSendVolumeChange: (v: number) => void;
  onOutputVolumeChange: (v: number) => void;
  onReset: () => void;
}

function SliderHead({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex justify-between items-center text-[13px] font-bold uppercase tracking-[0.18em]">
      <span className="text-muted">{label}</span>
      <span className="text-accent tabular-nums">{value}</span>
    </div>
  );
}

export function AudioCard({
  onEngineSelect,
  onMicDeviceSelect,
  onSendVolumeChange,
  onOutputVolumeChange,
  onReset,
}: Props) {
  const engine = useStore((s) => s.engine);
  const sendVolume = useStore((s) => s.sendVolume);
  const outputVolume = useStore((s) => s.outputVolume);
  const micDeviceId = useStore((s) => s.micDeviceId);
  const [micDevices, setMicDevices] = useState<MediaDeviceInfo[]>([]);
  // Remembers the last non-off engine so toggling the switch back on restores
  // the chosen variant rather than resetting to the default.
  const [lastVariant, setLastVariant] = useState<ActiveEngineKind>(
    engine === 'off' ? 'rnnoise' : engine,
  );
  useEffect(() => {
    if (engine !== 'off') setLastVariant(engine);
  }, [engine]);

  useEffect(() => {
    const md = navigator.mediaDevices;
    if (!md?.enumerateDevices) return;
    let cancelled = false;
    const refresh = async () => {
      try {
        const all = await md.enumerateDevices();
        if (cancelled) return;
        setMicDevices(
          all.filter(
            (d) =>
              d.kind === 'audioinput' &&
              d.deviceId &&
              d.deviceId !== 'default' &&
              d.deviceId !== 'communications',
          ),
        );
      } catch {
        // enumerateDevices can throw in restricted contexts; leave list empty.
      }
    };
    // Wait for mic permission before probing: without it labels are anonymous
    // and Chromium logs a Permissions-Policy violation for the camera kind it
    // also enumerates internally.
    let permStatus: PermissionStatus | null = null;
    const onPermChange = () => {
      if (permStatus?.state === 'granted') void refresh();
    };
    const start = async () => {
      try {
        permStatus = await navigator.permissions.query({ name: 'microphone' as PermissionName });
        if (cancelled) return;
        if (permStatus.state === 'granted') void refresh();
        permStatus.addEventListener('change', onPermChange);
      } catch {
        // Permissions API not supported — fall back to immediate enumerate.
        void refresh();
      }
    };
    void start();
    md.addEventListener?.('devicechange', refresh);
    return () => {
      cancelled = true;
      md.removeEventListener?.('devicechange', refresh);
      permStatus?.removeEventListener('change', onPermChange);
    };
  }, []);

  // Drop the selected device if it disappeared (unplugged, permission revoked).
  useEffect(() => {
    if (!micDeviceId || micDevices.length === 0) return;
    const stillPresent = micDevices.some((d) => d.deviceId === micDeviceId);
    if (!stillPresent) onMicDeviceSelect(null);
  }, [micDeviceId, micDevices, onMicDeviceSelect]);

  const showMicPicker = micDevices.length > 1;

  return (
    <section className="card grid gap-5 p-6">
      <div className="flex items-center justify-between gap-3">
        <h2 className="card-title">Звук</h2>
        <button
          id="audio-reset"
          type="button"
          onClick={onReset}
          className="btn btn-secondary btn-mini text-muted hover:border-danger! hover:text-danger! active:translate-y-0! active:bg-danger! active:border-danger! active:text-accent-ink!"
        >
          Сбросить
        </button>
      </div>

      <div className="grid gap-2">
        <SliderHead label="Громкость микрофона" value={`${sendVolume}%`} />
        <input
          id="send-volume"
          type="range"
          min="0"
          max="300"
          step="5"
          value={sendVolume}
          onChange={(e) => onSendVolumeChange(Number(e.target.value))}
          className="vh-range"
          style={{ '--fill-pct': `${sendVolume / 3}%` } as React.CSSProperties}
        />
      </div>

      <div className="grid gap-2">
        <SliderHead label="Общая громкость" value={`${outputVolume}%`} />
        <input
          id="output-volume"
          type="range"
          min="0"
          max="300"
          step="5"
          value={outputVolume}
          onChange={(e) => onOutputVolumeChange(Number(e.target.value))}
          className="vh-range"
          style={{ '--fill-pct': `${outputVolume / 3}%` } as React.CSSProperties}
        />
      </div>

      {showMicPicker && (
        <div className="grid gap-2">
          <label htmlFor="mic-device" className="section-label">
            Микрофон
          </label>
          <div className="relative">
            <select
              id="mic-device"
              value={micDeviceId ?? ''}
              onChange={(e) => onMicDeviceSelect(e.target.value || null)}
              className="appearance-none w-full pl-3 pr-9 py-2.5 text-[13px] uppercase tracking-[0.1em]
                bg-bg-input border border-line text-muted cursor-pointer
                hover:border-muted-2 focus:outline-none focus:border-accent transition-colors"
            >
              <option value="">Системный по умолчанию</option>
              {micDevices.map((d, i) => (
                <option key={d.deviceId} value={d.deviceId}>
                  {d.label || `Микрофон ${i + 1}`}
                </option>
              ))}
            </select>
            <ChevronDown
              size={16}
              className="absolute right-2 top-1/2 -translate-y-1/2 text-muted-2 pointer-events-none"
            />
          </div>
        </div>
      )}

      <div className="grid gap-3">
        <ToggleRow
          label="Шумоподавление"
          checked={engine !== 'off'}
          onChange={() => onEngineSelect(engine === 'off' ? lastVariant : 'off')}
          ariaLabel="Шумоподавление"
        />

        {engine !== 'off' && (
          <div className="border-l border-line pl-4 ml-1">
            <div className="relative">
              <select
                id="engine-variant"
                aria-label="Алгоритм шумоподавления"
                value={engine}
                onChange={(e) => onEngineSelect(e.target.value as ActiveEngineKind)}
                className="appearance-none w-full pl-3 pr-9 py-2.5 text-[13px] uppercase tracking-[0.1em]
                  bg-bg-input border border-line text-muted cursor-pointer
                  hover:border-muted-2 focus:outline-none focus:border-accent transition-colors"
              >
                {ENGINE_OPTIONS.map((opt) => (
                  <option key={opt.value} value={opt.value}>
                    {opt.label}
                  </option>
                ))}
              </select>
              <ChevronDown
                size={16}
                className="absolute right-2 top-1/2 -translate-y-1/2 text-muted-2 pointer-events-none"
              />
            </div>
          </div>
        )}
      </div>
    </section>
  );
}
