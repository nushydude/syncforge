import { formatAction, formatPlanSummary } from '../../lib/planFormatting';
import type { SyncPlan } from '../../types';

interface PreviewTableProps {
  plan: SyncPlan | null;
  loading?: boolean;
  error?: string | null;
}

export function PreviewTable({ plan, loading, error }: PreviewTableProps) {
  if (loading) {
    return <p className="preview-status">Scanning folders…</p>;
  }

  if (error) {
    return (
      <p className="preview-error" role="alert">
        {error}
      </p>
    );
  }

  if (!plan) {
    return (
      <p className="preview-hint">
        Run preview to see planned copy, delete, and conflict actions. Preview
        does not change any files.
      </p>
    );
  }

  const rows = plan.actions.map((action) => formatAction(action));

  return (
    <section className="preview-table-section" aria-label="Sync preview">
      <header className="preview-table-header">
        <h3>Preview</h3>
        <p className="preview-summary">{formatPlanSummary(plan)}</p>
      </header>

      {plan.scanWarnings && plan.scanWarnings.length > 0 ? (
        <ul className="preview-warnings" role="status">
          {plan.scanWarnings.map((warning) => (
            <li key={warning}>{warning}</li>
          ))}
        </ul>
      ) : null}

      {rows.length === 0 ? (
        <p className="preview-empty">No changes needed — folders are in sync.</p>
      ) : (
        <div className="preview-table-wrap">
          <table className="preview-table">
            <thead>
              <tr>
                <th scope="col">Action</th>
                <th scope="col">Path</th>
                <th scope="col">Details</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row) => (
                <tr key={`${row.label}-${row.path}`} className={`tone-${row.tone}`}>
                  <td>{row.label}</td>
                  <td className="preview-path">{row.path}</td>
                  <td className="preview-detail">{row.detail || '—'}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}
