export function Toggle({
  checked,
  onChange,
  ariaLabel,
}: {
  checked: boolean;
  onChange: () => void;
  ariaLabel: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={ariaLabel}
      onClick={onChange}
      className="vh-toggle"
      data-checked={checked}
    >
      <span className="vh-toggle-dot" />
    </button>
  );
}

// A labeled settings row: a section label on the left, a Toggle on the right.
export function ToggleRow({
  label,
  checked,
  onChange,
  ariaLabel,
}: {
  label: string;
  checked: boolean;
  onChange: () => void;
  ariaLabel: string;
}) {
  return (
    <div className="flex items-center justify-between gap-3">
      <span className="section-label">{label}</span>
      <Toggle checked={checked} onChange={onChange} ariaLabel={ariaLabel} />
    </div>
  );
}
