import { useEffect, useMemo, useState, type RefObject } from "react";
import { formatAction, formatPlanSummary } from "../../lib/planFormatting";
import type { SyncPlan } from "../../types";

const PAGE_SIZE = 50;

export function PreviewResults({
  plan,
  resultsTitleRef,
}: {
  plan: SyncPlan;
  resultsTitleRef?: RefObject<HTMLHeadingElement | null>;
}) {
  const [page, setPage] = useState(1);
  const rows = useMemo(() => plan.actions.map(formatAction), [plan]);
  const pageCount = Math.max(1, Math.ceil(rows.length / PAGE_SIZE));
  const visibleRows = rows.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE);

  useEffect(() => setPage(1), [plan]);

  return (
    <section
      className="preview-results"
      aria-labelledby="preview-results-title"
    >
      <header className="preview-results-header">
        <div>
          <p className="preview-eyebrow">Preview complete</p>
          <h3 id="preview-results-title" tabIndex={-1} ref={resultsTitleRef}>
            Review changes
          </h3>
          <p className="preview-summary">{formatPlanSummary(plan)}</p>
        </div>
        <span className="preview-results-count">
          {rows.length === 0
            ? "No changes"
            : `${rows.length} change${rows.length === 1 ? "" : "s"}`}
        </span>
      </header>

      {plan.scanWarnings && plan.scanWarnings.length > 0 && (
        <ul className="preview-warnings" role="status">
          {plan.scanWarnings.map((warning) => (
            <li key={warning}>{warning}</li>
          ))}
        </ul>
      )}

      {visibleRows.length === 0 ? (
        <p className="preview-empty">
          No changes needed — folders are in sync.
        </p>
      ) : (
        <div className="preview-results-table-wrap">
          <table className="preview-table">
            <thead>
              <tr>
                <th>Action</th>
                <th>Path</th>
                <th>Details</th>
              </tr>
            </thead>
            <tbody>
              {visibleRows.map((row, index) => (
                <tr
                  key={`${row.label}-${row.path}-${index}`}
                  className={`tone-${row.tone}`}
                >
                  <td>{row.label}</td>
                  <td className="preview-path">{row.path}</td>
                  <td className="preview-detail">{row.detail || "—"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {pageCount > 1 && (
        <nav className="preview-pagination" aria-label="Preview results pages">
          <button
            type="button"
            onClick={() => setPage((current) => Math.max(1, current - 1))}
            disabled={page === 1}
          >
            Previous
          </button>
          <span>
            Page {page} of {pageCount}
          </span>
          <button
            type="button"
            onClick={() =>
              setPage((current) => Math.min(pageCount, current + 1))
            }
            disabled={page === pageCount}
          >
            Next
          </button>
        </nav>
      )}
    </section>
  );
}
