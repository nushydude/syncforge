import { ConflictDialog } from "../conflicts/ConflictDialog";
import { useCallback, useEffect, useRef, useState } from "react";
import { PreviewResults } from "../preview/PreviewResults";
import { PreviewScanProgress } from "../preview/PreviewScanProgress";
import { RunProgress } from "../run/RunProgress";
import { usePairsStore } from "../../hooks/usePairsStore";
import { useRunStore } from "../../hooks/useRunStore";
import type { PairsStoreState } from "../../store/pairsStore";
import { beginEdit, emptyPairPreview } from "../../store/pairsStore";
import {
  cancelConflictResolution,
  cancelPairPreview,
  confirmConflictResolutionAndRun,
  enqueuePairPreview,
  enqueuePairRun,
  isPairBusy,
  loadConflictPage,
  queuePosition,
  setConflictResolution,
} from "../../store/runStore";
import type { RunStoreState } from "../../store/runStore";
import type { ConflictPolicy, FolderPair, SyncMode } from "../../types";
import { LastSyncedText } from "./LastSyncedText";

interface PairDetailsProps {
  pair: FolderPair;
}

function modeLabel(mode: SyncMode): string {
  switch (mode) {
    case "synchronize":
      return "Synchronize both ways";
    case "echo":
      return "Echo left to right";
    case "contribute":
      return "Contribute to both folders";
  }
}

function conflictPolicyLabel(policy: ConflictPolicy): string {
  switch (policy) {
    case "newerWins":
      return "Newer file wins";
    case "left":
      return "Always use left";
    case "right":
      return "Always use right";
    case "keepBoth":
      return "Keep both files";
    case "ask":
      return "Ask before resolving";
  }
}

function listLabel(values: string[]): string {
  return values.length > 0 ? values.join(", ") : "None";
}

export function PairDetails({ pair }: PairDetailsProps) {
  const [showResults, setShowResults] = useState(false);
  const resultsTitleRef = useRef<HTMLHeadingElement>(null);
  const resultsTriggerRef = useRef<HTMLButtonElement>(null);
  const {
    plan: previewPlan,
    queued: previewQueued,
    loading: previewLoading,
    error: previewError,
    startedAt: scanStartedAt,
    scannedEntries,
    scanSide,
    scanPath,
  } = usePairsStore(
    useCallback(
      (s: PairsStoreState) => s.previews[pair.id] ?? emptyPairPreview,
      [pair.id],
    ),
  );
  const { busy, scanPosition, pendingConflicts, conflictResolutions } =
    useRunStore(
      useCallback(
        (s: RunStoreState) => ({
          busy: isPairBusy(s, pair.id),
          scanPosition: queuePosition(s, pair.id, "preview"),
          pendingConflicts: s.pendingConflicts,
          conflictResolutions: s.conflictResolutions,
        }),
        [pair.id],
      ),
    );
  const lastSyncedAt = usePairsStore(
    useCallback(
      (s: PairsStoreState) => s.lastSyncedAtByPair[pair.id],
      [pair.id],
    ),
  );
  const hasPendingConflicts = pendingConflicts?.pair.id === pair.id;
  const scanBusy = previewLoading || previewQueued;
  // Warnings still deserve a look, so only a warning-free empty plan is "clean".
  const previewIsClean =
    previewPlan !== null &&
    (previewPlan.actionCount ?? previewPlan.actions.length) === 0 &&
    (previewPlan.scanWarnings?.length ?? 0) === 0;

  useEffect(() => {
    setShowResults(false);
  }, [pair.id, previewPlan]);

  useEffect(() => {
    if (showResults) {
      window.requestAnimationFrame(() => resultsTitleRef.current?.focus());
    }
  }, [showResults]);

  return (
    <section className="pair-details" aria-labelledby="pair-details-title">
      <header className="pair-details-header">
        <div>
          <h2 id="pair-details-title">{pair.name}</h2>
          <p className="pair-details-subtitle">Folder pair details</p>
        </div>
        {/* While a scan is queued or running, cancelling is the only action —
            showing disabled Edit/Run buttons just invites dead clicks. The
            status card below already says what is happening. */}
        {!scanBusy && (
          <div className="pair-details-actions">
            <button type="button" onClick={beginEdit} disabled={busy}>
              Edit pair
            </button>
            <button
              type="button"
              className="btn-primary"
              onClick={() => enqueuePairPreview(pair)}
            >
              Preview sync
            </button>
            <button
              type="button"
              className="btn-primary"
              disabled={busy}
              onClick={() => enqueuePairRun(pair)}
            >
              {busy ? "Queued…" : "Run sync"}
            </button>
          </div>
        )}
      </header>

      {/* Live scan and sync status stay directly under the header. */}
      <PreviewScanProgress
        pairName={pair.name}
        loading={previewLoading}
        queued={previewQueued}
        position={scanPosition}
        startedAt={scanStartedAt}
        scannedEntries={scannedEntries}
        scanSide={scanSide}
        scanPath={scanPath}
        onCancel={() => void cancelPairPreview(pair.id)}
      />
      <RunProgress pairId={pair.id} />

      {showResults && previewPlan ? (
        <>
          <button
            type="button"
            className="preview-back-button"
            onClick={() => {
              setShowResults(false);
              window.requestAnimationFrame(() =>
                resultsTriggerRef.current?.focus(),
              );
            }}
          >
            ← Back to pair details
          </button>
          <PreviewResults
            plan={previewPlan}
            pair={pair}
            resultsTitleRef={resultsTitleRef}
          />
        </>
      ) : (
        <>
          {previewPlan && !previewLoading && (
            <section
              className={
                previewIsClean
                  ? "preview-ready-card preview-ready-clean"
                  : "preview-ready-card"
              }
              aria-live="polite"
            >
              <div>
                <strong>
                  {previewIsClean ? "Nothing to sync" : "Preview ready"}
                </strong>
                <p>
                  {previewIsClean
                    ? "Both folders are already identical."
                    : "Review the planned changes before syncing."}
                </p>
              </div>
              {/* A clean result has nothing to page through — do not make the
                  user click into an empty table to learn that. */}
              {!previewIsClean && (
                <button
                  ref={resultsTriggerRef}
                  type="button"
                  onClick={() => setShowResults(true)}
                >
                  View results
                </button>
              )}
            </section>
          )}

          <dl className="pair-details-grid">
            <div>
              <dt>Left folder</dt>
              <dd className="pair-details-value-path">{pair.leftPath}</dd>
            </div>
            <div>
              <dt>Right folder</dt>
              <dd className="pair-details-value-path">{pair.rightPath}</dd>
            </div>
            <div>
              <dt>Sync mode</dt>
              <dd>{modeLabel(pair.mode)}</dd>
            </div>
            <div>
              <dt>Conflict policy</dt>
              <dd>{conflictPolicyLabel(pair.conflictPolicy)}</dd>
            </div>
            <div>
              <dt>Status</dt>
              <dd>{pair.enabled ? "Enabled" : "Disabled"}</dd>
            </div>
            <div>
              <dt>Last synced</dt>
              <dd>
                <LastSyncedText timestamp={lastSyncedAt} />
              </dd>
            </div>
            <div>
              <dt>Watch for changes</dt>
              <dd>{pair.watchEnabled ? "Enabled" : "Disabled"}</dd>
            </div>
            <div>
              <dt>Scheduled sync</dt>
              <dd>
                {pair.scheduleEnabled
                  ? `Enabled${pair.scheduleCron ? ` (${pair.scheduleCron})` : ""}`
                  : "Disabled"}
              </dd>
            </div>
            <div className="pair-details-filters">
              <dt>Include filters</dt>
              <dd>{listLabel(pair.filters.include)}</dd>
              <dt>Exclude filters</dt>
              <dd>{listLabel(pair.filters.exclude)}</dd>
            </div>
          </dl>
          {previewError && (
            <p className="preview-error" role="alert">
              {previewError}
            </p>
          )}
        </>
      )}

      {hasPendingConflicts && pendingConflicts && (
        <ConflictDialog
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
        />
      )}
    </section>
  );
}
