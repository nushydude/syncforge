import { useMemo, useState } from "react";
import { useHistoryStore } from "../../hooks/useHistoryStore";
import {
  loadMoreRunItems,
  loadPreviousRunItems,
  retryRunItemsPage,
  selectRun,
} from "../../store/historyStore";
import { selectPairsList, usePairsStore } from "../../hooks/usePairsStore";
import { exportHistoryRun } from "../../api/history";
import {
  formatBytes,
  formatDuration,
  runStatusLabel,
  statsFromReport,
} from "../../lib/syncStats";
import type { HistoryStoreState } from "../../store/historyStore";
import type { RunActionFilter } from "../../types";

const actionLabels: Record<RunActionFilter, string> = {
  deletions: "Deletions",
  deleteLeft: "Delete from left",
  deleteRight: "Delete from right",
  copyLeftToRight: "Copy left to right",
  copyRightToLeft: "Copy right to left",
  createDirLeft: "Create folder on left",
  createDirRight: "Create folder on right",
  conflict: "Conflict",
  skip: "Skip",
};

function formatTimestamp(ms: number): string {
  return new Date(ms).toLocaleString();
}

const selectRunDetailHistory = (s: HistoryStoreState) => ({
  detail: s.detail,
  actionFilter: s.actionFilter,
  detailLoading: s.detailLoading,
  detailPageLoading: s.detailPageLoading,
  error: s.error,
  pageError: s.pageError,
  pageCursor: s.pageCursor,
});

export function RunDetail() {
  const {
    detail,
    actionFilter,
    detailLoading,
    detailPageLoading,
    error,
    pageError,
    pageCursor,
  } = useHistoryStore(selectRunDetailHistory);
  const [exportError, setExportError] = useState<{
    runId: string;
    message: string;
  } | null>(null);
  const [exporting, setExporting] = useState(false);
  const pairs = usePairsStore(selectPairsList);
  const pairNames = useMemo(
    () => pairs.map((p) => ({ id: p.id, name: p.name })),
    [pairs],
  );

  if (!detail && detailLoading) {
    return <p className="history-status">Loading run details…</p>;
  }

  if (!detail && error) {
    return (
      <p className="form-error" role="alert">
        {error}
      </p>
    );
  }

  if (!detail) {
    return null;
  }

  const { report, items } = detail;
  const stats = statsFromReport(report);
  const pairName =
    pairNames.find((p) => p.id === report.pairId)?.name ?? report.pairId;

  const hasMore = detail.nextCursor != null;
  const canExport = report.status !== "running" && !exporting;
  const exportHint =
    report.status === "running"
      ? "Export is available after the run finishes"
      : undefined;
  const handleExport = async (format: "json" | "csv") => {
    setExportError(null);
    setExporting(true);
    try {
      await exportHistoryRun(report.runId, format);
    } catch (e) {
      setExportError({
        runId: report.runId,
        message: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setExporting(false);
    }
  };

  return (
    <article className="run-detail">
      <header className="run-detail-header">
        <div>
          <h3>{pairName}</h3>
          <p className="run-detail-subtitle">
            {formatTimestamp(report.startedAt)}
            {report.finishedAt != null &&
              ` → ${formatTimestamp(report.finishedAt)}`}
          </p>
        </div>
        <div className="run-detail-actions">
          <button
            type="button"
            onClick={() => void handleExport("json")}
            disabled={!canExport}
            title={exportHint}
          >
            Export JSON
          </button>
          <button
            type="button"
            onClick={() => void handleExport("csv")}
            disabled={!canExport}
            title={exportHint}
          >
            Export CSV
          </button>
        </div>
      </header>

      {exportError?.runId === report.runId && (
        <p className="form-error" role="alert">
          {exportError.message}
        </p>
      )}
      <dl className="run-detail-stats">
        <div>
          <dt>Status</dt>
          <dd>{runStatusLabel(report.status)}</dd>
        </div>
        <div>
          <dt>Duration</dt>
          <dd>{formatDuration(stats.durationMs)}</dd>
        </div>
        <div>
          <dt>Files copied</dt>
          <dd>{stats.filesCopied}</dd>
        </div>
        <div>
          <dt>Files deleted</dt>
          <dd>{stats.filesDeleted}</dd>
        </div>
        <div>
          <dt>Bytes transferred</dt>
          <dd>{formatBytes(stats.bytesTransferred)}</dd>
        </div>
        <div>
          <dt>{actionFilter ? "Matching records" : "Item records"}</dt>
          <dd>
            {detailLoading ? "Loading…" : items.length}
            {!detailLoading && hasMore ? " loaded" : ""}
          </dd>
        </div>
      </dl>

      {report.errors.length > 0 && (
        <section className="run-detail-errors">
          <h4>Errors</h4>
          <ul>
            {report.errors.map((message, index) => (
              <li key={`${index}-${message}`}>{message}</li>
            ))}
          </ul>
        </section>
      )}

      <section className="run-detail-items">
        <div className="run-detail-items-header">
          <h4>
            Items
            {!detailLoading && !error && ` (${items.length} on this page)`}
          </h4>
          <label className="history-filter">
            <span>Action</span>
            <select
              value={actionFilter ?? ""}
              onChange={(event) =>
                void selectRun(
                  report.runId,
                  (event.target.value || null) as RunActionFilter | null,
                )
              }
            >
              <option value="">All actions</option>
              {Object.entries(actionLabels).map(([value, label]) => (
                <option key={value} value={value}>
                  {label}
                </option>
              ))}
            </select>
          </label>
        </div>
        {detailLoading ? (
          <p className="history-status" role="status">
            Loading items…
          </p>
        ) : error ? (
          <p className="form-error" role="alert">
            {error}
          </p>
        ) : items.length === 0 ? (
          <p className="history-status">
            {actionFilter
              ? "No items match this action."
              : "No per-file records for this run."}
          </p>
        ) : (
          <>
            <div className="run-detail-table-wrap">
              <table className="run-detail-table">
                <thead>
                  <tr>
                    <th>Path</th>
                    <th>Action</th>
                    <th>Status</th>
                    <th>Bytes</th>
                    <th>Message</th>
                  </tr>
                </thead>
                <tbody>
                  {items.map((item) => (
                    <tr key={item.id}>
                      <td className="run-detail-path">{item.path}</td>
                      <td>
                        {actionLabels[item.action as RunActionFilter] ??
                          item.action}
                      </td>
                      <td>{item.status}</td>
                      <td>
                        {item.bytes != null ? formatBytes(item.bytes) : "—"}
                      </td>
                      <td>{item.message ?? "—"}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
            {pageError && (
              <div>
                <p className="form-error" role="alert">
                  {pageError}
                </p>
                <button
                  type="button"
                  disabled={detailPageLoading}
                  onClick={() => void retryRunItemsPage()}
                >
                  Retry page
                </button>
              </div>
            )}
            {pageCursor > 0 && (
              <button
                type="button"
                disabled={detailPageLoading}
                onClick={() => void loadPreviousRunItems()}
              >
                Previous page
              </button>
            )}
            {hasMore && (
              <button
                type="button"
                className="run-detail-load-more"
                disabled={detailPageLoading}
                onClick={() => void loadMoreRunItems()}
              >
                {detailPageLoading ? "Loading…" : "Next page"}
              </button>
            )}
          </>
        )}
      </section>

      <p className="run-detail-summary">
        Summary: {stats.filesChanged} file change(s) ·{" "}
        {formatBytes(stats.bytesTransferred)} ·{" "}
        {formatDuration(stats.durationMs)}
      </p>
    </article>
  );
}
