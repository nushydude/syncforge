import { useCallback, useEffect, useRef, useState } from "react";
import { ConflictDialog } from "../conflicts/ConflictDialog";
import { HistoryView } from "../history/HistoryView";
import { usePairsStore } from "../../hooks/usePairsStore";
import { useRunStore } from "../../hooks/useRunStore";
import {
  isPreviewRefreshRequired,
  loadPairs,
  selectPair,
  startNewPair,
} from "../../store/pairsStore";
import type { PairsStoreState } from "../../store/pairsStore";
import {
  dismissWatchSkipped,
  cancelConflictResolution,
  confirmConflictResolutionAndRun,
  ensureWatchSkippedListener,
  ensureProgressListener,
  loadConflictPage,
  refreshStaleConflictPreview,
  setConflictResolution,
} from "../../store/runStore";
import type { RunStoreState } from "../../store/runStore";
import { RunQueuePanel } from "../run/RunQueuePanel";
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
const selectStalePairId = (s: PairsStoreState) =>
  s.pairs.find((pair) => isPreviewRefreshRequired(s.previews[pair.id]?.error))
    ?.id;

export function PairsPanel({ active = true }: { active?: boolean }) {
  const [section, setSection] = useState<"pairs" | "history">("pairs");
  const [reportRunId, setReportRunId] = useState<string>();
  const [openConflictPlan, setOpenConflictPlan] = useState<string>();
  const pairsTab = useRef<HTMLButtonElement>(null);
  const historyTab = useRef<HTMLButtonElement>(null);
  const openRunReport = useCallback((runId: string) => {
    setReportRunId(runId);
    setSection("history");
  }, []);
  const { pairs, editing, editorOpen, selectedId, loading, error } =
    usePairsStore(selectPairsPanel);
  const stalePairId = usePairsStore(selectStalePairId);
  const watchSkipped = useRunStore(selectWatchSkipped);
  const pendingConflicts = useRunStore((state) => state.pendingConflicts);
  const conflictResolutions = useRunStore((state) => state.conflictResolutions);
  const showConflict =
    active &&
    pendingConflicts &&
    (pendingConflicts.planId === openConflictPlan ||
      (section === "pairs" && selectedId === pendingConflicts.pair.id));
  const stalePair =
    section === "history"
      ? pairs.find((pair) => pair.id === stalePairId)
      : undefined;
  const restoreWorkspaceFocus = useCallback(() => {
    const currentTab =
      historyTab.current?.getAttribute("aria-selected") === "true"
        ? historyTab.current
        : pairsTab.current;
    currentTab?.focus();
  }, []);
  const showPairForRefresh = (pairId: string) => {
    if (selectedId !== pairId) selectPair(pairId);
    setSection("pairs");
  };

  useEffect(() => {
    void loadPairs();
    void ensureWatchSkippedListener();
    void ensureProgressListener();
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
          ref={pairsTab}
          type="button"
          role="tab"
          aria-selected={section === "pairs"}
          className={section === "pairs" ? "active" : ""}
          onClick={() => setSection("pairs")}
        >
          Folder pairs
        </button>
        <button
          ref={historyTab}
          type="button"
          role="tab"
          aria-selected={section === "history"}
          className={section === "history" ? "active" : ""}
          onClick={() => {
            setReportRunId(undefined);
            setSection("history");
          }}
        >
          Sync history
        </button>
      </div>
      {/* Visible on both tabs so a parked queue is never a mystery. */}
      <RunQueuePanel onResolveConflict={setOpenConflictPlan} />
      {stalePair && (
        <div className="watch-skipped-banner" role="alert">
          <p>Preview needs refreshing for “{stalePair.name}”.</p>
          <button
            type="button"
            onClick={() => showPairForRefresh(stalePair.id)}
          >
            Open folder pair
          </button>
        </div>
      )}
      {section === "history" ? (
        <HistoryView active initialRunId={reportRunId} />
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
                <PairEditor onViewReport={openRunReport} />
              ) : selectedPair ? (
                <PairDetails pair={selectedPair} onViewReport={openRunReport} />
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
      {showConflict && pendingConflicts && (
        <ConflictDialog
          key={pendingConflicts.planId}
          conflicts={pendingConflicts.conflicts}
          resolutions={conflictResolutions}
          onChoose={setConflictResolution}
          onConfirm={() => void confirmConflictResolutionAndRun()}
          onCancel={cancelConflictResolution}
          page={Math.floor(pendingConflicts.cursor / 200) + 1}
          total={pendingConflicts.total}
          hasNextPage={pendingConflicts.nextCursor != null}
          hasPreviousPage={pendingConflicts.cursor > 0}
          loading={pendingConflicts.loading}
          onNextPage={() =>
            void loadConflictPage(pendingConflicts.nextCursor ?? 0)
          }
          onPreviousPage={() =>
            void loadConflictPage(Math.max(0, pendingConflicts.cursor - 200))
          }
          pageError={pendingConflicts.pageError}
          onRetryPage={() => {
            if (pendingConflicts.pageError) {
              void loadConflictPage(pendingConflicts.pageError.cursor);
            }
          }}
          onRefresh={() => {
            const pairId = refreshStaleConflictPreview();
            if (pairId) showPairForRefresh(pairId);
          }}
          onFocusFallback={restoreWorkspaceFocus}
        />
      )}
    </>
  );
}
