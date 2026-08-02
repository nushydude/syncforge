import type { RunItem, RunReport } from "../types";

export interface SyncStats {
  filesCopied: number;
  filesDeleted: number;
  filesChanged: number;
  bytesTransferred: number;
  durationMs: number | null;
  errorCount: number;
}

export function statsFromReport(report: RunReport): SyncStats {
  const durationMs =
    report.finishedAt != null && report.finishedAt >= report.startedAt
      ? report.finishedAt - report.startedAt
      : null;

  return {
    filesCopied: report.filesCopied,
    filesDeleted: report.filesDeleted,
    filesChanged: report.filesCopied + report.filesDeleted,
    bytesTransferred: report.bytesTransferred,
    durationMs,
    errorCount: report.errors.length,
  };
}

/** Sum bytes from per-item records (falls back to report totals when no items). */
export function statsFromItems(report: RunReport, items: RunItem[]): SyncStats {
  const base = statsFromReport(report);
  if (items.length === 0) {
    return base;
  }

  const bytesFromItems = items
    .filter((item) => item.status === "completed")
    .reduce((sum, item) => sum + (item.bytes ?? 0), 0);
  const completedItems = items.filter((item) => item.status === "completed");

  return {
    ...base,
    filesChanged: completedItems.length,
    bytesTransferred:
      bytesFromItems > 0 ? bytesFromItems : base.bytesTransferred,
  };
}

export function formatDuration(durationMs: number | null): string {
  if (durationMs == null || durationMs < 0) {
    return "—";
  }
  if (durationMs < 1000) {
    return `${durationMs} ms`;
  }
  const seconds = Math.round(durationMs / 1000);
  if (seconds < 60) {
    return `${seconds}s`;
  }
  const minutes = Math.floor(seconds / 60);
  const rem = seconds % 60;
  return rem > 0 ? `${minutes}m ${rem}s` : `${minutes}m`;
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) {
    return `${bytes} B`;
  }
  if (bytes < 1024 * 1024) {
    return `${(bytes / 1024).toFixed(1)} KiB`;
  }
  if (bytes < 1024 * 1024 * 1024) {
    return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
  }
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GiB`;
}
