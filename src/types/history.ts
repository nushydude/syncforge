export type RunStatus = 'running' | 'completed' | 'failed' | 'cancelled' | 'interrupted';

export interface RunReport {
  runId: string;
  pairId: string;
  startedAt: number;
  finishedAt?: number;
  status: RunStatus;
  filesCopied: number;
  filesDeleted: number;
  bytesTransferred: number;
  errors: string[];
}

export interface RunItem {
  id: string;
  runId: string;
  path: string;
  action: string;
  status: string;
  message?: string;
  bytes?: number;
}

export interface RunDetail {
  report: RunReport;
  items: RunItem[];
}

export interface Snapshot {
  id: string;
  pairId: string;
  capturedAt: number;
  entries: import('./plan').FileEntry[];
}
