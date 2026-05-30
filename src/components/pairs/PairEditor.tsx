import { ConflictDialog } from "../conflicts/ConflictDialog";
import { PreviewTable } from "../preview/PreviewTable";
import { RunProgress } from "../run/RunProgress";
import { usePairsStore } from "../../hooks/usePairsStore";
import { useSyncProgress } from "../../hooks/useSyncProgress";
import {
  cancelEdit,
  deleteSelected,
  pickFolderForSide,
  previewSelectedPair,
  saveEditing,
  updateEditing,
} from "../../store/pairsStore";
import {
  cancelConflictResolution,
  confirmConflictResolutionAndRun,
  runSelectedPair,
  setConflictResolution,
} from "../../store/runStore";
import { ConflictPolicySelector } from "./ConflictPolicySelector";
import { FilterEditor } from "./FilterEditor";
import { ModeSelector } from "./ModeSelector";

export function PairEditor() {
  const {
    editing,
    saving,
    validationErrors,
    error,
    selectedId,
    previewPlan,
    previewLoading,
    previewError,
    watchWarning,
    scheduleError,
    scheduleDescription,
  } = usePairsStore();
  const {
    running: runInProgress,
    pendingConflicts,
    conflictResolutions,
  } = useSyncProgress();

  if (!editing) {
    return null;
  }

  const isNew = !editing.id;

  return (
    <form
      className="pair-editor"
      onSubmit={(e) => {
        e.preventDefault();
        void saveEditing();
      }}
    >
      <header className="pair-editor-header">
        <h2>{isNew ? "New folder pair" : "Edit folder pair"}</h2>
      </header>

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
                  ? editing.scheduleCron ?? "0 9 * * *"
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

      {!isNew && (
        <PreviewTable
          plan={previewPlan}
          loading={previewLoading}
          error={previewError}
        />
      )}

      {!isNew && <RunProgress />}

      <div className="form-actions">
        {!isNew && (
          <>
            <button
              type="button"
              className="btn-primary"
              disabled={saving || previewLoading || runInProgress}
              onClick={() => void previewSelectedPair()}
            >
              {previewLoading ? "Previewing…" : "Preview sync"}
            </button>
            <button
              type="button"
              className="btn-primary"
              disabled={
                saving ||
                previewLoading ||
                runInProgress ||
                !!(pendingConflicts && pendingConflicts.pair.id === editing.id)
              }
              onClick={() => editing && void runSelectedPair(editing)}
            >
              {runInProgress ? "Running…" : "Run sync"}
            </button>
          </>
        )}
        <button type="submit" className="btn-primary" disabled={saving}>
          {saving ? "Saving…" : "Save pair"}
        </button>
        <button
          type="button"
          onClick={cancelEdit}
          disabled={saving}
        >
          Cancel
        </button>
        {!isNew && (
          <button
            type="button"
            className="btn-danger"
            disabled={saving}
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
        />
      )}
    </form>
  );
}
