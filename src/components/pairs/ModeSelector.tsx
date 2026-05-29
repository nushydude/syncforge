import type { SyncMode } from "../../types";

const MODES: { value: SyncMode; label: string; hint: string }[] = [
  {
    value: "synchronize",
    label: "Synchronize",
    hint: "Changes flow both ways; deletes propagate.",
  },
  {
    value: "echo",
    label: "Echo",
    hint: "Left is the source; right mirrors left.",
  },
  {
    value: "contribute",
    label: "Contribute",
    hint: "New and updated files copy left to right only.",
  },
];

interface ModeSelectorProps {
  value: SyncMode;
  onChange: (mode: SyncMode) => void;
  disabled?: boolean;
}

export function ModeSelector({ value, onChange, disabled }: ModeSelectorProps) {
  return (
    <fieldset className="mode-selector" disabled={disabled}>
      <legend>Sync mode</legend>
      <div className="mode-options">
        {MODES.map((mode) => (
          <label key={mode.value} className="mode-option">
            <input
              type="radio"
              name="syncMode"
              value={mode.value}
              checked={value === mode.value}
              onChange={() => onChange(mode.value)}
            />
            <span className="mode-label">{mode.label}</span>
            <span className="mode-hint">{mode.hint}</span>
          </label>
        ))}
      </div>
    </fieldset>
  );
}
