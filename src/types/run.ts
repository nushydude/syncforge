import type { ConflictChoice } from './pair';
import type { RunReport } from './history';

export type SyncProgressPhase =
  | 'scanning'
  | 'running'
  | 'completed'
  | 'failed'
  | 'cancelled';

export interface SyncProgress {
  runId: string;
  pairId: string;
  phase: SyncProgressPhase | string;
  current: number;
  total: number;
  path?: string;
  message?: string;
  report?: RunReport;
}

export interface RunPairOptions {
  verifyHashes?: boolean;
  useRecycleBin?: boolean;
  conflictResolutions?: Record<string, ConflictChoice>;
  /** When true (default), the first non-conflict action failure stops the run. */
  stopOnError?: boolean;
}

/** Emitted when a debounced watch run is skipped (conflicts, errors). */
export interface WatchSkippedNotice {
  pairId: string;
  reason: string;
}
