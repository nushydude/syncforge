import { beforeEach, describe, expect, it, vi } from "vitest";
import * as previewApi from "../api/preview";
import * as runApi from "../api/run";
import type { FolderPair, RunReport } from "../types";
import {
  cancelActiveRun,
  confirmConflictResolutionAndRun,
  dismissWatchSkipped,
  ensureWatchSkippedListener,
  getRunState,
  resetRunStoreForTests,
  runSelectedPair,
  setConflictResolution,
} from "../store/runStore";
import * as pairsStore from "../store/pairsStore";

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
  watchEnabled: false,
  scheduleEnabled: false,
  scheduleCron: null,
  createdAt: 1,
  updatedAt: 2,
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

describe("runStore", () => {
  beforeEach(() => {
    resetRunStoreForTests();
    vi.clearAllMocks();
    vi.restoreAllMocks();
    vi.spyOn(window, "confirm").mockReturnValue(true);
    for (const key of Object.keys(listenHandlers)) {
      delete listenHandlers[key];
    }
  });

  it("runs a pair and stores the report", async () => {
    vi.mocked(runApi.runPair).mockResolvedValue(sampleReport);
    const report = await runSelectedPair(samplePair);
    expect(report).toEqual(sampleReport);
    expect(getRunState().running).toBe(false);
    expect(getRunState().lastReport).toEqual(sampleReport);
  });

  it("surfaces run errors", async () => {
    vi.mocked(runApi.runPair).mockRejectedValue(new Error("disk full"));
    const report = await runSelectedPair(samplePair);
    expect(report).toBeNull();
    expect(getRunState().error).toBe("disk full");
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

  it("opens conflict dialog when ask policy finds conflicts", async () => {
    vi.mocked(previewApi.previewPair).mockResolvedValue({
      pairId: "pair-1",
      scannedLeft: 1,
      scannedRight: 1,
      actions: [
        {
          kind: "conflict",
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
    });

    const report = await runSelectedPair({
      ...samplePair,
      mode: "synchronize",
      conflictPolicy: "ask",
    });
    expect(report).toBeNull();
    expect(runApi.runPair).not.toHaveBeenCalled();
    expect(getRunState().pendingConflicts?.conflicts).toHaveLength(1);
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

    expect(getRunState().progress).toBeNull();
  });

  it("ignores progress for a different pair during UI run", async () => {
    let resolveRun: (report: RunReport) => void = () => {};
    const runDeferred = new Promise<RunReport>((resolve) => {
      resolveRun = resolve;
    });
    vi.mocked(runApi.runPair).mockReturnValue(runDeferred);

    const runPromise = runSelectedPair(samplePair);
    await vi.waitFor(() => getRunState().running);
    await vi.waitFor(() => listenHandlers["sync://progress"]);

    listenHandlers["sync://progress"]?.({
      payload: {
        runId: "run-1",
        pairId: "other-pair",
        phase: "running",
        current: 1,
        total: 5,
      },
    });
    expect(getRunState().progress).toBeNull();

    listenHandlers["sync://progress"]?.({
      payload: {
        runId: "run-1",
        pairId: "pair-1",
        phase: "running",
        current: 2,
        total: 5,
      },
    });
    expect(getRunState().progress?.current).toBe(2);

    resolveRun(sampleReport);
    await runPromise;
  });

  it("clears running state optimistically and recovers on cancel error", async () => {
    vi.mocked(runApi.runPair).mockImplementation(() => new Promise(() => {}));
    vi.mocked(runApi.cancelRun).mockRejectedValue(new Error("not running"));

    void runSelectedPair(samplePair);
    await vi.waitFor(() => getRunState().running);

    await cancelActiveRun();
    expect(getRunState().running).toBe(true);
    expect(getRunState().error).toBe("not running");
  });

  it("ignores duplicate runSelectedPair while conflicts are pending", async () => {
    vi.mocked(previewApi.previewPair).mockResolvedValue({
      pairId: "pair-1",
      scannedLeft: 1,
      scannedRight: 1,
      actions: [
        {
          kind: "conflict",
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
    });

    const askPair = {
      ...samplePair,
      mode: "synchronize" as const,
      conflictPolicy: "ask" as const,
    };

    await runSelectedPair(askPair);
    expect(getRunState().pendingConflicts?.conflicts).toHaveLength(1);

    await runSelectedPair(askPair);
    expect(previewApi.previewPair).toHaveBeenCalledTimes(1);
    expect(runApi.runPair).not.toHaveBeenCalled();
  });

  it("ignores duplicate runSelectedPair while in flight", async () => {
    vi.mocked(runApi.runPair).mockImplementation(() => new Promise(() => {}));

    void runSelectedPair(samplePair);
    await vi.waitFor(() => getRunState().running);
    await runSelectedPair(samplePair);

    expect(runApi.runPair).toHaveBeenCalledTimes(1);
  });

  it("blocks run while preview is loading", async () => {
    vi.spyOn(pairsStore, "isPreviewLoading").mockReturnValue(true);

    const report = await runSelectedPair(samplePair);
    expect(report).toBeNull();
    expect(runApi.runPair).not.toHaveBeenCalled();
  });

  it("ignores duplicate confirmConflictResolutionAndRun while running", async () => {
    vi.mocked(previewApi.previewPair).mockResolvedValue({
      pairId: "pair-1",
      scannedLeft: 1,
      scannedRight: 1,
      actions: [
        {
          kind: "conflict",
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
    });
    let resolveRun: (report: RunReport) => void = () => {};
    const runDeferred = new Promise<RunReport>((resolve) => {
      resolveRun = resolve;
    });
    vi.mocked(runApi.runPair).mockReturnValue(runDeferred);

    await runSelectedPair({
      ...samplePair,
      mode: "synchronize",
      conflictPolicy: "ask",
    });
    setConflictResolution("both.txt", "left");

    void confirmConflictResolutionAndRun();
    await vi.waitFor(() => getRunState().running);

    const report = await confirmConflictResolutionAndRun();
    expect(report).toBeNull();
    expect(runApi.runPair).toHaveBeenCalledTimes(1);

    resolveRun(sampleReport);
  });

  it("runs with user conflict resolutions after confirm", async () => {
    vi.mocked(previewApi.previewPair).mockResolvedValue({
      pairId: "pair-1",
      scannedLeft: 1,
      scannedRight: 1,
      actions: [
        {
          kind: "conflict",
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
    });
    vi.mocked(runApi.runPair).mockResolvedValue(sampleReport);

    await runSelectedPair({
      ...samplePair,
      mode: "synchronize",
      conflictPolicy: "ask",
    });
    setConflictResolution("both.txt", "left");
    const report = await confirmConflictResolutionAndRun();

    expect(report).toEqual(sampleReport);
    expect(runApi.runPair).toHaveBeenCalledWith(
      expect.objectContaining({ id: "pair-1" }),
      expect.objectContaining({
        conflictResolutions: { "both.txt": "left" },
      }),
    );
  });
});
