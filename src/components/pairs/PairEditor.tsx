import { ConflictDialog } from "../conflicts/ConflictDialog";
import { PreviewResults } from "../preview/PreviewResults";
import { PreviewScanProgress } from "../preview/PreviewScanProgress";
import { RunProgress } from "../run/RunProgress";
import { useCallback, useEffect, useRef, useState } from "react";
import { usePairsStore } from "../../hooks/usePairsStore";
import { useRunStore } from "../../hooks/useRunStore";
import {
  cancelEdit,
  deleteSelected,
  emptyPairPreview,
  pickFolderForSide,
  saveEditing,
  updateEditing,
} from "../../store/pairsStore";
import type { PairsStoreState } from "../../store/pairsStore";
import {
  cancelConflictResolution,
  confirmConflictResolutionAndRun,
  enqueuePairRun,
  cancelPairPreview,
  enqueuePairPreview,
  isPairBusy,
  queuePosition,
  loadConflictPage,
  setConflictResolution,
} from "../../store/runStore";
import type { RunStoreState } from "../../store/runStore";
import { ConflictPolicySelector } from "./ConflictPolicySelector";
import { FilterEditor } from "./FilterEditor";
import { ModeSelector } from "./ModeSelector";

const selectPairEditor = (s: PairsStoreState) => ({
  editing: s.editing,
  saving: s.saving,
  validationErrors: s.validationErrors,
  error: s.error,
  selectedId: s.selectedId,
  watchWarning: s.watchWarning,
  scheduleError: s.scheduleError,
  scheduleDescription: s.scheduleDescription,
});

export function PairEditor() {
  const {
    editing,
    saving,
    validationErrors,
    error,
    selectedId,
    watchWarning,
    scheduleError,
    scheduleDescription,
  } = usePairsStore(selectPairEditor);
  const editingId = editing?.id ?? "";
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
      (s: PairsStoreState) => s.previews[editingId] ?? emptyPairPreview,
      [editingId],
    ),
  );
  const {
    busy: runInProgress,
    scanPosition,
    pendingConflicts,
    conflictResolutions,
  } = useRunStore(
    useCallback(
      (s: RunStoreState) => ({
        busy: isPairBusy(s, editingId),
        scanPosition: queuePosition(s, editingId, "preview"),
        pendingConflicts: s.pendingConflicts,
        conflictResolutions: s.conflictResolutions,
      }),
      [editingId],
    ),
  );
  const [showResults, setShowResults] = useState(false);
  const resultsTitleRef = useRef<HTMLHeadingElement>(null);
  const resultsTriggerRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    setShowResults(false);
  }, [editing?.id, previewPlan]);

  useEffect(() => {
    if (showResults) {
      window.requestAnimationFrame(() => resultsTitleRef.current?.focus());
    }
  }, [showResults]);

  if (!editing) {
    return null;
  }

  const isNew = !editing.id;
  // Warnings still deserve a look, so only a warning-free empty plan is "clean".
  const previewIsClean =
    previewPlan !== null &&
    (previewPlan.actionCount ?? previewPlan.actions.length) === 0 &&
    (previewPlan.scanWarnings?.length ?? 0) === 0;

  return (
    <form
      className="pair-editor"
      onSubmit={(e) => {
        e.preventDefault();
        if (runInProgress) return;
        void saveEditing();
      }}
    >
      <header className="pair-editor-header">
        <h2>{isNew ? "New folder pair" : "Edit folder pair"}</h2>
      </header>

      {/* Live scan and sync status stay directly under the header. */}
      {!isNew && (
        <PreviewScanProgress
          pairName={editing.name}
          loading={previewLoading}
          queued={previewQueued}
          position={scanPosition}
          startedAt={scanStartedAt}
          scannedEntries={scannedEntries}
          scanSide={scanSide}
          scanPath={scanPath}
          onCancel={() => void cancelPairPreview(editingId)}
        />
      )}
      {!isNew && <RunProgress pairId={editing.id} />}

      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}
      {validationErrors.length > 0 && (
        <ul className="validation-errors" role="alert">
          {validationErrors.map((msg) => (
            <li key={msg}>{msg}</li>
          ))}
        </ul>
      )}

      <label className="field">
        Name
        <input
          type="text"
          value={editing.name}
          onChange={(e) => updateEditing({ name: e.target.value })}
          placeholder="e.g. Documents backup"
          disabled={saving}
          required
        />
      </label>

      <label className="field">
        Left folder
        <div className="path-row">
          <input
            type="text"
            value={editing.leftPath}
            onChange={(e) => updateEditing({ leftPath: e.target.value })}
            placeholder="C:\Users\you\Documents"
            disabled={saving}
          />
          <button
            type="button"
            onClick={() => void pickFolderForSide("leftPath")}
            disabled={saving}
          >
            Browse…
          </button>
        </div>
      </label>

      <label className="field">
        Right folder
        <div className="path-row">
          <input
            type="text"
            value={editing.rightPath}
            onChange={(e) => updateEditing({ rightPath: e.target.value })}
            placeholder="D:\Backup\Documents"
            disabled={saving}
          />
          <button
            type="button"
            onClick={() => void pickFolderForSide("rightPath")}
            disabled={saving}
          >
            Browse…
          </button>
        </div>
      </label>

      <ModeSelector
        value={editing.mode}
        onChange={(mode) => updateEditing({ mode })}
        disabled={saving}
      />

      <ConflictPolicySelector
        value={editing.conflictPolicy}
        onChange={(conflictPolicy) => updateEditing({ conflictPolicy })}
        disabled={saving}
      />

      <FilterEditor
        filters={editing.filters}
        onChange={(filters) => updateEditing({ filters })}
        disabled={saving}
      />

      <label className="field checkbox-field">
        <input
          type="checkbox"
          checked={editing.enabled}
          onChange={(e) => updateEditing({ enabled: e.target.checked })}
          disabled={saving}
        />
        Enabled
      </label>

      <label className="field checkbox-field">
        <input
          type="checkbox"
          checked={editing.watchEnabled}
          onChange={(e) => updateEditing({ watchEnabled: e.target.checked })}
          disabled={saving || !editing.enabled}
        />
        Watch for changes (auto-sync)
      </label>
      {watchWarning && (
        <p className="form-warning" role="status">
          {watchWarning}
        </p>
      )}

      <fieldset className="schedule-fieldset">
        <legend>Scheduled sync</legend>
        <label className="field checkbox-field">
          <input
            type="checkbox"
            checked={editing.scheduleEnabled}
            onChange={(e) =>
              updateEditing({
                scheduleEnabled: e.target.checked,
                scheduleCron: e.target.checked
                  ? (editing.scheduleCron ?? "0 9 * * *")
                  : null,
              })
            }
            disabled={saving || !editing.enabled}
          />
          Enable scheduled sync
        </label>
        {editing.scheduleEnabled && (
          <>
            <label className="field">
              Cron expression
              <input
                type="text"
                value={editing.scheduleCron ?? ""}
                onChange={(e) =>
                  updateEditing({ scheduleCron: e.target.value })
                }
                placeholder="0 9 * * * (minute hour day month weekday)"
                disabled={saving}
                spellCheck={false}
              />
            </label>
            {scheduleDescription && (
              <p className="schedule-description" role="status">
                {scheduleDescription}
              </p>
            )}
            {scheduleError && (
              <p className="form-error" role="alert">
                {scheduleError}
              </p>
            )}
          </>
        )}
      </fieldset>

      {!isNew && previewPlan && !previewLoading && !showResults && (
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
          {/* Nothing to page through when the plan is empty. */}
          {!previewIsClean && (
            <button
              ref={resultsTriggerRef}
              type="button"
              onClick={() => setShowResults(true)}
              disabled={runInProgress}
            >
              View results
            </button>
          )}
        </section>
      )}

      {!isNew && showResults && previewPlan && (
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
            disabled={runInProgress}
          >
            ← Back to pair editor
          </button>
          <PreviewResults
            plan={previewPlan}
            pair={editing}
            resultsTitleRef={resultsTitleRef}
          />
        </>
      )}

      {!isNew && previewError && (
        <p className="preview-error" role="alert">
          {previewError}
        </p>
      )}

      <div className="form-actions">
        {!isNew && (
          <>
            <button
              type="button"
              className="btn-primary"
              disabled={saving || previewLoading || previewQueued}
              onClick={() => editing && enqueuePairPreview(editing)}
            >
              {previewQueued
                ? "Queued to scan…"
                : previewLoading
                  ? "Scanning…"
                  : "Preview sync"}
            </button>
            <button
              type="button"
              className="btn-primary"
              disabled={saving || runInProgress}
              onClick={() => editing && enqueuePairRun(editing)}
            >
              {runInProgress ? "Queued…" : "Run sync"}
            </button>
          </>
        )}
        <button
          type="submit"
          className="btn-primary"
          disabled={saving || runInProgress}
        >
          {saving ? "Saving…" : "Save pair"}
        </button>
        <button
          type="button"
          onClick={cancelEdit}
          disabled={saving || runInProgress}
        >
          Cancel
        </button>
        {!isNew && (
          <button
            type="button"
            className="btn-danger"
            disabled={saving || runInProgress}
            onClick={() => {
              if (
                window.confirm(
                  `Delete pair "${editing.name}"? This cannot be undone.`,
                )
              ) {
                void deleteSelected();
              }
            }}
          >
            Delete
          </button>
        )}
      </div>
      {!isNew && selectedId && (
        <p className="pair-id-hint">Pair ID: {selectedId}</p>
      )}

      {pendingConflicts && pendingConflicts.pair.id === editing.id && (
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
    </form>
  );
}
