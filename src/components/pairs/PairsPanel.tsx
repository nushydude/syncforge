import { useEffect } from "react";
import { usePairsStore } from "../../hooks/usePairsStore";
import { useRunStore } from "../../hooks/useRunStore";
import { loadPairs, startNewPair } from "../../store/pairsStore";
import type { PairsStoreState } from "../../store/pairsStore";
import {
  dismissWatchSkipped,
  ensureWatchSkippedListener,
} from "../../store/runStore";
import type { RunStoreState } from "../../store/runStore";
import { PairEditor } from "./PairEditor";
import { PairList } from "./PairList";

const selectPairsPanel = (s: PairsStoreState) => ({
  pairs: s.pairs,
  editing: s.editing,
  loading: s.loading,
});

const selectWatchSkipped = (s: RunStoreState) => s.watchSkipped;

export function PairsPanel() {
  const { pairs, editing, loading } = usePairsStore(selectPairsPanel);
  const watchSkipped = useRunStore(selectWatchSkipped);

  useEffect(() => {
    void loadPairs();
    void ensureWatchSkippedListener();
  }, []);

  const showEmpty =
    !loading && pairs.length === 0 && editing === null;

  const skippedPairName =
    watchSkipped &&
    pairs.find((p) => p.id === watchSkipped.pairId)?.name;

  return (
    <>
      {watchSkipped && (
        <div className="watch-skipped-banner" role="alert">
          <p>
            Watch auto-sync skipped
            {skippedPairName ? ` for “${skippedPairName}”` : ""}:{" "}
            {watchSkipped.reason}
          </p>
          <button type="button" onClick={dismissWatchSkipped}>
            Dismiss
          </button>
        </div>
      )}
      <div className="pairs-panel">
      <PairList />
      <section className="pairs-main">
        {showEmpty ? (
          <div className="pairs-empty-state">
            <h2>No folder pairs</h2>
            <p>
              Create a pair to sync two folders with include/exclude filters and
              a sync mode.
            </p>
            <button type="button" className="btn-primary" onClick={startNewPair}>
              Create your first pair
            </button>
          </div>
        ) : editing ? (
          <PairEditor />
        ) : (
          <div className="pairs-hint">
            <p>Select a pair from the list or create a new one.</p>
            <button type="button" onClick={startNewPair}>
              New pair
            </button>
          </div>
        )}
      </section>
      </div>
    </>
  );
}
