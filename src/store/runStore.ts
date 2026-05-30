import { listen } from '@tauri-apps/api/event';
import * as previewApi from '../api/preview';
import * as runApi from '../api/run';
import { listConflictActions, type ConflictAction } from '../lib/conflictPolicy';
import { isPreviewLoading } from './pairsStore';
import type {
  ConflictChoice,
  FolderPair,
  RunReport,
  SyncProgress,
  WatchSkippedNotice,
} from '../types';
import { defaultAppSettings } from '../types';

export interface PendingConflicts {
  pair: FolderPair;
  conflicts: ConflictAction[];
}

export interface RunStoreState {
  running: boolean;
  runningPairId: string | null;
  progress: SyncProgress | null;
  lastReport: RunReport | null;
  error: string | null;
  pendingConflicts: PendingConflicts | null;
  conflictResolutions: Record<string, ConflictChoice>;
  watchSkipped: WatchSkippedNotice | null;
}

type Listener = () => void;

let state: RunStoreState = {
  running: false,
  runningPairId: null,
  progress: null,
  lastReport: null,
  error: null,
  pendingConflicts: null,
  conflictResolutions: {},
  watchSkipped: null,
};

const listeners = new Set<Listener>();
let unlistenProgress: (() => void) | null = null;
let unlistenWatchSkipped: (() => void) | null = null;

/** UI-initiated run ownership — progress events must match these to update state. */
let activeRunId: string | null = null;
let activePairId: string | null = null;
let runInFlight = false;

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
    if (!state.running || !activePairId) {
      return;
    }
    if (progress.pairId !== activePairId) {
      return;
    }
    if (activeRunId === null) {
      activeRunId = progress.runId;
    } else if (progress.runId !== activeRunId) {
      return;
    }
    state = {
      ...state,
      progress,
      lastReport: progress.report ?? state.lastReport,
    };
    emit();
  });
}

/** Subscribes to backend watch-auto-sync skip events (conflicts, errors). */
export async function ensureWatchSkippedListener(): Promise<void> {
  if (unlistenWatchSkipped) {
    return;
  }
  unlistenWatchSkipped = await listen<WatchSkippedNotice>(
    'sync://watch-skipped',
    (event) => {
      state = { ...state, watchSkipped: event.payload };
      emit();
    },
  );
}

export function dismissWatchSkipped(): void {
  state = { ...state, watchSkipped: null };
  emit();
}

function clearActiveRunOwnership(): void {
  activeRunId = null;
  activePairId = null;
}

async function executeRun(
  pair: FolderPair,
  conflictResolutions: Record<string, ConflictChoice>,
): Promise<RunReport | null> {
  const settings = defaultAppSettings();
  activeRunId = null;
  activePairId = pair.id;
  state = {
    ...state,
    running: true,
    runningPairId: pair.id,
    progress: null,
    lastReport: null,
    error: null,
    pendingConflicts: null,
    conflictResolutions: {},
  };
  emit();

  try {
    await ensureProgressListener();
    const report = await runApi.runPair(pair, {
      verifyHashes: settings.verifyHashesAfterCopy,
      useRecycleBin: settings.moveDeletesToRecycleBin,
      conflictResolutions,
    });
    clearActiveRunOwnership();
    state = {
      ...state,
      running: false,
      runningPairId: null,
      lastReport: report,
      progress: state.progress
        ? { ...state.progress, report, phase: report.status }
        : null,
    };
    emit();
    return report;
  } catch (e) {
    clearActiveRunOwnership();
    state = {
      ...state,
      running: false,
      runningPairId: null,
      error: e instanceof Error ? e.message : String(e),
    };
    emit();
    return null;
  }
}

export async function runSelectedPair(pair: FolderPair): Promise<RunReport | null> {
  if (!pair.id || state.running || runInFlight || isPreviewLoading()) {
    return null;
  }

  runInFlight = true;
  try {
    if (pair.conflictPolicy === 'ask') {
      try {
        const plan = await previewApi.previewPair(pair);
        const conflicts = listConflictActions(plan);
        if (conflicts.length > 0) {
          state = {
            ...state,
            pendingConflicts: { pair, conflicts },
            conflictResolutions: {},
            error: null,
          };
          emit();
          return null;
        }
      } catch (e) {
        state = {
          ...state,
          error: e instanceof Error ? e.message : String(e),
        };
        emit();
        return null;
      }
    }

    return await executeRun(pair, {});
  } finally {
    runInFlight = false;
  }
}

export function setConflictResolution(
  path: string,
  choice: ConflictChoice,
): void {
  state = {
    ...state,
    conflictResolutions: { ...state.conflictResolutions, [path]: choice },
  };
  emit();
}

export function cancelConflictResolution(): void {
  state = {
    ...state,
    pendingConflicts: null,
    conflictResolutions: {},
  };
  emit();
}

export async function confirmConflictResolutionAndRun(): Promise<RunReport | null> {
  const pending = state.pendingConflicts;
  if (!pending) {
    return null;
  }
  return executeRun(pending.pair, state.conflictResolutions);
}

export async function cancelActiveRun(): Promise<void> {
  if (!state.running || !state.runningPairId) {
    return;
  }
  const pairId = state.runningPairId;
  state = {
    ...state,
    running: false,
    runningPairId: null,
    progress: null,
    error: null,
  };
  clearActiveRunOwnership();
  emit();

  try {
    await runApi.cancelRun(pairId);
  } catch (e) {
    activePairId = pairId;
    state = {
      ...state,
      running: true,
      runningPairId: pairId,
      error: e instanceof Error ? e.message : String(e),
    };
    emit();
  }
}

/** Test helper — reset module state between Vitest cases. */
export function resetRunStoreForTests(): void {
  unlistenProgress = null;
  unlistenWatchSkipped = null;
  activeRunId = null;
  activePairId = null;
  runInFlight = false;
  state = {
    running: false,
    runningPairId: null,
    progress: null,
    lastReport: null,
    error: null,
    pendingConflicts: null,
    conflictResolutions: {},
    watchSkipped: null,
  };
  emit();
}
