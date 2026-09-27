import { useStore } from '../store/useStore';

export function StatusPill() {
  const statusText = useStore((s) => s.statusText);
  const statusState = useStore((s) => s.statusState);

  const isOk = statusState === 'ok';
  const isErr = statusState === 'err';

  const wrapColor = isOk ? 'text-good' : isErr ? 'text-danger' : 'text-muted';
  const dotColor = isOk ? 'bg-good' : isErr ? 'bg-danger' : 'bg-muted-2';

  return (
    <div
      id="status"
      className={`inline-flex items-center gap-2 px-3 h-9 bg-bg-0 border border-line text-[11px] font-bold uppercase tracking-[0.14em] mobile:h-11 mobile:max-w-full ${wrapColor}`}
      data-state={statusState}
    >
      <span className={`w-1.5 h-1.5 shrink-0 ${dotColor}`} />
      <span className="mobile:truncate">{statusText}</span>
    </div>
  );
}
