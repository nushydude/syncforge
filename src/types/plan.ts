export interface FileEntry {
  relativePath: string;
  size: number;
  modifiedSecs: number;
  isDir: boolean;
  hash?: string;
}

export type SyncAction =
  | { kind: 'copyLeftToRight'; path: string }
  | { kind: 'copyRightToLeft'; path: string }
  | { kind: 'deleteLeft'; path: string }
  | { kind: 'deleteRight'; path: string }
  | { kind: 'createDirLeft'; path: string }
  | { kind: 'createDirRight'; path: string }
  | {
      kind: 'conflict';
      path: string;
      left: FileEntry;
      right: FileEntry;
    }
  | { kind: 'skip'; path: string; reason: string };

export interface SyncPlan {
  pairId: string;
  actions: SyncAction[];
  scannedLeft: number;
  scannedRight: number;
  scanWarnings?: string[];
}
