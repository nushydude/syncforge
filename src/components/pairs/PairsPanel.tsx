import { useEffect, useState } from "react";
import { HistoryView } from "../history/HistoryView";
import { usePairsStore } from "../../hooks/usePairsStore";
import { useRunStore } from "../../hooks/useRunStore";
import { loadPairs, startNewPair } from "../../store/pairsStore";
import type { PairsStoreState } from "../../store/pairsStore";
import {
  dismissWatchSkipped,
  ensureWatchSkippedListener,
} from "../../store/runStore";
import type { RunStoreState } from "../../store/runStore";
import { PairDetails } from "./PairDetails";
import { PairEditor } from "./PairEditor";
import { PairList } from "./PairList";

const selectPairsPanel = (s: PairsStoreState) => ({
  pairs: s.pairs,
  editing: s.editing,
  editorOpen: s.editorOpen,
  selectedId: s.selectedId,
  loading: s.loading,
  error: s.error,
});

const selectWatchSkipped = (s: RunStoreState) => s.watchSkipped;

export function PairsPanel() {
  const [section, setSection] = useState<"pairs" | "history">("pairs");
  const { pairs, editing, editorOpen, selectedId, loading, error } =
    usePairsStore(selectPairsPanel);
  const watchSkipped = useRunStore(selectWatchSkipped);

  useEffect(() => {
    void loadPairs();
    void ensureWatchSkippedListener();
  }, []);

  const showEmpty = !loading && pairs.length === 0 && editing === null;

  const skippedPairName =
    watchSkipped && pairs.find((p) => p.id === watchSkipped.pairId)?.name;
  const selectedPair = pairs.find((pair) => pair.id === selectedId);

  return (
    <>
      <div
        className="pairs-workspace-tabs"
        role="tablist"
        aria-label="Sync folder pairs"
      >
        <button
          type="button"
          role="tab"
          aria-selected={section === "pairs"}
          className={section === "pairs" ? "active" : ""}
          onClick={() => setSection("pairs")}
        >
          Folder pairs
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={section === "history"}
          className={section === "history" ? "active" : ""}
          onClick={() => setSection("history")}
        >
          Sync history
        </button>
      </div>
      {section === "history" ? (
        <HistoryView active />
      ) : (
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
              {error ? (
                <div className="pairs-load-error" role="alert">
                  <h2>Could not load folder pairs</h2>
                  <p>{error}</p>
                  <button type="button" onClick={() => void loadPairs()}>
                    Retry
                  </button>
                </div>
              ) : showEmpty ? (
                <div className="pairs-empty-state">
                  <h2>No folder pairs</h2>
                  <p>
                    Create a pair to sync two folders with include/exclude
                    filters and a sync mode.
                  </p>
                  <button
                    type="button"
                    className="btn-primary"
                    onClick={startNewPair}
                  >
                    Create your first pair
                  </button>
                </div>
              ) : editing && editorOpen ? (
                <PairEditor />
              ) : selectedPair ? (
                <PairDetails pair={selectedPair} />
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
      )}
    </>
  );
}
