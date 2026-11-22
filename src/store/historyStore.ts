import * as historyApi from "../api/history";
import type { RunActionFilter, RunDetail, RunReport } from "../types";

export interface HistoryStoreState {
  runs: RunReport[];
  pairFilter: string | null;
  selectedRunId: string | null;
  actionFilter: RunActionFilter | null;
  detail: RunDetail | null;
  loading: boolean;
  detailLoading: boolean;
  detailPageLoading: boolean;
  error: string | null;
}

type Listener = () => void;

let state: HistoryStoreState = {
  runs: [],
  pairFilter: null,
  selectedRunId: null,
  actionFilter: null,
  detail: null,
  loading: false,
  detailLoading: false,
  detailPageLoading: false,
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
    actionFilter: null,
    detail: null,
    detailLoading: false,
    detailPageLoading: false,
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
      error: state.selectedRunId
        ? state.error
        : e instanceof Error
          ? e.message
          : String(e),
    };
  }
  emit();
}

export function setPairFilter(pairId: string | null): void {
  void loadHistory(pairId);
}

export async function selectRun(
  runId: string,
  actionFilter: RunActionFilter | null = null,
): Promise<void> {
  const requestId = ++historyDetailRequestId;
  const previous = state.selectedRunId === runId ? state.detail : null;
  state = {
    ...state,
    selectedRunId: runId,
    actionFilter,
    detailLoading: true,
    detailPageLoading: false,
    detail: previous ? { report: previous.report, items: [] } : null,
    error: null,
  };
  emit();

  try {
    const detail = actionFilter
      ? await historyApi.getRunDetail(runId, 0, undefined, actionFilter)
      : await historyApi.getRunDetail(runId);
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

export async function loadMoreRunItems(): Promise<void> {
  const detail = state.detail;
  const runId = state.selectedRunId;
  if (!detail || !runId || detail.nextCursor == null || state.detailPageLoading)
    return;
  const requestId = historyDetailRequestId;
  state = { ...state, detailPageLoading: true };
  emit();
  try {
    const next = state.actionFilter
      ? await historyApi.getRunDetail(
          runId,
          detail.nextCursor,
          undefined,
          state.actionFilter,
        )
      : await historyApi.getRunDetail(runId, detail.nextCursor);
    if (requestId !== historyDetailRequestId || !next) {
      if (requestId === historyDetailRequestId) {
        state = { ...state, detailPageLoading: false };
        emit();
      }
      return;
    }
    state = {
      ...state,
      detailPageLoading: false,
      detail: {
        ...detail,
        items: [...detail.items, ...next.items],
        nextCursor: next.nextCursor,
      },
    };
    emit();
  } catch (e) {
    if (requestId !== historyDetailRequestId) {
      return;
    }
    state = {
      ...state,
      detailPageLoading: false,
      error: e instanceof Error ? e.message : String(e),
    };
    emit();
  }
}

export function clearRunSelection(): void {
  historyDetailRequestId++;
  state = {
    ...state,
    selectedRunId: null,
    actionFilter: null,
    detail: null,
    detailLoading: false,
    detailPageLoading: false,
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
    actionFilter: null,
    detail: null,
    loading: false,
    detailLoading: false,
    detailPageLoading: false,
    error: null,
  };
  emit();
}
