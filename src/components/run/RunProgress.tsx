import { memo, useCallback } from "react";
import { useRunStore } from "../../hooks/useRunStore";
import {
  cancelPairRun,
  dismissPairRun,
  getPairRun,
  queuePosition,
  type PairRunState,
  type RunStoreState,
} from "../../store/runStore";

function phaseLabel(run: PairRunState): string {
  switch (run.status) {
    case "queued":
      return "Queued";
    case "awaitingInput":
      return "Waiting for conflict choices";
    case "completed":
      return "Completed";
    case "failed":
      return "Failed";
    case "cancelled":
      return "Cancelled";
    default:
      break;
  }
  switch (run.progress?.phase) {
    case "scanning":
      return "Scanning…";
    case "running":
      return "Syncing…";
    default:
      return "Running…";
  }
}

export const RunProgress = memo(function RunProgress({
  pairId,
}: {
  pairId: string;
}) {
  const selectRun = useCallback(
    (s: RunStoreState) => getPairRun(s, pairId),
    [pairId],
  );
  const run = useRunStore(selectRun);
  const waitingAt = useRunStore(
    useCallback(
      (s: RunStoreState) => queuePosition(s, pairId, "sync"),
      [pairId],
    ),
  );

  if (!run) {
    return null;
  }

  const active =
    run.status === "queued" ||
    run.status === "running" ||
    run.status === "awaitingInput";
  const report = run.report;
  const total = run.progress?.total ?? 0;
  const current = run.progress?.current ?? 0;
  const percent =
    total > 0 ? Math.min(100, Math.round((current / total) * 100)) : 0;

  return (
    <section
      className={`run-progress run-progress-${run.status}`}
      aria-live="polite"
    >
      <header className="run-progress-header">
        <h3>{phaseLabel(run)}</h3>
        {active ? (
          <button
            type="button"
            className="btn-danger"
            onClick={() => void cancelPairRun(pairId)}
          >
            Cancel
          </button>
        ) : (
          <button type="button" onClick={() => dismissPairRun(pairId)}>
            Dismiss
          </button>
        )}
      </header>

      {run.status === "queued" && (
        <p className="run-progress-message">
          {waitingAt >= 0
            ? `Waiting for ${waitingAt + 1} job${
                waitingAt === 0 ? "" : "s"
              } ahead of it.`
            : "Waiting to start."}
        </p>
      )}

      {run.error && (
        <p className="form-error" role="alert">
          {run.error}
        </p>
      )}

      {run.progress?.message && (
        <p className="run-progress-message">{run.progress.message}</p>
      )}

      {run.progress?.path && (
        <p className="run-progress-path" title={run.progress.path}>
          {run.progress.path}
        </p>
      )}

      {total > 0 && (
        <div className="run-progress-bar-wrap">
          <progress max={100} value={percent} />
          <span className="run-progress-count">
            {current} / {total}
          </span>
        </div>
      )}

      {report && !active && (
        <p className="run-progress-summary">
          Copied {report.filesCopied} · Deleted {report.filesDeleted} ·{" "}
          {(report.bytesTransferred / 1024).toFixed(1)} KiB
          {report.errors.length > 0 && ` · ${report.errors.length} error(s)`}
        </p>
      )}
    </section>
  );
});
