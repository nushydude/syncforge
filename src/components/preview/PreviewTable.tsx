import { useVirtualizer } from "@tanstack/react-virtual";
import { useMemo, useRef, type RefObject, type ReactNode } from "react";
import { formatAction, formatPlanSummary } from "../../lib/planFormatting";
import type { FormattedAction } from "../../lib/planFormatting";
import type { SyncPlan } from "../../types";

interface PreviewTableProps {
  plan: SyncPlan | null;
  loading?: boolean;
  error?: string | null;
}

const PREVIEW_ROW_HEIGHT = 40;
const PREVIEW_TABLE_MAX_HEIGHT = 360;
/** Below this count, a plain table is cheaper than virtualizer setup. */
const VIRTUALIZE_THRESHOLD = 100;

function PreviewRowsTable({ rows }: { rows: FormattedAction[] }) {
  return (
    <>
      {rows.map((row, index) => (
        <tr
          key={`${row.label}-${row.path}-${index}`}
          className={`tone-${row.tone}`}
          data-index={index}
        >
          <td>{row.label}</td>
          <td className="preview-path">{row.path}</td>
          <td className="preview-detail">{row.detail || "—"}</td>
        </tr>
      ))}
    </>
  );
}

function VirtualizedPreviewRows({ rows }: { rows: FormattedAction[] }) {
  const scrollRef = useRef<HTMLDivElement>(null);

  const rowVirtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => PREVIEW_ROW_HEIGHT,
    overscan: 12,
    observeElementRect: (instance, cb) => {
      const element = instance.scrollElement;
      if (!element) {
        return;
      }
      const height =
        element.clientHeight > 0
          ? element.clientHeight
          : PREVIEW_TABLE_MAX_HEIGHT;
      const width = element.clientWidth > 0 ? element.clientWidth : 800;
      cb({
        width,
        height,
        top: 0,
        left: 0,
        right: width,
        bottom: height,
      } as DOMRect);
    },
  });

  const virtualRows = rowVirtualizer.getVirtualItems();
  const paddingTop = virtualRows.length > 0 ? virtualRows[0].start : 0;
  const paddingBottom =
    virtualRows.length > 0
      ? rowVirtualizer.getTotalSize() - virtualRows[virtualRows.length - 1].end
      : 0;

  return (
    <PreviewTableShell scrollRef={scrollRef}>
      {paddingTop > 0 && (
        <tr aria-hidden="true">
          <td
            colSpan={3}
            style={{ height: paddingTop, padding: 0, border: 0 }}
          />
        </tr>
      )}
      {virtualRows.map((virtualRow) => {
        const row = rows[virtualRow.index];
        return (
          <tr
            key={`${row.label}-${row.path}-${virtualRow.index}`}
            className={`tone-${row.tone}`}
            data-index={virtualRow.index}
          >
            <td>{row.label}</td>
            <td className="preview-path">{row.path}</td>
            <td className="preview-detail">{row.detail || "—"}</td>
          </tr>
        );
      })}
      {paddingBottom > 0 && (
        <tr aria-hidden="true">
          <td
            colSpan={3}
            style={{ height: paddingBottom, padding: 0, border: 0 }}
          />
        </tr>
      )}
    </PreviewTableShell>
  );
}

function PreviewTableShell({
  scrollRef,
  children,
}: {
  scrollRef?: RefObject<HTMLDivElement | null>;
  children: ReactNode;
}) {
  return (
    <div
      ref={scrollRef}
      className="preview-table-wrap preview-table-virtual"
      style={{ maxHeight: PREVIEW_TABLE_MAX_HEIGHT }}
    >
      <table className="preview-table">
        <thead>
          <tr>
            <th scope="col">Action</th>
            <th scope="col">Path</th>
            <th scope="col">Details</th>
          </tr>
        </thead>
        <tbody>{children}</tbody>
      </table>
    </div>
  );
}

function PreviewRowsBody({ rows }: { rows: FormattedAction[] }) {
  if (rows.length <= VIRTUALIZE_THRESHOLD) {
    return (
      <PreviewTableShell>
        <PreviewRowsTable rows={rows} />
      </PreviewTableShell>
    );
  }
  return <VirtualizedPreviewRows rows={rows} />;
}

export function PreviewTable({ plan, loading, error }: PreviewTableProps) {
  const rows = useMemo(
    () => (plan ? plan.actions.map((action) => formatAction(action)) : []),
    [plan],
  );

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
        <p className="preview-empty">
          No changes needed — folders are in sync.
        </p>
      ) : (
        <PreviewRowsBody rows={rows} />
      )}
    </section>
  );
}
