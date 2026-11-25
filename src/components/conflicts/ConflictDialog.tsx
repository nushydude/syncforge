import { useEffect, useId, useRef } from "react";
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
  pageError?: { message: string; stale: boolean };
  onRetryPage: () => void;
  onRefresh: () => void;
  onFocusFallback: () => void;
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
  pageError,
  onRetryPage,
  onRefresh,
  onFocusFallback,
}: ConflictDialogProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const headingRef = useRef<HTMLHeadingElement>(null);
  const titleId = useId();
  const stale = pageError?.stale === true;
  const ready = !loading && !stale && Object.keys(resolutions).length === total;

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    const previous = document.activeElement;
    if (typeof dialog.showModal === "function") dialog.showModal();
    else dialog.setAttribute("open", "");
    const firstChoice = dialog.querySelector<HTMLInputElement>("input");
    (firstChoice ?? headingRef.current)?.focus();

    return () => {
      if (typeof dialog.close === "function") dialog.close();
      else dialog.removeAttribute("open");
      if (
        previous instanceof HTMLElement &&
        previous.isConnected &&
        !previous.matches(":disabled")
      ) {
        previous.focus();
      } else {
        onFocusFallback();
      }
    };
  }, [onFocusFallback]);

  return (
    <dialog
      ref={dialogRef}
      className="conflict-dialog"
      aria-modal="true"
      aria-labelledby={titleId}
      onCancel={(event) => {
        event.preventDefault();
        onCancel();
      }}
    >
      <header className="conflict-dialog-header">
        <h3 id={titleId} ref={headingRef} tabIndex={-1}>
          Resolve conflicts
        </h3>
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
                      disabled={stale}
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

      {pageError && (
        <div className="conflict-dialog-pagination">
          <p className="form-error" role="alert">
            {pageError.message}
          </p>
          <button type="button" onClick={stale ? onRefresh : onRetryPage}>
            {stale ? "Return to preview" : "Retry page"}
          </button>
        </div>
      )}

      <nav className="conflict-dialog-pagination" aria-label="Conflict pages">
        <button
          type="button"
          disabled={!hasPreviousPage || loading || stale}
          onClick={onPreviousPage}
        >
          Previous
        </button>
        <button
          type="button"
          disabled={!hasNextPage || loading || stale}
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
    </dialog>
  );
}
