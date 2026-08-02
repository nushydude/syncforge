export interface FolderSizeEntry {
  name: string;
  path: string;
  size: number;
  isFolder: boolean;
  childCount: number;
}

export interface FolderSizeResult {
  path: string;
  size: number;
  files: number;
  folders: number;
  skipped: number;
  entries: FolderSizeEntry[];
}
