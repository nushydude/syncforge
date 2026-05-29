import { CONFLICT_POLICY_OPTIONS } from '../../lib/conflictPolicy';
import type { ConflictPolicy } from '../../types';

interface ConflictPolicySelectorProps {
  value: ConflictPolicy;
  onChange: (policy: ConflictPolicy) => void;
  disabled?: boolean;
}

export function ConflictPolicySelector({
  value,
  onChange,
  disabled,
}: ConflictPolicySelectorProps) {
  return (
    <fieldset className="conflict-policy-selector" disabled={disabled}>
      <legend>Conflict policy</legend>
      <div className="conflict-policy-options">
        {CONFLICT_POLICY_OPTIONS.map((option) => (
          <label key={option.value} className="conflict-policy-option">
            <input
              type="radio"
              name="conflictPolicy"
              value={option.value}
              checked={value === option.value}
              onChange={() => onChange(option.value)}
            />
            <span className="conflict-policy-label">{option.label}</span>
            <span className="conflict-policy-hint">{option.hint}</span>
          </label>
        ))}
      </div>
    </fieldset>
  );
}
