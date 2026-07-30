import type { ConflictPolicy } from './pair';

export interface AppSettings {
  defaultConflictPolicy: ConflictPolicy;
  confirmBeforeRun: boolean;
  moveDeletesToRecycleBin: boolean;
  verifyHashesAfterCopy: boolean;
  theme: 'system' | 'light' | 'dark';
}

export const defaultAppSettings = (): AppSettings => ({
  defaultConflictPolicy: 'newerWins',
  confirmBeforeRun: true,
  moveDeletesToRecycleBin: true,
  verifyHashesAfterCopy: false,
  theme: 'system',
});

export const APP_SETTINGS_STORAGE_KEY = 'syncforge.appSettings';
