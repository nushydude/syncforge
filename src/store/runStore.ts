import { listen } from '@tauri-apps/api/event';
import * as runApi from '../api/run';
import type { FolderPair, RunReport, SyncProgress } from '../types';
import { defaultAppSettings } from '../types';

export interface RunStoreState {
  running: boolean;
  progress: SyncProgress | null;
  lastReport: RunReport | null;
  error: string | null;
}

type Listener = () => void;

let state: RunStoreState = {
  running: false,
  progress: null,
  lastReport: null,
  error: null,
};

const listeners = new Set<Listener>();
let unlistenProgress: (() => void) | null = null;

function emit() {
  listeners.forEach((l) => l());
}

export function getRunState(): RunStoreState {
  return state;
}

export function subscribeRun(listener: Listener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

async function ensureProgressListener(): Promise<void> {
  if (unlistenProgress) {
    return;
  }
  unlistenProgress = await listen<SyncProgress>('sync://progress', (event) => {
    const progress = event.payload;
    state = {
      ...state,
      progress,
      lastReport: progress.report ?? state.lastReport,
    };
    emit();
  });
}

export async function runSelectedPair(pair: FolderPair): Promise<RunReport | null> {
  if (!pair.id || state.running) {
    return null;
  }

  const settings = defaultAppSettings();
  state = {
    ...state,
    running: true,
    progress: null,
    lastReport: null,
    error: null,
  };
  emit();

  try {
    await ensureProgressListener();
    const report = await runApi.runPair(pair, {
      verifyHashes: settings.verifyHashesAfterCopy,
      useRecycleBin: settings.moveDeletesToRecycleBin,
    });
    state = {
      ...state,
      running: false,
      lastReport: report,
      progress: state.progress
        ? { ...state.progress, report, phase: report.status }
        : null,
    };
    emit();
    return report;
  } catch (e) {
    state = {
      ...state,
      running: false,
      error: e instanceof Error ? e.message : String(e),
    };
    emit();
    return null;
  }
}

export async function cancelActiveRun(): Promise<void> {
  if (!state.running) {
    return;
  }
  try {
    await runApi.cancelRun();
  } catch (e) {
    state = {
      ...state,
      error: e instanceof Error ? e.message : String(e),
    };
    emit();
  }
}

/** Test helper — reset module state between Vitest cases. */
export function resetRunStoreForTests(): void {
  state = {
    running: false,
    progress: null,
    lastReport: null,
    error: null,
  };
  emit();
}
