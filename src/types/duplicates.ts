export type DuplicateMatchMode = "filename" | "size" | "hash";

export type DuplicateScanStatus =
  | "running"
  | "completed"
  | "cancelled"
  | "interrupted"
  | "failed";

export type DuplicateScanPhase = "collecting" | "hashing" | "finalizing";

export interface DuplicateFile {
  id?: string;
  fingerprint?: string;
  relativePath: string;
  name: string;
  size: number;
  hash?: string;
}

export interface DuplicateResultSummary {
  revision: string;
  cleanupAvailable: boolean;
  root: string;
  mode: DuplicateMatchMode;
  scannedFiles: number;
  hashedFiles: number;
  skippedFiles: number;
  potentialSavingsBytes: number;
  groupCount: number;
  warnings: string[];
}
export interface DuplicateGroupSummary {
  id: string;
  key: string;
  fileCount: number;
  potentialSavingsBytes: number;
  firstSize: number;
}
export interface DuplicateGroupsPage {
  revision: string;
  groups: DuplicateGroupSummary[];
  nextCursor: number | null;
}
export interface DuplicateMembersPage {
  revision: string;
  groupId: string;
  files: DuplicateFile[];
  nextCursor: number | null;
}

export interface DuplicateScanJob {
  attempt: number;
  sequence: number;
  summary: DuplicateResultSummary | null;
  id: string;
  root: string;
  mode: DuplicateMatchMode;
  status: DuplicateScanStatus;
  phase: DuplicateScanPhase | null;
  filesFound: number;
  totalFiles: number | null;
  hashedFiles: number;
  hashTotal: number | null;
  bytesProcessed: number;
  bytesTotal: number | null;
  currentPath: string | null;
  cancelRequested: boolean;
  error: string | null;
  startedAt: number;
  updatedAt: number;
}
