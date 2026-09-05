export type SnifferStatus =
  | "queued"
  | "scanning"
  | "cancelling"
  | "completed"
  | "cancelled"
  | "failed";

export interface SnifferError {
  code: string;
  operation: string;
  retryable: boolean;
  message: string;
}

export interface SnifferScan {
  id: string;
  generationId: string;
  root: string;
  rootNodeId: string | null;
  status: SnifferStatus;
  revision: number;
  filesVisited: string;
  foldersVisited: string;
  logicalBytes: string;
  issueCount: string;
  coverageComplete: boolean;
  stale: boolean;
  currentDirectory: string | null;
  startedAt: number;
  finishedAt: number | null;
  error: SnifferError | null;
}

export interface SnifferEntry {
  nodeId: string;
  parentId: string | null;
  name: string;
  fullPath: string;
  relativePath: string;
  kind: "file" | "directory" | "link" | "unsupported";
  logicalSize: string;
  files: string | null;
  folders: string | null;
  modifiedAt: number | null;
  status: string;
}

export interface SnifferQuery {
  scanId: string;
  generationId: string;
  directoryId: string;
  scope: "children" | "subtreeFiles";
  sortBy: "name" | "size" | "files" | "modified";
  sortDirection: "asc" | "desc";
  search: string;
  extension?: string;
  minSize?: string;
  maxSize?: string;
  modifiedFrom?: number;
  modifiedTo?: number;
  itemKind?: string;
  cursor?: string;
  limit: number;
}

export interface SnifferEntryPage {
  rows: SnifferEntry[];
  nextCursor: string | null;
  matchCount: string;
  matchedBytes: string;
  directoryBytes: string;
  revision: number;
  coverageComplete: boolean;
  stale: boolean;
}

export interface SnifferMapTile {
  kind: "entry" | "other" | "looseFiles";
  nodeId: string | null;
  name: string;
  logicalSize: string;
  itemCount: string;
}

export interface SnifferSummary {
  directory: SnifferEntry;
  logicalBytes: string;
  files: string;
  folders: string;
  zeroSizeCount: string;
  tiles: SnifferMapTile[];
  revision: number;
  coverageComplete: boolean;
  stale: boolean;
}

export interface SnifferActionReview {
  token: string;
  action: "rename" | "recycle";
  fullPath: string;
  kind: string;
  newPath: string | null;
  expiresAt: number;
}

export interface SnifferIssuePage {
  rows: Array<{
    id: string;
    nodeId: string | null;
    category: string;
    path: string;
    code: string | null;
    message: string;
  }>;
  nextCursor: string | null;
  total: string;
  omittedDetails: string;
}
