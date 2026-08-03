import { ConflictDialog } from "../conflicts/ConflictDialog";
import { useEffect, useRef, useState } from "react";
import { PreviewResults } from "../preview/PreviewResults";
import { PreviewScanProgress } from "../preview/PreviewScanProgress";
import { RunProgress } from "../run/RunProgress";
import { usePairsStore } from "../../hooks/usePairsStore";
import { useRunStore } from "../../hooks/useRunStore";
import type { PairsStoreState } from "../../store/pairsStore";
import { beginEdit, previewSelectedPair } from "../../store/pairsStore";
import {
  cancelConflictResolution,
  confirmConflictResolutionAndRun,
  runSelectedPair,
  setConflictResolution,
} from "../../store/runStore";
import type { RunStoreState } from "../../store/runStore";
import type { ConflictPolicy, FolderPair, SyncMode } from "../../types";

interface PairDetailsProps {
  pair: FolderPair;
}

const selectPairDetails = (s: PairsStoreState) => ({
  previewPlan: s.previewPlan,
  previewLoading: s.previewLoading,
  previewError: s.previewError,
});

const selectPairDetailsRun = (s: RunStoreState) => ({
  running: s.running,
  pendingConflicts: s.pendingConflicts,
  conflictResolutions: s.conflictResolutions,
});

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
  const { previewPlan, previewLoading, previewError } =
    usePairsStore(selectPairDetails);
  const { running, pendingConflicts, conflictResolutions } =
    useRunStore(selectPairDetailsRun);
  const hasPendingConflicts = pendingConflicts?.pair.id === pair.id;

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
        <div className="pair-details-actions">
          <button type="button" onClick={beginEdit} disabled={running}>
            Edit pair
          </button>
          <button
            type="button"
            className="btn-primary"
            disabled={previewLoading || running}
            onClick={() => void previewSelectedPair()}
          >
            {previewLoading ? "Previewing..." : "Preview sync"}
          </button>
          <button
            type="button"
            className="btn-primary"
            disabled={previewLoading || running || hasPendingConflicts}
            onClick={() => void runSelectedPair(pair)}
          >
            {running ? "Running..." : "Run sync"}
          </button>
        </div>
      </header>

      <PreviewScanProgress loading={previewLoading} />

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
            disabled={running}
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
            <section className="preview-ready-card" aria-live="polite">
              <div>
                <strong>Preview ready</strong>
                <p>
                  {previewPlan.actions.length === 0
                    ? "Folders are already in sync."
                    : "Review the planned changes before syncing."}
                </p>
              </div>
              <button
                ref={resultsTriggerRef}
                type="button"
                onClick={() => setShowResults(true)}
                disabled={running}
              >
                View results
              </button>
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
      <RunProgress />

      {hasPendingConflicts && pendingConflicts && (
        <ConflictDialog
          conflicts={pendingConflicts.conflicts}
          resolutions={conflictResolutions}
          onChoose={setConflictResolution}
          onConfirm={() => void confirmConflictResolutionAndRun()}
          onCancel={cancelConflictResolution}
        />
      )}
    </section>
  );
}
