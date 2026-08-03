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

export interface PairsStoreState {
  pairs: FolderPair[];
  selectedId: string | null;
  editing: FolderPair | null;
  editorOpen: boolean;
  loading: boolean;
  saving: boolean;
  error: string | null;
  validationErrors: string[];
  previewPlan: PreviewSummary | null;
  previewLoading: boolean;
  previewError: string | null;
  watchWarning: string | null;
  scheduleError: string | null;
  scheduleDescription: string | null;
}

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

let state: PairsStoreState = {
  pairs: [],
  selectedId: null,
  editing: null,
  editorOpen: false,
  loading: false,
  saving: false,
  error: null,
  validationErrors: [],
  previewPlan: null,
  previewLoading: false,
  previewError: null,
  watchWarning: null,
  scheduleError: null,
  scheduleDescription: null,
};

const WATCH_INACTIVE_MSG =
  "Auto-sync watch is saved but inactive until both folder paths exist on disk.";

const listeners = new Set<Listener>();

/** Monotonic token — stale preview responses are ignored when this changes. */
let previewRequestId = 0;

const PATH_EXISTS_DEBOUNCE_MS = 300;
let watchWarningTimer: ReturnType<typeof setTimeout> | null = null;
let watchWarningRequestId = 0;

function emit() {
  listeners.forEach((l) => l());
}

export function isPreviewLoading(): boolean {
  return state.previewLoading;
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
    const pairs = await pairsApi.listPairs();
    state = {
      ...state,
      pairs,
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

export function selectPair(id: string): void {
  const pair = state.pairs.find((p) => p.id === id);
  if (!pair) {
    return;
  }
  previewRequestId++;
  state = {
    ...state,
    selectedId: id,
    editing: { ...pair, filters: { ...pair.filters } },
    editorOpen: false,
    validationErrors: [],
    error: null,
    previewPlan: null,
    previewLoading: false,
    previewError: null,
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
  previewRequestId++;
  state = {
    ...state,
    editing: null,
    editorOpen: false,
    validationErrors: [],
    error: null,
    previewPlan: null,
    previewLoading: false,
    previewError: null,
    watchWarning: null,
    scheduleError: null,
    scheduleDescription: null,
  };
  emit();
}

export async function previewSelectedPair(): Promise<void> {
  const editing = state.editing;
  if (!editing?.id) {
    return;
  }

  const requestId = ++previewRequestId;
  state = {
    ...state,
    previewLoading: true,
    previewError: null,
    previewPlan: null,
  };
  emit();

  try {
    const previewPlan = await previewApi.previewPair(editing);
    if (requestId !== previewRequestId) {
      return;
    }
    state = { ...state, previewPlan, previewLoading: false };
  } catch (e) {
    if (requestId !== previewRequestId) {
      return;
    }
    state = {
      ...state,
      previewLoading: false,
      previewError: e instanceof Error ? e.message : String(e),
    };
  }
  emit();
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
    state = {
      ...state,
      pairs,
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
  previewRequestId = 0;
  watchWarningRequestId = 0;
  if (watchWarningTimer) {
    clearTimeout(watchWarningTimer);
    watchWarningTimer = null;
  }
  state = {
    pairs: [],
    selectedId: null,
    editing: null,
    editorOpen: false,
    loading: false,
    saving: false,
    error: null,
    validationErrors: [],
    previewPlan: null,
    previewLoading: false,
    previewError: null,
    watchWarning: null,
    scheduleError: null,
    scheduleDescription: null,
  };
  emit();
}
