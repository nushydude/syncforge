import { useEffect, useMemo, useRef } from "react";
import { useHistoryStore } from "../../hooks/useHistoryStore";
import { selectPairsList, usePairsStore } from "../../hooks/usePairsStore";
import {
  clearRunSelection,
  loadHistory,
  selectRun,
  setPairFilter,
} from "../../store/historyStore";
import type { HistoryStoreState } from "../../store/historyStore";
import { RunDetail } from "./RunDetail";

function formatTimestamp(ms: number): string {
  return new Date(ms).toLocaleString();
}

function statusLabel(status: string): string {
  switch (status) {
    case "completed":
      return "Completed";
    case "failed":
      return "Failed";
    case "cancelled":
      return "Cancelled";
    case "running":
      return "Running";
    default:
      return status;
  }
}

const selectHistoryList = (s: HistoryStoreState) => ({
  runs: s.runs,
  pairFilter: s.pairFilter,
  selectedRunId: s.selectedRunId,
  loading: s.loading,
  error: s.error,
});

interface HistoryViewProps {
  /** When false, history is not fetched (panel may stay mounted but hidden). */
  active?: boolean;
}

export function HistoryView({ active = true }: HistoryViewProps) {
  const pairs = usePairsStore(selectPairsList);
  const pairNames = useMemo(
    () => pairs.map((p) => ({ id: p.id, name: p.name })),
    [pairs],
  );
  const { runs, pairFilter, selectedRunId, loading, error } =
    useHistoryStore(selectHistoryList);
  const historyLoadedRef = useRef(false);

  useEffect(() => {
    if (!active || historyLoadedRef.current) {
      return;
    }
    historyLoadedRef.current = true;
    void loadHistory();
  }, [active]);

  const pairNameById = useMemo(
    () => new Map(pairNames.map((p) => [p.id, p.name])),
    [pairNames],
  );
  const pairName = (pairId: string) => pairNameById.get(pairId) ?? pairId;

  return (
    <div className="history-panel">
      <aside className="history-sidebar">
        <header className="history-sidebar-header">
          <h2>Sync history</h2>
          <label className="history-filter">
            <span>Pair</span>
            <select
              value={pairFilter ?? ""}
              onChange={(e) =>
                setPairFilter(e.target.value ? e.target.value : null)
              }
            >
              <option value="">All pairs</option>
              {pairNames.map((pair) => (
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
                    ? "history-run-item selected"
                    : "history-run-item"
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
