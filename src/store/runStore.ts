import { listen } from "@tauri-apps/api/event";
import * as previewApi from "../api/preview";
import * as runApi from "../api/run";
import type { ConflictAction } from "../lib/conflictPolicy";
import {
  clearPairPreview,
  getPairEditRevision,
  getPairPreview,
  isPreviewLoadingForPair,
  isPreviewRefreshRequired,
  markPreviewQueued,
  previewPairById,
  requireFreshPreview,
  setPreviewError,
  setLastSyncedAt,
} from "./pairsStore";
import type {
  ConflictChoice,
  FolderPair,
  RunReport,
  RunPairOptions,
  SyncProgress,
  WatchSkippedNotice,
} from "../types";
import { getAppSettings } from "./settingsStore";

export interface PendingConflicts {
  pair: FolderPair;
  conflicts: ConflictAction[];
  planId: string;
  cursor: number;
  nextCursor?: number;
  total: number;
  loading: boolean;
  pageError?: { cursor: number; message: string; stale: boolean };
}

/**
 * Lifecycle of one queued sync. `awaitingInput` means the run reached the front
 * of the queue but needs conflict choices before it can execute.
 */
export type PairRunStatus =
  | "queued"
  | "running"
  | "awaitingInput"
  | "completed"
  | "completedWithErrors"
  | "failed"
  | "cancelled";

export interface PairRunState {
  pairId: string;
  pairName: string;
  status: PairRunStatus;
  progress: SyncProgress | null;
  report: RunReport | null;
  error: string | null;
  queuedAt: number;
  startedAt: number | null;
  finishedAt: number | null;
}

/**
 * Previews and syncs share one queue: both walk both folders, and running two
 * at once thrashes spinning disks. One job touches the disk at a time.
 */
export type QueueJobKind = "preview" | "sync";

export interface QueuedJob {
  pairId: string;
  pairName: string;
  kind: QueueJobKind;
}

export interface RunStoreState {
  /** Latest run record per pair — kept after completion so results stay per pair. */
  runsByPair: Record<string, PairRunState>;
  /** Jobs waiting their turn, in FIFO order. Excludes the active job. */
  queue: QueuedJob[];
  /** Pair currently executing (or awaiting conflict input). */
  activePairId: string | null;
  /** Display name of the active pair — scans have no run record to read it from. */
  activePairName: string | null;
  /** What the active pair is doing — a scan or a sync. */
  activeKind: QueueJobKind | null;
  pendingConflicts: PendingConflicts | null;
  conflictResolutions: Record<string, ConflictChoice>;
  watchSkipped: WatchSkippedNotice | null;
}

type Listener = () => void;

function emptyState(): RunStoreState {
  return {
    runsByPair: {},
    queue: [],
    activePairId: null,
    activePairName: null,
    activeKind: null,
    pendingConflicts: null,
    conflictResolutions: {},
    watchSkipped: null,
  };
}

let state: RunStoreState = emptyState();

const listeners = new Set<Listener>();
let unlistenProgress: (() => void) | null = null;
let unlistenWatchSkipped: (() => void) | null = null;

/** Full pair snapshots for queued ids — the run uses the config as queued. */
const queuedPairs = new Map<string, FolderPair>();
interface QueuedSync {
  options: RunPairOptions;
  pairEditRevision: number;
  conflictCount?: number;
  choicesReady?: boolean;
}
const queuedSyncs = new Map<string, QueuedSync>();
/** UI-initiated run ownership — progress events must match to update state. */
const activeRunIds = new Map<string, string | null>();
/** Active pairs whose backend run has not started yet — cancels abort locally. */
const pendingStarts = new Set<string>();
/** Cancels received before the backend run existed, applied at the start gate. */
const abortRequested = new Set<string>();
let conflictPageRequestId = 0;

/** True when a cancel arrived while the pair was still starting up. */
function consumeAbort(pairId: string): boolean {
  pendingStarts.delete(pairId);
  return abortRequested.delete(pairId);
}

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

export function getPairRun(
  runState: RunStoreState,
  pairId: string | null | undefined,
): PairRunState | null {
  if (!pairId) return null;
  return runState.runsByPair[pairId] ?? null;
}

/** True while a pair is queued, executing, or waiting on conflict choices. */
export function isPairBusy(
  runState: RunStoreState,
  pairId: string | null | undefined,
): boolean {
  const run = getPairRun(runState, pairId);
  return (
    run !== null &&
    (run.status === "queued" ||
      run.status === "running" ||
      run.status === "awaitingInput")
  );
}

/** True while anything is executing or waiting in the queue. */
export function isQueueActive(runState: RunStoreState): boolean {
  return runState.activePairId !== null || runState.queue.length > 0;
}

/**
 * True only while a pair is actually being scanned or synced. A queue parked on
 * a conflict prompt is not "executing" — it must not lock the user out of the
 * rest of the app while it waits for them.
 */
export function isRunExecuting(runState: RunStoreState): boolean {
  if (runState.activePairId === null) return false;
  if (runState.activeKind === "preview") return true;
  return getPairRun(runState, runState.activePairId)?.status === "running";
}

/** True when a scan for this pair is queued or running. */
export function isPreviewQueued(
  runState: RunStoreState,
  pairId: string | null | undefined,
): boolean {
  if (!pairId) return false;
  return (
    (runState.activePairId === pairId && runState.activeKind === "preview") ||
    runState.queue.some(
      (job) => job.pairId === pairId && job.kind === "preview",
    )
  );
}

/** Position of a pair's job in the waiting list, or -1 when not waiting. */
export function queuePosition(
  runState: RunStoreState,
  pairId: string | null | undefined,
  kind: QueueJobKind,
): number {
  if (!pairId) return -1;
  return runState.queue.findIndex(
    (job) => job.pairId === pairId && job.kind === kind,
  );
}

function patchRun(pairId: string, patch: Partial<PairRunState>): void {
  const current = state.runsByPair[pairId];
  if (!current) return;
  state = {
    ...state,
    runsByPair: { ...state.runsByPair, [pairId]: { ...current, ...patch } },
  };
}

export async function ensureProgressListener(): Promise<void> {
  if (unlistenProgress) {
    return;
  }
  unlistenProgress = await listen<SyncProgress>("sync://progress", (event) => {
    const progress = event.payload;
    const pairId = progress.pairId;
    if (
      progress.report?.status === "completed" &&
      progress.report.finishedAt != null
    ) {
      setLastSyncedAt(pairId, progress.report.finishedAt);
    }
    // Only UI-owned runs drive the queue view; watch/schedule runs are ignored.
    if (state.activePairId !== pairId || !activeRunIds.has(pairId)) {
      return;
    }
    const ownedRunId = activeRunIds.get(pairId) ?? null;
    if (ownedRunId === null) {
      activeRunIds.set(pairId, progress.runId);
    } else if (progress.runId !== ownedRunId) {
      return;
    }
    const current = state.runsByPair[pairId];
    if (!current) {
      return;
    }
    patchRun(pairId, {
      progress,
      report: progress.report ?? current.report,
    });
    emit();
  });
}

/** Subscribes to backend watch-auto-sync skip events (conflicts, errors). */
export async function ensureWatchSkippedListener(): Promise<void> {
  if (unlistenWatchSkipped) {
    return;
  }
  unlistenWatchSkipped = await listen<WatchSkippedNotice>(
    "sync://watch-skipped",
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

function finishRun(
  pairId: string,
  status: SettledRunStatus,
  patch: Partial<PairRunState> = {},
): void {
  activeRunIds.delete(pairId);
  queuedPairs.delete(queueKey({ pairId, kind: "sync" }));
  queuedSyncs.delete(pairId);
  pendingStarts.delete(pairId);
  abortRequested.delete(pairId);
  patchRun(pairId, { status, finishedAt: Date.now(), ...patch });
  clearActiveJob(pairId);
}

type SettledRunStatus = Extract<
  PairRunStatus,
  "completed" | "completedWithErrors" | "failed" | "cancelled"
>;

function statusFromReport(report: RunReport): SettledRunStatus {
  switch (report.status) {
    case "completedWithErrors":
    case "failed":
    case "cancelled":
      return report.status;
    default:
      return "completed";
  }
}

/** Backend error raised when watch/schedule already holds the pair's run slot. */
function isPairAlreadyRunningError(message: string): boolean {
  return message.includes("sync run already in progress");
}

/** How often a pair may be sent to the back of the queue before giving up. */
const MAX_BUSY_REQUEUES = 3;
const BUSY_RETRY_DELAY_MS = 1500;
const busyRequeues = new Map<string, number>();

function verifyQueuedPairRevision(pairId: string, expected: number): void {
  if (getPairEditRevision(pairId) !== expected) {
    throw new Error(
      "Preview needs refreshing: the pair changed after this run was queued.",
    );
  }
}

async function executeRun(
  pair: FolderPair,
  options: RunPairOptions,
  pairEditRevision: number,
): Promise<RunReport | null> {
  if (abortRequested.has(pair.id)) {
    finishRun(pair.id, "cancelled");
    emit();
    return null;
  }
  activeRunIds.set(pair.id, null);
  patchRun(pair.id, {
    status: "running",
    startedAt: state.runsByPair[pair.id]?.startedAt ?? Date.now(),
    progress: null,
    report: null,
    error: null,
  });
  emit();

  try {
    await ensureProgressListener();
    if (consumeAbort(pair.id)) {
      finishRun(pair.id, "cancelled");
      emit();
      return null;
    }
    if (options.intent.kind === "reviewed") {
      verifyQueuedPairRevision(pair.id, pairEditRevision);
    }
    const report = await runApi.runPair(pair, options);
    busyRequeues.delete(pair.id);
    const progress = state.runsByPair[pair.id]?.progress ?? null;
    finishRun(pair.id, statusFromReport(report), {
      report,
      progress: progress ? { ...progress, report, phase: report.status } : null,
    });
    // The backend consumes a reused plan, so the cached preview is now stale.
    clearPairPreview(pair.id);
    emit();
    return report;
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    // Watch or schedule beat us to this pair; wait our turn instead of failing.
    if (
      isPairAlreadyRunningError(message) &&
      requeueBusyPair(pair, options, pairEditRevision)
    ) {
      return null;
    }
    busyRequeues.delete(pair.id);
    finishRun(pair.id, "failed", {
      error: message,
      // Never keep progress adopted from another run in the failed record.
      progress: null,
    });
    if (
      isPreviewRefreshRequired(message) ||
      options.intent.kind === "reviewed"
    ) {
      requireFreshPreview(
        pair.id,
        "Preview needs refreshing: review a new preview before syncing.",
      );
    } else {
      clearPairPreview(pair.id);
    }
    emit();
    return null;
  }
}

/** Sends a pair that lost its slot to a background run back to the queue. */
function requeueBusyPair(
  pair: FolderPair,
  options: RunPairOptions,
  pairEditRevision: number,
): boolean {
  const attempts = (busyRequeues.get(pair.id) ?? 0) + 1;
  if (attempts > MAX_BUSY_REQUEUES) {
    return false;
  }
  busyRequeues.set(pair.id, attempts);
  activeRunIds.delete(pair.id);
  queuedPairs.set(queueKey({ pairId: pair.id, kind: "sync" }), pair);
  queuedSyncs.set(pair.id, { options, pairEditRevision, choicesReady: true });
  patchRun(pair.id, {
    status: "queued",
    progress: null,
    error: "Waiting for an automatic sync of this pair to finish.",
  });
  state = {
    ...state,
    queue: [
      ...state.queue,
      { pairId: pair.id, pairName: pair.name, kind: "sync" },
    ],
  };
  clearActiveJob(pair.id);
  emit();
  return true;
}

/** Scans first for `ask` pairs so conflicts can be resolved before executing. */
async function resolveConflictsThenRun(
  pair: FolderPair,
  queued: QueuedSync,
): Promise<RunReport | null> {
  const { options, pairEditRevision } = queued;
  // The pre-run scan is real work — show it, and let the user cancel out of it.
  patchRun(pair.id, {
    status: "running",
    startedAt: Date.now(),
    progress: null,
    error: null,
  });
  emit();
  try {
    if (abortRequested.has(pair.id)) {
      consumeAbort(pair.id);
      finishRun(pair.id, "cancelled");
      emit();
      return null;
    }
    verifyQueuedPairRevision(pair.id, pairEditRevision);
    const plan =
      options.intent.kind === "fresh"
        ? await previewApi.previewPair(pair)
        : null;
    if (abortRequested.has(pair.id)) {
      consumeAbort(pair.id);
      finishRun(pair.id, "cancelled");
      emit();
      return null;
    }
    const planId =
      options.intent.kind === "reviewed" ? options.intent.planId : plan?.planId;
    if (!planId)
      throw new Error("Preview needs refreshing: missing reviewed plan.");
    const total = plan?.conflictCount ?? queued.conflictCount;
    const firstPage =
      total === 0
        ? { actions: [], nextCursor: undefined }
        : plan && total === undefined
          ? { actions: plan.actions, nextCursor: undefined }
          : await previewApi.getPreviewConflicts(pair, planId, 0);
    if (abortRequested.has(pair.id)) {
      consumeAbort(pair.id);
      finishRun(pair.id, "cancelled");
      emit();
      return null;
    }
    verifyQueuedPairRevision(pair.id, pairEditRevision);
    const conflicts = firstPage.actions.filter(
      (action): action is ConflictAction => action.kind === "conflict",
    );
    if (conflicts.length > 0) {
      patchRun(pair.id, { status: "awaitingInput" });
      state = {
        ...state,
        pendingConflicts: {
          pair,
          conflicts,
          planId,
          cursor: 0,
          nextCursor: firstPage.nextCursor,
          total: total ?? conflicts.length,
          loading: false,
        },
        conflictResolutions: {},
      };
      conflictPageRequestId++;
      emit();
      return null;
    }
    return await executeRun(
      pair,
      { ...options, intent: { kind: "reviewed", planId } },
      pairEditRevision,
    );
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    finishRun(pair.id, "failed", {
      error: message,
    });
    if (isPreviewRefreshRequired(message)) {
      requireFreshPreview(
        pair.id,
        "Preview needs refreshing: review a new preview before syncing.",
      );
    }
    emit();
    return null;
  }
}

/**
 * Starts the next queued pair when nothing is executing. Runs are sequential by
 * design — one pair at a time, in the order they were queued.
 */
function pumpQueue(): void {
  if (state.activePairId !== null || state.pendingConflicts) {
    return;
  }
  const [next, ...rest] = state.queue;
  if (!next) {
    return;
  }
  const nextId = next.pairId;
  const pair = queuedPairs.get(queueKey(next));
  state = {
    ...state,
    queue: rest,
    activePairId: nextId,
    activePairName: next.pairName,
    activeKind: next.kind,
  };
  if (!pair) {
    if (next.kind === "sync") {
      finishRun(nextId, "failed", {
        error: "queued pair is no longer available",
      });
    } else {
      clearActiveJob(nextId);
    }
    emit();
    pumpQueue();
    return;
  }

  if (next.kind === "preview") {
    emit();
    void runQueuedPreview(pair).finally(() => {
      pumpQueue();
    });
    return;
  }

  pendingStarts.add(nextId);
  const queued = queuedSyncs.get(nextId);
  emit();

  // A pair bounced by a background sync gets a breather before trying again.
  const retryDelay = busyRequeues.has(nextId) ? BUSY_RETRY_DELAY_MS : 0;
  const started = (async () => {
    if (retryDelay > 0) {
      await new Promise((resolve) => setTimeout(resolve, retryDelay));
    }
    if (!queued) {
      finishRun(pair.id, "failed", {
        error: "queued sync settings are missing",
      });
      emit();
      return null;
    }
    return pair.conflictPolicy === "ask" && !queued.choicesReady
      ? await resolveConflictsThenRun(pair, queued)
      : await executeRun(pair, queued.options, queued.pairEditRevision);
  })();
  void started.finally(() => {
    pumpQueue();
  });
}

/** Queue entries are keyed by kind so a pair can hold a scan and a sync. */
function queueKey(job: QueuedJob | { pairId: string; kind: QueueJobKind }) {
  return `${job.kind}:${job.pairId}`;
}

function clearWaitingPreview(pairId: string): void {
  if (getPairPreview(pairId).reviewRequired) {
    requireFreshPreview(
      pairId,
      "Preview needs refreshing: review a new preview before syncing.",
    );
  } else {
    clearPairPreview(pairId);
  }
}

function clearActiveJob(pairId: string): void {
  if (state.activePairId === pairId) {
    state = {
      ...state,
      activePairId: null,
      activePairName: null,
      activeKind: null,
    };
  }
}

async function runQueuedPreview(pair: FolderPair): Promise<void> {
  queuedPairs.delete(queueKey({ pairId: pair.id, kind: "preview" }));
  try {
    if (getPairPreview(pair.id).queued) await previewPairById(pair);
  } finally {
    clearActiveJob(pair.id);
    emit();
  }
}

/**
 * Queues a pair for syncing. Returns false when the pair is already queued or
 * running, has no id, or the user declined the confirmation prompt.
 */
function canQueuePairRun(pair: FolderPair): boolean {
  const preview = getPairPreview(pair.id);
  return (
    !!pair.id &&
    !isPairBusy(state, pair.id) &&
    !isPreviewRefreshRequired(preview.error) &&
    !preview.reviewRequired
  );
}

export function enqueuePairRun(pair: FolderPair, skipConfirm = false): boolean {
  if (!canQueuePairRun(pair)) return false;
  const preview = getPairPreview(pair.id);
  const settings = getAppSettings();
  if (
    !skipConfirm &&
    settings.confirmBeforeRun &&
    typeof window !== "undefined"
  ) {
    const position = state.activePairId !== null ? state.queue.length + 1 : 0;
    const message =
      position > 0
        ? `Queue sync for "${pair.name}"? It will start after ${position} run${
            position === 1 ? "" : "s"
          } ahead of it.`
        : `Start sync for "${pair.name}"?`;
    if (!window.confirm(message)) {
      return false;
    }
  }

  const plan = preview.plan?.pairId === pair.id ? preview.plan : null;
  queuedSyncs.set(pair.id, {
    options: {
      intent: plan?.planId
        ? { kind: "reviewed", planId: plan.planId }
        : { kind: "fresh" },
      verifyHashes: settings.verifyHashesAfterCopy,
      useRecycleBin: settings.moveDeletesToRecycleBin,
      conflictResolutions: {},
    },
    pairEditRevision: getPairEditRevision(pair.id),
    conflictCount: plan?.conflictCount,
  });
  queuedPairs.set(queueKey({ pairId: pair.id, kind: "sync" }), pair);
  const now = Date.now();
  state = {
    ...state,
    queue: [
      ...state.queue,
      { pairId: pair.id, pairName: pair.name, kind: "sync" },
    ],
    runsByPair: {
      ...state.runsByPair,
      [pair.id]: {
        pairId: pair.id,
        pairName: pair.name,
        status: "queued",
        progress: null,
        report: null,
        error: null,
        queuedAt: now,
        startedAt: null,
        finishedAt: null,
      },
    },
  };
  emit();
  pumpQueue();
  return true;
}

/**
 * Queues every pair that is not disabled and not already busy. Confirmation is
 * asked once for the batch rather than once per pair.
 */
export function enqueuePairRuns(pairs: FolderPair[]): number {
  const startable = pairs.filter(canQueuePairRun);
  if (startable.length === 0) {
    return 0;
  }

  const settings = getAppSettings();
  if (settings.confirmBeforeRun && typeof window !== "undefined") {
    const names = startable.map((pair) => pair.name).join(", ");
    if (
      !window.confirm(
        `Queue sync for ${startable.length} pair${
          startable.length === 1 ? "" : "s"
        } (${names})? They run one at a time.`,
      )
    ) {
      return 0;
    }
  }

  let queued = 0;
  for (const pair of startable) {
    if (enqueuePairRun(pair, true)) {
      queued += 1;
    }
  }
  return queued;
}

/**
 * Queues a scan for `pair`. Scans share the sync queue so only one job walks the
 * disk at a time — concurrent scans are what make spinning drives crawl.
 */
export function enqueuePairPreview(pair: FolderPair): boolean {
  if (!pair.id || isPreviewQueued(state, pair.id)) {
    return false;
  }
  queuedPairs.set(queueKey({ pairId: pair.id, kind: "preview" }), pair);
  markPreviewQueued(pair.id);
  state = {
    ...state,
    queue: [
      ...state.queue,
      { pairId: pair.id, pairName: pair.name, kind: "preview" },
    ],
  };
  emit();
  pumpQueue();
  return true;
}

/** Drops a waiting scan, or stops the running one via the backend. */
export async function cancelPairPreview(pairId: string): Promise<void> {
  const waitingAt = queuePosition(state, pairId, "preview");
  if (waitingAt >= 0) {
    queuedPairs.delete(queueKey({ pairId, kind: "preview" }));
    state = {
      ...state,
      queue: state.queue.filter(
        (job) => !(job.pairId === pairId && job.kind === "preview"),
      ),
    };
    clearWaitingPreview(pairId);
    emit();
    return;
  }

  if (state.activePairId !== pairId || state.activeKind !== "preview") {
    return;
  }

  try {
    await previewApi.cancelPreview(pairId);
  } catch (e) {
    // Never swallow this: a cancel that silently fails leaves the user watching
    // a scan they asked to stop.
    const message = e instanceof Error ? e.message : String(e);
    if (isPreviewLoadingForPair(pairId)) {
      setPreviewError(pairId, `Could not cancel the scan: ${message}`);
    }
  }
}

/**
 * Queues one pair and resolves once that pair's run settles. Kept for existing
 * call sites and tests that await a single run.
 */
export async function runSelectedPair(
  pair: FolderPair,
): Promise<RunReport | null> {
  if (!enqueuePairRun(pair)) {
    return null;
  }
  return await waitForPairRun(pair.id);
}

function waitForPairRun(pairId: string): Promise<RunReport | null> {
  return new Promise((resolve) => {
    const settle = () => {
      const run = state.runsByPair[pairId];
      if (!run) {
        // The record was dropped (queue cleared or dismissed) — nothing to await.
        unsubscribe();
        resolve(null);
        return;
      }
      if (run.status === "queued" || run.status === "running") {
        return;
      }
      if (run.status === "awaitingInput") {
        unsubscribe();
        resolve(null);
        return;
      }
      unsubscribe();
      resolve(run.report ?? null);
    };
    const unsubscribe = subscribeRun(settle);
    settle();
  });
}

/** Removes a queued pair, or cancels it when it is the pair being executed. */
export async function cancelPairRun(pairId: string): Promise<void> {
  const run = state.runsByPair[pairId];
  if (!run) {
    return;
  }

  if (state.activePairId !== pairId || state.activeKind !== "sync") {
    queuedPairs.delete(queueKey({ pairId, kind: "sync" }));
    queuedSyncs.delete(pairId);
    busyRequeues.delete(pairId);
    const runsByPair = { ...state.runsByPair };
    delete runsByPair[pairId];
    state = {
      ...state,
      queue: state.queue.filter(
        (job) => !(job.pairId === pairId && job.kind === "sync"),
      ),
      runsByPair,
    };
    emit();
    return;
  }

  if (run.status === "awaitingInput") {
    cancelConflictResolution();
    return;
  }

  // The backend has no run for this pair yet (queue delay or pre-run scan), so
  // cancel_run would fail — or worse, cancel an unrelated background sync.
  if (pendingStarts.has(pairId)) {
    abortRequested.add(pairId);
    patchRun(pairId, { error: null });
    emit();
    return;
  }

  try {
    await runApi.cancelRun(pairId);
  } catch (e) {
    patchRun(pairId, { error: e instanceof Error ? e.message : String(e) });
    emit();
  }
}

/** Drops every waiting job; the job already executing keeps going. */
export function clearRunQueue(): void {
  if (state.queue.length === 0) {
    return;
  }
  const runsByPair = { ...state.runsByPair };
  const waitingPreviews: string[] = [];
  for (const job of state.queue) {
    queuedPairs.delete(queueKey(job));
    if (job.kind === "sync") {
      queuedSyncs.delete(job.pairId);
      busyRequeues.delete(job.pairId);
      delete runsByPair[job.pairId];
    } else {
      waitingPreviews.push(job.pairId);
    }
  }
  state = { ...state, queue: [], runsByPair };
  emit();
  waitingPreviews.forEach(clearWaitingPreview);
}

/** Clears a settled run record so the pair view returns to its idle state. */
export function dismissPairRun(pairId: string): void {
  const run = state.runsByPair[pairId];
  if (!run || isPairBusy(state, pairId)) {
    return;
  }
  const runsByPair = { ...state.runsByPair };
  delete runsByPair[pairId];
  state = { ...state, runsByPair };
  emit();
}

export async function loadConflictPage(cursor: number): Promise<void> {
  const pending = state.pendingConflicts;
  if (!pending || pending.loading || pending.pageError?.stale) return;
  const requestId = ++conflictPageRequestId;
  state = {
    ...state,
    pendingConflicts: { ...pending, loading: true, pageError: undefined },
  };
  emit();
  try {
    const page = await previewApi.getPreviewConflicts(
      pending.pair,
      pending.planId,
      cursor,
    );
    if (
      requestId !== conflictPageRequestId ||
      state.pendingConflicts?.planId !== pending.planId
    ) {
      return;
    }
    state = {
      ...state,
      pendingConflicts: {
        ...pending,
        conflicts: page.actions.filter(
          (action): action is ConflictAction => action.kind === "conflict",
        ),
        cursor,
        nextCursor: page.nextCursor,
        loading: false,
        pageError: undefined,
      },
    };
  } catch (e) {
    if (
      requestId !== conflictPageRequestId ||
      state.pendingConflicts?.planId !== pending.planId
    ) {
      return;
    }
    const message = e instanceof Error ? e.message : String(e);
    const stale = isPreviewRefreshRequired(message);
    state = {
      ...state,
      pendingConflicts: {
        ...pending,
        loading: false,
        pageError: { cursor, message, stale },
      },
      conflictResolutions: stale ? {} : state.conflictResolutions,
    };
  }
  emit();
}

export function setConflictResolution(
  path: string,
  choice: ConflictChoice,
): void {
  if (
    state.pendingConflicts?.pageError?.stale ||
    !state.pendingConflicts?.conflicts.some(
      (conflict) => conflict.path === path,
    )
  )
    return;
  state = {
    ...state,
    conflictResolutions: { ...state.conflictResolutions, [path]: choice },
  };
  emit();
}

export function cancelConflictResolution(): void {
  const pending = state.pendingConflicts;
  conflictPageRequestId++;
  state = {
    ...state,
    pendingConflicts: null,
    conflictResolutions: {},
  };
  if (pending) {
    finishRun(pending.pair.id, "cancelled");
  }
  emit();
  pumpQueue();
}

export function refreshStaleConflictPreview(): string | null {
  const pending = state.pendingConflicts;
  if (!pending?.pageError?.stale) return null;
  requireFreshPreview(
    pending.pair.id,
    "Preview needs refreshing: review a new preview before syncing.",
  );
  cancelConflictResolution();
  return pending.pair.id;
}

export async function confirmConflictResolutionAndRun(): Promise<RunReport | null> {
  const pending = state.pendingConflicts;
  const queued = pending && queuedSyncs.get(pending.pair.id);
  if (
    !pending ||
    !queued ||
    pending.loading ||
    pending.pageError?.stale ||
    Object.keys(state.conflictResolutions).length !== pending.total ||
    state.runsByPair[pending.pair.id]?.status === "running"
  ) {
    return null;
  }
  const resolutions = state.conflictResolutions;
  state = { ...state, pendingConflicts: null, conflictResolutions: {} };
  emit();
  try {
    return await executeRun(
      pending.pair,
      {
        ...queued.options,
        conflictResolutions: resolutions,
        intent: { kind: "reviewed", planId: pending.planId },
      },
      queued.pairEditRevision,
    );
  } finally {
    pumpQueue();
  }
}

/** Test helper — reset module state between Vitest cases. */
export function resetRunStoreForTests(): void {
  unlistenProgress = null;
  unlistenWatchSkipped = null;
  queuedPairs.clear();
  queuedSyncs.clear();
  activeRunIds.clear();
  pendingStarts.clear();
  abortRequested.clear();
  busyRequeues.clear();
  conflictPageRequestId = 0;
  state = emptyState();
  emit();
}
