import { usePairsStore } from "../../hooks/usePairsStore";
import {
  isPreviewLoadingForPair,
  selectPair,
  startNewPair,
} from "../../store/pairsStore";
import type { PairsStoreState } from "../../store/pairsStore";
import { useRunStore } from "../../hooks/useRunStore";
import { enqueuePairRuns, type RunStoreState } from "../../store/runStore";
import type { PairRunState } from "../../store/runStore";

const selectPairList = (s: PairsStoreState) => ({
  pairs: s.pairs,
  selectedId: s.selectedId,
  loading: s.loading,
});
const selectRuns = (s: RunStoreState) => s.runsByPair;

function statusBadge(run: PairRunState | undefined): string | null {
  switch (run?.status) {
    case "queued":
      return "Queued";
    case "running":
      return run.progress?.phase === "scanning" ? "Scanning" : "Syncing";
    case "awaitingInput":
      return "Needs input";
    case "completed":
      return "Done";
    case "failed":
      return "Failed";
    case "cancelled":
      return "Cancelled";
    default:
      return null;
  }
}

export function PairList() {
  const { pairs, selectedId, loading } = usePairsStore(selectPairList);
  const runsByPair = useRunStore(selectRuns);

  const queueable = pairs.filter((pair) => {
    const status = runsByPair[pair.id]?.status;
    return (
      pair.enabled &&
      status !== "queued" &&
      status !== "running" &&
      status !== "awaitingInput" &&
      // A pair mid-scan is rejected by the store, so do not count it here.
      !isPreviewLoadingForPair(pair.id)
    );
  });

  return (
    <aside className="pair-list">
      <div className="pair-list-header">
        <h2>Folder pairs</h2>
        <button type="button" className="btn-primary" onClick={startNewPair}>
          New pair
        </button>
      </div>

      {pairs.length > 1 && (
        <button
          type="button"
          className="pair-list-sync-all"
          disabled={queueable.length === 0}
          onClick={() => enqueuePairRuns(queueable)}
        >
          Queue sync for {queueable.length} enabled pair
          {queueable.length === 1 ? "" : "s"}
        </button>
      )}

      {loading && <p className="pair-list-status">Loading…</p>}

      {!loading && pairs.length === 0 && (
        <p className="pair-list-empty">
          No pairs yet. Create one to get started.
        </p>
      )}

      <ul className="pair-list-items">
        {pairs.map((pair) => {
          const badge = statusBadge(runsByPair[pair.id]);
          const run = runsByPair[pair.id];
          return (
            <li key={pair.id}>
              <button
                type="button"
                className={
                  selectedId === pair.id ? "pair-item selected" : "pair-item"
                }
                onClick={() => selectPair(pair.id)}
              >
                <span className="pair-item-name">
                  {pair.name}
                  {badge && (
                    <span className={`pair-item-badge badge-${run?.status}`}>
                      {badge}
                    </span>
                  )}
                </span>
                <span className="pair-item-paths">
                  {pair.leftPath} ↔ {pair.rightPath}
                </span>
                <span className="pair-item-meta">
                  {pair.mode}
                  {!pair.enabled && " · disabled"}
                </span>
              </button>
            </li>
          );
        })}
      </ul>
    </aside>
  );
}
