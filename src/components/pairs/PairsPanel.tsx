import { useEffect } from "react";
import { usePairsStore } from "../../hooks/usePairsStore";
import { loadPairs, startNewPair } from "../../store/pairsStore";
import { PairEditor } from "./PairEditor";
import { PairList } from "./PairList";

export function PairsPanel() {
  const { pairs, editing, loading } = usePairsStore();

  useEffect(() => {
    void loadPairs();
  }, []);

  const showEmpty =
    !loading && pairs.length === 0 && editing === null;

  return (
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
  );
}
