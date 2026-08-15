import { beforeEach, describe, expect, it, vi } from "vitest";
import * as pairsApi from "../api/pairs";
import * as previewApi from "../api/preview";
import type { FolderPair, SyncPlan } from "../types";
import {
  beginEdit,
  cancelEdit,
  clearPairPreview,
  getPairPreview,
  getPairsState,
  loadPairs,
  previewPairById,
  resetPairsStoreForTests,
  saveEditing,
  selectPair,
  startNewPair,
  updateEditing,
} from "../store/pairsStore";

vi.mock("../api/pairs", () => ({
  listPairs: vi.fn(),
  savePair: vi.fn(),
  deletePair: vi.fn(),
  setSchedule: vi.fn(),
  pickFolder: vi.fn(),
  pathExists: vi.fn(),
  pathsEqual: vi.fn(),
}));

vi.mock("../api/preview", () => ({
  previewPair: vi.fn(),
}));

const samplePair: FolderPair = {
  id: "pair-1",
  name: "Docs",
  leftPath: "C:\\left",
  rightPath: "D:\\right",
  mode: "echo",
  filters: { include: ["*.txt"], exclude: [] },
  conflictPolicy: "newerWins",
  enabled: true,
  watchEnabled: false,
  scheduleEnabled: false,
  scheduleCron: null,
  createdAt: 1,
  updatedAt: 2,
};

describe("pairsStore", () => {
  beforeEach(() => {
    resetPairsStoreForTests();
    vi.clearAllMocks();
  });

  it("loads pairs from the backend", async () => {
    vi.mocked(pairsApi.listPairs).mockResolvedValue([samplePair]);
    await loadPairs();
    expect(getPairsState().pairs).toEqual([samplePair]);
    expect(getPairsState().loading).toBe(false);
  });

  it("surfaces load errors", async () => {
    vi.mocked(pairsApi.listPairs).mockRejectedValue(new Error("db offline"));
    await loadPairs();
    expect(getPairsState().error).toBe("db offline");
  });

  it("starts a new empty draft", () => {
    startNewPair();
    const { editing } = getPairsState();
    expect(editing).not.toBeNull();
    expect(editing?.id).toBe("");
    expect(editing?.name).toBe("");
    expect(getPairsState().editorOpen).toBe(true);
  });

  it("selects a pair for editing", async () => {
    vi.mocked(pairsApi.listPairs).mockResolvedValue([samplePair]);
    await loadPairs();
    selectPair("pair-1");
    expect(getPairsState().selectedId).toBe("pair-1");
    expect(getPairsState().editing?.name).toBe("Docs");
    expect(getPairsState().editorOpen).toBe(false);

    beginEdit();
    expect(getPairsState().editorOpen).toBe(true);
  });

  it("validates before save", async () => {
    startNewPair();
    updateEditing({
      name: "",
      leftPath: "C:\\missing",
      rightPath: "D:\\missing",
    });
    vi.mocked(pairsApi.pathExists).mockResolvedValue(false);
    vi.mocked(pairsApi.pathsEqual).mockResolvedValue(false);

    const ok = await saveEditing();
    expect(ok).toBe(false);
    expect(pairsApi.savePair).not.toHaveBeenCalled();
    expect(getPairsState().validationErrors.length).toBeGreaterThan(0);
  });

  it("warns when watch is enabled but folder paths are missing on disk", async () => {
    vi.mocked(pairsApi.listPairs).mockResolvedValue([
      { ...samplePair, watchEnabled: true },
    ]);
    vi.mocked(pairsApi.pathExists).mockResolvedValue(false);
    await loadPairs();
    selectPair("pair-1");
    await vi.waitFor(() => {
      expect(getPairsState().watchWarning).toContain("inactive");
    });
  });

  it("clears watch warning after save when both paths exist", async () => {
    startNewPair();
    updateEditing({
      name: "Backup",
      leftPath: "C:\\left",
      rightPath: "D:\\right",
      watchEnabled: true,
    });
    vi.mocked(pairsApi.pathExists).mockResolvedValue(true);
    vi.mocked(pairsApi.pathsEqual).mockResolvedValue(false);
    vi.mocked(pairsApi.savePair).mockResolvedValue({
      ...samplePair,
      id: "new-id",
      name: "Backup",
      watchEnabled: true,
    });
    vi.mocked(pairsApi.setSchedule).mockResolvedValue({
      ...samplePair,
      id: "new-id",
      name: "Backup",
      watchEnabled: true,
    });

    const ok = await saveEditing();
    expect(ok).toBe(true);
    expect(getPairsState().watchWarning).toBeNull();
  });

  it("saves a valid pair", async () => {
    startNewPair();
    updateEditing({
      name: "Backup",
      leftPath: "C:\\left",
      rightPath: "D:\\right",
    });
    vi.mocked(pairsApi.pathExists).mockResolvedValue(true);
    vi.mocked(pairsApi.pathsEqual).mockResolvedValue(false);
    vi.mocked(pairsApi.savePair).mockResolvedValue({
      ...samplePair,
      id: "new-id",
      name: "Backup",
    });
    vi.mocked(pairsApi.setSchedule).mockResolvedValue({
      ...samplePair,
      id: "new-id",
      name: "Backup",
    });

    const ok = await saveEditing();
    expect(ok).toBe(true);
    expect(pairsApi.savePair).toHaveBeenCalled();
    expect(getPairsState().pairs).toHaveLength(1);
    expect(getPairsState().selectedId).toBe("new-id");
  });

  it("loads preview for selected pair", async () => {
    const plan: SyncPlan = {
      pairId: "pair-1",
      scannedLeft: 1,
      scannedRight: 1,
      actions: [{ kind: "copyLeftToRight", path: "a.txt" }],
    };
    vi.mocked(pairsApi.listPairs).mockResolvedValue([samplePair]);
    vi.mocked(previewApi.previewPair).mockResolvedValue(plan);
    await loadPairs();
    selectPair("pair-1");
    updateEditing({ mode: "synchronize" });
    await previewPairById(getPairsState().editing ?? samplePair);
    expect(previewApi.previewPair).toHaveBeenCalledWith({
      ...samplePair,
      mode: "synchronize",
    });
    expect(getPairPreview("pair-1").plan).toEqual(plan);
    expect(getPairPreview("pair-1").loading).toBe(false);
    expect(getPairPreview("pair-1").error).toBeNull();
  });

  it("keeps preview results attached to their own pair", async () => {
    const pair2: FolderPair = { ...samplePair, id: "pair-2", name: "Other" };
    vi.mocked(pairsApi.listPairs).mockResolvedValue([samplePair, pair2]);
    let resolvePreview!: (plan: SyncPlan) => void;
    vi.mocked(previewApi.previewPair).mockImplementation(
      () =>
        new Promise((resolve) => {
          resolvePreview = resolve;
        }),
    );

    await loadPairs();
    selectPair("pair-1");
    const previewPromise = previewPairById(samplePair);
    await vi.waitFor(() => expect(previewApi.previewPair).toHaveBeenCalled());
    selectPair("pair-2");

    const plan: SyncPlan = {
      pairId: "pair-1",
      scannedLeft: 1,
      scannedRight: 1,
      actions: [],
    };
    resolvePreview(plan);
    await previewPromise;

    // The late result belongs to pair-1 and must not leak into pair-2's view.
    expect(getPairPreview("pair-1").plan).toEqual(plan);
    expect(getPairPreview("pair-1").loading).toBe(false);
    expect(getPairPreview("pair-2").plan).toBeNull();
  });

  it("drops a superseded preview for the same pair", async () => {
    vi.mocked(pairsApi.listPairs).mockResolvedValue([samplePair]);
    const resolvers: ((plan: SyncPlan) => void)[] = [];
    vi.mocked(previewApi.previewPair).mockImplementation(
      () => new Promise((resolve) => resolvers.push(resolve)),
    );

    await loadPairs();
    selectPair("pair-1");
    const first = previewPairById(samplePair);
    const second = previewPairById(samplePair);
    await vi.waitFor(() => expect(resolvers).toHaveLength(2));

    resolvers[1]({
      pairId: "pair-1",
      scannedLeft: 2,
      scannedRight: 2,
      actions: [],
    });
    resolvers[0]({
      pairId: "pair-1",
      scannedLeft: 1,
      scannedRight: 1,
      actions: [{ kind: "copyLeftToRight", path: "stale.txt" }],
    });
    await Promise.all([first, second]);

    expect(getPairPreview("pair-1").plan?.scannedLeft).toBe(2);
  });

  it("keeps a running preview attached to the pair after cancelEdit", async () => {
    vi.mocked(pairsApi.listPairs).mockResolvedValue([samplePair]);
    vi.mocked(previewApi.previewPair).mockImplementation(
      () => new Promise(() => {}),
    );
    await loadPairs();
    selectPair("pair-1");
    void previewPairById(samplePair);
    expect(getPairPreview("pair-1").loading).toBe(true);

    cancelEdit();
    expect(getPairPreview("pair-1").loading).toBe(true);
    expect(getPairsState().editing).toBeNull();
  });

  it("clears a pair's cached preview once a run consumes it", async () => {
    const plan: SyncPlan = {
      pairId: "pair-1",
      scannedLeft: 1,
      scannedRight: 1,
      actions: [],
    };
    vi.mocked(pairsApi.listPairs).mockResolvedValue([samplePair]);
    vi.mocked(previewApi.previewPair).mockResolvedValue(plan);
    await loadPairs();
    selectPair("pair-1");
    await previewPairById(getPairsState().editing ?? samplePair);
    expect(getPairPreview("pair-1").plan).toEqual(plan);

    clearPairPreview("pair-1");
    expect(getPairPreview("pair-1").plan).toBeNull();
  });

  it("debounces path-exists checks for watch warning", async () => {
    vi.useFakeTimers();
    vi.mocked(pairsApi.pathExists).mockResolvedValue(true);

    startNewPair();
    updateEditing({
      name: "Watch",
      leftPath: "C:\\left",
      rightPath: "D:\\right",
      watchEnabled: true,
    });
    updateEditing({ leftPath: "C:\\left2" });
    updateEditing({ leftPath: "C:\\left3" });

    expect(pairsApi.pathExists).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(300);
    await vi.waitFor(() => {
      expect(pairsApi.pathExists).toHaveBeenCalledTimes(2);
    });
    vi.useRealTimers();
  });

  it("surfaces preview errors", async () => {
    vi.mocked(pairsApi.listPairs).mockResolvedValue([samplePair]);
    vi.mocked(previewApi.previewPair).mockRejectedValue(
      new Error("scan left failed"),
    );
    await loadPairs();
    selectPair("pair-1");
    await previewPairById(getPairsState().editing ?? samplePair);
    expect(getPairPreview("pair-1").error).toBe("scan left failed");
    expect(getPairPreview("pair-1").plan).toBeNull();
  });

  it("rejects identical paths", async () => {
    startNewPair();
    updateEditing({
      name: "Same",
      leftPath: "C:\\folder",
      rightPath: "c:\\folder",
    });
    vi.mocked(pairsApi.pathExists).mockResolvedValue(true);
    vi.mocked(pairsApi.pathsEqual).mockResolvedValue(true);

    const ok = await saveEditing();
    expect(ok).toBe(false);
    expect(getPairsState().validationErrors).toContain(
      "Left and right folders must be different",
    );
  });
});
