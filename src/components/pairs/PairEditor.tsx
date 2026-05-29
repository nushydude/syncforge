import { usePairsStore } from "../../hooks/usePairsStore";
import {
  cancelEdit,
  deleteSelected,
  pickFolderForSide,
  saveEditing,
  updateEditing,
} from "../../store/pairsStore";
import { FilterEditor } from "./FilterEditor";
import { ModeSelector } from "./ModeSelector";

export function PairEditor() {
  const { editing, saving, validationErrors, error, selectedId } =
    usePairsStore();

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

      <div className="form-actions">
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
    </form>
  );
}
