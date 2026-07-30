export type DuplicateMatchMode = "filename" | "size" | "hash";

export type DuplicateScanStatus =
  | "running"
  | "completed"
  | "cancelled"
  | "interrupted"
  | "failed";

export type DuplicateScanPhase = "collecting" | "hashing" | "finalizing";

export interface DuplicateFile {
  relativePath: string;
  name: string;
  size: number;
  hash?: string;
}

export interface DuplicateGroup {
  key: string;
  files: DuplicateFile[];
  potentialSavingsBytes: number;
}

export interface DuplicateScanResult {
  root: string;
  mode: DuplicateMatchMode;
  scannedFiles: number;
  hashedFiles: number;
  skippedFiles: number;
  potentialSavingsBytes: number;
  groups: DuplicateGroup[];
  warnings: string[];
}

export interface DuplicateScanJob {
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
  result: DuplicateScanResult | null;
  error: string | null;
  startedAt: number;
  updatedAt: number;
}

export interface DuplicateCleanupResult {
  removed: string[];
  errors: string[];
}
