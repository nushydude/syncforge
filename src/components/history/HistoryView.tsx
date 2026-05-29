import { useEffect } from 'react';
import { useHistoryStore } from '../../hooks/useHistoryStore';
import { usePairsStore } from '../../hooks/usePairsStore';
import { loadPairs } from '../../store/pairsStore';
import {
  clearRunSelection,
  loadHistory,
  selectRun,
  setPairFilter,
} from '../../store/historyStore';
import { RunDetail } from './RunDetail';

function formatTimestamp(ms: number): string {
  return new Date(ms).toLocaleString();
}

function statusLabel(status: string): string {
  switch (status) {
    case 'completed':
      return 'Completed';
    case 'failed':
      return 'Failed';
    case 'cancelled':
      return 'Cancelled';
    case 'running':
      return 'Running';
    default:
      return status;
  }
}

export function HistoryView() {
  const { pairs } = usePairsStore();
  const {
    runs,
    pairFilter,
    selectedRunId,
    loading,
    error,
  } = useHistoryStore();

  useEffect(() => {
    void loadPairs();
    void loadHistory();
  }, []);

  const pairName = (pairId: string) =>
    pairs.find((p) => p.id === pairId)?.name ?? pairId;

  return (
    <div className="history-panel">
      <aside className="history-sidebar">
        <header className="history-sidebar-header">
          <h2>Sync history</h2>
          <label className="history-filter">
            <span>Pair</span>
            <select
              value={pairFilter ?? ''}
              onChange={(e) =>
                setPairFilter(e.target.value ? e.target.value : null)
              }
            >
              <option value="">All pairs</option>
              {pairs.map((pair) => (
                <option key={pair.id} value={pair.id}>
                  {pair.name}
                </option>
              ))}
            </select>
          </label>
        </header>

        {loading && <p className="history-status">Loading history…</p>}
        {error && !selectedRunId && (
          <p className="form-error" role="alert">
            {error}
          </p>
        )}

        {!loading && runs.length === 0 && (
          <p className="history-status">No sync runs recorded yet.</p>
        )}

        <ul className="history-run-list">
          {runs.map((run) => (
            <li key={run.runId}>
              <button
                type="button"
                className={
                  run.runId === selectedRunId
                    ? 'history-run-item selected'
                    : 'history-run-item'
                }
                onClick={() => void selectRun(run.runId)}
              >
                <span className="history-run-pair">{pairName(run.pairId)}</span>
                <span className={`history-run-status status-${run.status}`}>
                  {statusLabel(run.status)}
                </span>
                <span className="history-run-time">
                  {formatTimestamp(run.startedAt)}
                </span>
                <span className="history-run-meta">
                  {run.filesCopied} copied · {run.filesDeleted} deleted
                </span>
              </button>
            </li>
          ))}
        </ul>
      </aside>

      <section className="history-main">
        {selectedRunId ? (
          <>
            <button
              type="button"
              className="history-back"
              onClick={clearRunSelection}
            >
              ← Back to list
            </button>
            <RunDetail />
          </>
        ) : (
          <div className="history-hint">
            <p>Select a run to view details and export a report.</p>
          </div>
        )}
      </section>
    </div>
  );
}
