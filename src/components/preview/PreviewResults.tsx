import { useEffect, useMemo, useRef, useState, type RefObject } from "react";
import { getPreviewActions } from "../../api/preview";
import { formatAction, formatPlanSummary } from "../../lib/planFormatting";
import type { FolderPair, PreviewSummary, SyncAction } from "../../types";

const PAGE_SIZE = 50;
const BACKEND_PAGE_SIZE = 200;

export function PreviewResults({
  plan,
  pair,
  resultsTitleRef,
}: {
  plan: PreviewSummary;
  pair?: FolderPair;
  resultsTitleRef?: RefObject<HTMLHeadingElement | null>;
}) {
  const [page, setPage] = useState(1);
  const [actions, setActions] = useState<SyncAction[]>(plan.actions);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const requestId = useRef(0);
  const visibleActions = plan.planId
    ? actions.slice(0, BACKEND_PAGE_SIZE)
    : actions.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE);
  const rows = useMemo(
    () => visibleActions.map(formatAction),
    [visibleActions],
  );
  const totalActions = plan.actionCount ?? plan.actions.length;
  const pageCount = Math.max(
    1,
    Math.ceil(totalActions / (plan.planId ? BACKEND_PAGE_SIZE : PAGE_SIZE)),
  );

  useEffect(() => {
    setPage(1);
    setActions(plan.actions);
    setLoading(false);
    setError(null);
    return () => {
      requestId.current += 1;
    };
  }, [plan]);

  async function changePage(nextPage: number): Promise<void> {
    if (loading || nextPage < 1 || nextPage > pageCount || nextPage === page)
      return;
    if (!plan.planId) {
      setPage(nextPage);
      return;
    }
    if (!pair) return;
    const currentRequest = ++requestId.current;
    setLoading(true);
    setError(null);
    try {
      const result = await getPreviewActions(
        pair,
        plan.planId,
        (nextPage - 1) * BACKEND_PAGE_SIZE,
        BACKEND_PAGE_SIZE,
      );
      if (currentRequest !== requestId.current) return;
      setActions(result.actions);
      setPage(nextPage);
    } catch (loadError) {
      if (currentRequest === requestId.current) {
        setError(
          `Could not load preview page: ${loadError instanceof Error ? loadError.message : String(loadError)}. Try again.`,
        );
      }
    } finally {
      if (currentRequest === requestId.current) setLoading(false);
    }
  }

  return (
    <section
      className="preview-results"
      aria-labelledby="preview-results-title"
      aria-busy={loading}
    >
      <header className="preview-results-header">
        <div>
          <p className="preview-eyebrow">Preview complete</p>
          <h3 id="preview-results-title" tabIndex={-1} ref={resultsTitleRef}>
            Review changes
          </h3>
          <p className="preview-summary">
            {plan.actionCount !== undefined
              ? `${plan.actionCount} action${plan.actionCount === 1 ? "" : "s"}`
              : formatPlanSummary(plan)}
          </p>
        </div>
        <span className="preview-results-count">
          {totalActions === 0
            ? "No changes"
            : `${totalActions} change${totalActions === 1 ? "" : "s"}`}
        </span>
      </header>

      {plan.scanWarnings && plan.scanWarnings.length > 0 && (
        <ul className="preview-warnings" role="status">
          {plan.scanWarnings.map((warning) => (
            <li key={warning}>{warning}</li>
          ))}
        </ul>
      )}

      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}

      {rows.length === 0 ? (
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
              {rows.map((row, index) => (
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
            onClick={() => void changePage(page - 1)}
            disabled={page === 1 || loading}
          >
            Previous
          </button>
          <span>
            Page {page} of {pageCount}
          </span>
          <button
            type="button"
            onClick={() => void changePage(page + 1)}
            disabled={page === pageCount || loading}
          >
            {loading ? "Loading..." : "Next"}
          </button>
        </nav>
      )}
    </section>
  );
}
