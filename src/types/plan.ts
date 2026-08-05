export interface FileEntry {
  relativePath: string;
  size: number;
  modifiedSecs: number;
  /** Subsecond fraction of modifiedSecs (0–999_999_999). Omitted in older snapshots. */
  modifiedNanos?: number;
  isDir: boolean;
  hash?: string;
  deleted?: boolean;
}

export type SyncAction =
  | { kind: "copyLeftToRight"; path: string }
  | { kind: "copyRightToLeft"; path: string }
  | { kind: "deleteLeft"; path: string }
  | { kind: "deleteRight"; path: string }
  | { kind: "createDirLeft"; path: string }
  | { kind: "createDirRight"; path: string }
  | {
      kind: "conflict";
      path: string;
      left: FileEntry;
      right: FileEntry;
    }
  | { kind: "skip"; path: string; reason: string };

export interface SyncPlan {
  pairId: string;
  actions: SyncAction[];
  scannedLeft: number;
  scannedRight: number;
  scanSkippedLeft?: number;
  scanSkippedRight?: number;
  scanWarnings?: string[];
  requiresAttention?: boolean;
}

export interface PreviewSummary extends SyncPlan {
  planId?: string;
  configFingerprint?: string;
  createdAt?: number;
  actionCounts?: Record<string, number>;
  actionCount?: number;
  conflictCount?: number;
  nextCursor?: number;
}

export interface PreviewActionPage {
  planId: string;
  cursor: number;
  nextCursor?: number;
  actions: SyncAction[];
}
