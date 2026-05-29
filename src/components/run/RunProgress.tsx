import { useSyncProgress } from '../../hooks/useSyncProgress';
import { cancelActiveRun } from '../../store/runStore';

function phaseLabel(phase: string | undefined): string {
  switch (phase) {
    case 'scanning':
      return 'Scanning…';
    case 'running':
      return 'Syncing…';
    case 'completed':
      return 'Completed';
    case 'failed':
      return 'Failed';
    case 'cancelled':
      return 'Cancelled';
    default:
      return 'Running…';
  }
}

export function RunProgress() {
  const { running, progress, lastReport, error } = useSyncProgress();

  if (!running && !progress && !lastReport && !error) {
    return null;
  }

  const phase = progress?.phase ?? lastReport?.status;
  const total = progress?.total ?? 0;
  const current = progress?.current ?? 0;
  const percent =
    total > 0 ? Math.min(100, Math.round((current / total) * 100)) : 0;

  return (
    <section className="run-progress" aria-live="polite">
      <header className="run-progress-header">
        <h3>{phaseLabel(phase)}</h3>
        {running && (
          <button
            type="button"
            className="btn-danger"
            onClick={() => void cancelActiveRun()}
          >
            Cancel
          </button>
        )}
      </header>

      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}

      {progress?.message && (
        <p className="run-progress-message">{progress.message}</p>
      )}

      {progress?.path && (
        <p className="run-progress-path" title={progress.path}>
          {progress.path}
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

      {lastReport && !running && (
        <p className="run-progress-summary">
          Copied {lastReport.filesCopied} · Deleted {lastReport.filesDeleted} ·{' '}
          {(lastReport.bytesTransferred / 1024).toFixed(1)} KiB
          {lastReport.errors.length > 0 &&
            ` · ${lastReport.errors.length} error(s)`}
        </p>
      )}
    </section>
  );
}
