import { beforeEach, describe, expect, it, vi } from "vitest";
import * as previewApi from "../api/preview";
import * as runApi from "../api/run";
import * as pairsApi from "../api/pairs";
import type { FolderPair, RunReport } from "../types";
import {
  cancelConflictResolution,
  cancelPairRun,
  clearRunQueue,
  confirmConflictResolutionAndRun,
  dismissPairRun,
  dismissWatchSkipped,
  cancelPairPreview,
  enqueuePairPreview,
  enqueuePairRun,
  enqueuePairRuns,
  ensureWatchSkippedListener,
  ensureWatchHealthListener,
  getPairRun,
  getRunState,
  isPairBusy,
  isQueueActive,
  resetRunStoreForTests,
  runSelectedPair,
  setConflictResolution,
} from "../store/runStore";
import * as pairsStore from "../store/pairsStore";
import {
  resetSettingsForTests,
  updateAppSettings,
} from "../store/settingsStore";

const listenHandlers: Record<string, (event: { payload: unknown }) => void> =
  {};

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(
    async (event: string, handler: (event: { payload: unknown }) => void) => {
      listenHandlers[event] = handler;
      return () => {
        delete listenHandlers[event];
      };
    },
  ),
}));

vi.mock("../api/preview", () => ({
  previewPair: vi.fn(),
  cancelPreview: vi.fn(),
  getPreviewConflicts: vi.fn(),
}));

vi.mock("../api/run", () => ({
  runPair: vi.fn(),
  cancelRun: vi.fn(),
}));

const samplePair: FolderPair = {
  id: "pair-1",
  name: "Docs",
  leftPath: "C:\\left",
  rightPath: "D:\\right",
  mode: "echo",
  filters: { include: [], exclude: [] },
  conflictPolicy: "newerWins",
  enabled: true,
  continueOnError: true,
  watchEnabled: false,
  scheduleEnabled: false,
  scheduleCron: null,
  createdAt: 1,
  updatedAt: 2,
};

const secondPair: FolderPair = {
  ...samplePair,
  id: "pair-2",
  name: "Photos",
};

const sampleReport: RunReport = {
  runId: "run-1",
  pairId: "pair-1",
  startedAt: 1,
  finishedAt: 2,
  status: "completed",
  filesCopied: 1,
  filesDeleted: 0,
  bytesTransferred: 100,
  errors: [],
};

const conflictPlan = {
  pairId: "pair-1",
  planId: "ask-reviewed",
  scannedLeft: 1,
  scannedRight: 1,
  actions: [
    {
      kind: "conflict" as const,
      path: "both.txt",
      left: {
        relativePath: "both.txt",
        size: 1,
        modifiedSecs: 1,
        isDir: false,
      },
      right: {
        relativePath: "both.txt",
        size: 2,
        modifiedSecs: 2,
        isDir: false,
      },
    },
  ],
};

function runOf(pairId: string) {
  return getPairRun(getRunState(), pairId);
}

function queueIds() {
  return getRunState().queue.map((job) => job.pairId);
}

describe("runStore", () => {
  beforeEach(() => {
    resetRunStoreForTests();
    pairsStore.resetPairsStoreForTests();
    resetSettingsForTests();
    vi.clearAllMocks();
    vi.restoreAllMocks();
    vi.spyOn(window, "confirm").mockReturnValue(true);
    for (const key of Object.keys(listenHandlers)) {
      delete listenHandlers[key];
    }
  });

  it("runs a pair and stores the report against that pair", async () => {
    vi.mocked(runApi.runPair).mockResolvedValue(sampleReport);
    const report = await runSelectedPair(samplePair);
    expect(report).toEqual(sampleReport);
    expect(isQueueActive(getRunState())).toBe(false);
    expect(runOf("pair-1")?.status).toBe("completed");
    expect(runOf("pair-1")?.report).toEqual(sampleReport);
  });

  it("surfaces run errors on the pair record", async () => {
    vi.mocked(runApi.runPair).mockRejectedValue(new Error("disk full"));
    const report = await runSelectedPair(samplePair);
    expect(report).toBeNull();
    expect(runOf("pair-1")?.status).toBe("failed");
    expect(runOf("pair-1")?.error).toBe("disk full");
  });

  it("does not start a manual run when confirmation is declined", async () => {
    vi.mocked(runApi.runPair).mockResolvedValue(sampleReport);
    vi.mocked(window.confirm).mockReturnValue(false);
    const report = await runSelectedPair(samplePair);
    expect(report).toBeNull();
    expect(window.confirm).toHaveBeenCalledWith('Start sync for "Docs"?');
    expect(runApi.runPair).not.toHaveBeenCalled();
  });

  it("ignores run without pair id", async () => {
    const report = await runSelectedPair({ ...samplePair, id: "" });
    expect(report).toBeNull();
    expect(runApi.runPair).not.toHaveBeenCalled();
  });

  it("runs queued pairs one at a time in order", async () => {
    const resolvers: ((report: RunReport) => void)[] = [];
    vi.mocked(runApi.runPair).mockImplementation(
      () => new Promise((resolve) => resolvers.push(resolve)),
    );

    expect(enqueuePairRun(samplePair)).toBe(true);
    expect(enqueuePairRun(secondPair)).toBe(true);

    // Only the first pair is handed to the backend; the second waits.
    await vi.waitFor(() => expect(runApi.runPair).toHaveBeenCalledTimes(1));
    expect(getRunState().activePairId).toBe("pair-1");
    expect(queueIds()).toEqual(["pair-2"]);
    expect(runOf("pair-2")?.status).toBe("queued");

    resolvers[0]({ ...sampleReport, pairId: "pair-1" });
    await vi.waitFor(() => expect(runApi.runPair).toHaveBeenCalledTimes(2));

    expect(getRunState().activePairId).toBe("pair-2");
    expect(queueIds()).toEqual([]);
    expect(runOf("pair-1")?.status).toBe("completed");

    resolvers[1]({ ...sampleReport, runId: "run-2", pairId: "pair-2" });
    await vi.waitFor(() => expect(isQueueActive(getRunState())).toBe(false));
    expect(runOf("pair-2")?.status).toBe("completed");
    // Each pair keeps its own result.
    expect(runOf("pair-1")?.report?.runId).toBe("run-1");
    expect(runOf("pair-2")?.report?.runId).toBe("run-2");
  });

  it("queues many pairs at once and skips ones already busy", () => {
    vi.mocked(runApi.runPair).mockImplementation(() => new Promise(() => {}));

    expect(enqueuePairRuns([samplePair, secondPair])).toBe(2);
    expect(enqueuePairRuns([samplePair, secondPair])).toBe(0);
    expect(queueIds()).toEqual(["pair-2"]);
  });

  it("removes a waiting pair without touching the running one", async () => {
    vi.mocked(runApi.runPair).mockImplementation(() => new Promise(() => {}));
    enqueuePairRun(samplePair);
    enqueuePairRun(secondPair);

    await cancelPairRun("pair-2");

    expect(queueIds()).toEqual([]);
    expect(runOf("pair-2")).toBeNull();
    expect(getRunState().activePairId).toBe("pair-1");
    expect(runApi.cancelRun).not.toHaveBeenCalled();
  });

  it("clears every waiting pair but keeps the active run", () => {
    vi.mocked(runApi.runPair).mockImplementation(() => new Promise(() => {}));
    enqueuePairRuns([samplePair, secondPair]);

    clearRunQueue();

    expect(queueIds()).toEqual([]);
    expect(getRunState().activePairId).toBe("pair-1");
    expect(runOf("pair-1")?.status).toBe("running");
  });

  it("cancels the active run through the backend", async () => {
    vi.mocked(runApi.runPair).mockImplementation(() => new Promise(() => {}));
    vi.mocked(runApi.cancelRun).mockResolvedValue(undefined);
    enqueuePairRun(samplePair);
    await vi.waitFor(() => expect(runApi.runPair).toHaveBeenCalled());

    await cancelPairRun("pair-1");

    expect(runApi.cancelRun).toHaveBeenCalledWith("pair-1");
    // The record stays active until the backend reports the run finished.
    expect(runOf("pair-1")?.status).toBe("running");
  });

  it("records a cancel failure without dropping the run", async () => {
    vi.mocked(runApi.runPair).mockImplementation(() => new Promise(() => {}));
    vi.mocked(runApi.cancelRun).mockRejectedValue(new Error("not running"));
    enqueuePairRun(samplePair);
    await vi.waitFor(() => expect(runApi.runPair).toHaveBeenCalled());

    await cancelPairRun("pair-1");

    expect(runOf("pair-1")?.status).toBe("running");
    expect(runOf("pair-1")?.error).toBe("not running");
  });

  it("cancels a manual start before it can call an unrelated backend run", async () => {
    enqueuePairRun(samplePair);
    await cancelPairRun(samplePair.id);
    await vi.waitFor(() =>
      expect(runOf(samplePair.id)?.status).toBe("cancelled"),
    );
    expect(runApi.cancelRun).not.toHaveBeenCalled();
    expect(runApi.runPair).not.toHaveBeenCalled();
  });

  it("starts the next pair after a failed run", async () => {
    vi.mocked(runApi.runPair)
      .mockRejectedValueOnce(new Error("disk full"))
      .mockResolvedValueOnce({ ...sampleReport, pairId: "pair-2" });

    enqueuePairRuns([samplePair, secondPair]);
    await vi.waitFor(() => expect(isQueueActive(getRunState())).toBe(false));

    expect(runOf("pair-1")?.status).toBe("failed");
    expect(runOf("pair-2")?.status).toBe("completed");
  });

  it("dismisses a settled run record", async () => {
    vi.mocked(runApi.runPair).mockResolvedValue(sampleReport);
    await runSelectedPair(samplePair);

    dismissPairRun("pair-1");
    expect(runOf("pair-1")).toBeNull();
  });

  it("stores watch-skipped events from the backend", async () => {
    await ensureWatchSkippedListener();
    listenHandlers["sync://watch-skipped"]?.({
      payload: {
        pairId: "pair-1",
        reason: "conflicts require manual resolution",
      },
    });
    expect(getRunState().watchSkipped).toEqual({
      pairId: "pair-1",
      reason: "conflicts require manual resolution",
    });
    dismissWatchSkipped();
    expect(getRunState().watchSkipped).toBeNull();
  });

  it("keeps a live Watch health event when the initial request resolves later", async () => {
    let finishHealth!: (
      health: Awaited<ReturnType<typeof pairsApi.getWatchHealth>>,
    ) => void;
    vi.spyOn(pairsApi, "getWatchHealth").mockImplementation(
      () => new Promise((resolve) => (finishHealth = resolve)),
    );

    const listening = ensureWatchHealthListener();
    await vi.waitFor(() => expect(finishHealth).toBeDefined());
    const watching = { pairId: samplePair.id, status: "watching", issues: [] };
    listenHandlers["watch://health"]({ payload: [watching] });
    finishHealth([
      {
        pairId: samplePair.id,
        status: "degraded",
        issues: ["Right unavailable"],
      },
    ]);
    await listening;

    expect(getRunState().watchHealth[samplePair.id]).toEqual(watching);
  });

  it("ignores progress when no UI run is active", async () => {
    vi.mocked(runApi.runPair).mockResolvedValue(sampleReport);
    await runSelectedPair(samplePair);

    listenHandlers["sync://progress"]?.({
      payload: {
        runId: "bg-run",
        pairId: "pair-1",
        phase: "running",
        current: 1,
        total: 10,
      },
    });

    expect(runOf("pair-1")?.progress).toBeNull();
  });

  it("routes progress to the pair it belongs to", async () => {
    let resolveRun: (report: RunReport) => void = () => {};
    const runDeferred = new Promise<RunReport>((resolve) => {
      resolveRun = resolve;
    });
    vi.mocked(runApi.runPair).mockReturnValue(runDeferred);

    const runPromise = runSelectedPair(samplePair);
    await vi.waitFor(() =>
      expect(listenHandlers["sync://progress"]).toBeTruthy(),
    );

    listenHandlers["sync://progress"]?.({
      payload: {
        runId: "run-1",
        pairId: "other-pair",
        phase: "running",
        current: 1,
        total: 5,
      },
    });
    expect(runOf("pair-1")?.progress).toBeNull();

    listenHandlers["sync://progress"]?.({
      payload: {
        runId: "run-1",
        pairId: "pair-1",
        phase: "running",
        current: 2,
        total: 5,
      },
    });
    expect(runOf("pair-1")?.progress?.current).toBe(2);

    resolveRun(sampleReport);
    await runPromise;
  });

  it("ignores a second enqueue for a pair already queued", async () => {
    vi.mocked(runApi.runPair).mockImplementation(() => new Promise(() => {}));

    expect(enqueuePairRun(samplePair)).toBe(true);
    expect(enqueuePairRun(samplePair)).toBe(false);
    expect(isPairBusy(getRunState(), "pair-1")).toBe(true);
    await vi.waitFor(() => expect(runApi.runPair).toHaveBeenCalledTimes(1));
  });

  it("runs one scan at a time and never alongside a sync", async () => {
    let resolvePreview!: (plan: typeof conflictPlan) => void;
    vi.mocked(previewApi.previewPair).mockImplementation(
      () => new Promise((resolve) => (resolvePreview = resolve)),
    );
    vi.mocked(runApi.runPair).mockResolvedValue(sampleReport);

    expect(enqueuePairPreview(samplePair)).toBe(true);
    expect(enqueuePairPreview(secondPair)).toBe(true);
    enqueuePairRun(samplePair);

    // Only the first scan is in flight; the second scan and the sync wait.
    await vi.waitFor(() =>
      expect(previewApi.previewPair).toHaveBeenCalledTimes(1),
    );
    expect(getRunState().activeKind).toBe("preview");
    expect(queueIds()).toEqual(["pair-2", "pair-1"]);
    expect(runApi.runPair).not.toHaveBeenCalled();

    resolvePreview({ ...conflictPlan, actions: [] });
    await vi.waitFor(() =>
      expect(previewApi.previewPair).toHaveBeenCalledTimes(2),
    );
    // Still no sync — the second scan holds the queue.
    expect(runApi.runPair).not.toHaveBeenCalled();
  });

  it("refuses to queue the same scan twice", () => {
    vi.mocked(previewApi.previewPair).mockImplementation(
      () => new Promise(() => {}),
    );

    expect(enqueuePairPreview(samplePair)).toBe(true);
    expect(enqueuePairPreview(samplePair)).toBe(false);
  });

  it("drops a waiting scan without calling the backend", async () => {
    vi.mocked(previewApi.previewPair).mockImplementation(
      () => new Promise(() => {}),
    );
    enqueuePairPreview(samplePair);
    enqueuePairPreview(secondPair);

    await cancelPairPreview("pair-2");

    expect(queueIds()).toEqual([]);
    expect(previewApi.cancelPreview).not.toHaveBeenCalled();
  });

  it("stops a running scan through the backend", async () => {
    vi.mocked(previewApi.previewPair).mockImplementation(
      () => new Promise(() => {}),
    );
    vi.mocked(previewApi.cancelPreview).mockResolvedValue(undefined);
    enqueuePairPreview(samplePair);
    await vi.waitFor(() => expect(previewApi.previewPair).toHaveBeenCalled());

    await cancelPairPreview("pair-1");

    expect(previewApi.cancelPreview).toHaveBeenCalledWith("pair-1");
  });

  it("opens the conflict dialog and holds the queue for an ask pair", async () => {
    vi.mocked(previewApi.previewPair).mockResolvedValue(conflictPlan);
    vi.mocked(runApi.runPair).mockImplementation(() => new Promise(() => {}));

    const askPair = {
      ...samplePair,
      mode: "synchronize" as const,
      conflictPolicy: "ask" as const,
    };
    enqueuePairRun(askPair);
    enqueuePairRun(secondPair);
    await vi.waitFor(() =>
      expect(getRunState().pendingConflicts?.conflicts).toHaveLength(1),
    );

    expect(runOf("pair-1")?.status).toBe("awaitingInput");
    // The queue does not advance past a pair waiting on the user.
    expect(runApi.runPair).not.toHaveBeenCalled();
    expect(queueIds()).toEqual(["pair-2"]);
  });

  it("runs with user conflict resolutions, then continues the queue", async () => {
    vi.mocked(previewApi.previewPair).mockResolvedValue(conflictPlan);
    vi.mocked(runApi.runPair).mockResolvedValue(sampleReport);

    enqueuePairRun({
      ...samplePair,
      mode: "synchronize",
      conflictPolicy: "ask",
    });
    enqueuePairRun(secondPair);
    await vi.waitFor(() => expect(getRunState().pendingConflicts).toBeTruthy());

    setConflictResolution("both.txt", "left");
    const report = await confirmConflictResolutionAndRun();

    expect(report).toEqual(sampleReport);
    expect(runApi.runPair).toHaveBeenCalledWith(
      expect.objectContaining({ id: "pair-1" }),
      expect.objectContaining({ conflictResolutions: { "both.txt": "left" } }),
    );
    await vi.waitFor(() => expect(runOf("pair-2")?.status).toBe("completed"));
  });

  it("cancelling the conflict dialog releases the queue", async () => {
    vi.mocked(previewApi.previewPair).mockResolvedValue(conflictPlan);
    vi.mocked(runApi.runPair).mockResolvedValue({
      ...sampleReport,
      pairId: "pair-2",
    });

    enqueuePairRun({
      ...samplePair,
      mode: "synchronize",
      conflictPolicy: "ask",
    });
    enqueuePairRun(secondPair);
    await vi.waitFor(() => expect(getRunState().pendingConflicts).toBeTruthy());

    cancelConflictResolution();

    expect(runOf("pair-1")?.status).toBe("cancelled");
    await vi.waitFor(() => expect(runOf("pair-2")?.status).toBe("completed"));
  });

  it("requires a new preview instead of executing with an expired plan", async () => {
    const previewSpy = vi.spyOn(pairsStore, "getPairPreview").mockReturnValue({
      ...pairsStore.emptyPairPreview,
      plan: { ...conflictPlan, planId: "plan-stale", actions: [] },
      queued: false,
      loading: false,
      error: null,
    });
    vi.mocked(runApi.runPair).mockRejectedValueOnce(
      new Error("Preview needs refreshing: preview plan expired or not found"),
    );

    const report = await runSelectedPair(samplePair);

    expect(report).toBeNull();
    expect(runOf("pair-1")?.status).toBe("failed");
    expect(runApi.runPair).toHaveBeenCalledExactlyOnceWith(
      samplePair,
      expect.objectContaining({
        intent: { kind: "reviewed", planId: "plan-stale" },
      }),
    );
    expect(previewApi.previewPair).not.toHaveBeenCalled();
    previewSpy.mockRestore();
    expect(pairsStore.getPairPreview(samplePair.id).reviewRequired).toBe(true);
  });

  it("holds a replacement preview until its results have been reviewed", async () => {
    pairsStore.requireFreshPreview(
      samplePair.id,
      "Preview needs refreshing: changed",
    );
    vi.mocked(previewApi.previewPair).mockResolvedValue(conflictPlan);
    vi.mocked(runApi.runPair).mockResolvedValue(sampleReport);
    expect(enqueuePairRun(samplePair)).toBe(false);
    expect(enqueuePairPreview(samplePair)).toBe(true);
    await vi.waitFor(() =>
      expect(pairsStore.getPairPreview(samplePair.id).plan).not.toBeNull(),
    );
    expect(enqueuePairRun(samplePair)).toBe(false);
    pairsStore.markPreviewReviewed(samplePair.id, conflictPlan.planId);
    await runSelectedPair(samplePair);
    expect(runApi.runPair).toHaveBeenCalledExactlyOnceWith(
      samplePair,
      expect.objectContaining({
        intent: { kind: "reviewed", planId: conflictPlan.planId },
      }),
    );
  });

  it("cancels a pair that is still scanning before its backend run exists", async () => {
    let resolvePreview!: (plan: typeof conflictPlan) => void;
    vi.mocked(previewApi.previewPair).mockImplementation(
      () => new Promise((resolve) => (resolvePreview = resolve)),
    );
    vi.mocked(runApi.runPair).mockResolvedValue(sampleReport);

    enqueuePairRun({
      ...samplePair,
      mode: "synchronize",
      conflictPolicy: "ask",
    });
    await vi.waitFor(() => expect(previewApi.previewPair).toHaveBeenCalled());

    await cancelPairRun("pair-1");
    // cancel_run would error (or hit an unrelated background run) — never call it.
    expect(runApi.cancelRun).not.toHaveBeenCalled();

    resolvePreview({ ...conflictPlan, actions: [] });
    await vi.waitFor(() => expect(runOf("pair-1")?.status).toBe("cancelled"));
    // The sync must not run after the user cancelled.
    expect(runApi.runPair).not.toHaveBeenCalled();
  });

  it("requeues a pair whose slot is held by a background sync", async () => {
    vi.useFakeTimers();
    try {
      vi.mocked(runApi.runPair)
        .mockRejectedValueOnce(
          new Error("sync run already in progress for pair pair-1"),
        )
        .mockResolvedValueOnce(sampleReport);

      enqueuePairRun(samplePair);
      await vi.advanceTimersByTimeAsync(2000);

      // The pair went back to the queue and started again instead of failing.
      await vi.waitFor(() => expect(runOf("pair-1")?.status).toBe("completed"));
      expect(runApi.runPair).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps the accepted conflict choices and recycle option during a busy retry", async () => {
    vi.useFakeTimers();
    try {
      vi.mocked(previewApi.previewPair).mockResolvedValue(conflictPlan);
      vi.mocked(runApi.runPair)
        .mockRejectedValueOnce(new Error("sync run already in progress"))
        .mockResolvedValueOnce(sampleReport);
      const pair = { ...samplePair, conflictPolicy: "ask" as const };
      updateAppSettings({ moveDeletesToRecycleBin: true });
      enqueuePairRun(pair);
      await vi.advanceTimersByTimeAsync(0);
      expect(getRunState().pendingConflicts?.planId).toBe(conflictPlan.planId);
      setConflictResolution("both.txt", "left");
      updateAppSettings({ moveDeletesToRecycleBin: false });
      await confirmConflictResolutionAndRun();
      await vi.advanceTimersByTimeAsync(1500);
      const options = {
        intent: { kind: "reviewed", planId: conflictPlan.planId },
        conflictResolutions: { "both.txt": "left" },
        useRecycleBin: true,
      };
      expect(runApi.runPair).toHaveBeenNthCalledWith(
        1,
        pair,
        expect.objectContaining(options),
      );
      expect(runApi.runPair).toHaveBeenNthCalledWith(
        2,
        pair,
        expect.objectContaining(options),
      );
      expect(previewApi.previewPair).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("asks once when queueing a batch", () => {
    vi.mocked(runApi.runPair).mockImplementation(() => new Promise(() => {}));
    updateAppSettings({ confirmBeforeRun: true });

    expect(enqueuePairRuns([samplePair, secondPair])).toBe(2);
    expect(window.confirm).toHaveBeenCalledTimes(1);
  });

  it("marks a run cancelled when the backend reports a cancelled report", async () => {
    vi.mocked(runApi.runPair).mockResolvedValue({
      ...sampleReport,
      status: "cancelled",
    });
    await runSelectedPair(samplePair);
    expect(runOf("pair-1")?.status).toBe("cancelled");
  });
});
