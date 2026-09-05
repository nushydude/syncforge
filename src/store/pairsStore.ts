import { listen } from "@tauri-apps/api/event";
import * as pairsApi from "../api/pairs";
import * as previewApi from "../api/preview";
import { validatePairForm } from "../lib/pairValidation";
import {
  validateCronExpression,
  describeCronExpression,
} from "../lib/scheduleParsing";
import type {
  ConflictPolicy,
  FolderPair,
  PreviewSummary,
  SyncMode,
} from "../types";
import { defaultFilters } from "../types";
import { getAppSettings } from "./settingsStore";

/** Scan/preview result for a single pair — kept per pair so views stay independent. */
export interface PairPreviewState {
  plan: PreviewSummary | null;
  /** Waiting in the shared work queue, not scanning yet. */
  queued: boolean;
  loading: boolean;
  error: string | null;
  /** Epoch ms the scan started — kept in the store so the timer survives
   * navigating away and back, which would reset component-local state. */
  startedAt: number | null;
  /** Live heartbeat from the backend walk. */
  scannedEntries: number;
  scanSide: string | null;
  scanPath: string | null;
}

export interface PairsStoreState {
  pairs: FolderPair[];
  /** Most recent completed sync timestamp per pair, loaded from history. */
  lastSyncedAtByPair: Record<string, number>;
  selectedId: string | null;
  editing: FolderPair | null;
  editorOpen: boolean;
  loading: boolean;
  saving: boolean;
  error: string | null;
  validationErrors: string[];
  /** Keyed by pair id; a pair's preview survives switching to another pair. */
  previews: Record<string, PairPreviewState>;
  watchWarning: string | null;
  scheduleError: string | null;
  scheduleDescription: string | null;
}

/** Stable identity so selectors comparing snapshots stay referentially equal. */
export const emptyPairPreview: PairPreviewState = {
  plan: null,
  queued: false,
  loading: false,
  error: null,
  startedAt: null,
  scannedEntries: 0,
  scanSide: null,
  scanPath: null,
};

type Listener = () => void;

const defaultConflictPolicy: ConflictPolicy = "newerWins";
const defaultMode: SyncMode = "synchronize";

function emptyPair(): FolderPair {
  const settings = getAppSettings();
  return {
    id: "",
    name: "",
    leftPath: "",
    rightPath: "",
    mode: defaultMode,
    filters: defaultFilters(),
    conflictPolicy: settings.defaultConflictPolicy ?? defaultConflictPolicy,
    enabled: true,
    watchEnabled: false,
    scheduleEnabled: false,
    scheduleCron: null,
    createdAt: 0,
    updatedAt: 0,
  };
}

function emptyState(): PairsStoreState {
  return {
    pairs: [],
    lastSyncedAtByPair: {},
    selectedId: null,
    editing: null,
    editorOpen: false,
    loading: false,
    saving: false,
    error: null,
    validationErrors: [],
    previews: {},
    watchWarning: null,
    scheduleError: null,
    scheduleDescription: null,
  };
}

let state: PairsStoreState = emptyState();

const WATCH_INACTIVE_MSG =
  "Auto-sync watch is saved but inactive until both folder paths exist on disk.";

const listeners = new Set<Listener>();

/** Monotonic token per pair — stale preview responses are ignored when it changes. */
const previewRequestIds = new Map<string, number>();

function bumpPreviewRequest(pairId: string): number {
  const next = (previewRequestIds.get(pairId) ?? 0) + 1;
  previewRequestIds.set(pairId, next);
  return next;
}

function setPreview(pairId: string, patch: Partial<PairPreviewState>): void {
  const current = state.previews[pairId] ?? emptyPairPreview;
  state = {
    ...state,
    previews: { ...state.previews, [pairId]: { ...current, ...patch } },
  };
}

const PATH_EXISTS_DEBOUNCE_MS = 300;
let watchWarningTimer: ReturnType<typeof setTimeout> | null = null;
let watchWarningRequestId = 0;

function emit() {
  listeners.forEach((l) => l());
}

export function getPairPreview(
  pairId: string | null | undefined,
): PairPreviewState {
  if (!pairId) return emptyPairPreview;
  return state.previews[pairId] ?? emptyPairPreview;
}

export function isPreviewLoadingForPair(
  pairId: string | null | undefined,
): boolean {
  return getPairPreview(pairId).loading;
}

/** Surfaces a preview-related failure against the pair it belongs to. */
export function setPreviewError(pairId: string, message: string): void {
  setPreview(pairId, { error: message });
  emit();
}

/** Drops a pair's cached plan — used once a run consumes the backend plan. */
export function clearPairPreview(pairId: string): void {
  if (!state.previews[pairId]) {
    return;
  }
  bumpPreviewRequest(pairId);
  const previews = { ...state.previews };
  delete previews[pairId];
  state = { ...state, previews };
  emit();
}

export function getPairsState(): PairsStoreState {
  return state;
}

export function subscribePairs(listener: Listener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

async function refreshScheduleDescription(
  pair: FolderPair | null,
): Promise<void> {
  if (!pair?.scheduleEnabled || !pair.scheduleCron?.trim()) {
    state = { ...state, scheduleDescription: null, scheduleError: null };
    emit();
    return;
  }

  const validation = validateCronExpression(pair.scheduleCron);
  state = {
    ...state,
    scheduleError: validation.valid
      ? null
      : (validation.error ?? "Invalid cron expression"),
    scheduleDescription: validation.valid
      ? describeCronExpression(pair.scheduleCron)
      : null,
  };
  emit();
}

function refreshWatchWarning(pair: FolderPair | null): void {
  if (watchWarningTimer) {
    clearTimeout(watchWarningTimer);
    watchWarningTimer = null;
  }

  if (!pair?.watchEnabled || !pair.enabled) {
    watchWarningRequestId++;
    state = { ...state, watchWarning: null };
    emit();
    return;
  }

  const left = pair.leftPath.trim();
  const right = pair.rightPath.trim();
  if (!left || !right) {
    watchWarningRequestId++;
    state = { ...state, watchWarning: WATCH_INACTIVE_MSG };
    emit();
    return;
  }

  const requestId = ++watchWarningRequestId;
  watchWarningTimer = setTimeout(() => {
    watchWarningTimer = null;
    void (async () => {
      if (requestId !== watchWarningRequestId) {
        return;
      }
      const [leftExists, rightExists] = await Promise.all([
        pairsApi.pathExists(left),
        pairsApi.pathExists(right),
      ]);
      if (requestId !== watchWarningRequestId) {
        return;
      }
      state = {
        ...state,
        watchWarning: leftExists && rightExists ? null : WATCH_INACTIVE_MSG,
      };
      emit();
    })();
  }, PATH_EXISTS_DEBOUNCE_MS);
}

export async function loadPairs(): Promise<void> {
  state = { ...state, loading: true, error: null };
  emit();
  try {
    const [pairsResult, lastSyncedResult] = await Promise.allSettled([
      pairsApi.listPairs(),
      pairsApi.getLastSyncedAtByPair(),
    ]);
    if (pairsResult.status === "rejected") {
      throw pairsResult.reason;
    }
    const pairs = pairsResult.value;
    state = {
      ...state,
      pairs,
      lastSyncedAtByPair:
        lastSyncedResult.status === "fulfilled" ? lastSyncedResult.value : {},
      loading: false,
      selectedId:
        state.selectedId && pairs.some((p) => p.id === state.selectedId)
          ? state.selectedId
          : null,
    };
  } catch (e) {
    state = {
      ...state,
      loading: false,
      error: e instanceof Error ? e.message : String(e),
    };
  }
  emit();
}

export function setLastSyncedAt(pairId: string, timestamp: number): void {
  if (state.lastSyncedAtByPair[pairId] === timestamp) {
    return;
  }
  state = {
    ...state,
    lastSyncedAtByPair: {
      ...state.lastSyncedAtByPair,
      [pairId]: timestamp,
    },
  };
  emit();
}

export function selectPair(id: string): void {
  const pair = state.pairs.find((p) => p.id === id);
  if (!pair) {
    return;
  }
  state = {
    ...state,
    selectedId: id,
    editing: { ...pair, filters: { ...pair.filters } },
    editorOpen: false,
    validationErrors: [],
    error: null,
    watchWarning: null,
    scheduleError: null,
    scheduleDescription: null,
  };
  emit();
  refreshWatchWarning(state.editing);
  void refreshScheduleDescription(state.editing);
}

export function startNewPair(): void {
  state = {
    ...state,
    selectedId: null,
    editing: emptyPair(),
    editorOpen: true,
    validationErrors: [],
    error: null,
    watchWarning: null,
    scheduleError: null,
    scheduleDescription: null,
  };
  emit();
}

export function beginEdit(): void {
  const pair = state.pairs.find((p) => p.id === state.selectedId);
  if (!pair) {
    return;
  }
  state = {
    ...state,
    editing: { ...pair, filters: { ...pair.filters } },
    editorOpen: true,
    validationErrors: [],
    error: null,
  };
  emit();
  refreshWatchWarning(state.editing);
  void refreshScheduleDescription(state.editing);
}

export function cancelEdit(): void {
  state = {
    ...state,
    editing: null,
    editorOpen: false,
    validationErrors: [],
    error: null,
    watchWarning: null,
    scheduleError: null,
    scheduleDescription: null,
  };
  emit();
}

/** Marks a pair as waiting in the shared work queue for its turn to scan. */
export function markPreviewQueued(pairId: string): void {
  bumpPreviewRequest(pairId);
  setPreview(pairId, {
    ...emptyPairPreview,
    queued: true,
  });
  emit();
}

let unlistenScanProgress: (() => void) | null = null;

/** Subscribes to backend scan heartbeats so a long walk shows real progress. */
export async function ensureScanProgressListener(): Promise<void> {
  if (unlistenScanProgress) {
    return;
  }
  unlistenScanProgress = await listen<{
    pairId: string;
    side: string;
    entries: number;
    path: string;
  }>("preview://scan-progress", (event) => {
    const { pairId, side, entries, path } = event.payload;
    if (!state.previews[pairId]?.loading) {
      return;
    }
    setPreview(pairId, {
      scannedEntries: entries,
      scanSide: side,
      scanPath: path || null,
    });
    emit();
  });
}

/**
 * Scans one pair; the result is stored against that pair id only. Callers go
 * through the shared work queue rather than invoking this directly, so that only
 * one scan walks the disk at a time.
 */
export async function previewPairById(pair: FolderPair): Promise<void> {
  if (!pair.id) {
    return;
  }
  const pairId = pair.id;
  const requestId = bumpPreviewRequest(pairId);
  setPreview(pairId, {
    ...emptyPairPreview,
    loading: true,
    startedAt: Date.now(),
  });
  emit();

  try {
    await ensureScanProgressListener();
    const plan = await previewApi.previewPair(pair);
    if (requestId !== previewRequestIds.get(pairId)) {
      return;
    }
    setPreview(pairId, { plan, loading: false, scanPath: null });
  } catch (e) {
    if (requestId !== previewRequestIds.get(pairId)) {
      return;
    }
    const message = e instanceof Error ? e.message : String(e);
    // A cancelled scan is a user action, not a failure to report.
    setPreview(pairId, {
      loading: false,
      error: isCancelledScan(message) ? null : message,
    });
  }
  emit();
}

function isCancelledScan(message: string): boolean {
  return message.includes("cancelled");
}

export function updateEditing(patch: Partial<FolderPair>): void {
  if (!state.editing) {
    return;
  }
  const editing = { ...state.editing, ...patch };
  state = {
    ...state,
    editing,
    validationErrors: [],
  };
  emit();
  if (
    "watchEnabled" in patch ||
    "enabled" in patch ||
    "leftPath" in patch ||
    "rightPath" in patch
  ) {
    refreshWatchWarning(editing);
  }
  if ("scheduleEnabled" in patch || "scheduleCron" in patch) {
    void refreshScheduleDescription(editing);
  }
}

export async function pickFolderForSide(
  side: "leftPath" | "rightPath",
): Promise<void> {
  if (!state.editing) {
    return;
  }
  try {
    const path = await pairsApi.pickFolder();
    if (path) {
      updateEditing({ [side]: path });
    }
  } catch (e) {
    state = {
      ...state,
      error: e instanceof Error ? e.message : String(e),
    };
    emit();
  }
}

async function validateEditing(): Promise<boolean> {
  if (!state.editing) {
    return false;
  }
  const { name, leftPath, rightPath } = state.editing;
  const [leftExists, rightExists, pathsEqual] = await Promise.all([
    leftPath.trim() ? pairsApi.pathExists(leftPath) : Promise.resolve(false),
    rightPath.trim() ? pairsApi.pathExists(rightPath) : Promise.resolve(false),
    leftPath.trim() && rightPath.trim()
      ? pairsApi.pathsEqual(leftPath, rightPath)
      : Promise.resolve(false),
  ]);
  const validationErrors = validatePairForm(
    { name, leftPath, rightPath },
    { leftExists, rightExists, pathsEqual },
  );

  if (state.editing.scheduleEnabled) {
    const cron = state.editing.scheduleCron?.trim() ?? "";
    const cronValidation = validateCronExpression(cron);
    if (!cronValidation.valid) {
      validationErrors.push(
        cronValidation.error ?? "Invalid schedule cron expression",
      );
    }
  }

  state = { ...state, validationErrors };
  emit();
  return validationErrors.length === 0;
}

export async function saveEditing(): Promise<boolean> {
  const editing = state.editing;
  if (!editing) {
    return false;
  }
  state = { ...state, saving: true, error: null };
  emit();

  const valid = await validateEditing();
  if (!valid) {
    state = { ...state, saving: false };
    emit();
    return false;
  }

  try {
    const saved = await pairsApi.savePair(editing);
    const scheduled = await pairsApi.setSchedule(
      saved.id,
      editing.scheduleEnabled,
      editing.scheduleEnabled ? (editing.scheduleCron?.trim() ?? null) : null,
    );
    const exists = state.pairs.some((p) => p.id === scheduled.id);
    const pairs = exists
      ? state.pairs.map((p) => (p.id === scheduled.id ? scheduled : p))
      : [...state.pairs, scheduled];
    state = {
      ...state,
      pairs,
      selectedId: scheduled.id,
      editing: { ...scheduled, filters: { ...scheduled.filters } },
      editorOpen: false,
      saving: false,
      validationErrors: [],
    };
    // Config changes invalidate the stored plan's fingerprint on the backend.
    clearPairPreview(scheduled.id);
    emit();
    refreshWatchWarning(state.editing);
    await refreshScheduleDescription(state.editing);
    return true;
  } catch (e) {
    state = {
      ...state,
      saving: false,
      error: e instanceof Error ? e.message : String(e),
    };
    emit();
    return false;
  }
}

export async function deleteSelected(): Promise<boolean> {
  const id = state.selectedId ?? state.editing?.id;
  if (!id) {
    return false;
  }
  state = { ...state, saving: true, error: null };
  emit();
  try {
    await pairsApi.deletePair(id);
    const pairs = state.pairs.filter((p) => p.id !== id);
    const previews = { ...state.previews };
    delete previews[id];
    previewRequestIds.delete(id);
    state = {
      ...state,
      pairs,
      previews,
      selectedId: null,
      editing: null,
      editorOpen: false,
      saving: false,
      validationErrors: [],
    };
    emit();
    return true;
  } catch (e) {
    state = {
      ...state,
      saving: false,
      error: e instanceof Error ? e.message : String(e),
    };
    emit();
    return false;
  }
}

/** Test helper — reset module state between Vitest cases. */
export function resetPairsStoreForTests(): void {
  previewRequestIds.clear();
  watchWarningRequestId = 0;
  if (watchWarningTimer) {
    clearTimeout(watchWarningTimer);
    watchWarningTimer = null;
  }
  state = emptyState();
  emit();
}
