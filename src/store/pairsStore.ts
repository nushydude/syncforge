import * as pairsApi from "../api/pairs";
import * as previewApi from "../api/preview";
import { validatePairForm } from "../lib/pairValidation";
import type { ConflictPolicy, FolderPair, SyncMode, SyncPlan } from "../types";
import { defaultFilters } from "../types";

export interface PairsStoreState {
  pairs: FolderPair[];
  selectedId: string | null;
  editing: FolderPair | null;
  loading: boolean;
  saving: boolean;
  error: string | null;
  validationErrors: string[];
  previewPlan: SyncPlan | null;
  previewLoading: boolean;
  previewError: string | null;
  watchWarning: string | null;
}

type Listener = () => void;

const defaultConflictPolicy: ConflictPolicy = "newerWins";
const defaultMode: SyncMode = "synchronize";

function emptyPair(): FolderPair {
  return {
    id: "",
    name: "",
    leftPath: "",
    rightPath: "",
    mode: defaultMode,
    filters: defaultFilters(),
    conflictPolicy: defaultConflictPolicy,
    enabled: true,
    watchEnabled: false,
    createdAt: 0,
    updatedAt: 0,
  };
}

let state: PairsStoreState = {
  pairs: [],
  selectedId: null,
  editing: null,
  loading: false,
  saving: false,
  error: null,
  validationErrors: [],
  previewPlan: null,
  previewLoading: false,
  previewError: null,
  watchWarning: null,
};

const WATCH_INACTIVE_MSG =
  "Auto-sync watch is saved but inactive until both folder paths exist on disk.";

const listeners = new Set<Listener>();

function emit() {
  listeners.forEach((l) => l());
}

export function getPairsState(): PairsStoreState {
  return state;
}

export function subscribePairs(listener: Listener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

async function refreshWatchWarning(pair: FolderPair | null): Promise<void> {
  if (!pair?.watchEnabled || !pair.enabled) {
    state = { ...state, watchWarning: null };
    emit();
    return;
  }
  const left = pair.leftPath.trim();
  const right = pair.rightPath.trim();
  if (!left || !right) {
    state = { ...state, watchWarning: WATCH_INACTIVE_MSG };
    emit();
    return;
  }
  const [leftExists, rightExists] = await Promise.all([
    pairsApi.pathExists(left),
    pairsApi.pathExists(right),
  ]);
  state = {
    ...state,
    watchWarning:
      leftExists && rightExists ? null : WATCH_INACTIVE_MSG,
  };
  emit();
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
  state = {
    ...state,
    selectedId: id,
    editing: { ...pair, filters: { ...pair.filters } },
    validationErrors: [],
    error: null,
    previewPlan: null,
    previewError: null,
    watchWarning: null,
  };
  emit();
  void refreshWatchWarning(state.editing);
}

export function startNewPair(): void {
  state = {
    ...state,
    selectedId: null,
    editing: emptyPair(),
    validationErrors: [],
    error: null,
    watchWarning: null,
  };
  emit();
}

export function cancelEdit(): void {
  state = {
    ...state,
    editing: null,
    validationErrors: [],
    error: null,
    previewPlan: null,
    previewError: null,
    watchWarning: null,
  };
  emit();
}

export async function previewSelectedPair(): Promise<void> {
  const editing = state.editing;
  if (!editing?.id) {
    return;
  }

  state = {
    ...state,
    previewLoading: true,
    previewError: null,
    previewPlan: null,
  };
  emit();

  try {
    const previewPlan = await previewApi.previewPair(editing);
    state = { ...state, previewPlan, previewLoading: false };
  } catch (e) {
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
    void refreshWatchWarning(editing);
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
    const exists = state.pairs.some((p) => p.id === saved.id);
    const pairs = exists
      ? state.pairs.map((p) => (p.id === saved.id ? saved : p))
      : [...state.pairs, saved];
    state = {
      ...state,
      pairs,
      selectedId: saved.id,
      editing: { ...saved, filters: { ...saved.filters } },
      saving: false,
      validationErrors: [],
    };
    emit();
    await refreshWatchWarning(state.editing);
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
  state = {
    pairs: [],
    selectedId: null,
    editing: null,
    loading: false,
    saving: false,
    error: null,
    validationErrors: [],
    previewPlan: null,
    previewLoading: false,
    previewError: null,
    watchWarning: null,
  };
  emit();
}
