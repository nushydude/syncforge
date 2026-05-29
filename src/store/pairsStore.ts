import * as pairsApi from "../api/pairs";
import { validatePairForm } from "../lib/pairValidation";
import type { ConflictPolicy, FolderPair, SyncMode } from "../types";
import { defaultFilters } from "../types";

export interface PairsStoreState {
  pairs: FolderPair[];
  selectedId: string | null;
  editing: FolderPair | null;
  loading: boolean;
  saving: boolean;
  error: string | null;
  validationErrors: string[];
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
};

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
  };
  emit();
}

export function startNewPair(): void {
  state = {
    ...state,
    selectedId: null,
    editing: emptyPair(),
    validationErrors: [],
    error: null,
  };
  emit();
}

export function cancelEdit(): void {
  state = {
    ...state,
    editing: null,
    validationErrors: [],
    error: null,
  };
  emit();
}

export function updateEditing(patch: Partial<FolderPair>): void {
  if (!state.editing) {
    return;
  }
  state = {
    ...state,
    editing: { ...state.editing, ...patch },
    validationErrors: [],
  };
  emit();
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
  };
  emit();
}
