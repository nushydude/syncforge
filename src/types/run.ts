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
}
