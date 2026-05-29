export type SyncMode = 'synchronize' | 'echo' | 'contribute';

export type ConflictPolicy =
  | 'newerWins'
  | 'left'
  | 'right'
  | 'keepBoth'
  | 'ask';

export interface Filters {
  include: string[];
  exclude: string[];
}

export interface FolderPair {
  id: string;
  name: string;
  leftPath: string;
  rightPath: string;
  mode: SyncMode;
  filters: Filters;
  conflictPolicy: ConflictPolicy;
  enabled: boolean;
  createdAt: number;
  updatedAt: number;
}

export const defaultFilters = (): Filters => ({
  include: [],
  exclude: [],
});
