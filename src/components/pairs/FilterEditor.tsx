import { useState } from "react";
import {
  addFilterPattern,
  filtersToText,
  parseFilterLines,
  removeFilterPattern,
} from "../../lib/filters";
import type { Filters } from "../../types";

interface FilterEditorProps {
  filters: Filters;
  onChange: (filters: Filters) => void;
  disabled?: boolean;
}

function PatternList({
  title,
  patterns,
  onRemove,
  disabled,
}: {
  title: string;
  patterns: string[];
  onRemove: (index: number) => void;
  disabled?: boolean;
}) {
  return (
    <div className="filter-list">
      <h4>{title}</h4>
      {patterns.length === 0 ? (
        <p className="filter-empty">No patterns</p>
      ) : (
        <ul>
          {patterns.map((pattern, index) => (
            <li key={`${pattern}-${index}`}>
              <code>{pattern}</code>
              <button
                type="button"
                className="btn-icon"
                onClick={() => onRemove(index)}
                disabled={disabled}
                aria-label={`Remove ${pattern}`}
              >
                ×
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

export function FilterEditor({
  filters,
  onChange,
  disabled,
}: FilterEditorProps) {
  const [includeDraft, setIncludeDraft] = useState("");
  const [excludeDraft, setExcludeDraft] = useState("");

  const applyInclude = () => {
    const lines = parseFilterLines(includeDraft);
    if (lines.length === 0) {
      return;
    }
    let include = [...filters.include];
    for (const line of lines) {
      include = addFilterPattern(include, line);
    }
    onChange({ ...filters, include });
    setIncludeDraft("");
  };

  const applyExclude = () => {
    const lines = parseFilterLines(excludeDraft);
    if (lines.length === 0) {
      return;
    }
    let exclude = [...filters.exclude];
    for (const line of lines) {
      exclude = addFilterPattern(exclude, line);
    }
    onChange({ ...filters, exclude });
    setExcludeDraft("");
  };

  return (
    <fieldset className="filter-editor" disabled={disabled}>
      <legend>Include / exclude filters</legend>
      <p className="field-hint">
        Glob patterns (e.g. <code>*.txt</code>). One per line when adding.
      </p>

      <div className="filter-add">
        <label>
          Add include patterns
          <textarea
            value={includeDraft}
            onChange={(e) => setIncludeDraft(e.target.value)}
            placeholder={filtersToText(["*.doc", "*.pdf"])}
            rows={2}
          />
        </label>
        <button type="button" onClick={applyInclude} disabled={disabled}>
          Add include
        </button>
      </div>

      <div className="filter-add">
        <label>
          Add exclude patterns
          <textarea
            value={excludeDraft}
            onChange={(e) => setExcludeDraft(e.target.value)}
            placeholder={filtersToText(["*.tmp", "Thumbs.db"])}
            rows={2}
          />
        </label>
        <button type="button" onClick={applyExclude} disabled={disabled}>
          Add exclude
        </button>
      </div>

      <div className="filter-columns">
        <PatternList
          title="Include"
          patterns={filters.include}
          onRemove={(index) =>
            onChange({
              ...filters,
              include: removeFilterPattern(filters.include, index),
            })
          }
          disabled={disabled}
        />
        <PatternList
          title="Exclude"
          patterns={filters.exclude}
          onRemove={(index) =>
            onChange({
              ...filters,
              exclude: removeFilterPattern(filters.exclude, index),
            })
          }
          disabled={disabled}
        />
      </div>
    </fieldset>
  );
}
