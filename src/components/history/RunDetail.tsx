import { useMemo } from "react";
import { useHistoryStore } from "../../hooks/useHistoryStore";
import { loadMoreRunItems, selectRun } from "../../store/historyStore";
import { selectPairsList, usePairsStore } from "../../hooks/usePairsStore";
import { exportRunAsCsv, exportRunAsJson } from "../../lib/exportRun";
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
});

export function RunDetail() {
  const { detail, actionFilter, detailLoading, detailPageLoading, error } =
    useHistoryStore(selectRunDetailHistory);
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
  const canExport = !actionFilter && !detailLoading && !error && !hasMore;
  const exportHint = actionFilter
    ? "Select All actions to export the full report"
    : hasMore
      ? "Load all items before exporting"
      : undefined;

  const handleExportJson = () => exportRunAsJson(detail);
  const handleExportCsv = () => exportRunAsCsv(detail);

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
            onClick={handleExportJson}
            disabled={!canExport}
            title={exportHint}
          >
            Export JSON
          </button>
          <button
            type="button"
            onClick={handleExportCsv}
            disabled={!canExport}
            title={exportHint}
          >
            Export CSV
          </button>
        </div>
      </header>

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
            {!detailLoading &&
              !error &&
              ` (${items.length}${hasMore ? "+" : ""})`}
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
            {hasMore && (
              <button
                type="button"
                className="run-detail-load-more"
                disabled={detailPageLoading}
                onClick={() => void loadMoreRunItems()}
              >
                {detailPageLoading ? "Loading…" : "Load more"}
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
