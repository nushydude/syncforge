import * as historyApi from "../api/history";
import type { RunDetail, RunReport } from "../types";

export interface HistoryStoreState {
  runs: RunReport[];
  pairFilter: string | null;
  selectedRunId: string | null;
  detail: RunDetail | null;
  loading: boolean;
  detailLoading: boolean;
  error: string | null;
}

type Listener = () => void;

let state: HistoryStoreState = {
  runs: [],
  pairFilter: null,
  selectedRunId: null,
  detail: null,
  loading: false,
  detailLoading: false,
  error: null,
};

const listeners = new Set<Listener>();

let historyLoadRequestId = 0;
let historyDetailRequestId = 0;

function emit() {
  listeners.forEach((l) => l());
}

export function getHistoryState(): HistoryStoreState {
  return state;
}

export function subscribeHistory(listener: Listener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export async function loadHistory(pairFilter?: string | null): Promise<void> {
  const filter =
    pairFilter === undefined ? state.pairFilter : (pairFilter ?? null);
  const requestId = ++historyLoadRequestId;
  historyDetailRequestId++;
  state = {
    ...state,
    loading: true,
    error: null,
    pairFilter: filter,
    selectedRunId: null,
    detail: null,
    detailLoading: false,
  };
  emit();

  try {
    const runs = await historyApi.getHistory(filter);
    if (requestId !== historyLoadRequestId) {
      return;
    }
    state = { ...state, runs, loading: false };
  } catch (e) {
    if (requestId !== historyLoadRequestId) {
      return;
    }
    state = {
      ...state,
      loading: false,
      error: e instanceof Error ? e.message : String(e),
    };
  }
  emit();
}

export function setPairFilter(pairId: string | null): void {
  void loadHistory(pairId);
}

export async function selectRun(runId: string): Promise<void> {
  const requestId = ++historyDetailRequestId;
  state = {
    ...state,
    selectedRunId: runId,
    detailLoading: true,
    detail: null,
    error: null,
  };
  emit();

  try {
    const detail = await historyApi.getRunDetail(runId);
    if (requestId !== historyDetailRequestId) {
      return;
    }
    state = {
      ...state,
      detail,
      detailLoading: false,
      error: detail ? null : "Run not found",
    };
  } catch (e) {
    if (requestId !== historyDetailRequestId) {
      return;
    }
    state = {
      ...state,
      detailLoading: false,
      error: e instanceof Error ? e.message : String(e),
    };
  }
  emit();
}

export function clearRunSelection(): void {
  state = {
    ...state,
    selectedRunId: null,
    detail: null,
    detailLoading: false,
  };
  emit();
}

/** Test helper — reset module state between Vitest cases. */
export function resetHistoryStoreForTests(): void {
  historyLoadRequestId = 0;
  historyDetailRequestId = 0;
  state = {
    runs: [],
    pairFilter: null,
    selectedRunId: null,
    detail: null,
    loading: false,
    detailLoading: false,
    error: null,
  };
  emit();
}
