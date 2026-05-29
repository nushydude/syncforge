import { usePairsStore } from "../../hooks/usePairsStore";
import { selectPair, startNewPair } from "../../store/pairsStore";

export function PairList() {
  const { pairs, selectedId, loading } = usePairsStore();

  return (
    <aside className="pair-list">
      <div className="pair-list-header">
        <h2>Folder pairs</h2>
        <button type="button" className="btn-primary" onClick={startNewPair}>
          New pair
        </button>
      </div>

      {loading && <p className="pair-list-status">Loading…</p>}

      {!loading && pairs.length === 0 && (
        <p className="pair-list-empty">No pairs yet. Create one to get started.</p>
      )}

      <ul className="pair-list-items">
        {pairs.map((pair) => (
          <li key={pair.id}>
            <button
              type="button"
              className={
                selectedId === pair.id ? "pair-item selected" : "pair-item"
              }
              onClick={() => selectPair(pair.id)}
            >
              <span className="pair-item-name">{pair.name}</span>
              <span className="pair-item-paths">
                {pair.leftPath} ↔ {pair.rightPath}
              </span>
              <span className="pair-item-meta">
                {pair.mode}
                {!pair.enabled && " · disabled"}
              </span>
            </button>
          </li>
        ))}
      </ul>
    </aside>
  );
}
