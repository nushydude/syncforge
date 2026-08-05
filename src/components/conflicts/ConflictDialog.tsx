import {
  classifyPathChange,
  pathChangeKindLabel,
  type ConflictAction,
} from "../../lib/conflictPolicy";
import type { ConflictChoice, FileEntry } from "../../types";

function formatEntry(entry: FileEntry): string {
  if (entry.deleted) {
    return "deleted";
  }
  if (entry.isDir) {
    return "folder";
  }
  return `${entry.size} B · mtime ${entry.modifiedSecs}`;
}

interface ConflictDialogProps {
  conflicts: ConflictAction[];
  resolutions: Record<string, ConflictChoice>;
  onChoose: (path: string, choice: ConflictChoice) => void;
  onConfirm: () => void;
  onCancel: () => void;
  page: number;
  total: number;
  hasNextPage: boolean;
  hasPreviousPage: boolean;
  loading: boolean;
  onNextPage: () => void;
  onPreviousPage: () => void;
}

export function ConflictDialog({
  conflicts,
  resolutions,
  onChoose,
  onConfirm,
  onCancel,
  page,
  total,
  hasNextPage,
  hasPreviousPage,
  loading,
  onNextPage,
  onPreviousPage,
}: ConflictDialogProps) {
  const ready = Object.keys(resolutions).length >= total;

  return (
    <div
      className="conflict-dialog-backdrop"
      role="dialog"
      aria-modal="true"
      aria-labelledby="conflict-dialog-title"
    >
      <div className="conflict-dialog">
        <header className="conflict-dialog-header">
          <h3 id="conflict-dialog-title">Resolve conflicts</h3>
          <p>
            Choose how to handle each path. Sync will continue after you confirm
            all choices.
          </p>
          <p>
            Page {page} · {total} total conflicts
          </p>
        </header>

        <ul className="conflict-list">
          {conflicts.map((conflict) => {
            const kind = classifyPathChange(
              conflict.left,
              conflict.right,
              undefined,
            );
            const choice = resolutions[conflict.path];

            return (
              <li key={conflict.path} className="conflict-item">
                <div className="conflict-item-head">
                  <span className="conflict-path">{conflict.path}</span>
                  <span className="conflict-kind">
                    {pathChangeKindLabel(kind)}
                  </span>
                </div>
                <div className="conflict-sides">
                  <div>
                    <strong>Left</strong>
                    <span>{formatEntry(conflict.left)}</span>
                  </div>
                  <div>
                    <strong>Right</strong>
                    <span>{formatEntry(conflict.right)}</span>
                  </div>
                </div>
                <div
                  className="conflict-choices"
                  role="group"
                  aria-label={`Resolution for ${conflict.path}`}
                >
                  {(
                    [
                      ["left", "Use left"],
                      ["right", "Use right"],
                      ["keepBoth", "Keep both"],
                      ["skip", "Skip"],
                    ] as const
                  ).map(([value, label]) => (
                    <label key={value} className="conflict-choice">
                      <input
                        type="radio"
                        name={`conflict-${conflict.path}`}
                        checked={choice === value}
                        onChange={() => onChoose(conflict.path, value)}
                      />
                      {label}
                    </label>
                  ))}
                </div>
              </li>
            );
          })}
        </ul>

        <nav className="conflict-dialog-pagination" aria-label="Conflict pages">
          <button
            type="button"
            disabled={!hasPreviousPage || loading}
            onClick={onPreviousPage}
          >
            Previous
          </button>
          <button
            type="button"
            disabled={!hasNextPage || loading}
            onClick={onNextPage}
          >
            {loading ? "Loading…" : "Next"}
          </button>
        </nav>

        <footer className="conflict-dialog-actions">
          <button
            type="button"
            className="btn-primary"
            disabled={!ready}
            onClick={onConfirm}
          >
            Continue sync
          </button>
          <button type="button" onClick={onCancel}>
            Cancel
          </button>
        </footer>
      </div>
    </div>
  );
}
