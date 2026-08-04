import { useMemo } from "react";
import { useHistoryStore } from "../../hooks/useHistoryStore";
import { loadMoreRunItems } from "../../store/historyStore";
import { selectPairsList, usePairsStore } from "../../hooks/usePairsStore";
import { exportRunAsCsv, exportRunAsJson } from "../../lib/exportRun";
import {
  formatBytes,
  formatDuration,
  statsFromReport,
  statsFromItems,
} from "../../lib/syncStats";
import type { HistoryStoreState } from "../../store/historyStore";

function formatTimestamp(ms: number): string {
  return new Date(ms).toLocaleString();
}

const selectRunDetailHistory = (s: HistoryStoreState) => ({
  detail: s.detail,
  detailLoading: s.detailLoading,
  detailPageLoading: s.detailPageLoading,
  error: s.error,
});

export function RunDetail() {
  const { detail, detailLoading, detailPageLoading, error } = useHistoryStore(
    selectRunDetailHistory,
  );
  const pairs = usePairsStore(selectPairsList);
  const pairNames = useMemo(
    () => pairs.map((p) => ({ id: p.id, name: p.name })),
    [pairs],
  );

  if (detailLoading) {
    return <p className="history-status">Loading run details…</p>;
  }

  if (error) {
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
  const stats =
    detail.nextCursor != null
      ? statsFromReport(report)
      : statsFromItems(report, items);
  const pairName =
    pairNames.find((p) => p.id === report.pairId)?.name ?? report.pairId;

  const hasMore = detail.nextCursor != null;

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
            disabled={hasMore}
            title={hasMore ? "Load all items before exporting" : undefined}
          >
            Export JSON
          </button>
          <button
            type="button"
            onClick={handleExportCsv}
            disabled={hasMore}
            title={hasMore ? "Load all items before exporting" : undefined}
          >
            Export CSV
          </button>
        </div>
      </header>

      <dl className="run-detail-stats">
        <div>
          <dt>Status</dt>
          <dd>{report.status}</dd>
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
          <dt>Item records</dt>
          <dd>
            {items.length}
            {hasMore ? " loaded" : ""}
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
        <h4>
          Items ({items.length}
          {hasMore ? "+" : ""})
        </h4>
        {items.length === 0 ? (
          <p className="history-status">No per-file records for this run.</p>
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
                      <td>{item.action}</td>
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
