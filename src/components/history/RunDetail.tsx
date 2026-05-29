import { useHistoryStore } from '../../hooks/useHistoryStore';
import { usePairsStore } from '../../hooks/usePairsStore';
import { exportRunAsCsv, exportRunAsJson } from '../../lib/exportRun';
import {
  formatBytes,
  formatDuration,
  statsFromItems,
} from '../../lib/syncStats';

function formatTimestamp(ms: number): string {
  return new Date(ms).toLocaleString();
}

export function RunDetail() {
  const { pairs } = usePairsStore();
  const { detail, detailLoading, error } = useHistoryStore();

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
  const stats = statsFromItems(report, items);
  const pairName =
    pairs.find((p) => p.id === report.pairId)?.name ?? report.pairId;

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
          <button type="button" onClick={handleExportJson}>
            Export JSON
          </button>
          <button type="button" onClick={handleExportCsv}>
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
          <dd>{items.length}</dd>
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
        <h4>Items ({items.length})</h4>
        {items.length === 0 ? (
          <p className="history-status">No per-file records for this run.</p>
        ) : (
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
                      {item.bytes != null ? formatBytes(item.bytes) : '—'}
                    </td>
                    <td>{item.message ?? '—'}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>

      <p className="run-detail-summary">
        Summary: {stats.filesChanged} file change(s) ·{' '}
        {formatBytes(stats.bytesTransferred)} ·{' '}
        {formatDuration(stats.durationMs)}
      </p>
    </article>
  );
}
