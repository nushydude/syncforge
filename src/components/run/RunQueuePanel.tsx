import { useRunStore } from "../../hooks/useRunStore";
import {
  cancelPairPreview,
  cancelPairRun,
  clearRunQueue,
  getPairRun,
  isQueueActive,
  type RunStoreState,
} from "../../store/runStore";
import { useCallback } from "react";
import { usePairsStore } from "../../hooks/usePairsStore";
import {
  emptyPairPreview,
  selectPair,
  type PairPreviewState,
  type PairsStoreState,
} from "../../store/pairsStore";

// Selectors must return stable references — deriving a list here would build a
// fresh array on every snapshot read and loop useSyncExternalStore.
const selectQueue = (s: RunStoreState) => s;

const SCAN_PHASE_LABEL: Record<string, string> = {
  left: "Scanning left",
  right: "Scanning right",
  comparing: "Comparing contents",
  "hashing left": "Hashing left",
  "hashing right": "Hashing right",
};

function scanLabel(preview: PairPreviewState): string {
  const phase = SCAN_PHASE_LABEL[preview.scanSide ?? ""] ?? "Scanning";
  return preview.scannedEntries > 0
    ? `${phase} · ${preview.scannedEntries.toLocaleString()}`
    : `${phase}…`;
}

function activeLabel(
  runState: RunStoreState,
  preview: PairPreviewState,
): string {
  if (runState.activeKind === "preview") {
    return scanLabel(preview);
  }
  const active = getPairRun(runState, runState.activePairId);
  if (active?.status === "awaitingInput") {
    return "Needs conflict choices";
  }
  if (active?.progress?.phase === "scanning") {
    return "Scanning…";
  }
  const total = active?.progress?.total ?? 0;
  if (total > 0) {
    const percent = Math.min(
      100,
      Math.round(((active?.progress?.current ?? 0) / total) * 100),
    );
    return `Syncing ${percent}%`;
  }
  return "Syncing…";
}

/**
 * The shared work queue. Scans and syncs both walk the disk, so they run one at
 * a time in the order they were requested.
 */
export function RunQueuePanel() {
  const runState = useRunStore(selectQueue);
  const activePreview = usePairsStore(
    useCallback(
      (s: PairsStoreState) =>
        (runState.activePairId
          ? s.previews[runState.activePairId]
          : undefined) ?? emptyPairPreview,
      [runState.activePairId],
    ),
  );
  const activePairName =
    runState.activePairName ??
    getPairRun(runState, runState.activePairId)?.pairName ??
    runState.activePairId;
  const queued = runState.queue;

  if (!isQueueActive(runState)) {
    return null;
  }

  return (
    <section className="run-queue" aria-label="Work queue">
      <header className="run-queue-header">
        <h2>
          Work queue
          <span className="run-queue-count">
            {queued.length === 0
              ? "1 running"
              : `1 running · ${queued.length} waiting`}
          </span>
        </h2>
        {queued.length > 0 && (
          <button type="button" onClick={clearRunQueue}>
            Clear waiting
          </button>
        )}
      </header>

      <ol className="run-queue-items">
        {runState.activePairId && (
          <li className="run-queue-item run-queue-item-active">
            <span className={`run-queue-kind kind-${runState.activeKind}`}>
              {runState.activeKind === "preview" ? "Scan" : "Sync"}
            </span>
            <button
              type="button"
              className="run-queue-name"
              onClick={() => selectPair(runState.activePairId!)}
            >
              {activePairName}
            </button>
            <span className="run-queue-status">
              {activeLabel(runState, activePreview)}
            </span>
            {getPairRun(runState, runState.activePairId)?.status ===
              "awaitingInput" && (
              <button
                type="button"
                className="btn-primary"
                onClick={() => selectPair(runState.activePairId!)}
              >
                Resolve
              </button>
            )}
            <button
              type="button"
              className="btn-danger"
              onClick={() =>
                runState.activeKind === "preview"
                  ? void cancelPairPreview(runState.activePairId!)
                  : void cancelPairRun(runState.activePairId!)
              }
            >
              Cancel
            </button>
          </li>
        )}
        {queued.map((job, index) => (
          <li key={`${job.kind}:${job.pairId}`} className="run-queue-item">
            <span className={`run-queue-kind kind-${job.kind}`}>
              {job.kind === "preview" ? "Scan" : "Sync"}
            </span>
            <button
              type="button"
              className="run-queue-name"
              onClick={() => selectPair(job.pairId)}
            >
              {job.pairName}
            </button>
            <span className="run-queue-status">Waiting ({index + 1})</span>
            <button
              type="button"
              onClick={() =>
                job.kind === "preview"
                  ? void cancelPairPreview(job.pairId)
                  : void cancelPairRun(job.pairId)
              }
            >
              Cancel
            </button>
          </li>
        ))}
      </ol>
    </section>
  );
}
